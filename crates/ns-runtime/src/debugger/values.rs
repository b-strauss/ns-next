//! The values a stop hands out (`docs/design/debugger.md`, "Values the IDE
//! holds"): each held as a `Root` in the stop's table and named to the
//! agent by a `ValueId`, described by the heap's accessors alone -- no
//! Newspeak code runs while the program is stopped.

use std::collections::HashMap;

use super::remote::{ValueDescription, ValueId, Variable, VariableValue};
use crate::heap::{MixinId, ObjectReference, ObjectShape};
use crate::interpreter::Mutator;
use crate::mixin::FieldName;
use crate::{Root, Symbol, Value, ValueKind};

/// The values one stop handed out, by id. Its ids continue those of the
/// stop before (`first`), so that an id of an earlier stop, whose table
/// is gone, names nothing here rather than another value; the table goes
/// with its stop, releasing every `Root`.
pub(crate) struct StopValues {
    /// The id of the table's first value.
    first: u32,
    roots: Vec<Root>,
}

impl StopValues {
    /// An empty table whose first id is `first`.
    pub(crate) fn starting_at(first: u32) -> StopValues {
        StopValues {
            first,
            roots: Vec::new(),
        }
    }

    /// The id the next stop's table starts at: past every id this one
    /// handed out.
    /// Ids wrap only after 2^32 values handed out across the stops of
    /// one attachment, far past what an agent asks for.
    pub(crate) fn next_first(&self) -> u32 {
        let length = u32::try_from(self.roots.len()).unwrap_or(u32::MAX);
        self.first.wrapping_add(length)
    }

    /// `value`, held for the stop and described for the agent.
    pub(crate) fn describe<'gc>(
        &mut self,
        mutator: &Mutator<'gc>,
        value: Value<'gc>,
    ) -> ValueDescription {
        let id = ValueId(self.next_first());
        self.roots.push(mutator.heap.stash(value));
        ValueDescription {
            id,
            label: mutator.heap.describe(value),
            class_name: mutator.heap.class_display_name(mutator.class_of(value)),
            child_count: child_count(mutator, value),
            indexed: is_indexed(mutator, value),
        }
    }

    /// The value `id` names, if this stop handed it out.
    fn fetch<'gc>(&self, mutator: &Mutator<'gc>, id: ValueId) -> Option<Value<'gc>> {
        let index = usize::try_from(id.0.wrapping_sub(self.first)).ok()?;
        self.roots.get(index).map(|root| mutator.heap.fetch(root))
    }

    /// `count` children of the value `id` names, from the `start`th: an
    /// instance's fields, an `Array`'s elements; `None` for an id this
    /// stop did not hand out.
    pub(crate) fn children<'gc>(
        &mut self,
        mutator: &Mutator<'gc>,
        id: ValueId,
        start: usize,
        count: usize,
    ) -> Option<Vec<Variable>> {
        let value = self.fetch(mutator, id)?;
        let total = child_count(mutator, value);
        let end = start.saturating_add(count).min(total);
        let ValueKind::Object(object) = value.kind() else {
            return Some(Vec::new());
        };
        let is_array = mutator.heap.object_shape(object) == ObjectShape::Array;
        let mut labels = if is_array {
            Vec::new()
        } else {
            field_labels(mutator, object, total)
        };
        let mut children = Vec::new();
        for index in start..end {
            let (name, child, filling) = if is_array {
                (
                    format!("[{}]", index + 1),
                    mutator.heap.array_element(object, index),
                    Filling::Always,
                )
            } else {
                let label = labels
                    .get_mut(index)
                    .map(std::mem::take)
                    .expect("internal: every field below the count is labelled");
                (
                    label.name,
                    mutator.heap.instance_field(object, index),
                    label.filling,
                )
            };
            let Some(child) = child else {
                unreachable!("internal: a child below the count is readable")
            };
            let value = if filling.is_unfilled(child) {
                VariableValue::NotComputed
            } else {
                VariableValue::Shown(self.describe(mutator, child))
            };
            children.push(Variable { name, value });
        }
        Some(children)
    }
}

/// How a field comes to hold its value, which says what its word means
/// before it does.
#[derive(Clone, Copy, Default)]
enum Filling {
    /// Filled by the initializer, or a word no declaration names: what it
    /// holds is its value, `nil` included.
    #[default]
    Always,
    /// A lazy slot (§6.3.3), filled by its getter's first send: `nil`
    /// until then, which the getter reads as unfilled
    /// (`Instruction::ReturnSlotIfFilled`).
    Lazy,
    /// A nested class's hidden slot, filled by its accessor: `nil` until
    /// then, and a creation serial while the class is being built
    /// (`interpreter/nested.rs`).
    NestedClass,
}

impl Filling {
    /// Whether `word` says the field holds no value yet.
    fn is_unfilled(self, word: Value<'_>) -> bool {
        match self {
            Filling::Always => false,
            Filling::Lazy => word.is_nil(),
            Filling::NestedClass => word.is_nil() || word.as_small_integer().is_some(),
        }
    }
}

/// What one field of an instance is called and how it is filled.
#[derive(Default)]
struct FieldLabel {
    name: String,
    filling: Filling,
}

/// The names of `object`'s `field_count` fields, from the layout of its
/// class's application -- `MixinApplication::field_layout`, the walk an
/// install copies fields by, so that the two never disagree: a slot is
/// named as it was declared, a nested class's hidden slot by the nested
/// class's name -- both structure, which an image keeps without its
/// source. A name declared on more than one level (a superclass's
/// private `x` under a subclass's `x`, a nested class a subclass declares
/// again) is the plain name only on the level nearest the object's class,
/// which the program's own code there reads; each level above it is
/// `x (Super)`, its declaring mixin's name added, and where one chain
/// applies that same mixin more than once, each further one up is
/// numbered, `x (Super 2)`, counted by the mixin's id. Two different
/// mixins that share a name, each shadowing the same name, are told apart
/// by their declarations' paths from their module roots, `x
/// (Outer.Super)` and `x (Other.Super)` (`qualifiers`); a mixin whose
/// name no other shares there keeps the short form. A field no
/// declaration names is `field <index>`: an instance whose class holds no
/// application, one with more fields than its application lays out, or
/// one whose layout names a mixin the heap holds no id for, which a query
/// shows rather than fail on (`MixinApplication::checked_field_layout`).
fn field_labels<'gc>(
    mutator: &Mutator<'gc>,
    object: ObjectReference<'gc>,
    field_count: usize,
) -> Vec<FieldLabel> {
    let heap = &mutator.heap;
    let layout = heap
        .class_application(heap.object_class(object))
        .and_then(|application| application.checked_field_layout(heap))
        .unwrap_or_default();
    let declared: Vec<Option<Declared>> = (0..field_count)
        .map(|index| {
            let field = layout.get(index)?;
            match field.name {
                FieldName::Slot { mixin, name, .. } => Some(Declared {
                    name,
                    declarer: mixin,
                    declarer_name: heap.mixin(mixin)?.name,
                    level: field.level,
                    filling: if field.lazy {
                        Filling::Lazy
                    } else {
                        Filling::Always
                    },
                }),
                FieldName::Nested { declaration, .. } => {
                    let declarer = heap.enclosing_mixin_id(declaration)?;
                    Some(Declared {
                        name: heap.mixin(declaration)?.name,
                        declarer,
                        declarer_name: heap.mixin(declarer)?.name,
                        level: field.level,
                        filling: Filling::NestedClass,
                    })
                }
            }
        })
        .collect();
    let mut nearest: HashMap<Symbol, usize> = HashMap::new();
    for field in declared.iter().flatten() {
        let level = nearest.entry(field.name).or_insert(field.level);
        *level = (*level).max(field.level);
    }
    let qualifiers = qualifiers(mutator, &declared, &nearest);
    // Numbered from the object's class upward, as the shadowed fields are
    // met reading up the chain; fields lie in level order, so that is the
    // reverse of field order.
    let mut shadowed: HashMap<(Symbol, MixinId), u32> = HashMap::new();
    let mut labels: Vec<FieldLabel> = Vec::with_capacity(field_count);
    for (index, field) in declared.iter().enumerate().rev() {
        let label = match field {
            None => FieldLabel {
                name: format!("field {index}"),
                filling: Filling::Always,
            },
            Some(field) if nearest.get(&field.name) == Some(&field.level) => FieldLabel {
                name: field.name.to_string(),
                filling: field.filling,
            },
            Some(field) => {
                let count = shadowed.entry((field.name, field.declarer)).or_insert(0);
                *count += 1;
                let declarer = qualifiers
                    .get(&(field.name, field.declarer))
                    .map_or_else(|| field.declarer_name.to_string(), Clone::clone);
                let name = match *count {
                    1 => format!("{} ({declarer})", field.name),
                    count => format!("{} ({declarer} {count})", field.name),
                };
                FieldLabel {
                    name,
                    filling: field.filling,
                }
            }
        };
        labels.push(label);
    }
    labels.reverse();
    labels
}

/// How each shadowed field's declarer is named in its label, by the
/// field's name and the declarer's id: its mixin's name, unless another
/// declarer of a field shadowed under the same name shares that name --
/// then its declaration's path from its module root, `Outer.Super`, and
/// where that is shared too, as by the roots of two modules of one name,
/// that path from the root's own name on, `Module.Outer.Super`. Where two
/// declarers still read alike -- two modules of one source, or one
/// declarer's path from its root that another's path from its own root's
/// name spells -- each of them takes its place among them in the chain,
/// counted from the nearest, as an ordinal, `Module.Outer.Super #2`, so
/// that no two labels are alike. A field of the nearest level shadows
/// nothing and has none.
fn qualifiers(
    mutator: &Mutator<'_>,
    declared: &[Option<Declared>],
    nearest: &HashMap<Symbol, usize>,
) -> HashMap<(Symbol, MixinId), String> {
    let mut declarers: HashMap<Symbol, Vec<(MixinId, Symbol)>> = HashMap::new();
    for field in declared.iter().flatten() {
        if nearest.get(&field.name) == Some(&field.level) {
            continue;
        }
        let ids = declarers.entry(field.name).or_default();
        if !ids.iter().any(|(id, _)| *id == field.declarer) {
            ids.push((field.declarer, field.declarer_name));
        }
    }
    // A module root's own path is empty: it is named by its name.
    let path = |id: MixinId, from_root: bool| {
        let (root, path) = mutator.path_from_module_root(id);
        let mut names: Vec<String> = path.iter().map(ToString::to_string).collect();
        if from_root || names.is_empty() {
            if let Some(root) = mutator.heap.mixin(root) {
                names.insert(0, root.name.to_string());
            }
        }
        names.join(".")
    };
    let mut qualifiers = HashMap::new();
    for (name, ids) in &declarers {
        let tiered: Vec<String> = ids
            .iter()
            .map(|(id, short)| {
                let sharing: Vec<MixinId> = ids
                    .iter()
                    .filter(|(other, other_short)| other != id && other_short == short)
                    .map(|(other, _)| *other)
                    .collect();
                if sharing.is_empty() {
                    return short.to_string();
                }
                let own = path(*id, false);
                if sharing.iter().all(|other| path(*other, false) != own) {
                    own
                } else {
                    path(*id, true)
                }
            })
            .collect();
        for (position, (id, _)) in ids.iter().enumerate() {
            let qualifier = &tiered[position];
            let alike: Vec<usize> = (0..tiered.len())
                .filter(|other| tiered[*other] == *qualifier)
                .collect();
            let qualifier = if alike.len() == 1 {
                qualifier.clone()
            } else {
                // The declarers lie in level order, the root's first, and
                // are numbered from the object's class upward, as a mixin
                // applied twice is.
                let ordinal = alike
                    .iter()
                    .position(|other| *other == position)
                    .map_or(0, |place| alike.len() - place);
                format!("{qualifier} #{ordinal}")
            };
            qualifiers.insert((*name, *id), qualifier);
        }
    }
    qualifiers
}

/// One field a declaration names: the name it was declared with, the id
/// and the name of the mixin of the level declaring it, that level's place
/// in the chain, counted from the root, and how the field is filled.
struct Declared {
    name: Symbol,
    declarer: MixinId,
    declarer_name: Symbol,
    level: usize,
    filling: Filling,
}

/// Whether `value`'s children are elements, counted from 1, rather than
/// fields: decided by its shape, as `children` names them, never by its
/// class's name, which a program's own class may share.
fn is_indexed<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> bool {
    let ValueKind::Object(object) = value.kind() else {
        return false;
    };
    mutator.heap.object_shape(object) == ObjectShape::Array
}

/// How many children `value` shows: an instance's fields, an `Array`'s
/// elements, none for anything else -- a reified mixin's one field is an
/// id, which tells a reader nothing (`Heap::describe`).
fn child_count<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> usize {
    let ValueKind::Object(object) = value.kind() else {
        return 0;
    };
    let count = match mutator.heap.object_shape(object) {
        ObjectShape::Instance => mutator.heap.instance_field_count(object),
        ObjectShape::Array => mutator.heap.array_element_count(object),
        _ => None,
    };
    count.unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::field_labels;
    use crate::mixin::{DispatchIds, LevelFacts, Mixin, MixinApplication};
    use crate::{Interpreter, Symbol};

    /// An instance whose class's application names a mixin the heap holds
    /// no id for -- a class no program can make, built by hand -- shows
    /// its fields by number: a debugger query never ends the program it
    /// inspects on a layout it cannot name.
    #[test]
    fn a_layout_naming_an_unregistered_mixin_numbers_the_fields() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let mixin = Rc::new(Mixin::empty(Symbol::intern("Unregistered")));
            let level_facts = LevelFacts::of_mixin(&mixin);
            let application = mutator.heap.register_application(MixinApplication {
                index: crate::heap::ApplicationIndex::UNREGISTERED,
                mixin,
                superclass: None,
                inherits_value: false,
                first_mutable_slot: None,
                instantiation_refusal: None,
                nested_slot: crate::hash::IdentityMap::default(),
                declared_slots: Vec::new(),
                slot_count: 0,
                own_slot_base: 0,
                dispatch_ids: DispatchIds::NONE,
                instance_header: None,
                level_facts,
            });
            let class = mutator
                .heap
                .allocate_self_referential_class(crate::heap::NewClass {
                    application,
                    enclosing: None,
                    declaring_class: None,
                    superclass: None,
                    builtin: None,
                });
            let object = mutator.heap.allocate_instance(class, 2);
            let names: Vec<String> = field_labels(mutator, object, 2)
                .into_iter()
                .map(|label| label.name)
                .collect();
            assert_eq!(names, ["field 0", "field 1"]);
        });
    }
}

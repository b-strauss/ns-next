//! The memory view (`docs/design/debugger.md`, "The memory view"): at a
//! stop, how many instances each class declaration has, a declaration's
//! instances, and the objects that refer to one, each answered by a walk
//! of the object space (`Heap::each_object`). No Newspeak code runs at a
//! stop and nothing collects, so the walk finds every object the space
//! holds -- those nothing reaches any more too -- and every object it
//! hands out is a value of the stop's, described as the variables view
//! describes one (`StopValues::describe`).

use std::collections::HashMap;
use std::rc::Rc;

use super::metadata::again::declaration_path;
use super::remote::{ClassName, InstanceCount, ModuleName, Variable, VariableValue};
use super::stack::is_named_register;
use super::values::StopValues;
use crate::Value;
use crate::heap::{ClassReference, ObjectReference};
use crate::interpreter::Mutator;
use crate::mixin::Mixin;

/// The declaration a class object is the class of, as the memory view
/// groups by it: by mixin, never by class object, of which there is one
/// per enclosing instance (`docs/design/object-model.md`, "Three layers,
/// not one").
#[derive(Clone)]
struct Declaration {
    /// Its name, as a step filter names it; `None` for a class side, whose
    /// declaration is its instance side's and whose instances are class
    /// objects, which the view does not count.
    name: Option<ClassName>,
    /// Whether a shipped module declares it: its instances are withheld.
    shipped: bool,
}

/// The declarations of the class objects one walk met, by the address of
/// the declaring mixin: every instance of one class asks the same
/// question, and every class object of one declaration answers alike.
#[derive(Default)]
struct Declarations {
    known: HashMap<*const Mixin, Option<Declaration>>,
}

impl Declarations {
    /// The declaration `class` is the class of (`Heap::class_declaration`),
    /// named as a step filter names one -- a builtin class, `String` or
    /// `Array`, under its kernel declaration -- and unnamed for a class
    /// side and a mixin no heap registered; `None` for a class object
    /// without an application.
    fn of_class<'gc>(
        &mut self,
        mutator: &Mutator<'gc>,
        class: ClassReference<'gc>,
    ) -> Option<Declaration> {
        let mixin = mutator.heap.class_declaration(class)?;
        self.known
            .entry(Rc::as_ptr(&mixin))
            .or_insert_with(|| {
                let name = if mixin.is_class_side() {
                    None
                } else {
                    declaration_path(mutator, &mixin).map(|(root, path)| ClassName {
                        module: ModuleName::of_root(&root),
                        path: path.iter().map(ToString::to_string).collect(),
                    })
                };
                Some(Declaration {
                    name,
                    shipped: mixin.shipped,
                })
            })
            .clone()
    }
}

/// The name of the declaration `object` is an instance of, as
/// `Declarations::of_class` names it, and whether a shipped module
/// declares it; `None` for an object the view does not count -- one of a
/// class side or of no named declaration, and the VM mirror, whose class
/// has no declaration and so no name (`docs/design/core-library.md`, "The
/// VM mirror is the whole Rust boundary"), though the mixin its class
/// applies has one.
fn declared<'gc>(
    mutator: &Mutator<'gc>,
    declarations: &mut Declarations,
    object: ObjectReference<'gc>,
) -> Option<(ClassName, bool)> {
    if mutator.is_vm_mirror(Value::object(object)) {
        return None;
    }
    let declaration = declarations.of_class(mutator, mutator.heap.object_class(object))?;
    Some((declaration.name?, declaration.shipped))
}

/// How many instances each class declaration has, one count per
/// declaration with instances, ordered by count, the largest first, then
/// by name (`Command::InstanceCounts`).
pub(crate) fn instance_counts(mutator: &Mutator<'_>) -> Vec<InstanceCount> {
    let mut declarations = Declarations::default();
    let mut counts: Vec<InstanceCount> = Vec::new();
    // Each count's index in `counts`, by its name's parts.
    let mut indices: HashMap<(ModuleName, Vec<String>), usize> = HashMap::new();
    mutator.heap.each_object(|object| {
        let Some((name, _)) = declared(mutator, &mut declarations, object) else {
            return;
        };
        let key = (name.module.clone(), name.path.clone());
        let index = *indices.entry(key).or_insert_with(|| {
            counts.push(InstanceCount {
                class: name,
                count: 0,
            });
            counts.len() - 1
        });
        counts[index].count += 1;
    });
    counts.sort_by(|first, second| {
        second.count.cmp(&first.count).then_with(|| {
            (&first.class.module, &first.class.path)
                .cmp(&(&second.class.module, &second.class.path))
        })
    });
    counts
}

/// `count` of the instances of the declaration `class` names, from the
/// `start`th, each named `#<n>`, counted from 1 over all of them
/// (`Command::Instances`): grouped by application, in the order the walk
/// first meets each, and within one in the space's order. An instance of
/// a declaration a shipped module declares is withheld. Empty for a
/// name no declaration with instances has.
pub(crate) fn instances<'gc>(
    mutator: &Mutator<'gc>,
    values: &mut StopValues,
    class: &ClassName,
    start: usize,
    count: usize,
) -> Vec<Variable> {
    let mut declarations = Declarations::default();
    let mut shipped = false;
    // The instances found, by application, in the order first met.
    let mut groups: Vec<(Option<usize>, Vec<ObjectReference<'gc>>)> = Vec::new();
    mutator.heap.each_object(|object| {
        let Some((name, declared_shipped)) = declared(mutator, &mut declarations, object) else {
            return;
        };
        if name != *class {
            return;
        }
        shipped = declared_shipped;
        let application = mutator
            .heap
            .class_application_index(mutator.heap.object_class(object))
            .map(|index| index.index());
        match groups.iter_mut().find(|(group, _)| *group == application) {
            Some((_, objects)) => objects.push(object),
            None => groups.push((application, vec![object])),
        }
    });
    let found: Vec<ObjectReference<'gc>> = groups
        .into_iter()
        .flat_map(|(_, objects)| objects)
        .collect();
    numbered(mutator, values, &found, start, count, |_| shipped)
}

/// `count` of the objects that refer to `value`, from the `start`th, in
/// the space's order, each named `#<n>`, counted from 1 over all of them
/// (`Command::Referrers`): every object a word of which that keeps what
/// it names alive names `value` (`Heap::refers_to`) -- a closure by its
/// copied values or its captured frame, a frame object by its fixed
/// fields and its registers, a running activation's read where they live
/// -- but for the class word. An activation no block captured has no
/// frame object and so is no referrer: the stack shows what it holds. A
/// referrer of a class a shipped module declares, a frame object among
/// them, is withheld. Empty for an immediate value, which no word names.
pub(crate) fn referrers<'gc>(
    mutator: &Mutator<'gc>,
    values: &mut StopValues,
    value: Value<'gc>,
    start: usize,
    count: usize,
) -> Vec<Variable> {
    let Some(target) = value.as_object() else {
        return Vec::new();
    };
    let heap = &mutator.heap;
    let mut found = Vec::new();
    heap.each_object(|holder| {
        if heap.refers_to(holder, target) || registers_refer_to(mutator, holder, target) {
            found.push(holder);
        }
    });
    let mut declarations = Declarations::default();
    numbered(mutator, values, &found, start, count, |object| {
        declarations
            .of_class(mutator, heap.object_class(object))
            .is_some_and(|declaration| declaration.shipped)
    })
}

/// Whether `holder` is a frame object married to a running activation
/// one of whose named registers -- an argument, or one past the header's
/// numbers below the first scratch register -- holds `target`, read as
/// the variables view reads a frame's: only a register the leftover
/// analysis does not find tainted at the activation's program counter,
/// since a tainted one may hold a word a callee or a popped frame left.
fn registers_refer_to<'gc>(
    mutator: &Mutator<'gc>,
    holder: ObjectReference<'gc>,
    target: ObjectReference<'gc>,
) -> bool {
    let heap = &mutator.heap;
    let Some(frame) = heap.object_frame(holder) else {
        return false;
    };
    let Some(activation) = heap.frame_activation(frame) else {
        return false;
    };
    let code = heap.activation_code(activation);
    let Ok(program_counter) = u32::try_from(heap.activation_program_counter(activation)) else {
        return false;
    };
    let tainted = code.tainted_before(program_counter);
    (0..code.local_count)
        .filter(|register| is_named_register(&code, *register))
        .filter(|register| !tainted.iter().any(|range| range.contains(register)))
        .any(|register| {
            heap.frame_local(frame, register)
                .as_object()
                .is_some_and(|object| object.same(target))
        })
}

/// `count` of `objects` from the `start`th, each named by its place among
/// them, `#1` the first, and described for the stop, withheld where
/// `withheld` says so.
fn numbered<'gc>(
    mutator: &Mutator<'gc>,
    values: &mut StopValues,
    objects: &[ObjectReference<'gc>],
    start: usize,
    count: usize,
    mut withheld: impl FnMut(ObjectReference<'gc>) -> bool,
) -> Vec<Variable> {
    objects
        .iter()
        .enumerate()
        .skip(start)
        .take(count)
        .map(|(index, object)| Variable {
            name: format!("#{}", index + 1),
            value: VariableValue::Shown(values.describe(
                mutator,
                Value::object(*object),
                withheld(*object),
            )),
        })
        .collect()
}

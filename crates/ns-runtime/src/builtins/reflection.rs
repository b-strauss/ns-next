//! The leaves the shipped mirrors module reads classes, mixins, methods
//! and objects through (`docs/design/liveness-and-reflection.md`,
//! "Mirrors that read"). A class leaf takes a class
//! or a metaclass, a mixin leaf a reified mixin (`Mutator::reified_mixin`),
//! and each declines -- answers `None`, so the failure block runs -- for
//! anything else; none raises. Two leaves serve the module beside them:
//! identity and a Symbol's text, which the kernel's rows answer too, but
//! seen through a future. One writes: the bulk forward, `become:`
//! (`forward_identity`). Two serve the MirrorBuilder: one checks an edit
//! of a class, the other installs it (`edit`).
//!
//! A future is read as the object it is, never as the value it stands
//! for: the failure path sees through no leaf of this family
//! (`interpreter::future::after_a_failed_leaf`), so no mirror read runs a
//! future's computation.
//!
//! Structure -- names, selectors, access modifiers, slot declarations,
//! source -- is answered of every mixin, shipped or not. A value is
//! answered only through a level whose mixin is not shipped: a slot's
//! value, an enclosing object. The VM mirror and a shipped module's
//! instance carry the authority the capability's locks keep from a
//! program, and a shipped level is where either is held, so the rule is
//! structural, by level; behind it, a value that is either is declined
//! whatever level it was reached through (`Mutator::is_withheld`).

use crate::interpreter::Arguments;
use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;

use crate::builtins::primitive::ReflectionPrimitive;
use crate::heap::{Heap, MixinId};
use crate::interpreter::edit::{Edit, EditedTexts};
use crate::mixin::{Mixin, MixinApplication, MixinMethod};
use crate::types::*;
use crate::{Mutator, Symbol};

pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: ReflectionPrimitive,
    receiver: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Value<'gc>> {
    match primitive {
        ReflectionPrimitive::MixinOf => {
            let class = class_object(mutator, receiver)?;
            let mixin = mutator.heap.class_application(class)?.mixin.clone();
            Some(Value::object(mutator.reified_mixin(&mixin)))
        }
        ReflectionPrimitive::IsMetaclass => {
            let class = class_object(mutator, receiver)?;
            Some(Value::boolean(
                mutator.heap.class_this_class(class).is_some(),
            ))
        }
        ReflectionPrimitive::EnclosingObjectOf => enclosing_object(mutator, receiver),
        ReflectionPrimitive::SlotNamesOf => Some(slot_names(mutator, receiver)),
        ReflectionPrimitive::SlotNamed => slot_named(mutator, receiver, arguments),
        ReflectionPrimitive::MixinName => {
            let mixin = reified(mutator, receiver)?;
            Some(Value::object(mutator.new_string(mixin.reflected_name())))
        }
        ReflectionPrimitive::MixinIsMeta => {
            let mixin = reified(mutator, receiver)?;
            Some(Value::boolean(mixin.is_class_side()))
        }
        ReflectionPrimitive::MixinEnclosing => {
            let id = reified_id(mutator, receiver)?;
            let heap = &mutator.heap;
            let enclosing = instance_side_id(heap, id)
                .and_then(|instance| heap.enclosing_mixin_id(instance))
                .and_then(|enclosing| heap.mixin(enclosing));
            Some(reified_or_nil(mutator, enclosing))
        }
        ReflectionPrimitive::MixinInstanceSide => {
            let id = reified_id(mutator, receiver)?;
            if !mutator.heap.mixin(id)?.is_class_side() {
                return Some(receiver);
            }
            let instance =
                instance_side_id(&mutator.heap, id).and_then(|id| mutator.heap.mixin(id));
            Some(reified_or_nil(mutator, instance))
        }
        ReflectionPrimitive::MixinClassSide => {
            let mixin = reified(mutator, receiver)?;
            if mixin.is_class_side() {
                return Some(receiver);
            }
            let class_side = mutator.metaclass_mixin(&mixin);
            Some(Value::object(mutator.reified_mixin(&class_side)))
        }
        ReflectionPrimitive::MixinSlots => {
            let mixin = reified(mutator, receiver)?;
            Some(mixin_slots(mutator, &mixin))
        }
        ReflectionPrimitive::MixinMethods => {
            let mixin = reified(mutator, receiver)?;
            Some(mixin_methods(mutator, &mixin))
        }
        ReflectionPrimitive::MixinNested => {
            let id = reified_id(mutator, receiver)?;
            Some(mixin_nested(mutator, id))
        }
        ReflectionPrimitive::MixinAccess => {
            let mixin = reified(mutator, receiver)?;
            let access = instance_side(&mixin).map_or(mixin.access, |instance| instance.access);
            Some(access_symbol(mutator, access))
        }
        ReflectionPrimitive::MethodSource => method_source(mutator, receiver, arguments),
        ReflectionPrimitive::MixinSource => {
            let id = reified_id(mutator, receiver)?;
            let text = instance_side_id(&mutator.heap, id)
                .and_then(|instance| mutator.heap.declaration_source(instance));
            Some(string_or_nil(mutator, text))
        }
        ReflectionPrimitive::MixinIsShipped => {
            let mixin = reified(mutator, receiver)?;
            Some(Value::boolean(is_shipped(&mixin)))
        }
        ReflectionPrimitive::Identical => {
            Some(Value::boolean(crate::builtins::object_class::identical(
                &mutator.heap,
                receiver,
                arguments.first(&mutator.heap)?,
            )))
        }
        ReflectionPrimitive::SymbolText => {
            let symbol = mutator.heap.symbol_value(receiver.as_object()?)?;
            Some(Value::object(
                mutator.new_string(symbol.as_str().to_string()),
            ))
        }
        ReflectionPrimitive::ForwardIdentity => forward_identity(mutator, receiver, arguments),
        ReflectionPrimitive::CheckEdit => {
            edit(mutator, receiver, arguments, |mutator, id, edit| {
                mutator
                    .prepare_edit(id, edit)
                    .map(|prepared| prepared.texts)
            })
        }
        ReflectionPrimitive::InstallEdit => {
            edit(mutator, receiver, arguments, |mutator, id, edit| {
                mutator.install_edit(id, edit)
            })
        }
    }
}

/// `reflectionMixin:checkEdit:ifFail:` and
/// `reflectionMixin:installEdit:ifFail:`: `run` -- preparing or
/// installing -- on the declaration the subject, a reified mixin, belongs
/// to, a class side's being its instance side's, and the edit the
/// argument stands for (`Edit::from_value`). Answers `{true.
/// declarationText. headerText}` or `{false. diagnostic}`, one shape for
/// both, so that the builder reads a check and an install alike, and
/// takes the header from the parse rather than reading syntax itself; a
/// refusal is an
/// answer rather than a decline because a failure block receives no
/// argument to carry the diagnostic in. Declined for any other subject
/// and for a malformed edit.
fn edit<'gc>(
    mutator: &mut Mutator<'gc>,
    subject: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
    run: impl FnOnce(&mut Mutator<'gc>, MixinId, &Edit) -> Result<EditedTexts, String>,
) -> Option<Value<'gc>> {
    let id = instance_side_id(&mutator.heap, reified_id(mutator, subject)?)?;
    let edit = Edit::from_value(&mutator.heap, arguments.first(&mutator.heap)?)?;
    let answer = match run(mutator, id, &edit) {
        Ok(texts) => {
            let declaration = Value::object(mutator.new_string(texts.declaration));
            let header = Value::object(mutator.new_string(texts.header));
            vec![Value::boolean(true), declaration, header]
        }
        Err(diagnostic) => {
            let diagnostic = Value::object(mutator.new_string(diagnostic));
            vec![Value::boolean(false), diagnostic]
        }
    };
    Some(Value::object(mutator.new_array(answer)))
}

/// `elementsOf:forwardIdentityToElementsOf:ifFail:`: forwards every
/// object of the subject, an `Array`, to the object of the argument, an
/// `Array`, at the same index, and answers `nil`
/// (`heap::world::forward_identity`). Declined, with nothing changed,
/// when either is no `Array`, when an element of either may not be
/// forwarded (`may_be_forwarded`), and when the heap refuses the pairs:
/// unequal sizes, a split or a chain.
///
/// The handles collected here are dropped with the leaf's frame, and
/// nothing reads them after the forward: one naming a forwarded object
/// names a forwarder from then on.
fn forward_identity<'gc>(
    mutator: &mut Mutator<'gc>,
    subject: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Value<'gc>> {
    let old = forwardable_elements(mutator, subject)?;
    let new = forwardable_elements(mutator, arguments.first(&mutator.heap)?)?;
    crate::heap::forward_identity(mutator, &old, &new).ok()?;
    Some(Value::nil())
}

/// The elements of `array`, when it is an `Array` every element of which
/// may be forwarded; `None` otherwise.
fn forwardable_elements<'gc>(
    mutator: &Mutator<'gc>,
    array: Value<'gc>,
) -> Option<Vec<ObjectReference<'gc>>> {
    mutator
        .heap
        .array_elements(array.as_object()?)?
        .into_iter()
        .map(|element| {
            element
                .as_object()
                .filter(|object| may_be_forwarded(mutator, *object))
        })
        .collect()
}

/// Whether `object` may stand on either side of a forward: an ordinary
/// instance -- no class, closure, frame, `Array`, String, number or
/// reified mixin -- of a class that is no builtin and whose mixin is not
/// shipped, and no value a mirror withholds.
///
/// The leaf's one caller, the atomic install, only ever forwards a
/// program's instances to their rebuilt replacements, so this refuses
/// nothing it asks for. It is a lock of its own all the same: a defect in
/// the install, or in whatever else comes to hold the mirror, must not
/// forward the kernel's objects, a shipped module's instance or the VM
/// mirror -- which would hand the capability to every holder of the
/// object forwarded to it -- nor an object whose layout the VM reads
/// natively (`docs/design/object-model.md`, "What the forward refuses").
/// The VM mirror's class is a builtin whose mixin is not marked shipped,
/// so the builtin test, and behind it the withheld one, are what refuse
/// it.
pub(crate) fn may_be_forwarded<'gc>(mutator: &Mutator<'gc>, object: ObjectReference<'gc>) -> bool {
    let heap = &mutator.heap;
    if heap.instance_field_count(object).is_none() || heap.mixin_object_id(object).is_some() {
        return false;
    }
    let class = heap.object_class(object);
    heap.class_builtin(class).is_none()
        && heap
            .class_application(class)
            .is_some_and(|application| !is_shipped(&application.mixin))
        && !mutator.is_withheld(Value::object(object))
}

/// The subject as a class object, a class or a metaclass; `None` for
/// anything else.
fn class_object<'gc>(mutator: &Mutator<'gc>, subject: Value<'gc>) -> Option<ClassReference<'gc>> {
    subject
        .as_object()
        .filter(|object| mutator.heap.is_class(*object))
}

/// The mixin the subject stands for, when it is a reified mixin; `None`
/// for anything else.
fn reified<'gc>(mutator: &Mutator<'gc>, subject: Value<'gc>) -> Option<Rc<Mixin>> {
    mutator.mixin_of_reified(subject.as_object()?)
}

/// The id of the mixin the subject stands for, when it is a reified mixin;
/// `None` for anything else. A leaf that reads the mixin's place -- the
/// mixin it is declared in, the text it is read in -- reads it through the
/// id, whose row keeps it across a replacement.
fn reified_id<'gc>(mutator: &Mutator<'gc>, subject: Value<'gc>) -> Option<MixinId> {
    mutator.heap.mixin_object_id(subject.as_object()?)
}

/// The id of the instance side the mixin `id` belongs to: `id` itself, or
/// a class side's instance side's.
fn instance_side_id(heap: &Heap<'_>, id: MixinId) -> Option<MixinId> {
    if heap.mixin(id)?.is_class_side() {
        heap.enclosing_mixin_id(id)
    } else {
        Some(id)
    }
}

/// The reified `mixin`, or `nil` when there is none.
fn reified_or_nil<'gc>(mutator: &mut Mutator<'gc>, mixin: Option<Rc<Mixin>>) -> Value<'gc> {
    mixin.map_or(Value::nil(), |mixin| {
        Value::object(mutator.reified_mixin(&mixin))
    })
}

/// The instance side `mixin` belongs to: itself, or a class side's, while
/// it lives. A class side's name, access, source and shipped flag are its
/// instance side's, which its declaration is.
fn instance_side(mixin: &Rc<Mixin>) -> Option<Rc<Mixin>> {
    if mixin.is_class_side() {
        mixin.enclosing_mixin()
    } else {
        Some(mixin.clone())
    }
}

/// Whether `mixin` was compiled from shipped source, a class side by its
/// instance side: a synthesized class side is built empty, unmarked,
/// whoever's class it stands for. A class side whose instance side no
/// longer lives counts as shipped, so that a level whose origin cannot be
/// told never hands out a value.
fn is_shipped(mixin: &Rc<Mixin>) -> bool {
    instance_side(mixin).is_none_or(|instance| instance.shipped)
}

/// `#public`, `#protected` or `#private`.
fn access_symbol<'gc>(mutator: &mut Mutator<'gc>, access: ast::AccessModifier) -> Value<'gc> {
    let name = match access {
        ast::AccessModifier::Public => "public",
        ast::AccessModifier::Protected => "protected",
        ast::AccessModifier::Private => "private",
    };
    mutator.new_symbol(Symbol::intern(name))
}

/// A String of `text`, or `nil` when there is none.
fn string_or_nil<'gc>(mutator: &mut Mutator<'gc>, text: Option<String>) -> Value<'gc> {
    text.map_or(Value::nil(), |text| Value::object(mutator.new_string(text)))
}

/// `enclosingObject`: a class object's enclosing object, `nil` for a
/// module's class. Declined for a class whose mixin is shipped -- a facet
/// class's enclosing object is its module's root instance, a private
/// shipped class's one that carries the module's authority -- and for a
/// value that is withheld.
fn enclosing_object<'gc>(mutator: &Mutator<'gc>, subject: Value<'gc>) -> Option<Value<'gc>> {
    let class = class_object(mutator, subject)?;
    if is_shipped(&mutator.heap.class_declaration(class)?) {
        return None;
    }
    let enclosing = mutator
        .heap
        .class_reflected_enclosing(class)
        .map_or(Value::nil(), Value::object);
    (!mutator.is_withheld(enclosing)).then_some(enclosing)
}

/// Every level of `subject`'s class chain, nearest first, shipped ones
/// included, when `subject` is an ordinary instance -- the only shape whose
/// state is named slots. Empty for anything else.
fn instance_levels<'gc>(mutator: &Mutator<'gc>, subject: Value<'gc>) -> Vec<Rc<MixinApplication>> {
    let Some(object) = subject.as_object() else {
        return Vec::new();
    };
    if mutator.heap.is_class(object) || mutator.heap.instance_field_count(object).is_none() {
        return Vec::new();
    }
    let mut levels = Vec::new();
    let mut level = mutator
        .heap
        .class_application(mutator.heap.object_class(object));
    while let Some(application) = level {
        level = application.superclass.clone();
        levels.push(application);
    }
    levels
}

/// The slots of `subject` a mirror reads, each name once, with its field
/// index: superclass level first, declaration order within a level. Under
/// each name only the slot the object's own code sees counts -- the one
/// the nearest level declaring the name declares -- and a name whose
/// nearest declaration is lazy, or on a shipped level, is left out
/// entirely, never answered by a farther slot it shadows: a mirror reads
/// what the name names, so a shadowed slot's value never stands under it.
fn visible_slots<'gc>(mutator: &Mutator<'gc>, subject: Value<'gc>) -> Vec<(Symbol, usize)> {
    let levels = instance_levels(mutator, subject);
    let fields = subject
        .as_object()
        .and_then(|object| mutator.heap.instance_field_count(object))
        .unwrap_or(0);
    // The nearest declaration of each name: its level's index in `levels`
    // and its position among that level's own slots.
    let mut nearest: HashMap<Symbol, (usize, usize)> = HashMap::new();
    for (index, application) in levels.iter().enumerate() {
        for (position, slot) in application.mixin.slots.iter().enumerate() {
            nearest.entry(slot.name).or_insert((index, position));
        }
    }
    let mut visible = Vec::new();
    for (index, application) in levels.iter().enumerate().rev() {
        if is_shipped(&application.mixin) {
            continue;
        }
        for (position, slot) in application.mixin.slots.iter().enumerate() {
            let field = application.own_slot_base + position;
            if !slot.lazy && field < fields && nearest.get(&slot.name) == Some(&(index, position)) {
                visible.push((slot.name, field));
            }
        }
    }
    visible
}

/// `slots`: the names of `subject`'s visible slots (`visible_slots`),
/// superclass level first and in declaration order within a level.
fn slot_names<'gc>(mutator: &mut Mutator<'gc>, subject: Value<'gc>) -> Value<'gc> {
    let names = visible_slots(mutator, subject);
    let symbols = names
        .into_iter()
        .map(|(name, _)| mutator.new_symbol(name))
        .collect();
    Value::object(mutator.new_array(symbols))
}

/// `getSlot:`: the value of `subject`'s visible slot (`visible_slots`) of
/// the name the argument, a Symbol, gives. Declined for a name that is no
/// Symbol, for a name no level declares, for one whose nearest
/// declaration is lazy or on a shipped level -- never reading a farther
/// slot it shadows -- and for a value that is withheld.
fn slot_named<'gc>(
    mutator: &Mutator<'gc>,
    subject: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Value<'gc>> {
    let name = mutator
        .heap
        .symbol_value(arguments.first(&mutator.heap)?.as_object()?)?;
    let object = subject.as_object()?;
    let field = visible_slots(mutator, subject)
        .into_iter()
        .find_map(|(slot, field)| (slot == name).then_some(field))?;
    let value = mutator.heap.instance_field(object, field)?;
    (!mutator.is_withheld(value)).then_some(value)
}

/// `slots` of a mixin: one `{name. isMutable. accessModifier. isLazy}`
/// per own slot, in declaration order.
fn mixin_slots<'gc>(mutator: &mut Mutator<'gc>, mixin: &Mixin) -> Value<'gc> {
    let mut declarations = Vec::with_capacity(mixin.slots.len());
    for slot in &mixin.slots {
        let name = mutator.new_symbol(slot.name);
        let access = access_symbol(mutator, slot.access);
        declarations.push([
            name,
            Value::boolean(slot.mutable),
            access,
            Value::boolean(slot.lazy),
        ]);
    }
    let elements = declarations
        .into_iter()
        .map(|declaration| Value::object(mutator.new_array(declaration.to_vec())))
        .collect();
    Value::object(mutator.new_array(elements))
}

/// The methods `mixin`'s declaration writes, by their position in the
/// source, then by selector, so the order is the same whatever the
/// table's hashing: its compiled methods but a lazy slot's getter. A
/// slot's accessors, the factory and a primitive are no compiled method,
/// and a nested class's accessor is not in the table.
fn declared_methods(mixin: &Mixin) -> Vec<&Rc<MixinMethod>> {
    let mut methods: Vec<&Rc<MixinMethod>> = mixin
        .compiled_methods()
        .filter(|method| {
            !mixin
                .slots
                .iter()
                .any(|slot| slot.lazy && slot.name == method.selector)
        })
        .collect();
    methods.sort_by(|first, second| {
        first
            .code
            .source
            .start
            .cmp(&second.code.source.start)
            .then_with(|| first.selector.as_str().cmp(second.selector.as_str()))
    });
    methods
}

/// `methods` of a mixin: one `{selector. accessModifier}` per declared
/// method.
fn mixin_methods<'gc>(mutator: &mut Mutator<'gc>, mixin: &Mixin) -> Value<'gc> {
    let members: Vec<(Symbol, ast::AccessModifier)> = declared_methods(mixin)
        .into_iter()
        .map(|method| (method.selector, method.access))
        .collect();
    let mut pairs = Vec::with_capacity(members.len());
    for (selector, access) in members {
        let selector = mutator.new_symbol(selector);
        let access = access_symbol(mutator, access);
        pairs.push([selector, access]);
    }
    let elements = pairs
        .into_iter()
        .map(|pair| Value::object(mutator.new_array(pair.to_vec())))
        .collect();
    Value::object(mutator.new_array(elements))
}

/// `nestedClasses` of the mixin `id`: the current versions of its nested
/// mixins, reified, in declaration order; none for a class side, which
/// declares no class.
fn mixin_nested<'gc>(mutator: &mut Mutator<'gc>, id: MixinId) -> Value<'gc> {
    let nested: Vec<Rc<Mixin>> = mutator
        .heap
        .nested_of(id)
        .iter()
        .filter_map(|nested_id| mutator.heap.mixin(*nested_id))
        .collect();
    let nested: Vec<Value<'gc>> = nested
        .iter()
        .map(|nested| Value::object(mutator.reified_mixin(nested)))
        .collect();
    Value::object(mutator.new_array(nested))
}

/// `source` of a method: the text of the declared method the argument, a
/// Symbol, names, from its access modifier to its closing parenthesis, or
/// `nil` when the mixin holds no text or the span does not lie within it.
/// Declined for a name that is no Symbol or names no declared method.
fn method_source<'gc>(
    mutator: &mut Mutator<'gc>,
    subject: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Value<'gc>> {
    let id = reified_id(mutator, subject)?;
    let mixin = mutator.heap.mixin(id)?;
    let selector = mutator
        .heap
        .symbol_value(arguments.first(&mutator.heap)?.as_object()?)?;
    let method = declared_methods(&mixin)
        .into_iter()
        .find(|method| method.selector == selector)?;
    // A class side's methods lie in its instance side's declaration, whose
    // text a class side shares.
    let source = mixin
        .text
        .as_ref()
        .and_then(|text| text.slice(method.code.source))
        .map(str::to_string);
    Some(string_or_nil(mutator, source))
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use crate::{HostResult, HostSignal, Interpreter, Mutator, Root, Symbol, Value, ValueKind};

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// What a leaf's failure block answers in these tests: a value no leaf
    /// here ever produces, so a decline is unmistakable.
    const DECLINED: i64 = -1;

    /// `selector`'s leaf run on `subject` with `arguments`, through a send
    /// to the mirror, the way the mirrors module will send it.
    fn leaf(
        interpreter: &mut Interpreter,
        selector: &str,
        subject: &Root,
        arguments: &[Root],
    ) -> HostResult {
        let mirror = interpreter.enter(|mutator| {
            let mirror = mutator.vm_mirror();
            mutator.stash(mirror)
        });
        let mut sent = vec![subject.clone()];
        sent.extend(arguments.iter().cloned());
        sent.push(interpreter.stash(Value::small_integer(DECLINED)));
        interpreter.send(&mirror, Symbol::intern(selector), &sent)
    }

    /// `selector`'s leaf on `subject`, its answer kept as a root.
    fn answer_of(
        interpreter: &mut Interpreter,
        selector: &str,
        subject: &Root,
        arguments: &[Root],
    ) -> Root {
        let result = leaf(interpreter, selector, subject, arguments);
        expect_ok(result, selector)
    }

    /// `selector`'s leaf on `subject`, its answer rendered by `render`.
    fn rendered(
        interpreter: &mut Interpreter,
        selector: &str,
        subject: &Root,
        arguments: &[Root],
    ) -> String {
        let answer = answer_of(interpreter, selector, subject, arguments);
        interpreter.enter(|mutator| {
            let value = mutator.fetch(&answer);
            render(mutator, value)
        })
    }

    /// An answer as these tests compare it: a Symbol as `#name`, a String
    /// quoted, a reified mixin as `<name>`, an `Array` in brace syntax, a
    /// decline as `declined`.
    fn render<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> String {
        match value.kind() {
            ValueKind::Nil => "nil".to_string(),
            ValueKind::Boolean(flag) => flag.to_string(),
            ValueKind::SmallInteger(DECLINED) => "declined".to_string(),
            ValueKind::SmallInteger(integer) => integer.to_string(),
            ValueKind::Object(object) => {
                let heap = mutator.heap();
                if let Some(symbol) = heap.symbol_value(object) {
                    format!("#{symbol}")
                } else if let Some(text) = heap.str_value(object) {
                    format!("'{text}'")
                } else if let Some(mixin) = mutator.mixin_of_reified(object) {
                    format!("<{}>", mixin.reflected_name())
                } else if let Some(elements) = heap.array_elements(object) {
                    let parts: Vec<String> = elements
                        .iter()
                        .map(|element| render(mutator, *element))
                        .collect();
                    format!("{{{}}}", parts.join(". "))
                } else {
                    heap.describe(value)
                }
            }
            _ => mutator.heap().describe(value),
        }
    }

    /// Whether two roots hold one object.
    fn same(interpreter: &mut Interpreter, first: &Root, second: &Root) -> bool {
        interpreter.enter(|mutator| {
            match (
                mutator.fetch(first).as_object(),
                mutator.fetch(second).as_object(),
            ) {
                (Some(first), Some(second)) => first.same(second),
                _ => false,
            }
        })
    }

    fn send(interpreter: &mut Interpreter, receiver: &Root, selector: &str) -> Root {
        expect_ok(
            interpreter.send(receiver, Symbol::intern(selector), &[]),
            selector,
        )
    }

    /// The class of `subject`, read on the host side.
    fn class_of(interpreter: &mut Interpreter, subject: &Root) -> Root {
        interpreter.enter(|mutator| {
            let class = mutator.class_of(mutator.fetch(subject));
            mutator.stash(Value::object(class))
        })
    }

    fn a_symbol(interpreter: &mut Interpreter, name: &str) -> Root {
        interpreter.enter(|mutator| {
            let symbol = mutator.new_symbol(Symbol::intern(name));
            mutator.stash(symbol)
        })
    }

    fn a_string(interpreter: &mut Interpreter, text: &str) -> Root {
        interpreter.enter(|mutator| {
            let string = mutator.new_string(text.to_string());
            mutator.stash(Value::object(string))
        })
    }

    /// The instance of the shipped module `name`, read on the host side,
    /// where no program can reach it.
    fn module_instance(interpreter: &mut Interpreter, name: &str) -> Root {
        interpreter.enter(|mutator| {
            let Some(instance) = mutator.shipped_world.module_instance(Symbol::intern(name)) else {
                panic!("{name} is recorded");
            };
            mutator.stash(Value::object(instance))
        })
    }

    /// A module over the platform: `Base` and `Derived` for the members and
    /// the slot walk, `Plain` with no class side, and `Failed`, whose levels
    /// above it are the kernel's `Error` and `Exception`, which declare
    /// slots.
    fn a_fixture() -> (Interpreter, Root) {
        let source = indoc! {"
            class Fixture usingPlatform: platform = Object new (
              | public transcript = platform transcript. public collections = platform collections. |
            ) (
              public class Base = (
                | public inherited ::= 0. |
              ) ()
              public class Derived = Base (
                | own ::= 1. private secret = 2. |
              ) (
                public class Inner = () ()
                lazy later ::= 3.
                public b = (^1)
                a = (^2)
                private c = (^3)
              ) : (
                make = (^self new)
              )
              public class Plain = () ()
              public class Failed = Error () ()
              public derived = (^Derived new)
              public failed = (^Failed new)
              public list = (^collections List)
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the fixture parses");
        let resolved = match crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("reflection_fixture.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the fixture resolves"),
        };
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let platform = interpreter.platform();
        let fixture = expect_ok(
            interpreter.instantiate(&class, &[platform]),
            "Fixture usingPlatform:",
        );
        (interpreter, fixture)
    }

    #[test]
    fn a_class_answers_its_mixin_and_a_metaclass_its_class_side() {
        let (mut interpreter, fixture) = a_fixture();
        let derived = send(&mut interpreter, &fixture, "Derived");
        let metaclass = class_of(&mut interpreter, &derived);
        let mixin = answer_of(&mut interpreter, "reflectionMixinOf:ifFail:", &derived, &[]);
        let class_side = answer_of(
            &mut interpreter,
            "reflectionMixinOf:ifFail:",
            &metaclass,
            &[],
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinIsMeta:ifFail:",
                &mixin,
                &[]
            ),
            "false"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinIsMeta:ifFail:",
                &class_side,
                &[]
            ),
            "true"
        );
        assert_eq!(
            rendered(&mut interpreter, "reflectionMixinName:ifFail:", &mixin, &[]),
            "'Derived'"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinName:ifFail:",
                &class_side,
                &[]
            ),
            "'Derived class'"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionIsMetaclass:ifFail:",
                &derived,
                &[]
            ),
            "false"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionIsMetaclass:ifFail:",
                &metaclass,
                &[]
            ),
            "true"
        );
        let three = interpreter.stash(Value::small_integer(3));
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionIsMetaclass:ifFail:",
                &three,
                &[]
            ),
            "declined"
        );
        assert_eq!(
            rendered(&mut interpreter, "reflectionMixinOf:ifFail:", &three, &[]),
            "declined"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinName:ifFail:",
                &derived,
                &[]
            ),
            "declined"
        );
        let again = answer_of(&mut interpreter, "reflectionMixinOf:ifFail:", &derived, &[]);
        assert!(same(&mut interpreter, &mixin, &again));
    }

    #[test]
    fn the_members_of_a_mixin_are_answered_in_their_order() {
        let (mut interpreter, fixture) = a_fixture();
        let derived = send(&mut interpreter, &fixture, "Derived");
        let metaclass = class_of(&mut interpreter, &derived);
        let mixin = answer_of(&mut interpreter, "reflectionMixinOf:ifFail:", &derived, &[]);
        let class_side = answer_of(
            &mut interpreter,
            "reflectionMixinOf:ifFail:",
            &metaclass,
            &[],
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinSlots:ifFail:",
                &mixin,
                &[]
            ),
            "{{#own. true. #protected. false}. {#secret. false. #private. false}. \
             {#later. true. #protected. true}}"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinMethods:ifFail:",
                &mixin,
                &[]
            ),
            "{{#b. #public}. {#a. #protected}. {#c. #private}}"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinNested:ifFail:",
                &mixin,
                &[]
            ),
            "{<Inner>}"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinAccess:ifFail:",
                &mixin,
                &[]
            ),
            "#public"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinMethods:ifFail:",
                &class_side,
                &[]
            ),
            "{{#make. #protected}}"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinNested:ifFail:",
                &class_side,
                &[]
            ),
            "{}"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinSlots:ifFail:",
                &class_side,
                &[]
            ),
            "{}"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinAccess:ifFail:",
                &class_side,
                &[]
            ),
            "#public"
        );
        // The nested mixin answered is the one the nested class applies.
        let derived_instance = send(&mut interpreter, &fixture, "derived");
        let inner = send(&mut interpreter, &derived_instance, "Inner");
        let inner_mixin = answer_of(&mut interpreter, "reflectionMixinOf:ifFail:", &inner, &[]);
        let nested = answer_of(
            &mut interpreter,
            "reflectionMixinNested:ifFail:",
            &mixin,
            &[],
        );
        let first = interpreter.enter(|mutator| {
            let Some(array) = mutator.fetch(&nested).as_object() else {
                panic!("nestedClasses is an Array");
            };
            let Some(first) = mutator.heap().array_element(array, 0) else {
                panic!("nestedClasses holds Inner");
            };
            mutator.stash(first)
        });
        assert!(same(&mut interpreter, &first, &inner_mixin));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_synthesized_class_side_is_one_reified_mixin() {
        let (mut interpreter, fixture) = a_fixture();
        let plain = send(&mut interpreter, &fixture, "Plain");
        let mixin = answer_of(&mut interpreter, "reflectionMixinOf:ifFail:", &plain, &[]);
        let class_side = answer_of(
            &mut interpreter,
            "reflectionMixinClassSide:ifFail:",
            &mixin,
            &[],
        );
        let again = answer_of(
            &mut interpreter,
            "reflectionMixinClassSide:ifFail:",
            &mixin,
            &[],
        );
        assert!(same(&mut interpreter, &class_side, &again));
        let instance_side = answer_of(
            &mut interpreter,
            "reflectionMixinInstanceSide:ifFail:",
            &class_side,
            &[],
        );
        assert!(same(&mut interpreter, &instance_side, &mixin));
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinName:ifFail:",
                &class_side,
                &[]
            ),
            "'Plain class'"
        );
        // The class side the metaclass applies is the same one.
        let metaclass = class_of(&mut interpreter, &plain);
        let applied = answer_of(
            &mut interpreter,
            "reflectionMixinOf:ifFail:",
            &metaclass,
            &[],
        );
        assert!(same(&mut interpreter, &applied, &class_side));
        // An instance side is its own instance side, a class side its own
        // class side.
        let itself = answer_of(
            &mut interpreter,
            "reflectionMixinInstanceSide:ifFail:",
            &mixin,
            &[],
        );
        assert!(same(&mut interpreter, &itself, &mixin));
        let itself = answer_of(
            &mut interpreter,
            "reflectionMixinClassSide:ifFail:",
            &class_side,
            &[],
        );
        assert!(same(&mut interpreter, &itself, &class_side));
        interpreter.collect_now();
        let after = answer_of(
            &mut interpreter,
            "reflectionMixinClassSide:ifFail:",
            &mixin,
            &[],
        );
        assert!(same(&mut interpreter, &after, &class_side));
        assert_eq!(
            rendered(&mut interpreter, "reflectionMixinName:ifFail:", &after, &[]),
            "'Plain class'"
        );
        let instance_side = answer_of(
            &mut interpreter,
            "reflectionMixinInstanceSide:ifFail:",
            &after,
            &[],
        );
        assert!(same(&mut interpreter, &instance_side, &mixin));
    }

    #[test]
    fn a_mixin_answers_its_enclosing_mixin() {
        let (mut interpreter, fixture) = a_fixture();
        let root_class = class_of(&mut interpreter, &fixture);
        let root = answer_of(
            &mut interpreter,
            "reflectionMixinOf:ifFail:",
            &root_class,
            &[],
        );
        let derived = send(&mut interpreter, &fixture, "Derived");
        let derived_mixin = answer_of(&mut interpreter, "reflectionMixinOf:ifFail:", &derived, &[]);
        let derived_instance = send(&mut interpreter, &fixture, "derived");
        let inner = send(&mut interpreter, &derived_instance, "Inner");
        let inner_mixin = answer_of(&mut interpreter, "reflectionMixinOf:ifFail:", &inner, &[]);
        let metaclass = class_of(&mut interpreter, &derived);
        let class_side = answer_of(
            &mut interpreter,
            "reflectionMixinOf:ifFail:",
            &metaclass,
            &[],
        );

        let enclosing = answer_of(
            &mut interpreter,
            "reflectionMixinEnclosing:ifFail:",
            &inner_mixin,
            &[],
        );
        assert!(same(&mut interpreter, &enclosing, &derived_mixin));
        let enclosing = answer_of(
            &mut interpreter,
            "reflectionMixinEnclosing:ifFail:",
            &derived_mixin,
            &[],
        );
        assert!(same(&mut interpreter, &enclosing, &root));
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinEnclosing:ifFail:",
                &root,
                &[]
            ),
            "nil"
        );
        let enclosing = answer_of(
            &mut interpreter,
            "reflectionMixinEnclosing:ifFail:",
            &class_side,
            &[],
        );
        assert!(same(&mut interpreter, &enclosing, &root));
    }

    #[test]
    fn a_method_and_a_class_answer_their_source() {
        let (mut interpreter, fixture) = a_fixture();
        let derived = send(&mut interpreter, &fixture, "Derived");
        let mixin = answer_of(&mut interpreter, "reflectionMixinOf:ifFail:", &derived, &[]);
        let selector = a_symbol(&mut interpreter, "b");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixin:methodSource:ifFail:",
                &mixin,
                &[selector]
            ),
            "'public b = (^1)'"
        );
        let accessor = a_symbol(&mut interpreter, "own");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixin:methodSource:ifFail:",
                &mixin,
                &[accessor]
            ),
            "declined"
        );
        let lazy_getter = a_symbol(&mut interpreter, "later");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixin:methodSource:ifFail:",
                &mixin,
                &[lazy_getter]
            ),
            "declined"
        );
        let text = a_string(&mut interpreter, "b");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixin:methodSource:ifFail:",
                &mixin,
                &[text]
            ),
            "declined"
        );
        let derived_instance = send(&mut interpreter, &fixture, "derived");
        let inner = send(&mut interpreter, &derived_instance, "Inner");
        let inner_mixin = answer_of(&mut interpreter, "reflectionMixinOf:ifFail:", &inner, &[]);
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinSource:ifFail:",
                &inner_mixin,
                &[]
            ),
            "'public class Inner = () ()'"
        );
        // A class side's source is its instance side's declaration.
        let metaclass = class_of(&mut interpreter, &inner);
        let class_side = answer_of(
            &mut interpreter,
            "reflectionMixinOf:ifFail:",
            &metaclass,
            &[],
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinSource:ifFail:",
                &class_side,
                &[]
            ),
            "'public class Inner = () ()'"
        );
    }

    #[test]
    fn the_slots_of_an_object_skip_lazy_and_shipped_levels() {
        let (mut interpreter, fixture) = a_fixture();
        let derived = send(&mut interpreter, &fixture, "derived");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotNamesOf:ifFail:",
                &derived,
                &[]
            ),
            "{#inherited. #own. #secret}"
        );
        let failed = send(&mut interpreter, &fixture, "failed");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotNamesOf:ifFail:",
                &failed,
                &[]
            ),
            "{}"
        );
        let three = interpreter.stash(Value::small_integer(3));
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotNamesOf:ifFail:",
                &three,
                &[]
            ),
            "{}"
        );
        let text = a_string(&mut interpreter, "text");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotNamesOf:ifFail:",
                &text,
                &[]
            ),
            "{}"
        );
        let class = send(&mut interpreter, &fixture, "Derived");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotNamesOf:ifFail:",
                &class,
                &[]
            ),
            "{}"
        );
    }

    #[test]
    fn a_slot_is_read_by_name_from_the_nearest_level() {
        let (mut interpreter, fixture) = a_fixture();
        let derived = send(&mut interpreter, &fixture, "derived");
        for (name, expected) in [
            ("own", "1"),
            ("secret", "2"),
            ("inherited", "0"),
            ("later", "declined"),
            ("missing", "declined"),
        ] {
            let symbol = a_symbol(&mut interpreter, name);
            assert_eq!(
                rendered(
                    &mut interpreter,
                    "reflectionSlotOf:named:ifFail:",
                    &derived,
                    &[symbol]
                ),
                expected,
                "{name}"
            );
        }
        let text = a_string(&mut interpreter, "own");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotOf:named:ifFail:",
                &derived,
                &[text]
            ),
            "declined"
        );
    }

    /// A module whose `Derived` redeclares two of `Base`'s slots: `shared`
    /// eagerly, so the object's own code sees `Derived`'s, and `hidden`
    /// lazily, so a mirror may read neither the lazy slot nor the eager
    /// one it shadows.
    fn a_shadowing_fixture() -> (Interpreter, Root) {
        let source = indoc! {"
            class Shadowing = Object new () (
              public class Base = (
                | public first ::= 1. public shared ::= 2. public hidden ::= 3. |
              ) ()
              public class Derived = Base (
                | public shared ::= 20. public last ::= 4. |
              ) (
                lazy public hidden = 30.
              )
              public derived = (^Derived new)
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the fixture parses");
        let resolved = match crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("shadowing_fixture.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the fixture resolves"),
        };
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let fixture = expect_ok(interpreter.instantiate(&class, &[]), "Shadowing new");
        (interpreter, fixture)
    }

    #[test]
    fn a_shadowed_name_is_answered_once_with_the_nearest_value() {
        let (mut interpreter, fixture) = a_shadowing_fixture();
        let derived = send(&mut interpreter, &fixture, "derived");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotNamesOf:ifFail:",
                &derived,
                &[]
            ),
            "{#first. #shared. #last}"
        );
        let shared = a_symbol(&mut interpreter, "shared");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotOf:named:ifFail:",
                &derived,
                &[shared]
            ),
            "20"
        );
    }

    #[test]
    fn a_nearer_lazy_declaration_hides_a_farther_eager_one() {
        let (mut interpreter, fixture) = a_shadowing_fixture();
        let derived = send(&mut interpreter, &fixture, "derived");
        let names = rendered(
            &mut interpreter,
            "reflectionSlotNamesOf:ifFail:",
            &derived,
            &[],
        );
        assert!(!names.contains("#hidden"), "{names}");
        let hidden = a_symbol(&mut interpreter, "hidden");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotOf:named:ifFail:",
                &derived,
                &[hidden]
            ),
            "declined"
        );
    }

    #[test]
    fn a_withheld_value_is_declined_whatever_its_level() {
        let (mut interpreter, fixture) = a_fixture();
        let transcript_module = module_instance(&mut interpreter, "TranscriptFactory");
        let vm_mirror_name = a_symbol(&mut interpreter, "vmMirror");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotOf:named:ifFail:",
                &transcript_module,
                &[vm_mirror_name]
            ),
            "declined"
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotNamesOf:ifFail:",
                &transcript_module,
                &[]
            ),
            "{}"
        );
        // A program's own slot holding a root instance, or the VM mirror,
        // declines too.
        let derived = send(&mut interpreter, &fixture, "derived");
        interpreter.enter(|mutator| {
            let Some(object) = mutator.fetch(&derived).as_object() else {
                panic!("a Derived is an object");
            };
            let Some(instance) = mutator
                .shipped_world
                .module_instance(Symbol::intern("TranscriptFactory"))
            else {
                panic!("TranscriptFactory is recorded");
            };
            let class = mutator.heap().object_class(object);
            let Some(application) = mutator.heap().class_application(class) else {
                panic!("a Derived's class is a class");
            };
            let field = |name: &str| {
                let Some(position) = application
                    .mixin
                    .slots
                    .iter()
                    .position(|slot| slot.name.as_str() == name)
                else {
                    panic!("Derived declares {name}");
                };
                application.own_slot_base + position
            };
            let mirror = mutator.vm_mirror();
            assert!(mutator
                .heap
                .instance_set_field(object, field("own"), Value::object(instance)));
            assert!(mutator
                .heap
                .instance_set_field(object, field("secret"), mirror));
        });
        for name in ["own", "secret"] {
            let symbol = a_symbol(&mut interpreter, name);
            assert_eq!(
                rendered(
                    &mut interpreter,
                    "reflectionSlotOf:named:ifFail:",
                    &derived,
                    &[symbol]
                ),
                "declined",
                "{name}"
            );
        }
        let transcript = send(&mut interpreter, &fixture, "transcript");
        let transcript_class = class_of(&mut interpreter, &transcript);
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionEnclosingObjectOf:ifFail:",
                &transcript_class,
                &[]
            ),
            "declined"
        );
        let transcript_metaclass = class_of(&mut interpreter, &transcript_class);
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionEnclosingObjectOf:ifFail:",
                &transcript_metaclass,
                &[]
            ),
            "declined"
        );
        let derived_class = send(&mut interpreter, &fixture, "Derived");
        let enclosing = answer_of(
            &mut interpreter,
            "reflectionEnclosingObjectOf:ifFail:",
            &derived_class,
            &[],
        );
        assert!(same(&mut interpreter, &enclosing, &fixture));
        let root_class = class_of(&mut interpreter, &fixture);
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionEnclosingObjectOf:ifFail:",
                &root_class,
                &[]
            ),
            "nil"
        );
        let three = interpreter.stash(Value::small_integer(3));
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionEnclosingObjectOf:ifFail:",
                &three,
                &[]
            ),
            "declined"
        );
    }

    /// A program slot holding the kernel's instance, or the platform
    /// module's, declines as one holding the transcript module's does: the
    /// leaf withholds every recorded root instance, not one alone.
    #[test]
    fn a_program_slot_holding_any_shipped_root_declines() {
        let (mut interpreter, fixture) = a_fixture();
        for module in ["Kernel", "PlatformFactory", "TranscriptFactory"] {
            let derived = send(&mut interpreter, &fixture, "derived");
            interpreter.enter(|mutator| {
                let Some(object) = mutator.fetch(&derived).as_object() else {
                    panic!("a Derived is an object");
                };
                let Some(instance) = mutator
                    .shipped_world
                    .module_instance(Symbol::intern(module))
                else {
                    panic!("{module} is recorded");
                };
                let class = mutator.heap().object_class(object);
                let Some(application) = mutator.heap().class_application(class) else {
                    panic!("a Derived's class is a class");
                };
                let Some(position) = application
                    .mixin
                    .slots
                    .iter()
                    .position(|slot| slot.name.as_str() == "own")
                else {
                    panic!("Derived declares own");
                };
                assert!(mutator.heap.instance_set_field(
                    object,
                    application.own_slot_base + position,
                    Value::object(instance)
                ));
            });
            let own = a_symbol(&mut interpreter, "own");
            assert_eq!(
                rendered(
                    &mut interpreter,
                    "reflectionSlotOf:named:ifFail:",
                    &derived,
                    &[own]
                ),
                "declined",
                "{module}"
            );
            // The name is still listed: the slot is the program's, only its
            // value is withheld.
            assert_eq!(
                rendered(
                    &mut interpreter,
                    "reflectionSlotNamesOf:ifFail:",
                    &derived,
                    &[]
                ),
                "{#inherited. #own. #secret}",
                "{module}"
            );
        }
    }

    /// A module whose classes stand over the collections module's `List`,
    /// which declares `size_`, `firstIndex`, `data` and `modifications`:
    /// `Redeclaring` declares `data` again, and `Extending` none of
    /// `List`'s names.
    fn an_over_list_fixture() -> (Interpreter, Root) {
        let source = indoc! {"
            class OverList usingPlatform: platform = Object new (
              | public List = platform collections List. |
            ) (
              public class Redeclaring = List new: 2 (
                | public data ::= 5. public extra ::= 6. |
              ) ()
              public class Extending = List new: 2 (
                | public extra ::= 7. |
              ) ()
              public redeclaring = (^Redeclaring new)
              public extending = (^Extending new)
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the fixture parses");
        let resolved = match crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("over_list_fixture.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the fixture resolves"),
        };
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let platform = interpreter.platform();
        let fixture = expect_ok(
            interpreter.instantiate(&class, &[platform]),
            "OverList usingPlatform:",
        );
        (interpreter, fixture)
    }

    /// A program level that redeclares a name a shipped level farther up
    /// declares is the nearest declaration, so its slot is listed and read;
    /// a name only the shipped level declares is neither listed nor read.
    #[test]
    fn a_program_level_over_a_shipped_one_answers_only_its_own_names() {
        let (mut interpreter, fixture) = an_over_list_fixture();
        let redeclaring = send(&mut interpreter, &fixture, "redeclaring");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotNamesOf:ifFail:",
                &redeclaring,
                &[]
            ),
            "{#data. #extra}"
        );
        for (name, expected) in [
            ("data", "5"),
            ("extra", "6"),
            ("size_", "declined"),
            ("firstIndex", "declined"),
            ("modifications", "declined"),
        ] {
            let symbol = a_symbol(&mut interpreter, name);
            assert_eq!(
                rendered(
                    &mut interpreter,
                    "reflectionSlotOf:named:ifFail:",
                    &redeclaring,
                    &[symbol]
                ),
                expected,
                "Redeclaring {name}"
            );
        }
        let extending = send(&mut interpreter, &fixture, "extending");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionSlotNamesOf:ifFail:",
                &extending,
                &[]
            ),
            "{#extra}"
        );
        for (name, expected) in [("extra", "7"), ("data", "declined"), ("size_", "declined")] {
            let symbol = a_symbol(&mut interpreter, name);
            assert_eq!(
                rendered(
                    &mut interpreter,
                    "reflectionSlotOf:named:ifFail:",
                    &extending,
                    &[symbol]
                ),
                expected,
                "Extending {name}"
            );
        }
    }

    #[test]
    fn a_mixin_reports_whether_it_is_shipped() {
        let (mut interpreter, fixture) = a_fixture();
        let list = send(&mut interpreter, &fixture, "list");
        let list_metaclass = class_of(&mut interpreter, &list);
        let derived = send(&mut interpreter, &fixture, "Derived");
        let derived_metaclass = class_of(&mut interpreter, &derived);
        for (class, expected) in [
            (&list, "true"),
            (&derived, "false"),
            (&derived_metaclass, "false"),
            (&list_metaclass, "true"),
        ] {
            let mixin = answer_of(&mut interpreter, "reflectionMixinOf:ifFail:", class, &[]);
            assert_eq!(
                rendered(
                    &mut interpreter,
                    "reflectionMixinIsShipped:ifFail:",
                    &mixin,
                    &[]
                ),
                expected
            );
        }
    }

    /// A module whose `future` answers a future of a `Plain`, which records
    /// in `ran` that its block ran.
    fn a_future_fixture() -> (Interpreter, Root) {
        let source = indoc! {"
            class FutureFixture usingPlatform: platform = Object new (
              |
                Future = platform kernel Future.
                public ran ::= false.
              |
            ) (
              public class Plain = () ()
              public future = (
                ^Future computing: [
                  ran:: true.
                  Plain new.
                ].
              )
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the fixture parses");
        let resolved = match crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("future_fixture.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the fixture resolves"),
        };
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let platform = interpreter.platform();
        let fixture = expect_ok(
            interpreter.instantiate(&class, &[platform]),
            "FutureFixture usingPlatform:",
        );
        (interpreter, fixture)
    }

    /// The fixture's `ran`, rendered.
    fn block_ran(interpreter: &mut Interpreter, fixture: &Root) -> String {
        let ran = send(interpreter, fixture, "ran");
        interpreter.enter(|mutator| render(mutator, mutator.fetch(&ran)))
    }

    /// A leaf reads a future as the object it is: one that declines for
    /// it declines, and its failure path forces nothing, so the future's
    /// block never runs. Whether it ran is asked after every leaf.
    #[test]
    fn no_leaf_forces_a_future() {
        let (mut interpreter, fixture) = a_future_fixture();
        let future = send(&mut interpreter, &fixture, "future");
        let name = a_symbol(&mut interpreter, "ran");
        let three = interpreter.stash(Value::small_integer(3));
        let cases: [(&str, &[Root], &str); 10] = [
            (
                "reflectionSlotOf:named:ifFail:",
                std::slice::from_ref(&name),
                "declined",
            ),
            ("reflectionSlotNamesOf:ifFail:", &[], "{}"),
            ("reflectionMixinOf:ifFail:", &[], "declined"),
            ("reflectionIsMetaclass:ifFail:", &[], "declined"),
            ("reflectionEnclosingObjectOf:ifFail:", &[], "declined"),
            ("reflectionMixinName:ifFail:", &[], "declined"),
            ("reflectionMixinClassSide:ifFail:", &[], "declined"),
            (
                "reflectionObject:identicalTo:ifFail:",
                std::slice::from_ref(&three),
                "false",
            ),
            (
                "reflectionObject:identicalTo:ifFail:",
                std::slice::from_ref(&future),
                "true",
            ),
            ("reflectionSymbolText:ifFail:", &[], "declined"),
        ];
        for (selector, arguments, expected) in cases {
            assert_eq!(
                rendered(&mut interpreter, selector, &future, arguments),
                expected,
                "{selector} on a future"
            );
            assert_eq!(
                block_ran(&mut interpreter, &fixture),
                "false",
                "{selector} ran the future's block"
            );
        }
        // A future as the argument: a slot's name that is no Symbol, and
        // the other object of an identity.
        let plain = send(&mut interpreter, &fixture, "Plain");
        let instance = send(&mut interpreter, &plain, "new");
        for (selector, subject, expected) in [
            ("reflectionSlotOf:named:ifFail:", &instance, "declined"),
            ("reflectionObject:identicalTo:ifFail:", &three, "false"),
        ] {
            assert_eq!(
                rendered(
                    &mut interpreter,
                    selector,
                    subject,
                    std::slice::from_ref(&future)
                ),
                expected,
                "{selector} given a future"
            );
        }
        assert_eq!(
            block_ran(&mut interpreter, &fixture),
            "false",
            "a leaf given a future ran its block"
        );
    }

    const FORWARD: &str = "elementsOf:forwardIdentityToElementsOf:ifFail:";

    /// A module for the bulk forward: `Point` and the wider `Wide`, each
    /// with a `sum` of its slots, `Failure`, a program's subclass of the
    /// kernel's `Error`, and `swap:`, which forwards a `Point` to a `Wide`
    /// between two rounds of the one send site `point sum`, through the VM
    /// mirror it is handed.
    fn a_forward_fixture(lowering: crate::Lowering) -> (Interpreter, Root) {
        let source = indoc! {"
            class Forwarding usingPlatform: platform = Object new (
              | public collections = platform collections. |
            ) (
              public class Point = (
                | public x ::= 1. public y ::= 2. |
              ) (
                public sum = (^x + y)
              )
              public class Wide = (
                | public x ::= 10. public y ::= 20. public z ::= 30. |
              ) (
                public sum = (^x + y + z)
              )
              public class Failure = Error (
                | public detail ::= 1. |
              ) ()
              public point = (^Point new)
              public wide = (^Wide new)
              public failure = (^Failure new)
              public closure = (^[1])
              public list = (^collections List new)
              public swap: mirror = (
                | point wide holder sums |
                point:: Point new.
                wide:: Wide new.
                holder:: {point}.
                sums:: Array new: 2.
                1 to: 2 do: [:index |
                  sums at: index put: point sum.
                  index = 1 ifTrue: [
                    mirror elementsOf: {point} forwardIdentityToElementsOf: {wide} ifFail: [^#declined].
                  ].
                ].
                ^{sums at: 1. sums at: 2. (holder at: 1) sum. point == wide}
              )
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the fixture parses");
        let resolved = match crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("forward_fixture.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the fixture resolves"),
        };
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        let platform = interpreter.platform();
        let fixture = expect_ok(
            interpreter.instantiate(&class, &[platform]),
            "Forwarding usingPlatform:",
        );
        (interpreter, fixture)
    }

    /// An `Array` of `elements`, built on the host side.
    fn an_array(interpreter: &mut Interpreter, elements: &[&Root]) -> Root {
        interpreter.enter(|mutator| {
            let values = elements
                .iter()
                .map(|element| mutator.fetch(element))
                .collect();
            let array = mutator.new_array(values);
            mutator.stash(Value::object(array))
        })
    }

    /// The VM mirror, read on the host side, where no program can reach it.
    fn the_vm_mirror(interpreter: &mut Interpreter) -> Root {
        interpreter.enter(|mutator| {
            let mirror = mutator.vm_mirror();
            mutator.stash(mirror)
        })
    }

    crate::for_each_lowering! {
        /// The method's register, the inlined block's use of it and the
        /// array element all name the `Wide` after the forward, and the one
        /// site `point sum`, which the first round filled for `Point`,
        /// answers `Wide`'s method in the second. That the forward vacates
        /// the caches this does not show: the `Wide`'s guard would miss the
        /// `Point` entry anyway.
        fn the_leaf_forwards_a_program_instance(lowering) {
            let (mut interpreter, fixture) = a_forward_fixture(lowering);
            let mirror = the_vm_mirror(&mut interpreter);
            let answer = expect_ok(
                interpreter.send(&fixture, Symbol::intern("swap:"), &[mirror]),
                "swap:",
            );
            let answer = interpreter.enter(|mutator| render(mutator, mutator.fetch(&answer)));
            assert_eq!(answer, "{3. 60. 60. true}");
        }
    }

    /// Every refused forward declines, and leaves both sides as they were:
    /// each old object is still itself, no root names its replacement, and
    /// each `Point` and `Wide` still answers its own `sum`.
    #[test]
    fn a_refused_forward_changes_nothing() {
        let (mut interpreter, fixture) = a_forward_fixture(crate::Lowering::Optimized);
        let point = send(&mut interpreter, &fixture, "point");
        let wide = send(&mut interpreter, &fixture, "wide");
        let other_wide = send(&mut interpreter, &fixture, "wide");
        let three = interpreter.stash(Value::small_integer(3));
        let text = a_string(&mut interpreter, "text");
        let one = interpreter.stash(Value::small_integer(1));
        let two = interpreter.stash(Value::small_integer(2));
        let literal_array = an_array(&mut interpreter, &[&one, &two]);
        let point_class = send(&mut interpreter, &fixture, "Point");
        let closure = send(&mut interpreter, &fixture, "closure");
        let list = send(&mut interpreter, &fixture, "list");
        let platform = interpreter.platform();
        let point_mixin = answer_of(
            &mut interpreter,
            "reflectionMixinOf:ifFail:",
            &point_class,
            &[],
        );
        let mirror = the_vm_mirror(&mut interpreter);
        let kernel = module_instance(&mut interpreter, "Kernel");
        let refused: [(&str, &Root); 10] = [
            ("a SmallInteger", &three),
            ("a String", &text),
            ("an Array", &literal_array),
            ("a class", &point_class),
            ("a closure", &closure),
            ("a shipped class's instance", &list),
            ("the platform", &platform),
            ("a reified mixin", &point_mixin),
            ("the VM mirror", &mirror),
            ("the kernel", &kernel),
        ];
        let mut cases: Vec<(String, Vec<&Root>, Vec<&Root>)> = vec![
            (
                "unequal sizes".to_string(),
                vec![&point],
                vec![&wide, &other_wide],
            ),
            (
                "a split".to_string(),
                vec![&point, &point],
                vec![&wide, &other_wide],
            ),
            (
                "a chain".to_string(),
                vec![&point, &wide],
                vec![&wide, &other_wide],
            ),
        ];
        for (name, object) in refused {
            cases.push((format!("{name} as old"), vec![object], vec![&wide]));
            cases.push((format!("{name} as new"), vec![&point], vec![object]));
        }
        for (name, old, new) in cases {
            let old_array = an_array(&mut interpreter, &old);
            let new_array = an_array(&mut interpreter, &new);
            assert_eq!(
                rendered(&mut interpreter, FORWARD, &old_array, &[new_array]),
                "declined",
                "{name}"
            );
            for (index, element) in old.iter().enumerate() {
                let kept = interpreter.enter(|mutator| {
                    let Some(array) = mutator.fetch(&old_array).as_object() else {
                        panic!("the old side is an Array");
                    };
                    let Some(kept) = mutator.heap().array_element(array, index) else {
                        panic!("the old side keeps its elements");
                    };
                    mutator.stash(kept)
                });
                let same_value = interpreter.enter(|mutator| {
                    crate::builtins::object_class::identical(
                        mutator.heap(),
                        mutator.fetch(&kept),
                        mutator.fetch(element),
                    )
                });
                assert!(same_value, "{name}: element {index} is still itself");
                if let Some(replacement) = new.get(index) {
                    assert!(
                        !same(&mut interpreter, element, replacement),
                        "{name}: element {index} names its replacement"
                    );
                }
            }
            for (subject, expected) in [(&point, "3"), (&wide, "60"), (&other_wide, "60")] {
                let sum = send(&mut interpreter, subject, "sum");
                let sum = interpreter.enter(|mutator| render(mutator, mutator.fetch(&sum)));
                assert_eq!(sum, expected, "{name}: a sum changed");
            }
        }
    }

    /// A module for the recorded guard of the own-slot instructions: each
    /// class forwards its own instance, from one of its initializers,
    /// through the VM mirror it is handed, while the activation that
    /// initializes it still runs.
    fn a_guard_fixture(lowering: crate::Lowering) -> (Interpreter, Root) {
        let source = indoc! {"
            class Guarding = Object new () (
              public class Wide = (
                | public x ::= 10. public y ::= 20. public z ::= 30. |
              ) ()
              public class Shifting usingMirror: mirror into: target = (
                |
                public a = mirror isNil ifFalse: [mirror elementsOf: {self} forwardIdentityToElementsOf: {target} ifFail: [nil]].
                public b = 2.
                |
              ) ()
              public class Narrower usingMirror: mirror into: target = Shifting usingMirror: mirror into: target (
                | public c = 3. |
              ) ()
              public class Lazy = (
                |
                public mirror ::= nil.
                |
              ) (
                lazy public late = forwardAndAnswer.
                forwardAndAnswer = (
                  mirror elementsOf: {self} forwardIdentityToElementsOf: {Wide new} ifFail: [nil].
                  ^3
                )
              )
              public class Clause usingMirror: mirror into: target = (
                ||
                  public a = mirror elementsOf: {self} forwardIdentityToElementsOf: {target} ifFail: [nil].
                  public b = 2.
                ||
              ) ()
              public wide = (^Wide new)
              public aLazy = (^Lazy new)
              public shifting: mirror into: target = (^Shifting usingMirror: mirror into: target)
              public narrower = (^Narrower usingMirror: nil into: nil)
              public clause: mirror into: target = (^Clause usingMirror: mirror into: target)
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the fixture parses");
        let resolved = match crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("guard_fixture.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the fixture resolves"),
        };
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        let fixture = expect_ok(interpreter.instantiate(&class, &[]), "Guarding new");
        (interpreter, fixture)
    }

    /// The text of the Newspeak error `result` raised.
    fn error_text(result: HostResult, what: &str) -> String {
        match result {
            Ok(_) => panic!("{what}: answered instead of raising"),
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => error.message.to_string(),
        }
    }

    crate::for_each_lowering! {
        /// The initializer forwards its receiver to a `Wide` before it
        /// stores `a`: the store would write `Shifting`'s layout into a
        /// `Wide`, so it raises instead.
        fn an_initializer_whose_receiver_was_forwarded_raises(lowering) {
            let (mut interpreter, fixture) = a_guard_fixture(lowering);
            let mirror = the_vm_mirror(&mut interpreter);
            let wide = send(&mut interpreter, &fixture, "wide");
            let result =
                interpreter.send(&fixture, Symbol::intern("shifting:into:"), &[mirror, wide]);
            assert_eq!(
                error_text(result, "shifting:into:"),
                "an instance of Wide no longer has the slots of Shifting, which the running code was compiled for"
            );
        }
    }

    crate::for_each_lowering! {
        /// A lazy slot's getter forwards its receiver before it stores the
        /// slot: its `InitializeSlot` raises.
        fn a_lazy_getter_whose_receiver_was_forwarded_raises(lowering) {
            let (mut interpreter, fixture) = a_guard_fixture(lowering);
            let mirror = the_vm_mirror(&mut interpreter);
            let lazy = send(&mut interpreter, &fixture, "aLazy");
            expect_ok(
                interpreter.send(&lazy, Symbol::intern("mirror:"), &[mirror]),
                "mirror:",
            );
            let result = interpreter.send(&lazy, Symbol::intern("late"), &[]);
            assert_eq!(
                error_text(result, "late"),
                "an instance of Wide no longer has the slots of Lazy, which the running code was compiled for"
            );
        }
    }

    crate::for_each_lowering! {
        /// A simultaneous clause's first initializer forwards the receiver:
        /// what the clause reads or writes of its own slots afterwards
        /// raises.
        fn a_simultaneous_clause_whose_receiver_was_forwarded_raises(lowering) {
            let (mut interpreter, fixture) = a_guard_fixture(lowering);
            let mirror = the_vm_mirror(&mut interpreter);
            let wide = send(&mut interpreter, &fixture, "wide");
            let result =
                interpreter.send(&fixture, Symbol::intern("clause:into:"), &[mirror, wide]);
            assert_eq!(
                error_text(result, "clause:into:"),
                "an instance of Wide no longer has the slots of Clause, which the running code was compiled for"
            );
        }
    }

    crate::for_each_lowering! {
        /// Forwarded to another instance of its own class, the receiver
        /// still has every slot the initializer addresses: the initializer
        /// completes, on the replacement, which the factory answers.
        fn an_initializer_whose_receiver_was_forwarded_to_its_own_application_proceeds(lowering) {
            let (mut interpreter, fixture) = a_guard_fixture(lowering);
            let nothing = interpreter.stash(Value::nil());
            let twin = expect_ok(
                interpreter.send(
                    &fixture,
                    Symbol::intern("shifting:into:"),
                    &[nothing.clone(), nothing],
                ),
                "the twin",
            );
            let mirror = the_vm_mirror(&mut interpreter);
            let answer = expect_ok(
                interpreter.send(
                    &fixture,
                    Symbol::intern("shifting:into:"),
                    &[mirror, twin.clone()],
                ),
                "shifting:into:",
            );
            assert!(same(&mut interpreter, &answer, &twin));
            let second = send(&mut interpreter, &twin, "b");
            let second = interpreter.enter(|mutator| render(mutator, mutator.fetch(&second)));
            assert_eq!(second, "2");
        }
    }

    crate::for_each_lowering! {
        /// Forwarded to an instance of a subclass, the receiver's guard is
        /// another application's, but its chain still contains the level
        /// the initializer was entered at: the cold check passes, records
        /// the new guard, and the initializer completes on the replacement.
        fn an_initializer_whose_receiver_was_forwarded_to_a_subclass_instance_proceeds(lowering) {
            let (mut interpreter, fixture) = a_guard_fixture(lowering);
            let narrower = send(&mut interpreter, &fixture, "narrower");
            let mirror = the_vm_mirror(&mut interpreter);
            let answer = expect_ok(
                interpreter.send(
                    &fixture,
                    Symbol::intern("shifting:into:"),
                    &[mirror, narrower.clone()],
                ),
                "shifting:into:",
            );
            assert!(same(&mut interpreter, &answer, &narrower));
            for (selector, expected) in [("a", "nil"), ("b", "2"), ("c", "3")] {
                let slot = send(&mut interpreter, &narrower, selector);
                let slot = interpreter.enter(|mutator| render(mutator, mutator.fetch(&slot)));
                assert_eq!(slot, expected, "{selector}");
            }
        }
    }

    /// A module for the recorded guard of the private and super slot
    /// accessors: `Keeper` and `Derived` forward their own receiver through
    /// the VM mirror they are handed between two reads of a slot, and
    /// `Holder`'s one `read` site serves instances of three subclasses.
    fn an_accessor_guard_fixture(lowering: crate::Lowering) -> (Interpreter, Root) {
        let source = indoc! {"
            class Accessing = Object new () (
              public class Wide = (
                | public x ::= 10. public y ::= 20. public z ::= 30. |
              ) ()
              public class Keeper = (
                | private secret ::= 7. |
              ) (
                public peek: mirror = (
                  secret.
                  mirror elementsOf: {self} forwardIdentityToElementsOf: {Wide new} ifFail: [nil].
                  ^secret
                )
                public again: mirror = (
                  | answer |
                  1 to: 2 do: [:round |
                    answer:: secret.
                    mirror elementsOf: {self} forwardIdentityToElementsOf: {Wide new} ifFail: [nil]
                  ].
                  ^answer
                )
              )
              public class Base = (
                | public value ::= 7. |
              ) ()
              public class Derived = Base () (
                public value = (^0)
                public peek: mirror = (
                  super value.
                  mirror elementsOf: {self} forwardIdentityToElementsOf: {Wide new} ifFail: [nil].
                  ^super value
                )
              )
              public class Holder = (
                | private secret ::= 1. |
              ) (
                public read = (^secret)
                public store: value = (secret:: value)
              )
              public class Catcher = (
                | public doesNotUnderstand ::= nil. |
              ) ()
              public class Probe = Catcher () (
                public peek: mirror = (
                  mirror elementsOf: {self} forwardIdentityToElementsOf: {Wide new} ifFail: [nil].
                  ^super missing
                )
              )
              public class One = Holder () ()
              public class Two = Holder () ()
              public class Three = Holder () ()
              public keeper = (^Keeper new)
              public wide = (^Wide new)
              public probe = (^Probe new)
              public derived = (^Derived new)
              public one = (^One new store: 1)
              public two = (^Two new store: 2)
              public three = (^Three new store: 3)
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the fixture parses");
        let resolved = match crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("accessor_guard_fixture.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the fixture resolves"),
        };
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        let fixture = expect_ok(interpreter.instantiate(&class, &[]), "Accessing new");
        (interpreter, fixture)
    }

    crate::for_each_lowering! {
        /// The second read of a private slot finds its receiver forwarded
        /// to a `Wide`: reading `Keeper`'s field of a `Wide` would answer
        /// one of the `Wide`'s own slots, so it raises instead.
        fn a_private_slot_read_after_its_receiver_was_forwarded_raises(lowering) {
            let (mut interpreter, fixture) = an_accessor_guard_fixture(lowering);
            let keeper = send(&mut interpreter, &fixture, "keeper");
            let mirror = the_vm_mirror(&mut interpreter);
            let result = interpreter.send(&keeper, Symbol::intern("peek:"), &[mirror]);
            assert_eq!(
                error_text(result, "peek:"),
                "an instance of Wide no longer has the slots of Keeper, which the slot accessor secret addresses"
            );
        }
    }

    crate::for_each_lowering! {
        /// One private slot site reads `secret` of a `Keeper`, then, on the
        /// loop's second round, of the `Wide` its receiver was forwarded
        /// to: under the optimized lowering a cache hit whose recorded
        /// guard no longer matches, which raises as the uncached read does.
        fn a_private_slot_site_hit_after_its_receiver_was_forwarded_raises(lowering) {
            let (mut interpreter, fixture) = an_accessor_guard_fixture(lowering);
            let keeper = send(&mut interpreter, &fixture, "keeper");
            let mirror = the_vm_mirror(&mut interpreter);
            let result = interpreter.send(&keeper, Symbol::intern("again:"), &[mirror]);
            assert_eq!(
                error_text(result, "again:"),
                "an instance of Wide no longer has the slots of Keeper, which the slot accessor secret addresses"
            );
        }
    }

    crate::for_each_lowering! {
        /// The second `super value` finds its receiver forwarded to a
        /// `Wide`: it raises rather than read `Base`'s field of the `Wide`.
        fn a_super_slot_read_after_its_receiver_was_forwarded_raises(lowering) {
            let (mut interpreter, fixture) = an_accessor_guard_fixture(lowering);
            let derived = send(&mut interpreter, &fixture, "derived");
            let mirror = the_vm_mirror(&mut interpreter);
            let result = interpreter.send(&derived, Symbol::intern("peek:"), &[mirror]);
            assert_eq!(
                error_text(result, "peek:"),
                "an instance of Wide no longer has the slots of Base, which the slot accessor value addresses"
            );
        }
    }

    crate::for_each_lowering! {
        /// A `super` send nothing in S answers reaches S's slot handler,
        /// the setter of a slot named `doesNotUnderstand`, whose field is
        /// decided from S's chain: after its receiver was forwarded to a
        /// `Wide`, it raises rather than store the message into the
        /// `Wide`.
        fn a_super_slot_handler_after_its_receiver_was_forwarded_raises(lowering) {
            let (mut interpreter, fixture) = an_accessor_guard_fixture(lowering);
            let probe = send(&mut interpreter, &fixture, "probe");
            let mirror = the_vm_mirror(&mut interpreter);
            let result = interpreter.send(&probe, Symbol::intern("peek:"), &[mirror]);
            assert_eq!(
                error_text(result, "peek:"),
                "an instance of Wide no longer has the slots of Catcher, which the slot accessor doesNotUnderstand: addresses"
            );
        }
    }

    /// The uncached path, the reference lowering's oracle, always walks:
    /// a verified pair that wrongly says a `Wide` has `Keeper`'s slots --
    /// which only a cached check consults -- does not make it pass, and a
    /// walk of its own that passes records nothing.
    #[test]
    fn the_uncached_slot_check_walks_and_records_nothing() {
        let (mut interpreter, fixture) = an_accessor_guard_fixture(crate::Lowering::Reference);
        let keeper = send(&mut interpreter, &fixture, "keeper");
        let wide = send(&mut interpreter, &fixture, "wide");
        interpreter.enter(|mutator| {
            let keeper = mutator.fetch(&keeper);
            let wide = mutator.fetch(&wide);
            let keeper_object = keeper.as_object().expect("a Keeper is an object");
            let level = mutator
                .heap()
                .class_application_index(mutator.heap().object_class(keeper_object))
                .expect("Keeper's application");
            let wide_guard = mutator.heap().dispatch_id(wide);
            let keeper_guard = mutator.heap().dispatch_id(keeper);
            mutator.caches.record_slot_level(wide_guard, level);
            assert!(
                mutator.slot_level_holds(wide, level, crate::mixin::DispatchId::NONE),
                "a cached check consults the pair"
            );
            assert!(
                !mutator.slot_level_holds(wide, level, crate::mixin::DispatchId::VACANT),
                "the uncached check walks"
            );
            assert!(mutator.slot_level_holds(keeper, level, crate::mixin::DispatchId::VACANT));
            assert!(
                !mutator.caches.slot_level_verified(keeper_guard, level),
                "the uncached check records nothing"
            );
            assert!(mutator.caches.slot_level_verified(wide_guard, level));
        });
    }

    crate::for_each_lowering! {
        /// One private slot site sees instances of three subclasses in
        /// turn: the entry records the first receiver's guard, the others
        /// take the cold check, and every read answers its own receiver's
        /// slot.
        fn a_private_slot_site_with_subclass_receivers_answers_each_slot(lowering) {
            let (mut interpreter, fixture) = an_accessor_guard_fixture(lowering);
            let receivers = ["one", "two", "three"].map(|name| send(&mut interpreter, &fixture, name));
            for _ in 0..3 {
                for (receiver, expected) in receivers.iter().zip(["1", "2", "3"]) {
                    let answer = send(&mut interpreter, receiver, "read");
                    let answer = interpreter.enter(|mutator| render(mutator, mutator.fetch(&answer)));
                    assert_eq!(answer, expected);
                }
            }
        }
    }

    /// A module whose receivers are forwarded just before code that
    /// addresses their own slots is pushed, by a send that binds its method
    /// without looking at the receiver: `Lazy`'s private lexical send,
    /// `Derived`'s super send, and `Applied`'s initializer, whose `<:`
    /// clause's argument forwards the instance `Registering`'s initializer
    /// registered before `Mix`'s own part is pushed. `Host`'s
    /// nested class `Inner` has a superclass clause that forwards the
    /// enclosing object while the class is being created, when the `Host`
    /// was handed a mirror; `HostChild` inherits it, and `Inner`'s
    /// `pushing` answers the module instance its `outer` walk reaches.
    fn a_pushed_guard_fixture(lowering: crate::Lowering) -> (Interpreter, Root) {
        let source = indoc! {"
            class Pushing = Object new (
              |
              public mirror ::= nil.
              public target ::= nil.
              public registered ::= nil.
              |
            ) (
              public class Wide = (
                | public x ::= 10. public y ::= 20. public z ::= 30. |
              ) ()
              public class Falsy = (
                | public x ::= 10. public y ::= 20. public z ::= false. |
              ) ()
              public class Lazy = (
                | public mirror ::= nil. |
              ) (
                lazy private late = 3.
                public peek = (
                  mirror elementsOf: {self} forwardIdentityToElementsOf: {Wide new} ifFail: [nil].
                  ^late
                )
              )
              public class Base = () (
                lazy public late = 3.
              )
              public class Derived = Base () (
                public peek: mirror = (
                  mirror elementsOf: {self} forwardIdentityToElementsOf: {Wide new} ifFail: [nil].
                  ^super late
                )
              )
              public class Registering = (
                | public registeredSlot = outer Pushing register: self. |
              ) ()
              public class Mix withValue: value = (
                | public added = value. |
              ) ()
              public class Applied = Registering new <: Mix withValue: (outer Pushing forwardRegistered) () ()
              public class Host = (
                | public mirror ::= nil. public target ::= nil. |
              ) (
                public class Inner = swapAndObject () (
                  public pushing = (^outer Pushing)
                )
                swapAndObject = (
                  mirror isNil ifFalse: [
                    mirror elementsOf: {self} forwardIdentityToElementsOf: {target} ifFail: [nil].
                  ].
                  ^Object
                )
                public probe = (^Inner)
              )
              public class HostChild = Host () ()
              public class Twin = (
                | public mirror ::= nil. public target ::= nil. public peer ::= nil. |
              ) (
                public class Inner = clause () ()
                clause = (
                  peer isNil ifFalse: [peer Inner].
                  mirror isNil ifFalse: [
                    mirror elementsOf: {self} forwardIdentityToElementsOf: {target} ifFail: [nil].
                  ].
                  ^Object
                )
                public probe = (^Inner)
              )
              public twin = (^Twin new)
              public wide = (^Wide new)
              public falsy = (^Falsy new)
              public aLazy = (^Lazy new)
              public derived = (^Derived new)
              public host = (^Host new)
              public hostChild = (^HostChild new)
              public register: object = (
                registered:: object.
                ^1
              )
              public forwardRegistered = (
                mirror elementsOf: {registered} forwardIdentityToElementsOf: {target} ifFail: [nil].
                ^7
              )
              public applied = (^Applied new)
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the fixture parses");
        let resolved = match crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("pushed_guard_fixture.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the fixture resolves"),
        };
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        let fixture = expect_ok(interpreter.instantiate(&class, &[]), "Pushing new");
        (interpreter, fixture)
    }

    /// What `selector` answers of `receiver`, rendered.
    fn rendered_send(interpreter: &mut Interpreter, receiver: &Root, selector: &str) -> String {
        let answer = send(interpreter, receiver, selector);
        interpreter.enter(|mutator| render(mutator, mutator.fetch(&answer)))
    }

    crate::for_each_lowering! {
        /// `peek` forwards its receiver, then reads its private lazy slot
        /// by a lexical send, which binds the getter without looking at the
        /// receiver: the getter's push must not record the `Wide`'s guard
        /// as one that has `Lazy`'s slots, so its `ReturnSlotIfFilled`
        /// raises rather than answering the `Wide`'s `y`.
        fn a_private_lazy_getter_pushed_after_a_forward_raises(lowering) {
            let (mut interpreter, fixture) = a_pushed_guard_fixture(lowering);
            let mirror = the_vm_mirror(&mut interpreter);
            let lazy = send(&mut interpreter, &fixture, "aLazy");
            expect_ok(
                interpreter.send(&lazy, Symbol::intern("mirror:"), &[mirror]),
                "mirror:",
            );
            let result = interpreter.send(&lazy, Symbol::intern("peek"), &[]);
            assert_eq!(
                error_text(result, "peek"),
                "an instance of Wide no longer has the slots of Lazy, which the running code was compiled for"
            );
        }
    }

    crate::for_each_lowering! {
        /// `peek:` forwards its receiver, then reads the lazy slot its
        /// superclass declares by a super send, which binds the getter
        /// without looking at the receiver: the getter raises rather than
        /// answering the `Wide`'s `x`.
        fn a_super_lazy_getter_pushed_after_a_forward_raises(lowering) {
            let (mut interpreter, fixture) = a_pushed_guard_fixture(lowering);
            let mirror = the_vm_mirror(&mut interpreter);
            let derived = send(&mut interpreter, &fixture, "derived");
            let result = interpreter.send(&derived, Symbol::intern("peek:"), &[mirror]);
            assert_eq!(
                error_text(result, "peek:"),
                "an instance of Wide no longer has the slots of Base, which the running code was compiled for"
            );
        }
    }

    crate::for_each_lowering! {
        /// `Applied`'s `<:` clause message forwards the instance being
        /// created after `Registering`'s level is initialized and before
        /// `Mix`'s is pushed: `Mix`'s initializer raises rather than
        /// writing `added` over the `Wide`'s `y`.
        fn a_level_pushed_after_its_clause_forwarded_the_instance_raises(lowering) {
            let (mut interpreter, fixture) = a_pushed_guard_fixture(lowering);
            let mirror = the_vm_mirror(&mut interpreter);
            let wide = send(&mut interpreter, &fixture, "wide");
            expect_ok(
                interpreter.send(&fixture, Symbol::intern("mirror:"), &[mirror]),
                "mirror:",
            );
            expect_ok(
                interpreter.send(&fixture, Symbol::intern("target:"), std::slice::from_ref(&wide)),
                "target:",
            );
            let result = interpreter.send(&fixture, Symbol::intern("applied"), &[]);
            assert_eq!(
                error_text(result, "applied"),
                "an instance of Wide no longer has the slots of Mix, which the running code was compiled for"
            );
            assert_eq!(rendered_send(&mut interpreter, &wide, "y"), "20");
        }
    }

    /// A `Host` handed the VM mirror and `target`, which `Inner`'s
    /// superclass clause forwards it to.
    fn a_host_forwarding_to(interpreter: &mut Interpreter, fixture: &Root, target: &Root) -> Root {
        let mirror = the_vm_mirror(interpreter);
        let host = send(interpreter, fixture, "host");
        expect_ok(
            interpreter.send(&host, Symbol::intern("mirror:"), &[mirror]),
            "mirror:",
        );
        expect_ok(
            interpreter.send(
                &host,
                Symbol::intern("target:"),
                std::slice::from_ref(target),
            ),
            "target:",
        );
        host
    }

    crate::for_each_lowering! {
        /// `Inner`'s superclass clause forwards the enclosing `Host` to a
        /// `Wide`, which has no slot to cache `Inner` in: the creation
        /// raises rather than writing the class over the `Wide`'s `z`.
        fn a_nested_class_whose_enclosing_object_was_forwarded_raises(lowering) {
            let (mut interpreter, fixture) = a_pushed_guard_fixture(lowering);
            let wide = send(&mut interpreter, &fixture, "wide");
            let host = a_host_forwarding_to(&mut interpreter, &fixture, &wide);
            let result = interpreter.send(&host, Symbol::intern("probe"), &[]);
            assert_eq!(
                error_text(result, "probe"),
                "an instance of Wide no longer has the slots of Host, which the nested class Inner is cached in"
            );
            assert_eq!(rendered_send(&mut interpreter, &wide, "z"), "30");
        }
    }

    crate::for_each_lowering! {
        /// The same forward to an object whose field at the cache slot's
        /// index holds `false`, a value that is no marker: the end of the
        /// creation leaves that field as the program wrote it.
        fn a_forwarded_enclosing_object_keeps_a_false_field(lowering) {
            let (mut interpreter, fixture) = a_pushed_guard_fixture(lowering);
            let falsy = send(&mut interpreter, &fixture, "falsy");
            let host = a_host_forwarding_to(&mut interpreter, &fixture, &falsy);
            let result = interpreter.send(&host, Symbol::intern("probe"), &[]);
            error_text(result, "probe");
            assert_eq!(rendered_send(&mut interpreter, &falsy, "z"), "false");
        }
    }

    crate::for_each_lowering! {
        /// Forwarded to an instance of a subclass, the enclosing object
        /// still has the declaring level's slots: the class is created,
        /// cached in the replacement's slot, and answered again from there.
        fn a_nested_class_whose_enclosing_object_was_forwarded_to_a_subclass_instance_is_cached(lowering) {
            let (mut interpreter, fixture) = a_pushed_guard_fixture(lowering);
            let child = send(&mut interpreter, &fixture, "hostChild");
            let host = a_host_forwarding_to(&mut interpreter, &fixture, &child);
            let created = send(&mut interpreter, &host, "probe");
            let cached = send(&mut interpreter, &child, "probe");
            assert!(same(&mut interpreter, &created, &cached));
            assert_eq!(rendered_send(&mut interpreter, &cached, "name"), "'Inner'");
        }
    }

    crate::for_each_lowering! {
        /// The replacement has its own `Inner` cached already: the creation
        /// the forward interrupted answers that class rather than replacing
        /// it, so the replacement keeps one class for the declaration.
        fn a_nested_class_forwarded_to_an_object_with_its_class_cached_answers_that_class(lowering) {
            let (mut interpreter, fixture) = a_pushed_guard_fixture(lowering);
            let child = send(&mut interpreter, &fixture, "hostChild");
            let cached = send(&mut interpreter, &child, "probe");
            let host = a_host_forwarding_to(&mut interpreter, &fixture, &child);
            let created = send(&mut interpreter, &host, "probe");
            assert!(same(&mut interpreter, &created, &cached));
            let again = send(&mut interpreter, &child, "probe");
            assert!(same(&mut interpreter, &again, &cached));
        }
    }

    crate::for_each_lowering! {
        /// The first twin's `Inner` clause creates the second twin's `Inner`,
        /// whose clause forwards the second twin to the first: the inner
        /// creation now ends in the slot the outer one marked. It must leave
        /// that marker alone, so the second twin's access, a cycle through
        /// the first twin's creation, is still diagnosed rather than
        /// answering a class.
        fn a_cycle_after_a_forward_to_a_creation_underway_is_diagnosed(lowering) {
            let (mut interpreter, fixture) = a_pushed_guard_fixture(lowering);
            let mirror = the_vm_mirror(&mut interpreter);
            let first = send(&mut interpreter, &fixture, "twin");
            let second = send(&mut interpreter, &fixture, "twin");
            expect_ok(
                interpreter.send(&second, Symbol::intern("mirror:"), &[mirror]),
                "mirror:",
            );
            expect_ok(
                interpreter.send(&second, Symbol::intern("target:"), std::slice::from_ref(&first)),
                "target:",
            );
            expect_ok(
                interpreter.send(&first, Symbol::intern("peer:"), std::slice::from_ref(&second)),
                "peer:",
            );
            let result = interpreter.send(&first, Symbol::intern("probe"), &[]);
            assert_eq!(
                error_text(result, "probe"),
                "cyclic class definition: evaluating the superclass clause of class Inner \
                 requires class Inner itself"
            );
        }
    }

    crate::for_each_lowering! {
        /// The replacement belongs to another instance of the module, so the
        /// `Host` level of its chain is another class object, over the same
        /// application: the class created for it declares itself in that
        /// level, and its `outer` walk reaches the replacement's module
        /// instance, not the forwarded object's.
        fn a_nested_class_forwarded_to_another_module_instance_walks_its_chain(lowering) {
            let (mut interpreter, fixture) = a_pushed_guard_fixture(lowering);
            let module_class = interpreter.enter(|mutator| {
                let class = mutator.class_of(mutator.fetch(&fixture));
                mutator.stash(Value::object(class))
            });
            let other_fixture = send(&mut interpreter, &module_class, "new");
            let child = send(&mut interpreter, &other_fixture, "hostChild");
            let host = a_host_forwarding_to(&mut interpreter, &fixture, &child);
            let created = send(&mut interpreter, &host, "probe");
            let instance = send(&mut interpreter, &created, "new");
            let pushing = send(&mut interpreter, &instance, "pushing");
            assert!(same(&mut interpreter, &pushing, &other_fixture));
        }
    }

    /// An instance of a program's subclass of a shipped class is the
    /// program's: its class's own mixin is not shipped.
    #[test]
    fn a_program_subclass_of_a_shipped_class_is_forwarded() {
        let (mut interpreter, fixture) = a_forward_fixture(crate::Lowering::Optimized);
        let failure = send(&mut interpreter, &fixture, "failure");
        let replacement = send(&mut interpreter, &fixture, "failure");
        let old_array = an_array(&mut interpreter, &[&failure]);
        let new_array = an_array(&mut interpreter, &[&replacement]);
        assert_eq!(
            rendered(&mut interpreter, FORWARD, &old_array, &[new_array]),
            "nil"
        );
        assert!(same(&mut interpreter, &failure, &replacement));
    }

    const CHECK: &str = "reflectionMixin:checkEdit:ifFail:";
    const INSTALL: &str = "reflectionMixin:installEdit:ifFail:";

    /// An edit on the wire, `{ header. instanceOperations. classOperations }`,
    /// built on the host side: each operation's kind a Symbol, its argument a
    /// String for an addition and a Symbol for a removal.
    fn an_edit(
        interpreter: &mut Interpreter,
        header: Option<&str>,
        instance_side: &[(&str, &str)],
        class_side: &[(&str, &str)],
    ) -> Root {
        interpreter.enter(|mutator| {
            let header = match header {
                Some(text) => Value::object(mutator.new_string(text.to_string())),
                None => Value::nil(),
            };
            let mut elements = vec![header];
            for operations in [instance_side, class_side] {
                let mut pairs = Vec::new();
                for (kind, argument) in operations {
                    let kind_symbol = mutator.new_symbol(Symbol::intern(kind));
                    let argument = if kind.starts_with("remove") {
                        mutator.new_symbol(Symbol::intern(argument))
                    } else {
                        Value::object(mutator.new_string(argument.to_string()))
                    };
                    pairs.push(Value::object(
                        mutator.new_array(vec![kind_symbol, argument]),
                    ));
                }
                elements.push(Value::object(mutator.new_array(pairs)));
            }
            let edit = mutator.new_array(elements);
            mutator.stash(Value::object(edit))
        })
    }

    /// The reified mixin of the fixture's class `name`.
    fn fixture_mixin(interpreter: &mut Interpreter, fixture: &Root, name: &str) -> Root {
        let class = send(interpreter, fixture, name);
        answer_of(interpreter, "reflectionMixinOf:ifFail:", &class, &[])
    }

    /// How a leaf's answer ends for an accepted edit of the fixture's
    /// `Derived`: the declaration's text, then its header up to the
    /// members' parenthesis, the space before it included.
    const DERIVED_HEADER_ANSWERED: &str =
        ")'. 'public class Derived = Base (\n    | own ::= 1. private secret = 2. |\n  ) '}";

    /// The check leaf answers the declaration with the edit spliced in, and
    /// stages nothing: the mixin's members, its source and an existing
    /// instance are as before.
    #[test]
    fn the_check_leaf_answers_the_staged_declaration() {
        let (mut interpreter, fixture) = a_fixture();
        let mixin = fixture_mixin(&mut interpreter, &fixture, "Derived");
        let source_before = rendered(
            &mut interpreter,
            "reflectionMixinSource:ifFail:",
            &mixin,
            &[],
        );
        let edit = an_edit(
            &mut interpreter,
            None,
            &[("addMethod", "public d = ( ^4 )")],
            &[],
        );
        let answer = rendered(&mut interpreter, CHECK, &mixin, &[edit]);
        assert!(
            answer.starts_with("{true. 'public class Derived = Base ("),
            "{answer}"
        );
        assert!(answer.contains("public d = ( ^4 )"), "{answer}");
        assert!(answer.ends_with(DERIVED_HEADER_ANSWERED), "{answer}");
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinSource:ifFail:",
                &mixin,
                &[]
            ),
            source_before
        );
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinMethods:ifFail:",
                &mixin,
                &[]
            ),
            "{{#b. #public}. {#a. #protected}. {#c. #private}}"
        );
        let derived = send(&mut interpreter, &fixture, "derived");
        let result = interpreter.send(&derived, Symbol::intern("d"), &[]);
        error_text(result, "d");
    }

    /// The check leaf answers a refused edit's diagnostic, and the edit
    /// changes nothing: a shipped mixin, and an edit that does not parse,
    /// which the install leaf refuses alike.
    #[test]
    fn the_check_leaf_answers_a_diagnostic_and_changes_nothing() {
        let (mut interpreter, fixture) = a_fixture();
        let list = send(&mut interpreter, &fixture, "list");
        let list_mixin = answer_of(&mut interpreter, "reflectionMixinOf:ifFail:", &list, &[]);
        let edit = an_edit(
            &mut interpreter,
            None,
            &[("addMethod", "public d = ( ^4 )")],
            &[],
        );
        assert_eq!(
            rendered(&mut interpreter, CHECK, &list_mixin, &[edit]),
            "{false. 'List belongs to the shipped core library, which no MirrorBuilder edits'}"
        );
        let mixin = fixture_mixin(&mut interpreter, &fixture, "Derived");
        let broken = an_edit(
            &mut interpreter,
            None,
            &[("addMethod", "public b = ( ^ )")],
            &[],
        );
        for selector in [CHECK, INSTALL] {
            let answer = rendered(
                &mut interpreter,
                selector,
                &mixin,
                std::slice::from_ref(&broken),
            );
            assert!(
                answer.starts_with("{false. 'Derived's edited source does not parse: "),
                "{selector}: {answer}"
            );
        }
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinMethods:ifFail:",
                &mixin,
                &[]
            ),
            "{{#b. #public}. {#a. #protected}. {#c. #private}}"
        );
        let derived = send(&mut interpreter, &fixture, "derived");
        assert_eq!(rendered_send(&mut interpreter, &derived, "b"), "1");
    }

    /// The install leaf answers the installed declaration, and an instance
    /// made before answers the new method; a class side's subject edits
    /// its instance side's declaration.
    #[test]
    fn the_install_leaf_installs() {
        let (mut interpreter, fixture) = a_fixture();
        let derived = send(&mut interpreter, &fixture, "derived");
        let mixin = fixture_mixin(&mut interpreter, &fixture, "Derived");
        let edit = an_edit(
            &mut interpreter,
            None,
            &[("addMethod", "public b = ( ^5 )")],
            &[("addMethod", "public other = ( ^6 )")],
        );
        let answer = rendered(&mut interpreter, INSTALL, &mixin, &[edit]);
        assert!(
            answer.starts_with("{true. 'public class Derived = Base ("),
            "{answer}"
        );
        assert!(answer.contains("public b = ( ^5 )"), "{answer}");
        assert!(answer.ends_with(DERIVED_HEADER_ANSWERED), "{answer}");
        assert_eq!(rendered_send(&mut interpreter, &derived, "b"), "5");
        let class = send(&mut interpreter, &fixture, "Derived");
        assert_eq!(rendered_send(&mut interpreter, &class, "other"), "6");
        let metaclass = class_of(&mut interpreter, &class);
        let class_side = answer_of(
            &mut interpreter,
            "reflectionMixinOf:ifFail:",
            &metaclass,
            &[],
        );
        let edit = an_edit(&mut interpreter, None, &[("removeMethod", "b")], &[]);
        let answer = rendered(&mut interpreter, INSTALL, &class_side, &[edit]);
        assert!(
            answer.starts_with("{true. 'public class Derived"),
            "{answer}"
        );
        let result = interpreter.send(&derived, Symbol::intern("b"), &[]);
        error_text(result, "b");
    }

    /// Anything but a reified mixin and an edit on the wire is declined by
    /// both leaves, with nothing changed.
    #[test]
    fn a_malformed_edit_is_declined() {
        let (mut interpreter, fixture) = a_fixture();
        let mixin = fixture_mixin(&mut interpreter, &fixture, "Derived");
        let text = a_string(&mut interpreter, "public d = ( ^4 )");
        let unknown = an_edit(
            &mut interpreter,
            None,
            &[("addThing", "public d = ( ^4 )")],
            &[],
        );
        let lazy_on_the_class_side = an_edit(
            &mut interpreter,
            None,
            &[],
            &[("addLazySlot", "lazy d = 4.")],
        );
        let well_formed = an_edit(
            &mut interpreter,
            None,
            &[("addMethod", "public d = ( ^4 )")],
            &[],
        );
        let nothing = interpreter.stash(Value::nil());
        let class = send(&mut interpreter, &fixture, "Derived");
        let cases: [(&str, &Root, &Root); 5] = [
            ("a String for the edit", &mixin, &text),
            ("an unknown operation", &mixin, &unknown),
            (
                "a lazy slot on the class side",
                &mixin,
                &lazy_on_the_class_side,
            ),
            ("a nil subject", &nothing, &well_formed),
            ("a class for the subject", &class, &well_formed),
        ];
        for selector in [CHECK, INSTALL] {
            for (name, subject, edit) in cases {
                assert_eq!(
                    rendered(
                        &mut interpreter,
                        selector,
                        subject,
                        std::slice::from_ref(edit)
                    ),
                    "declined",
                    "{selector}: {name}"
                );
            }
        }
        assert_eq!(
            rendered(
                &mut interpreter,
                "reflectionMixinMethods:ifFail:",
                &mixin,
                &[]
            ),
            "{{#b. #public}. {#a. #protected}. {#c. #private}}"
        );
    }

    /// A module whose code installs edits of its own classes, through the
    /// VM mirror the host hands it, while that code runs.
    fn an_installing_fixture(lowering: crate::Lowering) -> (Interpreter, Root) {
        let source = indoc! {"
            class Installing = Object new () (
              public class Kept usingMirror: mirror = (
                |
                public first = mirror
                  reflectionMixin: (mirror reflectionMixinOf: self class ifFail: [nil])
                  installEdit: {nil. {{#addMethod. 'public added = ( ^#added )'}}. {}}
                  ifFail: [#declined].
                public second = 2.
                |
              ) ()
              public class Grown usingMirror: mirror = (
                |
                public first = mirror
                  reflectionMixin: (mirror reflectionMixinOf: self class ifFail: [nil])
                  installEdit: {'public class Grown usingMirror: mirror = (| public first = nil. public second = 2. public third = 3. |)'. {}. {}}
                  ifFail: [#declined].
                public second = 2.
                |
              ) ()
              public class Changing = () (
                public run: mirror = (
                  | answer |
                  answer:: mirror
                    reflectionMixin: (mirror reflectionMixinOf: self class ifFail: [nil])
                    installEdit: {nil. {{#addMethod. 'public run: mirror = ( ^#replaced )'}. {#addMethod. 'public helper = ( ^#newHelper )'}}. {}}
                    ifFail: [^#declined].
                  ^{#oldRemainder. helper. answer at: 1}
                )
                public helper = ( ^#oldHelper )
              )
              public kept: mirror = (^Kept usingMirror: mirror)
              public grown: mirror = (^Grown usingMirror: mirror)
              public changing = (^Changing new)
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("the fixture parses");
        let resolved = match crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("installing_fixture.ns")),
        ) {
            Ok(resolved) => resolved,
            Err(_) => panic!("the fixture resolves"),
        };
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        let fixture = expect_ok(interpreter.instantiate(&class, &[]), "Installing new");
        (interpreter, fixture)
    }

    crate::for_each_lowering! {
        /// The first slot's initializer installs a methods-only edit of its
        /// own class: the level the initializer was entered at has a
        /// successor with the same slots, so the store of `second` passes
        /// the own-slot check, and the instance has both slots and the new
        /// method.
        fn an_interrupted_initializer_runs_on_when_its_slots_stayed(lowering) {
            let (mut interpreter, fixture) = an_installing_fixture(lowering);
            let mirror = the_vm_mirror(&mut interpreter);
            let kept = expect_ok(
                interpreter.send(&fixture, Symbol::intern("kept:"), &[mirror]),
                "kept:",
            );
            let first = rendered_send(&mut interpreter, &kept, "first");
            assert!(
                first.starts_with("{true. 'public class Kept usingMirror: mirror"),
                "{first}"
            );
            assert_eq!(rendered_send(&mut interpreter, &kept, "second"), "2");
            assert_eq!(rendered_send(&mut interpreter, &kept, "added"), "#added");
        }
    }

    crate::for_each_lowering! {
        /// The first slot's initializer installs a header that adds a slot:
        /// the instance being initialized migrates to the new layout, and
        /// the initializer's next own-slot store raises rather than writing
        /// the old layout into it.
        fn an_interrupted_initializer_raises_when_its_slots_changed(lowering) {
            let (mut interpreter, fixture) = an_installing_fixture(lowering);
            let mirror = the_vm_mirror(&mut interpreter);
            let result = interpreter.send(&fixture, Symbol::intern("grown:"), &[mirror]);
            assert_eq!(
                error_text(result, "grown:"),
                "an instance of Grown no longer has the slots of Grown, which the running code was compiled for"
            );
        }
    }

    crate::for_each_lowering! {
        /// A method that installs a replacement of itself runs its old code
        /// to its end, while its send of a helper the same edit replaced
        /// answers the helper's new value; the next send runs the new code.
        fn a_method_that_installs_finishes_with_its_old_code(lowering) {
            let (mut interpreter, fixture) = an_installing_fixture(lowering);
            let mirror = the_vm_mirror(&mut interpreter);
            let changing = send(&mut interpreter, &fixture, "changing");
            let answer = expect_ok(
                interpreter.send(&changing, Symbol::intern("run:"), std::slice::from_ref(&mirror)),
                "run:",
            );
            let answer = interpreter.enter(|mutator| render(mutator, mutator.fetch(&answer)));
            assert_eq!(answer, "{#oldRemainder. #newHelper. true}");
            let again = expect_ok(
                interpreter.send(&changing, Symbol::intern("run:"), &[mirror]),
                "run:",
            );
            let again = interpreter.enter(|mutator| render(mutator, mutator.fetch(&again)));
            assert_eq!(again, "#replaced");
        }
    }
}

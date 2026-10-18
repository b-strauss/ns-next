//! The leaves the shipped mirrors module reads classes, mixins, methods
//! and objects through (`docs/design/liveness-and-reflection.md`,
//! "Mirrors that read"). A class leaf takes a class
//! or a metaclass, a mixin leaf a reified mixin (`Mutator::reified_mixin`),
//! and each declines -- answers `None`, so the failure block runs -- for
//! anything else; none raises. Two leaves serve the module beside them:
//! identity and a Symbol's text, which the kernel's rows answer too, but
//! seen through a future.
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

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;

use crate::builtins::primitive::ReflectionPrimitive;
use crate::heap::{Heap, MixinId};
use crate::mixin::{source_text, Mixin, MixinApplication, MixinMethod};
use crate::types::*;
use crate::{Mutator, Symbol};

pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: ReflectionPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
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
            let mixin = reified(mutator, receiver)?;
            Some(mixin_nested(mutator, &mixin))
        }
        ReflectionPrimitive::MixinAccess => {
            let mixin = reified(mutator, receiver)?;
            let access = instance_side(&mixin).map_or(mixin.access, |instance| instance.access);
            Some(access_symbol(mutator, access))
        }
        ReflectionPrimitive::MethodSource => method_source(mutator, receiver, arguments),
        ReflectionPrimitive::MixinSource => {
            let id = reified_id(mutator, receiver)?;
            let heap = &mutator.heap;
            let text = instance_side_id(heap, id).and_then(|instance| {
                let text = heap.mixin_module_text(instance)?;
                let declaration = heap.mixin(instance)?;
                source_text(&text.0, declaration.span).map(str::to_string)
            });
            Some(string_or_nil(mutator, text))
        }
        ReflectionPrimitive::MixinIsShipped => {
            let mixin = reified(mutator, receiver)?;
            Some(Value::boolean(is_shipped(&mixin)))
        }
        ReflectionPrimitive::Identical => Some(Value::boolean(
            crate::builtins::object_class::identical(&mutator.heap, receiver, *arguments.first()?),
        )),
        ReflectionPrimitive::SymbolText => {
            let symbol = mutator.heap.symbol_value(receiver.as_object()?)?;
            Some(Value::object(
                mutator.new_string(symbol.as_str().to_string()),
            ))
        }
    }
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
    if is_shipped(&mutator.heap.class_application(class)?.mixin) {
        return None;
    }
    let enclosing = mutator
        .heap
        .class_enclosing(class)
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
    arguments: &[Value<'gc>],
) -> Option<Value<'gc>> {
    let name = mutator.heap.symbol_value(arguments.first()?.as_object()?)?;
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

/// `nestedClasses` of a mixin: its nested mixins, reified, in declaration
/// order; none for a class side, which declares no class.
fn mixin_nested<'gc>(mutator: &mut Mutator<'gc>, mixin: &Mixin) -> Value<'gc> {
    let nested: Vec<Value<'gc>> = if mixin.is_class_side() {
        Vec::new()
    } else {
        mixin
            .nested
            .iter()
            .map(|nested| Value::object(mutator.reified_mixin(nested)))
            .collect()
    };
    Value::object(mutator.new_array(nested))
}

/// `source` of a method: the text of the declared method the argument, a
/// Symbol, names, from its access modifier to its closing parenthesis, or
/// `nil` when the mixin holds no text or the span does not lie within it.
/// Declined for a name that is no Symbol or names no declared method.
fn method_source<'gc>(
    mutator: &mut Mutator<'gc>,
    subject: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Value<'gc>> {
    let id = reified_id(mutator, subject)?;
    let mixin = mutator.heap.mixin(id)?;
    let selector = mutator.heap.symbol_value(arguments.first()?.as_object()?)?;
    let method = declared_methods(&mixin)
        .into_iter()
        .find(|method| method.selector == selector)?;
    // A class side's methods lie in its instance side's module, whose text
    // a declared class side shares.
    let heap = &mutator.heap;
    let text = heap.mixin_module_text(id).or_else(|| {
        instance_side_id(heap, id).and_then(|instance| heap.mixin_module_text(instance))
    });
    let source = text.and_then(|text| source_text(&text.0, method.code.source).map(str::to_string));
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
}

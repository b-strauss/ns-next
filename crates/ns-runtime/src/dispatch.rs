//! The three lookup algorithms and the kth-enclosing-object computation.
//! See `docs/design/dispatch.md`; cites `newspeak-spec.pdf`
//! §3.3, §5.5, §5.9, §5.10, §6.3.1.

use std::rc::Rc;

use ns_syntax::ast;

use crate::builtins::primitive::Primitive;
use crate::heap::Heap;
use crate::mixin::{MethodEntry, Mixin, MixinApplication, MixinMethod, SlotAccessor};
use crate::types::*;
use crate::Symbol;

pub enum Lookup<'gc> {
    Method(Rc<MixinMethod>, ClassReference<'gc>),
    /// One of the accessors a slot declaration defines (`newspeak-spec.pdf`
    /// §6.3.2), with the absolute field it reads or writes: the declaring
    /// application's `own_slot_base` plus the slot's mixin-relative index.
    SlotAccessor(SlotAccessor, usize),
    /// A nested class declaration, reached through the implicit accessor
    /// `newspeak-spec.pdf` §6 says every such declaration defines in its
    /// enclosing class. The `ClassReference` is the class object of the level
    /// that declares it -- the same "defining class" a `Lookup::Method`
    /// reports, so a caller can treat both arms uniformly; the nested class
    /// object itself does not exist until `interpreter::nested::nested_class`
    /// materializes it for a particular receiver.
    NestedClass(Rc<Mixin>, ClassReference<'gc>),
    /// A builtin's primitive, with the class object of the level whose
    /// method table holds it: a primitive that fails passes that level over,
    /// and the lookup continues from its superclass (`lookup_above`).
    Primitive(Primitive, ClassReference<'gc>),
    NotFound,
}

pub fn effective_access(access: Option<ast::AccessModifier>) -> ast::AccessModifier {
    access.unwrap_or(ast::AccessModifier::Protected)
}

/// The accessibility of the implicit accessor a nested declaration defines: §6,
/// "implicitly defines a method with the same name *and accessibility* in OC".
/// Stored on the mixin, already defaulted, because the image's mixin table has
/// to carry it: a decoded mixin has no `ast::ClassDeclaration` to read it back
/// off, and a lookup that guessed would answer `Protected` for a declaration
/// written `public class Inner = ...`.
pub fn nested_access(mixin: &Mixin) -> ast::AccessModifier {
    mixin.access
}

/// §3.3: the enclosing object at lexical `depth` for an activation of a
/// method defined in `defining_class` and running on `self_value`.
///
/// `depth` is *lexical*: the resolver fixed it against the chain of class
/// declarations open at the place the code was written, so it counts levels
/// out from the class that **defines** the running method -- which
/// `Frame.current_class` names -- and not from the class of whatever object
/// receives it. Those two are the same class only when the method is not
/// inherited across a nesting boundary; seeding the walk from the receiver
/// instead follows the receiver's own chain and lands on an object of an
/// unrelated class (or `nil`) for an inherited one.
///
/// So depth 0 is `self`, and depth k > 0 is reached by walking k levels of
/// the *declaration* chain out from `defining_class`, taking each level's
/// own enclosing object -- §3.3's "the object that received the message
/// that created the class object". `ClassData::declaring_class` is that
/// chain; the receiver's class chain is not, for the same reason the seed
/// is not.
///
/// A chain shorter than `depth` yields `nil` rather than the nearest
/// object it did reach, which would be dispatched against silently. Only
/// the outermost level ends a chain: a module root class, which no
/// declaration encloses, and a `None` `defining_class`, a top-level
/// activation. Both have a lexical chain of exactly one level, so no
/// positive depth is resolvable in either.
pub fn kth_enclosing_object<'gc>(
    heap: &Heap<'gc>,
    self_value: &Value<'gc>,
    defining_class: Option<&ClassReference<'gc>>,
    depth: u16,
) -> Value<'gc> {
    if depth == 0 {
        return *self_value;
    }
    let Some(mut level) = defining_class.copied() else {
        return Value::Nil;
    };
    let mut remaining = depth;
    loop {
        let Some(enclosing) = heap.class_enclosing(level) else {
            return Value::Nil;
        };
        remaining -= 1;
        if remaining == 0 {
            return Value::Object(enclosing);
        }
        // One step out along the declaration chain -- the class object of
        // the level that lexically declares `level`.
        let Some(next) = heap.class_declaring(level) else {
            return Value::Nil;
        };
        level = next;
    }
}

/// What a lookup does with a declaration of its selector found at one level,
/// decided by the declaration's accessibility.
enum Verdict {
    /// The declaration is the answer.
    Accept,
    /// The declaration is passed over; the search continues at the
    /// superclass.
    Skip,
    /// The search ends with no answer.
    Stop,
}

/// Which of the three lookups a walk is (`newspeak-spec.pdf` §5.5, §5.9,
/// §3.5) -- what it does with a declaration it finds at one level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// §5.5's lookupPublic: public accepted, protected stops, private skipped.
    Public,
    /// §5.9's lookupProtected: public and protected accepted, private skipped.
    Protected,
    /// §3.5's "defined for", `doesNotUnderstand:`'s: every declaration accepted.
    DefinedFor,
}

impl Rule {
    fn verdict(self, access: ast::AccessModifier) -> Verdict {
        match (self, access) {
            (Rule::DefinedFor, _)
            | (Rule::Public, ast::AccessModifier::Public)
            | (Rule::Protected, ast::AccessModifier::Public | ast::AccessModifier::Protected) => {
                Verdict::Accept
            }
            (Rule::Public, ast::AccessModifier::Protected) => Verdict::Stop,
            (Rule::Public | Rule::Protected, ast::AccessModifier::Private) => Verdict::Skip,
        }
    }
}

/// The class declaration at lexical depth `depth` from `defining_class` --
/// §5.9's `N`, "a class whose declaration D lexically encloses the outer
/// send", at the depth the resolver fixed. Depth 0 is `defining_class`
/// itself, the class that declares the running method; each further level
/// is one step out along the declaration chain, the chain
/// `kth_enclosing_object` walks. `None` where the chain is shorter than
/// `depth`, and for a top-level activation (no `defining_class`).
pub fn kth_lexical_class<'gc>(
    heap: &Heap<'gc>,
    defining_class: Option<ClassReference<'gc>>,
    depth: u16,
) -> Option<ClassReference<'gc>> {
    let mut level = defining_class?;
    for _ in 0..depth {
        level = heap.class_declaring(level)?;
    }
    Some(level)
}

/// §5.9: "If N declares a private method f named s then the value of the
/// send is the result of invoking f". Asks `lexical_class`'s *own* mixin
/// only -- a private member of a subclass or a superclass of N is not N's
/// -- and answers `NotFound` unless that mixin declares `selector` private.
/// A private nested class is a private member on the same footing (§6: its
/// accessor has "the same name and accessibility"). Primordial Soup's
/// `LexicalSend` asks the same one table (`MethodAt` on N's application).
pub fn private_member<'gc>(
    heap: &Heap<'gc>,
    lexical_class: ClassReference<'gc>,
    selector: Symbol,
) -> Lookup<'gc> {
    let Some(application) = heap.class_application(lexical_class) else {
        return Lookup::NotFound;
    };
    if let Some(entry) = application.mixin.methods.get(&selector) {
        return if entry.access() == ast::AccessModifier::Private {
            found_entry(entry, &application, lexical_class)
        } else {
            Lookup::NotFound
        };
    }
    match application
        .mixin
        .nested
        .iter()
        .find(|nested| nested.name == selector)
    {
        Some(nested) if nested_access(nested) == ast::AccessModifier::Private => {
            Lookup::NestedClass(nested.clone(), lexical_class)
        }
        _ => Lookup::NotFound,
    }
}

/// §5.5, ordinary (explicit-receiver) sends: lookupPublic(n, C) is "m, if the
/// mixin of C defines a method m with selector n and public access.
/// Undefined, if the mixin of C defines a method m with selector n and
/// protected access. lookupPublic(n, superclass(C)) otherwise." So a
/// `protected` declaration shadows an inherited `public` one and ends the
/// search, while a `private` one falls to the "otherwise" and is passed over
/// -- Primordial Soup's `OrdinarySendMiss` does the same.
pub fn lookup_public<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    selector: Symbol,
) -> Lookup<'gc> {
    lookup(heap, Rule::Public, class, selector)
}

/// §5.9's lookupProtected(n, C): "m, if the mixin of C defines a method m
/// with selector n and either public or protected access", and
/// lookupProtected(n, superclass(C)) otherwise -- a `private` declaration is
/// passed over. The lookup of a lexical send (§5.9) and of a super send
/// (§5.10).
pub fn lookup_protected<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    selector: Symbol,
) -> Lookup<'gc> {
    lookup(heap, Rule::Protected, class, selector)
}

/// The plain, access-blind "defined for" walk (§3.5): used by
/// `doesNotUnderstand:` resolution, which is exempt from every access check
/// (§5.5: "the #doesNotUnderstand: method defined for (3.5) the class of o").
pub fn lookup_defined_for<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    selector: Symbol,
) -> Lookup<'gc> {
    lookup(heap, Rule::DefinedFor, class, selector)
}

/// The lookup `rule` names, of `selector`, from `class`: `class`'s own level
/// first, then each superclass in turn.
pub fn lookup<'gc>(
    heap: &Heap<'gc>,
    rule: Rule,
    class: ClassReference<'gc>,
    selector: Symbol,
) -> Lookup<'gc> {
    walk(heap, class, selector, rule)
}

/// The same lookup, continued from the level above `level`: where a
/// primitive found at `level` that failed passes the send on
/// (`Mutator::settle`). `NotFound` above the root.
pub fn lookup_above<'gc>(
    heap: &Heap<'gc>,
    rule: Rule,
    level: ClassReference<'gc>,
    selector: Symbol,
) -> Lookup<'gc> {
    match heap.class_superclass(level) {
        Some(superclass) => lookup(heap, rule, superclass, selector),
        None => Lookup::NotFound,
    }
}

/// Walks `class`'s superclass chain for a declaration of `selector`, asking
/// `rule` what to do with each one it finds.
fn walk<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    selector: Symbol,
    rule: Rule,
) -> Lookup<'gc> {
    let mut level = level_of(heap, class);
    while let Some((level_class, application)) = level {
        // `level_of` established `level_class` names a class only for the first
        // iteration; every iteration after that reaches `level_class` through
        // `next_level`'s unverified class-object chain (see its doc), so a
        // hierarchy where that chain diverges from the application chain must
        // end the search here rather than read stale `data` off a `level_class`
        // that no longer names a class.
        let Some(superclass) = heap.class_lookup_step(level_class) else {
            return Lookup::NotFound;
        };
        let next = next_level(superclass, &application);
        if let Some(entry) = application.mixin.methods.get(&selector) {
            match rule.verdict(entry.access()) {
                Verdict::Accept => return found_entry(entry, &application, level_class),
                Verdict::Stop => return Lookup::NotFound,
                Verdict::Skip => {
                    level = next;
                    continue;
                }
            }
        }
        // §6: a nested declaration is a member of its enclosing class on
        // exactly the same footing as a method, so it is searched at the
        // same level and under the same access rules -- after the method
        // dictionary, because a method is the only thing that can shadow
        // an accessor the compiler generated implicitly. A linear scan of
        // `nested` costs nothing on the hot path: it runs only once the
        // method dictionary has already missed, and `nested` is empty for
        // every builtin and for most user classes.
        if let Some(nested) = application
            .mixin
            .nested
            .iter()
            .find(|nested| nested.name == selector)
            .cloned()
        {
            match rule.verdict(nested_access(&nested)) {
                Verdict::Accept => return Lookup::NestedClass(nested, level_class),
                Verdict::Stop => return Lookup::NotFound,
                Verdict::Skip => {
                    level = next;
                    continue;
                }
            }
        }
        level = next;
    }
    Lookup::NotFound
}

/// The answer a method-table entry found at `level_class`'s level gives: a
/// compiled method with that level as its defining class, a slot accessor
/// with its field resolved against that level's application -- the level
/// whose mixin declares the slot, so `own_slot_base` places it (§6.3.2, as
/// `InitializeSlot` places a slot's initial value) -- or a primitive with
/// that level, which its failure passes over.
fn found_entry<'gc>(
    entry: &MethodEntry,
    application: &MixinApplication,
    level_class: ClassReference<'gc>,
) -> Lookup<'gc> {
    match entry {
        MethodEntry::Compiled(method) => Lookup::Method(method.clone(), level_class),
        MethodEntry::SlotAccessor(accessor) => Lookup::SlotAccessor(
            *accessor,
            application.own_slot_base + accessor.own_index as usize,
        ),
        MethodEntry::Primitive(method) => Lookup::Primitive(method.primitive, level_class),
    }
}

/// One level of the two chains a lookup walks: the application supplies the
/// code, the class object supplies the identity `Frame.current_class` needs
/// for `super` and `outer`. Returns `None` for a non-class object, which is
/// what ends a walk that was handed one.
fn level_of<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
) -> Option<(ClassReference<'gc>, Rc<MixinApplication>)> {
    heap.class_application(class)
        .map(|application| (class, application))
}

/// Steps both chains together. Every construction site (`load_module`,
/// `build_builtin_classes`) links a class object's superclass exactly when
/// it links its application's superclass, so the two chains have the same
/// length; requiring both halves here means a hierarchy that somehow lost
/// one of them ends the search instead of reporting a `Lookup::Method`
/// whose class object comes from a different level than its code.
fn next_level<'gc>(
    superclass: Option<ClassReference<'gc>>,
    application: &Rc<MixinApplication>,
) -> Option<(ClassReference<'gc>, Rc<MixinApplication>)> {
    let superclass = superclass?;
    let super_application = application.superclass.clone()?;
    Some((superclass, super_application))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::primitive::{IntegerPrimitive, ObjectPrimitive, Primitive};
    use indoc::formatdoc;
    use std::collections::HashMap;

    /// A placeholder `ClassReference` whose own `.class` field points back to
    /// itself. `Object.class` is non-optional (`ClassReference`, not
    /// `Option<ClassReference>`), so constructing *any* `Object` requires
    /// already having a `ClassReference` in hand for its `class` field -- and
    /// these test classes are never linked into a real bootstrapped class
    /// hierarchy, so there is no other object to point at. None of these
    /// tests (nor `walk`/`lookup_*`) ever read a class object's own
    /// `.class` field -- `walk` only reads `.data` -- so any
    /// non-panicking placeholder is sufficient; a self-referential one
    /// is the simplest to build. The construction itself lives in
    /// `Heap::allocate_self_referential_class`.
    fn dummy_class<'gc>(heap: &mut Heap<'gc>) -> ClassReference<'gc> {
        heap.allocate_self_referential_class(ObjectData::Instance(vec![]))
    }

    fn class_with_method<'gc>(
        heap: &mut Heap<'gc>,
        name: &str,
        selector: &str,
        access: Option<ast::AccessModifier>,
        superclass: Option<ClassReference<'gc>>,
    ) -> ClassReference<'gc> {
        // Build a minimal MethodDeclaration via ns_syntax::parse of a throwaway
        // module and pull the method out of it -- avoids hand-constructing
        // deeply nested AST literals here.
        let access_keyword = access
            .map(|modifier| format!("{modifier:?} ").to_lowercase())
            .unwrap_or_default();
        let source = formatdoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              {access_keyword}{selector} = ( ^1 )
            ) : ()
        "};
        let module = ns_syntax::parse(&source).expect("parse");
        let method = module.root_class.instance_side.methods[0].clone();
        let mut methods = crate::hash::IdentityMap::default();
        methods.insert(
            Symbol::intern(selector.trim_end_matches(':')),
            MethodEntry::Compiled(Rc::new(MixinMethod::from_declaration(
                Rc::new(method),
                &Rc::new(HashMap::new()),
                crate::compiler::Lowering::Optimized,
            ))),
        );
        // The application chain must be linked wherever the class-object
        // chain is: `next_level` steps both, so a superclass linked on only
        // one of them would end the walk one level early and these tests
        // would pass for the wrong reason.
        let super_application = superclass
            .as_ref()
            .and_then(|superclass_class| heap.class_application(*superclass_class));
        let application = Rc::new(MixinApplication {
            mixin: Rc::new(crate::mixin::Mixin {
                methods,
                ..crate::mixin::Mixin::empty(Symbol::intern(name))
            }),
            superclass: super_application,
            nested_slot: crate::hash::IdentityMap::default(),
            slot_count: 0,
            own_slot_base: 0,
        });
        let class_of_class = dummy_class(heap);
        heap.allocate_object(
            class_of_class,
            ObjectData::Class(ClassData {
                application,
                enclosing: None,
                declaring_class: None,
                superclass,
                builtin: None,
                metaclass: None,
            }),
        )
    }

    #[test]
    fn lookup_public_finds_a_public_method() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = class_with_method(
                &mut mutator.heap,
                "X",
                "foo",
                Some(ast::AccessModifier::Public),
                None,
            );
            assert!(matches!(
                lookup_public(&mutator.heap, class, Symbol::intern("foo")),
                Lookup::Method(..)
            ));
        });
    }

    #[test]
    fn lookup_public_hard_stops_on_a_protected_declaration() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let base = class_with_method(
                &mut mutator.heap,
                "Base",
                "foo",
                Some(ast::AccessModifier::Public),
                None,
            );
            let derived = class_with_method(
                &mut mutator.heap,
                "Derived",
                "foo",
                Some(ast::AccessModifier::Protected),
                Some(base),
            );
            assert!(matches!(
                lookup_public(&mutator.heap, derived, Symbol::intern("foo")),
                Lookup::NotFound
            ));
        });
    }

    #[test]
    fn lookup_protected_skips_a_private_declaration_transparently() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let base = class_with_method(
                &mut mutator.heap,
                "Base",
                "foo",
                Some(ast::AccessModifier::Public),
                None,
            );
            let derived = class_with_method(
                &mut mutator.heap,
                "Derived",
                "foo",
                Some(ast::AccessModifier::Private),
                Some(base),
            );
            // lookup_protected on Derived's own `foo` (private) must not match,
            // but must continue to Base's public `foo`.
            assert!(matches!(
                lookup_protected(&mutator.heap, derived, Symbol::intern("foo")),
                Lookup::Method(..)
            ));
        });
    }

    #[test]
    fn default_access_is_protected() {
        assert_eq!(effective_access(None), ast::AccessModifier::Protected);
    }

    /// A builtin's own native is an entry of its own method table.
    #[test]
    fn a_builtins_own_primitive_is_found_at_its_own_level() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let integer = mutator.builtin_class(BuiltinClass::Integer);
            match lookup_public(&mutator.heap, integer, Symbol::intern("+")) {
                Lookup::Primitive(Primitive::Integer(IntegerPrimitive::Add), level) => {
                    assert!(level.same(integer), "found above Integer's own level")
                }
                _ => panic!("`3 + 4` must find Integer's own `+`"),
            }
        });
    }

    /// `Integer` has no `==`; the ordinary walk reaches `Object`'s, since
    /// `Integer`'s superclass is `Object` -- no hand-restored inheritance.
    #[test]
    fn a_primitive_a_builtin_lacks_is_found_at_objects_level() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let integer = mutator.builtin_class(BuiltinClass::Integer);
            let object = mutator.builtin_class(BuiltinClass::Object);
            match lookup_public(&mutator.heap, integer, Symbol::intern("==")) {
                Lookup::Primitive(Primitive::Object(ObjectPrimitive::Identical), level) => {
                    assert!(level.same(object))
                }
                _ => panic!("`3 == 3` must find Object's `==`"),
            }
        });
    }

    /// What a failed primitive passes to: the same lookup, from the level
    /// above the one that declared it.
    #[test]
    fn a_lookup_continued_above_a_level_passes_that_levels_entry_over() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let string = mutator.builtin_class(BuiltinClass::String);
            match lookup_above(&mutator.heap, Rule::Public, string, Symbol::intern("=")) {
                Lookup::Primitive(Primitive::Object(ObjectPrimitive::Equal), _) => {}
                _ => panic!("above String, `=` is Object's"),
            }
            let object = mutator.builtin_class(BuiltinClass::Object);
            assert!(matches!(
                lookup_above(&mutator.heap, Rule::Public, object, Symbol::intern("=")),
                Lookup::NotFound
            ));
        });
    }

    /// §5.5: `Object`'s `doesNotUnderstand:` is protected -- lookupPublic
    /// stops at it, lookupProtected and the "defined for" walk accept it.
    #[test]
    fn objects_does_not_understand_is_protected() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let integer = mutator.builtin_class(BuiltinClass::Integer);
            let selector = Symbol::DOES_NOT_UNDERSTAND;
            assert!(matches!(
                lookup_public(&mutator.heap, integer, selector),
                Lookup::NotFound
            ));
            assert!(matches!(
                lookup_protected(&mutator.heap, integer, selector),
                Lookup::Primitive(Primitive::Object(ObjectPrimitive::DoesNotUnderstand), _)
            ));
            assert!(matches!(
                lookup_defined_for(&mutator.heap, integer, selector),
                Lookup::Primitive(Primitive::Object(ObjectPrimitive::DoesNotUnderstand), _)
            ));
        });
    }
}

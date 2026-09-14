//! The three lookup algorithms and the kth-enclosing-object computation.
//! See `docs/design/dispatch.md`; cites `newspeak-spec.pdf`
//! §3.3, §5.5, §5.9, §5.10, §6.3.1.

use std::rc::Rc;

use ns_syntax::ast;

use crate::heap::Heap;
use crate::mixin::{Mixin, MixinApplication, MixinMethod};
use crate::types::*;
use crate::Symbol;

pub enum Lookup<'gc> {
    Method(Rc<MixinMethod>, ClassReference<'gc>),
    /// A nested class declaration, reached through the implicit accessor
    /// `newspeak-spec.pdf` §6 says every such declaration defines in its
    /// enclosing class. The `ClassReference` is the class object of the level
    /// that declares it -- the same "defining class" a `Lookup::Method`
    /// reports, so a caller can treat both arms uniformly; the nested class
    /// object itself does not exist until `interpreter::nested::nested_class`
    /// materializes it for a particular receiver.
    NestedClass(Rc<Mixin>, ClassReference<'gc>),
    Native(BuiltinClass),
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
    if let Some(method) = application.mixin.methods.get(&selector) {
        return if method.access == ast::AccessModifier::Private {
            Lookup::Method(method.clone(), lexical_class)
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
    walk(heap, class, selector, |access| match access {
        ast::AccessModifier::Public => Verdict::Accept,
        ast::AccessModifier::Protected => Verdict::Stop,
        ast::AccessModifier::Private => Verdict::Skip,
    })
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
    walk(heap, class, selector, |access| match access {
        ast::AccessModifier::Public | ast::AccessModifier::Protected => Verdict::Accept,
        ast::AccessModifier::Private => Verdict::Skip,
    })
}

/// The plain, access-blind "defined for" walk (§3.5): used by
/// `doesNotUnderstand:` resolution, which is exempt from every access check
/// (§5.5: "the #doesNotUnderstand: method defined for (3.5) the class of o").
pub fn lookup_defined_for<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    selector: Symbol,
) -> Lookup<'gc> {
    walk(heap, class, selector, |_| Verdict::Accept)
}

/// Walks `class`'s superclass chain for a declaration of `selector`, asking
/// `verdict` what to do with each one it finds.
fn walk<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    selector: Symbol,
    verdict: impl Fn(ast::AccessModifier) -> Verdict,
) -> Lookup<'gc> {
    let mut level = level_of(heap, class);
    while let Some((level_class, application)) = level {
        // `level_of` established `level_class` names a class only for the first
        // iteration; every iteration after that reaches `level_class` through
        // `next_level`'s unverified class-object chain (see its doc), so a
        // hierarchy where that chain diverges from the application chain must
        // end the search here rather than read stale `data` off a `level_class`
        // that no longer names a class.
        let Some((builtin, superclass)) = heap.class_lookup_step(level_class) else {
            return Lookup::NotFound;
        };
        let next = next_level(superclass, &application);
        if let Some(method) = application.mixin.methods.get(&selector).cloned() {
            match verdict(method.access) {
                Verdict::Accept => return Lookup::Method(method, level_class),
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
            match verdict(nested_access(&nested)) {
                Verdict::Accept => return Lookup::NestedClass(nested, level_class),
                Verdict::Stop => return Lookup::NotFound,
                Verdict::Skip => {
                    level = next;
                    continue;
                }
            }
        }
        if let Some(kind) = builtin {
            return Lookup::Native(kind);
        }
        level = next;
    }
    Lookup::NotFound
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

/// `name`'s absolute instance-field index, if any. No chain walk: the
/// application's `slot_index` already carries every inherited slot at its
/// absolute offset, with an own slot's entry overriding an inherited one of
/// the same name. There is no access check -- slot access is never
/// access-controlled the way a method send is, which is a known gap. Used
/// by `sends::decide_lexical_send` before falling through to
/// `lookup_defined_for`/`lookup_protected`.
pub fn lookup_slot<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    name: Symbol,
) -> Option<usize> {
    heap.class_application(class)?
        .slot_index
        .get(&name)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let mut methods = HashMap::new();
        methods.insert(
            Symbol::intern(selector.trim_end_matches(':')),
            Rc::new(MixinMethod::from_declaration(
                Rc::new(method),
                &Rc::new(HashMap::new()),
                crate::compiler::Lowering::Optimized,
            )),
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
            slot_index: HashMap::new(),
            nested_slot: HashMap::new(),
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
}

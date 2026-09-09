//! The three lookup algorithms and the kth-enclosing-object computation.
//! See the runtime spec's Dispatch section; cites `newspeak-spec.pdf`
//! §3.3, §5.5, §5.9, §5.10, §6.3.1.

use std::rc::Rc;

use ns_syntax::ast;

use crate::heap::Heap;
use crate::mixin::{Mixin, MixinApplication, MixinMethod};
use crate::types::*;
use crate::Symbol;

pub enum Lookup {
    Method(Rc<MixinMethod>, ClassRef),
    /// A nested class declaration, reached through the implicit accessor
    /// `newspeak-spec.pdf` §6 says every such declaration defines in its
    /// enclosing class. The `ClassRef` is the class object of the level that
    /// declares it -- the same "defining class" a `Lookup::Method` reports,
    /// so a caller can treat both arms uniformly; the nested class object
    /// itself does not exist until `interpreter::nested::nested_class`
    /// materializes it for a particular receiver.
    NestedClass(Rc<Mixin>, ClassRef),
    Native(BuiltinClass),
    NotFound,
}

pub fn effective_access(access: Option<ast::AccessModifier>) -> ast::AccessModifier {
    access.unwrap_or(ast::AccessModifier::Protected)
}

/// The accessibility of the implicit accessor a nested declaration defines:
/// §6, "implicitly defines a method with the same name *and accessibility*
/// in OC". Stored on the mixin, already defaulted, because the image's
/// mixin table has to carry it: a decoded mixin has no `ast::ClassDecl` to
/// read it back off, and a lookup that guessed would answer `Protected` for
/// a declaration written `public class Inner = ...`.
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
pub fn kth_enclosing_object(
    heap: &Heap,
    self_value: &Value,
    defining_class: Option<&ClassRef>,
    depth: u16,
) -> Value {
    if depth == 0 {
        return *self_value;
    }
    let Some(mut level) = defining_class.cloned() else {
        return Value::Nil;
    };
    let mut remaining = depth;
    loop {
        let Some(enclosing) = heap.class_enclosing(&level) else {
            return Value::Nil;
        };
        remaining -= 1;
        if remaining == 0 {
            return Value::Obj(enclosing);
        }
        // One step out along the declaration chain -- the class object of
        // the level that lexically declares `level`.
        let Some(next) = heap.class_declaring(&level) else {
            return Value::Nil;
        };
        level = next;
    }
}

/// §5.5: ordinary (explicit-receiver) sends. A `protected`/`private`
/// declaration shadows an inherited `public` one -- the search stops at
/// the first declaration found, matching or not.
pub fn lookup_public(heap: &Heap, class: &ClassRef, selector: Symbol) -> Lookup {
    walk(
        heap,
        class,
        selector,
        |access| access == ast::AccessModifier::Public,
        true,
    )
}

/// §5.9 (non-`private` half of a `LexicalSend`): matches `public` or
/// `protected`; a `private` declaration is transparently skipped, the walk
/// continues past it.
pub fn lookup_protected(heap: &Heap, class: &ClassRef, selector: Symbol) -> Lookup {
    walk(
        heap,
        class,
        selector,
        |access| access != ast::AccessModifier::Private,
        false,
    )
}

/// The plain, access-blind "defined for" walk (§3.5): used by `super`
/// sends (§5.10) and by `doesNotUnderstand:` resolution, which is exempt
/// from every access check (§5.5).
pub fn lookup_defined_for(heap: &Heap, class: &ClassRef, selector: Symbol) -> Lookup {
    walk(heap, class, selector, |_| true, false)
}

/// `hard_stop_on_mismatch`: `lookupPublic`'s distinguishing rule -- when
/// the mixin declares `selector` at all but the access predicate rejects
/// it, stop searching (return `NotFound`) instead of continuing up.
/// `lookupProtected`/`lookup_defined_for` never hard-stop: a declaration
/// that doesn't match the predicate is transparently skipped.
fn walk(
    heap: &Heap,
    class: &ClassRef,
    selector: Symbol,
    accept: impl Fn(ast::AccessModifier) -> bool,
    hard_stop_on_mismatch: bool,
) -> Lookup {
    let mut level = level_of(heap, class);
    while let Some((c, application)) = level {
        // `level_of` established `c` names a class only for the first
        // iteration; every iteration after that reaches `c` through
        // `next_level`'s unverified class-object chain (see its doc), so a
        // hierarchy where that chain diverges from the application chain
        // must end the search here rather than read stale `data` off a `c`
        // that no longer names a class.
        let Some((builtin, superclass)) = heap.class_lookup_step(&c) else {
            return Lookup::NotFound;
        };
        let next = next_level(superclass, &application);
        if let Some(method) = application.mixin.methods.get(&selector).cloned() {
            let access = method.access;
            return if accept(access) {
                Lookup::Method(method, c)
            } else if hard_stop_on_mismatch {
                Lookup::NotFound
            } else {
                level = next;
                continue;
            };
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
            let access = nested_access(&nested);
            return if accept(access) {
                Lookup::NestedClass(nested, c)
            } else if hard_stop_on_mismatch {
                Lookup::NotFound
            } else {
                level = next;
                continue;
            };
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
fn level_of(heap: &Heap, class: &ClassRef) -> Option<(ClassRef, Rc<MixinApplication>)> {
    heap.class_application(class)
        .map(|application| (*class, application))
}

/// Steps both chains together. Every construction site (`load_module`,
/// `build_builtin_classes`) links a class object's superclass exactly when
/// it links its application's superclass, so the two chains have the same
/// length; requiring both halves here means a hierarchy that somehow lost
/// one of them ends the search instead of reporting a `Lookup::Method`
/// whose class object comes from a different level than its code.
fn next_level(
    superclass: Option<ClassRef>,
    application: &Rc<MixinApplication>,
) -> Option<(ClassRef, Rc<MixinApplication>)> {
    let superclass = superclass?;
    let super_application = application.superclass.clone()?;
    Some((superclass, super_application))
}

/// `name`'s absolute instance-field index, if any. No chain walk: the
/// application's `slot_index` already carries every inherited slot at its
/// absolute offset, with an own slot's entry overriding an inherited one of
/// the same name. There is no access check -- slot access is never
/// access-controlled the way a method send is in Milestone 1; see Task 7's
/// design note on this known gap. Used by `eval.rs::dispatch_lexical_send`
/// before falling through to `lookup_defined_for`/`lookup_protected`.
pub fn lookup_slot(heap: &Heap, class: &ClassRef, name: Symbol) -> Option<usize> {
    heap.class_application(class)?
        .slot_index
        .get(&name)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A placeholder `ClassRef` whose own `.class` field points back to
    /// itself. `Object.class` is non-optional (`ClassRef`, not
    /// `Option<ClassRef>`), so constructing *any* `Object` requires
    /// already having a `ClassRef` in hand for its `class` field -- and
    /// these test classes are never linked into a real bootstrapped class
    /// hierarchy, so there is no other object to point at. None of these
    /// tests (nor `walk`/`lookup_*`) ever read a class object's own
    /// `.class` field -- `walk` only reads `.data` -- so any
    /// non-panicking placeholder is sufficient; a self-referential one
    /// is the simplest to build. The construction itself lives in
    /// `Heap::alloc_self_referential_class`.
    fn dummy_class(heap: &mut Heap) -> ClassRef {
        heap.alloc_self_referential_class(ObjectData::Instance(vec![]))
    }

    fn class_with_method(
        heap: &mut Heap,
        name: &str,
        selector: &str,
        access: Option<ast::AccessModifier>,
        superclass: Option<ClassRef>,
    ) -> ClassRef {
        // Build a minimal MethodDecl via ns_syntax::parse of a throwaway
        // module and pull the method out of it -- avoids hand-constructing
        // deeply nested AST literals here.
        let src =
            format!(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  {access_kw}{selector} = ( ^1 )\n) : ()",
            access_kw = access.map(|a| format!("{a:?} ").to_lowercase()).unwrap_or_default(),
        );
        let module = ns_syntax::parse(&src).expect("parse");
        let method = module.root_class.instance_side.methods[0].clone();
        let mut methods = HashMap::new();
        methods.insert(
            Symbol::intern(selector.trim_end_matches(':')),
            Rc::new(MixinMethod::from_decl(Rc::new(method))),
        );
        // The application chain must be linked wherever the class-object
        // chain is: `next_level` steps both, so a superclass linked on only
        // one of them would end the walk one level early and these tests
        // would pass for the wrong reason.
        let super_application = superclass.as_ref().and_then(|s| heap.class_application(s));
        let application = Rc::new(MixinApplication {
            mixin: Rc::new(crate::mixin::Mixin {
                methods,
                ..crate::mixin::Mixin::empty(Symbol::intern(name))
            }),
            superclass: super_application,
            slot_index: HashMap::new(),
            nested_slot: HashMap::new(),
            num_slots: 0,
            own_slot_base: 0,
        });
        let class_of_class = dummy_class(heap);
        heap.alloc_placeholder_object(
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
        let mut heap = Heap::new();
        let class = class_with_method(
            &mut heap,
            "X",
            "foo",
            Some(ast::AccessModifier::Public),
            None,
        );
        assert!(matches!(
            lookup_public(&heap, &class, Symbol::intern("foo")),
            Lookup::Method(..)
        ));
    }

    #[test]
    fn lookup_public_hard_stops_on_a_protected_declaration() {
        let mut heap = Heap::new();
        let base = class_with_method(
            &mut heap,
            "Base",
            "foo",
            Some(ast::AccessModifier::Public),
            None,
        );
        let derived = class_with_method(
            &mut heap,
            "Derived",
            "foo",
            Some(ast::AccessModifier::Protected),
            Some(base),
        );
        assert!(matches!(
            lookup_public(&heap, &derived, Symbol::intern("foo")),
            Lookup::NotFound
        ));
    }

    #[test]
    fn lookup_protected_skips_a_private_declaration_transparently() {
        let mut heap = Heap::new();
        let base = class_with_method(
            &mut heap,
            "Base",
            "foo",
            Some(ast::AccessModifier::Public),
            None,
        );
        let derived = class_with_method(
            &mut heap,
            "Derived",
            "foo",
            Some(ast::AccessModifier::Private),
            Some(base),
        );
        // lookup_protected on Derived's own `foo` (private) must not match,
        // but must continue to Base's public `foo`.
        assert!(matches!(
            lookup_protected(&heap, &derived, Symbol::intern("foo")),
            Lookup::Method(..)
        ));
    }

    #[test]
    fn default_access_is_protected() {
        assert_eq!(effective_access(None), ast::AccessModifier::Protected);
    }
}

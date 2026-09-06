//! The three lookup algorithms and the kth-enclosing-object computation.
//! See the runtime spec's Dispatch section; cites `newspeak-spec.pdf`
//! §3.3, §5.5, §5.9, §5.10, §6.3.1.

use std::rc::Rc;

use ns_syntax::ast;

use crate::mixin::MixinApplication;
use crate::types::*;
use crate::Symbol;

/// The behaviour half of a class object, or `None` if `class` does not
/// point at one. One accessor, so a construction site that needs a
/// superclass's application never has to re-derive the borrow-and-match.
pub fn class_application(class: &ClassRef) -> Option<Rc<MixinApplication>> {
    let borrowed = class.borrow();
    match &borrowed.data {
        ObjectData::Class(data) => Some(data.application.clone()),
        _ => None,
    }
}

pub enum Lookup {
    Method(Rc<ast::MethodDecl>, ClassRef),
    Native(BuiltinClass),
    NotFound,
}

pub fn effective_access(access: Option<ast::AccessModifier>) -> ast::AccessModifier {
    access.unwrap_or(ast::AccessModifier::Protected)
}

/// §3.3, simplified for Milestone 1 -- see the task's design note.
pub fn kth_enclosing_object(self_value: &Value, depth: u16) -> Value {
    let mut current = self_value.clone();
    for _ in 0..depth {
        let Value::Obj(obj) = &current else {
            return Value::Nil;
        };
        let enclosing = {
            let borrowed = obj.borrow();
            let (ObjectData::Instance(_) | ObjectData::Class(_)) = &borrowed.data else {
                return Value::Nil;
            };
            class_enclosing(&borrowed.class)
        };
        match enclosing {
            Some(next) => current = Value::Obj(next),
            None => return Value::Nil,
        }
    }
    current
}

fn class_enclosing(class: &ClassRef) -> Option<ObjRef> {
    let borrowed = class.borrow();
    match &borrowed.data {
        ObjectData::Class(data) => data.enclosing.clone(),
        _ => None,
    }
}

/// §5.5: ordinary (explicit-receiver) sends. A `protected`/`private`
/// declaration shadows an inherited `public` one -- the search stops at
/// the first declaration found, matching or not.
pub fn lookup_public(class: &ClassRef, selector: Symbol) -> Lookup {
    walk(
        class,
        selector,
        |access| access == ast::AccessModifier::Public,
        true,
    )
}

/// §5.9 (non-`private` half of a `LexicalSend`): matches `public` or
/// `protected`; a `private` declaration is transparently skipped, the walk
/// continues past it.
pub fn lookup_protected(class: &ClassRef, selector: Symbol) -> Lookup {
    walk(
        class,
        selector,
        |access| access != ast::AccessModifier::Private,
        false,
    )
}

/// The plain, access-blind "defined for" walk (§3.5): used by `super`
/// sends (§5.10) and by `doesNotUnderstand:` resolution, which is exempt
/// from every access check (§5.5).
pub fn lookup_defined_for(class: &ClassRef, selector: Symbol) -> Lookup {
    walk(class, selector, |_| true, false)
}

/// `hard_stop_on_mismatch`: `lookupPublic`'s distinguishing rule -- when
/// the mixin declares `selector` at all but the access predicate rejects
/// it, stop searching (return `NotFound`) instead of continuing up.
/// `lookupProtected`/`lookup_defined_for` never hard-stop: a declaration
/// that doesn't match the predicate is transparently skipped.
fn walk(
    class: &ClassRef,
    selector: Symbol,
    accept: impl Fn(ast::AccessModifier) -> bool,
    hard_stop_on_mismatch: bool,
) -> Lookup {
    let mut level = level_of(class);
    while let Some((c, application)) = level {
        let (builtin, superclass) = {
            let borrowed = c.borrow();
            match &borrowed.data {
                ObjectData::Class(data) => (data.builtin, data.superclass.clone()),
                _ => return Lookup::NotFound,
            }
        };
        let next = next_level(superclass, &application);
        if let Some(method) = application.mixin.methods.get(&selector).cloned() {
            let access = effective_access(access_of(&method));
            return if accept(access) {
                Lookup::Method(method, c)
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
fn level_of(class: &ClassRef) -> Option<(ClassRef, Rc<MixinApplication>)> {
    let borrowed = class.borrow();
    match &borrowed.data {
        ObjectData::Class(data) => Some((class.clone(), data.application.clone())),
        _ => None,
    }
}

/// Steps both chains together. Every construction site (`load_class`,
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

fn access_of(method: &ast::MethodDecl) -> Option<ast::AccessModifier> {
    method.access
}

/// `name`'s absolute instance-field index, if any. No chain walk: the
/// application's `slot_index` already carries every inherited slot at its
/// absolute offset, with an own slot's entry overriding an inherited one of
/// the same name. There is no access check -- slot access is never
/// access-controlled the way a method send is in Milestone 1; see Task 7's
/// design note on this known gap. Used by `eval.rs::dispatch_lexical_send`
/// before falling through to `lookup_defined_for`/`lookup_protected`.
pub fn lookup_slot(class: &ClassRef, name: Symbol) -> Option<usize> {
    let borrowed = class.borrow();
    match &borrowed.data {
        ObjectData::Class(data) => data.application.slot_index.get(&name).copied(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
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
    /// is the simplest to build.
    ///
    /// Safe Rust cannot construct a strong `Rc<RefCell<T>>` that already
    /// holds a strong `Rc` to itself in one expression (the value must
    /// exist before the `Rc` wrapping it can be cloned into its own
    /// field). This uses the same allocate-uninitialized-then-write
    /// technique as `types.rs`'s own `dummy_class` test helper: mint a
    /// second strong handle to the same not-yet-initialized allocation
    /// (balancing the strong count via `Rc::increment_strong_count`
    /// before calling `Rc::from_raw`), write the real value using that
    /// handle as `class`, then `assume_init`.
    fn dummy_class() -> ClassRef {
        let uninit: Rc<std::mem::MaybeUninit<RefCell<Object>>> = Rc::new_uninit();
        let raw_mut = Rc::as_ptr(&uninit) as *mut std::mem::MaybeUninit<RefCell<Object>>;
        let data_ptr = raw_mut as *const RefCell<Object>;
        // SAFETY: `data_ptr` points at the allocation `uninit` owns; we
        // bump its strong count by one before minting a second `Rc`
        // handle to it via `from_raw`, so the two handles (`uninit`, and
        // the `class` field written below) share a correctly balanced
        // count. `MaybeUninit<T>` is guaranteed to have the same layout
        // as `T`, so the pointer cast is sound; we never dereference
        // through either handle before the `write` below completes.
        unsafe { Rc::increment_strong_count(data_ptr) };
        let self_ref: ClassRef = unsafe { Rc::from_raw(data_ptr) };
        unsafe {
            (*raw_mut).write(RefCell::new(Object {
                class: self_ref,
                identity_hash: 0,
                data: ObjectData::Instance(vec![]),
            }));
        }
        // SAFETY: the write above just initialized the payload; `uninit`
        // is the allocation's original handle, now fully initialized.
        unsafe { uninit.assume_init() }
    }

    fn class_with_method(
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
            Rc::new(method),
        );
        // The application chain must be linked wherever the class-object
        // chain is: `next_level` steps both, so a superclass linked on only
        // one of them would end the walk one level early and these tests
        // would pass for the wrong reason.
        let super_application = superclass.as_ref().and_then(class_application);
        let application = Rc::new(MixinApplication {
            mixin: Rc::new(crate::mixin::Mixin {
                name: Symbol::intern(name),
                methods,
                instance_slots: vec![],
                init_exprs: vec![],
                factory: None,
                nested: vec![],
                class_side: None,
                decl: None,
            }),
            superclass: super_application,
            slot_index: HashMap::new(),
            num_slots: 0,
        });
        Rc::new(RefCell::new(Object {
            class: dummy_class(),
            identity_hash: 0,
            data: ObjectData::Class(ClassData {
                application,
                enclosing: None,
                superclass,
                builtin: None,
            }),
        }))
    }

    #[test]
    fn lookup_public_finds_a_public_method() {
        let class = class_with_method("X", "foo", Some(ast::AccessModifier::Public), None);
        assert!(matches!(
            lookup_public(&class, Symbol::intern("foo")),
            Lookup::Method(..)
        ));
    }

    #[test]
    fn lookup_public_hard_stops_on_a_protected_declaration() {
        let base = class_with_method("Base", "foo", Some(ast::AccessModifier::Public), None);
        let derived = class_with_method(
            "Derived",
            "foo",
            Some(ast::AccessModifier::Protected),
            Some(base),
        );
        assert!(matches!(
            lookup_public(&derived, Symbol::intern("foo")),
            Lookup::NotFound
        ));
    }

    #[test]
    fn lookup_protected_skips_a_private_declaration_transparently() {
        let base = class_with_method("Base", "foo", Some(ast::AccessModifier::Public), None);
        let derived = class_with_method(
            "Derived",
            "foo",
            Some(ast::AccessModifier::Private),
            Some(base),
        );
        // lookup_protected on Derived's own `foo` (private) must not match,
        // but must continue to Base's public `foo`.
        assert!(matches!(
            lookup_protected(&derived, Symbol::intern("foo")),
            Lookup::Method(..)
        ));
    }

    #[test]
    fn default_access_is_protected() {
        assert_eq!(effective_access(None), ast::AccessModifier::Protected);
    }
}

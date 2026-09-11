//! Implicit-receiver and `super` sends: `newspeak-spec.pdf` §5.7-§5.10.
//! The VM's `SendLexical` and `SendSuper` call these; an ordinary send is
//! `Interpreter::send`.

use ns_syntax::ast;

use crate::dispatch::{self, Lookup};
use crate::heap::ActivationRef;
use crate::types::*;
use crate::Symbol;

use super::{EvalResult, Interpreter, Signal};

/// A slot's getter (`x`, no colon) or setter (`x:`, exactly one trailing
/// colon and no others) shape, per `newspeak-spec.pdf` §6.3.2 -- a slot's
/// getter/setter are "implicitly defined by the slot declaration", not real
/// `MethodDecl`s in the class's mixin, so `dispatch_lexical_send` must
/// recognize and serve them directly rather than relying on method lookup
/// to ever find them. Returns `None` for a genuine multi-keyword selector
/// (e.g. `at:put:`), which can never denote a slot access.
fn slot_selector_shape(selector: Symbol) -> Option<(Symbol, bool)> {
    let s = selector.as_str();
    if !s.contains(':') {
        return Some((selector, false));
    }
    if s.ends_with(':') && s.matches(':').count() == 1 {
        return Some((Symbol::intern(&s[..s.len() - 1]), true));
    }
    None
}

/// What an implicit-receiver send resolves to on one class, per
/// `newspeak-spec.pdf` §5.9: a `private` declaration *at exactly this
/// class* wins first, because a private member is never overridden and so
/// there is no search; anything else goes through the ordinary
/// `lookup_protected` walk.
///
/// A `private class N` is a private declaration on exactly the same footing
/// (§6: the implicit accessor has the declaration's own accessibility), and
/// has to be caught by the same first step: `lookup_protected` skips a
/// private declaration transparently and keeps walking, so leaving the
/// nested case out would make `private class N` unreachable from the class
/// that declares it while `private n = (...)` is reachable.
///
/// One definition because `dispatch_lexical_send` asks it of two classes --
/// a class object's metaclass and then the receiver's own class.
fn lexical_lookup(heap: &crate::heap::Heap, class: ClassRef, selector: Symbol) -> Lookup {
    match dispatch::lookup_defined_for(heap, class, selector) {
        Lookup::Method(method, defining_class) if method.access == ast::AccessModifier::Private => {
            Lookup::Method(method, defining_class)
        }
        Lookup::NestedClass(mixin, declaring_class)
            if dispatch::nested_access(&mixin) == ast::AccessModifier::Private =>
        {
            Lookup::NestedClass(mixin, declaring_class)
        }
        _ => dispatch::lookup_protected(heap, class, selector),
    }
}

/// `newspeak-spec.pdf` §5.7-§5.9: `o` = the kth enclosing object of this
/// activation's defining level (`dispatch::kth_enclosing_object`);
/// dispatch privately (direct, non-virtual) if the target is `private` at
/// depth `k`, otherwise virtually via `lookup_protected` on `o`'s actual
/// class -- or, when `o` is a class object, on its metaclass first (§6.3).
pub(crate) fn dispatch_lexical_send(
    interp: &mut Interpreter,
    act: ActivationRef,
    depth: u16,
    selector: Symbol,
    args: Vec<Value>,
) -> EvalResult {
    // The lexical depth counts levels out from the class that *defines* this
    // method, so the walk needs this activation's `current_class`, not the
    // receiver's own class -- see `dispatch::kth_enclosing_object`.
    let (self_value, defining_class) = interp.heap.activation_receiver_and_current_class(act);
    let o =
        dispatch::kth_enclosing_object(&interp.heap, &self_value, defining_class.as_ref(), depth);
    let class = interp.class_of(&o);
    // A `LexicalSend` to a user-defined class's own slot must resolve here,
    // *before* the method/native lookups below -- `build_mixin` never puts a
    // slot's implicit getter/setter into the mixin's method table, so
    // `lookup_defined_for`/`lookup_protected` would never find one.
    // Milestone 1 known gap: unlike a method send, this is never
    // access-controlled by the slot's own declared modifier -- revisit if a
    // fixture needs it.
    if let Some((name, is_setter)) = slot_selector_shape(selector) {
        if let Some(index) = dispatch::lookup_slot(&interp.heap, class, name) {
            if let Value::Obj(obj) = &o {
                if is_setter {
                    let value = args.first().copied().unwrap_or(Value::Nil);
                    interp.heap.set_instance_field(*obj, index, value);
                    return Ok(o);
                } else if let Some(value) = interp.heap.instance_field(*obj, index) {
                    return Ok(value);
                }
            }
        }
    }
    // §6.3: a class object's class side is the behaviour of that class
    // object, so a send written inside a class-side method with no explicit
    // receiver has to search the metaclass before the class object's own
    // (native `Object`) behaviour -- the same order `Interpreter::send`
    // gives an explicit-receiver send. Without it a class side reaches only
    // what `lookup_public` on the metaclass happens to answer once the send
    // has fallen all the way through to `send`: its `protected` and
    // `private` members are unreachable, and any selector `Object` answers
    // natively silently wins over the class side's own declaration.
    let metaclass = match &o {
        Value::Obj(obj) if interp.heap.is_class(*obj) => interp.metaclass_of(*obj),
        _ => None,
    };
    // A metaclass that does not declare the selector falls through to the
    // class object's own behaviour, exactly as in `send`. Only a real
    // declaration stops the search: a metaclass chain carries no builtin
    // tag, so `Lookup::Native` can only come from the class-object chain
    // below and means the same thing either way.
    let lookup = metaclass
        .as_ref()
        .map(|metaclass| lexical_lookup(&interp.heap, *metaclass, selector))
        .filter(|found| matches!(found, Lookup::Method(..) | Lookup::NestedClass(..)))
        .unwrap_or_else(|| lexical_lookup(&interp.heap, class, selector));
    match lookup {
        Lookup::Method(method, defining_class) => {
            interp.invoke_method(o, method, defining_class, args)
        }
        Lookup::NestedClass(mixin, declaring_class) => {
            crate::interpreter::nested::nested_class(interp, &o, &mixin, declaring_class)
        }
        Lookup::Native(kind) => {
            match crate::builtins::dispatch_native(interp, kind, o, selector, &args) {
                Some(result) => result,
                None => interp.send(o, selector, args),
            }
        }
        Lookup::NotFound => interp.send(o, selector, args),
    }
}

/// `newspeak-spec.pdf` §5.10: starts at the superclass of the *current
/// activation's* `current_class`, not `self`'s own class. On success, the
/// new activation's `current_class` is `definingClass(selector, superclass)`
/// -- `invoke_method`'s `defining_class` argument (from `lookup_defined_for`)
/// already carries that.
pub(crate) fn dispatch_super_send(
    interp: &mut Interpreter,
    act: ActivationRef,
    selector: Symbol,
    args: Vec<Value>,
) -> EvalResult {
    let Some(current_class) = interp.heap.activation_current_class(act) else {
        return Err(Signal::Error(NewspeakError {
            selector,
            receiver: interp.heap.activation_receiver(act),
            message:
                "super send with no enclosing current_class (used outside a method activation)"
                    .to_string(),
        }));
    };
    let superclass = interp.heap.class_superclass(current_class);
    let Some(superclass) = superclass else {
        return Err(Signal::Error(NewspeakError {
            selector,
            receiver: interp.heap.activation_receiver(act),
            message: "super send with no superclass (current class is Object or Top)".to_string(),
        }));
    };
    let self_value = interp.heap.activation_receiver(act);
    match dispatch::lookup_defined_for(&interp.heap, superclass, selector) {
        Lookup::Method(method, defining_class) => {
            interp.invoke_method(self_value, method, defining_class, args)
        }
        // `super N` for a nested class the superclass declares. The
        // receiver stays `self` (§5.10), which is also the enclosing object
        // the accessor caches against -- so `super N` and `N` name the same
        // class object exactly when they resolve to the same declaration,
        // and different ones when a subclass redeclares `N`: the accessor
        // caches against the declaration's own hidden slot
        // (`MixinApplication::nested_slot`), not against its name.
        Lookup::NestedClass(mixin, declaring_class) => {
            crate::interpreter::nested::nested_class(interp, &self_value, &mixin, declaring_class)
        }
        Lookup::Native(kind) => {
            match crate::builtins::dispatch_native(interp, kind, self_value, selector, &args) {
                Some(result) => result,
                None => interp.send(self_value, selector, args),
            }
        }
        Lookup::NotFound => interp.send(self_value, selector, args),
    }
}

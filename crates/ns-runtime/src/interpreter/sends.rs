//! Implicit-receiver and `super` sends: `newspeak-spec.pdf` §5.7-§5.10.
//! The VM's `SendLexical` and `SendSuper` call these; an ordinary send is
//! `Interpreter::send`.

use crate::dispatch::{self, Lookup};
use crate::heap::ActivationReference;
use crate::types::*;
use crate::Symbol;

use super::{Mutator, Signal, Step, StepArguments};

/// A slot's getter (`x`, no colon) or setter (`x:`, exactly one trailing
/// colon and no others) shape, per `newspeak-spec.pdf` §6.3.2 -- a slot's
/// getter/setter are "implicitly defined by the slot declaration", not real
/// `MethodDeclaration`s in the class's mixin, so `decide_lexical_send` must
/// recognize and serve them directly rather than relying on method lookup
/// to ever find them. Returns `None` for a genuine multi-keyword selector
/// (e.g. `at:put:`), which can never denote a slot access.
fn slot_selector_shape(selector: Symbol) -> Option<(Symbol, bool)> {
    let text = selector.as_str();
    if !text.contains(':') {
        return Some((selector, false));
    }
    if text.ends_with(':') && text.matches(':').count() == 1 {
        return Some((Symbol::intern(&text[..text.len() - 1]), true));
    }
    None
}

/// `newspeak-spec.pdf` §5.7-§5.9: an implicit receiver send whose declaring
/// class N is at lexical depth `k` is `outer N m`. `enclosing` is the kth
/// enclosing object of this activation's defining level
/// (`dispatch::kth_enclosing_object`). §5.9: "If N declares a private method
/// f named s then the value of the send is the result of invoking f ... on
/// o" -- asked of N's own mixin alone (`dispatch::private_member`), since a
/// private member is never overridden and there is nothing to search.
/// "Otherwise, Let f = lookupProtected(s, R) where R is the class of o" --
/// virtual, on `enclosing`'s actual class, and, when `enclosing` is a class
/// object, on its metaclass first (§6.3).
pub(crate) fn decide_lexical_send<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    depth: u16,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    // The lexical depth counts levels out from the class that *defines* this
    // method, so the walk needs this activation's `current_class`, not the
    // receiver's own class -- see `dispatch::kth_enclosing_object`.
    let (self_value, defining_class) = mutator
        .heap
        .activation_receiver_and_current_class(activation);
    let enclosing =
        dispatch::kth_enclosing_object(&mutator.heap, &self_value, defining_class.as_ref(), depth);
    let class = mutator.class_of(&enclosing);
    // A `LexicalSend` to a user-defined class's own slot must resolve here,
    // *before* the method/native lookups below -- `build_mixin` never puts a
    // slot's implicit getter/setter into the mixin's method table, so
    // `lookup_defined_for`/`lookup_protected` would never find one.
    // Known gap: unlike a method send, this is never
    // access-controlled by the slot's own declared modifier -- revisit if a
    // fixture needs it.
    if let Some((name, is_setter)) = slot_selector_shape(selector) {
        if let Some(index) = dispatch::lookup_slot(&mutator.heap, class, name) {
            if let Value::Object(object) = &enclosing {
                if is_setter {
                    let value = arguments.first().copied().unwrap_or(Value::Nil);
                    mutator.heap.set_instance_field(*object, index, value);
                    return Ok(Step::Value(enclosing));
                } else if let Some(value) = mutator.heap.instance_field(*object, index) {
                    return Ok(Step::Value(value));
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
    let metaclass = match &enclosing {
        Value::Object(object) if mutator.heap.is_class(*object) => mutator.metaclass_of(*object),
        _ => None,
    };
    // A metaclass that does not declare the selector falls through to the
    // class object's own behaviour, exactly as in `send`. Only a real
    // declaration stops the search: a metaclass chain carries no builtin
    // tag, so `Lookup::Native` can only come from the class-object chain
    // below and means the same thing either way.
    let private = dispatch::kth_lexical_class(&mutator.heap, defining_class, depth)
        .map(|lexical_class| dispatch::private_member(&mutator.heap, lexical_class, selector))
        .filter(|found| !matches!(found, Lookup::NotFound));
    let lookup = private.unwrap_or_else(|| {
        metaclass
            .as_ref()
            .map(|metaclass| dispatch::lookup_protected(&mutator.heap, *metaclass, selector))
            .filter(|found| matches!(found, Lookup::Method(..) | Lookup::NestedClass(..)))
            .unwrap_or_else(|| dispatch::lookup_protected(&mutator.heap, class, selector))
    });
    match lookup {
        Lookup::Method(method, defining_class) => Ok(Step::Method {
            receiver: enclosing,
            method,
            defining_class,
            arguments: StepArguments::Send,
        }),
        Lookup::NestedClass(mixin, declaring_class) => {
            crate::interpreter::nested::nested_class(mutator, &enclosing, &mixin, declaring_class)
        }
        Lookup::Native(kind) => {
            match crate::builtins::dispatch_native(mutator, kind, enclosing, selector, arguments) {
                Some(result) => result,
                None => mutator.decide_send(enclosing, selector, arguments),
            }
        }
        Lookup::NotFound => mutator.decide_send(enclosing, selector, arguments),
    }
}

/// `newspeak-spec.pdf` §5.10: starts at S, the superclass of the *current
/// activation's* `current_class`, not `self`'s own class. "If a public or
/// protected method f named s is defined for (3.5) S, the value of the send
/// is the result of invoking f in response to (3.5) message µ on o" -- so the
/// walk is `lookup_protected`, which passes a private method over, as
/// Primordial Soup's `SuperSendMiss` does with its `ProtectedSend`. On
/// success, the new activation's `current_class` is
/// `definingClass(selector, S)`, which the lookup's defining class carries.
/// "Otherwise, the #doesNotUnderstand: method defined for S is invoked":
/// `Mutator::decide_super_does_not_understand`.
pub(crate) fn decide_super_send<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let Some(current_class) = mutator.heap.activation_current_class(activation) else {
        return Err(Signal::Error(NewspeakError {
            selector,
            receiver: mutator.heap.activation_receiver(activation),
            message:
                "super send with no enclosing current_class (used outside a method activation)"
                    .to_string(),
        }));
    };
    let superclass = mutator.heap.class_superclass(current_class);
    let Some(superclass) = superclass else {
        return Err(Signal::Error(NewspeakError {
            selector,
            receiver: mutator.heap.activation_receiver(activation),
            message: "super send with no superclass (current class is Object or Top)".to_string(),
        }));
    };
    let self_value = mutator.heap.activation_receiver(activation);
    match dispatch::lookup_protected(&mutator.heap, superclass, selector) {
        Lookup::Method(method, defining_class) => Ok(Step::Method {
            receiver: self_value,
            method,
            defining_class,
            arguments: StepArguments::Send,
        }),
        // `super N` for a nested class the superclass declares. The
        // receiver stays `self` (§5.10), which is also the enclosing object
        // the accessor caches against -- so `super N` and `N` name the same
        // class object exactly when they resolve to the same declaration,
        // and different ones when a subclass redeclares `N`: the accessor
        // caches against the declaration's own hidden slot
        // (`MixinApplication::nested_slot`), not against its name.
        Lookup::NestedClass(mixin, declaring_class) => {
            crate::interpreter::nested::nested_class(mutator, &self_value, &mixin, declaring_class)
        }
        Lookup::Native(kind) => {
            match mutator.dispatch_native_inheriting_object(kind, &self_value, selector, arguments)
            {
                Some(result) => result,
                None => mutator
                    .decide_super_does_not_understand(self_value, superclass, selector, arguments),
            }
        }
        Lookup::NotFound => {
            mutator.decide_super_does_not_understand(self_value, superclass, selector, arguments)
        }
    }
}

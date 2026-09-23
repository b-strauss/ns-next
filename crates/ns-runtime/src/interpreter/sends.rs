//! Implicit-receiver and `super` sends, uncached: `newspeak-spec.pdf`
//! §5.7-§5.10. The VM's `SendLexical` and `SendSuper` reach them through
//! `inline_cache::send_lexical` and `send_super` whenever a send runs
//! uncached; an ordinary send's uncached counterpart is
//! `Mutator::decide_send`.

use crate::dispatch;
use crate::heap::ActivationReference;
use crate::types::*;
use crate::Symbol;

use super::{Mutator, Signal, Step};

/// `newspeak-spec.pdf` §5.7-§5.9: an implicit receiver send whose declaring
/// class N is at lexical depth `k` is `outer N m`. `enclosing` is the kth
/// enclosing object of this activation's defining level
/// (`dispatch::kth_enclosing_object`). §5.9: "If N declares a private method
/// f named s then the value of the send is the result of invoking f ... on
/// o" -- asked of N's own mixin alone (`dispatch::private_member`), since a
/// private member is never overridden and there is nothing to search.
/// "Otherwise, Let f = lookupProtected(s, R) where R is the class of o" --
/// virtual, on `enclosing`'s actual class, and, when `enclosing` is a class
/// object, on its metaclass first (§6.3). A slot's accessors are found by
/// the same lookups as any method (§6.3.2), so a subclass that overrides one
/// is obeyed here too. The lookup is `Mutator::resolve_lexical`.
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
    let lexical_class = dispatch::kth_lexical_class(&mutator.heap, defining_class, depth);
    let resolved = mutator.resolve_lexical(enclosing, lexical_class, selector);
    mutator.act(resolved, enclosing, selector, arguments)
}

/// `newspeak-spec.pdf` §5.10: starts at S, the superclass of the *current
/// activation's* `current_class`, not `self`'s own class. "If a public or
/// protected method f named s is defined for (3.5) S, the value of the send
/// is the result of invoking f in response to (3.5) message µ on o" -- so the
/// walk is `lookup_protected`, which passes a private method over, as
/// Primordial Soup's `SuperSendMiss` does with its `ProtectedSend`. On
/// success, the new activation's `current_class` is
/// `definingClass(selector, S)`, which the lookup's defining class carries.
/// "Otherwise, the #doesNotUnderstand: method defined for S is invoked". The
/// lookup is `Mutator::resolve_super`.
///
/// `#[inline(never)]`: only an uncached `super` send comes here
/// (`inline_cache::send_super` is the cached one), and the run loop's
/// instruction step, which inlines every send, need not carry a copy of
/// its lookup. Inlined, it once made the step a function of its own, and
/// every instruction -- not only every send -- paid a call and a `Flow`
/// returned through memory, 6.6-13.6% on all seven benchmarks; `vm::run`
/// now forces the step inline.
#[inline(never)]
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
    let resolved = mutator.resolve_super(superclass, selector);
    mutator.act(resolved, self_value, selector, arguments)
}

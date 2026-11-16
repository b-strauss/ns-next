//! Implicit-receiver and `super` sends, uncached: `newspeak-spec.pdf`
//! §5.7-§5.10. The VM's `SendLexical` and `SendSuper` reach them through
//! `inline_cache::send_lexical` and `send_super` whenever a send runs
//! uncached; an ordinary send's uncached counterpart is
//! `Mutator::decide_send`.

use super::Arguments;
use crate::Symbol;
use crate::dispatch::{self, LexicalOrigin};
use crate::heap::{ActivationReference, Heap};
use crate::types::*;

use super::{Mutator, Signal, Step};

/// `activation`'s receiver and where its lexical walk starts: the one
/// place that decides it, for the uncached sends here and the cached ones
/// in `inline_cache`. The lexical depth counts levels out from the class
/// that *defines* the running code, so the walk starts at the
/// activation's `current_class`, not the receiver's own class -- see
/// `dispatch::kth_enclosing_object` -- except where the running code is a
/// body-less declaration's own code (`declaration_code`, which the site
/// reads off its body: `CompiledCode::is_declaration_code`): that code
/// was written in the declaration, whose scope its class object records
/// (`Heap::class_declaration_scope`), while the class object's own
/// enclosing object and declaring class are the named class's, which that
/// class's methods walk out from (§6.2; `newspeak-spec.pdf` §3.3, §5.9).
/// Decided from the code, which runs nowhere but at its declaration's
/// class object, so every other send reads nothing more than the
/// activation's receiver and class.
#[inline(always)]
pub(crate) fn lexical_origin<'gc>(
    heap: &Heap<'gc>,
    activation: ActivationReference,
    declaration_code: bool,
) -> (Value<'gc>, LexicalOrigin<'gc>) {
    let (self_value, defining_class) = heap.activation_receiver_and_current_class(activation);
    match defining_class {
        Some(class) if declaration_code => {
            (self_value, declaration_origin(heap, activation, class))
        }
        _ => {
            debug_assert!(
                !defining_class.is_some_and(|class| runs_declaration_code(heap, activation, class)),
                "internal: a body-less declaration's own code not marked as such"
            );
            (self_value, LexicalOrigin::Class(defining_class))
        }
    }
}

/// `lexical_origin` for a body-less declaration's own code running at its
/// class object `class`, out of line: only such code comes here.
#[cold]
#[inline(never)]
fn declaration_origin<'gc>(
    heap: &Heap<'gc>,
    activation: ActivationReference,
    class: ClassReference<'gc>,
) -> LexicalOrigin<'gc> {
    debug_assert!(
        runs_declaration_code(heap, activation, class),
        "internal: code marked as a body-less declaration's own runs elsewhere"
    );
    match heap.class_declaration_scope(class) {
        Some(scope) => LexicalOrigin::Declaration(scope),
        None => LexicalOrigin::Class(Some(class)),
    }
}

/// §5.10's S for a `super` send in an activation whose current class is
/// `current_class`: "let *S* be the superclass of *C*". The one place
/// that decides it, for `decide_super_send` and
/// `inline_cache::send_super`. A body-less declaration's own code is no
/// exception: it runs at its class object, which is the class *I* §6.2
/// makes, so its S is that object's superclass, as for every other code
/// (`newspeak-spec.pdf` §3.6, §5.10, §6.2). `None` where there is no
/// superclass.
#[inline(always)]
pub(crate) fn super_start<'gc>(
    heap: &Heap<'gc>,
    current_class: ClassReference<'gc>,
) -> Option<ClassReference<'gc>> {
    heap.class_superclass(current_class)
}

/// Whether `activation` runs a body-less declaration's own code at its
/// class object `class`, asked of the class object and the activation:
/// what a debug build holds the code's own answer to.
fn runs_declaration_code<'gc>(
    heap: &Heap<'gc>,
    activation: ActivationReference,
    class: ClassReference<'gc>,
) -> bool {
    heap.class_has_declaration_scope(class)
        && heap.activation_runs_declaration_code(activation, class)
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
/// object, on its metaclass first (§6.3). A slot's accessors are found by
/// the same lookups as any method (§6.3.2), so a subclass that overrides one
/// is obeyed here too. The lookup is `Mutator::resolve_lexical`.
pub(crate) fn decide_lexical_send<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    depth: u16,
    declaration_code: bool,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    let (self_value, origin) = lexical_origin(&mutator.heap, activation, declaration_code);
    let enclosing = dispatch::kth_enclosing_object(&mutator.heap, self_value, origin, depth);
    let lexical_class = dispatch::kth_lexical_class(&mutator.heap, origin, depth);
    let resolved = mutator.resolve_lexical(enclosing, lexical_class, selector);
    mutator.act(resolved, enclosing, selector, arguments)
}

/// `outer N` as a value: the object `outer N m` sends `m` to, found the same
/// way `decide_lexical_send` finds it.
pub(crate) fn enclosing_object<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    depth: u16,
    declaration_code: bool,
) -> Value<'gc> {
    let (self_value, origin) = lexical_origin(&mutator.heap, activation, declaration_code);
    dispatch::kth_enclosing_object(&mutator.heap, self_value, origin, depth)
}

/// `newspeak-spec.pdf` §5.10: starts at S (`super_start`), the superclass of the *current
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
/// stretch, which inlines every send, need not carry a copy of its lookup.
/// Inlined, it once made the per-instruction step a function of its own,
/// and every instruction -- not only every send -- paid a call and a `Flow`
/// returned through memory, 6.6-13.6% on all seven benchmarks; `vm::run`
/// now forces its stretch inline.
#[inline(never)]
pub(crate) fn decide_super_send<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    let Some(current_class) = mutator.heap.activation_current_class(activation) else {
        return Err(Signal::Error(NewspeakError::error(
            selector,
            mutator.heap.activation_receiver(activation),
            "super send with no enclosing current_class (used outside a method activation)",
        )));
    };
    let superclass = super_start(&mutator.heap, current_class);
    let Some(superclass) = superclass else {
        return Err(Signal::Error(NewspeakError::error(
            selector,
            mutator.heap.activation_receiver(activation),
            "super send with no superclass (current class is Object or Top)",
        )));
    };
    let self_value = mutator.heap.activation_receiver(activation);
    let resolved = mutator.resolve_super(superclass, selector);
    mutator.check_super_handler_slot(&resolved, self_value)?;
    mutator.act(resolved, self_value, selector, arguments)
}

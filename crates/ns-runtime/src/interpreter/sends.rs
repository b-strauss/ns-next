//! Implicit-receiver and `super` sends: `newspeak-spec.pdf` §5.7-§5.10.
//! The VM's `SendLexical` and `SendSuper` call these; an ordinary send is
//! `Interpreter::send`.

use crate::dispatch::{self, Lookup};
use crate::heap::ActivationReference;
use crate::types::*;
use crate::Symbol;

use super::{Mutator, Signal, Step, StepArguments};

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
/// is obeyed here too.
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
    // §6.3: a class object's class side is the behaviour of that class
    // object, so a send written inside a class-side method with no explicit
    // receiver has to search the metaclass before the class object's own
    // behaviour, `Object`'s primitives -- the same order `Interpreter::send`
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
    // declaration stops the search: no metaclass chain holds a primitive --
    // a metaclass's mixins are class sides or empty stand-ins -- so a
    // `Lookup::Primitive` can only come from the class-object chain below,
    // and the filter would pass one over to reach that chain anyway.
    let private = dispatch::kth_lexical_class(&mutator.heap, defining_class, depth)
        .map(|lexical_class| dispatch::private_member(&mutator.heap, lexical_class, selector))
        .filter(|found| !matches!(found, Lookup::NotFound));
    let lookup = private.unwrap_or_else(|| {
        metaclass
            .as_ref()
            .map(|metaclass| dispatch::lookup_protected(&mutator.heap, *metaclass, selector))
            .filter(|found| {
                matches!(
                    found,
                    Lookup::Method(..) | Lookup::SlotAccessor(..) | Lookup::NestedClass(..)
                )
            })
            .unwrap_or_else(|| dispatch::lookup_protected(&mutator.heap, class, selector))
    });
    decide_lexical_lookup(mutator, lookup, enclosing, selector, arguments)
}

/// What the lexical send's walk found, turned into the step it names. The
/// `Lookup` is matched as it is, rather than routed through a wider enum
/// that also carries a primitive's finished `Result<Step, Signal>`: see
/// `Mutator::decide_public_lookup`, which this mirrors, for what that
/// measured, and for why this is `#[inline(always)]`.
#[inline(always)]
fn decide_lexical_lookup<'gc>(
    mutator: &mut Mutator<'gc>,
    lookup: Lookup<'gc>,
    enclosing: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    match lookup {
        Lookup::Method(method, defining_class) => Ok(Step::Method {
            receiver: enclosing,
            method,
            defining_class,
            arguments: StepArguments::Send,
        }),
        Lookup::SlotAccessor(accessor, field) => {
            mutator.decide_slot_accessor(enclosing, accessor, field, arguments)
        }
        Lookup::NestedClass(mixin, declaring_class) => {
            crate::interpreter::nested::nested_class(mutator, &enclosing, &mixin, declaring_class)
        }
        Lookup::Primitive(primitive, level) => {
            match crate::builtins::execute(mutator, primitive, enclosing, selector, arguments) {
                Some(result) => result,
                None => decide_lexical_above(mutator, level, enclosing, selector, arguments),
            }
        }
        Lookup::NotFound => mutator.decide_send(enclosing, selector, arguments),
    }
}

/// Where a failed primitive sends a lexical send on: §5.9's lookupProtected,
/// continued from the level above the one that declared the primitive.
/// `#[cold]` for the reason `Mutator::decide_public_above` gives -- a
/// primitive fails only on a receiver whose shape it does not work on.
#[cold]
#[inline(never)]
fn decide_lexical_above<'gc>(
    mutator: &mut Mutator<'gc>,
    level: ClassReference<'gc>,
    enclosing: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let lookup = dispatch::lookup_above(&mutator.heap, dispatch::Rule::Protected, level, selector);
    decide_lexical_lookup(mutator, lookup, enclosing, selector, arguments)
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
    let lookup = dispatch::lookup_protected(&mutator.heap, superclass, selector);
    decide_super_lookup(mutator, lookup, self_value, superclass, selector, arguments)
}

/// What the `super` walk found, turned into the step it names. `superclass`
/// is S, carried along because §5.10's failure looks `doesNotUnderstand:` up
/// from S rather than from the receiver's own class. `#[inline(always)]` for
/// the reason `Mutator::decide_public_lookup` gives.
#[inline(always)]
fn decide_super_lookup<'gc>(
    mutator: &mut Mutator<'gc>,
    lookup: Lookup<'gc>,
    self_value: Value<'gc>,
    superclass: ClassReference<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    match lookup {
        Lookup::Method(method, defining_class) => Ok(Step::Method {
            receiver: self_value,
            method,
            defining_class,
            arguments: StepArguments::Send,
        }),
        // `super x` reaches the superclass's accessor even where the
        // receiver's own class overrides it (§5.10, §6.3.2).
        Lookup::SlotAccessor(accessor, field) => {
            mutator.decide_slot_accessor(self_value, accessor, field, arguments)
        }
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
        Lookup::Primitive(primitive, level) => {
            match crate::builtins::execute(mutator, primitive, self_value, selector, arguments) {
                Some(result) => result,
                None => {
                    decide_super_above(mutator, level, self_value, superclass, selector, arguments)
                }
            }
        }
        Lookup::NotFound => {
            mutator.decide_super_does_not_understand(self_value, superclass, selector, arguments)
        }
    }
}

/// Where a failed primitive sends a `super` send on: §5.10's walk, continued
/// from the level above the one that declared the primitive. `#[cold]` for
/// the reason `Mutator::decide_public_above` gives.
#[cold]
#[inline(never)]
fn decide_super_above<'gc>(
    mutator: &mut Mutator<'gc>,
    level: ClassReference<'gc>,
    self_value: Value<'gc>,
    superclass: ClassReference<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let lookup = dispatch::lookup_above(&mutator.heap, dispatch::Rule::Protected, level, selector);
    decide_super_lookup(mutator, lookup, self_value, superclass, selector, arguments)
}

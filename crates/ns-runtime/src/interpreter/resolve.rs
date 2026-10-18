//! A send's decision in two halves (`docs/design/dispatch.md`, "Resolve,
//! then act"): `resolve_*` is the lookup, in today's order and by today's
//! rules, answering what it found as a `Resolved`; `act` turns a `Resolved`
//! into the step. The uncached path is `act(resolve(..))`; a cache hit is
//! `act` on a `Resolved` rebuilt from an entry. So there is one place that
//! executes a target, and a hit cannot do anything a miss would not.

use std::rc::Rc;

use crate::builtins::primitive::Primitive;
use crate::dispatch::{self, Lookup};
use crate::mixin::{AccessorKind, MethodFastPath, Mixin, MixinMethod};
use crate::types::*;
use crate::{Heap, Symbol};

use super::{nested, FeedbackBase, Mutator, Signal, Step, StepArguments};

/// Which lookup found a target -- and so which class object its levels are
/// counted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chain {
    /// §5.5's lookupPublic, from the receiver's class -- for a class
    /// object, its metaclass (§3.2), so its class side is searched first.
    Public,
    /// §5.9's lookupProtected, from the class of `o`.
    Protected,
    /// §5.9's private rule: N's own member, N itself the level.
    Private,
    /// §5.10: lookupProtected from S, the superclass of `current_class`.
    Super,
    /// §5.5's `doesNotUnderstand:`, defined for the receiver's class.
    DoesNotUnderstand,
    /// §5.10's `doesNotUnderstand:`, defined for S.
    SuperDoesNotUnderstand,
}

/// What a send resolved to, with its defining class as a handle: the form
/// `act` executes, uncached or materialized from a cache entry.
pub(crate) enum Resolved<'gc> {
    Method {
        method: Rc<MixinMethod>,
        defining_class: ClassReference<'gc>,
        chain: Chain,
    },
    Slot {
        kind: AccessorKind,
        field: usize,
        chain: Chain,
    },
    NestedClass {
        mixin: Rc<Mixin>,
        declaring_class: ClassReference<'gc>,
        /// `act` does not read it -- a nested class's accessor is the same
        /// whichever chain found it -- but a cache entry records it
        /// (`inline_cache::Target::NestedClass`), to count `level` from.
        chain: Chain,
    },
    Primitive {
        primitive: Primitive,
        level: ClassReference<'gc>,
        chain: Chain,
    },
    /// §6.3's primary factory, found on `level`'s metaclass; `level` is the
    /// class object §6.3 instantiates, which is the send's receiver. `chain`
    /// is `Public` or `Protected`: a factory entry lives on a metaclass,
    /// which only a class object's own public or protected lookup starts
    /// from, and metaclasses are flat (§3.2), so no other chain reaches
    /// one (`resolved_for`).
    Factory {
        level: ClassReference<'gc>,
        chain: Chain,
    },
    /// Nothing handles it, not even `doesNotUnderstand:`. Never recorded.
    Unhandled,
}

impl Resolved<'_> {
    /// The chain a resolution was found on; `None` for `Unhandled`.
    pub(crate) fn chain(&self) -> Option<Chain> {
        match self {
            Resolved::Method { chain, .. }
            | Resolved::Slot { chain, .. }
            | Resolved::NestedClass { chain, .. }
            | Resolved::Primitive { chain, .. }
            | Resolved::Factory { chain, .. } => Some(*chain),
            Resolved::Unhandled => None,
        }
    }
}

impl<'gc> Mutator<'gc> {
    /// §5.5, the explicit-receiver send's lookup, which `decide_send` acts
    /// on: `lookupPublic` on the receiver's class, then
    /// `doesNotUnderstand:`.
    ///
    /// A class object needs no case of its own: its class is its metaclass
    /// (§3.2), so the one lookup searches its class side, then `Class`,
    /// `Behavior` and `Object` (§6). `ClassName new` needs none either:
    /// §6.3 makes the primary factory an entry of the metaclass mixin, so
    /// the lookup finds it like any other member.
    pub(crate) fn resolve_public(
        &mut self,
        receiver: Value<'gc>,
        selector: Symbol,
    ) -> Resolved<'gc> {
        let class = self.class_of(receiver);
        let lookup = dispatch::lookup_public(&self.heap, class, selector);
        self.resolved_public(lookup, receiver)
    }

    /// `lookup` as a resolution on the public chain; nothing found is
    /// `doesNotUnderstand:` for the receiver's class (§5.5).
    ///
    /// The `Lookup` is matched as it is and answered as a `Resolved` no
    /// wider than it, never through a "what the send settled on" enum that
    /// also carries a primitive's finished `Result<Step, Signal>`: that enum
    /// is 64 bytes, and returning one made every send -- slot and method
    /// sends included, which never touch a primitive -- write and re-read a
    /// decision through memory. Dropping it measured 6.5% (`bench_loop.ns`)
    /// and 5.1% (`bench_send.ns`) faster (`docs/design/execution.md`, "What
    /// primitives as method-table entries moved").
    #[inline(always)]
    fn resolved_public(&mut self, lookup: Lookup<'gc>, receiver: Value<'gc>) -> Resolved<'gc> {
        match lookup {
            Lookup::NotFound => {
                self.resolve_does_not_understand(receiver, Chain::DoesNotUnderstand)
            }
            found => resolved_for(&self.heap, found, receiver, Chain::Public),
        }
    }

    /// §5.7-§5.9, the lexical send's lookup:
    /// N's private member first (`lexical_class` is N, `None` when the
    /// chain is shorter than the depth), then lookupProtected on `o`'s
    /// class; nothing found is an ordinary send to `o`. This is the order
    /// `dispatch::lexical_member` states, taken inline here because a
    /// resolution also records which of the two steps found the member
    /// (`Chain::Private`, `Chain::Protected`); a change to one is a change
    /// to both.
    ///
    /// When `o` is a class object -- a send written inside a class-side
    /// method with no explicit receiver -- `o`'s class is its metaclass
    /// (§3.2), so lookupProtected searches the class side first, with its
    /// `protected` members, and `Class`, `Behavior` and `Object` after it,
    /// in the order `resolve_public` gives an explicit-receiver send.
    pub(crate) fn resolve_lexical(
        &mut self,
        enclosing: Value<'gc>,
        lexical_class: Option<ClassReference<'gc>>,
        selector: Symbol,
    ) -> Resolved<'gc> {
        if let Some(lexical_class) = lexical_class {
            match dispatch::private_member(&self.heap, lexical_class, selector) {
                Lookup::NotFound => {}
                found => return resolved(found, Chain::Private),
            }
        }
        let class = self.class_of(enclosing);
        match dispatch::lookup_protected(&self.heap, class, selector) {
            Lookup::NotFound => self.resolve_public(enclosing, selector),
            found => resolved_for(&self.heap, found, enclosing, Chain::Protected),
        }
    }

    /// §5.10's lookup, once `decide_super_send` or `send_super` has ruled
    /// out its two errors: lookupProtected from S; nothing found is the
    /// `doesNotUnderstand:` defined for S.
    ///
    /// The receiver is not a parameter: §5.10 invokes what it finds on `o`,
    /// but only `act` needs `o` for that. The lookup itself reads S alone --
    /// it used to take the receiver for the factory check that stood ahead
    /// of the handler, which §6.3's factory entry has replaced.
    pub(crate) fn resolve_super(
        &mut self,
        superclass: ClassReference<'gc>,
        selector: Symbol,
    ) -> Resolved<'gc> {
        match dispatch::lookup_protected(&self.heap, superclass, selector) {
            Lookup::NotFound => self.resolve_super_does_not_understand(superclass),
            found => resolved(found, Chain::Super),
        }
    }

    /// §5.10's failure: "Otherwise, the #doesNotUnderstand: method defined
    /// for S is invoked" -- looked up from `superclass`, S, rather than from
    /// the receiver's own class, so a subclass's own `doesNotUnderstand:`
    /// does not answer a `super` send that its superclass cannot.
    ///
    /// Nothing is checked ahead of the handler any more. A `super` send of a
    /// factory selector used to instantiate from here, because a class side
    /// was inherited and so an inherited class-side method's `super new`
    /// could be the only way to reach the receiver's own factory. §3.2 makes
    /// a metaclass flat, so a factory entry is reachable only on the
    /// receiver's own metaclass, never on S -- and S, for a class-side
    /// method, is `Class` (§6: a metaclass is its class side applied to
    /// `Class`, and a class-side method's `current_class` is the
    /// metaclass).
    ///
    /// `#[cold]`: it is what a `super` send does when nothing answered it.
    #[cold]
    #[inline(never)]
    fn resolve_super_does_not_understand(
        &mut self,
        superclass: ClassReference<'gc>,
    ) -> Resolved<'gc> {
        self.resolve_handler(superclass, Chain::SuperDoesNotUnderstand)
    }

    /// §5.5's `doesNotUnderstand:` fallback -- access-blind
    /// (`lookup_defined_for`), from the receiver's class, per
    /// `docs/design/dispatch.md`.
    ///
    /// `#[cold]`: it is what a send does when nothing answered it, so the
    /// "not found" arm of an inlined resolution must not take a register or
    /// a cache line from the arms that did find something.
    #[cold]
    #[inline(never)]
    fn resolve_does_not_understand(&mut self, receiver: Value<'gc>, chain: Chain) -> Resolved<'gc> {
        let class = self.class_of(receiver);
        self.resolve_handler(class, chain)
    }

    /// The `doesNotUnderstand:` defined for `class`. A user-overridden one
    /// arrives as a method; so does `Object`'s own, a kernel method every
    /// class that does not override it inherits, which signals a
    /// `MessageNotUnderstood`.
    fn resolve_handler(&mut self, class: ClassReference<'gc>, chain: Chain) -> Resolved<'gc> {
        match dispatch::lookup_defined_for(&self.heap, class, Symbol::DOES_NOT_UNDERSTAND) {
            // A nested declaration can never be the handler: `walk` matches
            // a declaration by name, and a class name is never a keyword
            // selector. Folded into "nothing found" rather than given an arm
            // of its own so the unreachable case still reports the ordinary
            // "nothing handles this" error instead of a second,
            // differently-worded one.
            Lookup::NotFound | Lookup::NestedClass(..) => Resolved::Unhandled,
            found => resolved(found, chain),
        }
    }
}

/// A found `Lookup` as a resolution on `chain`. `NotFound` is the caller's.
#[inline(always)]
fn resolved<'gc>(lookup: Lookup<'gc>, chain: Chain) -> Resolved<'gc> {
    match lookup {
        Lookup::Method(method, defining_class) => Resolved::Method {
            method,
            defining_class,
            chain,
        },
        Lookup::SlotAccessor(accessor, field) => Resolved::Slot {
            kind: accessor.kind,
            field,
            chain,
        },
        Lookup::NestedClass(mixin, declaring_class) => Resolved::NestedClass {
            mixin,
            declaring_class,
            chain,
        },
        Lookup::Primitive(primitive, level) => Resolved::Primitive {
            primitive,
            level,
            chain,
        },
        // §6.3's factory is a member of a metaclass mixin, and the only
        // lookups that can start from a metaclass -- a public or protected
        // lookup from a class object's class -- hand their answer to
        // `resolved_for` below, which maps it. Reaching here means a
        // private, `super` or handler lookup found one, which only a crafted
        // image can arrange (`image::mixins::build` derives one for a
        // class-side row alone): refused, rather than instantiating whatever
        // object happened to receive the send.
        Lookup::Factory(_) => Resolved::Unhandled,
        Lookup::NotFound => unreachable!("internal: the caller resolves what nothing found"),
    }
}

/// A found `Lookup` of a public or protected lookup from `receiver`'s class
/// as a resolution on `chain`.
///
/// §6.3's primary factory is an entry of a metaclass mixin, and it
/// instantiates the class object the message was sent to -- so it is
/// honoured only when it was found on that receiver's own class, its
/// metaclass. Found anywhere else it could only have been put there by a
/// crafted image (`image::mixins::build` derives one for a class-side row
/// alone), and it is refused rather than instantiating whatever received
/// the send.
#[inline(always)]
fn resolved_for<'gc>(
    heap: &Heap<'gc>,
    lookup: Lookup<'gc>,
    receiver: Value<'gc>,
    chain: Chain,
) -> Resolved<'gc> {
    match (lookup, receiver.as_object()) {
        (Lookup::Factory(level), Some(class_object))
            if heap.is_class(class_object) && heap.object_class(class_object).same(level) =>
        {
            Resolved::Factory {
                level: class_object,
                chain,
            }
        }
        (Lookup::Factory(_), _) => Resolved::Unhandled,
        (found, _) => resolved(found, chain),
    }
}

impl<'gc> Mutator<'gc> {
    /// Runs what a send resolved to. `receiver` is the send's receiver --
    /// `o` for a lexical send, `self` for `super`.
    ///
    /// `#[inline(always)]` so that the caller holding the `Resolved` also
    /// builds the step, rather than handing it to a call that answers a
    /// 56-byte decision back through memory; the `doesNotUnderstand:`
    /// handlers below are `#[cold]` and `#[inline(never)]`, so only the hot
    /// half is duplicated into the senders. That split measured
    /// 1.9%/2.6% faster than a decision behind a function boundary
    /// (`docs/design/execution.md`, "What primitives as method-table entries
    /// moved").
    #[inline(always)]
    pub(crate) fn act(
        &mut self,
        resolved: Resolved<'gc>,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        self.act_with_base(
            resolved,
            FeedbackBase::NOT_ASSIGNED,
            receiver,
            selector,
            arguments,
        )
    }

    /// `act`, with the base a method step's activation takes -- a cache hit
    /// knows its target's family, so the push need not ask the map. The one
    /// executor both paths share; `act` is this with `NOT_ASSIGNED`.
    #[inline(always)]
    pub(crate) fn act_with_base(
        &mut self,
        resolved: Resolved<'gc>,
        feedback_base: FeedbackBase,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        match resolved {
            Resolved::Method {
                method,
                defining_class,
                chain,
            } => match chain {
                Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand => {
                    self.act_handler_method(method, defining_class, receiver, selector, arguments)
                }
                _ => {
                    // Read off the method on every send, hit or miss: the
                    // cache records `Target::Method` and never decides what
                    // the method does.
                    match method.fast_path {
                        Some(MethodFastPath::BuiltinClass(kind)) => {
                            return Ok(Step::Value(Value::object(self.builtin_class(kind))));
                        }
                        Some(MethodFastPath::Mirror(primitive)) => {
                            if let Some(result) = crate::interpreter::mirror::execute_method(
                                self, primitive, receiver, selector, arguments,
                            ) {
                                return result;
                            }
                        }
                        Some(MethodFastPath::Constant(constant)) => {
                            return Ok(Step::Value(constant.value()));
                        }
                        Some(MethodFastPath::Receiver) => return Ok(Step::Value(receiver)),
                        // Read with `get` rather than indexed: a send the
                        // compiler emitted matches its selector's count,
                        // but `Message>>sendTo:` delivers whatever
                        // arguments a program put in the message, so a
                        // missing one activates the method -- whose arity
                        // check refuses the count -- rather than panicking.
                        Some(MethodFastPath::Parameter(position)) => {
                            if let Some(argument) = arguments.get(usize::from(position)) {
                                return Ok(Step::Value(*argument));
                            }
                        }
                        // The step a Boolean leaf used to answer: the block
                        // to activate, with no activation of the method. It
                        // is marked as control flow -- which `push_step`
                        // exempts from the depth count for a
                        // `SendInlinable` -- only for a Boolean receiver,
                        // the one the optimized lowering inlines; an
                        // instance of a subclass of `True` reaches the same
                        // method through a real send under both lowerings,
                        // so its block is counted under both. A non-closure
                        // activates the method, whose `value` send answers
                        // what any other send of `value` to it would.
                        Some(MethodFastPath::ValueOfParameter(position)) => {
                            if let Some(object) = arguments
                                .get(usize::from(position))
                                .and_then(|value| value.as_object())
                            {
                                if self.heap().is_closure(object) {
                                    let block_arguments = if receiver.as_boolean().is_some() {
                                        StepArguments::ControlFlow
                                    } else {
                                        StepArguments::Owned(Vec::new())
                                    };
                                    return Ok(Step::Block {
                                        closure: object,
                                        arguments: block_arguments,
                                        then: Continuation::Deliver,
                                    });
                                }
                            }
                        }
                        // `cull:`'s rule, as `Closure>>cull:` states it: the
                        // receiver as the block's argument when it takes one,
                        // nothing when it takes none. A block that takes more
                        // is handed the one and fails the arity check exactly
                        // as `value:` would. Never control flow: no lowering
                        // inlines these sends, so both count the block.
                        Some(MethodFastPath::CullParameter(position)) => {
                            if let Some(object) = arguments
                                .get(usize::from(position))
                                .and_then(|value| value.as_object())
                            {
                                if let Some(parameter_count) =
                                    self.heap().closure_parameter_count(object)
                                {
                                    let block_arguments = if parameter_count == 0 {
                                        Vec::new()
                                    } else {
                                        vec![receiver]
                                    };
                                    return Ok(Step::Block {
                                        closure: object,
                                        arguments: StepArguments::Owned(block_arguments),
                                        then: Continuation::Deliver,
                                    });
                                }
                            }
                        }
                        None => {}
                    }
                    Ok(Step::Method {
                        receiver,
                        method,
                        defining_class,
                        arguments: StepArguments::Send,
                        feedback_base,
                    })
                }
            },
            Resolved::Slot { kind, field, chain } => match chain {
                Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand => {
                    self.act_handler_slot(kind, field, receiver, selector, arguments)
                }
                // §6.3.2: a slot's accessors answer a send exactly when a
                // method with the slot's accessibility would; `super x`
                // reaches the superclass's accessor even where the
                // receiver's own class overrides it (§5.10).
                _ => self.decide_slot_accessor(receiver, kind, selector, field, arguments),
            },
            // §6: the implicit accessor a nested declaration defines is
            // reached exactly like any other member, and it ignores
            // `arguments` -- the accessor is unary, so a send that supplied
            // arguments did not name it (`walk` matched on the bare
            // selector). For `super N` the receiver stays `self` (§5.10),
            // which is also the enclosing object the accessor caches
            // against -- so `super N` and `N` name the same class object
            // exactly when they resolve to the same declaration, and
            // different ones when a subclass redeclares `N`: the accessor
            // caches against the declaration's own hidden slot
            // (`MixinApplication::nested_slot`), not against its name.
            Resolved::NestedClass {
                mixin,
                declaring_class,
                ..
            } => nested::nested_class(self, receiver, &mixin, declaring_class),
            // A primitive entry is a mirror row, and a mirror row's failure
            // path is its block: `builtins::execute` answers `Some` for every
            // one, so no primitive passes its level over any more. The
            // `None` arm is what keeps that an error rather than a panic if
            // it ever stopped holding. A `doesNotUnderstand:` handler is
            // never a primitive -- `Object`'s is a kernel method, and the
            // mirror answers no `doesNotUnderstand:` of its own -- so a
            // handler chain's primitive, which only a crafted image could
            // arrange, is the same error.
            Resolved::Primitive {
                primitive, chain, ..
            } => match chain {
                Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand => {
                    Err(unhandled_error(receiver, selector))
                }
                _ => crate::builtins::execute(self, primitive, receiver, selector, arguments)
                    .unwrap_or_else(|| Err(unhandled_error(receiver, selector))),
            },
            // §6.3's factory entry, run with no re-check: the lookup found
            // it on the receiver's own metaclass, and the class-side
            // dispatch id a cache entry is guarded by determines that
            // metaclass, so a hit reaches the class a miss would.
            Resolved::Factory { level, .. } => self.decide_factory(level, arguments),
            Resolved::Unhandled => Err(unhandled_error(receiver, selector)),
        }
    }

    /// A `doesNotUnderstand:` handler that is a method: activated on the
    /// message. `#[cold]` for the reason `resolve_does_not_understand` gives.
    #[cold]
    #[inline(never)]
    fn act_handler_method(
        &mut self,
        method: Rc<MixinMethod>,
        defining_class: ClassReference<'gc>,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let message = self.handler_message(selector, arguments);
        // §5.5 makes a message nothing understood the VM's error, so the
        // kernel's handler takes the depth limit's reserve near the limit
        // as a VM error's `signal` does; an override is the program's own.
        self.take_depth_reserve_for_does_not_understand(defining_class);
        // Activated whatever its fast path: `Object`'s own handler is
        // Newspeak that signals a `MessageNotUnderstood`, and no kernel
        // handler forwards to a leaf; a builtin-class accessor is unary and
        // a quick method answers no message, so neither is one either.
        Ok(handler_method_step(
            receiver,
            method,
            defining_class,
            message,
        ))
    }

    /// A `doesNotUnderstand:` handler that is a slot's setter: §6.3.2, a
    /// mutable slot named `doesNotUnderstand` defines the setter
    /// `doesNotUnderstand:`, a method like any other -- so it is the
    /// handler, and stores the message.
    #[cold]
    #[inline(never)]
    fn act_handler_slot(
        &mut self,
        kind: AccessorKind,
        field: usize,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let message = self.handler_message(selector, arguments);
        self.decide_slot_accessor(
            receiver,
            kind,
            Symbol::DOES_NOT_UNDERSTAND,
            field,
            &[message],
        )
    }

    /// The message every `doesNotUnderstand:` handler takes.
    ///
    /// Nothing is checked ahead of it any more: §5.10's failure path used to
    /// try the factory on `self` first, because an inherited class-side
    /// method's `super` send could be the only way to reach the receiver's
    /// own factory. §6.3 makes the factory an entry of the metaclass mixin
    /// and §3.2 makes the metaclass flat, so a factory is reached by the
    /// ordinary class-side lookup or not at all.
    ///
    /// The message is a `Message` of the kernel's, its `selector` a Symbol
    /// and its `arguments` an Array, which `Object>>doesNotUnderstand:`
    /// sends again with `sendTo:` when the debugger resumed it
    /// (`docs/design/core-library.md`, "Exceptions"). It is built
    /// once the handler is found; allocation order is unobservable, identity
    /// hashes being drawn lazily.
    fn handler_message(&mut self, selector: Symbol, arguments: &[Value<'gc>]) -> Value<'gc> {
        let selector_value = self.new_symbol(selector);
        let arguments_value = Value::object(self.new_array(arguments.to_vec()));
        Value::object(
            self.new_kernel_instance(BuiltinClass::Message, |name| match name {
                "selector" => selector_value,
                "arguments" => arguments_value,
                other => {
                    panic!("internal: Kernel.ns's Message has a slot {other} the VM does not fill")
                }
            }),
        )
    }
}

/// A `doesNotUnderstand:` method's activation, on the message alone.
fn handler_method_step<'gc>(
    receiver: Value<'gc>,
    method: Rc<MixinMethod>,
    defining_class: ClassReference<'gc>,
    message: Value<'gc>,
) -> Step<'gc> {
    Step::Method {
        receiver,
        method,
        defining_class,
        arguments: StepArguments::Owned(vec![message]),
        feedback_base: FeedbackBase::NOT_ASSIGNED,
    }
}

/// A send nothing handles, not even `doesNotUnderstand:`.
fn unhandled_error<'gc>(receiver: Value<'gc>, selector: Symbol) -> Signal<'gc> {
    Signal::Error(NewspeakError::error(
        selector,
        receiver,
        format!(
            "doesNotUnderstand: {selector} (and no doesNotUnderstand: found anywhere in the \
             class chain)"
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::primitive::mirror_primitive_named;
    use crate::{HostResult, HostSignal, Interpreter, Root, SourceOrigin};
    use indoc::indoc;
    use std::path::PathBuf;

    fn load(source: &str) -> (Interpreter, Root) {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        (interpreter, class)
    }

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// §3.2's accessor on `Object` is answered by `act` itself: the
    /// resolution of the bare name, an implicit-receiver send, is the
    /// ordinary `Method`, and the builtin class comes back as a value with
    /// nothing activated. The accessor is protected, so the same selector
    /// sent to the object from outside reaches `doesNotUnderstand:`.
    #[test]
    fn act_answers_a_class_accessor_without_an_activation() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let receiver = Value::small_integer(3);
            let selector = Symbol::intern("String");
            assert!(
                matches!(
                    mutator.resolve_public(receiver, selector).chain(),
                    Some(Chain::DoesNotUnderstand)
                ),
                "`3 String` from outside must reach doesNotUnderstand:"
            );
            let resolved = mutator.resolve_lexical(receiver, None, selector);
            let Resolved::Method { ref method, .. } = resolved else {
                panic!("a bare `String` must resolve to Object's accessor");
            };
            assert_eq!(
                method.fast_path,
                Some(MethodFastPath::BuiltinClass(BuiltinClass::String))
            );
            let string = mutator.builtin_class(BuiltinClass::String);
            match mutator.act_with_base(
                resolved,
                FeedbackBase::NOT_ASSIGNED,
                receiver,
                selector,
                &[],
            ) {
                Ok(Step::Value(answer)) => {
                    assert!(
                        answer
                            .as_object()
                            .is_some_and(|answered| answered.same(string)),
                        "the accessor answers String itself"
                    );
                }
                _ => panic!("the accessor must answer a value without an activation"),
            }
            assert_eq!(mutator.heap().activation_depth(), 0, "nothing was pushed");
        });
    }

    /// `resolve` decides and runs nothing; each send kind's resolution names
    /// the chain it was found on (`docs/design/dispatch.md`, "Resolve, then
    /// act").
    #[test]
    fn resolving_names_the_chain_and_runs_nothing() {
        let source = indoc! {"
            class Subject = Object new (
            ) (
              public foo = ( ^1 )
            ) : (
              public make = ( ^2 )
            )
        "};
        let (mut interpreter, class) = load(source);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        interpreter.enter(|mutator| {
            let receiver = mutator.fetch(&instance);
            let class_object = mutator.fetch(&class);
            assert!(matches!(
                mutator.resolve_public(receiver, Symbol::intern("foo")),
                Resolved::Method {
                    chain: Chain::Public,
                    ..
                }
            ));
            // A class object's class is its metaclass (§3.2), so its class
            // side is found by the ordinary public lookup.
            assert!(matches!(
                mutator.resolve_public(class_object, Symbol::intern("make")),
                Resolved::Method {
                    chain: Chain::Public,
                    ..
                }
            ));
            // §6.3: the factory is an entry of the metaclass mixin, so the
            // ordinary lookup is what finds it -- not a case ahead of it.
            assert!(matches!(
                mutator.resolve_public(class_object, Symbol::NEW),
                Resolved::Factory {
                    chain: Chain::Public,
                    ..
                }
            ));
            // `Integer`'s `+` is a kernel primitive method (`Kernel.ns`),
            // found as a `Resolved::Method` whose
            // `MixinMethod.fast_path` names the mirror row.
            match mutator.resolve_public(Value::small_integer(3), Symbol::intern("+")) {
                Resolved::Method {
                    method,
                    chain: Chain::Public,
                    ..
                } => assert_eq!(
                    method.fast_path,
                    mirror_primitive_named("integer:add:ifFail:").map(MethodFastPath::Mirror)
                ),
                _ => panic!("`3 + 4` must resolve to Integer's `+`"),
            }
            // `Object`'s `doesNotUnderstand:` is an ordinary kernel method
            // (`Kernel.ns`), which signals a `MessageNotUnderstood`.
            match mutator.resolve_public(receiver, Symbol::intern("zork")) {
                Resolved::Method {
                    method,
                    chain: Chain::DoesNotUnderstand,
                    ..
                } => assert_eq!(method.fast_path, None),
                _ => panic!("an unhandled selector must resolve to Object's doesNotUnderstand:"),
            }
            assert_eq!(mutator.heap().activation_depth(), 0, "nothing was pushed");
        });
    }

    /// `resolve_lexical` takes `dispatch::lexical_member`'s two steps
    /// inline; both find the same method for a private member of N, for a
    /// protected one found on `o`'s class, and neither finds a selector
    /// nothing declares (the send then falls to an ordinary send).
    #[test]
    fn a_lexical_resolution_finds_what_lexical_member_finds() {
        let source = indoc! {"
            class Subject = Object new (
            ) (
              private helper = ( ^1 )
              protected other = ( ^2 )
            ) : ()
        "};
        let (mut interpreter, class) = load(source);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        interpreter.enter(|mutator| {
            let receiver = mutator.fetch(&instance);
            let Some(class) = mutator.fetch(&class).as_object() else {
                panic!("load_module answers a class object");
            };
            for (name, expected_chain) in [("helper", Chain::Private), ("other", Chain::Protected)]
            {
                let selector = Symbol::intern(name);
                let Lookup::Method(found, _) =
                    dispatch::lexical_member(mutator.heap(), Some(class), class, selector)
                else {
                    panic!("lexical_member finds {name}");
                };
                match mutator.resolve_lexical(receiver, Some(class), selector) {
                    Resolved::Method { method, chain, .. } => {
                        assert!(Rc::ptr_eq(&method, &found), "{name}: the same method");
                        assert_eq!(chain, expected_chain, "{name}");
                    }
                    _ => panic!("resolve_lexical finds {name}"),
                }
            }
            assert!(matches!(
                dispatch::lexical_member(
                    mutator.heap(),
                    Some(class),
                    class,
                    Symbol::intern("zork")
                ),
                Lookup::NotFound
            ));
        });
    }

    /// A resolution that `act` runs answers what `decide_send` answered
    /// before the split: a native leaf is its value.
    #[test]
    fn acting_on_a_primitive_answers_its_value() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let resolved = mutator.resolve_public(Value::small_integer(3), Symbol::intern("+"));
            match mutator.act(
                resolved,
                Value::small_integer(3),
                Symbol::intern("+"),
                &[Value::small_integer(4)],
            ) {
                Ok(Step::Value(answer)) if answer.as_small_integer() == Some(7) => {}
                _ => panic!("3 + 4 is 7"),
            }
        });
    }

    /// Every resolution the hot path returns stays small: 3a measured a
    /// 64-byte decision on the return path at 5.5-22.4% (`execution.md`,
    /// "What primitives as method-table entries moved"). 24 bytes in a
    /// release build, `Lookup`'s size: a `Slot` carries the accessor's kind
    /// rather than the whole `SlotAccessor`, but `Method`, `NestedClass` and
    /// `Primitive` each hold an 8-byte field beside an 8-byte handle, so
    /// their chain and the variant tag take a third word. It was 16 while a
    /// handle was 4 bytes and the two fitted in the bytes it left spare; the
    /// 8-byte `ClassReference` grew it.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn a_resolution_stays_the_size_the_hot_path_was_measured_at() {
        assert_eq!(std::mem::size_of::<Resolved>(), 24);
    }
}

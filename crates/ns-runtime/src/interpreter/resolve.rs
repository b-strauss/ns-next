//! A send's decision in two halves (`docs/design/dispatch.md`, "Resolve,
//! then act"): `resolve_*` is the lookup, in today's order and by today's
//! rules, answering what it found as a `Resolved`; `act` turns a `Resolved`
//! into the step. The uncached path is `act(resolve(..))`; a cache hit is
//! `act` on a `Resolved` rebuilt from an entry. So there is one place that
//! executes a target, and a hit cannot do anything a miss would not.

use std::rc::Rc;

use crate::builtins::primitive::Primitive;
use crate::dispatch::{self, Lookup, Rule};
use crate::mixin::{AccessorKind, Mixin, MixinMethod};
use crate::types::*;
use crate::Symbol;

use super::instantiate::is_factory_send;
use super::{nested, FeedbackBase, Mutator, Signal, Step, StepArguments};

/// Which lookup found a target -- and so which class object its levels are
/// counted from, and where a failed primitive continues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chain {
    /// §5.5's lookupPublic, from the receiver's class.
    Public,
    /// §5.5 on the receiver's metaclass (§6.3); the receiver is a class object.
    PublicClassSide,
    /// §5.9's lookupProtected, from the class of `o`.
    Protected,
    /// §5.9 on `o`'s metaclass.
    ProtectedClassSide,
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
    /// `try_factory_send` answers the send (§6.1); `chain` is `Public` or
    /// `SuperDoesNotUnderstand`.
    Factory { chain: Chain },
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
            | Resolved::Factory { chain } => Some(*chain),
            Resolved::Unhandled => None,
        }
    }
}

impl<'gc> Mutator<'gc> {
    /// §5.5, the explicit-receiver send's lookup, which `decide_send` acts
    /// on: the factory check first (§6.1, `try_factory_send`'s condition),
    /// then a class object's class side (§6.3), then `lookupPublic` on the
    /// class, then `doesNotUnderstand:`.
    ///
    /// The factory check comes before any lookup because `ClassName new` (or
    /// a class's own non-default factory selector) never has a real method
    /// to find: every class object's own `.class` is a shared placeholder
    /// whose method table holds only `Object`'s primitives, none of them a
    /// factory, so an ordinary `lookup_public` would fall through to
    /// `doesNotUnderstand:` every time. Only a class object has a factory
    /// selector (`primary_factory_selector`), so asking `is_class` first
    /// changes no answer.
    ///
    /// §6.3: a class object's class-side methods are reached through its
    /// metaclass, consulted before the class object's own (ordinary,
    /// `Object`-native) behaviour. A whole branch of its own, called out of
    /// line, rather than a `Lookup` folded into the walk below: a receiver
    /// that is not a class object -- every receiver on a hot send path --
    /// then pays one `is_class` test and nothing else.
    pub(crate) fn resolve_public(
        &mut self,
        receiver: Value<'gc>,
        selector: Symbol,
    ) -> Resolved<'gc> {
        if let Value::Object(object) = receiver {
            if self.heap.is_class(object) {
                if is_factory_send(&self.heap, &receiver, selector) {
                    return Resolved::Factory {
                        chain: Chain::Public,
                    };
                }
                if let Some(resolved) =
                    self.resolve_class_side(object, selector, Chain::PublicClassSide)
                {
                    return resolved;
                }
            }
        }
        let class = self.class_of(&receiver);
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
            found => resolved(found, Chain::Public),
        }
    }

    /// The class side, first (§6.3): a metaclass that answers is the
    /// resolution; one that does not falls through to the class object's
    /// own behaviour. There is no second `doesNotUnderstand:` path here,
    /// only a second class to try first. `#[inline(never)]`: only a class
    /// object reaches it.
    ///
    /// A metaclass primitive on the public chain is resolved like any
    /// member; its failure continues on the metaclass chain and then falls
    /// through, in `act`. No metaclass chain holds a primitive -- a
    /// metaclass's mixins are class sides or empty stand-ins, never a
    /// builtin's instance mixin -- so that is reached only in principle; it
    /// is written out so that nothing here depends on it.
    #[inline(never)]
    fn resolve_class_side(
        &mut self,
        class_object: ObjectReference<'gc>,
        selector: Symbol,
        chain: Chain,
    ) -> Option<Resolved<'gc>> {
        let metaclass = self.metaclass_of(class_object)?;
        let lookup = match chain {
            Chain::PublicClassSide => dispatch::lookup_public(&self.heap, metaclass, selector),
            _ => dispatch::lookup_protected(&self.heap, metaclass, selector),
        };
        match lookup {
            Lookup::NotFound => None,
            // §5.9 on a class side:
            // only a real declaration stops the search, so a primitive there
            // would be passed over to reach the class-object chain anyway.
            Lookup::Primitive(..) if chain == Chain::ProtectedClassSide => None,
            found => Some(resolved(found, chain)),
        }
    }

    /// §5.7-§5.9, the lexical send's lookup:
    /// N's private member first (`lexical_class` is N, `None` when the
    /// chain is shorter than the depth), then the class side of `o` when
    /// `o` is a class object, then lookupProtected on `o`'s class; nothing
    /// found is an ordinary send to `o`.
    ///
    /// §6.3: a class object's class side is the behaviour of that class
    /// object, so a send written inside a class-side method with no explicit
    /// receiver has to search the metaclass before the class object's own
    /// behaviour, `Object`'s primitives -- the same order `resolve_public`
    /// gives an explicit-receiver send. Without it a class side reaches only
    /// what `lookup_public` on the metaclass happens to answer once the send
    /// has fallen all the way through to the public chain: its `protected`
    /// and `private` members are unreachable, and any selector `Object`
    /// answers natively silently wins over the class side's own declaration.
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
        if let Value::Object(object) = enclosing {
            if self.heap.is_class(object) {
                if let Some(resolved) =
                    self.resolve_class_side(object, selector, Chain::ProtectedClassSide)
                {
                    return resolved;
                }
            }
        }
        let class = self.class_of(&enclosing);
        match dispatch::lookup_protected(&self.heap, class, selector) {
            Lookup::NotFound => self.resolve_public(enclosing, selector),
            found => resolved(found, Chain::Protected),
        }
    }

    /// §5.10's lookup, once `decide_super_send` or `send_super` has ruled
    /// out its two errors:
    /// lookupProtected from S; nothing found is the factory check on `self`,
    /// then the `doesNotUnderstand:` defined for S.
    pub(crate) fn resolve_super(
        &mut self,
        self_value: Value<'gc>,
        superclass: ClassReference<'gc>,
        selector: Symbol,
    ) -> Resolved<'gc> {
        match dispatch::lookup_protected(&self.heap, superclass, selector) {
            Lookup::NotFound => {
                self.resolve_super_does_not_understand(self_value, superclass, selector)
            }
            found => resolved(found, Chain::Super),
        }
    }

    /// §5.10's failure: "Otherwise, the #doesNotUnderstand: method defined
    /// for S is invoked" -- looked up from `superclass`, S, rather than from
    /// the receiver's own class, so a subclass's own `doesNotUnderstand:`
    /// does not answer a `super` send that its superclass cannot. A class
    /// object's `super` send of its own factory selector still instantiates
    /// (`try_factory_send`), exactly as the same send to it does.
    ///
    /// `#[cold]`, although this is not purely a failure path: a `super new`
    /// that does instantiate -- a class overriding `new` and delegating to
    /// `super` -- passes through here too. That is accepted because
    /// instantiation's own allocations (the instance, its factory's levels,
    /// every initializer activation) dwarf what marking this cold costs a
    /// codegen decision that runs once per `super`-prefixed factory send in
    /// the whole program, where an ordinary `super` failure runs on every
    /// miss.
    #[cold]
    #[inline(never)]
    fn resolve_super_does_not_understand(
        &mut self,
        self_value: Value<'gc>,
        superclass: ClassReference<'gc>,
        selector: Symbol,
    ) -> Resolved<'gc> {
        if is_factory_send(&self.heap, &self_value, selector) {
            return Resolved::Factory {
                chain: Chain::SuperDoesNotUnderstand,
            };
        }
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
        let class = self.class_of(&receiver);
        self.resolve_handler(class, chain)
    }

    /// The `doesNotUnderstand:` defined for `class`. A user-overridden one
    /// arrives as a method; `Object`'s own primitive one, which every class
    /// that does not override it inherits, as a primitive, run like any
    /// other.
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
        Lookup::NotFound => unreachable!("internal: the caller resolves what nothing found"),
    }
}

impl<'gc> Mutator<'gc> {
    /// Runs what a send resolved to. `receiver` is the send's receiver --
    /// `o` for a lexical send, `self` for `super` -- and `superclass` is S
    /// for a `super` send, `None` otherwise.
    ///
    /// `#[inline(always)]` so that the caller holding the `Resolved` also
    /// builds the step, rather than handing it to a call that answers a
    /// 56-byte decision back through memory; the `#[cold]` and
    /// `#[inline(never)]` continuations below break the recursion, so only
    /// the hot half is duplicated into the senders. That split measured
    /// 1.9%/2.6% faster than a decision behind a function boundary
    /// (`docs/design/execution.md`, "What primitives as method-table entries
    /// moved").
    #[inline(always)]
    pub(crate) fn act(
        &mut self,
        resolved: Resolved<'gc>,
        receiver: Value<'gc>,
        superclass: Option<ClassReference<'gc>>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        self.act_with_base(
            resolved,
            FeedbackBase::NOT_ASSIGNED,
            receiver,
            superclass,
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
        superclass: Option<ClassReference<'gc>>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        match resolved {
            Resolved::Method {
                method,
                defining_class,
                chain,
            } => match chain {
                Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand => self
                    .act_handler_method(
                        method,
                        defining_class,
                        chain,
                        receiver,
                        selector,
                        arguments,
                    ),
                _ => Ok(Step::Method {
                    receiver,
                    method,
                    defining_class,
                    arguments: StepArguments::Send,
                    feedback_base,
                }),
            },
            Resolved::Slot { kind, field, chain } => match chain {
                Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand => {
                    self.act_handler_slot(kind, field, chain, receiver, selector, arguments)
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
            } => nested::nested_class(self, &receiver, &mixin, declaring_class),
            Resolved::Primitive {
                primitive,
                level,
                chain,
            } => match chain {
                Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand => self
                    .act_handler_primitive(primitive, level, chain, receiver, selector, arguments),
                _ => match crate::builtins::execute(self, primitive, receiver, selector, arguments)
                {
                    Some(result) => result,
                    None => self.act_above(level, chain, receiver, superclass, selector, arguments),
                },
            },
            Resolved::Factory { chain } => {
                self.act_factory(chain, receiver, superclass, selector, arguments)
            }
            Resolved::Unhandled => Err(unhandled_error(receiver, selector)),
        }
    }

    /// Where a failed primitive sends a send on: the same chain's lookup,
    /// continued from the level above the one that declared it
    /// (`docs/design/dispatch.md`, "A primitive can fail").
    ///
    /// `#[cold]`, and out of line, because a primitive fails only when its
    /// receiver has the wrong shape for it -- an instance of a user subclass
    /// of `String`, which carries no string, or a platform object sent
    /// another platform object's selector. Every send in a running program
    /// that reaches a primitive at all normally has the shape that primitive
    /// works on, so this path must not take registers or a branch from the
    /// one that does.
    ///
    /// It recurses back into `act` rather than looping, which keeps the
    /// loop's state out of the hot function entirely. The recursion
    /// terminates and is shallow: each step moves at least one level up a
    /// superclass chain that `image::decode` refuses to let form a cycle.
    #[cold]
    #[inline(never)]
    fn act_above(
        &mut self,
        level: ClassReference<'gc>,
        chain: Chain,
        receiver: Value<'gc>,
        superclass: Option<ClassReference<'gc>>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let resolved = match chain {
            Chain::Public => {
                let lookup = dispatch::lookup_above(&self.heap, Rule::Public, level, selector);
                self.resolved_public(lookup, receiver)
            }
            Chain::PublicClassSide => {
                match dispatch::lookup_above(&self.heap, Rule::Public, level, selector) {
                    // The metaclass does not answer after all: fall through
                    // to the class object's own behaviour, as
                    // `resolve_public` does when the metaclass finds nothing.
                    Lookup::NotFound => {
                        let class = self.class_of(&receiver);
                        let lookup = dispatch::lookup_public(&self.heap, class, selector);
                        self.resolved_public(lookup, receiver)
                    }
                    found => resolved(found, Chain::PublicClassSide),
                }
            }
            Chain::Protected | Chain::ProtectedClassSide | Chain::Private => {
                match dispatch::lookup_above(&self.heap, Rule::Protected, level, selector) {
                    // §5.9: nothing found is an ordinary send to `o`.
                    Lookup::NotFound => self.resolve_public(receiver, selector),
                    found => resolved(found, Chain::Protected),
                }
            }
            Chain::Super => {
                // Only a `super` send builds this chain, and it passes S.
                let superclass = superclass.expect("internal: a super chain carries S");
                match dispatch::lookup_above(&self.heap, Rule::Protected, level, selector) {
                    Lookup::NotFound => {
                        self.resolve_super_does_not_understand(receiver, superclass, selector)
                    }
                    found => resolved(found, Chain::Super),
                }
            }
            Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand => {
                unreachable!("internal: a handler's primitive fails in act_handler_primitive")
            }
        };
        self.act(resolved, receiver, superclass, selector, arguments)
    }

    /// §6.1's instantiation, re-checked here rather than trusted from the
    /// resolution (`docs/design/dispatch.md`, "Resolve, then act"): a
    /// resolution rebuilt from a cache entry was decided for another
    /// receiver with the same guard. A receiver that is not the factory send
    /// after all continues where its chain would have without it -- the
    /// public lookup, which cannot answer `Factory` again since it asks the
    /// same condition, or the `doesNotUnderstand:` defined for S.
    ///
    /// `#[inline(never)]`: it recurses into `act`, and an instantiation's
    /// own work dwarfs the call.
    #[inline(never)]
    fn act_factory(
        &mut self,
        chain: Chain,
        receiver: Value<'gc>,
        superclass: Option<ClassReference<'gc>>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        if let Some(result) = self.try_factory_send(&receiver, selector, arguments) {
            return result;
        }
        let resolved = match chain {
            Chain::SuperDoesNotUnderstand => {
                // Only a `super` send builds this chain, and it passes S.
                let superclass = superclass.expect("internal: a super chain carries S");
                self.resolve_handler(superclass, Chain::SuperDoesNotUnderstand)
            }
            _ => self.resolve_public(receiver, selector),
        };
        self.act(resolved, receiver, superclass, selector, arguments)
    }

    /// A `doesNotUnderstand:` handler that is a method: activated on the
    /// message. `#[cold]` for the reason `resolve_does_not_understand` gives.
    #[cold]
    #[inline(never)]
    fn act_handler_method(
        &mut self,
        method: Rc<MixinMethod>,
        defining_class: ClassReference<'gc>,
        chain: Chain,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let message = match self.handler_message(chain, receiver, selector, arguments) {
            Ok(message) => message,
            Err(answered) => return answered,
        };
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
        chain: Chain,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let message = match self.handler_message(chain, receiver, selector, arguments) {
            Ok(message) => message,
            Err(answered) => return answered,
        };
        self.decide_slot_accessor(
            receiver,
            kind,
            Symbol::DOES_NOT_UNDERSTAND,
            field,
            &[message],
        )
    }

    /// A `doesNotUnderstand:` handler that is a primitive. It loops, for the
    /// reason `act_above` recurses: a handler that is a primitive can fail --
    /// a platform object handed `Object`'s `doesNotUnderstand:` still
    /// answers it, but a primitive that does not recognize its receiver
    /// passes its level over and the walk continues above it. Nothing here
    /// is a hot path: reaching it at all means a send found no method.
    #[cold]
    #[inline(never)]
    fn act_handler_primitive(
        &mut self,
        primitive: Primitive,
        level: ClassReference<'gc>,
        chain: Chain,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let message = match self.handler_message(chain, receiver, selector, arguments) {
            Ok(message) => message,
            Err(answered) => return answered,
        };
        let mut primitive = primitive;
        let mut level = level;
        loop {
            if let Some(result) = crate::builtins::execute(
                self,
                primitive,
                receiver,
                Symbol::DOES_NOT_UNDERSTAND,
                &[message],
            ) {
                return result;
            }
            match dispatch::lookup_above(
                &self.heap,
                Rule::DefinedFor,
                level,
                Symbol::DOES_NOT_UNDERSTAND,
            ) {
                Lookup::Method(method, defining_class) => {
                    return Ok(handler_method_step(
                        receiver,
                        method,
                        defining_class,
                        message,
                    ));
                }
                Lookup::SlotAccessor(accessor, field) => {
                    return self.decide_slot_accessor(
                        receiver,
                        accessor.kind,
                        Symbol::DOES_NOT_UNDERSTAND,
                        field,
                        &[message],
                    );
                }
                Lookup::Primitive(above, above_level) => {
                    primitive = above;
                    level = above_level;
                }
                // As in `resolve_handler`: a nested declaration is never the
                // handler.
                Lookup::NotFound | Lookup::NestedClass(..) => {
                    return Err(unhandled_error(receiver, selector));
                }
            }
        }
    }

    /// The message every `doesNotUnderstand:` handler takes -- or, on
    /// §5.10's chain, the instantiation that answers the send instead. The
    /// factory check comes first there, before any handler runs, as §5.10's
    /// failure path starts with it; a resolution rebuilt from a cache entry
    /// keyed on S does not cover it, since two class objects inheriting one
    /// class-side method can have different factory selectors.
    ///
    /// The message is a minimal message-mirror-shaped value: an Array
    /// `{selector. arguments}` rather than a real MessageMirror object, since
    /// mirrors do not exist yet (`docs/design/liveness-and-reflection.md`,
    /// "Mirrors and MirrorBuilder"). Callers relying on a real
    /// `MessageMirror` API are out of scope. It is built once the handler is
    /// found; allocation order is unobservable, identity hashes being drawn
    /// lazily.
    fn handler_message(
        &mut self,
        chain: Chain,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Value<'gc>, Result<Step<'gc>, Signal<'gc>>> {
        if chain == Chain::SuperDoesNotUnderstand {
            if let Some(answered) = self.try_factory_send(&receiver, selector, arguments) {
                return Err(answered);
            }
        }
        let selector_value = self.new_symbol(selector);
        let arguments_value = Value::Object(self.new_array(arguments.to_vec()));
        Ok(Value::Object(
            self.new_array(vec![selector_value, arguments_value]),
        ))
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
    Signal::Error(NewspeakError {
        selector,
        receiver,
        message: format!("doesNotUnderstand: {selector} (and no doesNotUnderstand: found anywhere in the class chain)"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// `resolve` decides and runs nothing; each send kind's resolution names
    /// the chain it was found on (`docs/design/dispatch.md`, "Resolve, then
    /// act").
    #[test]
    fn resolving_names_the_chain_and_runs_nothing() {
        let source = indoc! {"
            Newspeak3
            'T'
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
            assert!(matches!(
                mutator.resolve_public(class_object, Symbol::intern("make")),
                Resolved::Method {
                    chain: Chain::PublicClassSide,
                    ..
                }
            ));
            assert!(matches!(
                mutator.resolve_public(class_object, Symbol::NEW),
                Resolved::Factory {
                    chain: Chain::Public
                }
            ));
            assert!(matches!(
                mutator.resolve_public(Value::SmallInteger(3), Symbol::intern("+")),
                Resolved::Primitive {
                    chain: Chain::Public,
                    ..
                }
            ));
            assert!(matches!(
                mutator.resolve_public(receiver, Symbol::intern("zork")),
                Resolved::Primitive {
                    chain: Chain::DoesNotUnderstand,
                    ..
                }
            ));
            assert_eq!(mutator.heap().activation_depth(), 0, "nothing was pushed");
        });
    }

    /// A resolution that `act` runs answers what `decide_send` answered
    /// before the split: a native leaf is its value.
    #[test]
    fn acting_on_a_primitive_answers_its_value() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let resolved = mutator.resolve_public(Value::SmallInteger(3), Symbol::intern("+"));
            match mutator.act(
                resolved,
                Value::SmallInteger(3),
                None,
                Symbol::intern("+"),
                &[Value::SmallInteger(4)],
            ) {
                Ok(Step::Value(Value::SmallInteger(7))) => {}
                _ => panic!("3 + 4 is 7"),
            }
        });
    }

    /// Every resolution the hot path returns stays small: 3a measured a
    /// 64-byte decision on the return path at 5.5-22.4% (`execution.md`,
    /// "What primitives as method-table entries moved"). 16 bytes in a
    /// release build, below `Lookup`'s 24: a `Slot` carries the accessor's
    /// kind rather than the whole `SlotAccessor`, and the variant tag fits
    /// in the bytes every variant leaves spare beside its 8-byte field.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn a_resolution_stays_the_size_the_hot_path_was_measured_at() {
        assert_eq!(std::mem::size_of::<Resolved>(), 16);
    }
}

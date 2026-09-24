//! Class instantiation: `newspeak-spec.pdf` §6.1, §6.3. See the runtime
//! spec's Class Instantiation section for the six numbered steps this
//! transcribes.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::hash::IdentityMap;
use crate::heap::{Heap, Root};
use crate::mixin::{
    slot_accessors, Factory, MethodEntry, Mixin, MixinApplicationClause, MixinMethod, MixinSlot,
    SuperclassClause,
};
use crate::types::*;
use crate::Symbol;

use super::{HostError, HostResult, HostSignal, HostStart, Interpreter, Mutator, Signal, Step};

/// How many finished walks' boxes `Mutator::retire_walk` keeps for
/// reuse, and how many levels of room each keeps. One box serves any number
/// of instantiations in sequence; more are held at once only while
/// instantiations nest bodies that have work after them.
const SPARE_WALKS: usize = 16;
const SPARE_LEVELS: usize = 64;

/// A class body's method table: its methods and the accessors its `slots`
/// define (`newspeak-spec.pdf` §6.3.2). Everything a lookup or
/// `Mutator::push_step` reads is lifted out of the declaration here, so that
/// a mixin decoded from an image -- which has no declaration at all --
/// answers the same questions from the same fields. No selector is declared
/// twice: the resolver has already refused a body that declares one member
/// twice (`resolver::classes::duplicate_member_names`).
fn build_methods(
    body: &ast::ClassBody,
    slots: &[MixinSlot],
    bindings: &Rc<HashMap<Span, Binding>>,
    kernel: bool,
    lowering: crate::compiler::Lowering,
) -> IdentityMap<Symbol, MethodEntry> {
    let mut methods: IdentityMap<Symbol, MethodEntry> = slot_accessors(slots).into_iter().collect();
    for declaration in &body.methods {
        let method =
            MixinMethod::from_declaration(Rc::new(declaration.clone()), bindings, kernel, lowering);
        let previous = methods.insert(method.selector, MethodEntry::Compiled(Rc::new(method)));
        debug_assert!(
            previous.is_none(),
            "the resolver refuses a duplicate member"
        );
    }
    methods
}

fn build_slots(body: &ast::ClassBody) -> Vec<MixinSlot> {
    body.slots
        .iter()
        .map(|slot| MixinSlot::from_declaration(Rc::new(slot.clone())))
        .collect()
}

/// The run-time-visible half of a superclass clause. The selector defaults
/// to `new` for an omitted message, per `newspeak-spec.pdf` §6.1 -- resolved
/// once, here, so no reader of the clause re-derives it from the
/// declaration.
fn superclass_clause_of(clause: &ast::SuperclassClause) -> SuperclassClause {
    SuperclassClause {
        selector: super::nested::superclass_clause_message(clause)
            .map_or(Symbol::NEW, crate::compiler::lower::selector_of),
        has_arguments: super::nested::superclass_clause_message_arguments_expression(clause)
            .is_some(),
    }
}

/// One `<:` clause's run-time half, read off the clause exactly as
/// `superclass_clause_of` reads a superclass clause: §6.2 builds each one from
/// §6.1's `inheritanceClause`, so the selector defaults to `new` the same way.
/// Its message body is the compiler's to fill.
fn mixin_application_clause_of(clause: &ast::SuperclassClause) -> MixinApplicationClause {
    let SuperclassClause {
        selector,
        has_arguments,
    } = superclass_clause_of(clause);
    MixinApplicationClause {
        selector,
        has_arguments,
        message_code: None,
    }
}

impl Interpreter {
    /// Materializes the module's root `ClassDeclaration` into a real class
    /// object. The superclass clause's *receiver* is resolved here, once -- it
    /// is what fixes the superclass (`newspeak-spec.pdf` §6.1). Its trailing
    /// *message* is deliberately not touched here: §6.2 defers it to every
    /// instantiation, where `climb_levels` evaluates it off the mixin.
    ///
    /// Building the mixin tree (`build_mixin`, below) is a separate step from
    /// applying it: a nested mixin is reachable the moment its enclosing
    /// mixin exists, but *applying* a mixin -- resolving a superclass and
    /// computing a slot layout -- only happens for the root here. A nested
    /// mixin gets its own application when something actually materializes
    /// it (the accessor); until then it is pure, unapplied structure.
    ///
    /// Every method and every per-mixin body is compiled onto the
    /// declaration it belongs to as the tree is built, so a class this
    /// returns is already runnable. It is compiled with the lowering this
    /// interpreter was bootstrapped with. The class comes back rooted, for
    /// the host that will instantiate it.
    pub fn load_module(&mut self, resolved: &ResolvedModule) -> Root {
        self.enter(|mutator| {
            let class = mutator.load_root_class(resolved);
            mutator.stash(Value::Object(class))
        })
    }

    /// Instantiates the class `class` holds, from a host: the six numbered
    /// steps in `docs/design/object-model.md`, "Instantiation", decided
    /// (`decide_instantiation`) and then run from the outside
    /// (`run_from_host`), like a host's `send`. A root that holds anything
    /// but a class object is refused as a Newspeak error, never a panic:
    /// the caller is a host.
    pub fn instantiate(&mut self, class: &Root, factory_arguments: &[Root]) -> HostResult {
        let depth = self.world.activation_depth();
        let start = self.enter(|mutator| {
            let value = mutator.fetch(class);
            let class = match value {
                Value::Object(object) if mutator.heap.is_class(object) => object,
                _ => {
                    return HostStart::Finished(Err(HostSignal::Error(HostError {
                        selector: Symbol::NEW,
                        receiver: mutator.stash(value),
                        message: format!(
                            "cannot instantiate {}: it is not a class",
                            mutator.heap.describe(&value)
                        ),
                    })))
                }
            };
            let factory_arguments: Vec<Value> = factory_arguments
                .iter()
                .map(|argument| mutator.fetch(argument))
                .collect();
            let decided = mutator.decide_instantiation(class, factory_arguments);
            mutator.start(decided, &[])
        });
        self.run_from_host(start, depth)
    }
}

impl<'gc> Mutator<'gc> {
    /// `load_module`'s work, inside its entry: the root class, built and
    /// compiled, for `load_module` to root.
    pub(crate) fn load_root_class(&mut self, resolved: &ResolvedModule) -> ClassReference<'gc> {
        let declaration = &resolved.module.root_class;
        let mixin = self.build_root_mixin(declaration, &resolved.bindings, resolved.kernel);
        // The rule leaves three shapes, and a defective root is built as the
        // first so that its class exists, answers its class side and can be
        // inspected -- only an instance is refused (`apply_mixin` carries the
        // defect into the application).
        let (superclass_kind, applies_value) = match &mixin.root_clause_defect {
            Some(_) => (BuiltinClass::Object, false),
            None => {
                let names_value = declaration
                    .superclass
                    .as_ref()
                    .is_some_and(|clause| clause.receiver.name == "Value");
                let superclass_kind = if names_value {
                    BuiltinClass::Value
                } else {
                    BuiltinClass::Object
                };
                (superclass_kind, !declaration.mixin_applications.is_empty())
            }
        };
        let superclass = Some(self.builtin_class(superclass_kind));
        let applied: Vec<(usize, Value<'gc>)> = if applies_value {
            vec![(0, Value::Object(self.builtin_class(BuiltinClass::Value)))]
        } else {
            Vec::new()
        };
        let superclass =
            match super::nested::build_applied_levels(self, &mixin, superclass, &applied) {
                Ok(top) => top,
                Err(_) => panic!("internal: every builtin class is a class object"),
            };
        // The slot layout is the application's job, not the class object's:
        // `apply_mixin` offsets this class's own slots past every inherited
        // one (`own_slot_base`), so a class object never computes -- or can
        // disagree about -- an instance's shape.
        let super_application = superclass
            .as_ref()
            .and_then(|superclass| self.heap.class_application(*superclass));
        let application = self.apply_mixin(&mixin, super_application.as_ref());
        // `enclosing`/`declaring_class` are both `None` -- the module's root
        // class is the outermost level, so no object encloses it and no
        // declaration contains it. Every other class object in the system
        // comes from `interpreter::nested`, which passes both.
        self.new_class_object(application, None, None, superclass)
    }

    /// Builds the mixin for `declaration`, recursively including its nested
    /// declarations and its class side. Pure structure -- no Newspeak code is
    /// evaluated here, which is what keeps module loading total.
    ///
    /// Every `declaration.instance_side.nested_classes` entry becomes a `Mixin`
    /// in `nested`, in declaration order, and `declaration.class_side` becomes
    /// a `Mixin` too, instead of being discarded. Neither is *applied* here (no
    /// superclass is resolved, no slot layout computed, no class object built)
    /// -- a nested mixin has no class object of its own until something
    /// materializes it (the accessor), and a class side until
    /// `Mutator::metaclass_of` builds the metaclass that applies it.
    ///
    /// `kernel` is the module's `ResolvedModule.kernel`, carried down to
    /// every method so that only a kernel method can carry a fast path
    /// (`MixinMethod::from_declaration`).
    pub(crate) fn build_mixin(
        &mut self,
        declaration: &ast::ClassDeclaration,
        bindings: &Rc<HashMap<Span, Binding>>,
        kernel: bool,
    ) -> Rc<Mixin> {
        Rc::new(self.mixin_of(declaration, bindings, kernel))
    }

    /// A module's root mixin: `build_mixin`'s, with the root rule's verdict
    /// recorded on it for `apply_mixin` and the encoder to read.
    pub(crate) fn build_root_mixin(
        &mut self,
        declaration: &ast::ClassDeclaration,
        bindings: &Rc<HashMap<Span, Binding>>,
        kernel: bool,
    ) -> Rc<Mixin> {
        let mut mixin = self.mixin_of(declaration, bindings, kernel);
        mixin.root_clause_defect = root_clause_defect(declaration);
        Rc::new(mixin)
    }

    /// `build_mixin`'s work, before the mixin is shared.
    fn mixin_of(
        &mut self,
        declaration: &ast::ClassDeclaration,
        bindings: &Rc<HashMap<Span, Binding>>,
        kernel: bool,
    ) -> Mixin {
        let slots = build_slots(&declaration.instance_side);
        let methods = build_methods(
            &declaration.instance_side,
            &slots,
            bindings,
            kernel,
            self.lowering,
        );
        let nested = declaration
            .instance_side
            .nested_classes
            .iter()
            .map(|nested| self.build_mixin(nested, bindings, kernel))
            .collect();
        // §6's unnumbered introduction, before §6.1 begins: "If the
        // signature is omitted it defaults to `new`" -- a class written
        // without a factory pattern is instantiated by `new`. A separate
        // rule, §6.1's own default for an omitted superclass-clause
        // *message*, defaults to `new` too -- the same selector, not the
        // same rule -- so `class C = (...)` inherits from `Object` through
        // `new` by two independent defaults landing on one value. Resolved
        // before the class side is built, because §6.3 makes the factory a
        // member of the metaclass mixin the class side becomes.
        let factory = match &declaration.factory {
            Some(pattern) => Factory {
                selector: Symbol::intern(&method_name(pattern)),
                argument_count: pattern_parameter_count(pattern),
            },
            None => Factory {
                selector: Symbol::NEW,
                argument_count: 0,
            },
        };
        let class_side = declaration
            .class_side
            .as_ref()
            .map(|side| self.build_class_side(side, factory, bindings, kernel));
        let mut mixin = Mixin {
            name: Symbol::intern(&declaration.name.name),
            access: crate::dispatch::effective_access(declaration.access),
            methods,
            slots,
            factory: Some(factory),
            superclass: declaration.superclass.as_ref().map(superclass_clause_of),
            mixin_applications: declaration
                .mixin_applications
                .iter()
                .map(mixin_application_clause_of)
                .collect(),
            has_initializer_expressions: !declaration
                .instance_side
                .initializer_expressions
                .is_empty(),
            // This class's own initExprs, lowered into the instance
            // initializer by `compiler::compile_mixin_initializer` (step 5 of
            // the six in `docs/design/object-model.md`, "Instantiation").
            initializer_expressions: declaration.instance_side.initializer_expressions.clone(),
            nested,
            class_side,
            span: declaration.span,
            declaration: Some(Rc::new(declaration.clone())),
            initializer_code: None,
            superclass_clause_code: None,
            superclass_message_code: None,
            root_clause_defect: None,
        };
        crate::compiler::compile_mixin_bodies(&mut mixin, bindings, self.lowering);
        mixin
    }

    /// The metaclass mixin a class side becomes: `newspeak-spec.pdf` §6.3's
    /// "the class side, together with the primary factory method". So
    /// `factory` -- the enclosing declaration's, since a class side declares
    /// no factory pattern of its own -- joins the class side's own members as
    /// an entry of this method table, found by the ordinary lookups like any
    /// other. Before that it was a case ahead of the lookup, which cost every
    /// send to a class object a factory-selector compare and needed a
    /// re-check wherever a cache could record the decision.
    ///
    /// Kept separate from `build_mixin` and deliberately small: a class side
    /// has no nested classes of its own to load -- only its instance side's
    /// `nested_classes` recurse. `declaration` is `None` because a
    /// `ClassBody` is not itself an `ast::ClassDeclaration`; there is no
    /// single declaration node to keep as provenance for it.
    fn build_class_side(
        &mut self,
        side: &ast::ClassBody,
        factory: Factory,
        bindings: &Rc<HashMap<Span, Binding>>,
        kernel: bool,
    ) -> Rc<Mixin> {
        let slots = build_slots(side);
        let mut methods = build_methods(side, &slots, bindings, kernel, self.lowering);
        let (selector, entry) = crate::mixin::factory_entry(Some(factory));
        methods.insert(selector, entry);
        let mut mixin = Mixin {
            methods,
            slots,
            has_initializer_expressions: !side.initializer_expressions.is_empty(),
            initializer_expressions: side.initializer_expressions.clone(),
            span: side.span,
            ..Mixin::empty(Symbol::CLASS_SIDE)
        };
        crate::compiler::compile_mixin_bodies(&mut mixin, bindings, self.lowering);
        Rc::new(mixin)
    }

    /// §6.3's instantiation, the action of a factory entry: a fresh instance
    /// of `class`, then its instance initializer (`newspeak-spec.pdf` §6.3:
    /// "the instance initializer is invoked on an instance *o* of the
    /// class").
    ///
    /// No re-check of what the lookup decided: the entry was found on the
    /// receiver's own metaclass, whose dispatch id is the class-side guard a
    /// cache entry is keyed by, so a hit and a miss reach the same class
    /// (§3.2 makes a metaclass flat, so there is no inherited class side an
    /// entry could have come from). `class` is the receiver itself, read per
    /// send rather than carried in an entry.
    ///
    /// No double-dispatch risk against the superclass clause's own factory
    /// message (`Object new` in `class Foo = Object new (...)`). That message
    /// is *evaluated* (`climb_levels` below), but never *sent*: §6.2 has the
    /// subclass's initializer invoke the superclass's instance initializer
    /// directly on the same object, so `climb_levels` runs it level by level
    /// and never decides a send -- and so never reaches here, which would
    /// otherwise allocate a second, discarded instance per level.
    pub(super) fn decide_factory(
        &mut self,
        class: ClassReference<'gc>,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        match self.heap.class_builtin(class) {
            // The VM mirror is a capability: there is exactly one, and it is
            // the bootstrap's to hand out (`docs/design/core-library.md`, "The
            // VM mirror is the whole Rust boundary").
            Some(BuiltinClass::VmMirror) => Err(Signal::Error(NewspeakError {
                selector: Symbol::NEW,
                receiver: Value::Object(class),
                message: "the VM mirror's class cannot be instantiated".to_string(),
            })),
            // `Array`/`OrderedCollection`'s representation is
            // `ObjectData::Array`, not `decide_instantiation`'s
            // `ObjectData::Instance` -- `builtins::collections`' primitives
            // pattern-match on `ObjectData::Array` directly, so handing them
            // a generic instance payload would leave every native method on
            // the result silently unable to find its data (falling straight
            // through to `doesNotUnderstand:` on the very next send). Built
            // directly with `new_instance_object`, tagged with `class`
            // itself (the receiver's own class -- *not* `Mutator::new_array`,
            // which unconditionally tags its result `BuiltinClass::Array`;
            // using that here would make `OrderedCollection new`'s result an
            // `Array`, whose method table has no `add:`).
            Some(BuiltinClass::OrderedCollection) | Some(BuiltinClass::Array) => Ok(Step::Value(
                Value::Object(self.new_instance_object(class, ObjectData::Array(vec![]))),
            )),
            _ => self.decide_instantiation(class, arguments.to_vec()),
        }
    }

    /// Decides an instantiation of `class` with `factory_arguments` without
    /// running any of its code: the instance comes back as a value when no
    /// level of the superclass chain has code to run, and otherwise the first
    /// body that must run, with the rest of the walk in its continuation
    /// (`Continuation::Instantiate`, carried on by `advance_instantiation`)
    /// when there is any. Steps 2-5 are `climb_levels` and `descend_levels`,
    /// for every level of the superclass chain.
    ///
    /// The walk runs in a box of the interpreter's spares (`walk_state`),
    /// which rides in the continuation of a body that has work left after it
    /// and goes back to the spares when the walk ends, so once one
    /// instantiation has run, the next allocates the instance's fields and
    /// nothing else (`descend_levels` says why its last body needs no
    /// continuation).
    pub(crate) fn decide_instantiation(
        &mut self,
        class: ClassReference<'gc>,
        factory_arguments: Vec<Value<'gc>>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let application = self.heap.class_application(class);
        // Asked before anything is allocated: a class whose instances could
        // never be what §3.1.1 or the root rule demands has no instance to
        // make. The answer was computed once, when the application was built
        // (`Mutator::apply_mixin`).
        if let Some(application) = application.as_ref() {
            if let Some(refusal) = &application.instantiation_refusal {
                return Err(Signal::Error(instantiation_refusal_error(
                    class,
                    application,
                    refusal,
                )));
            }
            // §3.1.1's other class-level condition, every enclosing object a
            // value (`interpreter::value_class::check_value_class`) --
            // decided when `class` was created and only recorded then
            // (`ClassData::value_class_refusal`); refused here, not there,
            // because the specification's own §2.3 example creates such a
            // class and never instantiates it
            // (`docs/design/object-model.md`, "A value object's four
            // conditions"). Behind `inherits_value` so a class that is no
            // value pays nothing beyond the flag it already carries, and a
            // presence test rather than a read of the refusal, so the hot
            // path copies nothing: only the cold error path clones it.
            if application.inherits_value && self.heap.class_has_value_refusal(class) {
                return Err(Signal::Error(value_class_refusal_error(
                    &self.heap,
                    class,
                    application,
                )));
            }
        }
        // Step 1: one instance, every slot nil. The application already
        // knows the whole layout, inherited slots included -- summing the
        // chain here again would be a second answer to the same question.
        let slot_count = application.map_or(0, |application| application.slot_count);
        let instance =
            self.new_instance_object(class, ObjectData::Instance(vec![Value::Nil; slot_count]));
        let state = self.walk_state(Value::Object(instance));
        self.climb_levels(state, class, factory_arguments)
    }

    /// The box a new walk of `instance`'s instantiation runs in: a spare one
    /// if a finished walk left one, so that neither the box nor its list of
    /// levels is allocated again.
    fn walk_state(&mut self, instance: Value<'gc>) -> Box<Instantiation<'gc>> {
        match self.spare_walks.pop() {
            Some(mut state) => {
                state.instance = instance;
                state
            }
            None => Box::new(Instantiation {
                instance,
                levels: Vec::new(),
                applying: 0,
                deferred: Vec::new(),
                // Never read: the phase is written wherever the box is handed
                // to a continuation, the one place that reads it.
                phase: InstantiationPhase::Descending { index: 0 },
            }),
        }
    }

    /// A finished walk's box, back to the spares, emptied so that it holds
    /// no handle. Bounded twice: the boxes held at once grow with how deeply
    /// instantiations nest -- each body with work after it holds one -- and
    /// the list of levels with the superclass chain, whose length is an
    /// image's to choose; neither peak should stay allocated for the rest of
    /// the run.
    fn retire_walk(&mut self, mut state: Box<Instantiation<'gc>>) {
        if self.spare_walks.len() < SPARE_WALKS {
            state.levels.clear();
            state.deferred.clear();
            state.levels.shrink_to(SPARE_LEVELS);
            state.instance = Value::Nil;
            // A walk that climbed through a clause message with arguments and
            // then found nothing to initialize below it retires straight out
            // of `descend_levels`'s "nothing below" branches, which never
            // write `Descending` into `phase` -- so without this, the box
            // would still hold the last `Climbing`'s two `ClassReference`s.
            // `Descending { index: 0 }` is otherwise never read (`walk_state`'s
            // comment), so any handle-free variant would do.
            state.phase = InstantiationPhase::Descending { index: 0 };
            self.spare_walks.push(state);
        }
    }

    /// One body an instantiation ran has returned `value`: carry the walk on
    /// from where `state` says it stopped. `Mutator::resume` calls this
    /// with its continuation's box, in which the walk goes on until it answers
    /// another body.
    pub(crate) fn advance_instantiation(
        &mut self,
        state: Box<Instantiation<'gc>>,
        value: Value<'gc>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        match state.phase {
            InstantiationPhase::Climbing { level, superclass } => {
                let super_arguments = argument_list(&self.heap, level, value)?;
                self.climb_levels(state, superclass, super_arguments)
            }
            // An initializer's value is not used; only its effects on the
            // instance are.
            InstantiationPhase::Descending { index } => self.descend_levels(state, index),
            InstantiationPhase::Applying { index, applying } => {
                self.advance_past_applied_message(state, index, applying, value)
            }
        }
    }

    /// `advance_instantiation` once the message into the applied level
    /// `levels[index]`, written in `levels[applying]`'s declaration, has
    /// answered `value`, that level's factory arguments. Out of line, so that
    /// the resumption of every other walk carries none of it.
    #[cold]
    #[inline(never)]
    fn advance_past_applied_message(
        &mut self,
        mut state: Box<Instantiation<'gc>>,
        index: usize,
        applying: usize,
        value: Value<'gc>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let arguments = argument_list(&self.heap, state.levels[applying].0, value)?;
        // The climb passed this level with its arguments still to come, so
        // its arity is checked now, before its initializer.
        self.check_factory_arity(state.levels[index].0, &arguments)?;
        state.levels[index].1 = arguments;
        // `index + 1`: the level itself is the next one to initialize, if it
        // has code.
        self.descend_levels(state, index + 1)
    }

    /// The upward walk, from `level` with `arguments`: each level's factory
    /// arity checked, then the message into its superclass evaluated to the
    /// superclass's arguments (step 3's first half). A message with arguments
    /// is code, so the walk stops there and answers its activation, and
    /// `advance_instantiation` resumes it with the value. Walked with a loop
    /// and a list rather than by recursing per level: the chain's length is an
    /// image's to choose, and a level with no code pushes no activation, so the
    /// activation limit would not see a long chain of them
    /// (`docs/design/execution.md`, "A superclass chain is walked
    /// iteratively"). At the top of the chain, the downward walk begins.
    ///
    /// §6.2's mixin application is the one thing that bends this shape. For
    /// `class C = S ms <: M1 m1 <: M2 m2`, the chain is `C`, `I2`, `I1`, `S`,
    /// and every message on the way from `C` to `S` is written in `C`'s
    /// declaration: `m2` into `I2`, `m1` into `I1`, `ms` into `S`, each
    /// evaluated with `C`'s factory arguments. So the message into an applied
    /// level is the clause that level was built for, and the message out of
    /// the last applied level is `C`'s own superclass clause -- which is why
    /// the walk keeps the index of `C` (`Instantiation::applying`) while it
    /// climbs through `C`'s levels. An ordinary level is its own applying
    /// level, so for it this is exactly the walk it always was: its own
    /// clause, with its own arguments.
    ///
    /// That is why this loop is the ordinary walk alone, and the first
    /// applied level it meets hands the rest of the climb to
    /// `climb_through_applied_levels`, out of line: a program that writes no
    /// `<:` pays one test per level for the mixin application, and nothing
    /// else. A walk the applied climb stopped for a clause message resumes
    /// here, at a superclass that begins a declaration of its own.
    fn climb_levels(
        &mut self,
        mut state: Box<Instantiation<'gc>>,
        mut level: ClassReference<'gc>,
        mut arguments: Vec<Value<'gc>>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        loop {
            self.check_factory_arity(level, &arguments)?;
            let Some(superclass) = self.heap.class_superclass(level) else {
                state.levels.push((level, arguments));
                let above_top = state.levels.len();
                return self.descend_levels(state, above_top);
            };
            if self.heap.class_is_applied_level(superclass) {
                state.levels.push((level, arguments));
                return self.climb_through_applied_levels(state, superclass);
            }
            let message = self.clause_message_body(level, superclass)?;
            let Some(code) = message else {
                // `Object new` and every other argument-free clause: nothing
                // to run, and the superclass gets no arguments.
                state.levels.push((level, arguments));
                level = superclass;
                arguments = Vec::new();
                continue;
            };
            // The message is evaluated on the instance being initialized and
            // with *this* level's factory arguments -- §6.3 runs each level's
            // slot clauses and statement "in the context of *i*".
            //
            // No `check_message_arity` here, unlike the applied climb: an
            // ordinary level's message body lives on its own mixin and is
            // compiled for its own factory -- by the compiler from source,
            // and held to it by `image::decode::verify_mixins` for an image
            // -- and `check_factory_arity` has just matched `arguments` to
            // that factory. Only a `<:` clause's body runs with another
            // level's arguments.
            let message_arguments = arguments.clone();
            state.levels.push((level, arguments));
            state.phase = InstantiationPhase::Climbing { level, superclass };
            return Ok(Step::ClauseMessage {
                level,
                code,
                arguments: message_arguments,
                // Its value is the superclass's arguments: the walk has
                // everything above this level still to do.
                then: state,
            });
        }
    }

    /// The rest of the upward walk from the last level in `state.levels`,
    /// whose superclass `superclass` is a level a mixin application added,
    /// to the top of the chain or the next clause message to run.
    ///
    /// Every message on the way from a declaration up through its applied
    /// levels is written in that declaration (`climb_levels`), so the walk
    /// keeps its index (`Instantiation::applying`) until it reaches a
    /// superclass that is no applied level and so begins a declaration of
    /// its own.
    ///
    /// §6.2 also orders the messages: `I1`'s initializer "evaluates ms to μs
    /// … invokes the instance initializer of S on o with the arguments of μs.
    /// Then, m is evaluated to μm". So only `ms` is evaluated on the way up;
    /// a message into an applied level has its selector checked here, where
    /// nothing has run yet, and is then left in `Instantiation::deferred` for
    /// the downward walk to evaluate just before that level's initializer --
    /// and the level's factory arity is checked once its arguments exist
    /// (`advance_instantiation`).
    #[cold]
    #[inline(never)]
    fn climb_through_applied_levels(
        &mut self,
        mut state: Box<Instantiation<'gc>>,
        mut superclass: ClassReference<'gc>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        // The level just pushed is the declaration the applied levels above
        // it belong to.
        state.applying = state.levels.len() - 1;
        loop {
            let applying_index = state.applying;
            let applying = state.levels[applying_index].0;
            let applied_clause = self.heap.class_applied_clause(superclass);
            // The superclass begins a declaration of its own unless it is one
            // of the applying declaration's levels, whose messages are still
            // the applying declaration's.
            let into_applied_level = applied_clause.is_some();
            if !into_applied_level {
                state.applying = state.levels.len();
            }
            let message = match applied_clause {
                Some(clause) => self.applied_clause_message_body(applying, superclass, clause)?,
                None => self.clause_message_body(applying, superclass)?,
            };
            // Whether the superclass's arguments are a deferred message's,
            // not yet evaluated, so its arity cannot be checked yet.
            let mut arguments_deferred = false;
            if let Some(code) = message {
                // The message is evaluated on the instance being initialized
                // and with the factory arguments of the declaration it is
                // written in -- §6.3 runs each level's slot clauses and
                // statement "in the context of *i*".
                //
                // Checked for both kinds of message, though only a `<:`
                // clause's needs it. A clause's body is compiled for the
                // applying declaration's factory and runs with the arguments
                // of the level below the applied ones, and an image may put a
                // level above a class that is not its applying row
                // (`verify_mixins` checks the body against the row alone), so
                // the two can disagree. `ms` -- the `None` arm above -- is the
                // applying level's own superclass clause, compiled for its own
                // factory, whose arity `check_factory_arity` has already
                // matched those arguments to, so for it the check cannot fail,
                // exactly as `climb_levels` says of the ordinary climb. It
                // stays unconditional so the two arms share one path: this
                // walk is cold, and a program without `<:` never reaches it.
                self.check_message_arity(&code, applying, &state.levels[applying_index].1)?;
                if !into_applied_level {
                    let message_arguments = state.levels[applying_index].1.clone();
                    state.phase = InstantiationPhase::Climbing {
                        level: applying,
                        superclass,
                    };
                    return Ok(Step::ClauseMessage {
                        level: applying,
                        code,
                        arguments: message_arguments,
                        // Its value is the superclass's arguments: the walk
                        // has everything above this level still to do.
                        then: state,
                    });
                }
                state.deferred.push(DeferredMessage {
                    level: state.levels.len(),
                    applying: applying_index,
                    code,
                });
                arguments_deferred = true;
            }
            // `Object new` and every other argument-free clause: nothing to
            // run, and the superclass gets no arguments; a deferred message's
            // arrive on the way down.
            let arguments = Vec::new();
            if !arguments_deferred {
                self.check_factory_arity(superclass, &arguments)?;
            }
            let Some(next) = self.heap.class_superclass(superclass) else {
                state.levels.push((superclass, arguments));
                let above_top = state.levels.len();
                return self.descend_levels(state, above_top);
            };
            state.levels.push((superclass, arguments));
            superclass = next;
        }
    }

    /// The downward walk (steps 2, 4 and 5): below `next`, from the top of the
    /// chain down to the class instantiated, each level's own slot clauses and
    /// `initExprs`, in an activation of its own with the arguments its own
    /// subclass's clause supplied -- one activation per initializing level, so
    /// a superclass's initializers see the arguments *its* clause supplied. A
    /// level with nothing to initialize runs no code and is passed over --
    /// every chain ends at a builtin, which is exactly such a level -- and the
    /// first that has code is answered as an activation;
    /// `advance_instantiation` carries on below it when it returns, unless
    /// nothing below it has code. Past the last level, the instance (step 6).
    ///
    /// A walk the climb left messages into applied levels to evaluate
    /// descends through `descend_to_deferred_message` instead, until the
    /// last of them has run.
    fn descend_levels(
        &mut self,
        mut state: Box<Instantiation<'gc>>,
        next: usize,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        if !state.deferred.is_empty() {
            return Ok(self.descend_to_deferred_message(state, next));
        }
        let instance = state.instance;
        let Some(index) = initializing_level_below(&self.heap, &state.levels, next) else {
            self.retire_walk(state);
            return Ok(Step::Value(instance));
        };
        let level = state.levels[index].0;
        let arguments = std::mem::take(&mut state.levels[index].1);
        // With no level below this one left to initialize, this is the last
        // body of the walk, so all that is left once it returns is step 6,
        // answering the instance -- which is exactly what the initializer
        // answers: the compiler ends every instance initializer by answering
        // `self` and discards a top-level `^` (`compiler::lower`'s
        // `finish_body`, `compiler::reference`'s `end`). So its activation
        // delivers its own value like a method's, and nothing of the walk
        // outlives this step: its box goes back to the spares now. Making an
        // initializer answer anything else breaks `new`;
        // `tests/instantiate.rs`'s
        // `new_answers_the_instance_its_initializers_ran_on` pins it. (A
        // decoded image's initializer is not held to this; `new` then answers
        // what that code answers, which is the image's program running, not a
        // fault.)
        if initializing_level_below(&self.heap, &state.levels, index).is_none() {
            self.retire_walk(state);
            return Ok(Step::Initializer {
                receiver: instance,
                level,
                arguments,
            });
        }
        state.phase = InstantiationPhase::Descending { index };
        Ok(Step::InitializerThen {
            level,
            arguments,
            then: state,
        })
    }

    /// `descend_levels` below `next` while the climb has left messages into
    /// applied levels to evaluate. Out of line, so that a walk with nothing
    /// deferred -- every walk of a program that writes no `<:` -- carries
    /// none of it.
    ///
    /// The last deferred message is evaluated at the same point of the walk
    /// as its level's initializer would run, before it and after everything
    /// above it (§6.2's "Then, m is evaluated"), whether or not the level
    /// has code of its own. Until it is, a level above it with code is the
    /// next body, and never the walk's last: the message still has to run.
    #[cold]
    #[inline(never)]
    fn descend_to_deferred_message(
        &mut self,
        mut state: Box<Instantiation<'gc>>,
        next: usize,
    ) -> Step<'gc> {
        let deferred_level = state.deferred.last().map_or(0, |deferred| deferred.level);
        debug_assert!(
            deferred_level < next,
            "internal: the downward walk passed a deferred message"
        );
        let initializing = initializing_level_below(&self.heap, &state.levels, next)
            .filter(|&index| index > deferred_level);
        if let Some(index) = initializing {
            let level = state.levels[index].0;
            let arguments = std::mem::take(&mut state.levels[index].1);
            state.phase = InstantiationPhase::Descending { index };
            return Step::InitializerThen {
                level,
                arguments,
                then: state,
            };
        }
        // Its value is the factory arguments of the applied level it is the
        // message into (`InstantiationPhase::Applying`).
        let DeferredMessage {
            level: index,
            applying,
            code,
        } = state
            .deferred
            .pop()
            .expect("internal: descend_levels found a deferred message");
        let (applying_class, arguments) = state.levels[applying].clone();
        state.phase = InstantiationPhase::Applying { index, applying };
        Step::ClauseMessage {
            level: applying_class,
            code,
            arguments,
            then: state,
        }
    }

    /// The compiled instance initializer of `level`, a level
    /// `class_has_initializer_code` has confirmed, and the `slot_base` its
    /// activation runs with.
    pub(super) fn initializer_body(
        &self,
        level: ClassReference<'gc>,
    ) -> (Rc<crate::bytecode::CompiledCode>, usize) {
        let application = self
            .heap
            .class_application(level)
            .expect("internal: class_has_initializer_code confirmed this is a class object");
        let code = application.mixin.initializer_code.clone().unwrap_or_else(|| {
            panic!("internal: class needs an instance initializer and its mixin carries none (a mixin built from source always does; verify_mixins refuses a decoded one)")
        });
        // `InitializeSlot`'s `own_index` is relative to the mixin's own slots,
        // not an absolute field index, because the compiled body is shared by
        // every application of the mixin. This level's own
        // `MixinApplication.own_slot_base`, taken as the activation's
        // `slot_base`, turns it into an absolute index at run time; it is set
        // with the rest of the level's identity, and never changes.
        (code, application.own_slot_base)
    }

    /// Rejects a factory-arity mismatch as a NewspeakError before this
    /// level's activation is pushed or any slot initializer / initExpr runs.
    #[inline(always)]
    fn check_factory_arity(
        &self,
        class: ClassReference<'gc>,
        factory_arguments: &[Value<'gc>],
    ) -> Result<(), Signal<'gc>> {
        // A slot initializer or
        // initExpr may reference a factory parameter as `Binding::Local(i)`,
        // resolved against this class's *declared* factory pattern -- but
        // the activation `descend_levels` answers lays its registers out
        // from the arguments actually supplied, so a wrong count would shift
        // every temporary, and too few would leave `locals.rs`'s
        // `read_local`/`write_local` indexing past the end of its register
        // window.
        //
        // Checked at *every* level, not just the outermost. A level's
        // arguments come from its subclass's clause message, whose
        // selector `clause_message_body` has already matched against
        // this class's primary factory selector -- which pins the arity too,
        // since a selector's keyword count is its parameter count. This
        // guard is what keeps that reasoning from being the only thing
        // between a decoded image's message body and an out-of-bounds index.
        let declared_arity = self.heap.class_application(class).and_then(|application| {
            application
                .mixin
                .factory
                .map(|factory| factory.argument_count)
        });
        if let Some(declared_arity) = declared_arity {
            if factory_arguments.len() != declared_arity {
                return Err(Signal::Error(NewspeakError {
                    selector: Symbol::NEW,
                    receiver: Value::Object(class),
                    message: format!(
                        "wrong number of factory arguments: factory takes {}, got {}",
                        declared_arity,
                        factory_arguments.len()
                    ),
                }));
            }
        }
        Ok(())
    }

    /// Step 3's first half, up to running anything: §6.3's "evaluation of the
    /// message clause given in the class' superclass clause in the context of
    /// *i*" (`newspeak-spec.pdf`), for the message into `superclass` written
    /// in `applying`'s declaration.
    /// Answers the compiled body that evaluates the message's arguments, or
    /// `None` when there are none.
    ///
    /// Into a class that is no level a mixin application added, the message
    /// is `applying`'s superclass clause; into one that is, it is the `<:`
    /// clause that level was built for (`applied_clause_message_body`).
    ///
    /// §6.1 makes two things run time errors here. That the clause's
    /// expression is not a class object is caught where the clause's
    /// *receiver* is evaluated -- once, when the declaration is evaluated
    /// (`interpreter::nested`), not per instantiation. That the message's
    /// selector is not the superclass's primary factory selector is caught
    /// here, because only here are both halves known.
    ///
    /// The selector is checked *before* the arguments are evaluated: a clause
    /// that cannot be satisfied should not first run its arguments' side
    /// effects.
    #[inline(always)]
    fn clause_message_body(
        &self,
        applying: ClassReference<'gc>,
        superclass: ClassReference<'gc>,
    ) -> Result<Option<Rc<crate::bytecode::CompiledCode>>, Signal<'gc>> {
        // One lookup of the application for both questions below: the clause
        // this class was declared with, and the mixin its compiled message
        // body lives on.
        let application = self.heap.class_application(applying);
        let clause = application
            .as_ref()
            .and_then(|application| application.mixin.superclass);
        // §6.1: "if the message is omitted, it defaults to `new`" -- already
        // resolved into `SuperclassClause::selector` when the mixin was
        // built, and read from there rather than from the declaration: a
        // mixin decoded from an image has no declaration, and re-deriving
        // the default here would answer `new` for every one of them, so a
        // class whose superclass has a keyword factory would raise a
        // spurious "not the primary factory message" error on the first
        // instantiation after a load.
        let selector = clause.map_or(Symbol::NEW, |clause| clause.selector);
        let expected = primary_factory_selector(&self.heap, superclass);
        if Some(selector) != expected {
            return Err(not_the_primary_factory_error(
                &self.heap,
                ClauseKind::Superclass,
                applying,
                superclass,
                selector,
                expected,
            ));
        }
        if !clause.is_some_and(|clause| clause.has_arguments) {
            return Ok(None);
        }
        let code = application
            .as_ref()
            .and_then(|application| application.mixin.superclass_message_code.clone())
            .unwrap_or_else(|| {
                panic!("internal: mixin whose superclass clause message takes arguments carries no compiled form for it (a mixin built from source always does; verify_mixins refuses a decoded one)")
            });
        Ok(Some(code))
    }

    /// `clause_message_body` for a message into `superclass`, a level a
    /// mixin application added: the `<:` clause that level was built for,
    /// written in `applying`'s declaration. §6.2 gives the level "a primary
    /// factory method with the same selector as" that clause's message, and
    /// the named class's initializer takes its arguments, so the clause is
    /// checked against the named class's primary factory -- the level's own.
    fn applied_clause_message_body(
        &self,
        applying: ClassReference<'gc>,
        superclass: ClassReference<'gc>,
        clause: MixinApplicationClause,
    ) -> Result<Option<Rc<crate::bytecode::CompiledCode>>, Signal<'gc>> {
        let expected = primary_factory_selector(&self.heap, superclass);
        if Some(clause.selector) != expected {
            return Err(not_the_primary_factory_error(
                &self.heap,
                ClauseKind::MixinApplication,
                applying,
                superclass,
                clause.selector,
                expected,
            ));
        }
        if !clause.has_arguments {
            return Ok(None);
        }
        let code = clause.message_code.unwrap_or_else(|| {
            panic!("internal: mixin application clause whose message takes arguments carries no compiled form for it (a level built from source always does)")
        });
        Ok(Some(code))
    }

    /// A clause message's compiled arity is that of the declaration it is
    /// written in, which is what `arguments` were checked against when the
    /// walk reached it -- unless an applied level sits above a class that is
    /// not the declaration it was built for, which no source can build.
    /// Refused rather than pushed, since the activation would lay its
    /// registers out from the wrong count.
    fn check_message_arity(
        &self,
        code: &crate::bytecode::CompiledCode,
        applying: ClassReference<'gc>,
        arguments: &[Value<'gc>],
    ) -> Result<(), Signal<'gc>> {
        if usize::from(code.argument_count) == arguments.len() {
            return Ok(());
        }
        Err(Signal::Error(NewspeakError {
            // The factory message the program sent to the applying class.
            selector: primary_factory_selector(&self.heap, applying).unwrap_or(Symbol::NEW),
            receiver: Value::Object(applying),
            message: format!(
                "a clause message of class {} takes {} factory arguments, got {}",
                class_name(&self.heap, applying),
                code.argument_count,
                arguments.len()
            ),
        }))
    }
}

/// Which of a declaration's clauses a message was written in, for the error
/// that names it.
#[derive(Clone, Copy)]
enum ClauseKind {
    Superclass,
    MixinApplication,
}

/// The selector of `class`'s primary factory message: the selector its mixin
/// declares a factory pattern for, or `new` for a builtin, which declares
/// none. `None` for a non-class object.
///
/// Read only by `clause_message_body`, which decides whether a superclass
/// clause names it (§6.1). The other side of that question -- whether an
/// incoming send *is* the factory message -- is no longer asked: §6.3 makes
/// the factory an entry of the metaclass mixin, so the ordinary lookup
/// answers it (`mixin::factory_entry`).
pub(crate) fn primary_factory_selector<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
) -> Option<Symbol> {
    heap.class_application(class)
        .map(|application| match &application.mixin.factory {
            Some(factory) => factory.selector,
            None => Symbol::NEW,
        })
}

fn class_name<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>) -> Symbol {
    heap.class_application(class)
        .map_or(Symbol::AN_OBJECT, |application| application.mixin.name)
}

/// `newspeak-spec.pdf` §6.1: "It is a run time error if the message
/// selector specified by the inheritance clause is not the selector of the
/// primary factory of the class *C*."
fn not_the_primary_factory_error<'gc>(
    heap: &Heap<'gc>,
    clause: ClauseKind,
    class: ClassReference<'gc>,
    superclass: ClassReference<'gc>,
    selector: Symbol,
    expected: Option<Symbol>,
) -> Signal<'gc> {
    let subclass_name = class_name(heap, class);
    let clause = match clause {
        ClauseKind::Superclass => "the superclass clause",
        ClauseKind::MixinApplication => "a mixin application clause",
    };
    let superclass_name = class_name(heap, superclass);
    let expected = match expected {
        Some(expected) => format!("#{expected}"),
        // `superclass` is not a class object at all -- an internally
        // inconsistent chain rather than a program error, reported here
        // rather than swallowed.
        None => "no factory (it is not a class)".to_string(),
    };
    Signal::Error(NewspeakError {
        selector,
        receiver: Value::Object(superclass),
        message: format!(
            "{clause} of class {subclass_name} sends #{selector}, \
             but the primary factory of class {superclass_name} is {expected}"
        ),
    })
}

/// The evaluated clause message's arguments, unpacked from the array
/// `superclass_clause_message_arguments_expression` builds.
///
/// A `NewspeakError` rather than a `panic!` on the wrong shape: the compiled
/// body that produced this value can come off the wire, where
/// `image::decode::verify_mixins` reconciles its kind and arity but nothing
/// constrains what it returns. An image is untrusted input.
///
/// Inlined into the walk's resumption, where it runs once per clause message
/// with arguments; the refusal is out of line.
#[inline(always)]
fn argument_list<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    value: Value<'gc>,
) -> Result<Vec<Value<'gc>>, Signal<'gc>> {
    if let Value::Object(object) = &value {
        if let Some(items) = heap.array_elements(*object) {
            return Ok(items);
        }
    }
    Err(not_an_argument_list_error(heap, class, value))
}

#[cold]
#[inline(never)]
fn not_an_argument_list_error<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    value: Value<'gc>,
) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: Symbol::NEW,
        receiver: value,
        message: format!(
            "the superclass clause of class {} did not evaluate its message \
             arguments to an argument list",
            class_name(heap, class)
        ),
    })
}

/// `pub(crate)` (rather than private) so `compiler::lower` can reuse this
/// exact string->`BuiltinClass` mapping for its bare-builtin-name fallback,
/// instead of duplicating it. Returns `None` for any name that is not a
/// builtin class name. `Object` has an explicit arm rather than being a
/// catch-all default, because a caller must be able to tell "unrecognized
/// name" apart from a genuine `Object` match; the match arms are the one
/// list of builtin names, so no caller keeps a second one to guard with.
///
/// `Top` is deliberately absent: §3.2 lets only `Object` have it as its
/// superclass, so no program may name it -- and no artifact may either,
/// which is why `image::decode::read_program_builtin_class` refuses its tag
/// as it refuses the mirror's. `Platform` is absent for the reason
/// `kernel::kernel_class_name` gives.
pub(crate) fn builtin_kind_from_name(name: &str) -> Option<BuiltinClass> {
    Some(match name {
        "Object" => BuiltinClass::Object,
        "Value" => BuiltinClass::Value,
        "Boolean" => BuiltinClass::Boolean,
        "True" => BuiltinClass::True,
        "False" => BuiltinClass::False,
        "Number" => BuiltinClass::Number,
        "Integer" => BuiltinClass::Integer,
        "Float" => BuiltinClass::Float,
        "String" => BuiltinClass::String,
        "Symbol" => BuiltinClass::Symbol,
        "Character" => BuiltinClass::Character,
        "Array" => BuiltinClass::Array,
        "OrderedCollection" => BuiltinClass::OrderedCollection,
        "Nil" => BuiltinClass::Nil,
        "Block" => BuiltinClass::Block,
        _ => return None,
    })
}

/// §6.1 evaluates a module root's clauses "as if [each] was a top level
/// expression", where §3.8 leaves nothing to send to, so the only classes a
/// root can reach are the ones the rule names: `Object` or `Value` as its
/// superclass, and `Value` applied to `Object`. Decided with the user on
/// 2026-09-23 (`docs/design/object-model.md`, "The root rule"). Syntactic,
/// so it is decided here, once, and deferred to instantiation -- a module
/// whose root is wrong still loads and can be inspected and fixed.
pub(crate) fn root_clause_defect(
    declaration: &ast::ClassDeclaration,
) -> Option<crate::mixin::RootClauseDefect> {
    use crate::mixin::RootClauseDefect;
    let superclass_is_value = match &declaration.superclass {
        None => false,
        Some(clause) => {
            let named = clause.receiver.name.as_str();
            if clause.prefix.is_some() || !matches!(named, "Object" | "Value") {
                return Some(RootClauseDefect::Superclass {
                    clause: clause_as_written(clause),
                });
            }
            named == "Value"
        }
    };
    for (index, clause) in declaration.mixin_applications.iter().enumerate() {
        if index > 0 || clause.prefix.is_some() || clause.receiver.name != "Value" {
            return Some(RootClauseDefect::MixinApplication {
                index,
                clause: clause_as_written(clause),
            });
        }
        if superclass_is_value {
            return Some(RootClauseDefect::ValueOverValue);
        }
    }
    None
}

/// A clause's prefix and receiver, as the program wrote them -- what a
/// refusal names, since the message part is not at fault.
fn clause_as_written(clause: &ast::SuperclassClause) -> String {
    let prefix = match &clause.prefix {
        None => String::new(),
        Some(ast::SuperclassPrefix::SelfPrefix) => "self ".to_string(),
        Some(ast::SuperclassPrefix::SuperPrefix) => "super ".to_string(),
        Some(ast::SuperclassPrefix::Outer(name)) => format!("outer {} ", name.name),
    };
    format!("{prefix}{}", clause.receiver.name)
}

/// The refusal `decide_instantiation` raises, named under the class's own
/// primary factory as both value refusals are.
#[cold]
fn instantiation_refusal_error<'gc>(
    class: ClassReference<'gc>,
    application: &crate::mixin::MixinApplication,
    refusal: &crate::mixin::InstantiationRefusal,
) -> NewspeakError<'gc> {
    match refusal {
        crate::mixin::InstantiationRefusal::MutableValueSlot(violation) => {
            mutable_value_slot_error(class, application, *violation)
        }
        crate::mixin::InstantiationRefusal::RootClause(defect) => NewspeakError {
            selector: factory_selector(application),
            receiver: Value::Object(class),
            message: format!(
                "cannot instantiate {}: {}",
                application.mixin.name,
                defect.describe()
            ),
        },
        crate::mixin::InstantiationRefusal::IdentityMethod { declaring } => {
            identity_method_error(class, application, *declaring)
        }
    }
}

/// §3.1.1's first condition on a value class, refused where the instance
/// would come into being: "an object is a value object if it is deeply
/// immutable", and a mutable slot is state anything holding the object can
/// write afterwards. The class being instantiated and the class declaring
/// the slot are named apart, because a subclass inherits the fault.
///
/// The selector is the class's own primary factory, which is the selector
/// the send that got here carried -- a class declaring `x:y:` understands
/// no `new` -- defaulted the way `mixin::factory_entry` defaults it. That
/// is the rule `non_value_slot_error` follows too; its doc comment states
/// it for both.
fn mutable_value_slot_error<'gc>(
    class: ClassReference<'gc>,
    application: &crate::mixin::MixinApplication,
    violation: crate::mixin::MutableValueSlot,
) -> NewspeakError<'gc> {
    let instantiated = application.mixin.name;
    let declares = if instantiated == violation.class {
        "it declares".to_string()
    } else {
        format!("{} declares", violation.class)
    };
    NewspeakError {
        selector: factory_selector(application),
        receiver: Value::Object(class),
        message: format!(
            "cannot instantiate {instantiated}: a value object is deeply immutable, and \
             {declares} the mutable slot {}",
            violation.slot
        ),
    }
}

/// §3.1.1's third condition on a value class -- "does not define an
/// identity method (==)" -- decided once per *application*
/// (`Mutator::apply_mixin`, folded into `MixinApplication::instantiation_refusal`)
/// rather than per class object, and refused from there by
/// `instantiation_refusal_error`, alongside the root clause defect and the
/// mutable-slot refusal. The class being instantiated and the class
/// declaring `==` are named apart, because a subclass inherits the fault --
/// the same rule `mutable_value_slot_error` follows for its own violation.
#[cold]
#[inline(never)]
fn identity_method_error<'gc>(
    class: ClassReference<'gc>,
    application: &crate::mixin::MixinApplication,
    declaring: Symbol,
) -> NewspeakError<'gc> {
    NewspeakError {
        selector: factory_selector(application),
        receiver: Value::Object(class),
        message: format!(
            "cannot instantiate {}: a value class may not define ==, and {declaring} defines it",
            application.mixin.name
        ),
    }
}

/// §3.1.1's other class-level condition -- every enclosing object a value --
/// decided when the class object was created
/// (`interpreter::value_class::check_value_class`), and refused here rather
/// than there (`docs/design/object-model.md`, "A value object's four
/// conditions"). Named under the class's own primary factory, like every
/// other instantiation refusal. Reads the refusal off `class` itself, so
/// `decide_instantiation`'s hot path tests only for its presence
/// (`Heap::class_has_value_refusal`) and never copies it.
#[cold]
#[inline(never)]
fn value_class_refusal_error<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    application: &crate::mixin::MixinApplication,
) -> NewspeakError<'gc> {
    let instantiated = application.mixin.name;
    let Some(refusal) = heap.class_value_refusal(class) else {
        unreachable!("internal: value_class_refusal_error called for a class with no refusal");
    };
    let message = match refusal {
        ValueClassRefusal::NonValueEnclosingObject {
            level: None,
            described,
        } => format!(
            "cannot instantiate {instantiated}: a value class's enclosing objects must be \
             values, and its enclosing object, {described}, is not one"
        ),
        ValueClassRefusal::NonValueEnclosingObject {
            level: Some(level),
            described,
        } => format!(
            "cannot instantiate {instantiated}: a value class's enclosing objects must be \
             values, and {level}'s enclosing object, {described}, is not one"
        ),
    };
    NewspeakError {
        selector: factory_selector(application),
        receiver: Value::Object(class),
        message,
    }
}

/// §3.1.1's second condition, asked of every slot an initializer writes on
/// an instance of a value class: a value object is *deeply* immutable, "all
/// objects reachable from it are also deeply immutable", so a slot of one
/// may hold only another value. Anything else -- an `Array`, an ordinary
/// object, a class -- would make the whole value mutable through it, and
/// two copies of it would stop being equal the moment one was written.
///
/// Decided here rather than in `vm.rs`, which only calls it: an error's
/// text and the rule behind it live in `interpreter/`, so the reference
/// lowering, running the same instruction, raises the same error.
pub(crate) fn check_value_slot<'gc>(
    mutator: &Mutator<'gc>,
    activation: crate::heap::ActivationReference,
    receiver: Value<'gc>,
    own_index: usize,
    value: Value<'gc>,
) -> Result<(), Signal<'gc>> {
    // Every immediate is a value, whatever holds it, and this answers
    // without reading the heap at all: an initializer that stores a number
    // or `nil` -- most of them -- never reaches a class.
    let Value::Object(stored) = value else {
        return Ok(());
    };
    let Value::Object(instance) = receiver else {
        return Ok(());
    };
    let class = mutator.heap().class_of_object(instance);
    if !mutator.heap().class_inherits_value(class) {
        return Ok(());
    }
    if is_a_value(mutator, stored) {
        return Ok(());
    }
    Err(Signal::Error(non_value_slot_error(
        mutator, activation, class, receiver, own_index, value,
    )))
}

/// Whether `object` is a value object: §3.1.1's "its class inherits from
/// class `Value`", which covers a `String`, a `Symbol`, a `Character`, an
/// instance of a user class below `Value` and a `Value new` alike -- §3.2's
/// "subclass of" is inclusive, so `Value`'s own application carries the
/// flag (`Mutator::apply_mixin`). A class object is not one -- its own
/// class is `Object`.
///
/// **The other predicate about "is a value" is
/// `builtins::value::state_is_a_value`**, which asks the narrower question
/// of whether an instance's `=` is still `Value`'s own; the two must agree
/// about everything this one accepts and that one unfolds, and they
/// disagreed about `Value new` until 2026-09-23.
pub(crate) fn is_a_value<'gc>(mutator: &Mutator<'gc>, object: ObjectReference<'gc>) -> bool {
    !mutator.heap().is_class(object)
        && mutator
            .heap()
            .class_inherits_value(mutator.heap().class_of_object(object))
}

/// The refusal itself, off the checking path so that the check stays a
/// handful of loads. The slot is named from the level whose initializer is
/// running -- `own_index` is mixin-relative (`bytecode.rs`), so it indexes
/// that level's own slots and no other's.
///
/// **The rule both value refusals follow: the selector is the primary
/// factory of the class being *instantiated*, defaulted to `new`** -- the
/// selector the program actually sent, which is what a reader has to find
/// in the source to see the refusal. It is not the running level's factory:
/// each level of a chain runs its own initializer in its own activation
/// (§6.2), so an inherited level's store would otherwise be reported under
/// a selector the program never sent -- the superclass clause did.
/// `mutable_value_slot_error` names the instantiated class's factory for
/// the same reason, and it had no other candidate, which is how the two
/// came to disagree.
#[cold]
fn non_value_slot_error<'gc>(
    mutator: &Mutator<'gc>,
    activation: crate::heap::ActivationReference,
    instantiated: ClassReference<'gc>,
    receiver: Value<'gc>,
    own_index: usize,
    value: Value<'gc>,
) -> NewspeakError<'gc> {
    let level = mutator
        .heap()
        .activation_current_class(activation)
        .and_then(|class| mutator.heap().class_application(class));
    let class = level.as_ref().map_or_else(
        || "a value".to_string(),
        |level| level.mixin.name.to_string(),
    );
    let slot = level
        .as_ref()
        .and_then(|level| level.mixin.slots.get(own_index))
        .map_or_else(|| "a slot".to_string(), |slot| slot.name.to_string());
    NewspeakError {
        selector: mutator
            .heap()
            .class_application(instantiated)
            .as_ref()
            .map_or(Symbol::NEW, |application| factory_selector(application)),
        receiver,
        message: format!(
            "cannot store {} in {class}'s slot {slot}: a value object is deeply immutable, \
             so every slot it declares holds a value",
            mutator.heap().describe(&value)
        ),
    }
}

/// A class's primary factory selector, defaulted to `new` for a class that
/// declares none -- `mixin::factory_entry`'s rule, which is what puts the
/// entry in the metaclass's method table, asked again here so that a
/// refusal names the selector the program actually sent.
fn factory_selector(application: &crate::mixin::MixinApplication) -> Symbol {
    application
        .mixin
        .factory
        .map_or(Symbol::NEW, |factory| factory.selector)
}

/// A `MessagePattern` rendered as the selector string it declares. Also the
/// image encoder's `FrameKind::Method` representation: a frame names the
/// method it activates by selector, re-resolved on load against the method
/// map of the mixin the decoded mixin table rebuilt for the frame's own
/// `current_class` (`image::decode::method_declaration`).
pub(crate) fn method_name(pattern: &ast::MessagePattern) -> String {
    match pattern {
        ast::MessagePattern::Unary { selector } => selector.name.clone(),
        ast::MessagePattern::Binary { selector, .. } => selector.name.clone(),
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(keyword, _)| format!("{}:", keyword.name))
            .collect(),
    }
}

/// The number of parameters a message pattern declares -- used by
/// `instantiate`'s factory-arity guard above to compare a class's declared
/// factory pattern against the actual `factory_arguments` supplied at the call
/// site, and by `compiler` for a method's or an initializer's `argument_count`
/// (narrowed there to the `u16` `CompiledCode` stores it in). One
/// implementation, not one per caller.
pub(crate) fn pattern_parameter_count(pattern: &ast::MessagePattern) -> usize {
    match pattern {
        ast::MessagePattern::Unary { .. } => 0,
        ast::MessagePattern::Binary { .. } => 1,
        ast::MessagePattern::Keyword { parts } => parts.len(),
    }
}

/// Whether `class` declares any slot or `initExpr` at all -- `false` for
/// every `BuiltinClass` and for a non-class object. Delegates to
/// `mixin_needs_initializer_code`, the one definition of "needs initializer
/// code" a class-level and a mixin-level caller both share.
pub(crate) fn class_has_initializer_code<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
) -> bool {
    heap.class_application(class)
        .is_some_and(|application| mixin_needs_initializer_code(&application.mixin))
}

/// The kind of the activation a `Step::ClauseMessage` for `level` pushes,
/// whose walk continues with `then`.
///
/// A `<:` clause's message is evaluated during the descent, with the walk in
/// `InstantiationPhase::Applying`, whose `index` is the level that clause
/// added: its kind names that level, whose applied clause says which of the
/// applying class's message bodies runs (`image::decode::frame_body` finds a
/// frame's body by it). Every other clause message is the applying class's
/// own superclass clause's. Read off the walk rather than carried on the
/// step, which every send's decision is sized by.
pub(crate) fn clause_message_frame_kind<'gc>(
    level: ClassReference<'gc>,
    then: &Instantiation<'gc>,
) -> FrameKind<'gc> {
    match then.phase {
        InstantiationPhase::Applying { index, .. } => {
            FrameKind::MixinApplicationMessage(then.levels[index].0)
        }
        _ => FrameKind::SuperclassMessage(level),
    }
}

/// The nearest level to the top of the chain below `below` that has
/// initializer code: the next one the downward walk runs.
fn initializing_level_below<'gc>(
    heap: &Heap<'gc>,
    levels: &[(ClassReference<'gc>, Vec<Value<'gc>>)],
    below: usize,
) -> Option<usize> {
    levels[..below]
        .iter()
        .rposition(|(level, _)| class_has_initializer_code(heap, *level))
}

/// Whether `mixin` declares any slot or `initExpr` at all.
///
/// `pub(crate)` for `image::decode::verify_mixins`, which must refuse
/// an image that has no compiled initializer for a mixin that needs one
/// rather than let `Mutator::initializer_body` above panic on the invariant --
/// checked per mixin, since every mixin of the tree carries its own.
pub(crate) fn mixin_needs_initializer_code(mixin: &crate::mixin::Mixin) -> bool {
    !mixin.slots.is_empty() || mixin.has_initializer_expressions
}

#[cfg(test)]
mod tests {
    #[test]
    fn root_clause_defect_classifies_every_root_form() {
        use crate::mixin::RootClauseDefect;
        let defect_of = |header: &str| {
            let source = format!("Newspeak3\n'T'\n{header} ()\n");
            let module = ns_syntax::parse(&source).expect("parse");
            super::root_clause_defect(&module.root_class)
        };
        assert_eq!(defect_of("class R = ()"), None);
        assert_eq!(defect_of("class R = Object new ()"), None);
        assert_eq!(defect_of("class R = Value new ()"), None);
        assert_eq!(defect_of("class R = Object <: Value ()"), None);
        let superclass = |clause: &str| {
            Some(RootClauseDefect::Superclass {
                clause: clause.to_string(),
            })
        };
        let mixin = |index: usize, clause: &str| {
            Some(RootClauseDefect::MixinApplication {
                index,
                clause: clause.to_string(),
            })
        };
        assert_eq!(
            defect_of("class R = self Object new ()"),
            superclass("self Object")
        );
        assert_eq!(
            defect_of("class R = super Object new ()"),
            superclass("super Object")
        );
        assert_eq!(
            defect_of("class R = outer Missing Object new ()"),
            superclass("outer Missing Object")
        );
        assert_eq!(defect_of("class R = Array new ()"), superclass("Array"));
        assert_eq!(defect_of("class R = Top new ()"), superclass("Top"));
        assert_eq!(defect_of("class R = Missing new ()"), superclass("Missing"));
        assert_eq!(
            defect_of("class R = Object <: self Value ()"),
            mixin(0, "self Value")
        );
        assert_eq!(defect_of("class R = Object <: Array ()"), mixin(0, "Array"));
        assert_eq!(
            defect_of("class R = Object <: Value <: Value ()"),
            mixin(1, "Value")
        );
        assert_eq!(
            defect_of("class R = Value <: Value ()"),
            Some(RootClauseDefect::ValueOverValue)
        );
    }
}

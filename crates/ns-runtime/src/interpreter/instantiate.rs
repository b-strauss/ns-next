//! Class instantiation: `newspeak-spec.pdf` §6.1, §6.3. See the runtime
//! spec's Class Instantiation section for the six numbered steps this
//! transcribes.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::heap::{Heap, Root};
use crate::mixin::{Factory, Mixin, MixinMethod, MixinSlot, SuperclassClause};
use crate::types::*;
use crate::Symbol;

use super::{EvalResult, HostError, HostResult, HostSignal, Interpreter, Signal, Step};

/// How many finished walks' boxes `Interpreter::retire_walk` keeps for
/// reuse, and how many levels of room each keeps. One box serves any number
/// of instantiations in sequence; more are held at once only while
/// instantiations nest bodies that have work after them.
const SPARE_WALKS: usize = 16;
const SPARE_LEVELS: usize = 64;

/// A class body's methods as a mixin's method dictionary: everything a
/// lookup or `Interpreter::push_step` reads is lifted out of the
/// declaration here, so that a mixin decoded from an image -- which has no
/// declaration at all -- answers the same questions from the same fields.
fn build_methods(
    body: &ast::ClassBody,
    bindings: &Rc<HashMap<Span, Binding>>,
    lowering: crate::compiler::Lowering,
) -> HashMap<Symbol, Rc<MixinMethod>> {
    body.methods
        .iter()
        .map(|m| {
            let method = MixinMethod::from_decl(Rc::new(m.clone()), bindings, lowering);
            (method.selector, Rc::new(method))
        })
        .collect()
}

fn build_slots(body: &ast::ClassBody) -> Vec<MixinSlot> {
    body.slots
        .iter()
        .map(|s| MixinSlot::from_decl(Rc::new(s.clone())))
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
        has_args: super::nested::superclass_clause_message_args_expr(clause).is_some(),
    }
}

impl Interpreter {
    /// Materializes the module's root `ClassDecl` into a real class object.
    /// The superclass clause's *receiver* is resolved here, once -- it is
    /// what fixes the superclass (`newspeak-spec.pdf` §6.1). Its trailing
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
    /// interpreter was bootstrapped with.
    ///
    /// Transitional: the unrooted form the tests used before hosts held
    /// roots; removed once every test uses roots.
    pub fn load_module_unrooted(&mut self, resolved: &ResolvedModule) -> ClassRef {
        let decl = &resolved.module.root_class;
        let mixin = self.build_mixin(decl, &resolved.bindings);
        let superclass = self.resolve_superclass_receiver(decl);
        // The slot layout is the application's job, not the class object's:
        // `apply_mixin` offsets this class's own slots past every inherited
        // one and merges the superclass's map in, so a class object never
        // computes -- or can disagree about -- an instance's shape.
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

    /// Materializes the module's root class (`load_module_unrooted`), rooted
    /// for the host that will instantiate it.
    pub fn load_module(&mut self, resolved: &ResolvedModule) -> Root {
        let class = self.load_module_unrooted(resolved);
        self.stash(Value::Obj(class))
    }

    /// Builds the mixin for `decl`, recursively including its nested
    /// declarations and its class side. Pure structure -- no Newspeak code is
    /// evaluated here, which is what keeps module loading total.
    ///
    /// Every `decl.instance_side.nested_classes` entry becomes a `Mixin` in
    /// `nested`, in declaration order, and `decl.class_side` becomes a
    /// `Mixin` too, instead of being discarded. Neither is *applied* here (no
    /// superclass is resolved, no slot layout computed, no class object
    /// built) -- a nested mixin has no class object of its own until
    /// something materializes it (the accessor), and a class side until
    /// `Interpreter::metaclass_of` builds the metaclass that applies it.
    fn build_mixin(
        &mut self,
        decl: &ast::ClassDecl,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Rc<Mixin> {
        let methods = build_methods(&decl.instance_side, bindings, self.lowering);
        let slots = build_slots(&decl.instance_side);
        let nested = decl
            .instance_side
            .nested_classes
            .iter()
            .map(|n| self.build_mixin(n, bindings))
            .collect();
        let class_side = decl
            .class_side
            .as_ref()
            .map(|side| self.build_class_side(side, bindings));
        let mut mixin = Mixin {
            name: Symbol::intern(&decl.name.name),
            access: crate::dispatch::effective_access(decl.access),
            methods,
            slots,
            // §6.1: a class written without a factory pattern is
            // instantiated by `new`, which is also what an omitted
            // superclass-clause *message* defaults to -- the two halves of
            // the same rule, so `class C = (...)` inherits from `Object`
            // through `new`.
            factory: Some(match &decl.factory {
                Some(pattern) => Factory {
                    selector: Symbol::intern(&method_name(pattern)),
                    num_args: pattern_param_count(pattern),
                },
                None => Factory {
                    selector: Symbol::NEW,
                    num_args: 0,
                },
            }),
            superclass: decl.superclass.as_ref().map(superclass_clause_of),
            has_init_exprs: !decl.instance_side.init_exprs.is_empty(),
            // This class's own initExprs, lowered into the instance
            // initializer by `compiler::compile_mixin_initializer` (step 5 of
            // the six in `docs/design/object-model.md`, "Instantiation").
            init_exprs: decl.instance_side.init_exprs.clone(),
            nested,
            class_side,
            span: decl.span,
            decl: Some(Rc::new(decl.clone())),
            initializer_code: None,
            superclass_clause_code: None,
            superclass_message_code: None,
        };
        crate::compiler::compile_mixin_bodies(&mut mixin, bindings, self.lowering);
        Rc::new(mixin)
    }

    /// A class side's own methods, as a `Mixin`. Kept separate from
    /// `build_mixin` and deliberately small: a class side has no factory
    /// pattern of its own (its metaclass carries class-side behaviour and
    /// nothing more -- `docs/design/object-model.md`, "Class sides and
    /// metaclasses") and no nested classes of its own to load -- only its
    /// instance side's `nested_classes` recurse. `decl` is `None` because a
    /// `ClassBody` is not itself an `ast::ClassDecl`; there is no single
    /// declaration node to keep as provenance for it.
    fn build_class_side(
        &mut self,
        side: &ast::ClassBody,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Rc<Mixin> {
        let mut mixin = Mixin {
            methods: build_methods(side, bindings, self.lowering),
            slots: build_slots(side),
            has_init_exprs: !side.init_exprs.is_empty(),
            init_exprs: side.init_exprs.clone(),
            span: side.span,
            ..Mixin::empty(Symbol::CLASS_SIDE)
        };
        crate::compiler::compile_mixin_bodies(&mut mixin, bindings, self.lowering);
        Rc::new(mixin)
    }

    /// Intercepts an ordinary explicit-receiver send (`Interpreter::send`'s
    /// call site, above -- checked *before* `dispatch::lookup_public`) when
    /// `receiver` is a class object and `selector` is that class's own
    /// factory selector: the class's mixin `factory` pattern if the class
    /// declared one (every user-defined class does -- `build_mixin` above
    /// always fills this in, defaulting to a bare `new` pattern when the
    /// source class decl left it out), or a bare `new` selector for a
    /// builtin (`factory` is always `None` there, per
    /// `build_builtin_classes`). Returns `None` for anything else --
    /// including a `new`-selector send to a *non*-class object, or a
    /// class object receiving some other selector -- letting the caller
    /// fall through to ordinary dispatch.
    ///
    /// No user-overridable `new` method is ever registered anywhere to
    /// find (every `BuiltinClass`-tagged placeholder's method table is
    /// empty, and a user class's own `.class` is that same shared
    /// placeholder -- there is no reified metaclass hierarchy, see
    /// `docs/design/object-model.md`, "Class sides and metaclasses"), so
    /// without this check `ClassName new` would unconditionally end in
    /// `doesNotUnderstand:`, for every class, every time.
    ///
    /// No double-dispatch risk against the superclass clause's own factory
    /// message (e.g. `Object new` in `class Foo = Object new (...)`). That
    /// message is *evaluated* (`climb_levels` below), but it is
    /// never *sent*: §6.2 has the subclass's initializer invoke the
    /// superclass's instance initializer directly on the same object, so
    /// `climb_levels` runs it level by level and never calls
    /// `Interpreter::decide_send` -- and therefore never reaches this method,
    /// which would otherwise allocate a second, discarded instance per level.
    pub(super) fn try_factory_send(
        &mut self,
        receiver: &Value,
        selector: Symbol,
        args: &[Value],
    ) -> Option<Result<Step, Signal>> {
        let Value::Obj(obj) = receiver else {
            return None;
        };
        let factory_selector = primary_factory_selector(&self.heap, *obj)?;
        if selector != factory_selector {
            return None;
        }
        let builtin = self.heap.class_builtin(*obj);
        let class_ref = *obj;
        Some(match builtin {
            // `Array`/`OrderedCollection`'s representation is
            // `ObjectData::Array`, not the generic `instantiate()`'s
            // `ObjectData::Instance` -- `builtins::collections`' dispatch
            // functions pattern-match on `ObjectData::Array`
            // directly, so handing them a generic instance payload would
            // leave every native method on the result silently unable to
            // find its data (falling straight through to
            // `doesNotUnderstand:` on the very next send). Built directly
            // with `new_instance_object`, tagged with `class_ref` itself
            // (the receiver's own class -- *not* `Interpreter::new_array`,
            // which unconditionally tags its result `BuiltinClass::Array`;
            // using that here would make `OrderedCollection new`'s result
            // dispatch through `dispatch_array`, which has no `add:`).
            Some(BuiltinClass::OrderedCollection) | Some(BuiltinClass::Array) => Ok(Step::Value(
                Value::Obj(self.new_instance_object(class_ref, ObjectData::Array(vec![]))),
            )),
            _ => self.decide_instantiation(class_ref, args.to_vec()),
        })
    }

    fn resolve_superclass_receiver(&mut self, decl: &ast::ClassDecl) -> Option<ClassRef> {
        let Some(clause) = &decl.superclass else {
            return Some(self.builtin_class(BuiltinClass::Object));
        };
        // Only the common case (a builtin class name) is
        // handled -- full lexical/outer-prefixed superclass receivers, and
        // a not-yet-loaded sibling user-defined class referenced by name,
        // need the resolver's class-nesting chain (and/or a multi-class
        // load order) at *instantiation* time; both are a known gap.
        // Extend here if a fixture needs it (see `tests/instantiate.rs`'s
        // subclass test for how a fixture can route around this gap
        // without needing the extension).
        if let Some(kind) = builtin_kind_from_name(&clause.receiver.name) {
            return Some(self.builtin_class(kind));
        }
        None
    }

    /// Instantiates the class `class` holds, from a host: the six numbered
    /// steps in `docs/design/object-model.md`, "Instantiation", decided
    /// (`decide_instantiation`) and then run from the outside
    /// (`run_from_host`), like a host's `send`. A root that holds anything
    /// but a class object is refused as a Newspeak error, never a panic:
    /// the caller is a host.
    pub fn instantiate(&mut self, class: &Root, factory_args: &[Root]) -> HostResult {
        let value = self.fetch(class);
        let class = match value {
            Value::Obj(obj) if self.heap.is_class(obj) => obj,
            _ => {
                return Err(HostSignal::Error(HostError {
                    selector: Symbol::NEW,
                    receiver: self.stash(value),
                    message: format!(
                        "cannot instantiate {}: it is not a class",
                        self.heap.describe(&value)
                    ),
                }))
            }
        };
        let factory_args: Vec<Value> = factory_args.iter().map(|arg| self.fetch(arg)).collect();
        let result = match self.decide_instantiation(class, factory_args) {
            Ok(step) => self.run_from_host(step, &[]),
            Err(signal) => Err(signal),
        };
        self.to_host(result)
    }

    /// Transitional: the unrooted form the tests used before hosts held
    /// roots; removed once every test uses roots.
    pub fn instantiate_unrooted(
        &mut self,
        class: ClassRef,
        factory_args: Vec<Value>,
    ) -> EvalResult {
        match self.decide_instantiation(class, factory_args) {
            Ok(step) => self.run_from_host(step, &[]),
            Err(signal) => Err(signal),
        }
    }

    /// Decides an instantiation of `class` with `factory_args` without running
    /// any of its code: the instance comes back as a value when no level of
    /// the superclass chain has code to run, and otherwise the first body that
    /// must run, with the rest of the walk in its continuation
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
        class: ClassRef,
        factory_args: Vec<Value>,
    ) -> Result<Step, Signal> {
        // Step 1: one instance, every slot nil. The application already
        // knows the whole layout, inherited slots included -- summing the
        // chain here again would be a second answer to the same question.
        let slot_count = self
            .heap
            .class_application(class)
            .map_or(0, |a| a.num_slots);
        let instance =
            self.new_instance_object(class, ObjectData::Instance(vec![Value::Nil; slot_count]));
        let state = self.walk_state(Value::Obj(instance));
        self.climb_levels(state, class, factory_args)
    }

    /// The box a new walk of `instance`'s instantiation runs in: a spare one
    /// if a finished walk left one, so that neither the box nor its list of
    /// levels is allocated again.
    fn walk_state(&mut self, instance: Value) -> Box<Instantiation> {
        match self.spare_walks.pop() {
            Some(mut state) => {
                state.instance = instance;
                state
            }
            None => Box::new(Instantiation {
                instance,
                levels: Vec::new(),
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
    fn retire_walk(&mut self, mut state: Box<Instantiation>) {
        if self.spare_walks.len() < SPARE_WALKS {
            state.levels.clear();
            state.levels.shrink_to(SPARE_LEVELS);
            state.instance = Value::Nil;
            // A walk that climbed through a clause message with arguments and
            // then found nothing to initialize below it retires straight out
            // of `descend_levels`'s "nothing below" branches, which never
            // write `Descending` into `phase` -- so without this, the box
            // would still hold the last `Climbing`'s two `ClassRef`s.
            // `Descending { index: 0 }` is otherwise never read (`walk_state`'s
            // comment), so any handle-free variant would do.
            state.phase = InstantiationPhase::Descending { index: 0 };
            self.spare_walks.push(state);
        }
    }

    /// One body an instantiation ran has returned `value`: carry the walk on
    /// from where `state` says it stopped. `Interpreter::resume` calls this
    /// with its continuation's box, in which the walk goes on until it answers
    /// another body.
    pub(crate) fn advance_instantiation(
        &mut self,
        state: Box<Instantiation>,
        value: Value,
    ) -> Result<Step, Signal> {
        match state.phase {
            InstantiationPhase::Climbing { level, superclass } => {
                let super_args = argument_list(&self.heap, level, value)?;
                self.climb_levels(state, superclass, super_args)
            }
            // An initializer's value is not used; only its effects on the
            // instance are.
            InstantiationPhase::Descending { index } => self.descend_levels(state, index),
        }
    }

    /// The upward walk, from `level` with `args`: each level's factory arity
    /// checked, then its superclass clause's message evaluated to the
    /// superclass's arguments (step 3's first half). A message with arguments
    /// is code, so the walk stops there and answers its activation, and
    /// `advance_instantiation` resumes it with the value. Walked with a loop
    /// and a list rather than by recursing per level: the chain's length is an
    /// image's to choose, and a level with no code pushes no activation, so the
    /// activation limit would not see a long chain of them
    /// (`docs/design/execution.md`, "A superclass chain is walked
    /// iteratively"). At the top of the chain, the downward walk begins.
    fn climb_levels(
        &mut self,
        mut state: Box<Instantiation>,
        mut level: ClassRef,
        mut args: Vec<Value>,
    ) -> Result<Step, Signal> {
        loop {
            self.check_factory_arity(level, &args)?;
            let Some(superclass) = self.heap.class_superclass(level) else {
                state.levels.push((level, args));
                let above_top = state.levels.len();
                return self.descend_levels(state, above_top);
            };
            let message = self.clause_message_body(level, superclass)?;
            let Some(code) = message else {
                // `Object new` and every other argument-free clause: nothing
                // to run, and the superclass gets no arguments.
                state.levels.push((level, args));
                level = superclass;
                args = Vec::new();
                continue;
            };
            // The message is evaluated on the instance being initialized and
            // with *this* level's factory arguments -- §6.2's "in the context
            // of the instance initializer".
            let message_args = args.clone();
            state.levels.push((level, args));
            state.phase = InstantiationPhase::Climbing { level, superclass };
            return Ok(Step::ClauseMessage {
                level,
                code,
                args: message_args,
                // Its value is the superclass's arguments: the walk has
                // everything above this level still to do.
                then: state,
            });
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
    fn descend_levels(
        &mut self,
        mut state: Box<Instantiation>,
        next: usize,
    ) -> Result<Step, Signal> {
        let instance = state.instance;
        let Some(index) = initializing_level_below(&self.heap, &state.levels, next) else {
            self.retire_walk(state);
            return Ok(Step::Value(instance));
        };
        let level = state.levels[index].0;
        let args = std::mem::take(&mut state.levels[index].1);
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
                args,
            });
        }
        state.phase = InstantiationPhase::Descending { index };
        Ok(Step::InitializerThen {
            level,
            args,
            then: state,
        })
    }

    /// The compiled instance initializer of `level`, a level
    /// `class_has_initializer_code` has confirmed, and the `slot_base` its
    /// activation runs with.
    pub(super) fn initializer_body(
        &self,
        level: ClassRef,
    ) -> (Rc<crate::bytecode::CompiledCode>, usize) {
        let application = self
            .heap
            .class_application(level)
            .expect("internal: class_has_initializer_code confirmed this is a class object");
        let code = application.mixin.initializer_code.clone().unwrap_or_else(|| {
            panic!("internal: class needs an instance initializer and its mixin carries none (a mixin built from source always does; verify_mixins refuses a decoded one)")
        });
        // `InitSlot`'s `own_index` is relative to the mixin's own slots, not
        // an absolute field index, because the compiled body is shared by
        // every application of the mixin. This level's own
        // `MixinApplication.own_slot_base`, taken as the activation's
        // `slot_base`, turns it into an absolute index at run time; it is set
        // with the rest of the level's identity, and never changes.
        (code, application.own_slot_base)
    }

    /// Rejects a factory-arity mismatch as a NewspeakError before this
    /// level's activation is pushed or any slot initializer / initExpr runs.
    fn check_factory_arity(&self, class: ClassRef, factory_args: &[Value]) -> Result<(), Signal> {
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
        let declared_arity = self
            .heap
            .class_application(class)
            .and_then(|application| application.mixin.factory.map(|factory| factory.num_args));
        if let Some(declared_arity) = declared_arity {
            if factory_args.len() != declared_arity {
                return Err(Signal::Error(NewspeakError {
                    selector: Symbol::NEW,
                    receiver: Value::Obj(class),
                    message: format!(
                        "wrong number of factory arguments: factory takes {}, got {}",
                        declared_arity,
                        factory_args.len()
                    ),
                }));
            }
        }
        Ok(())
    }

    /// Step 3's first half, up to running anything: `newspeak-spec.pdf` §6.2's
    /// "the superclass clause's message is evaluated to a concrete message".
    /// Answers the compiled body that evaluates the message's arguments, or
    /// `None` when there are none.
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
    fn clause_message_body(
        &self,
        class: ClassRef,
        superclass: ClassRef,
    ) -> Result<Option<Rc<crate::bytecode::CompiledCode>>, Signal> {
        // One lookup of the application for both questions below: the clause
        // this class was declared with, and the mixin its compiled message
        // body lives on.
        let application = self.heap.class_application(class);
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
                &self.heap, class, superclass, selector, expected,
            ));
        }
        if !clause.is_some_and(|clause| clause.has_args) {
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
}

/// The selector of `class`'s primary factory message: the selector its mixin
/// declares a factory pattern for, or `new` for a builtin, which declares
/// none. `None` for a non-class object.
///
/// One definition, shared by `try_factory_send` (which decides whether an
/// incoming send *is* the factory message) and `clause_message_body`
/// (which decides whether a superclass clause names it) -- `newspeak-spec.pdf`
/// §6.1 makes those the same question asked from two sides.
pub(crate) fn primary_factory_selector(heap: &Heap, class: ClassRef) -> Option<Symbol> {
    heap.class_application(class)
        .map(|application| match &application.mixin.factory {
            Some(factory) => factory.selector,
            None => Symbol::NEW,
        })
}

fn class_name(heap: &Heap, class: ClassRef) -> Symbol {
    heap.class_application(class)
        .map_or(Symbol::AN_OBJECT, |application| application.mixin.name)
}

/// `newspeak-spec.pdf` §6.1: "It is a run time error if the message
/// determined by the inheritance clause is not the primary factory message
/// of C."
fn not_the_primary_factory_error(
    heap: &Heap,
    class: ClassRef,
    superclass: ClassRef,
    selector: Symbol,
    expected: Option<Symbol>,
) -> Signal {
    let subclass_name = class_name(heap, class);
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
        receiver: Value::Obj(superclass),
        message: format!(
            "the superclass clause of class {subclass_name} sends #{selector}, \
             but the primary factory of class {superclass_name} is {expected}"
        ),
    })
}

/// The evaluated clause message's arguments, unpacked from the array
/// `superclass_clause_message_args_expr` builds.
///
/// A `NewspeakError` rather than a `panic!` on the wrong shape: the compiled
/// body that produced this value can come off the wire, where
/// `image::decode::verify_mixins` reconciles its kind and arity but nothing
/// constrains what it returns. An image is untrusted input.
fn argument_list(heap: &Heap, class: ClassRef, value: Value) -> Result<Vec<Value>, Signal> {
    if let Value::Obj(obj) = &value {
        if let Some(items) = heap.array_elements(*obj) {
            return Ok(items);
        }
    }
    Err(Signal::Error(NewspeakError {
        selector: Symbol::NEW,
        receiver: value,
        message: format!(
            "the superclass clause of class {} did not evaluate its message \
             arguments to an argument list",
            class_name(heap, class)
        ),
    }))
}

/// `pub(crate)` (rather than private) so `compiler::lower` can reuse this
/// exact string->`BuiltinClass` mapping for its bare-builtin-name fallback,
/// instead of duplicating it. Returns `None` for any name that is not a
/// builtin class name. `Object` has an explicit arm rather than being a
/// catch-all default, because a caller must be able to tell "unrecognized
/// name" apart from a genuine `Object` match; the match arms are the one
/// list of builtin names, so no caller keeps a second one to guard with.
pub(crate) fn builtin_kind_from_name(name: &str) -> Option<BuiltinClass> {
    Some(match name {
        "Object" => BuiltinClass::Object,
        "Boolean" => BuiltinClass::Boolean,
        "True" => BuiltinClass::True,
        "False" => BuiltinClass::False,
        "Integer" => BuiltinClass::Integer,
        "Float" => BuiltinClass::Float,
        "String" => BuiltinClass::Str,
        "Symbol" => BuiltinClass::Sym,
        "Character" => BuiltinClass::Character,
        "Array" => BuiltinClass::Array,
        "OrderedCollection" => BuiltinClass::OrderedCollection,
        "Nil" => BuiltinClass::Nil,
        "Block" => BuiltinClass::Block,
        _ => return None,
    })
}

/// A `MessagePattern` rendered as the selector string it declares. Also the
/// image encoder's `FrameKind::Method` representation: a frame names the
/// method it activates by selector, re-resolved on load against the method
/// map of the mixin the decoded mixin table rebuilt for the frame's own
/// `current_class` (`image::decode::method_decl`).
pub(crate) fn method_name(pattern: &ast::MessagePattern) -> String {
    match pattern {
        ast::MessagePattern::Unary { selector } => selector.name.clone(),
        ast::MessagePattern::Binary { selector, .. } => selector.name.clone(),
        ast::MessagePattern::Keyword { parts } => {
            parts.iter().map(|(k, _)| format!("{}:", k.name)).collect()
        }
    }
}

/// The number of parameters a message pattern declares -- used by
/// `instantiate`'s factory-arity guard above to compare a class's declared
/// factory pattern against the actual `factory_args` supplied at the call
/// site, and by `compiler` for a method's or an initializer's `num_args`
/// (narrowed there to the `u16` `CompiledCode` stores it in). One
/// implementation, not one per caller.
pub(crate) fn pattern_param_count(pattern: &ast::MessagePattern) -> usize {
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
pub(crate) fn class_has_initializer_code(heap: &Heap, class: ClassRef) -> bool {
    heap.class_application(class)
        .is_some_and(|application| mixin_needs_initializer_code(&application.mixin))
}

/// The nearest level to the top of the chain below `below` that has
/// initializer code: the next one the downward walk runs.
fn initializing_level_below(
    heap: &Heap,
    levels: &[(ClassRef, Vec<Value>)],
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
/// rather than let `Interpreter::initializer_body` above panic on the invariant --
/// checked per mixin, since every mixin of the tree carries its own.
pub(crate) fn mixin_needs_initializer_code(mixin: &crate::mixin::Mixin) -> bool {
    !mixin.slots.is_empty() || mixin.has_init_exprs
}

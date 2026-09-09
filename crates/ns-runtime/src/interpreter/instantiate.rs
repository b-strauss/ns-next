//! Class instantiation: `newspeak-spec.pdf` §6.1, §6.3. See the runtime
//! spec's Class Instantiation section for the six numbered steps this
//! transcribes.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::heap::Heap;
use crate::mixin::{Factory, Mixin, MixinMethod, MixinSlot, SuperclassClause};
use crate::types::*;
use crate::Symbol;

use super::{Engine, EvalResult, Interpreter, Signal};

/// A class body's methods as a mixin's method dictionary: everything a
/// lookup or an engine hook reads is lifted out of the declaration here, so
/// that a mixin decoded from an image -- which has no declaration at all --
/// answers the same questions from the same fields.
fn build_methods(body: &ast::ClassBody) -> HashMap<Symbol, Rc<MixinMethod>> {
    body.methods
        .iter()
        .map(|m| {
            let method = MixinMethod::from_decl(Rc::new(m.clone()));
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
/// once, here, rather than at each of the two places that used to re-derive
/// it from the declaration.
fn superclass_clause_of(clause: &ast::SuperclassClause) -> SuperclassClause {
    SuperclassClause {
        selector: super::nested::superclass_clause_message(clause).map_or_else(
            || Symbol::intern("new"),
            crate::compiler::lower::selector_of,
        ),
        has_args: super::nested::superclass_clause_message_args_expr(clause).is_some(),
    }
}

impl Interpreter {
    /// Materializes the module's root `ClassDecl` into a real class object.
    /// The superclass clause's *receiver* is resolved here, once -- it is
    /// what fixes the superclass (`newspeak-spec.pdf` §6.1). Its trailing
    /// *message* is deliberately not touched here: §6.2 defers it to every
    /// instantiation, where `run_initializer_level` evaluates it off the
    /// mixin's own declaration.
    ///
    /// Building the mixin tree (`build_mixin`, below) is a separate step from
    /// applying it: a nested mixin is reachable the moment its enclosing
    /// mixin exists, but *applying* a mixin -- resolving a superclass and
    /// computing a slot layout -- only happens for the root here. A nested
    /// mixin gets its own application when something actually materializes
    /// it (the accessor, Task 5); until then it is pure, unapplied structure.
    pub fn load_module(&mut self, resolved: &ResolvedModule) -> ClassRef {
        let decl = &resolved.module.root_class;
        let mixin = self.build_mixin(decl);
        let superclass = self.resolve_superclass_receiver(decl);
        // The slot layout is the application's job, not the class object's:
        // `apply_mixin` offsets this class's own slots past every inherited
        // one and merges the superclass's map in, so a class object never
        // computes -- or can disagree about -- an instance's shape.
        let super_application = superclass
            .as_ref()
            .and_then(|superclass| self.heap.class_application(superclass));
        let application = self.apply_mixin(&mixin, super_application.as_ref());
        // `enclosing`/`declaring_class` are both `None` -- the module's root
        // class is the outermost level, so no object encloses it and no
        // declaration contains it. Every other class object in the system
        // comes from `interpreter::nested`, which passes both.
        self.new_class_object(application, None, None, superclass)
    }

    /// Note on the brief's original signature: `decl` is taken here as
    /// `&ast::ClassDecl`, not `&Rc<ast::ClassDecl>` -- `ast::Module::root_class`
    /// is a plain (non-`Rc`) `ClassDecl` (see `crates/ns-syntax/src/ast.rs`),
    /// so `load_module`'s `&resolved.module.root_class` argument is
    /// `&ast::ClassDecl`; a `&Rc<ast::ClassDecl>` parameter here would not
    /// type-check against that call site. This is the kind of small,
    /// mechanical fix every task in this plan has needed at least once.
    ///
    /// Builds the mixin for `decl`, recursively including its nested
    /// declarations and its class side. Pure structure -- no Newspeak code is
    /// evaluated here, which is what keeps module loading total.
    ///
    /// Every `decl.instance_side.nested_classes` entry becomes a `Mixin` in
    /// `nested`, in declaration order, and `decl.class_side` becomes a
    /// `Mixin` too, instead of being discarded. Neither is *applied* here (no
    /// superclass is resolved, no slot layout computed, no class object
    /// built) -- a nested mixin has no class object of its own until
    /// something materializes it (the accessor, Task 5), and a class side
    /// has no accessor to dispatch to yet (Task 9). Reachable in the tree is
    /// as far as this task goes.
    fn build_mixin(&mut self, decl: &ast::ClassDecl) -> Rc<Mixin> {
        let methods = build_methods(&decl.instance_side);
        let slots = build_slots(&decl.instance_side);
        let nested = decl
            .instance_side
            .nested_classes
            .iter()
            .map(|n| self.build_mixin(n))
            .collect();
        let class_side = decl
            .class_side
            .as_ref()
            .map(|side| self.build_class_side(side));
        Rc::new(Mixin {
            name: Symbol::intern(&decl.name.name),
            access: crate::dispatch::effective_access(decl.access),
            methods,
            slots,
            // §6.1: a class written without a factory pattern is
            // instantiated by `new`, which is also what an omitted
            // superclass-clause *message* defaults to -- the two halves of
            // the same rule, so `class C = (...)` inherits from `Object`
            // through `new` under both engines.
            factory: Some(match &decl.factory {
                Some(pattern) => Factory {
                    selector: Symbol::intern(&method_name(pattern)),
                    num_args: pattern_param_count(pattern),
                },
                None => Factory {
                    selector: Symbol::intern("new"),
                    num_args: 0,
                },
            }),
            superclass: decl.superclass.as_ref().map(superclass_clause_of),
            has_init_exprs: !decl.instance_side.init_exprs.is_empty(),
            // Class instantiation "must fix" #1: this class's own initExprs,
            // evaluated for real by `run_own_init_exprs` below (Step 5 of
            // the spec's six numbered steps).
            init_exprs: decl.instance_side.init_exprs.clone(),
            nested,
            class_side,
            span: decl.span,
            decl: Some(Rc::new(decl.clone())),
        })
    }

    /// A class side's own methods, as a `Mixin`. Kept separate from
    /// `build_mixin` and deliberately small: a class side has no factory
    /// pattern of its own (there is no metaclass to instantiate it through in
    /// this milestone) and no nested classes of its own to load -- only its
    /// instance side's `nested_classes` recurse. `decl` is `None` because a
    /// `ClassBody` is not itself an `ast::ClassDecl`; there is no single
    /// declaration node to keep as provenance for it.
    fn build_class_side(&mut self, side: &ast::ClassBody) -> Rc<Mixin> {
        Rc::new(Mixin {
            methods: build_methods(side),
            slots: build_slots(side),
            has_init_exprs: !side.init_exprs.is_empty(),
            init_exprs: side.init_exprs.clone(),
            span: side.span,
            ..Mixin::empty(Symbol::intern("class side"))
        })
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
    /// placeholder -- Milestone 1's "no metaclasses" simplification), so
    /// without this check `ClassName new` would unconditionally end in
    /// `doesNotUnderstand:`, for every class, every time.
    ///
    /// No double-dispatch risk against the superclass clause's own factory
    /// message (e.g. `Object new` in `class Foo = Object new (...)`). That
    /// message is *evaluated* now (`run_initializer_level` below), but it is
    /// never *sent*: §6.2 has the subclass's initializer invoke the
    /// superclass's instance initializer directly on the same object, so
    /// `run_initializer_level` recurses into itself and never calls
    /// `Interpreter::send` -- and therefore never reaches this method, which
    /// would otherwise allocate a second, discarded instance per level.
    pub(super) fn try_factory_send(
        &mut self,
        receiver: &Value,
        selector: Symbol,
        args: &[Value],
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Option<EvalResult> {
        let Value::Obj(obj) = receiver else {
            return None;
        };
        let factory_selector = primary_factory_selector(&self.heap, obj)?;
        if selector != factory_selector {
            return None;
        }
        let builtin = self.heap.class_builtin(obj);
        let class_ref = *obj;
        Some(match builtin {
            // `Array`/`OrderedCollection`'s Milestone 1 representation is
            // `ObjectData::Array`, not the generic `instantiate()`'s
            // `ObjectData::Instance` -- `builtins::collections`' dispatch
            // functions (Task 10) pattern-match on `ObjectData::Array`
            // directly, so handing them a generic instance payload would
            // leave every native method on the result silently unable to
            // find its data (falling straight through to
            // `doesNotUnderstand:` on the very next send). Built directly
            // with `new_instance_object`, tagged with `class_ref` itself
            // (the receiver's own class -- *not* `Interpreter::new_array`,
            // which unconditionally tags its result `BuiltinClass::Array`;
            // using that here would make `OrderedCollection new`'s result
            // dispatch through `dispatch_array`, which has no `add:`).
            Some(BuiltinClass::OrderedCollection) | Some(BuiltinClass::Array) => Ok(Value::Obj(
                self.new_instance_object(class_ref, ObjectData::Array(vec![])),
            )),
            _ => self.instantiate(class_ref, args.to_vec(), bindings),
        })
    }

    fn resolve_superclass_receiver(&mut self, decl: &ast::ClassDecl) -> Option<ClassRef> {
        let Some(clause) = &decl.superclass else {
            return Some(self.builtin_class(BuiltinClass::Object));
        };
        // Milestone 1: only the common case (a builtin class name) is
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

    /// The six numbered steps in the runtime spec's Class Instantiation
    /// section. Steps 2-5 are `run_initializer_level`, once per level of the
    /// superclass chain.
    pub fn instantiate(
        &mut self,
        class: ClassRef,
        factory_args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> EvalResult {
        // Step 1: one instance, every slot nil. The application already
        // knows the whole layout, inherited slots included -- summing the
        // chain here again would be a second answer to the same question.
        let slot_count = self
            .heap
            .class_application(&class)
            .map_or(0, |a| a.num_slots);
        let instance =
            self.new_instance_object(class, ObjectData::Instance(vec![Value::Nil; slot_count]));
        let receiver = Value::Obj(instance);

        // Steps 2-5, for this class and then, recursively, for every level
        // above it.
        self.run_initializer_level(&class, &receiver, factory_args, bindings)?;

        // Step 6.
        Ok(receiver)
    }

    /// One inheritance level's instance initializer (`newspeak-spec.pdf`
    /// §6.2): its own activation with its own arguments (step 2), then --
    /// before anything of its own runs -- the superclass clause's deferred
    /// message evaluated to a concrete message and the superclass's
    /// initializer invoked on the *same* object with those arguments (step
    /// 3), and only then this level's own slot clauses and `initExprs`
    /// (steps 4-5).
    ///
    /// One activation per level is the whole point: a superclass's slot
    /// initializers and `initExprs` see the arguments *its* clause supplied,
    /// not the subclass's. Milestone 1 threaded a single frame up the whole
    /// chain and never evaluated a clause message at all, so a superclass
    /// factory parameter read whatever happened to sit at the same index in
    /// the subclass's argument list -- or panicked when there was none.
    fn run_initializer_level(
        &mut self,
        class: &ClassRef,
        instance: &Value,
        factory_args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Result<(), Signal> {
        // Regression fix (re-review finding A): reject a factory-arity
        // mismatch as a NewspeakError before building this level's `Frame`
        // or running any slot initializer / initExpr. A slot initializer or
        // initExpr may reference a factory parameter as `Binding::Local(i)`,
        // resolved against this class's *declared* factory pattern -- but
        // the `Frame` built below starts with `temps: vec![]`, so if fewer
        // arguments were supplied than the pattern declares, `eval.rs`'s
        // `read_local`/`write_local` index straight past the end of both
        // `args` and `temps` and Rust panics with "index out of bounds".
        //
        // Checked at *every* level, not just the outermost. A level's
        // arguments now come from its subclass's clause message, whose
        // selector `eval_superclass_message` has already matched against
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
                    selector: Symbol::intern("new"),
                    receiver: Value::Obj(*class),
                    message: format!(
                        "wrong number of factory arguments: factory takes {}, got {}",
                        declared_arity,
                        factory_args.len()
                    ),
                }));
            }
        }

        // Step 3: the superclass clause's message, then the superclass's own
        // instance initializer on the same object -- deepest (Object) first,
        // because this recursion runs before this level's own steps 4-5.
        let superclass = self.heap.class_superclass(class);
        if let Some(superclass) = superclass {
            let super_args = self.eval_superclass_message(
                class,
                &superclass,
                instance,
                &factory_args,
                bindings,
            )?;
            self.run_initializer_level(&superclass, instance, super_args, bindings)?;
        }

        // Step 2, built here rather than above it: a level with no slots and
        // no `initExprs` runs no code at all under either engine (see
        // `run_own_initializer`), so its activation could never be observed
        // -- and every chain ends at a builtin, which is exactly such a
        // level. Building it only where something runs in it keeps one
        // activation per *initializing* level rather than one per level.
        //
        // Every field of it describes this one level: its arguments, its
        // class, and its `slot_base` -- the absolute index a compiled
        // `InitSlot`'s mixin-relative `own_index` is offset by. `slot_base`
        // is set here, with the rest of the level's identity, and never
        // changes: with a frame per level there is nothing to save and
        // restore, and no window in which a frame describes another level.
        if !class_has_initializer_code(&self.heap, class) {
            return Ok(());
        }
        let frame = self.heap.alloc_frame(Frame {
            receiver: *instance,
            args: factory_args,
            temps: vec![],
            pc: 0,
            // `run_own_initializer` below picks the body off the same
            // engine, and this frame is built only when that level has
            // initializer code to run -- so under `Engine::Bytecode` it
            // always ends up executing a `CompiledCode`.
            pc_kind: match &self.engine {
                Engine::TreeWalk => PcKind::TreeWalk,
                Engine::Bytecode(_) => PcKind::Bytecode,
            },
            sender: None,
            home: None,
            current_class: Some(*class),
            kind: FrameKind::Initializer(*class),
            uncontinuable: false,
            slot_base: self
                .heap
                .class_application(class)
                .map_or(0, |a| a.own_slot_base),
        });

        // Steps 4-5: this class's own slot clauses (declaration order), then
        // its own initExprs -- engine hook 3.
        self.run_own_initializer(class, instance, &frame, bindings)
    }

    /// Step 3's first half: `newspeak-spec.pdf` §6.2's "the superclass
    /// clause's message is evaluated to a concrete message". Returns the
    /// argument list the superclass's instance initializer is invoked with.
    ///
    /// §6.1 makes two things run time errors here. That the clause's
    /// expression is not a class object is caught where the clause's
    /// *receiver* is evaluated -- once, when the declaration is evaluated
    /// (`interpreter::nested`), not per instantiation. That the message's
    /// selector is not the superclass's primary factory selector is caught
    /// here, because only here are both halves known.
    ///
    /// The selector is checked *before* the arguments are evaluated: a
    /// clause that cannot be satisfied should not first run its arguments'
    /// side effects.
    fn eval_superclass_message(
        &mut self,
        class: &ClassRef,
        superclass: &ClassRef,
        instance: &Value,
        factory_args: &[Value],
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Result<Vec<Value>, Signal> {
        // One lookup of the application for both questions below: the clause
        // this class was declared with, and (under the bytecode engine) the
        // mixin its compiled message body is keyed by.
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
        let selector = clause.map_or_else(|| Symbol::intern("new"), |clause| clause.selector);
        let expected = primary_factory_selector(&self.heap, superclass);
        if Some(selector) != expected {
            return Err(not_the_primary_factory_error(
                &self.heap, class, superclass, selector, expected,
            ));
        }
        if !clause.is_some_and(|clause| clause.has_args) {
            // `Object new` and every other argument-free clause: nothing to
            // evaluate, no activation built, nothing allocated.
            return Ok(vec![]);
        }
        let compiled = match (&self.engine, &application) {
            (Engine::TreeWalk, _) | (_, None) => None,
            (Engine::Bytecode(module), Some(application)) => Some(
                module
                    .superclass_messages
                    .get(&crate::mixin::mixin_key(&application.mixin))
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!("internal: mixin whose superclass clause message takes arguments has no compiled form for it (compile_module coverage gap)")
                    }),
            ),
        };
        // The argument expressions are for the tree-walker alone, and
        // building them clones the whole argument list -- on a path that
        // runs at every instantiation, so it is built only when there is no
        // compiled body to run instead.
        let args_expr = match compiled {
            Some(_) => None,
            None => application
                .as_ref()
                .and_then(|application| application.mixin.decl.as_ref())
                .and_then(|decl| decl.superclass.as_ref())
                .and_then(super::nested::superclass_clause_message_args_expr),
        };
        // The message is evaluated on the instance being initialized and
        // with *this* level's factory arguments -- §6.2's "in the context of
        // the instance initializer". `run_clause_body` is the same shared
        // activation the nested-class accessor runs the clause's receiver
        // through, so neither engine gets a path of its own here.
        let value = super::nested::run_clause_body(
            self,
            instance,
            factory_args.to_vec(),
            class,
            FrameKind::SuperclassMessage(*class),
            compiled,
            args_expr.as_ref(),
            bindings,
        )?;
        argument_list(&self.heap, class, value)
    }

    /// Engine hook 3 (M2 spec, "`vm.rs` and the engine hook"): Class
    /// Instantiation steps 4-5 for one level of the chain -- this class's
    /// own slot initializers, then its own `initExprs`, on that level's own
    /// initializer frame. The tree-walker runs `run_own_slots` +
    /// `run_own_init_exprs`; the bytecode engine runs the class's compiled
    /// initializer (`compiler::compile_initializer`) after growing the
    /// frame's register file to fit it. Both engines run the same steps in
    /// the same order on the same frame.
    ///
    /// Review finding (round 1): `InitSlot`'s `own_index` is relative to the
    /// mixin's own slots, not an absolute field index -- the compiled body
    /// is shared by every application of the mixin (`compile_module` keys
    /// by `mixin_key`), so it cannot bake in any one application's offset.
    /// `frame.slot_base` is what turns `own_index` into an absolute index at
    /// run time: the same per-application value `run_own_slots`'s
    /// tree-walking sibling reads through `slot_index`, fetched once instead
    /// of per slot. Its one writer is `run_initializer_level`, which builds
    /// one frame per level and sets that frame's base from that level's own
    /// `MixinApplication.own_slot_base` -- so a frame's base is fixed for
    /// its whole life and cannot be observed describing another level. The
    /// assertion below states that invariant where this function depends on
    /// it, in place of the save/restore a single frame shared by the whole
    /// chain used to need.
    fn run_own_initializer(
        &mut self,
        class: &ClassRef,
        instance: &Value,
        frame: &FrameRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Result<(), Signal> {
        let compiled = match &self.engine {
            Engine::TreeWalk => None,
            Engine::Bytecode(module) => {
                // Every BuiltinClass, and any class with nothing to run:
                // a no-op under both engines. `run_initializer_level` does
                // not even build a frame for such a level; this repeats the
                // test because `compile_module` emits no initializer for
                // such a mixin, so the lookup below would have nothing to
                // find if some other caller ever arrived here.
                if !class_has_initializer_code(&self.heap, class) {
                    return Ok(());
                }
                Some({
                    // Keyed by mixin, not by class object (§6.2: every
                    // application of a mixin shares its code) -- so the
                    // lookup goes through the class's application, not its
                    // own arena index. The application itself is kept
                    // (not just its mixin) because its `own_slot_base` is
                    // what makes the shared code correct for *this* level.
                    let application = self.heap.class_application(class).expect(
                        "internal: class_has_initializer_code confirmed this is a class object",
                    );
                    let code = module
                        .initializers
                        .get(&crate::mixin::mixin_key(&application.mixin))
                        .cloned()
                        .unwrap_or_else(|| {
                            panic!("internal: class has no compiled instance initializer (compile_module coverage gap)")
                        });
                    (code, application.own_slot_base)
                })
            }
        };
        match compiled {
            None => {
                self.run_own_slots(class, instance, frame, bindings)?;
                self.run_own_init_exprs(class, frame, bindings)
            }
            Some((code, own_slot_base)) => {
                debug_assert_eq!(
                    self.heap.frame_slot_base(frame),
                    own_slot_base,
                    "an initializer frame must be built for the level it runs"
                );
                let needed = (code.num_registers - code.num_args) as usize;
                self.heap.grow_temps(frame, needed);
                crate::vm::run_code(self, frame, &code, bindings).map(|_| ())
            }
        }
    }

    fn run_own_slots(
        &mut self,
        class: &ClassRef,
        instance: &Value,
        frame: &FrameRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Result<(), Signal> {
        // The application itself, not copies of the slot list and the merged
        // `slot_index` map it holds: both are read-only here, and cloning
        // them once per level per instantiation is allocation on the
        // tree-walker's hottest path (and in the baseline `bench` hands the
        // next milestone). Holding the `Rc` also releases the borrow on
        // `class`, so the loop below can call back into `self`.
        let Some(application) = self.heap.class_application(class) else {
            return Ok(());
        };
        // Only the tree-walker reaches this function, and
        // `Interpreter::set_engine` refuses the tree-walker on a world
        // decoded from an image (`EngineUnavailable::TreeWalkAfterLoad`), so
        // every mixin that gets here was built from source and every slot
        // has its declaration. That refusal is what makes skipping a
        // declarationless slot below safe to read as "no slot is dropped":
        // without it a decoded slot would be skipped silently and the
        // instance would come back half-initialised.
        for slot in application
            .mixin
            .slots
            .iter()
            .filter_map(|s| s.decl.as_ref())
        {
            if let Some(init) = &slot.init {
                let value = crate::interpreter::eval::eval_expr(self, init, frame, bindings)?;
                let name = Symbol::intern(&slot.name.name);
                // Absolute index (superclass slots offset, per the
                // `slot_index` fix above) -- never the slot's local,
                // this-class-only position. Using a bare `enumerate()`
                // index here would silently overwrite an already-initialized
                // superclass slot at the same low index instead of this
                // class's own slot.
                if let Some(&index) = application.slot_index.get(&name) {
                    crate::interpreter::eval::store_instance_field(
                        &mut self.heap,
                        instance,
                        index,
                        value,
                    );
                }
            }
        }
        Ok(())
    }

    /// Class instantiation "must fix" #1: evaluates this class's own
    /// `initExprs` (spec's Class Instantiation step 5), in declaration
    /// order, discarding each statement's value -- mirroring
    /// `Interpreter::run_statements`'s loop but without a return-value
    /// capture. A `Stmt::Return` inside an `initExprs` list is not
    /// meaningful (there is no enclosing method to return from; M1's
    /// fixtures do not exercise it) -- treated the same as `Stmt::Expr` here
    /// (evaluate the expression, discard the value) rather than raising a
    /// non-local return with nowhere sensible to land.
    ///
    /// A mixin decoded from an image records *that* it has `initExprs`
    /// (`has_init_exprs`) but not the statements themselves, so this loop
    /// would run zero of them and report success. It never sees one:
    /// `Interpreter::set_engine` refuses the tree-walker after a load
    /// (`EngineUnavailable::TreeWalkAfterLoad`), and under the bytecode
    /// engine the same step is the class's compiled initializer, which
    /// `image::decode` refuses an image for omitting.
    fn run_own_init_exprs(
        &mut self,
        class: &ClassRef,
        frame: &FrameRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Result<(), Signal> {
        // The mixin, not a deep copy of the statements it holds: the AST is
        // read-only here, so this is an `Rc` bump rather than a clone of
        // every `initExpr` node on every instantiation. Holding it also
        // releases the borrow on `class` for the loop below.
        let Some(application) = self.heap.class_application(class) else {
            return Ok(());
        };
        for stmt in &application.mixin.init_exprs {
            match stmt {
                ast::Stmt::Expr(e) | ast::Stmt::Return(e) => {
                    crate::interpreter::eval::eval_expr(self, e, frame, bindings)?;
                }
            }
        }
        Ok(())
    }
}

/// The selector of `class`'s primary factory message: the selector its mixin
/// declares a factory pattern for, or `new` for a builtin, which declares
/// none. `None` for a non-class object.
///
/// One definition, shared by `try_factory_send` (which decides whether an
/// incoming send *is* the factory message) and `eval_superclass_message`
/// (which decides whether a superclass clause names it) -- `newspeak-spec.pdf`
/// §6.1 makes those the same question asked from two sides.
pub(crate) fn primary_factory_selector(heap: &Heap, class: &ClassRef) -> Option<Symbol> {
    heap.class_application(class)
        .map(|application| match &application.mixin.factory {
            Some(factory) => factory.selector,
            None => Symbol::intern("new"),
        })
}

fn class_name(heap: &Heap, class: &ClassRef) -> Symbol {
    heap.class_application(class).map_or_else(
        || Symbol::intern("an object"),
        |application| application.mixin.name,
    )
}

/// `newspeak-spec.pdf` §6.1: "It is a run time error if the message
/// determined by the inheritance clause is not the primary factory message
/// of C." Raised from shared orchestration, so both engines print the same
/// stderr byte for byte.
fn not_the_primary_factory_error(
    heap: &Heap,
    class: &ClassRef,
    superclass: &ClassRef,
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
        receiver: Value::Obj(*superclass),
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
/// `image::decode::build_module` reconciles its kind and arity but nothing
/// constrains what it returns. An image is untrusted input.
fn argument_list(heap: &Heap, class: &ClassRef, value: Value) -> Result<Vec<Value>, Signal> {
    if let Value::Obj(obj) = &value {
        if let Some(items) = heap.array_elements(obj) {
            return Ok(items);
        }
    }
    Err(Signal::Error(NewspeakError {
        selector: Symbol::intern("new"),
        receiver: value,
        message: format!(
            "the superclass clause of class {} did not evaluate its message \
             arguments to an argument list",
            class_name(heap, class)
        ),
    }))
}

/// `pub(crate)` (rather than private) so `eval.rs`'s `eval_ident` can reuse
/// this exact string->`BuiltinClass` mapping for Gap 1's bare-builtin-name
/// fallback, instead of duplicating it. Returns `None` for any name that is
/// not a Milestone 1 builtin class name (an explicit `Object` arm replaces
/// the old catch-all default of `BuiltinClass::Object`, since a caller
/// outside this module must be able to tell "unrecognized name" apart from
/// a genuine `Object` match -- `resolve_superclass_receiver` above used to
/// guard this call with a separate `BUILTIN_NAMES.contains` check for
/// exactly that reason; that list is now redundant with this function's own
/// match arms, so it was folded in here instead of kept as a second,
/// duplicate source of truth).
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
pub(crate) fn class_has_initializer_code(heap: &Heap, class: &ClassRef) -> bool {
    heap.class_application(class)
        .is_some_and(|application| mixin_needs_initializer_code(&application.mixin))
}

/// Whether `mixin` declares any slot or `initExpr` at all.
///
/// `pub(crate)` for `image::decode::build_module`, which must refuse
/// an image that has no compiled initializer for a mixin that needs one
/// rather than let `run_own_initializer` above panic on the invariant --
/// checked per mixin now that a code table covers the whole tree, not only
/// the root.
pub(crate) fn mixin_needs_initializer_code(mixin: &crate::mixin::Mixin) -> bool {
    !mixin.slots.is_empty() || mixin.has_init_exprs
}

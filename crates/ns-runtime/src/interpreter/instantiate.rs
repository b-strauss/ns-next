//! Class instantiation: `newspeak-spec.pdf` §6.1, §6.3. See the runtime
//! spec's Class Instantiation section for the six numbered steps this
//! transcribes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::types::*;
use crate::Symbol;

use super::{Engine, EvalResult, Interpreter, Signal};

impl Interpreter {
    /// Materializes the module's root `ClassDecl` into a real class object.
    /// The superclass clause's *receiver* is resolved here (once); its
    /// trailing *message* is a known Milestone 1 gap -- it is neither stored
    /// on `ClassData` nor ever evaluated (see `run_superclass_initializer`'s
    /// doc comment below for what that costs).
    pub fn load_module(&mut self, resolved: &ResolvedModule) -> ClassRef {
        self.load_class(&resolved.module.root_class, None)
    }

    /// Note on the brief's original signature: `decl` is taken here as
    /// `&ast::ClassDecl`, not `&Rc<ast::ClassDecl>` -- `ast::Module::root_class`
    /// is a plain (non-`Rc`) `ClassDecl` (see `crates/ns-syntax/src/ast.rs`),
    /// so `load_module`'s `&resolved.module.root_class` argument is
    /// `&ast::ClassDecl`; a `&Rc<ast::ClassDecl>` parameter here would not
    /// type-check against that call site. This is the kind of small,
    /// mechanical fix every task in this plan has needed at least once.
    ///
    /// # Milestone 1 known gaps
    ///
    /// **Nested classes are not loaded.** This function never recurses into
    /// `decl.instance_side.nested_classes` (nor the class side's), and
    /// `enclosing` is therefore always `None` for every class it builds. The
    /// *resolver* fully supports nesting -- it resolves `outer C foo` and
    /// depth-k implicit sends statically, and `dispatch::kth_enclosing_object`
    /// implements the runtime half -- but with no nested class ever loaded and
    /// no `ClassData.enclosing` ever set, any send that actually needs a
    /// depth > 0 enclosing object resolves statically and then fails at
    /// runtime (`kth_enclosing_object` returns `Value::Nil` as soon as it
    /// asks for an enclosing object that is not there). Loading nested
    /// classes is a substantial feature -- it needs a load order, a mutual
    /// reference story between an instance and its enclosing object, and the
    /// accessor a nested class declaration implicitly defines -- and no
    /// Milestone 1 fixture or corpus module needs it.
    ///
    /// **Class-side methods are discarded.** `class_side` below is set to
    /// `None` unconditionally: `decl.class_side`'s methods are parsed,
    /// resolved, and then dropped here. Class-side / class methods are out of
    /// scope for Milestone 1 (there are no metaclasses either -- every class
    /// object's own `.class` is the one shared `Object`-tagged placeholder),
    /// so there is nothing anywhere in the crate that would consume a
    /// class-side entry if one were stored.
    fn load_class(&mut self, decl: &ast::ClassDecl, enclosing: Option<ObjRef>) -> ClassRef {
        let superclass = self.resolve_superclass_receiver(decl);
        let mut methods = HashMap::new();
        for m in &decl.instance_side.methods {
            methods.insert(Symbol::intern(&method_name(&m.pattern)), Rc::new(m.clone()));
        }
        let instance_slots: Vec<Rc<ast::SlotDecl>> = decl
            .instance_side
            .slots
            .iter()
            .map(|s| Rc::new(s.clone()))
            .collect();
        // Slot access fix (Task 7 "must fix before committing" #2): each of
        // this class's own slots gets an absolute index into an instance's
        // `ObjectData::Instance` fields vector, offset past every inherited
        // slot -- mirrors `total_slot_count`'s walk below, but per-name.
        let mut slot_index = HashMap::new();
        let base = superclass.as_ref().map(total_slot_count).unwrap_or(0);
        for (i, slot) in instance_slots.iter().enumerate() {
            slot_index.insert(Symbol::intern(&slot.name.name), base + i);
        }
        let identity_hash = self.alloc_identity_hash();
        Rc::new(RefCell::new(Object {
            class: self.builtin_class(BuiltinClass::Object), // M1 simplification, see Task 7 Step 1
            identity_hash,
            data: ObjectData::Class(ClassData {
                name: Symbol::intern(&decl.name.name),
                superclass,
                methods,
                instance_slots,
                slot_index,
                // Class instantiation "must fix" #1: this class's own
                // initExprs, evaluated for real by `run_own_init_exprs`
                // below (Step 5 of the spec's six numbered steps).
                init_exprs: decl.instance_side.init_exprs.clone(),
                // Milestone 1 known gap (see this function's doc comment):
                // class-side methods are out of scope, so `decl.class_side`
                // is discarded here rather than stored. Nothing in the crate
                // reads `ClassData.class_side`; it exists for a later
                // milestone.
                class_side: None,
                enclosing,
                factory: Some(Rc::new(decl.factory.clone().unwrap_or(
                    ast::MessagePattern::Unary {
                        selector: ast::Ident {
                            name: "new".to_string(),
                            span: decl.span,
                        },
                    },
                ))),
                builtin: None,
            }),
        }))
    }

    /// Intercepts an ordinary explicit-receiver send (`Interpreter::send`'s
    /// call site, above -- checked *before* `dispatch::lookup_public`) when
    /// `receiver` is a class object and `selector` is that class's own
    /// factory selector: `class_data.factory`'s pattern if the class
    /// declared one (every user-defined class does -- `load_class` above
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
    /// message (e.g. `Object new` in `class Foo = Object new (...) ...`):
    /// that message is never evaluated at all in Milestone 1 (a known gap --
    /// see `run_superclass_initializer` below), so there is nothing here to
    /// double-dispatch against. `run_superclass_initializer` recurses
    /// directly into `run_superclass_initializer`/`run_own_slots`/
    /// `run_own_init_exprs` and never calls `Interpreter::send` (nor,
    /// therefore, this method) at all -- confirmed by inspection of every
    /// call site in this file.
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
        let (factory_selector, builtin) = {
            let borrowed = obj.borrow();
            let ObjectData::Class(class_data) = &borrowed.data else {
                return None;
            };
            let factory_selector = match &class_data.factory {
                Some(pattern) => Symbol::intern(&method_name(pattern)),
                None => Symbol::intern("new"),
            };
            (factory_selector, class_data.builtin)
        };
        if selector != factory_selector {
            return None;
        }
        let class_ref = obj.clone();
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
    /// section.
    pub fn instantiate(
        &mut self,
        class: ClassRef,
        factory_args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> EvalResult {
        // Regression fix (re-review finding A): reject a factory-arity
        // mismatch as a NewspeakError up front, before building the
        // instance-initializer `Frame` or running any slot initializer /
        // initExpr. Since 1e07dee, a slot initializer or initExpr may
        // reference a factory parameter as `Binding::Local(i)`, resolved
        // against this class's *declared* factory pattern -- but the
        // `Frame` built below has `temps: vec![]` always, so if fewer
        // `factory_args` were actually supplied than the pattern declares,
        // `eval.rs`'s `read_local`/`write_local` index straight past the end
        // of both `args` and the (empty) `temps` and Rust panics with "index
        // out of bounds". `try_factory_send`'s call sites always pass
        // exactly `class`'s own declared arity, so this can only fire when a
        // caller outside the ordinary factory-send path (today, only
        // `ns-cli`'s `cmd_run`, which hardcodes a single `usingPlatform:`
        // argument regardless of the root class's actual declared arity)
        // hands `instantiate` a mismatched `factory_args`. Mirrors
        // `eval.rs`'s `invoke_closure`, which has the exact same shape of
        // guard for the analogous block-arity case: never panic on this
        // Newspeak-program-input path.
        let declared_arity = {
            let borrowed = class.borrow();
            match &borrowed.data {
                ObjectData::Class(data) => data.factory.as_deref().map(pattern_param_count),
                _ => None,
            }
        };
        if let Some(declared_arity) = declared_arity {
            if factory_args.len() != declared_arity {
                return Err(Signal::Error(NewspeakError {
                    selector: Symbol::intern("new"),
                    receiver: Value::Obj(class),
                    message: format!(
                        "wrong number of factory arguments: factory takes {}, got {}",
                        declared_arity,
                        factory_args.len()
                    ),
                }));
            }
        }

        // Step 1: one instance, every slot nil.
        let slot_count = total_slot_count(&class);
        let instance = self.new_instance_object(
            class.clone(),
            ObjectData::Instance(vec![Value::Nil; slot_count]),
        );
        let receiver = Value::Obj(instance);

        // Step 2: the instance-initializer activation.
        let frame = Rc::new(RefCell::new(Frame {
            receiver: receiver.clone(),
            args: factory_args,
            temps: vec![],
            pc: 0,
            sender: None,
            home: None,
            current_class: Some(class.clone()),
            kind: FrameKind::Initializer(class.clone()),
            uncontinuable: false,
        }));

        // Step 3: run the superclass chain first, deepest (Object) first --
        // implemented as recursion that runs *before* this class's own
        // slots/initExprs, per the spec ("execution of the instance
        // initializer begins with evaluation of the superclass clause...
        // the superclass' instance initializer is then invoked on o").
        self.run_superclass_initializer(&class, &receiver, &frame, bindings)?;

        // Steps 4-5: this class's own slot clauses (declaration order),
        // then its own initExprs -- engine hook 3.
        self.run_own_initializer(&class, &receiver, &frame, bindings)?;

        // Step 6.
        Ok(receiver)
    }

    /// Runs the superclass chain's instance initializers, deepest first
    /// (spec step 3).
    ///
    /// # Milestone 1 known gaps
    ///
    /// **The superclass clause's message is never evaluated.** The spec has
    /// the subclass's initializer evaluate the superclass clause's trailing
    /// factory message (`Object new` in `class Foo = Object new (...)`, or
    /// `Super usingPlatform: p` with real arguments) in the *subclass's* own
    /// initializer context, and pass its arguments to the superclass's
    /// instance initializer. This function does none of that: it walks
    /// straight up `ClassData.superclass` and runs each level's slots and
    /// initExprs. The clause's message is not even carried on `ClassData` --
    /// `load_class` stores only `factory`, this class's *own* primary factory
    /// pattern, while the clause's own message list lives on
    /// `ast::SuperclassClause::messages`.
    ///
    /// This is fine exactly when the superclass factory message has no
    /// behavior beyond structural instantiation -- true for every Milestone 1
    /// fixture and for `Object new`, which is what every fixture and corpus
    /// module in this milestone actually writes -- and is a real gap for a
    /// superclass factory message that carries arguments or side effects.
    ///
    /// **Every level runs in the subclass's frame.** `subclass_frame` is
    /// reused for every superclass level, so a superclass's own slot
    /// initializers and initExprs see the *subclass's* factory arguments
    /// rather than their own. Correcting this means building one activation
    /// per level from the (unevaluated, see above) superclass clause's
    /// arguments -- the same missing piece. Milestone 1 never links a
    /// user-defined superclass through the ordinary pipeline anyway (see
    /// `resolve_superclass_receiver`: only builtin superclass names resolve,
    /// and builtins have no slots or initExprs), so no shipped path can
    /// currently observe the difference.
    fn run_superclass_initializer(
        &mut self,
        class: &ClassRef,
        instance: &Value,
        subclass_frame: &FrameRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Result<(), Signal> {
        let superclass = {
            let borrowed = class.borrow();
            match &borrowed.data {
                ObjectData::Class(data) => data.superclass.clone(),
                _ => None,
            }
        };
        let Some(superclass) = superclass else {
            return Ok(()); // reached Object (or a builtin) -- native classes have no init_exprs to run
        };
        // NOTE: the superclass clause's deferred factory message is NOT
        // evaluated here -- see this function's "Milestone 1 known gaps"
        // doc comment above for what that costs and why it is deferred.
        // `instance` is threaded through only for the recursive calls below
        // (it is the object every level's slots are written into); no
        // superclass-clause argument is computed from it today.
        //
        // Recurse first -- the *superclass's own* superclass initializer
        // must run before the superclass's own slots/initExprs, exactly the
        // same "deepest first" ordering this function's caller applies one
        // level up (see the comment at the `instantiate` call site above).
        // The brief's original draft of this function ran only one level
        // (this class's immediate superclass) with no further recursion,
        // which contradicts its own "deepest (Object) first" framing for
        // any hierarchy more than two levels deep (Sub -> Mid -> Object):
        // Mid's own superclass initializer (i.e. Object's, a no-op today,
        // but not a no-op in general once user classes can chain three or
        // more deep) would silently never run. Fixed by recursing here.
        self.run_superclass_initializer(&superclass, instance, subclass_frame, bindings)?;
        self.run_own_initializer(&superclass, instance, subclass_frame, bindings)?;
        Ok(())
    }

    /// Engine hook 3 (M2 spec, "`vm.rs` and the engine hook"): Class
    /// Instantiation steps 4-5 for one level of the chain -- this class's
    /// own slot initializers, then its own `initExprs`, on the shared
    /// initializer frame. The tree-walker runs `run_own_slots` +
    /// `run_own_init_exprs`; the bytecode engine runs the class's compiled
    /// initializer (`compiler::compile_initializer`) after growing the
    /// frame's register file to fit it. Every M1 instantiation gap is
    /// carried forward identically in both engines.
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
                if !class_has_initializer_code(class) {
                    // Every BuiltinClass, and any class with nothing to run:
                    // a no-op under both engines.
                    return Ok(());
                }
                Some(
                    module
                        .initializers
                        .get(&crate::bytecode::class_key(class))
                        .cloned()
                        .unwrap_or_else(|| {
                            panic!("internal: class has no compiled instance initializer (compile_module coverage gap)")
                        }),
                )
            }
        };
        match compiled {
            None => {
                self.run_own_slots(class, instance, frame, bindings)?;
                self.run_own_init_exprs(class, frame, bindings)
            }
            Some(code) => {
                {
                    let mut f = frame.borrow_mut();
                    let needed = (code.num_registers - code.num_args) as usize;
                    if f.temps.len() < needed {
                        f.temps.resize(needed, Value::Nil);
                    }
                }
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
        let slots = {
            let borrowed = class.borrow();
            match &borrowed.data {
                ObjectData::Class(data) => data.instance_slots.clone(),
                _ => return Ok(()),
            }
        };
        let slot_index = {
            let borrowed = class.borrow();
            match &borrowed.data {
                ObjectData::Class(data) => data.slot_index.clone(),
                _ => return Ok(()),
            }
        };
        for slot in &slots {
            if let Some(init) = &slot.init {
                let value = crate::interpreter::eval::eval_expr(self, init, frame, bindings)?;
                let name = Symbol::intern(&slot.name.name);
                // Absolute index (superclass slots offset, per the
                // `slot_index` fix above) -- never the slot's local,
                // this-class-only position. Using a bare `enumerate()`
                // index here would silently overwrite an already-initialized
                // superclass slot at the same low index instead of this
                // class's own slot.
                if let Some(&index) = slot_index.get(&name) {
                    crate::interpreter::eval::store_instance_field(instance, index, value);
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
    fn run_own_init_exprs(
        &mut self,
        class: &ClassRef,
        frame: &FrameRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Result<(), Signal> {
        let init_exprs = {
            let borrowed = class.borrow();
            match &borrowed.data {
                ObjectData::Class(data) => data.init_exprs.clone(),
                _ => return Ok(()),
            }
        };
        for stmt in &init_exprs {
            match stmt {
                ast::Stmt::Expr(e) | ast::Stmt::Return(e) => {
                    crate::interpreter::eval::eval_expr(self, e, frame, bindings)?;
                }
            }
        }
        Ok(())
    }
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
/// method it activates by selector, re-resolved on load against the reloaded
/// class's method map (`image::decode`).
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
/// every `BuiltinClass` and for a non-class object.
///
/// `pub(crate)` for `image::decode::install_code_table`, which must refuse
/// an image that has no compiled initializer for a class that needs one
/// rather than let `run_own_initializer` below panic on the invariant.
/// One definition of "needs initializer code", not one per caller.
pub(crate) fn class_has_initializer_code(class: &ClassRef) -> bool {
    let borrowed = class.borrow();
    match &borrowed.data {
        ObjectData::Class(data) => !(data.instance_slots.is_empty() && data.init_exprs.is_empty()),
        _ => false,
    }
}

fn total_slot_count(class: &ClassRef) -> usize {
    let mut count = 0;
    let mut current = Some(class.clone());
    while let Some(c) = current {
        let borrowed = c.borrow();
        let ObjectData::Class(data) = &borrowed.data else {
            break;
        };
        count += data.instance_slots.len();
        current = data.superclass.clone();
    }
    count
}

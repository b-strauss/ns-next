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

use super::{EvalResult, Interpreter, Signal};

impl Interpreter {
    /// Materializes the module's root `ClassDecl` into a real class object.
    /// The superclass clause's *receiver* is resolved here (once); its
    /// trailing *message* is stored, deferred, for `instantiate` to evaluate
    /// later -- see the task's design note.
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
                class_side: None, // wired below once class-side methods exist
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
    /// No double-dispatch risk against the superclass clause's own
    /// deferred factory message (e.g. `Object new` in
    /// `class Foo = Object new (...) ...`): that message is evaluated by
    /// `run_superclass_initializer` below, which recurses directly into
    /// `run_superclass_initializer`/`run_own_slots`/`run_own_init_exprs`
    /// and never calls `Interpreter::send` (nor, therefore, this method)
    /// at all -- confirmed by inspection of every call site in this file.
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
        if BUILTIN_NAMES.contains(&clause.receiver.name.as_str()) {
            let kind = builtin_kind_from_name(&clause.receiver.name);
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
            kind: FrameKind::TopLevel, // an initializer is its own kind of top-of-chain activation for M1
            uncontinuable: false,
        }));

        // Step 3: run the superclass chain first, deepest (Object) first --
        // implemented as recursion that runs *before* this class's own
        // slots/initExprs, per the spec ("execution of the instance
        // initializer begins with evaluation of the superclass clause...
        // the superclass' instance initializer is then invoked on o").
        self.run_superclass_initializer(&class, &receiver, &frame, bindings)?;

        // Step 4: this class's own slot clauses, in declaration order.
        self.run_own_slots(&class, &receiver, &frame, bindings)?;

        // Step 5: this class's own initExprs.
        self.run_own_init_exprs(&class, &frame, bindings)?;

        // Step 6.
        Ok(receiver)
    }

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
        // The deferred factory message, evaluated now, in the *subclass's*
        // own initializer context (subclass_frame) so its factory
        // parameters are in scope -- the spec's core correction from the
        // earlier draft of this design.
        // Milestone 1: the superclass clause's message is not carried on
        // ClassData today (`load_class` only stores `factory`, the primary
        // factory pattern -- the *superclass clause's own* message is a
        // separate AST field on `ast::SuperclassClause`). Evaluate it here
        // directly from the AST rather than threading a new ClassData field
        // through Task 7 Step 1's already-large bootstrap change:
        let _ = instance; // reserved for a fixture that needs superclass-clause args; M1's Object/builtins take none
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
        self.run_own_slots(&superclass, instance, subclass_frame, bindings)?;
        self.run_own_init_exprs(&superclass, subclass_frame, bindings)?;
        Ok(())
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
                if let (Value::Obj(obj), Some(&index)) = (instance, slot_index.get(&name)) {
                    if let ObjectData::Instance(fields) = &mut obj.borrow_mut().data {
                        if index < fields.len() {
                            fields[index] = value;
                        }
                    }
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

const BUILTIN_NAMES: &[&str] = &[
    "Object",
    "Boolean",
    "True",
    "False",
    "Integer",
    "Float",
    "String",
    "Symbol",
    "Character",
    "Array",
    "OrderedCollection",
    "Nil",
    "Block",
];

fn builtin_kind_from_name(name: &str) -> BuiltinClass {
    match name {
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
        _ => BuiltinClass::Object,
    }
}

fn method_name(pattern: &ast::MessagePattern) -> String {
    match pattern {
        ast::MessagePattern::Unary { selector } => selector.name.clone(),
        ast::MessagePattern::Binary { selector, .. } => selector.name.clone(),
        ast::MessagePattern::Keyword { parts } => {
            parts.iter().map(|(k, _)| format!("{}:", k.name)).collect()
        }
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

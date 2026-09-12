//! Class instantiation: `newspeak-spec.pdf` §6.1, §6.3. See the runtime
//! spec's Class Instantiation section for the six numbered steps this
//! transcribes.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::heap::{ActivationHeader, Heap};
use crate::mixin::{Factory, Mixin, MixinMethod, MixinSlot, SuperclassClause};
use crate::types::*;
use crate::Symbol;

use super::{EvalResult, Interpreter, Signal};

/// A class body's methods as a mixin's method dictionary: everything a
/// lookup or `Interpreter::invoke_method` reads is lifted out of the
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
    /// instantiation, where `run_initializer_levels` evaluates it off the
    /// mixin's own declaration.
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
    pub fn load_module(&mut self, resolved: &ResolvedModule) -> ClassRef {
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
    /// message is *evaluated* (`run_initializer_levels` below), but it is
    /// never *sent*: §6.2 has the subclass's initializer invoke the
    /// superclass's instance initializer directly on the same object, so
    /// `run_initializer_levels` runs it level by level and never calls
    /// `Interpreter::send` -- and therefore never reaches this method, which
    /// would otherwise allocate a second, discarded instance per level.
    pub(super) fn try_factory_send(
        &mut self,
        receiver: &Value,
        selector: Symbol,
        args: &[Value],
    ) -> Option<EvalResult> {
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
            Some(BuiltinClass::OrderedCollection) | Some(BuiltinClass::Array) => Ok(Value::Obj(
                self.new_instance_object(class_ref, ObjectData::Array(vec![])),
            )),
            _ => self.instantiate(class_ref, args.to_vec()),
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

    /// The six numbered steps in `docs/design/object-model.md`,
    /// "Instantiation"; see `instantiate_unchecked`. Like `send`, it leaves
    /// the activation stack as it found it.
    pub fn instantiate(&mut self, class: ClassRef, factory_args: Vec<Value>) -> EvalResult {
        let depth = self.heap.activation_depth();
        let result = self.instantiate_unchecked(class, factory_args);
        debug_assert_eq!(
            self.heap.activation_depth(),
            depth,
            "instantiation must leave the activation stack as it found it"
        );
        result
    }

    /// The six numbered steps in `docs/design/object-model.md`,
    /// "Instantiation". Steps 2-5 are `run_initializer_levels`, for every
    /// level of the superclass chain.
    fn instantiate_unchecked(&mut self, class: ClassRef, factory_args: Vec<Value>) -> EvalResult {
        // Step 1: one instance, every slot nil. The application already
        // knows the whole layout, inherited slots included -- summing the
        // chain here again would be a second answer to the same question.
        let slot_count = self
            .heap
            .class_application(class)
            .map_or(0, |a| a.num_slots);
        let instance =
            self.new_instance_object(class, ObjectData::Instance(vec![Value::Nil; slot_count]));
        let receiver = Value::Obj(instance);

        // Steps 2-5, for this class and every level above it.
        self.run_initializer_levels(class, &receiver, factory_args)?;

        // Step 6.
        Ok(receiver)
    }

    /// Class Instantiation steps 2-5 for every level of `class`'s superclass
    /// chain (`newspeak-spec.pdf` §6.2). Walked with an explicit list rather
    /// than by recursing per level: the chain's length is an image's to
    /// choose, and a level with no initializer code pushes no activation, so
    /// neither depth bound would see a long chain of them
    /// (`docs/design/execution.md`, "The trampoline (1b)").
    ///
    /// Upward first: each level's factory arguments are checked against its
    /// declared arity, then its superclass clause's message is evaluated to
    /// the superclass's arguments (step 3's first half). Then downward, from
    /// the topmost level: each level's own slot initializers and `initExprs`
    /// run in an activation of their own, with that level's own arguments
    /// (steps 2, 4 and 5) -- one activation per initializing level, so a
    /// superclass's initializers see the arguments *its* clause supplied.
    /// A single activation threaded up the whole chain would let a
    /// superclass factory parameter read whatever happens to sit at the same
    /// index in the subclass's argument list -- or index past its end when
    /// there is none.
    fn run_initializer_levels(
        &mut self,
        class: ClassRef,
        instance: &Value,
        factory_args: Vec<Value>,
    ) -> Result<(), Signal> {
        let mut levels: Vec<(ClassRef, Vec<Value>)> = Vec::new();
        let mut level = class;
        let mut args = factory_args;
        loop {
            self.check_factory_arity(level, &args)?;
            let next = match self.heap.class_superclass(level) {
                Some(superclass) => {
                    let super_args =
                        self.eval_superclass_message(level, superclass, instance, &args)?;
                    Some((superclass, super_args))
                }
                None => None,
            };
            levels.push((level, args));
            match next {
                Some((superclass, super_args)) => {
                    level = superclass;
                    args = super_args;
                }
                None => break,
            }
        }
        for (level, args) in levels.into_iter().rev() {
            self.run_own_initializer(level, instance, args)?;
        }
        Ok(())
    }

    /// Rejects a factory-arity mismatch as a NewspeakError before this
    /// level's activation is pushed or any slot initializer / initExpr runs.
    fn check_factory_arity(&self, class: ClassRef, factory_args: &[Value]) -> Result<(), Signal> {
        // A slot initializer or
        // initExpr may reference a factory parameter as `Binding::Local(i)`,
        // resolved against this class's *declared* factory pattern -- but
        // the activation `run_own_initializer` pushes lays its registers out
        // from the arguments actually supplied, so a wrong count would shift
        // every temporary, and too few would leave `locals.rs`'s
        // `read_local`/`write_local` indexing past the end of its register
        // window.
        //
        // Checked at *every* level, not just the outermost. A level's
        // arguments come from its subclass's clause message, whose
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

    /// One level's own slot clauses and `initExprs` (steps 2, 4 and 5), on
    /// the instance its subclasses share, with the arguments its own
    /// subclass's clause supplied -- or none at all when the level declares
    /// nothing to initialize.
    fn run_own_initializer(
        &mut self,
        class: ClassRef,
        instance: &Value,
        factory_args: Vec<Value>,
    ) -> Result<(), Signal> {
        // Step 2, built here rather than above it: a level with no slots and
        // no `initExprs` runs no code at all -- such a mixin may carry no
        // initializer at all -- so its activation could never be observed
        // -- and every chain ends at a builtin, which is exactly such a
        // level. Building it only where something runs in it keeps one
        // activation per *initializing* level rather than one per level.
        //
        // Every field of it describes this one level: its arguments, its
        // class, and its `slot_base` -- the absolute index a compiled
        // `InitSlot`'s mixin-relative `own_index` is offset by. `slot_base`
        // is set here, with the rest of the level's identity, and never
        // changes: with an activation per level there is nothing to save
        // and restore, and no window in which one describes another level.
        if !class_has_initializer_code(&self.heap, class) {
            return Ok(());
        }
        let application = self
            .heap
            .class_application(class)
            .expect("internal: class_has_initializer_code confirmed this is a class object");
        let code = application.mixin.initializer_code.clone().unwrap_or_else(|| {
            panic!("internal: class needs an instance initializer and its mixin carries none (a mixin built from source always does; verify_mixins refuses a decoded one)")
        });
        // `InitSlot`'s `own_index` is relative to the mixin's own slots, not
        // an absolute field index, because the compiled body is shared by
        // every application of the mixin. This level's own
        // `MixinApplication.own_slot_base`, taken as the activation's
        // `slot_base`, turns it into an absolute index at run time.
        let slot_base = application.own_slot_base;
        let temps = (code.num_registers - code.num_args) as usize;

        // Steps 4-5: this class's own slot clauses (declaration order), then
        // its own initExprs, on an activation pushed with the body's whole
        // register file. The loop pops it on every path out.
        self.reenter(|interp| {
            interp.push(
                ActivationHeader {
                    receiver: *instance,
                    current_class: Some(class),
                    kind: FrameKind::Initializer(class),
                    slot_base,
                    code,
                    then: Continuation::Deliver,
                    counted: true,
                },
                &factory_args,
                temps,
            )
        })
        .map(|_| ())
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
        class: ClassRef,
        superclass: ClassRef,
        instance: &Value,
        factory_args: &[Value],
    ) -> Result<Vec<Value>, Signal> {
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
            // `Object new` and every other argument-free clause: nothing to
            // evaluate, no activation built, nothing allocated.
            return Ok(vec![]);
        }
        let code = application
            .as_ref()
            .and_then(|application| application.mixin.superclass_message_code.clone())
            .unwrap_or_else(|| {
                panic!("internal: mixin whose superclass clause message takes arguments carries no compiled form for it (a mixin built from source always does; verify_mixins refuses a decoded one)")
            });
        // The message is evaluated on the instance being initialized and
        // with *this* level's factory arguments -- §6.2's "in the context of
        // the instance initializer" -- through the same activation the
        // nested-class accessor runs a clause's receiver in.
        let value = super::nested::run_clause_body(
            self,
            instance,
            factory_args.to_vec(),
            class,
            FrameKind::SuperclassMessage(class),
            code,
        )?;
        argument_list(&self.heap, class, value)
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

/// Whether `mixin` declares any slot or `initExpr` at all.
///
/// `pub(crate)` for `image::decode::verify_mixins`, which must refuse
/// an image that has no compiled initializer for a mixin that needs one
/// rather than let `run_own_initializer` above panic on the invariant --
/// checked per mixin, since every mixin of the tree carries its own.
pub(crate) fn mixin_needs_initializer_code(mixin: &crate::mixin::Mixin) -> bool {
    !mixin.slots.is_empty() || mixin.has_init_exprs
}

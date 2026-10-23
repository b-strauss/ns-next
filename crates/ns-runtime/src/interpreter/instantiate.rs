//! Class instantiation: `newspeak-spec.pdf` §6.1, §6.3. See
//! `docs/design/object-model.md`, "Instantiation", for the six numbered
//! steps the factory body and the initializers' instructions decide here.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::hash::IdentityMap;
use crate::heap::{ActivationReference, ClassLevel, Heap, Root};
use crate::mixin::{
    slot_accessors, ClauseMessage, DeclarationText, Factory, MethodEntry, Mixin,
    MixinApplicationClause, MixinMethod, MixinSlot,
};
use crate::types::*;
use crate::Symbol;

use super::{
    HostError, HostResult, HostSignal, HostStart, Interpreter, Mutator, Signal, Step, StepArguments,
};

/// A class body's method table: its methods, the accessors its `slots`
/// define (`newspeak-spec.pdf` §6.3.2) and each lazy slot's getter
/// (§6.3.3, compiled like a method). Everything a lookup or
/// `Mutator::push_step` reads is lifted out of the declaration here, so that
/// a mixin decoded from an image -- which has no declaration at all --
/// answers the same questions from the same fields. No selector is declared
/// twice: the resolver has already refused a body that declares one member
/// twice (`resolver::classes::duplicate_member_names`).
///
/// `kernel_block` says `body` is the instance side of the kernel's `Closure`,
/// the one body whose methods may carry an unwind marker
/// (`compiler::unwind_marker`). `class_accessor_root` is the module root a
/// method may be a pure class accessor of (`MixinMethod::class_accessor`):
/// the root of a shipped module other than the kernel, and `None` for every
/// other body.
fn build_methods(
    body: &ast::ClassBody,
    slots: &[MixinSlot],
    bindings: &Rc<HashMap<Span, Binding>>,
    kernel: bool,
    kernel_block: bool,
    class_accessor_root: Option<&ast::ClassDeclaration>,
    lowering: crate::compiler::Lowering,
) -> IdentityMap<Symbol, MethodEntry> {
    let mut methods: IdentityMap<Symbol, MethodEntry> = slot_accessors(slots).into_iter().collect();
    for declaration in body.methods() {
        let mut method =
            MixinMethod::from_declaration(Rc::new(declaration.clone()), bindings, kernel, lowering);
        method.class_accessor = class_accessor_root.and_then(|root| {
            crate::compiler::primitive_method::recognize_class_accessor(declaration, bindings, root)
        });
        if kernel_block {
            if let Some(marker) =
                crate::compiler::unwind_marker::recognize(declaration, &method.code)
            {
                let mut code = (*method.code).clone();
                code.unwind_marker = Some(marker);
                method.code = Rc::new(code);
            }
        }
        let previous = methods.insert(method.selector, MethodEntry::Compiled(Rc::new(method)));
        debug_assert!(
            previous.is_none(),
            "the resolver refuses a duplicate member"
        );
    }
    // §6.3.3: a lazy slot's getter runs its initializer while the slot is
    // empty, so it is a compiled method rather than an accessor entry
    // (`slot_accessors` leaves it out).
    for (position, slot) in slots.iter().enumerate() {
        if !slot.lazy {
            continue;
        }
        let Some(declaration) = &slot.declaration else {
            continue;
        };
        let Some(initializer) = &declaration.initializer else {
            unreachable!("internal: the grammar gives every lazy slot an initializer");
        };
        let own_index = u32::try_from(position).expect("a mixin's slot count fits a u32");
        let code = crate::compiler::compile_lazy_slot_getter(
            initializer,
            own_index,
            declaration.span,
            bindings,
            lowering,
        );
        let getter = MixinMethod::lazy_slot_getter(slot, Rc::new(code));
        let previous = methods.insert(getter.selector, MethodEntry::Compiled(Rc::new(getter)));
        debug_assert!(
            previous.is_none(),
            "the resolver refuses a duplicate member"
        );
    }
    methods
}

fn build_slots(body: &ast::ClassBody) -> Vec<MixinSlot> {
    body.slots()
        .map(|slot| MixinSlot::from_declaration(Rc::new(slot.clone())))
        .collect()
}

/// The run-time-visible half of a superclass clause's or a `<:` clause's
/// message: §6.2 builds a `<:` clause from §6.1's `inheritanceClause`, so
/// both read the same way. The selector defaults to `new` for an omitted
/// message, per `newspeak-spec.pdf` §6.1 -- resolved once, here, so no
/// reader of the clause re-derives it from the declaration.
fn clause_message_of(clause: &ast::SuperclassClause) -> ClauseMessage {
    ClauseMessage {
        selector: super::nested::superclass_clause_message(clause)
            .map_or(Symbol::NEW, crate::compiler::lower::selector_of),
        has_arguments: super::nested::superclass_clause_message_arguments_expression(clause)
            .is_some(),
    }
}

/// One `<:` clause's run-time half.
fn mixin_application_clause_of(clause: &ast::SuperclassClause) -> MixinApplicationClause {
    MixinApplicationClause {
        message: clause_message_of(clause),
        receiver: Some(Symbol::intern(&clause.receiver.name)),
    }
}

/// The text `declaration` was read in, its nested declarations cut out
/// (`Mixin::text`): the whole of `module_text` for the module's root
/// `root` -- its prologue and trailing text are the module's too -- and
/// the declaration's span otherwise. A shipped module's mixins share the
/// module's text, which every boot of the thread holds once and no edit
/// replaces; a program's own a copy each, so that no version an edit
/// supersedes keeps another's text alive.
fn declaration_text(
    declaration: &ast::ClassDeclaration,
    root: &ast::ClassDeclaration,
    source: ModuleSource,
    module_text: &Rc<str>,
) -> Option<Rc<DeclarationText>> {
    let range = if std::ptr::eq(declaration, root) {
        Span::new(0, u32::try_from(module_text.len()).ok()?)
    } else {
        declaration.span
    };
    let holes: Vec<Span> = declaration
        .instance_side
        .nested_classes()
        .map(|nested| nested.span)
        .collect();
    let text = if source.is_shipped() {
        DeclarationText::shared(Rc::clone(module_text), range, &holes)
    } else {
        DeclarationText::cut(module_text, range, &holes)
    };
    debug_assert!(
        text.is_some(),
        "internal: a parsed declaration's nested declarations lie within it, in order"
    );
    text.map(Rc::new)
}

impl Interpreter {
    /// Materializes the module's root `ClassDeclaration` into a real class
    /// object. The superclass clause's *receiver* is resolved here, once -- it
    /// is what fixes the superclass (`newspeak-spec.pdf` §6.1). Its trailing
    /// *message* is deliberately not touched here: §6.2 defers it to every
    /// instantiation, where the class's initializer evaluates it
    /// (`decide_superclass_initializer`).
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
            mutator.stash(Value::object(class))
        })
    }

    /// The module's root declaration with every body compiled onto it,
    /// exactly as `load_module` compiles it, but applied to nothing: no class
    /// object is built, so a caller after the code alone holds no handle and
    /// needs no heap to read it.
    pub fn compile_module(&mut self, resolved: &ResolvedModule) -> Rc<Mixin> {
        self.enter(|mutator| mutator.compile_root_mixin(resolved))
    }

    /// Instantiates the class `class` holds, from a host: the six numbered
    /// steps in `docs/design/object-model.md`, "Instantiation", run by a
    /// factory activation (`decide_instantiation`) from the outside
    /// (`run_from_host`), like a host's `send`. A root that holds anything
    /// but a class object is refused as a Newspeak error, never a panic:
    /// the caller is a host.
    pub fn instantiate(&mut self, class: &Root, factory_arguments: &[Root]) -> HostResult {
        let depth = self.world.activation_depth();
        let start = self.enter(|mutator| {
            let value = mutator.fetch(class);
            let class = match value.as_object() {
                Some(object) if mutator.heap.is_class(object) => object,
                _ => {
                    return HostStart::Finished(Err(HostSignal::Error(HostError {
                        selector: Symbol::NEW,
                        receiver: mutator.stash(value),
                        message: format!(
                            "cannot instantiate {}: it is not a class",
                            mutator.heap.describe(value)
                        ),
                    })))
                }
            };
            let factory_arguments: Vec<Value> = factory_arguments
                .iter()
                .map(|argument| mutator.fetch(argument))
                .collect();
            let decided = mutator.decide_instantiation(class, factory_arguments.len());
            mutator.start(Ok(decided), &factory_arguments)
        });
        self.run_from_host(start, depth)
    }
}

/// What a module's source is, for `Mutator::build_mixin`: a program's,
/// shipped core-library source (`ResolvedModule.kernel`), or `Kernel.ns`
/// itself, which is shipped too.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModuleSource {
    User,
    CoreLibrary,
    Kernel,
}

impl ModuleSource {
    pub(crate) fn is_shipped(self) -> bool {
        self != ModuleSource::User
    }
}

impl<'gc> Mutator<'gc> {
    /// `load_module`'s work, inside its entry: the root class, built and
    /// compiled, for `load_module` to root.
    pub(crate) fn load_root_class(&mut self, resolved: &ResolvedModule) -> ClassReference<'gc> {
        let mixin = self.compile_root_mixin(resolved);
        self.root_class_of_mixin(&mixin)
    }

    /// The root class of a finished root mixin, however the mixin came to
    /// be: compiled from source here, or (for the shipped modules) handed
    /// in by the boot.
    pub(crate) fn root_class_of_mixin(&mut self, mixin: &Rc<Mixin>) -> ClassReference<'gc> {
        // Whatever the spelling, the rule leaves three class shapes to build:
        // over `Object`, over `Value`, or a `Value` level over `Object`.
        // `resolve` refuses every other root, so a defect reaches here only
        // through a `ResolvedModule` a host built by hand; such a root is built
        // over `Object` so that its class exists and can be inspected, and only
        // an instance is refused (`apply_mixin` carries the defect into the
        // application).
        let shape = mixin.root_shape;
        // The mixin's verdict was read from the same declaration.
        debug_assert_eq!(shape.is_none(), mixin.root_clause_defect.is_some());
        let (superclass_kind, applies_value) = match shape {
            Some(shape) if shape.over_value => (BuiltinClass::Value, shape.applies_value),
            Some(shape) => (BuiltinClass::Object, shape.applies_value),
            None => (BuiltinClass::Object, false),
        };
        let builtin = self.builtin_class(superclass_kind);
        let superclass = self
            .heap
            .class_application(builtin)
            .map(|application| (builtin, application));
        let applied: Vec<(usize, Value<'gc>)> = if applies_value {
            vec![(0, Value::object(self.builtin_class(BuiltinClass::Value)))]
        } else {
            Vec::new()
        };
        let top = match super::nested::build_applied_levels(self, mixin, superclass, &applied) {
            Ok(top) => top,
            Err(_) => panic!("internal: every builtin class is a class object"),
        };
        // The slot layout is the application's job, not the class object's:
        // `apply_mixin` offsets this class's own slots past every inherited
        // one (`own_slot_base`), so a class object never computes -- or can
        // disagree about -- an instance's shape.
        // A body-less root is the level its `<:` clause added (decision 8
        // of `new` as compiled code), named for it and with its factory.
        if let (false, Some((top, _))) = (mixin.has_body, &top) {
            if !applied.is_empty() {
                return *top;
            }
        }
        let (superclass, super_application) = top.unzip();
        let application = self.apply_mixin(mixin, super_application.as_ref());
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
    /// `Mutator::attach_metaclass` builds the metaclass that applies it.
    ///
    /// `source` says what the module is. Whether it is shipped
    /// (`ResolvedModule.kernel`) is carried down to every method so that
    /// only a shipped method can carry a fast path
    /// (`MixinMethod::from_declaration`), and recorded on every mixin of the
    /// tree, class sides included, as `Mixin.shipped`; whether it is
    /// `Kernel.ns` itself decides which `Closure` may carry unwind markers,
    /// and whether it is another shipped module which methods are
    /// recognized as class accessors of `root`, the module's root
    /// declaration (`MixinMethod::class_accessor`). `module_text` is the
    /// module's text, from which each mixin of the tree takes its
    /// declaration's (`Mixin::text`, `declaration_text`); each mixin's
    /// nested ones and its class side are linked to it once it is shared
    /// (`Mixin::enclosing`).
    pub(crate) fn build_mixin(
        &mut self,
        declaration: &ast::ClassDeclaration,
        root: &ast::ClassDeclaration,
        bindings: &Rc<HashMap<Span, Binding>>,
        source: ModuleSource,
        module_text: &Rc<str>,
    ) -> Rc<Mixin> {
        let mixin = Rc::new(self.mixin_of(declaration, root, bindings, source, module_text));
        Mixin::set_enclosing_of_children(&mixin);
        mixin
    }

    /// A module's root mixin, every body compiled, for `load_root_class` and
    /// `Interpreter::compile_module`.
    pub(super) fn compile_root_mixin(&mut self, resolved: &ResolvedModule) -> Rc<Mixin> {
        let source = if resolved.kernel {
            ModuleSource::CoreLibrary
        } else {
            ModuleSource::User
        };
        self.build_root_mixin(
            &resolved.module.root_class,
            &resolved.bindings,
            source,
            &resolved.source,
        )
    }

    /// A module's root mixin: `build_mixin`'s, with the root rule's verdict
    /// recorded on it for `apply_mixin` and the encoder to read.
    /// `module_text` is the module's text, `ResolvedModule::source`.
    pub(crate) fn build_root_mixin(
        &mut self,
        declaration: &ast::ClassDeclaration,
        bindings: &Rc<HashMap<Span, Binding>>,
        source: ModuleSource,
        module_text: &Rc<str>,
    ) -> Rc<Mixin> {
        let mut mixin = self.mixin_of(declaration, declaration, bindings, source, module_text);
        mixin.root_clause_defect =
            crate::resolver::root_clause_defect(declaration).map(|(defect, _)| defect);
        mixin.root_shape = crate::resolver::root_shape(declaration);
        let mixin = Rc::new(mixin);
        Mixin::set_enclosing_of_children(&mixin);
        mixin
    }

    /// `build_mixin`'s work, before the mixin is shared.
    fn mixin_of(
        &mut self,
        declaration: &ast::ClassDeclaration,
        root: &ast::ClassDeclaration,
        bindings: &Rc<HashMap<Span, Binding>>,
        source: ModuleSource,
        module_text: &Rc<str>,
    ) -> Mixin {
        let kernel = source.is_shipped();
        let slots = build_slots(&declaration.instance_side);
        // `Kernel.ns`'s own `Closure`, not a class of that name in another
        // shipped module: an unwind marker makes the unwinder trust the
        // method's registers (`compiler::unwind_marker`).
        let kernel_block = source == ModuleSource::Kernel
            && declaration.name.name
                == super::kernel::kernel_class_name(crate::types::BuiltinClass::Closure);
        // Only a shipped module other than the kernel hands out its classes
        // by class accessors: `Kernel.ns`'s own `^outer Kernel <Name>` on
        // `Object` answers a builtin class, which an image names by its tag.
        let class_accessor_root = (source == ModuleSource::CoreLibrary).then_some(root);
        let methods = build_methods(
            &declaration.instance_side,
            &slots,
            bindings,
            kernel,
            kernel_block,
            class_accessor_root,
            self.lowering,
        );
        let nested = declaration
            .instance_side
            .nested_classes()
            .map(|nested| self.build_mixin(nested, root, bindings, source, module_text))
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
        let text = declaration_text(declaration, root, source, module_text);
        let class_side = declaration
            .class_side
            .as_ref()
            .map(|side| self.build_class_side(side, factory, bindings, kernel, text.clone()));
        let mut mixin = Mixin {
            name: Symbol::intern(&declaration.name.name),
            access: crate::dispatch::effective_access(declaration.access),
            methods,
            slots,
            factory: Some(factory),
            superclass: declaration.superclass.as_ref().map(clause_message_of),
            superclass_receiver: declaration
                .superclass
                .as_ref()
                .map(|clause| Symbol::intern(&clause.receiver.name)),
            mixin_applications: declaration
                .mixin_applications
                .iter()
                .map(mixin_application_clause_of)
                .collect(),
            // §6's grammar: a `mixinAppSuffix` ends in a dot or a class
            // body, and only the dot leaves the members undelimited.
            has_body: declaration.mixin_applications.is_empty()
                || declaration.instance_side.members_delimiters.is_some(),
            has_initializer_expressions: !declaration
                .instance_side
                .initializer_expressions
                .is_empty(),
            // This class's own initExprs, lowered into the instance
            // initializer by `compiler::compile_mixin_initializer` (step 5 of
            // the six in `docs/design/object-model.md`, "Instantiation").
            initializer_expressions: declaration.instance_side.initializer_expressions.clone(),
            nested_declarations: crate::mixin::NestedDeclarations::new(nested),
            class_side,
            span: declaration.span,
            declaration: Some(Rc::new(declaration.clone())),
            initializer_code: None,
            own_initializer_code: None,
            standalone_initializer_code: None,
            superclass_clause_code: None,
            root_clause_defect: None,
            root_shape: None,
            shipped: kernel,
            // Set by the caller once the mixin is shared.
            enclosing: std::cell::OnceCell::new(),
            text,
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
    /// single declaration node to keep as provenance for it. `text` is its
    /// instance side's, whose declaration holds the class side's text.
    fn build_class_side(
        &mut self,
        side: &ast::ClassBody,
        factory: Factory,
        bindings: &Rc<HashMap<Span, Binding>>,
        kernel: bool,
        text: Option<Rc<DeclarationText>>,
    ) -> Rc<Mixin> {
        let slots = build_slots(side);
        let mut methods = build_methods(side, &slots, bindings, kernel, false, None, self.lowering);
        let (selector, entry) = crate::mixin::factory_entry(Some(factory));
        methods.insert(selector, entry);
        let mut mixin = Mixin {
            methods,
            slots,
            has_initializer_expressions: !side.initializer_expressions.is_empty(),
            initializer_expressions: side.initializer_expressions.clone(),
            span: side.span,
            shipped: kernel,
            text,
            ..Mixin::empty(Symbol::CLASS_SIDE)
        };
        crate::compiler::compile_mixin_bodies(&mut mixin, bindings, self.lowering);
        Rc::new(mixin)
    }

    /// §6.3's instantiation, the action of a factory entry: the entry's
    /// body, activated on `class` -- the receiver, a class object -- with
    /// the send's arguments. The body allocates the instance
    /// (`allocate_for_factory`), runs `class`'s instance initializer on it
    /// (`decide_instance_initializer`; `newspeak-spec.pdf` §6.3: "the
    /// instance initializer is invoked on an instance *o* of the class")
    /// and answers it. `code` is the body the lookup found in the entry, or
    /// a cache hit recorded from it.
    ///
    /// No re-check of what the lookup decided: the entry was found on the
    /// receiver's own metaclass (`resolve::resolved_for` refuses one found
    /// anywhere else), whose instances id is the guard a cache entry is
    /// keyed by, so a hit and a miss reach the same class and the same
    /// entry (§3.2 makes a metaclass flat, so there is no inherited class
    /// side an entry could have come from) -- which is why a cache entry may
    /// record the body. `class` is the receiver itself, read per send rather
    /// than carried in an entry; the class's builtin, which decides how the
    /// instance is allocated, is read by the body's allocation, also per
    /// send.
    ///
    /// No double-dispatch risk against the superclass clause's own factory
    /// message (`Object new` in `class Foo = Object new (...)`). That
    /// message is *checked* and its arguments evaluated by the subclass's
    /// initializer (`InitializeSuperclass`), but never *sent*: §6.2 has the
    /// subclass's initializer invoke the superclass's instance initializer
    /// directly on the same object (`decide_superclass_initializer`), so it
    /// never reaches here, which would allocate a second, discarded
    /// instance per level.
    pub(super) fn decide_factory(
        class: ClassReference<'gc>,
        code: Rc<crate::bytecode::CompiledCode>,
    ) -> Step<'gc> {
        Step::Factory {
            class,
            code,
            arguments: StepArguments::Send,
        }
    }

    /// The factory activation a host's `instantiate` runs on `class` with
    /// `argument_count` arguments, which the step takes from the send's.
    /// Its body is `compiler::factory_body` of that count, the body every
    /// factory entry of that arity holds, so that a class object whose
    /// metaclass holds none -- one a host built by hand -- instantiates
    /// too, and the factory's arity is checked where a send's is, by
    /// `decide_instance_initializer`, after the allocation's refusals.
    pub(crate) fn decide_instantiation(
        &self,
        class: ClassReference<'gc>,
        argument_count: usize,
    ) -> Step<'gc> {
        let code = crate::compiler::factory_body(u16::try_from(argument_count).unwrap_or(u16::MAX));
        Step::Factory {
            class,
            code: Rc::new(code),
            arguments: StepArguments::Send,
        }
    }

    /// The factory activation `factory`, just pushed for `class` with the
    /// send's `arguments`, carried through its body's first two
    /// instructions (`compiler::factory_body`) here rather than by the run
    /// loop, which would run them next with nothing in between: the
    /// instance allocated into its register, and the initializer that
    /// `InitializeInstance` decides pushed above it -- or, for a class
    /// whose levels run no code, its answer written -- with the factory's
    /// program counter at `InitializeInstance`, as that send stores it.
    /// Answers the activation to run and where: the initializer from its
    /// start, or the factory at its `Return`.
    ///
    /// The run loop's order of errors is kept by not taking this path
    /// whenever one is raised: the initializer is decided first, which
    /// changes nothing, and only a decision that raises nothing is
    /// allocated for; an allocation that refuses allocates nothing. Either
    /// way the factory is answered from its start, and the loop runs the
    /// body, raising the allocation's refusal before the decision's error,
    /// from the factory's activation. An initializer whose push the depth
    /// limit refuses raises from the factory's activation, as
    /// `InitializeInstance`'s send does. What is skipped is the switch to
    /// the factory before its first instruction, a safepoint at which no
    /// program code runs on either side.
    pub(crate) fn enter_factory_body(
        &mut self,
        factory: ActivationReference,
        class: ClassReference<'gc>,
        registers: FactoryRegisters,
        arguments: &[Value<'gc>],
    ) -> Result<(ActivationReference, usize), Signal<'gc>> {
        let decided = self.decide_instance_initializer(class, Value::nil(), arguments);
        // Read where the decision left it rather than moved out whole: a
        // move read back in sixteen-byte pieces what the decision had just
        // written in smaller ones, a store-forwarding stall.
        let runs_code = match &decided {
            Ok(Step::Initializer { .. }) => true,
            Ok(Step::Value(_)) => false,
            _ => return Ok((factory, 0)),
        };
        let Ok(instance) = self.allocate_for_factory(class) else {
            return Ok((factory, 0));
        };
        let register_base = self.heap.activation_register_base(factory);
        self.heap
            .set_register(register_base, registers.instance, instance);
        self.heap
            .activation_set_program_counter(factory, FACTORY_INITIALIZE);
        let Ok(Step::Initializer {
            level,
            arguments: initializer_arguments,
            body,
            code,
            ..
        }) = &decided
        else {
            debug_assert!(!runs_code);
            // `InitializeInstance` answered the instance: written to its
            // destination, and the factory carries on at its `Return`.
            self.heap
                .set_register(register_base, registers.answer, instance);
            return Ok((factory, FACTORY_INITIALIZE + 1));
        };
        let initializer_arguments = initializer_arguments.slice(arguments);
        let pushed = self.push_initializer(
            instance,
            *level,
            initializer_arguments,
            *body,
            Rc::clone(code),
        )?;
        Ok((pushed, 0))
    }

    /// `AllocateInstance`'s work: a fresh instance of the class object
    /// `activation` runs on, a factory body's receiver. A receiver that is
    /// no class object -- only a crafted image can put the instruction
    /// anywhere but a factory body -- is refused as `instantiate` refuses
    /// one.
    pub(crate) fn allocate_instance(
        &mut self,
        activation: ActivationReference,
    ) -> Result<Value<'gc>, Signal<'gc>> {
        let receiver = self.heap.activation_receiver(activation);
        let class = self.class_receiver(receiver)?;
        self.allocate_for_factory(class)
    }

    /// The class object `receiver` is, for an instruction that runs on
    /// one; anything else is refused with `instantiate`'s text.
    fn class_receiver(&self, receiver: Value<'gc>) -> Result<ClassReference<'gc>, Signal<'gc>> {
        match receiver.as_object() {
            Some(object) if self.heap.is_class(object) => Ok(object),
            _ => Err(Signal::Error(NewspeakError::error(
                Symbol::NEW,
                receiver,
                format!(
                    "cannot instantiate {}: it is not a class",
                    self.heap.describe(receiver)
                ),
            ))),
        }
    }

    /// Step 1 of `docs/design/object-model.md`, "Instantiation": one
    /// instance of `class`, every slot nil, after every check that refuses
    /// such an instance -- the builtins whose instances are made otherwise
    /// or not at all, the application's refusal and the class object's
    /// value refusal, in that order. Nothing of the instance's initializer
    /// runs here.
    pub(crate) fn allocate_for_factory(
        &mut self,
        class: ClassReference<'gc>,
    ) -> Result<Value<'gc>, Signal<'gc>> {
        match self.heap.class_builtin(class) {
            // The VM mirror is a capability: there is exactly one, and it is
            // the bootstrap's to hand out (`docs/design/core-library.md`, "The
            // VM mirror is the whole Rust boundary").
            // Its class is no value the refusal may carry: the error would
            // hand a program the class and with it the capability.
            Some(BuiltinClass::VmMirror) => Err(Signal::Error(NewspeakError::cannot_instantiate(
                Symbol::NEW,
                Value::object(class),
                Value::nil(),
                "the VM mirror's class cannot be instantiated",
            ))),
            // `Array`'s shape is `ObjectShape::Array`, not an ordinary
            // instance's -- `builtins::collections`' primitives read the
            // array accessors directly, so handing it a generic instance
            // payload would leave every native method on the result silently
            // unable to find its data (falling straight through to
            // `doesNotUnderstand:` on the very next send). Built directly
            // with `Heap::allocate_array`, tagged with `class` itself (the
            // receiver's own class -- *not* `Mutator::new_array`, which
            // unconditionally tags its result `BuiltinClass::Array`, which
            // would be wrong for a user subclass of `Array`).
            Some(BuiltinClass::Array) => Ok(Value::object(self.heap.allocate_array(class, &[]))),
            // An empty `AlignedBytes` payload, as `Array`'s above: a slotless
            // instance would hold no bytes for the leaves to find.
            Some(BuiltinClass::ByteArray) => Ok(Value::object(
                self.heap.allocate_zeroed_byte_array(class, 0),
            )),
            // A Symbol is canonical, one object per text (§5.1.6), made only
            // by a literal or `asSymbol`; `new` would mint a second one.
            Some(BuiltinClass::Symbol) => Err(Signal::Error(NewspeakError::cannot_instantiate(
                Symbol::NEW,
                Value::object(class),
                Value::object(class),
                "cannot instantiate Symbol: a Symbol is made only by a literal or by asSymbol",
            ))),
            // A reified mixin is canonical too, one object per mixin, made
            // only by `Mutator::reified_mixin`; `new` would mint one no
            // mixin stands behind. The class is private to the kernel, so
            // the refusal carries nil rather than hand it out.
            Some(BuiltinClass::Mixin) => Err(Signal::Error(NewspeakError::cannot_instantiate(
                Symbol::NEW,
                Value::object(class),
                Value::nil(),
                "cannot instantiate Mixin: a mixin's object is made only by the runtime",
            ))),
            // A flat String payload, as `Array`'s above: a slotless instance
            // would answer none of `size`, `at:` or `,`.
            Some(BuiltinClass::String) => Ok(Value::object(
                self.new_string_from_code_points(crate::CodePoints::from("")),
            )),
            // The `Ephemeron` format, so that the collectors treat the first
            // field as the key; asked of the class's own builtin, so a user
            // subclass takes the arm below and holds strongly
            // (`docs/design/core-library.md`, "A user subclass gets no
            // payload").
            Some(BuiltinClass::Ephemeron) => self
                .allocate_refusing(class, |heap, class, slot_count, _| {
                    heap.allocate_ephemeron(class, slot_count)
                }),
            _ => self.allocate_refusing(class, |heap, class, slot_count, header| match header {
                Some(header) => heap.allocate_instance_with(class, header, slot_count),
                None => heap.allocate_instance(class, slot_count),
            }),
        }
    }

    /// `allocate_for_factory` for every class whose instance is an
    /// ordinary one or an ephemeron, whose format alone sets it apart: the
    /// application's refusals, then the fields `allocate` makes from the
    /// application's slot count and header. Generic rather than a function
    /// pointer, so that every allocation stays a direct call.
    fn allocate_refusing(
        &mut self,
        class: ClassReference<'gc>,
        allocate: impl FnOnce(
            &mut Heap<'gc>,
            ClassReference<'gc>,
            usize,
            Option<crate::heap::InstanceHeader>,
        ) -> ObjectReference<'gc>,
    ) -> Result<Value<'gc>, Signal<'gc>> {
        // Borrowed, not cloned: the facts the allocation needs are copied
        // out before the heap is written.
        let application = self.heap.class_level(class).application();
        // Asked before anything is allocated: a class whose instances could
        // never be what §3.1.1 or the root rule demands has no instance to
        // make. The answer was computed once, when the application was built
        // (`Mutator::apply_mixin`).
        if let Some(application) = application {
            if let Some(refusal) = &application.instantiation_refusal {
                let display_name = self.heap.class_display_name(class);
                return Err(Signal::Error(instantiation_refusal_error(
                    class,
                    &display_name,
                    Instantiated::of(&self.heap, class, application),
                    refusal,
                )));
            }
            // §3.1.1's other class-level condition, every enclosing object a
            // value (`interpreter::value_class::check_value_class`) --
            // decided when `class` was created and only recorded then
            // (`Mutator::record_value_class_refusal`); refused here, not there,
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
        // The application already knows the whole layout, inherited slots
        // included -- summing the chain here again would be a second answer
        // to the same question.
        let slot_count = application.map_or(0, |application| application.slot_count);
        let header = application.and_then(|application| application.instance_header);
        Ok(Value::object(allocate(
            &mut self.heap,
            class,
            slot_count,
            header,
        )))
    }

    /// `InitializeInstance`'s decision: the initializer of the class object
    /// `activation` runs on, a factory body's receiver, on `instance` with
    /// the factory's `arguments`.
    pub(crate) fn initialize_instance(
        &mut self,
        activation: ActivationReference,
        instance: Value<'gc>,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let receiver = self.heap.activation_receiver(activation);
        let class = self.class_receiver(receiver)?;
        self.decide_instance_initializer(class, instance, arguments)
    }

    /// Steps 2-5 of `docs/design/object-model.md`, "Instantiation", for
    /// `class` instantiated with `arguments`: the factory's arity checked,
    /// then `class`'s own initializer on `instance` (`level_initializer_of`),
    /// which runs the rest. A class whose level runs no code is not entered
    /// (`enter_or_pass`).
    pub(crate) fn decide_instance_initializer(
        &mut self,
        class: ClassReference<'gc>,
        instance: Value<'gc>,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let level = self.heap.class_level(class);
        check_arity(class, declared_factory_arity(level), arguments)?;
        self.enter_or_pass(class, level, instance)
    }

    /// `InitializeSuperclass`'s decision, step 3 of the running
    /// initializer: the class `steps` above the activation's class -- the
    /// class its superclass clause names, past the levels its own `<:`
    /// clauses added -- initialized on `self` with the clause's evaluated
    /// `arguments`, after §6.1's check that the clause's `selector` is that
    /// class's primary factory and the arity check against it. The applying
    /// class, whose clause it is, is the activation's class.
    ///
    /// A chain that ends before `steps` levels -- `Object`'s top, or a class
    /// object a host built without a superclass -- has nothing above to
    /// initialize, and answers the instance.
    pub(crate) fn decide_superclass_initializer(
        &mut self,
        activation: ActivationReference,
        steps: u16,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let (instance, applying) = self.heap.activation_receiver_and_current_class(activation);
        let Some(applying) = applying else {
            return Ok(Step::Value(instance));
        };
        let Some(target) = self.level_above(applying, steps) else {
            return Ok(Step::Value(instance));
        };
        // §6.3 evaluates the message "in the context of *i*" and only then
        // invokes the superclass's initializer, so the clause's arguments
        // have run by now, and their effects precede §6.1's error.
        let expected = primary_factory_selector(&self.heap, target);
        if Some(selector) != expected {
            return Err(not_the_primary_factory_error(
                &self.heap,
                ClauseKind::Superclass,
                applying,
                Clause {
                    receiver: target,
                    named: target,
                },
                selector,
                expected,
            ));
        }
        let level = self.heap.class_level(target);
        check_arity(target, declared_factory_arity(level), arguments)?;
        self.enter_or_pass(target, level, instance)
    }

    /// `CheckAppliedClause`'s work, step 1 of the running initializer: §6.1's
    /// check of a `<:` clause, before anything runs. §6.2 gives the level the
    /// clause adds "a primary factory method with the same selector as" the
    /// clause's message, and the named class's initializer takes its
    /// arguments, so `selector` is checked against the primary factory of
    /// that level, `steps` above the activation's class.
    pub(crate) fn check_applied_clause(
        &self,
        activation: ActivationReference,
        steps: u16,
        selector: Symbol,
    ) -> Result<(), Signal<'gc>> {
        let (receiver, applying) = self.heap.activation_receiver_and_current_class(activation);
        let (applying, level) = self.applied_level(receiver, applying, steps)?;
        self.check_clause_names_the_factory(applying, level, selector)
    }

    /// §6.1's check of the `<:` clause that added `level`, written in the
    /// declaration of `applying` with `selector`: against the primary
    /// factory of the class the clause named (`Heap::class_named`), which
    /// is the level's own except at a body-less declaration's class object,
    /// whose factory is its declaration's.
    fn check_clause_names_the_factory(
        &self,
        applying: ClassReference<'gc>,
        level: ClassReference<'gc>,
        selector: Symbol,
    ) -> Result<(), Signal<'gc>> {
        let named = self.heap.class_named(level).unwrap_or(level);
        let expected = primary_factory_selector(&self.heap, named);
        if Some(selector) != expected {
            return Err(not_the_primary_factory_error(
                &self.heap,
                ClauseKind::MixinApplication,
                applying,
                Clause {
                    receiver: level,
                    named,
                },
                selector,
                expected,
            ));
        }
        Ok(())
    }

    /// `InitializeAppliedLevel`'s decision, step 4 of the running
    /// initializer for one `<:` clause: the own part of the mixin of the
    /// level the clause added, `steps` above the activation's class, on
    /// `self` with the clause's evaluated `arguments` -- its slot clauses
    /// and `initExprs`, never its own superclass or `<:` clauses (§6.2: "m
    /// is evaluated to μm", and the named class's initializer runs "with
    /// the arguments of μm"). A mixin with no own code is not entered.
    pub(crate) fn decide_applied_level_initializer(
        &mut self,
        activation: ActivationReference,
        steps: u16,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let (instance, applying) = self.heap.activation_receiver_and_current_class(activation);
        let (_, level) = self.applied_level(instance, applying, steps)?;
        // Against the factory of the mixin whose own part runs, not the
        // named class's: the body's registers are laid out for it, and the
        // two differ for a body-less named class (decision 8).
        let own_part_arity = self
            .heap
            .class_level_facts(level)
            .and_then(|facts| facts.factory_arity);
        check_arity(level, own_part_arity, arguments)?;
        let body = InitializerBody::OwnPart;
        let code = self
            .heap
            .class_level(level)
            .application()
            .filter(|application| mixin_needs_initializer_code(&application.mixin))
            .and_then(|application| body.code_of(&application.mixin));
        let Some(code) = code else {
            return Ok(Step::Value(instance));
        };
        Ok(Step::Initializer {
            receiver: instance,
            level,
            arguments: StepArguments::Send,
            body,
            code,
        })
    }

    /// The applying class -- the activation's -- and the level `steps`
    /// above it that one of its `<:` clauses added. Only a crafted image
    /// can run the instructions that ask in a body whose class has no such
    /// level, and that is refused as an ordinary error.
    fn applied_level(
        &self,
        receiver: Value<'gc>,
        applying: Option<ClassReference<'gc>>,
        steps: u16,
    ) -> Result<(ClassReference<'gc>, ClassReference<'gc>), Signal<'gc>> {
        let found = applying.and_then(|applying| {
            self.level_above(applying, steps)
                .filter(|level| self.heap.class_is_applied_level(*level))
                .map(|level| (applying, level))
        });
        found.ok_or_else(|| {
            let name = applying.map_or(Symbol::AN_OBJECT, |applying| {
                class_name(&self.heap, applying)
            });
            Signal::Error(NewspeakError::error(
                Symbol::NEW,
                receiver,
                format!(
                    "the initializer of class {name} names a level a mixin application added \
                     {steps} above it, and its class has none there"
                ),
            ))
        })
    }

    /// The class object `steps` superclasses above `class`, if the chain
    /// is that long.
    fn level_above(&self, class: ClassReference<'gc>, steps: u16) -> Option<ClassReference<'gc>> {
        let mut level = class;
        for _ in 0..steps {
            level = self.heap.class_superclass(level)?;
        }
        Some(level)
    }

    /// `level`'s initializer on `instance` with the arguments of the send
    /// that decided it -- whose arity the caller has checked -- or, while
    /// `level`'s full body would run nothing, the levels above it, in a loop
    /// rather than by recursing, so that a chain of levels without code
    /// neither nests nor escapes the depth limit's count
    /// (`docs/design/execution.md`, "The depth limit").
    ///
    /// A level is passed when it is its declaration's own class (`Pass`),
    /// its declaration needs no initializer code, none of its clauses
    /// passes arguments, and every level its own `<:` clauses added is one
    /// whose mixin has no own code and whose factory takes no arguments:
    /// then its full body only checks -- each `<:` clause against the class
    /// it named, nearest level first, then the superclass clause against
    /// the class above them -- and this makes the same checks in the same
    /// order with the same texts, and carries on at that class, past the
    /// declaration's own applied levels, with no arguments. A level a `<:`
    /// clause added and reached here -- from outside its declaration
    /// (decision 4 of `new` as compiled code) -- is always entered. The
    /// chain's top answers the instance.
    fn enter_or_pass(
        &self,
        level: ClassReference<'gc>,
        view: ClassLevel<'_, 'gc>,
        instance: Value<'gc>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let (mut level, mut view) = (level, view);
        let mut arguments = StepArguments::Send;
        loop {
            let Some(pass) = self.passes(level, view) else {
                return Ok(match level_initializer_of(view) {
                    Some((code, body)) => Step::Initializer {
                        receiver: instance,
                        level,
                        arguments,
                        body,
                        code,
                    },
                    // A mixin no compiler filled -- one a host built by
                    // hand -- has no body to run; an image's carries every
                    // one (`image::decode::verify_mixins`).
                    None => Step::Value(instance),
                });
            };
            self.check_applied_clauses(level, view, pass)?;
            // `applied_levels_run_nothing` has held the count below the
            // steps' ceiling.
            let steps = u16::try_from(pass.applied_count + pass.own_level).unwrap_or(u16::MAX);
            let above = match steps {
                1 => view.superclass(),
                _ => self.level_above(level, steps),
            };
            let Some(superclass) = above else {
                return Ok(Step::Value(instance));
            };
            let expected = primary_factory_selector(&self.heap, superclass);
            if Some(pass.clause_selector) != expected {
                return Err(not_the_primary_factory_error(
                    &self.heap,
                    ClauseKind::Superclass,
                    level,
                    Clause {
                        receiver: superclass,
                        named: superclass,
                    },
                    pass.clause_selector,
                    expected,
                ));
            }
            let superclass_view = self.heap.class_level(superclass);
            check_arity(superclass, declared_factory_arity(superclass_view), &[])?;
            level = superclass;
            view = superclass_view;
            arguments = StepArguments::Owned(Vec::new());
        }
    }

    /// Whether `level`'s full body would only check, and so is passed
    /// (`enter_or_pass`), with what the pass checks; `None` when it is
    /// entered. `level` is its declaration's own class: a level no `<:`
    /// clause added, whose declaration is its mixin's and whose clauses'
    /// levels start one above it, or a body-less declaration's class
    /// object, the level its last clause added, from which its clauses'
    /// levels are counted (decision 8 of `new` as compiled code). Any other
    /// level a clause added is entered.
    fn passes(&self, level: ClassReference<'gc>, view: ClassLevel<'_, 'gc>) -> Option<Pass> {
        let (facts, own_level) = match view.body_less_declaration() {
            Some(declaration) => (Some(crate::mixin::LevelFacts::of(declaration)), 0),
            None if view.is_applied_level() => return None,
            None => (view.level_facts(), 1),
        };
        let applied_count = facts.map_or(0, |facts| facts.applied_clause_count);
        let runs_nothing = facts.is_none_or(|facts| {
            !facts.needs_initializer_code
                && !facts.clause_has_arguments
                && !facts.applied_clauses_pass_arguments
        }) && self.applied_levels_run_nothing(level, applied_count, own_level);
        runs_nothing.then_some(Pass {
            applied_count,
            own_level,
            // §6.1: "if the message is omitted, it defaults to `new`" --
            // already resolved into the facts when the mixin was built,
            // and read from there rather than from the declaration: a
            // mixin decoded from an image has none.
            clause_selector: facts.map_or(Symbol::NEW, |facts| facts.clause_selector),
        })
    }

    /// Whether the `count` levels the `<:` clauses of `level`'s declaration
    /// added, from `own_level` steps above `level` up, would each run
    /// nothing when its full body initializes them: each is a level a
    /// clause added, its mixin needs no own code and its factory takes the
    /// no arguments its clause passes. A count past what an initializer's
    /// steps address is never passed: its body is the too-large one, which
    /// raises when it runs.
    fn applied_levels_run_nothing(
        &self,
        level: ClassReference<'gc>,
        count: usize,
        own_level: usize,
    ) -> bool {
        if count >= usize::from(u16::MAX) {
            return false;
        }
        if count == 0 {
            return true;
        }
        let mut current = Some(level);
        for _ in 0..own_level {
            current = current.and_then(|class| self.heap.class_superclass(class));
        }
        for _ in 0..count {
            let Some(applied) = current else {
                return false;
            };
            let runs_nothing = self.heap.class_is_applied_level(applied)
                && self.heap.class_level_facts(applied).is_some_and(|facts| {
                    !facts.needs_initializer_code
                        && facts.factory_arity.is_none_or(|arity| arity == 0)
                });
            if !runs_nothing {
                return false;
            }
            current = self.heap.class_superclass(applied);
        }
        true
    }

    /// The checks a passed level's full body opens with
    /// (`CheckAppliedClause`): each of the `<:` clauses of `level`'s
    /// declaration against the primary factory of the class it named,
    /// nearest level first, with `check_applied_clause`'s text.
    fn check_applied_clauses(
        &self,
        level: ClassReference<'gc>,
        view: ClassLevel<'_, 'gc>,
        pass: Pass,
    ) -> Result<(), Signal<'gc>> {
        if pass.applied_count == 0 {
            return Ok(());
        }
        let declaration = match pass.own_level {
            0 => view.body_less_declaration(),
            _ => view.application().map(|application| &application.mixin),
        };
        let Some(declaration) = declaration else {
            return Ok(());
        };
        let mut current = Some(level);
        for _ in 0..pass.own_level {
            current = current.and_then(|class| self.heap.class_superclass(class));
        }
        for clause in declaration.mixin_applications.iter().rev() {
            let Some(applied) = current else {
                return Ok(());
            };
            self.check_clause_names_the_factory(level, applied, clause.message.selector)?;
            current = self.heap.class_superclass(applied);
        }
        Ok(())
    }
}

/// The body the initializer of the level `view` shows runs and which of
/// its mixin's three it is: the full one for a level no `<:` clause added,
/// which is its declaration's own, and for a body-less declaration's class
/// object the declaration's full one (decision 8 of `new` as compiled
/// code); for any other level one added, reached from outside the
/// declaration that applied it (decision 4), the standalone one -- its
/// mixin's own superclass clause, then its own part, never the mixin's own
/// `<:` clauses, whose levels are not above it -- or the full one when the
/// mixin writes no `<:` clause, which is then the same. `None` for a class
/// object with no application and for a mixin that carries no such body.
fn level_initializer_of(
    view: ClassLevel<'_, '_>,
) -> Option<(Rc<crate::bytecode::CompiledCode>, InitializerBody)> {
    if let Some(declaration) = view.body_less_declaration() {
        let body = InitializerBody::Full;
        return body.code_of(declaration).map(|code| (code, body));
    }
    let mixin = &view.application()?.mixin;
    let body = if view.is_applied_level() && mixin.standalone_initializer_code.is_some() {
        InitializerBody::Standalone
    } else {
        InitializerBody::Full
    };
    body.code_of(mixin).map(|code| (code, body))
}

/// The arity of the factory a level's factory arguments are held to
/// (`check_arity`) before its activation is pushed or any slot
/// initializer / initExpr runs: that of the declaration whose full or
/// standalone body the level `view` shows runs -- the body-less
/// declaration's for its class object, the mixin's otherwise -- since that
/// body's registers are laid out for it.
fn declared_factory_arity(view: ClassLevel<'_, '_>) -> Option<usize> {
    match view.body_less_declaration() {
        Some(declaration) => declaration.factory.map(|factory| factory.argument_count),
        None => view.level_facts().and_then(|facts| facts.factory_arity),
    }
}

/// Where `InitializeInstance` stands in a factory body
/// (`compiler::factory_body`).
const FACTORY_INITIALIZE: usize = 1;

/// The two registers a factory body (`compiler::factory_body`) writes
/// after its arguments.
#[derive(Clone, Copy)]
pub(crate) struct FactoryRegisters {
    instance: u16,
    answer: u16,
}

/// The registers of `code` when it is a factory body
/// (`compiler::factory_body`) in its eight-byte form: `AllocateInstance`
/// into the instance's register, `InitializeInstance` of it into the
/// answer's, `Return` of the instance. `None` for every other body, which
/// `Mutator::enter_factory_body` leaves to the run loop.
pub(crate) fn factory_registers(code: &crate::bytecode::CompiledCode) -> Option<FactoryRegisters> {
    use crate::bytecode::CompactInstruction;
    if code.instruction_count() != 3 {
        return None;
    }
    match (code.slot(0), code.slot(1), code.slot(2)) {
        (
            CompactInstruction::AllocateInstance { destination },
            CompactInstruction::InitializeInstance {
                destination: answer,
                instance,
            },
            CompactInstruction::Return { register },
        ) if instance == destination && register == destination => Some(FactoryRegisters {
            instance: u16::from(instance),
            answer: u16::from(answer),
        }),
        _ => None,
    }
}

/// What a passed level checks (`Mutator::passes`): how many `<:` clauses its
/// declaration writes, how many steps above it the first of their levels
/// is -- one for a declaration with a body, none for a body-less one's
/// class object, which is its last clause's level -- and its superclass
/// clause's selector.
#[derive(Clone, Copy)]
struct Pass {
    applied_count: usize,
    own_level: usize,
    clause_selector: Symbol,
}

/// `factory_arguments` held to `declared_arity`, the arity of the factory a
/// body's registers are laid out for; `None` checks nothing.
///
/// A slot initializer or initExpr may reference a factory parameter as
/// `Binding::Local(i)`, resolved against the declared factory pattern --
/// but the activation an initializer runs in lays its registers out from
/// the arguments actually supplied, so a wrong count would shift every
/// temporary, and too few would leave `locals.rs`'s `read_local` /
/// `write_local` indexing past the end of its register window.
///
/// Checked at *every* level, not just the outermost. A level's arguments
/// come from its subclass's clause message, whose selector §6.1's check has
/// already matched against the named class's primary factory selector --
/// which pins the arity too for a class with a body, since a selector's
/// keyword count is its parameter count, but not for a body-less named
/// class, whose factory is its declaration's while the own part its level
/// runs is its last clause's mixin's. This guard is what keeps that
/// reasoning from being the only thing between a decoded image's body and
/// an out-of-bounds index.
#[inline(always)]
fn check_arity<'gc>(
    class: ClassReference<'gc>,
    declared_arity: Option<usize>,
    factory_arguments: &[Value<'gc>],
) -> Result<(), Signal<'gc>> {
    if let Some(declared_arity) = declared_arity {
        if factory_arguments.len() != declared_arity {
            return Err(factory_arity_error(
                class,
                declared_arity,
                factory_arguments.len(),
            ));
        }
    }
    Ok(())
}

/// Which of a declaration's clauses a message was written in, for the error
/// that names it.
#[derive(Clone, Copy)]
enum ClauseKind {
    Superclass,
    MixinApplication,
}

/// Which of a mixin's three initializer bodies an initializer activation
/// runs (`docs/design/compiler.md`, "The three initializer bodies"), and so
/// which field of its level's mixin a frame of it names
/// (`FrameKind::Initializer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitializerBody {
    /// The declaration's own initializer, steps 1-5: its `<:` clauses
    /// checked, its superclass initialized, each applied level's own part,
    /// then its own slot clauses and `initExprs` (`Mixin::initializer_code`).
    Full,
    /// The slot clauses and `initExprs` alone, which the level a `<:`
    /// clause added runs (`Mixin::own_initializer_code`).
    OwnPart,
    /// The mixin's own superclass clause, then its own part, for a level a
    /// `<:` clause added that is reached from outside the declaration that
    /// applied it (`Mixin::standalone_initializer_code`).
    Standalone,
}

impl InitializerBody {
    /// The body of this kind `mixin` carries, if any.
    pub(crate) fn code_of(
        self,
        mixin: &crate::mixin::Mixin,
    ) -> Option<Rc<crate::bytecode::CompiledCode>> {
        match self {
            InitializerBody::Full => mixin.initializer_code.clone(),
            InitializerBody::OwnPart => mixin.own_initializer_code.clone(),
            InitializerBody::Standalone => mixin.standalone_initializer_code.clone(),
        }
    }

    /// The frame kind tag a frame running this body is written with, in a
    /// frame object and in an image (`image::opcodes`, `FRAME_*`).
    pub(crate) fn frame_tag(self) -> u8 {
        use crate::image::opcodes::{
            FRAME_INITIALIZER, FRAME_OWN_INITIALIZER, FRAME_STANDALONE_INITIALIZER,
        };
        match self {
            InitializerBody::Full => FRAME_INITIALIZER,
            InitializerBody::OwnPart => FRAME_OWN_INITIALIZER,
            InitializerBody::Standalone => FRAME_STANDALONE_INITIALIZER,
        }
    }

    /// The body a frame kind tag names, if it names an initializer's.
    pub(crate) fn of_frame_tag(tag: u8) -> Option<InitializerBody> {
        [
            InitializerBody::Full,
            InitializerBody::OwnPart,
            InitializerBody::Standalone,
        ]
        .into_iter()
        .find(|body| body.frame_tag() == tag)
    }
}

/// The body of the one factory entry the metaclass of `class` holds, for
/// a frame of a factory activation, whose body is derived rather than
/// carried. `None` for a class object whose class holds none -- one a host
/// built by hand -- and for anything else.
pub(crate) fn factory_code<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
) -> Option<Rc<crate::bytecode::CompiledCode>> {
    let metaclass = heap.class_application(heap.object_class(class))?;
    metaclass
        .mixin
        .methods
        .values()
        .find_map(|entry| match entry {
            MethodEntry::Factory(factory) => Some(factory.code.clone()),
            _ => None,
        })
}

/// A factory given another number of arguments than it declares: refused
/// before an activation lays out its registers from the wrong count.
#[cold]
pub(crate) fn factory_arity_error<'gc>(
    class: ClassReference<'gc>,
    declared: usize,
    given: usize,
) -> Signal<'gc> {
    Signal::Error(NewspeakError::error(
        Symbol::NEW,
        Value::object(class),
        format!("wrong number of factory arguments: factory takes {declared}, got {given}"),
    ))
}

/// The selector of `class`'s primary factory message (`Heap::class_factory`,
/// read through its metaclass): its declaration's, `new` for a builtin, and
/// for a level a `<:` clause added the named class's. `None` for a
/// non-class object and a metaclass.
///
/// Read by §6.1's checks, which decide whether a clause names it. The
/// other side of that question -- whether an
/// incoming send *is* the factory message -- is no longer asked: §6.3 makes
/// the factory an entry of the metaclass mixin, so the ordinary lookup
/// answers it (`mixin::factory_entry`).
pub(crate) fn primary_factory_selector<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
) -> Option<Symbol> {
    heap.class_factory(class).map(|factory| factory.selector)
}

/// `class`'s name for an error's text (`Heap::class_name`).
fn class_name<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>) -> Symbol {
    heap.class_name(class).unwrap_or(Symbol::AN_OBJECT)
}

/// The class a clause's message is checked against, for its error: the
/// class object the error is raised on, and the class whose primary factory
/// it names. The two differ only at a body-less declaration's class object,
/// whose `<:` clause named another class than itself.
#[derive(Clone, Copy)]
struct Clause<'gc> {
    receiver: ClassReference<'gc>,
    named: ClassReference<'gc>,
}

/// `newspeak-spec.pdf` §6.1: "It is a run time error if the message
/// selector specified by the inheritance clause is not the selector of the
/// primary factory of the class *C*."
fn not_the_primary_factory_error<'gc>(
    heap: &Heap<'gc>,
    clause_kind: ClauseKind,
    class: ClassReference<'gc>,
    clause: Clause<'gc>,
    selector: Symbol,
    expected: Option<Symbol>,
) -> Signal<'gc> {
    let subclass_name = class_name(heap, class);
    let kind = match clause_kind {
        ClauseKind::Superclass => "the superclass clause",
        ClauseKind::MixinApplication => "a mixin application clause",
    };
    let superclass_name = class_name(heap, clause.named);
    let expected = match expected {
        Some(expected) => format!("#{expected}"),
        // The class is not a class object at all -- an internally
        // inconsistent chain rather than a program error, reported here
        // rather than swallowed.
        None => "no factory (it is not a class)".to_string(),
    };
    Signal::Error(NewspeakError::error(
        selector,
        Value::object(clause.receiver),
        format!(
            "{kind} of class {subclass_name} sends #{selector}, \
             but the primary factory of class {superclass_name} is {expected}"
        ),
    ))
}

/// The class an instantiation refusal names: its name and the primary
/// factory the program sent it, both read through its metaclass
/// (`Heap::class_name`, `Heap::class_factory`), and its mixin's when it has
/// none yet.
#[derive(Clone, Copy)]
struct Instantiated {
    name: Symbol,
    selector: Symbol,
}

impl Instantiated {
    fn of<'gc>(
        heap: &Heap<'gc>,
        class: ClassReference<'gc>,
        application: &crate::mixin::MixinApplication,
    ) -> Instantiated {
        Instantiated {
            name: heap.class_name(class).unwrap_or(application.mixin.name),
            selector: heap
                .class_factory(class)
                .map_or_else(|| factory_selector(application), |factory| factory.selector),
        }
    }
}

/// The refusal `allocate_for_factory` raises, named under the class's own
/// primary factory as both value refusals are. `display_name` is
/// `Heap::class_display_name`'s, which this function has no heap to ask:
/// `Foo class` names a metaclass where its mixin's name would not.
#[cold]
fn instantiation_refusal_error<'gc>(
    class: ClassReference<'gc>,
    display_name: &str,
    instantiated: Instantiated,
    refusal: &crate::mixin::InstantiationRefusal,
) -> NewspeakError<'gc> {
    match refusal {
        crate::mixin::InstantiationRefusal::MutableValueSlot(violation) => {
            mutable_value_slot_error(class, instantiated, *violation)
        }
        // The resolver refuses a root that breaks the root rule, so no source
        // module reaches this; it stays because a `ResolvedModule` is a
        // public struct a host can build without resolving, and the encoder
        // reads the same verdict to keep such a root out of an image, which
        // is untrusted input (`image-format.md`, "Verification").
        crate::mixin::InstantiationRefusal::RootClause(defect) => {
            NewspeakError::cannot_instantiate(
                instantiated.selector,
                Value::object(class),
                Value::object(class),
                format!(
                    "cannot instantiate {}: {}",
                    instantiated.name,
                    defect.describe()
                ),
            )
        }
        crate::mixin::InstantiationRefusal::IdentityMethod { declaring } => {
            identity_method_error(class, instantiated, *declaring)
        }
        crate::mixin::InstantiationRefusal::ClassObject => NewspeakError::cannot_instantiate(
            instantiated.selector,
            Value::object(class),
            Value::object(class),
            format!(
                "cannot instantiate {display_name}: a class object is made only by evaluating \
                 its declaration"
            ),
        ),
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
    instantiated: Instantiated,
    violation: crate::mixin::MutableValueSlot,
) -> NewspeakError<'gc> {
    let Instantiated {
        name: instantiated,
        selector,
    } = instantiated;
    let declares = if instantiated == violation.class {
        "it declares".to_string()
    } else {
        format!("{} declares", violation.class)
    };
    NewspeakError::cannot_instantiate(
        selector,
        Value::object(class),
        Value::object(class),
        format!(
            "cannot instantiate {instantiated}: a value object is deeply immutable, and \
             {declares} the mutable slot {}",
            violation.slot
        ),
    )
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
    instantiated: Instantiated,
    declaring: Symbol,
) -> NewspeakError<'gc> {
    NewspeakError::cannot_instantiate(
        instantiated.selector,
        Value::object(class),
        Value::object(class),
        format!(
            "cannot instantiate {}: a value class may not define ==, and {declaring} defines it",
            instantiated.name
        ),
    )
}

/// §3.1.1's other class-level condition -- every enclosing object a value --
/// decided when the class object was created
/// (`interpreter::value_class::check_value_class`), and refused here rather
/// than there (`docs/design/object-model.md`, "A value object's four
/// conditions"). Named under the class's own primary factory, like every
/// other instantiation refusal. Reads the refusal off `class` itself, so
/// `allocate_for_factory`'s hot path tests only for its presence
/// (`Heap::class_has_value_refusal`) and never copies it.
#[cold]
#[inline(never)]
fn value_class_refusal_error<'gc>(
    heap: &Heap<'gc>,
    class: ClassReference<'gc>,
    application: &crate::mixin::MixinApplication,
) -> NewspeakError<'gc> {
    let Instantiated {
        name: instantiated,
        selector,
    } = Instantiated::of(heap, class, application);
    let Some(refusal) = heap.class_value_refusal(class) else {
        unreachable!("internal: value_class_refusal_error called for a class with no refusal");
    };
    let ValueClassRefusal::NonValueEnclosingObject { level, offender } = refusal;
    let described = heap.describe(Value::object(offender));
    let message = match level {
        None => format!(
            "cannot instantiate {instantiated}: a value class's enclosing objects must be \
             values, and its enclosing object, {described}, is not one"
        ),
        Some(level) => format!(
            "cannot instantiate {instantiated}: a value class's enclosing objects must be \
             values, and {level}'s enclosing object, {described}, is not one"
        ),
    };
    NewspeakError::cannot_instantiate(
        selector,
        Value::object(class),
        Value::object(class),
        message,
    )
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
    let Some(stored) = value.as_object() else {
        return Ok(());
    };
    let Some(instance) = receiver.as_object() else {
        return Ok(());
    };
    let class = mutator.heap().object_class(instance);
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
/// class `Value`", which covers a `String`, a `Symbol`, an
/// instance of a user class below `Value` and a `Value new` alike -- §3.2's
/// "subclass of" is inclusive, so `Value`'s own application carries the
/// flag (`Mutator::apply_mixin`). A class object is one when its enclosing
/// object is (`class_object_is_a_value`), whatever its own class (its
/// metaclass) inherits: `Behavior` does not inherit `Value`.
///
/// **The other predicate about "is a value" is
/// `builtins::value::state_is_a_value`**, which asks the narrower question
/// of whether an instance's `=` is still `Value`'s own; the two must agree
/// about everything this one accepts and that one unfolds, and they
/// disagreed about `Value new` until 2026-09-23.
pub(crate) fn is_a_value<'gc>(mutator: &Mutator<'gc>, object: ObjectReference<'gc>) -> bool {
    if mutator.heap().is_class(object) {
        return class_object_is_a_value(mutator, object);
    }
    mutator
        .heap()
        .class_inherits_value(mutator.heap().object_class(object))
}

/// Whether the class object `class` is a value: exactly when its enclosing
/// objects are -- a body-less declaration's class object has its
/// declaration's besides its level's (`Heap::class_declaration_scope`)
/// (`docs/design/object-model.md`, "Class sides and metaclasses",
/// after SOMns's `SClass.isValue`). A module definition has none and is one
/// (§6.4: "Module definitions are value objects"); a nested class's only
/// state is its enclosing object (§6.1), so §3.1.1's condition on enclosing
/// objects decides it. A metaclass shares its class's enclosing object
/// (`Mutator::attach_metaclass`) and so its verdict.
///
/// The equality walk (`builtins::value`) asks this of every pair of
/// distinct classes it meets, and a slot of a value asks it of a class it
/// is handed, so the answer must not depend on anything but the class
/// object itself.
pub(crate) fn class_object_is_a_value<'gc>(
    mutator: &Mutator<'gc>,
    class: ClassReference<'gc>,
) -> bool {
    // An enclosing object is always an instance, never a class object, so
    // this does not recurse -- and an image that claims otherwise gets
    // `false`, the answer that refuses rather than accepts.
    let is_a_value = |enclosing: Option<ObjectReference<'gc>>| match enclosing {
        None => true,
        Some(enclosing) => {
            !mutator.heap().is_class(enclosing)
                && mutator
                    .heap()
                    .class_inherits_value(mutator.heap().object_class(enclosing))
        }
    };
    // A body-less declaration's class object has two enclosing objects,
    // its declaration's and its level's (§3.3, §6), and is a value only
    // when both are; its metaclass shares its verdict. The level's own is
    // asked first, and the scope only of a class object that has one.
    let heap = mutator.heap();
    if !is_a_value(heap.class_enclosing(class)) {
        return false;
    }
    let instance_side = heap.class_this_class(class).unwrap_or(class);
    if !heap.class_has_declaration_scope(instance_side) {
        return true;
    }
    is_a_value(
        heap.class_declaration_scope(instance_side)
            .and_then(|scope| scope.enclosing),
    )
}

/// The refusal itself, off the checking path so that the check stays a
/// handful of loads. The slot is named from the level whose initializer or
/// lazy slot getter is running -- the activation's `current_class`, which
/// for a getter is its defining class -- because `own_index` is
/// mixin-relative (`bytecode.rs`), so it indexes that level's own slots and
/// no other's.
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
/// came to disagree. A lazy slot's getter follows the same rule to a
/// different answer: the store happens on the getter's send, long after
/// the instantiation, so the selector the program sent is the getter's.
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
    // A lazy slot's getter (§6.3.3) stores from a method activation, and
    // the send the program wrote there is the getter's, not a factory.
    let selector = match mutator.heap().activation_kind(activation) {
        FrameKind::Method(method) => method.selector,
        _ => mutator
            .heap()
            .class_factory(instantiated)
            .map_or(Symbol::NEW, |factory| factory.selector),
    };
    NewspeakError::error(
        selector,
        receiver,
        format!(
            "cannot store {} in {class}'s slot {slot}: a value object is deeply immutable, \
             so every slot it declares holds a value",
            mutator.heap().describe(value)
        ),
    )
}

/// A class's primary factory selector, defaulted to `new` for a class that
/// declares none -- `mixin::factory_entry`'s rule, which is what puts the
/// entry in the metaclass's method table, asked again here so that a
/// refusal names the selector the program actually sent.
pub(crate) fn factory_selector(application: &crate::mixin::MixinApplication) -> Symbol {
    application
        .mixin
        .factory
        .map_or(Symbol::NEW, |factory| factory.selector)
}

/// A `MessagePattern` rendered as the selector string it declares. Also the
/// image encoder's `FrameKind::Method` representation: a frame names the
/// method it activates by selector, access and body, and the load reuses
/// the method of that selector in the mixin the decoded mixin table rebuilt
/// for the frame's own `current_class` when it runs that very body
/// (`image::decode::frame_method`).
pub(crate) fn method_name(pattern: &ast::MessagePattern) -> String {
    match pattern {
        ast::MessagePattern::Unary { selector, .. } => selector.name.clone(),
        ast::MessagePattern::Binary { selector, .. } => selector.name.clone(),
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|part| format!("{}:", part.keyword.name))
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

/// Whether `mixin` declares any slot or `initExpr` at all: the one
/// definition of "needs initializer code", which `mixin::LevelFacts` records
/// per application.
///
/// `pub(crate)` for `image::decode::verify_mixins`, which must refuse
/// an image that has no own part for a mixin that needs one -- checked per
/// mixin, since every mixin of the tree carries its own.
pub(crate) fn mixin_needs_initializer_code(mixin: &crate::mixin::Mixin) -> bool {
    !mixin.slots.is_empty() || mixin.has_initializer_expressions
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::rc::Rc;

    use indoc::indoc;

    use crate::mixin::{LevelFacts, Mixin};
    use crate::types::{BuiltinClass, ErrorClass, SourceOrigin};
    use crate::{HostResult, HostSignal, Interpreter, Root, Symbol};

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

    /// The root class of `source` instantiated, and what each of `selectors`
    /// answers when sent to the instance, in order.
    fn send_to_root(source: &str, selectors: &[&str]) -> (Interpreter, Vec<Root>) {
        let (mut interpreter, class) = load(source);
        let root = expect_ok(interpreter.instantiate(&class, &[]), "instantiate the root");
        let answers = selectors
            .iter()
            .map(|selector| {
                expect_ok(
                    interpreter.send(&root, Symbol::intern(selector), &[]),
                    selector,
                )
            })
            .collect();
        (interpreter, answers)
    }

    /// How `answer` is described in Newspeak-facing text.
    fn described(interpreter: &mut Interpreter, answer: &Root) -> String {
        interpreter.enter(|mutator| mutator.heap().describe(mutator.fetch(answer)))
    }

    /// A class that declares no slot, no initializer expression and no
    /// superclass message with arguments still answers an instance of
    /// itself: its factory allocates one and enters no initializer.
    #[test]
    fn a_class_with_no_initializer_code_answers_its_instance() {
        let source = indoc! {"
            class Root = () (
              public class Empty = () ()
              public make = (
                ^Empty new.
              )
            )
        "};
        let (mut interpreter, answers) = send_to_root(source, &["Empty", "make"]);
        interpreter.enter(|mutator| {
            let class = mutator.fetch(&answers[0]).as_object().expect("a class");
            let made = mutator.fetch(&answers[1]).as_object().expect("an instance");
            assert!(mutator.heap().object_class(made).same(class));
        });
    }

    #[test]
    fn a_class_with_a_slot_initializes_it() {
        let source = indoc! {"
            class Root = () (
              public class Holder = (| public x = 3. |) ()
              public make = (
                ^Holder new x.
              )
            )
        "};
        let (mut interpreter, answers) = send_to_root(source, &["Holder", "make"]);
        assert_eq!(described(&mut interpreter, &answers[1]), "3");
    }

    #[test]
    fn a_superclass_clause_with_arguments_initializes_the_superclass_with_them() {
        let source = indoc! {"
            class Root = () (
              class Base value: v = (| public value = v. |) ()
              public class Derived = Base value: 4 () ()
              public make = (
                ^Derived new value.
              )
            )
        "};
        let (mut interpreter, answers) = send_to_root(source, &["Derived", "make"]);
        assert_eq!(described(&mut interpreter, &answers[1]), "4");
    }

    /// The classes of `goldens/language/class/mixin/mixin_body_less_application.ns`,
    /// its `main:` turned into one method per line it shows: a chain with a
    /// `<:` level answers what the golden records.
    #[test]
    fn a_class_with_an_applied_level_answers_what_the_golden_records() {
        let source = indoc! {"
            class MixinBodyLessApplication = Object new () (
              class Base = Object new () (
                public fromBase = (
                  ^'Base answers fromBase'.
                )
              )

              class Marker = Object new () (
                public mark = (
                  ^'Marker answers mark'.
                )
              )

              public class Applied = Object <: Marker.

              public class Tagged = Base new <: Marker.

              public appliedMark = (
                ^Applied new mark.
              )

              public taggedMark = (
                ^Tagged new mark.
              )

              public taggedFromBase = (
                ^Tagged new fromBase.
              )
            )
        "};
        let (mut interpreter, answers) = send_to_root(
            source,
            &[
                "Applied",
                "Tagged",
                "appliedMark",
                "taggedMark",
                "taggedFromBase",
            ],
        );
        let descriptions: Vec<String> = answers[2..]
            .iter()
            .map(|answer| described(&mut interpreter, answer))
            .collect();
        assert_eq!(
            descriptions,
            [
                "the String \"Marker answers mark\"",
                "the String \"Marker answers mark\"",
                "the String \"Base answers fromBase\""
            ]
        );
    }

    /// A host's instantiation with a count of arguments the factory does
    /// not declare raises the factory's arity error, under `new`.
    #[test]
    fn a_wrong_argument_count_still_raises_the_arity_error() {
        let (mut interpreter, class) = load("class Empty = () ()\n");
        let argument = interpreter.stash(crate::Value::small_integer(1));
        match interpreter.instantiate(&class, &[argument]) {
            Err(HostSignal::Error(error)) => {
                assert_eq!(
                    error.message,
                    "wrong number of factory arguments: factory takes 0, got 1"
                );
                assert_eq!(error.selector, Symbol::NEW);
            }
            Err(HostSignal::Return) => panic!("a non-local return escaped"),
            Ok(_) => panic!("expected the arity error"),
        }
    }

    /// A declaration's own level runs its full initializer, and a level a
    /// `<:` clause added runs its mixin's standalone one when the mixin
    /// writes `<:` clauses of its own, and its full one otherwise
    /// (decision 4 of `new` as compiled code).
    #[test]
    fn level_initializer_picks_the_full_body_or_for_an_applied_level_the_standalone_one() {
        let source = indoc! {"
            class Root = () (
              class Extra = () ()
              class Plain = () ()
              class Composite = Object new <: Extra (| c = 1. |) ()
              public class User = Object new <: Composite () ()
              public class Simple = Object new <: Plain () ()
            )
        "};
        let (mut interpreter, answers) = send_to_root(source, &["User", "Simple"]);
        interpreter.enter(|mutator| {
            let user = mutator.fetch(&answers[0]).as_object().expect("a class");
            let simple = mutator.fetch(&answers[1]).as_object().expect("a class");
            let body = |level| {
                super::level_initializer_of(mutator.heap.class_level(level)).map(|(_, body)| body)
            };
            assert_eq!(body(user), Some(super::InitializerBody::Full));
            let composite_level = mutator.heap().class_superclass(user).expect("a level");
            assert_eq!(
                body(composite_level),
                Some(super::InitializerBody::Standalone)
            );
            let plain_level = mutator.heap().class_superclass(simple).expect("a level");
            assert_eq!(body(plain_level), Some(super::InitializerBody::Full));
        });
    }

    /// Every application's level facts are what its own mixin says, at
    /// every level of a chain.
    #[test]
    fn level_facts_agree_with_the_mixin() {
        let source = indoc! {"
            class Root = () (
              class Base value: v = (| public value = v. |) ()
              public class Derived = Base value: 4 (| other = 5. |) ()
            )
        "};
        let (mut interpreter, answers) = send_to_root(source, &["Derived"]);
        interpreter.enter(|mutator| {
            let class = mutator.fetch(&answers[0]).as_object().expect("a class");
            let mut level = mutator.heap().class_application(class);
            let mut levels = 0;
            while let Some(application) = level {
                assert_eq!(
                    application.level_facts,
                    LevelFacts::of(&application.mixin),
                    "{}",
                    application.mixin.name
                );
                levels += 1;
                level = application.superclass.clone();
            }
            assert!(levels >= 3, "Derived, Base and Object at least");
        });
    }

    /// The nested mixin of `mixin` named `name`.
    fn nested_named(heap: &crate::heap::Heap<'_>, mixin: &Rc<Mixin>, name: &str) -> Rc<Mixin> {
        heap.nested_mixins(mixin)
            .into_iter()
            .find(|nested| nested.name.as_str() == name)
            .unwrap_or_else(|| panic!("{} declares {name}", mixin.name))
    }

    /// Whether `mixin`'s enclosing mixin is `expected`.
    fn encloses(expected: &Rc<Mixin>, mixin: &Mixin) -> bool {
        mixin
            .enclosing_mixin()
            .is_some_and(|enclosing| Rc::ptr_eq(&enclosing, expected))
    }

    /// Every mixin of a module built from source names the one it is
    /// declared in -- a class side its instance side -- and holds its own
    /// declaration's text, which a declared class side shares with its
    /// instance side.
    #[test]
    fn a_module_built_from_source_links_every_mixin_to_its_enclosing_one() {
        let source = indoc! {"
            class Outer = Object new () (
              class Inner = Object new () (
                class Innermost = Object new () ()
              ) : (
                make = (^self new)
              )
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        interpreter.enter(|mutator| {
            let class = mutator
                .fetch(&class)
                .as_object()
                .expect("a module's root class is an object");
            let outer = mutator
                .heap()
                .class_application(class)
                .expect("a root class has an application")
                .mixin
                .clone();
            let inner = nested_named(mutator.heap(), &outer, "Inner");
            let innermost = nested_named(mutator.heap(), &inner, "Innermost");
            let inner_class_side = inner
                .class_side
                .clone()
                .expect("Inner declares a class side");
            assert!(
                outer.enclosing_mixin().is_none(),
                "a module's root has none"
            );
            assert!(encloses(&outer, &inner));
            assert!(encloses(&inner, &innermost));
            assert!(encloses(&inner, &inner_class_side));
            for mixin in [&outer, &inner, &innermost] {
                let text = mixin.text.as_ref().expect("built from source");
                assert!(!text.is_shared(), "{} owns its text", mixin.name);
            }
            let inner_text = inner.text.as_ref().expect("built from source");
            assert!(inner_class_side
                .text
                .as_ref()
                .is_some_and(|text| Rc::ptr_eq(text, inner_text)));
        });
    }

    /// A class that declares no class side gets one synthesized, which
    /// names the instance side it was synthesized for and shares its text,
    /// none for a mixin with no declaration.
    #[test]
    fn a_synthesized_class_side_names_its_instance_side() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let instance_side = Rc::new(Mixin::empty(crate::Symbol::intern("Plain")));
            let class_side = mutator.metaclass_mixin(&instance_side);
            assert!(encloses(&instance_side, &class_side));
            assert!(class_side.text.is_none());
        });
    }

    /// A class side synthesized for a declaration shares its instance
    /// side's text, the same `Rc`, as a declared one does: no copy for the
    /// class side, and one text per declaration.
    #[test]
    fn a_synthesized_class_side_shares_its_instance_sides_text() {
        let source = indoc! {"
            class Outer = Object new () (
              class Inner = Object new () ()
            )
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        interpreter.enter(|mutator| {
            let class = mutator
                .fetch(&class)
                .as_object()
                .expect("a module's root class is an object");
            let outer = mutator
                .heap()
                .class_application(class)
                .expect("a root class has an application")
                .mixin
                .clone();
            let inner = nested_named(mutator.heap(), &outer, "Inner");
            assert!(inner.class_side.is_none(), "Inner declares no class side");
            let class_side = mutator.metaclass_mixin(&inner);
            let inner_text = inner.text.as_ref().expect("built from source");
            assert!(class_side
                .text
                .as_ref()
                .is_some_and(|text| Rc::ptr_eq(text, inner_text)));
        });
    }

    /// The mirror's class is the capability's own class, so the refusal
    /// carries nil as its `behavior`: a handler reading it must not be
    /// handed the class, and with it a way to the mirror.
    #[test]
    fn refusing_the_mirrors_class_signals_a_cannot_instantiate_whose_subject_is_nil() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = mutator.builtin_class(BuiltinClass::VmMirror);
            let Err(super::Signal::Error(error)) = mutator.allocate_for_factory(class) else {
                panic!("instantiating the VM mirror's class is refused with an error");
            };
            assert_eq!(error.class, ErrorClass::CannotInstantiate);
            assert!(error.subject.is_nil(), "the refusal names no behavior");
        });
    }

    /// A reified mixin is canonical, one per mixin, made only by the
    /// runtime: `Mixin new` is refused as `Symbol new` is, and the refusal
    /// names no behavior, since the class is private to the kernel.
    #[test]
    fn mixin_new_is_refused() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let symbol = mutator.builtin_class(BuiltinClass::Symbol);
            let Err(super::Signal::Error(symbol_error)) = mutator.allocate_for_factory(symbol)
            else {
                panic!("instantiating Symbol is refused with an error");
            };
            let class = mutator.builtin_class(BuiltinClass::Mixin);
            let Err(super::Signal::Error(error)) = mutator.allocate_for_factory(class) else {
                panic!("instantiating Mixin is refused with an error");
            };
            assert_eq!(error.class, symbol_error.class);
            assert_eq!(error.class, ErrorClass::CannotInstantiate);
            assert!(error.subject.is_nil(), "the refusal names no behavior");
        });
    }

    #[test]
    fn a_root_class_is_a_value_class_exactly_when_the_root_rule_builds_it_over_value() {
        let is_value = |header: &str| {
            let source = format!(
                "{header} ()
"
            );
            let module = ns_syntax::parse(&source).expect("parse");
            crate::resolver::root_class_is_value(&module.root_class)
        };
        for value in [
            "class R = Value ()",
            "class R = Value new ()",
            "class R = Object <: Value ()",
            "class R = Object new <: Value ()",
        ] {
            assert!(is_value(value), "{value}");
        }
        for other in [
            "class R = ()",
            "class R = Object ()",
            "class R = Object new ()",
            // A defect: the rule refuses it, so it is built over `Object`.
            "class R = Value <: Value ()",
            "class R = Value foo ()",
        ] {
            assert!(!is_value(other), "{other}");
        }
    }

    #[test]
    fn root_clause_defect_classifies_every_root_form() {
        use crate::mixin::RootClauseDefect;
        let defect_of = |header: &str| {
            let source = format!("{header} ()\n");
            let module = ns_syntax::parse(&source).expect("parse");
            crate::resolver::root_clause_defect(&module.root_class).map(|(defect, _)| defect)
        };
        for allowed in [
            "class R = ()",
            "class R = Object ()",
            "class R = Object new ()",
            "class R = Value ()",
            "class R = Value new ()",
            "class R = Object <: Value ()",
            "class R = Object new <: Value ()",
        ] {
            assert_eq!(defect_of(allowed), None, "{allowed}");
        }
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
        assert_eq!(
            defect_of("class R = Value new <: Value ()"),
            Some(RootClauseDefect::ValueOverValue)
        );
        let superclass_message = |clause: &str, selector: &str| {
            Some(RootClauseDefect::SuperclassMessage {
                clause: clause.to_string(),
                selector: selector.to_string(),
            })
        };
        assert_eq!(
            defect_of("class R = Object foo ()"),
            superclass_message("Object", "foo")
        );
        assert_eq!(
            defect_of("class R = Object new: 3 ()"),
            superclass_message("Object", "new:")
        );
        assert_eq!(
            defect_of("class R = Value + 3 ()"),
            superclass_message("Value", "+")
        );
        assert_eq!(
            defect_of("class R = Object <: Value new ()"),
            Some(RootClauseDefect::MixinApplicationMessage {
                index: 0,
                clause: "Value".to_string(),
                selector: "new".to_string(),
            })
        );
    }
}

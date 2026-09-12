//! Sends, method invocation, and the `Signal` non-local-return/error channel,
//! shared by everything that runs Newspeak code. Implicit-receiver and
//! `super` sends are in `sends.rs`, register and outer-local access in
//! `locals.rs`, the errors a deferred literal raises in `literals.rs`; the
//! bootstrap and class instantiation are in `instantiate.rs`.

pub mod instantiate;
pub(crate) mod literals;
pub(crate) mod locals;
pub(crate) mod nested;
pub(crate) mod sends;

use std::collections::HashMap;
use std::rc::Rc;

use crate::dispatch::{self, Lookup};
use crate::heap::{ActivationHeader, ActivationRef, Heap};
use crate::types::*;
use crate::Symbol;

pub enum Signal {
    Return(ActivationRef, Value),
    Error(NewspeakError),
}

pub type EvalResult = Result<Value, Signal>;

pub struct Interpreter {
    pub(crate) heap: Heap,
    /// Every builtin class object, indexed by `BuiltinClass as usize`.
    /// Indexed rather than hashed because `class_of` asks for one on every
    /// send to a primitive receiver. Empty only while `bootstrap` is
    /// building it; nothing can reach an `Interpreter` in that state.
    builtin_classes: Vec<ClassRef>,
    platform_singleton: Option<Value>,
    /// (mixin identity, superclass application identity) -> behaviour.
    /// Keyed on the superclass's *application*, never on a superclass class
    /// object: a nested superclass has one class object per enclosing
    /// instance, so a class-object key would never hit and every instance
    /// would get its own behaviour.
    applications: HashMap<(usize, usize), Rc<crate::mixin::MixinApplication>>,
    /// mixin identity -> the synthetic empty class side `metaclass_of`
    /// stands in for that mixin's own level when it declares none.
    ///
    /// Cached, not minted per call, because the mixin's `Rc` identity is
    /// what `applications` keys on: a fresh one per call would give two
    /// class objects that share an instance-side application two *different*
    /// metaclass applications, and the shared-application property is
    /// exactly what the inline caches will guard on. It would
    /// also grow `applications` without bound.
    empty_class_sides: HashMap<usize, Rc<crate::mixin::Mixin>>,
    /// The (receiver arena index, declaration identity) pairs whose nested
    /// class object is being created right now -- see
    /// `nested::cyclic_definition_error`. A stack, not a set: it is empty
    /// on every path that is not inside a superclass clause, and a clause
    /// nests only as deep as a program actually chains class creations, so
    /// a linear scan beats hashing and allocates nothing in the common
    /// case.
    nested_in_progress: Vec<(usize, usize)>,
}

impl Interpreter {
    pub fn bootstrap() -> Interpreter {
        let mut interp = Interpreter {
            heap: Heap::new(),
            builtin_classes: Vec::new(),
            platform_singleton: None,
            applications: HashMap::new(),
            empty_class_sides: HashMap::new(),
            nested_in_progress: Vec::new(),
        };
        interp.builtin_classes = build_builtin_classes(&mut interp);
        interp
    }

    pub fn builtin_class(&self, kind: BuiltinClass) -> ClassRef {
        self.builtin_classes[kind as usize]
    }

    /// A thin delegate; see `Heap::alloc_identity_hash`'s doc comment for why
    /// this saturates instead of wrapping or panicking.
    pub fn alloc_identity_hash(&mut self) -> u32 {
        self.heap.alloc_identity_hash()
    }

    /// After `image::load`: never hand out an identity hash a loaded object
    /// already carries (`docs/design/image-format.md`, "Sections").
    ///
    /// `max < u32::MAX` is the caller's guarantee, and an enforced one:
    /// `image::decode` refuses an image carrying `u32::MAX` outright
    /// (`DecodeError::IdentityHashExhausted`), because there is no counter
    /// value past it. That refusal is what keeps `Heap::advance_identity_hash_past`'s
    /// `max.saturating_add(1)` in range; `alloc_identity_hash`'s own
    /// saturation is what keeps the *next* allocation in range when the
    /// image's maximum is one below the ceiling.
    pub(crate) fn advance_identity_hash_past(&mut self, max: u32) {
        self.heap.advance_identity_hash_past(max);
    }

    /// The heap, for reading. Public so integration tests can exercise the
    /// seam directly; the field itself stays `pub(crate)` so no caller
    /// outside the crate can hold one across a call back into the
    /// interpreter.
    pub fn heap_ref(&self) -> &Heap {
        &self.heap
    }

    pub fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }

    /// An ordinary (explicit-receiver) send: `newspeak-spec.pdf` §5.5. See
    /// `dispatch_send` for the order the cases are tried in.
    ///
    /// A send leaves the activation stack exactly as it found it: every
    /// activation it pushes is popped on every exit, error paths included.
    /// Checked here, at every send, because a leaked entry is otherwise
    /// silent -- a later activation would sit on top of a dead one's
    /// registers.
    pub fn send(&mut self, receiver: Value, selector: Symbol, args: Vec<Value>) -> EvalResult {
        let depth = self.heap.activation_depth();
        let result = self.dispatch_send(receiver, selector, args);
        debug_assert_eq!(
            self.heap.activation_depth(),
            depth,
            "a send must leave the activation stack as it found it"
        );
        result
    }

    /// `send` without its stack check.
    ///
    /// Checked first, before any lookup: `ClassName new` (or a class's own
    /// non-default factory selector) never has a real method to find --
    /// every class object's own `.class` is a shared placeholder with an
    /// empty method table (see `build_builtin_classes` below), so an
    /// ordinary `lookup_public` search would just fall through to
    /// `doesNotUnderstand:` every time. `try_factory_send`
    /// (`instantiate.rs`) recognizes the shape and routes to
    /// `instantiate` directly; see its own doc comment for why this can
    /// never double-fire against the superclass clause's own deferred
    /// factory message (`run_initializer_level` never calls `send`).
    fn dispatch_send(&mut self, receiver: Value, selector: Symbol, args: Vec<Value>) -> EvalResult {
        if let Some(result) = self.try_factory_send(&receiver, selector, &args) {
            return result;
        }
        // §6.3: a class object's class-side methods are reached through its
        // metaclass, consulted before the class object's own (ordinary,
        // `Object`-native) behaviour. `metaclass_of` returns `None` for a
        // class whose mixin declares no class side, and `Lookup::NotFound`
        // here (the metaclass exists but doesn't answer `selector` either)
        // both fall through to the code below unchanged -- there is no
        // second `doesNotUnderstand:` path, only a second class to try
        // `lookup_public` against first.
        if let Value::Obj(obj) = &receiver {
            let is_class = self.heap.is_class(*obj);
            if is_class {
                if let Some(metaclass) = self.metaclass_of(*obj) {
                    match dispatch::lookup_public(&self.heap, metaclass, selector) {
                        Lookup::Method(method, defining_class) => {
                            return self.invoke_method(receiver, method, defining_class, args);
                        }
                        Lookup::NestedClass(mixin, declaring_class) => {
                            return nested::nested_class(self, &receiver, &mixin, declaring_class);
                        }
                        Lookup::Native(kind) => {
                            return self.dispatch_native_or_dnu(kind, receiver, selector, args);
                        }
                        Lookup::NotFound => {}
                    }
                }
            }
        }
        let class = self.class_of(&receiver);
        match dispatch::lookup_public(&self.heap, class, selector) {
            Lookup::Method(method, defining_class) => {
                self.invoke_method(receiver, method, defining_class, args)
            }
            // §6: the implicit accessor a nested declaration defines is
            // reached exactly like any other member, and it ignores `args`
            // -- the accessor is unary, so a send that supplied arguments
            // did not name it (`walk` matched on the bare selector).
            Lookup::NestedClass(mixin, declaring_class) => {
                nested::nested_class(self, &receiver, &mixin, declaring_class)
            }
            Lookup::Native(kind) => self.dispatch_native_or_dnu(kind, receiver, selector, args),
            Lookup::NotFound => self.does_not_understand(receiver, selector, args),
        }
    }

    /// Native dispatch that still inherits `Object`'s own native methods.
    ///
    /// `dispatch::walk` deliberately treats a class tagged
    /// `builtin: Some(kind)` as a *dispatch leaf*: it returns
    /// `Lookup::Native(kind)` the moment it reaches one, without continuing
    /// up that class's own superclass chain -- that early return is what
    /// keeps `lookupPublic`'s `hard_stop_on_mismatch` semantics uniform.
    /// Every non-`Object` builtin's superclass is `Object` (see
    /// `build_builtin_classes` below), and none of `numbers`/`text`/
    /// `collections`/`block`/`boolean` implements `class`/`==`/`=`/`hash`/
    /// `printString`/`doesNotUnderstand:` itself -- so without this helper
    /// those `Object` methods would be unreachable on *every* non-`Object`
    /// receiver, and `Object`'s spec-required `doesNotUnderstand:` would
    /// never fire for a builtin at all.
    ///
    /// Restores exactly the one missing inheritance step, at the consuming
    /// call site rather than inside `walk`: try `kind`'s own native table
    /// first (so a builtin that *does* implement a selector still wins),
    /// then `Object`'s. Returns `None` only when neither implements
    /// `selector`.
    fn dispatch_native_inheriting_object(
        &mut self,
        kind: BuiltinClass,
        receiver: &Value,
        selector: Symbol,
        args: &[Value],
    ) -> Option<EvalResult> {
        if let Some(result) =
            crate::builtins::dispatch_native(self, kind, *receiver, selector, args)
        {
            return Some(result);
        }
        if kind == BuiltinClass::Object {
            return None;
        }
        crate::builtins::dispatch_native(self, BuiltinClass::Object, *receiver, selector, args)
    }

    fn dispatch_native_or_dnu(
        &mut self,
        kind: BuiltinClass,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
    ) -> EvalResult {
        match self.dispatch_native_inheriting_object(kind, &receiver, selector, &args) {
            Some(result) => result,
            None => self.does_not_understand(receiver, selector, args),
        }
    }

    /// §5.5's `doesNotUnderstand:` fallback -- access-blind (`lookup_defined_for`),
    /// per `docs/design/dispatch.md`.
    ///
    /// Two cases reach here. A user-overridden `doesNotUnderstand:` arrives
    /// as `Lookup::Method`; `Object`'s own *native* one
    /// (`builtins::object_class::dispatch`) arrives as `Lookup::Native`,
    /// exactly like `dispatch_native_or_dnu` reaches any other native
    /// method. Without the native arm, `Object`'s builtin
    /// `doesNotUnderstand:` -- which every class inherits, since
    /// `lookup_defined_for` finds `ClassData.builtin` on any class with no
    /// method override -- would never actually run.
    fn does_not_understand(
        &mut self,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
    ) -> EvalResult {
        let class = self.class_of(&receiver);
        let dnu_selector = Symbol::DOES_NOT_UNDERSTAND;
        // A minimal message-mirror-shaped value: an Array `{selector. args}`
        // rather than a real MessageMirror object, since mirrors do not
        // exist yet (`docs/design/liveness-and-reflection.md`, "Mirrors and
        // MirrorBuilder"). Callers relying on a real `MessageMirror` API are
        // out of scope.
        match dispatch::lookup_defined_for(&self.heap, class, dnu_selector) {
            Lookup::Method(method, defining_class) => {
                let selector_value = self.new_symbol(selector);
                let args_value = Value::Obj(self.new_array(args));
                let message_arg = Value::Obj(self.new_array(vec![selector_value, args_value]));
                self.invoke_method(receiver, method, defining_class, vec![message_arg])
            }
            Lookup::Native(kind) => {
                let selector_value = self.new_symbol(selector);
                let args_value = Value::Obj(self.new_array(args));
                let message_arg = Value::Obj(self.new_array(vec![selector_value, args_value]));
                // Same two-step fallback as `dispatch_native_or_dnu`: `kind`
                // is whatever builtin class `walk` stopped at, and no
                // builtin implements `doesNotUnderstand:` itself, so
                // without the `Object` step this would always miss and fall
                // into the internal-error arm below instead of running
                // `builtins::object_class`'s real, spec-shaped
                // `doesNotUnderstand:`.
                match self.dispatch_native_inheriting_object(
                    kind,
                    &receiver,
                    dnu_selector,
                    &[message_arg],
                ) {
                    Some(result) => result,
                    None => Err(Signal::Error(NewspeakError {
                        selector,
                        receiver,
                        message: format!("doesNotUnderstand: {selector} (native class {kind:?} has no doesNotUnderstand: implementation)"),
                    })),
                }
            }
            // A nested declaration can never be the `doesNotUnderstand:`
            // handler: `walk` matches a declaration by name, and a class
            // name is never a keyword selector. Folded into `NotFound`
            // rather than given an arm of its own so the unreachable case
            // still reports the ordinary "nothing handles this" error
            // instead of a second, differently-worded one.
            Lookup::NotFound | Lookup::NestedClass(..) => Err(Signal::Error(NewspeakError {
                selector,
                receiver,
                message: format!("doesNotUnderstand: {selector} (and no doesNotUnderstand: found anywhere in the class chain)"),
            })),
        }
    }

    /// Pushes an activation, runs the method's compiled body, and unwinds a
    /// matching `Signal::Return` back into an ordinary value.
    ///
    /// The activation is popped on *every* path out -- `^`, falling off the
    /// end, or an unwinding error or return. If a block captured it, popping
    /// divorces its frame, which is what makes a later `^` from that block
    /// the spec's `cannotReturn` rather than a return with no live catcher.
    pub fn invoke_method(
        &mut self,
        receiver: Value,
        method: Rc<crate::mixin::MixinMethod>,
        defining_class: ClassRef,
        args: Vec<Value>,
    ) -> EvalResult {
        // An activation's locals are addressed as one run of registers --
        // the arguments first, the declared temporaries after them -- and
        // the run is laid out from how many arguments the activation was
        // *given*, not how many the method declares. A wrong-arity
        // activation therefore does not fail: it shifts every temporary by
        // the difference and reads an argument back where a temporary
        // should be, which no golden output necessarily shows. Silent wrong
        // output with exit status 0 is the failure mode this repository's
        // invariants single out, so the mismatch is refused here as an
        // ordinary Newspeak error -- never a panic, since the caller may be
        // a host.
        //
        // No Newspeak-level send can reach this: the parser builds a
        // message whose argument count matches its selector by
        // construction. The reachable callers are host-initiated sends --
        // `ns-next run --send`, `snapshot --send`, `load --send`, and any
        // future embedder driving `Interpreter::send` directly.
        if args.len() != method.num_args {
            return Err(Signal::Error(NewspeakError {
                selector: method.selector,
                receiver,
                message: format!(
                    "wrong number of arguments: `{}` takes {}, got {}",
                    method.selector,
                    method.num_args,
                    args.len()
                ),
            }));
        }
        let code = method.code.clone();
        let act = self.heap.push_activation(
            ActivationHeader {
                receiver,
                current_class: Some(defining_class),
                kind: FrameKind::Method(method),
                slot_base: 0, // a method body never contains InitSlot
            },
            &args,
            (code.num_registers - code.num_args) as usize,
        );
        let result = crate::vm::run_code(self, act, &code);
        self.heap.pop_activation(act);
        match result {
            Ok(value) => Ok(value),
            Err(Signal::Return(target, value)) if target.same(act) => Ok(value),
            Err(other) => Err(other),
        }
    }

    /// Invokes a closure with `args`. Called by `builtins::block` for `value`,
    /// `value:`, `value:value:` and so on, and by every native that runs a
    /// block argument, through `builtins::call_block`.
    ///
    /// The invoking activation is not passed: a live activation's sender is
    /// the one below it on the activation stack.
    pub fn invoke_closure(&mut self, closure: &Rc<ClosureData>, args: Vec<Value>) -> EvalResult {
        // A wrong argument count is refused before it can reach a register: the
        // window is laid out from the arguments actually given, so too few would
        // address registers past its end -- a panic -- and too many would
        // silently shift every temporary. Never panic on this
        // Newspeak-program-input path.
        if args.len() != closure.params.len() {
            let receiver = self.heap.frame_receiver(closure.captured_frame);
            return Err(Signal::Error(NewspeakError {
                selector: Symbol::VALUE,
                receiver,
                message: format!(
                    "wrong number of block arguments: block takes {}, got {}",
                    closure.params.len(),
                    args.len()
                ),
            }));
        }
        let receiver = self.heap.frame_receiver(closure.captured_frame);
        let current_class = self.heap.frame_current_class(closure.captured_frame);
        let code = &closure.code;
        let act = self.heap.push_activation(
            ActivationHeader {
                receiver,
                current_class,
                kind: FrameKind::Closure(closure.clone()),
                slot_base: 0, // a closure body never contains InitSlot
            },
            &args,
            (code.num_registers - code.num_args) as usize,
        );
        let result = crate::vm::run_code(self, act, code);
        // Popped on every path out. A closure activation catches no return: its
        // `^` targets its home.
        self.heap.pop_activation(act);
        result
    }

    /// Where a `^` in `act` returns to. A block returns from its home --
    /// the method or initializer that lexically encloses it -- and only
    /// while that activation is still running: its reified frame is married
    /// to a live stack entry exactly until the activation returns, and a
    /// frame the image decoder built never is. A dead home is §7.2's
    /// `cannotReturn`. Every other activation's `^` returns from itself.
    pub(crate) fn raise_return(&mut self, act: ActivationRef, value: Value) -> Signal {
        let target = match self.heap.activation_home(act) {
            None => act,
            Some(home) => match self.heap.frame_activation(home) {
                Some(live) => live,
                None => {
                    return Signal::Error(NewspeakError {
                        selector: Symbol::CARET,
                        receiver: value,
                        message:
                            "cannotReturn: the enclosing method activation has already returned"
                                .to_string(),
                    })
                }
            },
        };
        Signal::Return(target, value)
    }

    pub fn class_of(&self, value: &Value) -> ClassRef {
        match value {
            Value::Obj(obj) => self.heap.class_of_obj(*obj),
            // Nil/Bool/SmallInt/Float classes are wired up by the bootstrap
            // into interpreter-held globals, read from there via
            // `Interpreter::builtin_class(kind)`.
            _ => self.builtin_class_for_primitive(value),
        }
    }

    /// The metaclass carrying `class`'s class side (§6.3), created on first
    /// use and cached on `class` itself (`ClassData::metaclass`) -- not in a
    /// side table, and not in an instance slot the way a nested class's
    /// cache is: a class object's payload is `ObjectData::Class`, never
    /// `Instance`, so it has no slot vector for `nested_slot` to index into.
    /// Caching on the class object directly also gives every send against
    /// the same class object the same metaclass identity, which the inline
    /// caches will need.
    ///
    /// Returns `None` only when neither `class`'s own mixin nor any
    /// ancestor's declares a class side -- a class side is inherited down
    /// the ordinary subclass chain exactly like any other mixin member,
    /// single inheritance and nothing more. What stays out of scope is
    /// §3.2's *reification* of that fact: no `Class` superclass object, and
    /// no per-instance metaclass uniqueness (every class here still shares
    /// one metaclass, the way every application of a mixin shares one
    /// `MixinApplication`). Whether a class side is inherited at all is a
    /// different, much narrower question than how far the reified model
    /// goes, and the answer to it is the same answer §5's ordinary lookup
    /// already gives for a method.
    ///
    /// When `class`'s own mixin declares no class side but an ancestor's
    /// does, an empty synthetic mixin stands in for `class`'s own level --
    /// the same placeholder-mixin technique `builtin_mixin` below already
    /// uses for a builtin with no user-declared behaviour of its own. It
    /// contributes no methods, only relays the superclass's metaclass
    /// application onward, so `class`'s metaclass answers exactly what the
    /// nearest class-side-declaring ancestor's metaclass would.
    ///
    /// Either way, the metaclass's superclass is the superclass's
    /// metaclass, recursively, and the metaclass application chain stays
    /// linked to the metaclass class-object chain exactly as
    /// `dispatch::next_level` requires for the ordinary one.
    /// Iterative, over an explicit stack, for the same reason
    /// `dispatch::walk` is: the superclass chain's length is the *image's*
    /// to choose (`image::decode` builds one link per class record, at
    /// under twenty bytes apiece), and every send to a class object enters
    /// here. Recursing on it would overflow the stack on a chain a ten-
    /// megabyte image can describe -- an abort, not a diagnosable failure.
    ///
    /// The chain is finite: `image::decode::rebuild_applications` refuses a
    /// decoded graph whose superclass links form a cycle, and no other
    /// construction site can build one (a superclass is fixed before the
    /// class object that inherits it exists).
    pub fn metaclass_of(&mut self, class: ClassRef) -> Option<ClassRef> {
        // Walk out to the first level that already answers -- a cached
        // metaclass, a non-class object, or the end of the chain -- pushing
        // each level that still has to be built. `chain` is nearest-first,
        // so the build below runs it in reverse.
        let mut chain: Vec<ClassRef> = Vec::new();
        let mut super_metaclass: Option<ClassRef> = None;
        let mut level = class;
        loop {
            // Not a class object: it has no metaclass, and nothing below it
            // can inherit one through this level.
            if !self.heap.is_class(level) {
                break;
            }
            let cached = self.heap.class_metaclass(level);
            let superclass = self.heap.class_superclass(level);
            if cached.is_some() {
                super_metaclass = cached;
                break;
            }
            chain.push(level);
            let Some(superclass) = superclass else { break };
            level = superclass;
        }
        for level in chain.iter().rev() {
            let own_application = self
                .heap
                .class_application(*level)
                .expect("only class objects are pushed onto the chain");
            let own_mixin = own_application.mixin.clone();
            let enclosing = self.heap.class_enclosing(*level);
            let declaring_class = self.heap.class_declaring(*level);
            let class_side = own_mixin.class_side.clone();
            let class_side_mixin = match class_side {
                Some(mixin) => mixin,
                // No class side of its own to apply -- only worth building a
                // metaclass at all if an ancestor's class side needs relaying
                // through it. `super_metaclass.is_none()` here means neither
                // this level nor any ancestor declares a class side, which is
                // the ordinary "costs nothing" case: this level gets no
                // metaclass, nothing is cached for it, and the next level in
                // still gets its own chance to declare one.
                None => {
                    if super_metaclass.is_none() {
                        continue;
                    }
                    self.empty_class_side_mixin(&own_mixin)
                }
            };
            let super_application = super_metaclass
                .as_ref()
                .and_then(|metaclass| self.heap.class_application(*metaclass));
            let application = self.apply_mixin(&class_side_mixin, super_application.as_ref());
            // §6.3.1: a class side is written inside the same class
            // declaration its instance side is, and `resolver::resolve_class`
            // resolves both against the same lexical chain -- so a class-side
            // method's `outer` must reach exactly the objects an
            // instance-side method's does. The metaclass therefore borrows
            // the class's own place in the declaration chain rather than
            // starting a new, empty one.
            let metaclass =
                self.new_class_object(application, enclosing, declaring_class, super_metaclass);
            if !self.heap.set_class_metaclass(*level, metaclass) {
                unreachable!("only class objects are pushed onto the chain");
            }
            super_metaclass = Some(metaclass);
        }
        // The last level built is `chain[0]`, which is `class` itself; an
        // empty chain means the walk stopped on the first step, and
        // `super_metaclass` is then either `class`'s own cached metaclass or
        // `None` because it is not a class object at all.
        super_metaclass
    }

    fn builtin_class_for_primitive(&self, value: &Value) -> ClassRef {
        let kind = match value {
            Value::Nil => BuiltinClass::Nil,
            Value::Bool(true) => BuiltinClass::True,
            Value::Bool(false) => BuiltinClass::False,
            Value::SmallInt(_) => BuiltinClass::Integer,
            Value::Float(_) => BuiltinClass::Float,
            Value::Obj(_) => unreachable!("Value::Obj carries its own .class"),
        };
        self.builtin_class(kind)
    }

    fn new_instance_object(&mut self, class: ClassRef, data: ObjectData) -> ObjRef {
        self.heap.alloc_object(class, data)
    }

    pub fn new_array(&mut self, elements: Vec<Value>) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Array);
        self.new_instance_object(class, ObjectData::Array(elements))
    }

    pub fn new_string(&mut self, s: String) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Str);
        self.new_instance_object(class, ObjectData::Str(s))
    }

    pub fn new_symbol(&mut self, s: Symbol) -> Value {
        let class = self.builtin_class(BuiltinClass::Sym);
        Value::Obj(self.new_instance_object(class, ObjectData::Sym(s)))
    }

    pub fn new_character(&mut self, c: char) -> ObjRef {
        // There is no dedicated Character payload variant on
        // ObjectData -- represent a character as a one-`char` Rust String
        // wrapped the same way String literals are (Str), consistent with
        // how the language spec itself treats characters as "a special
        // case of string literals" going forward (`newspeak-spec.pdf`
        // §2.2.3). `builtins::text` tags the *class* as
        // `BuiltinClass::Character` so dispatch still lands on the right
        // native methods; only the payload representation is shared.
        let class = self.builtin_class(BuiltinClass::Character);
        self.new_instance_object(class, ObjectData::Str(c.to_string()))
    }

    pub fn new_closure(&mut self, closure: Rc<ClosureData>) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Block);
        self.new_instance_object(class, ObjectData::Closure(closure))
    }

    /// The `platform` object (`docs/design/core-library.md`):
    /// memoized on first call, so every reference within one interpreter
    /// sees the same object identity.
    pub fn platform(&mut self) -> Value {
        if let Some(p) = &self.platform_singleton {
            return *p;
        }
        let class = self.builtin_class(BuiltinClass::Platform);
        let obj = self.new_instance_object(class, ObjectData::Instance(vec![]));
        let value = Value::Obj(obj);
        self.platform_singleton = Some(value);
        value
    }

    /// A `Transcript`-style stdout stream object -- see `crate::platform`'s
    /// module doc for how its payload shape (a one-field, always-`Nil`
    /// `Instance`) tells it apart from the `platform` singleton itself
    /// (a zero-field `Instance`). `pub(crate)`, not `pub`: only
    /// `crate::platform::dispatch` (`platform transcript`) ever needs to
    /// mint one.
    pub(crate) fn new_stream_object(&mut self) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Platform);
        self.new_instance_object(class, ObjectData::Instance(vec![Value::Nil]))
    }

    /// The `platform collections` namespace object -- see `crate::platform`'s
    /// module doc for how its payload shape (a two-field, always-`Nil`
    /// `Instance`) tells it apart from the `platform` singleton and the
    /// transcript stream. Like `new_stream_object`, it is allocated fresh
    /// per send: the namespace is stateless, so identity across sends
    /// carries no meaning that any caller could observe.
    pub(crate) fn new_collections_namespace(&mut self) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Platform);
        self.new_instance_object(class, ObjectData::Instance(vec![Value::Nil, Value::Nil]))
    }

    /// A class object around an already-computed application. The one place
    /// the three-way invariant is established: a class object's `superclass`
    /// is linked exactly when its application's `superclass` is (callers
    /// derive `application` from `superclass` through `apply_mixin`), which
    /// is what `dispatch::next_level` relies on when it steps both chains
    /// together.
    ///
    /// `class` is the shared `Object` placeholder: a class object's own
    /// `.class` field never points at its metaclass. `metaclass_of` reaches
    /// a class side through a separate cache (`ClassData::metaclass`), not
    /// through this field -- §3.2's "every class an instance of a unique
    /// metaclass" stays unbuilt, by design (see `metaclass_of`'s doc
    /// comment).
    pub(crate) fn new_class_object(
        &mut self,
        application: Rc<crate::mixin::MixinApplication>,
        enclosing: Option<ObjRef>,
        declaring_class: Option<ClassRef>,
        superclass: Option<ClassRef>,
    ) -> ClassRef {
        let class = self.builtin_class(BuiltinClass::Object);
        self.heap.alloc_object(
            class,
            ObjectData::Class(ClassData {
                application,
                enclosing,
                declaring_class,
                superclass,
                builtin: None,
                metaclass: None,
            }),
        )
    }

    pub fn apply_mixin(
        &mut self,
        mixin: &Rc<crate::mixin::Mixin>,
        superclass: Option<&Rc<crate::mixin::MixinApplication>>,
    ) -> Rc<crate::mixin::MixinApplication> {
        let key = (
            crate::mixin::mixin_key(mixin),
            superclass.map_or(0, crate::mixin::application_key),
        );
        if let Some(existing) = self.applications.get(&key) {
            return existing.clone();
        }
        let inherited = superclass.map_or(0, |s| s.num_slots);
        let mut slot_index = superclass.map_or_else(HashMap::new, |s| s.slot_index.clone());
        for (i, slot) in mixin.slots.iter().enumerate() {
            slot_index.insert(slot.name, inherited + i);
        }
        // The nested-class cache slots. They are part of the instance layout
        // because the spec caches a nested class in the enclosing *object*;
        // a side table keyed by object would cost a hash lookup on a path
        // that is otherwise a slot read.
        //
        // Keyed by declaration identity: a subclass that redeclares an
        // inherited nested class shadows it for a plain `N` but not for
        // `super N`, so the two need separate slots -- see `nested_slot`.
        let mut nested_slot = superclass.map_or_else(HashMap::new, |s| s.nested_slot.clone());
        let mut next = inherited + mixin.slots.len();
        for nested in &mixin.nested {
            nested_slot.insert(crate::mixin::mixin_key(nested), next);
            next += 1;
        }
        let application = Rc::new(crate::mixin::MixinApplication {
            mixin: mixin.clone(),
            superclass: superclass.cloned(),
            num_slots: next,
            slot_index,
            nested_slot,
            own_slot_base: inherited,
        });
        self.applications.insert(key, application.clone());
        application
    }

    /// The empty mixin `metaclass_of` applies for a class that declares no
    /// class side of its own but inherits one from an ancestor -- same
    /// placeholder technique as `builtin_mixin` below, one `Rc` per owning
    /// mixin so the application cache keys each such level apart from its
    /// siblings. Contributes no methods or slots of its own; its only job
    /// is to give `apply_mixin` something to link the ancestor's metaclass
    /// application through.
    ///
    /// One per *mixin*, not one per call and not one per class object: a
    /// nested class has one class object per enclosing instance but a
    /// single mixin, and §6.2's "all applications of a mixin share code and
    /// structure" has to hold for the metaclass side too -- otherwise two
    /// class objects that share an instance-side application answer a
    /// class-side send out of two distinct metaclass applications, which is
    /// the property the inline caches will guard on.
    ///
    /// Keying the cache by `owner`'s address is safe for the same reason
    /// `applications` is: every mixin that reaches here is held for the
    /// interpreter's life by an entry in `applications`, so its address is
    /// never reused.
    fn empty_class_side_mixin(
        &mut self,
        owner: &Rc<crate::mixin::Mixin>,
    ) -> Rc<crate::mixin::Mixin> {
        let name = owner.name;
        self.empty_class_sides
            .entry(crate::mixin::mixin_key(owner))
            .or_insert_with(|| {
                Rc::new(crate::mixin::Mixin::empty(Symbol::intern(&format!(
                    "{} class",
                    name.as_str()
                ))))
            })
            .clone()
    }
}

/// Every `BuiltinClass` -- including `Object` itself -- gets the same shared
/// placeholder class object as its own `.class`, tagged
/// `BuiltinClass::Object`: there is no reified metaclass hierarchy (the
/// §3.2 deviation in `docs/design/object-model.md`, "Class sides and
/// metaclasses").
///
/// Every non-`Object` builtin's superclass below is `Object` directly, a
/// simplification of the real class hierarchy
/// (`newspeak-spec.pdf` places `Boolean` above `True`/`False`, for
/// instance): `builtins::boolean` must not rely on `True`/`False`
/// actually inheriting from a `Boolean` superclass node, since they do not
/// yet. Fix this only if a corpus module or fixture test needs real
/// `Boolean`/`True`/`False` inheritance to observe correct behavior.
fn build_builtin_classes(interp: &mut Interpreter) -> Vec<ClassRef> {
    // NOTE: deliberately *not* `use BuiltinClass::*;` here --
    // `BuiltinClass::Object` and `crate::types::Object`
    // (the object-model struct, brought into scope by `use crate::types::*;`
    // above) share the bare name `Object`. A glob-import of the enum's
    // variants shadows the struct, so any bare `Object { .. }` literal
    // elsewhere in this function would silently try to build a
    // `BuiltinClass::Object` unit variant with named fields instead of a
    // `types::Object` struct literal -- a compile error the type checker
    // catches, but only because the fields don't match; a differently-shaped
    // `Object` struct could have made this shadow compile into the wrong
    // construction entirely. Referring to every variant through its
    // `BuiltinClass::` qualifier avoids the collision.
    let kinds = [
        BuiltinClass::Object,
        BuiltinClass::Boolean,
        BuiltinClass::True,
        BuiltinClass::False,
        BuiltinClass::Integer,
        BuiltinClass::Float,
        BuiltinClass::Str,
        BuiltinClass::Sym,
        BuiltinClass::Character,
        BuiltinClass::Array,
        BuiltinClass::OrderedCollection,
        BuiltinClass::Nil,
        BuiltinClass::Block,
        BuiltinClass::Platform,
    ];
    // A builtin's behaviour is native (`builtins::dispatch_native`), so its
    // mixin declares nothing: the application exists to give the builtin the
    // same shape every other class object has, and -- for the non-`Object`
    // builtins -- to link an application chain parallel to the class-object
    // chain, which `dispatch::next_level` steps in lockstep.
    let object_application = interp.apply_mixin(&builtin_mixin(BuiltinClass::Object), None);
    let class_of_class =
        build_self_referential_object_class(&mut interp.heap, object_application.clone());
    let mut classes = Vec::with_capacity(kinds.len());
    for kind in kinds {
        let (superclass, application) = if kind == BuiltinClass::Object {
            (None, object_application.clone())
        } else {
            (
                Some(class_of_class),
                interp.apply_mixin(&builtin_mixin(kind), Some(&object_application)),
            )
        };
        let obj = interp.heap.alloc_object(
            class_of_class,
            ObjectData::Class(ClassData {
                application,
                enclosing: None,
                declaring_class: None,
                superclass,
                builtin: Some(kind),
                metaclass: None,
            }),
        );
        // `builtin_class` indexes by discriminant, so the vector must be
        // filled in exactly that order.
        debug_assert_eq!(classes.len(), kind as usize);
        // `Object`'s entry is the self-referential class every builtin has as
        // its `.class`; the object this iteration allocated for it is never
        // used, and is kept only because removing it would shift every later
        // identity hash.
        classes.push(if kind == BuiltinClass::Object {
            class_of_class
        } else {
            obj
        });
    }
    classes
}

/// The empty mixin behind one `BuiltinClass`. A fresh `Rc` per call, so the
/// application cache keys the builtins apart from each other even though
/// their mixins are structurally identical.
fn builtin_mixin(kind: BuiltinClass) -> Rc<crate::mixin::Mixin> {
    Rc::new(crate::mixin::Mixin::empty(Symbol::intern(&format!(
        "{kind:?}"
    ))))
}

/// Builds the single, genuinely self-referential `Object`-tagged class
/// object every `BuiltinClass` shares as its own `.class`, which is what
/// lets `Object.class` stay non-optional. The construction itself lives in
/// `Heap::alloc_self_referential_class`; under the arena it is ordinary
/// safe Rust, because the slot's index is known before the slot is filled.
fn build_self_referential_object_class(
    heap: &mut Heap,
    application: Rc<crate::mixin::MixinApplication>,
) -> ClassRef {
    heap.alloc_self_referential_class(ObjectData::Class(ClassData {
        application,
        enclosing: None,
        declaring_class: None,
        superclass: None,
        builtin: Some(BuiltinClass::Object),
        metaclass: None,
    }))
}

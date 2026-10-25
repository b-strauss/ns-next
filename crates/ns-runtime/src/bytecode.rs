//! The bytecode instruction set and compiled-code data shapes. Pure data -- see
//! `docs/design/compiler.md`'s "The instruction set" section. `Instruction` is
//! a plain Rust enum, `match`-dispatched by `vm.rs`; "architecture-neutral,
//! versioned" describes `image::encode`'s wire format for a `CompiledCode`, not
//! this in-memory type.

use std::ops::Range;
use std::rc::Rc;

use ns_syntax::Span;

use crate::Symbol;

mod compact;
pub(crate) mod leftovers;

pub(crate) use compact::CreatedBlock;
pub use compact::{
    BlockSite, CodeBuilder, CompactInstruction, Emitted, EncodedBody, MirrorSite, SendSite,
    SuperclassSite, WideSend,
};

/// A compiled instruction-set literal -- a lowered form of `ast::Literal` that
/// keeps only what execution needs. Int/float text is parsed once at compile
/// time; `String`/`Symbol`/`Array` still allocate a fresh heap
/// object each time they execute (a literal string is a fresh `Object` on every
/// evaluation).
///
/// `LargeInteger` is an integer literal beyond a `SmallInteger`'s 61 bits
/// (§5.1.1 bounds none), and never holds one that fits: that is a
/// `SmallInteger`. Its digits live in the code, which holds no handle and so
/// survives every move and every heap;
/// `LoadLiteral` allocates the `LargeInteger` object each time it runs, which
/// is sound because a value has no identity.
///
/// `UnresolvedIdentifier` is the one unbound name a *resolved* module can
/// still carry: an identifier with no binding -- a module root's clause
/// receiver, which the root rule decides and which never runs (see
/// `interpreter::literals::unresolved_identifier_error`). Compilation cannot
/// raise a `NewspeakError`, so it is deferred: it is no lowered
/// `ast::Literal` but the payload of a `LoadLiteral` that raises instead of
/// producing a value.
///
/// `BodyTooLarge` is the other deferral: a body past one of the compiler's
/// limits -- more registers than a `u16` names, more than `ARGUMENT_LIMIT`
/// arguments taken or sent, more than `INSTRUCTION_LIMIT` instructions --
/// compiles to a body that loads it first and raises
/// `interpreter::literals::body_too_large_error`, which names the limit
/// (`docs/design/compiler.md`, "Compilation cannot fail", "Limits"), so a
/// body never run never fails.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Nil,
    Boolean(bool),
    SmallInteger(i64),
    Float(f64),
    String(String),
    Symbol(Symbol),
    LargeInteger(crate::ArbitraryInteger),
    UnresolvedIdentifier(String),
    BodyTooLarge(BodyLimit),
}

/// The limit a body that compiled to `Literal::BodyTooLarge` passed, which
/// its error names (`docs/design/compiler.md`, "Limits"). When a body
/// passes several, it is the first its lowering met; a body whose named
/// locals do not fit the registers is judged before anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyLimit {
    /// It needs more registers than a `u16` numbers.
    Registers,
    /// It takes, or one of its sends or clause messages passes, more than
    /// `ARGUMENT_LIMIT` arguments.
    Arguments,
    /// It holds more than `INSTRUCTION_LIMIT` instructions.
    Instructions,
}

/// The most instructions one body holds (`docs/design/compiler.md`,
/// "Limits"). Past it a body compiles to `Literal::BodyTooLarge`, and an
/// image's body is refused where its count is read. It keeps every
/// instruction index, jump target and table index within 20 bits, and
/// bounds the work any pass over one body does.
pub(crate) const INSTRUCTION_LIMIT: u32 = 1 << 20;

/// The most parameters a method, block or factory takes, and the most
/// arguments a send or a clause message passes (`docs/design/compiler.md`,
/// "Limits"). Past it a body compiles to `Literal::BodyTooLarge`; the image
/// encoder refuses to write such a body and the decoder refuses to read
/// one, so that every program that runs from source can be packaged. An
/// argument count fits a byte.
pub(crate) const ARGUMENT_LIMIT: u16 = 255;

/// One send's identity within its code family (`docs/design/dispatch.md`,
/// "Code families"): its number among every send of one root body and the
/// blocks nested in it, dense from 0. `UNCACHED` is what the reference
/// lowering emits, so every send it compiles runs the full lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSite {
    pub number: u32,
}

impl CallSite {
    pub const UNCACHED: CallSite = CallSite { number: u32::MAX };
}

/// The integer leaves the compiler emits inline, guarded, with the ordinary
/// send as the fallback: every binary member of `IntegerPrimitive`. Each one
/// is computed with a `checked_*` operation and a range test, so the cases
/// that have no `SmallInteger` result -- one that does not fit 61 bits, a
/// zero divisor -- answer `None` and take the fallback, leaving the
/// `LargeInteger` promotion and every error and its text to
/// `builtins/numbers/integer.rs` and `builtins/numbers/tower.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegerOperation {
    Add,
    Subtract,
    Multiply,
    FloorDivide,
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
    Equal,
}

/// Every register operand is a `u16` index into the frame's unified
/// arguments-then-temporaries register file; every jump target is an instruction
/// index into the owning `CompiledCode`'s instructions.
///
/// **A send's arguments are contiguous slots.** Every send and every
/// initializer invocation but `InitializeInstance` names its arguments by
/// `argument_base` and `argument_sources`: the arguments are the registers
/// `argument_base .. argument_base + argument_sources.len()`, and before it
/// decides, the instruction copies `argument_sources[index]` into
/// `argument_base + index` wherever the two differ -- a named local passed
/// as an argument is copied by the send itself
/// (`docs/design/execution.md`, "Planned: overlapping register windows").
/// `InitializeInstance` names `0 .. argument_count`, its body's own
/// arguments.
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    LoadLiteral {
        destination: u16,
        literal: Literal,
    },
    LoadSelf {
        destination: u16,
    },
    /// `Binding::EnclosingObject`: `outer N` with no message after it, the
    /// kth enclosing object (`dispatch::kth_enclosing_object`), `depth` out
    /// from the running method's defining class.
    LoadEnclosingObject {
        destination: u16,
        depth: u16,
    },
    Move {
        destination: u16,
        source: u16,
    },
    /// `Binding::Local` write. A read needs no instruction -- the local's
    /// register *is* its value.
    StoreLocal {
        source: u16,
        slot: u16,
    },
    /// `Binding::OuterLocal` read/write through the `captured_frame` chain.
    LoadOuterLocal {
        destination: u16,
        depth: u16,
        index: u16,
    },
    StoreOuterLocal {
        source: u16,
        depth: u16,
        index: u16,
    },
    /// `Binding::LexicalSend`: kth-enclosing-object + private/`lookupProtected`.
    /// No receiver register -- the receiver is `self` by construction.
    SendLexical {
        depth: u16,
        selector: Symbol,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    /// An ordinary, explicit-receiver send (`lookupPublic`).
    SendPublic {
        selector: Symbol,
        receiver_register: u16,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    /// An ordinary explicit-receiver send, exactly `SendPublic`, emitted only
    /// by the reference lowering and only for the sends the optimized lowering
    /// inlines (`control_flow::recognize`). The mark is for the depth count: a
    /// control-flow step's block activated for it is not counted,
    /// because the optimized lowering never activates that block at all
    /// (`docs/design/compiler.md`, "How the limit counts").
    SendInlinable {
        selector: Symbol,
        receiver_register: u16,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    /// `Binding::SuperSend`: receiver is `self`, lookup starts at the
    /// current activation's `current_class`'s superclass.
    SendSuper {
        selector: Symbol,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    /// A send to the VM mirror whose failure block was lowered inline, at
    /// `failure` (`docs/design/compiler.md`, "Mirror sends"). Emitted only
    /// by the optimized lowering, only for a `Binding::MirrorSend`, so only
    /// into `Kernel.ns`, which no image carries -- it has no wire tag.
    ///
    /// `primitive` is the row the selector names, bound at compile time:
    /// the mirror's class and method table are built once from the same
    /// table, and nothing can add to or override them. The run-time check
    /// that `mirror_register` holds the mirror is what stands for the
    /// lookup (`interpreter::mirror::send_inlined`). `subject_register` is
    /// the kernel method's `self`, the slots at `argument_base` the leaf's
    /// own arguments; the failure block is not an argument, because it is
    /// code here, not a value.
    ///
    /// Reads and writes only registers of the activation executing it, so
    /// its `outer_reach_of` reach is 0.
    SendMirror {
        primitive: crate::builtins::primitive::MirrorPrimitive,
        mirror_register: u16,
        subject_register: u16,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
        failure: u32,
    },
    /// Builds a closure over the *compiled* block body. `captured_frame` is
    /// a runtime value the VM supplies from its current activation when the
    /// instruction executes.
    MakeClosure {
        destination: u16,
        parameters: Rc<Vec<Symbol>>,
        code: Rc<CompiledCode>,
        /// The registers whose values the closure is handed, in the order
        /// the block's own registers take them
        /// (`docs/design/execution.md`, "Converted representation"). Always
        /// empty under the reference lowering, and under the optimized one
        /// for a fallback closure, both of which reach every variable by
        /// reference.
        copied: Vec<u16>,
    },
    MakeArray {
        destination: u16,
        element_registers: Vec<u16>,
    },
    /// Builds this body's indirection vector: the `Array` its own
    /// captured-and-written locals live in, and which every block that needs
    /// one of them is handed a reference to
    /// (`docs/design/execution.md`, "Converted representation"). Emitted
    /// once, before the body's first statement, because a local slot's own
    /// initializer already writes through it.
    MakeVector {
        destination: u16,
        size: u16,
    },
    /// Reads one slot of an indirection vector built by `MakeVector`.
    /// `vector` must hold the `Array` the compiler built it as -- checked at
    /// run time, since a vector's size is a run-time value
    /// (`interpreter::locals::load_vector`).
    LoadVector {
        destination: u16,
        vector: u16,
        index: u16,
    },
    /// Writes one slot of an indirection vector; the mirror of `LoadVector`
    /// (`interpreter::locals::store_vector`).
    StoreVector {
        vector: u16,
        index: u16,
        source: u16,
    },
    /// Writes `self`'s instance field at the activation's slot base plus
    /// `own_index` -- emitted by instance-initializer compilation for a
    /// slot's own initializer expression, and by a lazy slot's getter
    /// (§6.3.3). `own_index` is the slot's position among the
    /// *mixin's own* slots (declaration order), never an absolute field
    /// index: the compiled initializer is shared by every application of
    /// its mixin (it lives on the mixin, not on a class object -- §6.2),
    /// and different applications legitimately have
    /// different absolute offsets for the same mixin's own slots depending
    /// on how many slots their own superclass contributes. The VM
    /// resolves the absolute index at run time via the activation's slot
    /// base (`Heap::activation_slot_base`): the own
    /// `MixinApplication.own_slot_base` of the application it was entered
    /// at, which is the level an initializer activation initializes -- one activation per
    /// level, so a base never describes a level other than its own -- and a
    /// lazy slot's getter's defining class. With `ReturnSlotIfFilled`, one of
    /// the two layout-dependent instructions (a class-shape-change flush must
    /// invalidate compiled initializers: `docs/design/liveness-and-reflection.md`,
    /// "Caches and their dependency keys") -- layout-dependent on the
    /// mixin's own slot order, not on the whole hierarchy's.
    InitializeSlot {
        source: u16,
        own_index: u32,
    },
    /// A lazy slot's getter's first instruction (§6.3.3): reads `self`'s
    /// field at the activation's slot base plus `own_index`, as
    /// `InitializeSlot` addresses it, and returns it from the method when it
    /// holds anything but `nil`; falls through to the initializer when it
    /// holds `nil`, which §6.3.3 reads as "not yet computed". A filled read
    /// therefore costs an activation. It was a `MethodFastPath` answered
    /// without one, but that arm, inlined into every send site of the run
    /// loop, taxed every method send for a rare feature
    /// (`docs/design/execution.md`, "What the platform module cost"); an
    /// instruction costs nothing to a send that never runs it.
    ReturnSlotIfFilled {
        own_index: u32,
    },
    /// §6.3.2: a new unresolved future of the closure in `closure`, one per
    /// initializer of a simultaneous slot clause
    /// (`docs/design/object-model.md`, "Simultaneous slots").
    MakeFuture {
        destination: u16,
        closure: u16,
    },
    /// The value `source` holds, seen through a future: itself when it is
    /// none, a resolved future's value, and an unresolved one forced first --
    /// the instruction then runs again (`interpreter::future`,
    /// `AfterForcing::Reexecute`). A simultaneous clause's end.
    ResolveFuture {
        destination: u16,
        source: u16,
    },
    /// `self`'s field at the activation's slot base plus `own_index`, as
    /// `InitializeSlot` addresses it: a simultaneous clause's end reads each
    /// slot back without a getter send, which a subclass could override.
    LoadOwnSlot {
        destination: u16,
        own_index: u32,
    },
    /// `InitializeSlot` without §3.1.1's check of a value's slot: the future
    /// a simultaneous clause stores first, which its end replaces with the
    /// value through an ordinary, checked `InitializeSlot`.
    InitializeSlotProvisionally {
        source: u16,
        own_index: u32,
    },
    /// A method or initializer body's `^`.
    Return {
        register: u16,
    },
    /// A body that fell off its end: a method returns `self`, a block its
    /// last expression's value. Treated by the VM exactly like `Return`;
    /// kept distinct so `disasm` output stays legible.
    ImplicitReturn {
        register: u16,
    },
    /// A `^` inside a block body -- unwinds to the activation's `home` via
    /// the shared `Mutator::raise_return`.
    NonLocalReturn {
        register: u16,
    },
    /// The guard-failure path of an inlined `whileTrue:`/`whileFalse:`:
    /// signals `NonBooleanReceiver` for the value in `register`, which the
    /// condition answered. Ends its path: nothing after it runs.
    SignalNonBoolean {
        register: u16,
    },
    /// Unconditional jump. Emitted only by control-flow inlining.
    Jump {
        target: u32,
    },
    /// Control-flow inlining's runtime guard: jumps if `register` does not hold
    /// a boolean at all.
    JumpIfNotBoolean {
        register: u16,
        target: u32,
    },
    /// Jumps if `register` holds exactly `false`; falls through
    /// for anything else (a `JumpIfNotBoolean` on the same register always
    /// precedes it in inlined code).
    BranchIfFalse {
        register: u16,
        target: u32,
    },
    /// Symmetric to `BranchIfFalse`; used for `whileFalse:`'s exit.
    BranchIfTrue {
        register: u16,
        target: u32,
    },
    /// An integer leaf's inlined fast path. Performs `operation` on `left` and
    /// `right` into `destination` when both hold a `SmallInteger` and the
    /// result fits a `SmallInteger`; otherwise jumps to `target`, where the
    /// ordinary send stands. That send's primitive promotes a result that
    /// does not fit to a `LargeInteger` and raises every error -- a
    /// non-integer operand, a zero divisor, a user subclass -- never this
    /// instruction, so there is one source for each of them.
    ///
    /// Reads and writes only registers of the activation executing it -- never
    /// another frame's by reference -- so its `outer_reach_of` reach is 0.
    IntegerBinary {
        operation: IntegerOperation,
        destination: u16,
        left: u16,
        right: u16,
        target: u32,
    },
    /// A factory body's allocation: a new instance of `self`, the class
    /// object the factory was sent to, into `destination`, after every
    /// check that refuses such an instance.
    ///
    /// Reads and writes only registers of the activation executing it, so
    /// its `outer_reach_of` reach is 0.
    AllocateInstance {
        destination: u16,
    },
    /// A factory body's initialization: runs the initializer of `self`'s
    /// level, the class object the factory was sent to, on the instance in
    /// `instance`, with the factory's arguments -- the executing
    /// activation's own argument registers, `0..argument_count` of its
    /// body, which it names by no list, so that a factory body's size does
    /// not grow with its arity; `destination` takes what it answers.
    ///
    /// Reads and writes only registers of the activation executing it, so
    /// its `outer_reach_of` reach is 0.
    InitializeInstance {
        instance: u16,
        destination: u16,
    },
    /// §6.1's check for a `<:` clause: the level the clause adds, `steps`
    /// above the activation's class, has a primary factory of `selector`.
    /// Writes no register.
    CheckAppliedClause {
        steps: u16,
        selector: Symbol,
    },
    /// An initializer's superclass initialization (§6.3): §6.1's check that
    /// the class `steps` above the activation's class has a primary
    /// factory of `selector`, then that class's initializer on `self`, with
    /// the clause's arguments in the slots at `argument_base`;
    /// `destination` takes what it answers.
    ///
    /// Reads and writes only registers of the activation executing it, so
    /// its `outer_reach_of` reach is 0.
    InitializeSuperclass {
        steps: u16,
        selector: Symbol,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
    },
    /// An initializer's initialization of a level a `<:` clause adds
    /// (§6.2): the own part of the level `steps` above the activation's
    /// class, on `self`, with the arguments in the slots at
    /// `argument_base`; `destination` takes what it answers.
    ///
    /// Reads and writes only registers of the activation executing it, so
    /// its `outer_reach_of` reach is 0.
    InitializeAppliedLevel {
        steps: u16,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeKind {
    Method,
    Block,
    Initializer,
    /// A class declaration's superclass clause: the expression
    /// `newspeak-spec.pdf` §6.1 calls `ec`, compiled as a zero-argument body
    /// that evaluates it and returns the class object it produced. It is a
    /// code kind of its own rather than a `Method` with no declaration
    /// because the frame it runs in belongs to the *enclosing* class and
    /// carries no factory arguments -- `image::decode` reconciles a decoded
    /// body against what it is attached to by kind, so the two must not be
    /// confusable there.
    SuperclassClause,
}

/// How many register numbers a frame's header takes, between its arguments
/// and the rest of its registers: registers `0 .. argument_count` are the
/// arguments, `argument_count .. argument_count + HEADER_REGISTERS` are
/// never named by an instruction, and the copied values, the temporaries,
/// the vector register and scratch start at `argument_count +
/// HEADER_REGISTERS` (`docs/design/compiler.md`, "Registers"). So a send's
/// slots, at the top of its sender's live registers, are its callee's
/// arguments, the callee's header laid where they end, and every register
/// is still one addition from the frame's start. The frame header's word
/// count (`heap::frames::frame_header::COUNT`), which a compile-time
/// assertion holds it to.
pub const HEADER_REGISTERS: u16 = 13;

#[derive(Debug, Clone)]
pub struct CompiledCode {
    pub kind: CodeKind,
    /// The instructions, stored only in their eight-byte encoding
    /// (`docs/design/execution.md`, "The compact instruction encoding"):
    /// the loop runs the slots, and every tool reads the decoded
    /// view (`instruction`, `instructions`).
    body: EncodedBody,
    pub argument_count: u16,
    /// One past the highest register number, in the order the registers
    /// are laid out: `argument_count`, then the `HEADER_REGISTERS` numbers
    /// of the frame's header, then `copied_count`, then the named
    /// temporaries, then one hidden register for the indirection vector
    /// when the body has vector variables, then the compiler-allocated
    /// scratch registers (`docs/design/compiler.md`, "Registers"). Both
    /// lowerings make it at least `argument_count + HEADER_REGISTERS`, but
    /// for a body past the register ceiling with too many arguments for
    /// that, whose registers are its arguments alone. An activation is
    /// pushed with `registers_above_header` registers above its header.
    pub register_count: u16,
    /// Maps a statement index to the half-open instruction range it compiled
    /// to -- the safepoint boundary, and how an activation's instruction
    /// `program_counter` maps back to the statement it is in. For a method or
    /// a block, the index runs through its local initializers (§3.6, in
    /// declaration order) first, then its `CodeBody.statements`; for an
    /// initializer, each slot initializer then each `initExpr`, in that
    /// order.
    pub statement_program_counter: Vec<(usize, Range<usize>)>,
    /// One past the highest call-site number among this body's own sends
    /// and its nested blocks' `family_size`s; 0 when there is none.
    pub family_size: u32,
    /// How many copied values a block's activation is handed, between its
    /// parameters and its temporaries. Zero for every other kind of body.
    pub copied_count: u16,
    /// The first scratch register: one past the arguments, the header's
    /// numbers, the copied values, the named temporaries and the
    /// indirection vector's register. Every send's argument slots start at
    /// or above it, so a callee laid on them has its header here or
    /// higher, and a block reaching this body's registers by reference
    /// reads and writes only those below it (`image::decode`'s
    /// verification holds an image to both). At most `register_count`.
    pub local_count: u16,
    /// How many frames out the deepest by-reference read or write this body
    /// or a block it creates can reach: `0` for converted code, `1` for a
    /// fallback closure that reads its own creator's registers, one more per
    /// level above that. `MakeClosure` marks the creating activation when
    /// this is non-zero, which is what makes a returning frame keep its
    /// registers (`docs/design/execution.md`, "Converted representation").
    /// Derived from the encoding by `from_builder`, never on the wire.
    ///
    /// **Every instruction that reads or writes another frame's registers by
    /// reference has to be named in `outer_reach_of`, and its eight-byte
    /// form in `compact::narrow_outer_reach`, answering its depth, never
    /// 0.** One that is not leaves the creating activation unmarked, its
    /// frame drops its registers when it returns, and the reads answer `nil`
    /// -- silent wrong output at exit status 0, not a crash. The round-trip
    /// tests' `assert_derived_as_decoded` holds the two lists to each other.
    pub outer_reach: u16,
    /// Whether the body itself -- not a block it creates, which has its own
    /// -- contains an instruction that addresses a slot of the level it was
    /// entered at (`addresses_own_slots_of` lists them). An activation of
    /// such a body records its receiver's guard when it is pushed, and only
    /// when the receiver's chain contains the level it is entered at, so
    /// that a receiver replaced before or during it is never written in the
    /// old layout
    /// (`docs/design/object-model.md`, "what a slot access checks"). Derived
    /// from the encoding by `from_builder`, never on the wire.
    ///
    /// **Every instruction that addresses a slot by `own_index` has to be
    /// named in `addresses_own_slots_of`, and its eight-byte form in
    /// `compact::narrow_addresses_own_slots`**: one that is not runs in an
    /// activation that recorded neither a guard nor an entered level, so
    /// its own-slot check (`interpreter::locals::check_own_slots`) refuses
    /// every object receiver and the instruction always raises. Both
    /// matches are exhaustive.
    pub addresses_own_slots: bool,
    /// Whether this is the body of `Closure>>on:do:`, `ensure:` or
    /// `ifCurtailed:`, and where its parameters and locals live: recognized
    /// only when the kernel is compiled (`compiler::unwind_marker`), so no
    /// other body carries one. The unwinder reads it off each activation's
    /// code as it pops (`interpreter::unwind`). Not on an image's wire, so a
    /// body an image decodes carries none; the embedded shipped code stores
    /// it, so a kernel body decoded from there carries its marker.
    pub unwind_marker: Option<UnwindMarker>,
    pub source: Span,
    /// This body's id in the heap that last asked (`Heap::code_id`), so a
    /// closure or frame made of it finds its id without the heap's map.
    /// Never on any wire; a copy starts empty.
    pub(crate) code_id_cache: crate::heap::CodeIdCache,
    /// Whether this body is a body-less declaration's own code: its full
    /// initializer (`Mixin::has_body`), or a block created in it at any
    /// depth. Such code runs only at the declaration's class object, and
    /// its lexical sends and `outer` start from the declaration's scope
    /// instead of the class object's (`interpreter::sends::lexical_origin`),
    /// so where they start is decided here, once, from the code -- never
    /// per send from the class object. Derived from the mixin the body is
    /// attached to (`Mixin::mark_declaration_code`) whenever a body-less
    /// mixin is built -- compiled, decoded from an image or from the
    /// embedded shipped code -- and never on any wire. A `Cell` because the
    /// decoders attach a body after it was shared.
    declaration_code: std::cell::Cell<bool>,
}

/// Every field of a `CompiledCode`, borrowed (`CompiledCode::fields`).
/// The ones a writer may leave out -- its one, the shipped code's encoder,
/// does -- are here all the same, so that leaving one out is a decision
/// written where it is made.
pub(crate) struct CompiledCodeFields<'code> {
    pub(crate) kind: &'code CodeKind,
    #[expect(dead_code, reason = "named so that a writer decides to leave it out")]
    pub(crate) body: &'code EncodedBody,
    pub(crate) argument_count: &'code u16,
    pub(crate) register_count: &'code u16,
    pub(crate) statement_program_counter: &'code [(usize, Range<usize>)],
    #[expect(dead_code, reason = "named so that a writer decides to leave it out")]
    pub(crate) family_size: &'code u32,
    pub(crate) copied_count: &'code u16,
    pub(crate) local_count: &'code u16,
    #[expect(dead_code, reason = "named so that a writer decides to leave it out")]
    pub(crate) outer_reach: &'code u16,
    #[expect(dead_code, reason = "named so that a writer decides to leave it out")]
    pub(crate) addresses_own_slots: &'code bool,
    pub(crate) unwind_marker: &'code Option<UnwindMarker>,
    pub(crate) source: &'code Span,
    #[expect(dead_code, reason = "named so that a writer decides to leave it out")]
    pub(crate) code_id_cache: &'code crate::heap::CodeIdCache,
    #[expect(dead_code, reason = "named so that a writer decides to leave it out")]
    pub(crate) declaration_code: &'code std::cell::Cell<bool>,
}

impl CompiledCode {
    /// How many registers an activation of this body has above its header:
    /// `register_count` less the arguments and the header's numbers, none
    /// for a body whose registers are its arguments alone.
    #[inline(always)]
    pub fn registers_above_header(&self) -> usize {
        usize::from(self.register_count)
            .saturating_sub(usize::from(self.argument_count) + usize::from(HEADER_REGISTERS))
    }

    /// How many registers a frame object of this body holds: its arguments
    /// and its registers above the header, the header's numbers left out
    /// (`Heap::frame_field_of_register`). Its arguments alone for a body
    /// whose registers are its arguments alone.
    #[inline]
    pub fn frame_register_count(&self) -> usize {
        usize::from(self.argument_count) + self.registers_above_header()
    }

    /// A body from its instructions, encoded at once; `family_size` and
    /// `outer_reach` and `addresses_own_slots` are derived from them, as the
    /// compiler derives them.
    /// It carries no unwind marker: only the kernel's compilation
    /// recognizes one. Its `local_count` is that of a body with no named
    /// temporaries and no indirection vector -- its arguments, the header's
    /// numbers and its copied values, at most `register_count` -- which a
    /// body built by hand with named temporaries sets itself.
    pub fn new(
        kind: CodeKind,
        instructions: Vec<Instruction>,
        argument_count: u16,
        register_count: u16,
        copied_count: u16,
        statement_program_counter: Vec<(usize, Range<usize>)>,
        source: Span,
    ) -> CompiledCode {
        let mut builder = CodeBuilder::new();
        for instruction in instructions {
            builder.emit(instruction);
        }
        let named =
            u32::from(argument_count) + u32::from(HEADER_REGISTERS) + u32::from(copied_count);
        let local_count = u16::try_from(named).unwrap_or(u16::MAX).min(register_count);
        CompiledCode::from_builder(
            kind,
            builder,
            argument_count,
            register_count,
            copied_count,
            local_count,
            statement_program_counter,
            source,
        )
    }

    /// A body from the builder a lowering or the image decoder emitted it
    /// into; `family_size`, `outer_reach` and `addresses_own_slots` are
    /// derived from it, read off its tables as `family_size_of`,
    /// `outer_reach_of` and `addresses_own_slots_of` read the decoded view,
    /// so that building a body decodes none of it.
    // Every field a body is built with but its derived ones, each a
    // different quantity, so a struct of them would only rename the list.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_builder(
        kind: CodeKind,
        builder: CodeBuilder,
        argument_count: u16,
        register_count: u16,
        copied_count: u16,
        local_count: u16,
        statement_program_counter: Vec<(usize, Range<usize>)>,
        source: Span,
    ) -> CompiledCode {
        CompiledCode {
            kind,
            family_size: builder.family_size(),
            outer_reach: builder.outer_reach(),
            addresses_own_slots: builder.addresses_own_slots(),
            body: builder.finish(),
            argument_count,
            register_count,
            copied_count,
            local_count,
            statement_program_counter,
            unwind_marker: None,
            source,
            code_id_cache: crate::heap::CodeIdCache::default(),
            declaration_code: std::cell::Cell::new(false),
        }
    }

    /// Every field, borrowed, for a writer that decides each one's
    /// treatment by destructuring the answer without `..`: `CompiledCode`
    /// itself cannot be destructured so outside this module, since its
    /// `body` is private to keep every body built through `from_builder`.
    pub(crate) fn fields(&self) -> CompiledCodeFields<'_> {
        let CompiledCode {
            kind,
            body,
            argument_count,
            register_count,
            statement_program_counter,
            family_size,
            copied_count,
            local_count,
            outer_reach,
            addresses_own_slots,
            unwind_marker,
            source,
            code_id_cache,
            declaration_code,
        } = self;
        CompiledCodeFields {
            kind,
            body,
            argument_count,
            register_count,
            statement_program_counter,
            family_size,
            copied_count,
            local_count,
            outer_reach,
            addresses_own_slots,
            unwind_marker,
            source,
            code_id_cache,
            declaration_code,
        }
    }

    /// Whether this body is a body-less declaration's own code
    /// (`declaration_code`): what a lexical send and `outer` in it read to
    /// decide where they start.
    #[inline(always)]
    pub(crate) fn is_declaration_code(&self) -> bool {
        self.declaration_code.get()
    }

    /// Marks this body and every block created in it, at any depth, as a
    /// body-less declaration's own code (`declaration_code`). Walks with a
    /// worklist, so a deep nest of blocks recurses nowhere, and passes over
    /// a body already marked, so a block an image names twice is walked
    /// once.
    pub(crate) fn mark_declaration_code(&self) {
        let mut pending: Vec<&CompiledCode> = vec![self];
        while let Some(code) = pending.pop() {
            if code.declaration_code.replace(true) {
                continue;
            }
            pending.extend(code.body.created_blocks().map(|block| &**block.code));
        }
    }

    /// The instruction at `index`, decoded: what was emitted there.
    pub fn instruction(&self, index: usize) -> Instruction {
        self.body.instruction(index)
    }

    /// Every instruction, decoded -- for tools and tests; the loop reads
    /// `slot`.
    pub fn instructions(&self) -> Vec<Instruction> {
        self.body.instructions()
    }

    pub fn instruction_count(&self) -> usize {
        self.body.len()
    }

    /// Whether this is the body a body past the register ceiling compiled
    /// to (`compiler::body_too_large`), with its arity saturated at
    /// `u16::MAX` because it stands for `given` arguments, more than a `u16`
    /// counts. Such a body raises before it reads a register, so it may be
    /// handed all of them.
    pub fn stands_for_too_many_arguments(&self, given: usize) -> bool {
        self.argument_count == u16::MAX
            && given > usize::from(u16::MAX)
            && self.instruction_count() > 0
            && matches!(
                self.instruction(0),
                Instruction::LoadLiteral {
                    literal: Literal::BodyTooLarge(_),
                    ..
                }
            )
    }

    pub(crate) fn slot(&self, index: usize) -> CompactInstruction {
        self.body.slot(index)
    }

    /// Whether the instruction at `index` is stored in a wide form -- a
    /// `Wide` slot, or a send's wide form -- rather than in eight bytes.
    /// For tests that a body past a table's sixteen-bit index really took
    /// the wide form; nothing else needs to tell the forms apart.
    #[doc(hidden)]
    pub fn is_wide(&self, index: usize) -> bool {
        matches!(
            self.body.slot(index),
            CompactInstruction::Wide { .. }
                | CompactInstruction::SendPublicWide { .. }
                | CompactInstruction::SendInlinableWide { .. }
                | CompactInstruction::SendLexicalWide { .. }
                | CompactInstruction::SendSuperWide { .. }
        )
    }

    /// Whether the `NS_NEXT_STRESS_WIDE` stress mode is on, which makes
    /// every instruction of a body built by the compiler take a wide form.
    /// The one reading of that variable, for the tests whose expectation of
    /// a form depends on it.
    #[doc(hidden)]
    pub fn every_instruction_is_wide() -> bool {
        crate::interpreter::safepoint::stress_wide_from_env()
    }

    pub(crate) fn body(&self) -> &EncodedBody {
        &self.body
    }

    pub(crate) fn body_mut(&mut self) -> &mut EncodedBody {
        &mut self.body
    }
}

/// Two bodies are equal when they mean the same: every field but the
/// encoding, and the instructions the encoding decodes to, so a body built
/// narrow equals the same body built by `CodeBuilder::always_wide`. A
/// `Float` literal compares by its bits: the question is whether two bodies
/// are the same, not whether their floats are numerically equal, so a NaN
/// equals the NaN of the same payload and no other, and `0.0` differs from
/// `-0.0` -- what the encoding keeps, and what a program can observe.
impl PartialEq for CompiledCode {
    fn eq(&self, other: &CompiledCode) -> bool {
        // Destructured, so that a field added to `CompiledCode` must be
        // decided here.
        let CompiledCode {
            kind,
            body: _,
            argument_count,
            register_count,
            statement_program_counter,
            family_size,
            copied_count,
            local_count,
            outer_reach,
            addresses_own_slots,
            unwind_marker,
            source,
            code_id_cache: _,
            // Derived from the mixin the body is attached to, not from the
            // body: a body compiled alone and the same body attached to a
            // body-less mixin mean the same instructions.
            declaration_code: _,
        } = self;
        *kind == other.kind
            && *argument_count == other.argument_count
            && *register_count == other.register_count
            && *statement_program_counter == other.statement_program_counter
            && *family_size == other.family_size
            && *copied_count == other.copied_count
            && *local_count == other.local_count
            && *outer_reach == other.outer_reach
            && *addresses_own_slots == other.addresses_own_slots
            && *unwind_marker == other.unwind_marker
            && *source == other.source
            && self.instruction_count() == other.instruction_count()
            && (0..self.instruction_count()).all(|index| {
                self.instruction(index)
                    .is_identical_to(&other.instruction(index))
            })
    }
}

/// What makes an activation matter to the unwinder
/// (`docs/design/core-library.md`, "Exceptions"): each variant
/// names the registers of the kernel method's parameters and locals, in the
/// layout `docs/design/compiler.md`, "Registers" gives a method -- its
/// parameters, then its named temporaries in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnwindMarker {
    /// `on:do:`: the registers holding the exception class, the handler
    /// block and the `handlerActive` local.
    Handler {
        class_register: u16,
        block_register: u16,
        active_register: u16,
    },
    /// `ensure:`: the block to run and the `complete` local that says it ran.
    Ensure {
        block_register: u16,
        complete_register: u16,
    },
    /// `ifCurtailed:`: the block, run only on an unwind, and its `complete`
    /// local.
    IfCurtailed {
        block_register: u16,
        complete_register: u16,
    },
}

/// The OR-pattern naming every instruction variant that carries a call
/// site, parameterized on the binding name so it expands identically inside
/// a `&self` match (binding a `&CallSite`) and a `&mut self` one (binding a
/// `&mut CallSite`) -- Rust's default binding modes pick the right one from
/// the match subject's own mutability. Declared once so `call_site` and
/// `call_site_mut` cannot enumerate the four send variants differently from
/// each other: before this existed, `image::decode`'s numbering pass
/// hand-listed the same four variants a second time, independently of
/// `call_site`'s list, and a fifth call-site-carrying variant added to one
/// list and not the other would decode silently wrong -- `family_size_of`
/// (which reads through `call_site`) would count its sends, but the
/// numbering pass would never assign them a number, so two such sends could
/// share a cache entry and run each other's targets at exit status 0.
macro_rules! call_site_variants {
    ($binding:ident) => {
        Instruction::SendLexical {
            call_site: $binding,
            ..
        } | Instruction::SendPublic {
            call_site: $binding,
            ..
        } | Instruction::SendInlinable {
            call_site: $binding,
            ..
        } | Instruction::SendSuper {
            call_site: $binding,
            ..
        }
    };
}

impl Instruction {
    /// A send's call site; `None` for every other instruction.
    pub fn call_site(&self) -> Option<CallSite> {
        match self {
            call_site_variants!(site) => Some(*site),
            _ => None,
        }
    }

    /// `call_site`'s mutable twin, for the one place that assigns a call
    /// site rather than reads one (`EncodedBody::for_each_emitted_mut`,
    /// through which the image decoder numbers call sites).
    /// Shares `call_site_variants!` with `call_site` above.
    pub fn call_site_mut(&mut self) -> Option<&mut CallSite> {
        match self {
            call_site_variants!(site) => Some(site),
            _ => None,
        }
    }

    /// The argument base and the argument sources of a send or an
    /// initializer invocation that names its argument slots; `None` for
    /// every other instruction, `InitializeInstance` among them, whose
    /// arguments are its body's own. No `_` arm, so that a new instruction
    /// must decide.
    pub fn argument_slots(&self) -> Option<(u16, &[u16])> {
        match self {
            Instruction::SendLexical {
                argument_base,
                argument_sources,
                ..
            }
            | Instruction::SendPublic {
                argument_base,
                argument_sources,
                ..
            }
            | Instruction::SendInlinable {
                argument_base,
                argument_sources,
                ..
            }
            | Instruction::SendSuper {
                argument_base,
                argument_sources,
                ..
            }
            | Instruction::SendMirror {
                argument_base,
                argument_sources,
                ..
            }
            | Instruction::InitializeSuperclass {
                argument_base,
                argument_sources,
                ..
            }
            | Instruction::InitializeAppliedLevel {
                argument_base,
                argument_sources,
                ..
            } => Some((*argument_base, argument_sources)),
            Instruction::LoadLiteral { .. }
            | Instruction::LoadSelf { .. }
            | Instruction::LoadEnclosingObject { .. }
            | Instruction::Move { .. }
            | Instruction::StoreLocal { .. }
            | Instruction::LoadOuterLocal { .. }
            | Instruction::StoreOuterLocal { .. }
            | Instruction::MakeClosure { .. }
            | Instruction::MakeArray { .. }
            | Instruction::MakeVector { .. }
            | Instruction::LoadVector { .. }
            | Instruction::StoreVector { .. }
            | Instruction::InitializeSlot { .. }
            | Instruction::ReturnSlotIfFilled { .. }
            | Instruction::MakeFuture { .. }
            | Instruction::ResolveFuture { .. }
            | Instruction::LoadOwnSlot { .. }
            | Instruction::InitializeSlotProvisionally { .. }
            | Instruction::Return { .. }
            | Instruction::ImplicitReturn { .. }
            | Instruction::NonLocalReturn { .. }
            | Instruction::SignalNonBoolean { .. }
            | Instruction::Jump { .. }
            | Instruction::JumpIfNotBoolean { .. }
            | Instruction::BranchIfFalse { .. }
            | Instruction::BranchIfTrue { .. }
            | Instruction::IntegerBinary { .. }
            | Instruction::AllocateInstance { .. }
            | Instruction::InitializeInstance { .. }
            | Instruction::CheckAppliedClause { .. } => None,
        }
    }

    /// Whether `self` and `other` are the same instruction: `==`, except
    /// that a `Float` literal compares by its bits, so that a NaN's payload
    /// and a zero's sign count (`CompiledCode`'s `PartialEq`).
    pub(crate) fn is_identical_to(&self, other: &Instruction) -> bool {
        match (self, other) {
            (
                Instruction::LoadLiteral {
                    destination: own_destination,
                    literal: Literal::Float(own_value),
                },
                Instruction::LoadLiteral {
                    destination: other_destination,
                    literal: Literal::Float(other_value),
                },
            ) => {
                own_destination == other_destination && own_value.to_bits() == other_value.to_bits()
            }
            _ => self == other,
        }
    }

    /// Sets the target of a jump, of an inlined integer operation's
    /// fallback, or of a mirror send's failure block -- the instructions a
    /// lowering emits before it knows where they go.
    pub(crate) fn set_target(&mut self, target: u32) {
        match self {
            Instruction::Jump {
                target: jump_target,
            }
            | Instruction::JumpIfNotBoolean {
                target: jump_target,
                ..
            }
            | Instruction::BranchIfFalse {
                target: jump_target,
                ..
            }
            | Instruction::BranchIfTrue {
                target: jump_target,
                ..
            }
            // The fast path's own guard: its `target` is where the fallback
            // send stands, patched exactly like a jump's.
            | Instruction::IntegerBinary {
                target: jump_target,
                ..
            }
            // A mirror send's `failure` is where its inlined block stands.
            | Instruction::SendMirror {
                failure: jump_target,
                ..
            } => *jump_target = target,
            other => unreachable!("internal: set_target on a non-jump instruction {other:?}"),
        }
    }
}

/// One past the highest call-site number among `instructions`' sends and
/// the `family_size` of every block they create; 0 when there is none.
/// `UNCACHED` sends are not numbered. The compiler and the decoder both
/// derive a body's through `CompiledCode::from_builder`, off the encoding
/// (`compact`'s `Tables::family_size`, which calls this for the wide
/// table), so the two cannot disagree about a body; the round-trip tests'
/// `assert_derived_as_decoded` hold that derivation to this function over
/// the decoded view.
pub(crate) fn family_size_of(instructions: &[Instruction]) -> u32 {
    instructions
        .iter()
        .map(|instruction| match instruction {
            Instruction::MakeClosure { code, .. } => code.family_size,
            other => match other.call_site() {
                Some(site) if site != CallSite::UNCACHED => site.number + 1,
                _ => 0,
            },
        })
        .max()
        .unwrap_or(0)
}

/// A body's `outer_reach`. A nested block's reach counts one level less from
/// here: the frame it reaches through is the one this body creates. The
/// compiler and the decoder both derive a body's through
/// `CompiledCode::from_builder`, off the encoding: `compact`'s
/// `narrow_outer_reach` for an eight-byte slot, this function for the wide
/// table. The round-trip tests' `assert_derived_as_decoded` hold that
/// derivation to this function over the decoded view.
///
/// **The match below answers every instruction's reach, with no `_` arm, and
/// so does `narrow_outer_reach` for the eight-byte forms**, so that a new
/// instruction must decide. One that reached another frame's registers by
/// reference but answered `0` would leave `MakeClosure` unable to mark the
/// creating activation: the frame would lose its registers when it returns,
/// and every read through it would answer `nil` without a word
/// (`docs/design/execution.md`, "The divorce").
pub(crate) fn outer_reach_of(instructions: &[Instruction]) -> u16 {
    instructions
        .iter()
        .map(|instruction| match instruction {
            Instruction::LoadOuterLocal { depth, .. }
            | Instruction::StoreOuterLocal { depth, .. } => *depth,
            Instruction::MakeClosure { code, .. } => code.outer_reach.saturating_sub(1),
            Instruction::LoadLiteral { .. }
            | Instruction::LoadSelf { .. }
            | Instruction::LoadEnclosingObject { .. }
            | Instruction::Move { .. }
            | Instruction::StoreLocal { .. }
            | Instruction::SendLexical { .. }
            | Instruction::SendPublic { .. }
            | Instruction::SendInlinable { .. }
            | Instruction::SendSuper { .. }
            | Instruction::SendMirror { .. }
            | Instruction::MakeArray { .. }
            | Instruction::MakeVector { .. }
            | Instruction::LoadVector { .. }
            | Instruction::StoreVector { .. }
            | Instruction::InitializeSlot { .. }
            | Instruction::ReturnSlotIfFilled { .. }
            | Instruction::MakeFuture { .. }
            | Instruction::ResolveFuture { .. }
            | Instruction::LoadOwnSlot { .. }
            | Instruction::InitializeSlotProvisionally { .. }
            | Instruction::Return { .. }
            | Instruction::ImplicitReturn { .. }
            | Instruction::NonLocalReturn { .. }
            | Instruction::SignalNonBoolean { .. }
            | Instruction::Jump { .. }
            | Instruction::JumpIfNotBoolean { .. }
            | Instruction::BranchIfFalse { .. }
            | Instruction::BranchIfTrue { .. }
            | Instruction::IntegerBinary { .. }
            | Instruction::AllocateInstance { .. }
            | Instruction::InitializeInstance { .. }
            | Instruction::CheckAppliedClause { .. }
            | Instruction::InitializeSuperclass { .. }
            | Instruction::InitializeAppliedLevel { .. } => 0,
        })
        .max()
        .unwrap_or(0)
}

/// The registers of the executing frame that `instruction` reads. The
/// `index` of an outer-local access names a register of another frame, so
/// it is not one of them; neither are the factory arguments
/// `InitializeInstance` hands on, which are its body's named arguments and
/// which it names by no operand, nor the argument slots a send copies its
/// sources into, which its callee reads, not the send.
///
/// **The match answers every instruction, with no `_` arm**, as
/// `register_written` and `outer_reach_of` do, so that a new instruction
/// must decide what it reads. One that read a register but answered none of
/// it would be read past every check that holds a body to what its
/// callees may have left (`docs/design/compiler.md`, "What a callee
/// leaves").
pub(crate) fn registers_read(instruction: &Instruction) -> impl Iterator<Item = u16> + '_ {
    let (fixed, listed): ([Option<u16>; 2], &[u16]) = match instruction {
        Instruction::Move { source, .. }
        | Instruction::StoreLocal { source, .. }
        | Instruction::StoreOuterLocal { source, .. }
        | Instruction::InitializeSlot { source, .. }
        | Instruction::InitializeSlotProvisionally { source, .. }
        | Instruction::ResolveFuture { source, .. } => ([Some(*source), None], &[]),
        Instruction::MakeFuture { closure, .. } => ([Some(*closure), None], &[]),
        Instruction::Return { register }
        | Instruction::ImplicitReturn { register }
        | Instruction::NonLocalReturn { register }
        | Instruction::SignalNonBoolean { register }
        | Instruction::JumpIfNotBoolean { register, .. }
        | Instruction::BranchIfFalse { register, .. }
        | Instruction::BranchIfTrue { register, .. } => ([Some(*register), None], &[]),
        Instruction::SendPublic {
            receiver_register,
            argument_sources,
            ..
        }
        | Instruction::SendInlinable {
            receiver_register,
            argument_sources,
            ..
        } => ([Some(*receiver_register), None], argument_sources),
        Instruction::SendMirror {
            mirror_register,
            subject_register,
            argument_sources,
            ..
        } => (
            [Some(*mirror_register), Some(*subject_register)],
            argument_sources,
        ),
        Instruction::SendLexical {
            argument_sources, ..
        }
        | Instruction::SendSuper {
            argument_sources, ..
        }
        | Instruction::InitializeSuperclass {
            argument_sources, ..
        }
        | Instruction::InitializeAppliedLevel {
            argument_sources, ..
        } => ([None, None], argument_sources),
        Instruction::MakeArray {
            element_registers, ..
        } => ([None, None], element_registers),
        // The values a converted closure is handed are read out of this
        // frame when the closure is made.
        Instruction::MakeClosure { copied, .. } => ([None, None], copied),
        Instruction::InitializeInstance { instance, .. } => ([Some(*instance), None], &[]),
        Instruction::LoadVector { vector, .. } => ([Some(*vector), None], &[]),
        Instruction::StoreVector { vector, source, .. } => ([Some(*vector), Some(*source)], &[]),
        Instruction::IntegerBinary { left, right, .. } => ([Some(*left), Some(*right)], &[]),
        Instruction::LoadLiteral { .. }
        | Instruction::LoadSelf { .. }
        | Instruction::LoadEnclosingObject { .. }
        | Instruction::LoadOuterLocal { .. }
        | Instruction::MakeVector { .. }
        | Instruction::ReturnSlotIfFilled { .. }
        | Instruction::LoadOwnSlot { .. }
        | Instruction::Jump { .. }
        | Instruction::AllocateInstance { .. }
        | Instruction::CheckAppliedClause { .. } => ([None, None], &[]),
    };
    fixed.into_iter().flatten().chain(listed.iter().copied())
}

/// The register of the executing frame `instruction` writes, if any: its
/// destination, or a `StoreLocal`'s slot. A send's argument slots are not
/// answered (`Instruction::argument_slots` names them), and neither is the
/// destination of a load of `Literal::BodyTooLarge`, which raises before it
/// writes: the body a body past the register ceiling compiles to
/// (`compiler::body_too_large`) names its last argument so when its
/// arguments leave no register above the header.
///
/// **The match answers every instruction, with no `_` arm**, so that a new
/// instruction must decide whether it writes a register.
pub(crate) fn register_written(instruction: &Instruction) -> Option<u16> {
    match instruction {
        Instruction::LoadLiteral {
            literal: Literal::BodyTooLarge(_),
            ..
        } => None,
        Instruction::LoadLiteral { destination, .. }
        | Instruction::LoadSelf { destination }
        | Instruction::LoadEnclosingObject { destination, .. }
        | Instruction::Move { destination, .. }
        | Instruction::LoadOuterLocal { destination, .. }
        | Instruction::SendLexical { destination, .. }
        | Instruction::SendPublic { destination, .. }
        | Instruction::SendInlinable { destination, .. }
        | Instruction::SendSuper { destination, .. }
        | Instruction::SendMirror { destination, .. }
        | Instruction::MakeClosure { destination, .. }
        | Instruction::MakeArray { destination, .. }
        | Instruction::MakeVector { destination, .. }
        | Instruction::LoadVector { destination, .. }
        | Instruction::IntegerBinary { destination, .. }
        | Instruction::MakeFuture { destination, .. }
        | Instruction::ResolveFuture { destination, .. }
        | Instruction::LoadOwnSlot { destination, .. }
        | Instruction::AllocateInstance { destination }
        | Instruction::InitializeInstance { destination, .. }
        | Instruction::InitializeSuperclass { destination, .. }
        | Instruction::InitializeAppliedLevel { destination, .. } => Some(*destination),
        Instruction::StoreLocal { slot, .. } => Some(*slot),
        Instruction::StoreOuterLocal { .. }
        | Instruction::StoreVector { .. }
        | Instruction::InitializeSlot { .. }
        | Instruction::InitializeSlotProvisionally { .. }
        | Instruction::ReturnSlotIfFilled { .. }
        | Instruction::CheckAppliedClause { .. }
        | Instruction::Return { .. }
        | Instruction::ImplicitReturn { .. }
        | Instruction::NonLocalReturn { .. }
        | Instruction::SignalNonBoolean { .. }
        | Instruction::Jump { .. }
        | Instruction::JumpIfNotBoolean { .. }
        | Instruction::BranchIfFalse { .. }
        | Instruction::BranchIfTrue { .. } => None,
    }
}

/// Where `instruction` may go other than to the next instruction: a jump's
/// or a branch's target, the fallback of an inlined integer operation, a
/// mirror send's failure block. `None` for every other instruction.
///
/// **The match answers every instruction, with no `_` arm**, so that a new
/// jump-carrying instruction must say where it goes rather than reach the
/// VM's `program_counter` unchecked by the decoder.
pub(crate) fn jump_target(instruction: &Instruction) -> Option<u32> {
    match instruction {
        Instruction::Jump { target }
        | Instruction::JumpIfNotBoolean { target, .. }
        | Instruction::BranchIfFalse { target, .. }
        | Instruction::BranchIfTrue { target, .. }
        | Instruction::IntegerBinary { target, .. } => Some(*target),
        // Never decoded (it has no wire tag); its target is still one.
        Instruction::SendMirror { failure, .. } => Some(*failure),
        Instruction::LoadLiteral { .. }
        | Instruction::LoadSelf { .. }
        | Instruction::LoadEnclosingObject { .. }
        | Instruction::Move { .. }
        | Instruction::StoreLocal { .. }
        | Instruction::LoadOuterLocal { .. }
        | Instruction::StoreOuterLocal { .. }
        | Instruction::SendLexical { .. }
        | Instruction::SendPublic { .. }
        | Instruction::SendInlinable { .. }
        | Instruction::SendSuper { .. }
        | Instruction::MakeClosure { .. }
        | Instruction::MakeArray { .. }
        | Instruction::MakeVector { .. }
        | Instruction::LoadVector { .. }
        | Instruction::StoreVector { .. }
        | Instruction::InitializeSlot { .. }
        | Instruction::ReturnSlotIfFilled { .. }
        | Instruction::MakeFuture { .. }
        | Instruction::ResolveFuture { .. }
        | Instruction::LoadOwnSlot { .. }
        | Instruction::InitializeSlotProvisionally { .. }
        | Instruction::Return { .. }
        | Instruction::ImplicitReturn { .. }
        | Instruction::NonLocalReturn { .. }
        | Instruction::SignalNonBoolean { .. }
        | Instruction::AllocateInstance { .. }
        | Instruction::InitializeInstance { .. }
        | Instruction::CheckAppliedClause { .. }
        | Instruction::InitializeSuperclass { .. }
        | Instruction::InitializeAppliedLevel { .. } => None,
    }
}

/// Whether control can pass from `instruction` to the instruction after
/// it: every instruction but a return, the signal that ends an inlined
/// loop's guard-failure path, and an unconditional jump.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn falls_through(instruction: &Instruction) -> bool {
    !matches!(
        instruction,
        Instruction::Return { .. }
            | Instruction::ImplicitReturn { .. }
            | Instruction::NonLocalReturn { .. }
            | Instruction::SignalNonBoolean { .. }
            | Instruction::Jump { .. }
    )
}

/// The instructions control can reach next from `instruction`, which
/// stands at `index`: the next one when it falls through, then its jump
/// target (`jump_target`). A send resumes at the next instruction however
/// its callee returns, and an instruction that forces a future runs again
/// from where it stands, so neither adds an edge.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn successors(index: u32, instruction: &Instruction) -> impl Iterator<Item = u32> {
    let next = if falls_through(instruction) {
        index.checked_add(1)
    } else {
        None
    };
    [next, jump_target(instruction)].into_iter().flatten()
}

/// Whether a body itself addresses a slot of the level it was entered at: it
/// contains an instruction that reads or writes the receiver's field at the
/// activation's slot base plus an `own_index`. A block a body creates is a
/// body of its own and does not count.
///
/// **The match below answers every instruction, with no `_` arm, and so does
/// `compact::narrow_addresses_own_slots` for the eight-byte forms**, so that a
/// new instruction must decide. One that addressed a slot by `own_index` but
/// answered `false` would run in an activation that recorded neither a guard
/// nor an entered level, and its own-slot check would refuse every object
/// receiver: the instruction would always raise
/// (`docs/design/object-model.md`, "what a slot access checks").
pub(crate) fn addresses_own_slots_of(instructions: &[Instruction]) -> bool {
    instructions.iter().any(|instruction| match instruction {
        Instruction::InitializeSlot { .. }
        | Instruction::InitializeSlotProvisionally { .. }
        | Instruction::LoadOwnSlot { .. }
        | Instruction::ReturnSlotIfFilled { .. } => true,
        Instruction::LoadLiteral { .. }
        | Instruction::LoadSelf { .. }
        | Instruction::LoadEnclosingObject { .. }
        | Instruction::Move { .. }
        | Instruction::LoadOuterLocal { .. }
        | Instruction::StoreOuterLocal { .. }
        | Instruction::StoreLocal { .. }
        | Instruction::SendLexical { .. }
        | Instruction::SendPublic { .. }
        | Instruction::SendInlinable { .. }
        | Instruction::SendSuper { .. }
        | Instruction::SendMirror { .. }
        | Instruction::MakeClosure { .. }
        | Instruction::MakeArray { .. }
        | Instruction::MakeVector { .. }
        | Instruction::LoadVector { .. }
        | Instruction::StoreVector { .. }
        | Instruction::MakeFuture { .. }
        | Instruction::ResolveFuture { .. }
        | Instruction::Return { .. }
        | Instruction::ImplicitReturn { .. }
        | Instruction::NonLocalReturn { .. }
        | Instruction::SignalNonBoolean { .. }
        | Instruction::Jump { .. }
        | Instruction::JumpIfNotBoolean { .. }
        | Instruction::BranchIfFalse { .. }
        | Instruction::BranchIfTrue { .. }
        | Instruction::IntegerBinary { .. }
        | Instruction::AllocateInstance { .. }
        | Instruction::InitializeInstance { .. }
        | Instruction::CheckAppliedClause { .. }
        | Instruction::InitializeSuperclass { .. }
        | Instruction::InitializeAppliedLevel { .. } => false,
    })
}

/// A human-readable listing of `code` for `ns-next-analyzer disasm`. Nested block
/// code is printed indented under its `MakeClosure`. Pure formatting.
pub fn disassemble(code: &CompiledCode, name: &str) -> String {
    let mut out = String::new();
    write_code(&mut out, code, name, 0);
    out
}

fn write_code(out: &mut String, code: &CompiledCode, name: &str, indent: usize) {
    use std::fmt::Write;
    let pad = " ".repeat(indent);
    let kind = match code.kind {
        CodeKind::Method => "method",
        CodeKind::Block => "block",
        CodeKind::Initializer => "initializer",
        CodeKind::SuperclassClause => "superclass clause",
    };
    writeln!(
        out,
        "{pad}{kind} {name} (args {}, registers {})",
        code.argument_count, code.register_count
    )
    .expect("writing to a String cannot fail");
    for (index, instruction) in code.instructions().iter().enumerate() {
        let statement = code
            .statement_program_counter
            .iter()
            .find(|(_, range)| range.contains(&index))
            .map(|(statement_index, _)| format!("stmt {statement_index}"))
            .unwrap_or_else(|| "-".to_string());
        writeln!(
            out,
            "{pad}{index:4}  {statement:>7}  {}",
            format_instruction(instruction)
        )
        .expect("writing to a String cannot fail");
        if let Instruction::MakeClosure { code: block, .. } = instruction {
            write_code(out, block, "block", indent + 8);
        }
    }
}

fn registers(registers: &[u16]) -> String {
    registers
        .iter()
        .map(|register| format!("r{register}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A send's argument slots as `disasm` prints them: each slot, and where
/// its source differs, the register the send copies into it.
fn argument_slots(argument_base: u16, argument_sources: &[u16]) -> String {
    argument_sources
        .iter()
        .enumerate()
        .map(|(offset, source)| {
            let slot = u32::from(argument_base) + offset as u32;
            if u32::from(*source) == slot {
                format!("r{slot}")
            } else {
                format!("r{slot} <- r{source}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_instruction(instruction: &Instruction) -> String {
    match instruction {
        Instruction::LoadLiteral {
            destination,
            literal,
        } => {
            format!("LoadLiteral r{destination}, {}", format_literal(literal))
        }
        Instruction::LoadSelf { destination } => format!("LoadSelf r{destination}"),
        Instruction::LoadEnclosingObject { destination, depth } => {
            format!("LoadEnclosingObject r{destination}, depth {depth}")
        }
        Instruction::Move {
            destination,
            source,
        } => format!("Move r{destination}, r{source}"),
        Instruction::StoreLocal { source, slot } => format!("StoreLocal r{slot} <- r{source}"),
        Instruction::LoadOuterLocal {
            destination,
            depth,
            index,
        } => {
            format!("LoadOuterLocal r{destination}, depth {depth} index {index}")
        }
        Instruction::StoreOuterLocal {
            source,
            depth,
            index,
        } => {
            format!("StoreOuterLocal depth {depth} index {index} <- r{source}")
        }
        Instruction::SendLexical {
            depth,
            selector,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => format!(
            "SendLexical r{destination} <- depth {depth} #{selector} ({}) {}",
            argument_slots(*argument_base, argument_sources),
            format_call_site(*call_site)
        ),
        Instruction::SendPublic {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => format!(
            "SendPublic r{destination} <- r{receiver_register} #{selector} ({}) {}",
            argument_slots(*argument_base, argument_sources),
            format_call_site(*call_site)
        ),
        Instruction::SendInlinable {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => format!(
            "SendInlinable r{destination} <- r{receiver_register} #{selector} ({}) {}",
            argument_slots(*argument_base, argument_sources),
            format_call_site(*call_site)
        ),
        Instruction::SendSuper {
            selector,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => format!(
            "SendSuper r{destination} <- #{selector} ({}) {}",
            argument_slots(*argument_base, argument_sources),
            format_call_site(*call_site)
        ),
        Instruction::SendMirror {
            primitive,
            mirror_register,
            subject_register,
            argument_base,
            argument_sources,
            destination,
            failure,
        } => {
            // The subject leads the list, as it leads the mirror selector's
            // arguments; the failure block is the target, not an operand.
            let slots = argument_slots(*argument_base, argument_sources);
            let operands = if slots.is_empty() {
                format!("r{subject_register}")
            } else {
                format!("r{subject_register}, {slots}")
            };
            format!(
                "SendMirror r{destination} <- r{mirror_register} #{} ({operands}) else -> {failure}",
                crate::builtins::primitive::mirror_entry(*primitive).selector,
            )
        }
        Instruction::MakeClosure {
            destination,
            parameters,
            copied,
            ..
        } => {
            let names = parameters
                .iter()
                .map(|parameter| format!(":{parameter}"))
                .collect::<Vec<_>>()
                .join(" ");
            // The copied list is appended only when there is one, so a
            // listing of by-reference code reads exactly as it did before
            // closure conversion existed.
            if copied.is_empty() {
                format!("MakeClosure r{destination}, [{names}] ->")
            } else {
                let copied = copied
                    .iter()
                    .map(|register| format!("r{register}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("MakeClosure r{destination}, [{names}] -> copying {copied}")
            }
        }
        Instruction::MakeArray {
            destination,
            element_registers,
        } => format!(
            "MakeArray r{destination}, ({})",
            registers(element_registers)
        ),
        Instruction::MakeVector { destination, size } => {
            format!("MakeVector r{destination}, size {size}")
        }
        Instruction::LoadVector {
            destination,
            vector,
            index,
        } => format!("LoadVector r{destination} <- r{vector}[{index}]"),
        Instruction::StoreVector {
            vector,
            index,
            source,
        } => format!("StoreVector r{vector}[{index}] <- r{source}"),
        Instruction::InitializeSlot { source, own_index } => {
            format!("InitSlot own {own_index} <- r{source}")
        }
        Instruction::ReturnSlotIfFilled { own_index } => {
            format!("ReturnSlotIfFilled own {own_index}")
        }
        Instruction::MakeFuture {
            destination,
            closure,
        } => {
            format!("MakeFuture r{destination} <- future of r{closure}")
        }
        Instruction::ResolveFuture {
            destination,
            source,
        } => {
            format!("ResolveFuture r{destination} <- r{source}")
        }
        Instruction::LoadOwnSlot {
            destination,
            own_index,
        } => {
            format!("LoadOwnSlot r{destination} <- own {own_index}")
        }
        Instruction::InitializeSlotProvisionally { source, own_index } => {
            format!("InitSlotProvisionally own {own_index} <- r{source}")
        }
        Instruction::Return { register } => format!("Return r{register}"),
        Instruction::ImplicitReturn { register } => format!("ImplicitReturn r{register}"),
        Instruction::NonLocalReturn { register } => format!("NonLocalReturn r{register}"),
        Instruction::SignalNonBoolean { register } => format!("SignalNonBoolean r{register}"),
        Instruction::Jump { target } => format!("Jump -> {target}"),
        Instruction::JumpIfNotBoolean { register, target } => {
            format!("JumpIfNotBoolean r{register} -> {target}")
        }
        Instruction::BranchIfFalse { register, target } => {
            format!("BranchIfFalse r{register} -> {target}")
        }
        Instruction::BranchIfTrue { register, target } => {
            format!("BranchIfTrue r{register} -> {target}")
        }
        Instruction::IntegerBinary {
            operation,
            destination,
            left,
            right,
            target,
        } => format!(
            "IntegerBinary {operation:?} r{destination} <- r{left}, r{right} else -> {target}"
        ),
        Instruction::AllocateInstance { destination } => {
            format!("AllocateInstance r{destination}")
        }
        Instruction::InitializeInstance {
            instance,
            destination,
        } => format!("InitializeInstance r{destination} <- r{instance} (arguments)"),
        Instruction::CheckAppliedClause { steps, selector } => {
            format!("CheckAppliedClause steps {steps} #{selector}")
        }
        Instruction::InitializeSuperclass {
            steps,
            selector,
            argument_base,
            argument_sources,
            destination,
        } => format!(
            "InitializeSuperclass r{destination} <- steps {steps} #{selector} ({})",
            argument_slots(*argument_base, argument_sources)
        ),
        Instruction::InitializeAppliedLevel {
            steps,
            argument_base,
            argument_sources,
            destination,
        } => format!(
            "InitializeAppliedLevel r{destination} <- steps {steps} ({})",
            argument_slots(*argument_base, argument_sources)
        ),
    }
}

/// A send's call site as `disasm` prints it: its family number, or `@-` for
/// `UNCACHED` -- the reference lowering's bodies carry no numbering at all.
fn format_call_site(call_site: CallSite) -> String {
    if call_site == CallSite::UNCACHED {
        "@-".to_string()
    } else {
        format!("@{}", call_site.number)
    }
}

fn format_literal(literal: &Literal) -> String {
    match literal {
        Literal::Nil => "nil".to_string(),
        Literal::Boolean(boolean) => boolean.to_string(),
        Literal::SmallInteger(integer) => integer.to_string(),
        Literal::Float(float) => crate::numeric::float_text::shortest(*float),
        Literal::String(string) => format!("'{string}'"),
        Literal::Symbol(symbol) => format!("#{symbol}"),
        Literal::LargeInteger(integer) => format!("large-int {}", integer.to_bounded_text()),
        Literal::UnresolvedIdentifier(name) => format!("unresolved-ident '{name}'"),
        Literal::BodyTooLarge(BodyLimit::Registers) => "body-too-large".to_string(),
        Literal::BodyTooLarge(BodyLimit::Arguments) => "body-too-large arguments".to_string(),
        Literal::BodyTooLarge(BodyLimit::Instructions) => "body-too-large instructions".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_code_construction_smoke_test() {
        let code = CompiledCode::new(
            CodeKind::Method,
            vec![
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: Literal::SmallInteger(42),
                },
                Instruction::Return { register: 1 },
            ],
            1,
            2,
            0,
            vec![(0, 0..2)],
            Span::new(0, 10),
        );
        assert_eq!(code.instruction_count(), 2);
        assert_eq!(code.register_count - code.argument_count, 1);
        assert_eq!(code, code.clone());
    }

    /// A body's reach is the deepest frame anything in it addresses: its own
    /// outer-local depths, and one level less for each block it creates,
    /// since the frame a nested block reaches through is the one this body
    /// creates. Non-zero is what makes `MakeClosure` mark its activation.
    #[test]
    fn outer_reach_counts_a_nested_blocks_reach_one_level_less() {
        let make = |outer_reach, instructions| {
            let code = CompiledCode::new(
                CodeKind::Block,
                instructions,
                0,
                1,
                0,
                vec![],
                Span::new(0, 0),
            );
            assert_eq!(code.outer_reach, outer_reach);
            Rc::new(code)
        };
        let inner = make(
            2,
            vec![
                Instruction::LoadOuterLocal {
                    destination: 0,
                    depth: 2,
                    index: 0,
                },
                Instruction::ImplicitReturn { register: 0 },
            ],
        );
        let creating = vec![
            Instruction::MakeClosure {
                destination: 0,
                parameters: Rc::new(vec![]),
                code: inner,
                copied: Vec::new(),
            },
            Instruction::ImplicitReturn { register: 0 },
        ];
        assert_eq!(outer_reach_of(&creating), 1);

        let converted = make(0, vec![Instruction::ImplicitReturn { register: 0 }]);
        assert_eq!(
            outer_reach_of(&[
                Instruction::MakeClosure {
                    destination: 0,
                    parameters: Rc::new(vec![]),
                    code: converted,
                    copied: Vec::new(),
                },
                Instruction::ImplicitReturn { register: 0 },
            ]),
            0,
            "a block that reaches nothing outer leaves its creator unmarked"
        );

        assert_eq!(
            outer_reach_of(&[
                Instruction::StoreOuterLocal {
                    source: 0,
                    depth: 1,
                    index: 0,
                },
                Instruction::ImplicitReturn { register: 0 },
            ]),
            1
        );
    }

    fn code_of(instructions: Vec<Instruction>) -> CompiledCode {
        CompiledCode::new(
            CodeKind::Method,
            instructions,
            0,
            2,
            0,
            vec![],
            Span::new(0, 0),
        )
    }

    #[test]
    fn a_body_with_an_own_slot_instruction_addresses_its_own_slots() {
        let instructions = [
            Instruction::InitializeSlot {
                source: 0,
                own_index: 0,
            },
            Instruction::InitializeSlotProvisionally {
                source: 0,
                own_index: 0,
            },
            Instruction::LoadOwnSlot {
                destination: 0,
                own_index: 0,
            },
            Instruction::ReturnSlotIfFilled { own_index: 0 },
        ];
        for instruction in instructions {
            let body = vec![
                Instruction::LoadSelf { destination: 0 },
                instruction.clone(),
                Instruction::Return { register: 0 },
            ];
            assert!(addresses_own_slots_of(&body), "{instruction:?}");
            assert!(code_of(body).addresses_own_slots, "{instruction:?}");
        }
    }

    #[test]
    fn a_body_without_one_does_not() {
        let body = vec![
            Instruction::LoadSelf { destination: 0 },
            Instruction::Return { register: 0 },
        ];
        assert!(!addresses_own_slots_of(&body));
        assert!(!code_of(body).addresses_own_slots);
        // A block it creates is another body, with its own flag.
        let block = Rc::new(code_of(vec![Instruction::ReturnSlotIfFilled {
            own_index: 0,
        }]));
        assert!(block.addresses_own_slots);
        let creating = code_of(vec![
            Instruction::MakeClosure {
                destination: 0,
                parameters: Rc::new(vec![]),
                code: block,
                copied: Vec::new(),
            },
            Instruction::Return { register: 0 },
        ]);
        assert!(!creating.addresses_own_slots);
    }

    /// A mirror send lists its mirror selector with the subject leading its
    /// operands, and names where its inlined failure block stands; it has
    /// no call site to print, and reaches no other frame.
    #[test]
    fn a_mirror_send_disassembles_with_its_selector_operands_and_failure_target() {
        let primitive =
            crate::builtins::primitive::mirror_primitive_named("string:concatenate:ifFail:")
                .expect("the mirror has string:concatenate:ifFail:");
        let instruction = Instruction::SendMirror {
            primitive,
            mirror_register: 1,
            subject_register: 2,
            argument_base: 4,
            argument_sources: vec![0],
            destination: 3,
            failure: 4,
        };
        assert_eq!(
            format_instruction(&instruction),
            "SendMirror r3 <- r1 #string:concatenate:ifFail: (r2, r4 <- r0) else -> 4"
        );
        assert_eq!(instruction.call_site(), None);
        assert_eq!(outer_reach_of(&[instruction]), 0);
    }

    /// The instructions of a compiled `new` disassemble as their names and
    /// operands, and carry no call site.
    #[test]
    fn the_instantiation_instructions_disassemble_with_their_operands() {
        let cases = [
            (
                Instruction::AllocateInstance { destination: 2 },
                "AllocateInstance r2",
            ),
            (
                Instruction::InitializeInstance {
                    instance: 2,
                    destination: 3,
                },
                "InitializeInstance r3 <- r2 (arguments)",
            ),
            (
                Instruction::CheckAppliedClause {
                    steps: 1,
                    selector: Symbol::intern("x:"),
                },
                "CheckAppliedClause steps 1 #x:",
            ),
            (
                Instruction::InitializeSuperclass {
                    steps: 2,
                    selector: Symbol::intern("with:"),
                    argument_base: 4,
                    argument_sources: vec![4],
                    destination: 5,
                },
                "InitializeSuperclass r5 <- steps 2 #with: (r4)",
            ),
            (
                Instruction::InitializeAppliedLevel {
                    steps: 3,
                    argument_base: 6,
                    argument_sources: vec![],
                    destination: 6,
                },
                "InitializeAppliedLevel r6 <- steps 3 ()",
            ),
        ];
        for (instruction, text) in cases {
            assert_eq!(format_instruction(&instruction), text);
            assert_eq!(instruction.call_site(), None);
        }
    }

    #[test]
    fn call_site_number_is_copy_and_comparable() {
        let original = CallSite { number: 3 };
        let copy = original;
        assert_eq!(original, copy);
    }

    #[test]
    fn send_instruction_variants_carry_a_call_site() {
        let send = Instruction::SendPublic {
            selector: Symbol::intern("foo:"),
            receiver_register: 0,
            argument_base: 1,
            argument_sources: vec![1],
            destination: 2,
            call_site: CallSite { number: 0 },
        };
        assert!(matches!(
            send,
            Instruction::SendPublic {
                call_site: CallSite { number: 0 },
                ..
            }
        ));
    }

    #[test]
    fn make_closure_carries_compiled_block_code() {
        let block = Rc::new(CompiledCode::new(
            CodeKind::Block,
            vec![Instruction::ImplicitReturn { register: 0 }],
            1,
            1,
            0,
            vec![(0, 0..1)],
            Span::new(0, 5),
        ));
        let make = Instruction::MakeClosure {
            destination: 0,
            parameters: Rc::new(vec![Symbol::intern("x")]),
            code: block,
            copied: Vec::new(),
        };
        let Instruction::MakeClosure { code, .. } = &make else {
            panic!()
        };
        assert_eq!(code.kind, CodeKind::Block);
    }

    #[test]
    fn disassemble_lists_instructions_with_statement_labels_and_nested_blocks() {
        let block = Rc::new(CompiledCode::new(
            CodeKind::Block,
            vec![
                Instruction::LoadOuterLocal {
                    destination: 0,
                    depth: 1,
                    index: 0,
                },
                Instruction::NonLocalReturn { register: 0 },
            ],
            0,
            1,
            0,
            vec![(0, 0..2)],
            Span::new(0, 5),
        ));
        assert_eq!(block.outer_reach, 1);
        let code = CompiledCode::new(
            CodeKind::Method,
            vec![
                Instruction::MakeClosure {
                    destination: 1,
                    parameters: Rc::new(vec![]),
                    code: block,
                    copied: Vec::new(),
                },
                Instruction::SendPublic {
                    selector: Symbol::intern("value"),
                    receiver_register: 1,
                    argument_base: 2,
                    argument_sources: vec![],
                    destination: 2,
                    call_site: CallSite { number: 1 },
                },
                Instruction::LoadSelf { destination: 3 },
                Instruction::ImplicitReturn { register: 3 },
            ],
            1,
            4,
            0,
            vec![(0, 0..2)],
            Span::new(0, 10),
        );
        assert_eq!(code.family_size, 2);
        assert_eq!(code.outer_reach, 0);
        let text = disassemble(&code, "foo:");
        let expected = "\
method foo: (args 1, registers 4)
   0   stmt 0  MakeClosure r1, [] ->
        block block (args 0, registers 1)
           0   stmt 0  LoadOuterLocal r0, depth 1 index 0
           1   stmt 0  NonLocalReturn r0
   1   stmt 0  SendPublic r2 <- r1 #value () @1
   2        -  LoadSelf r3
   3        -  ImplicitReturn r3
";
        assert_eq!(text, expected);
    }

    /// A `MakeClosure` with copied registers names them; one without reads
    /// exactly as it did before closure conversion, which is what keeps
    /// every recorded disassembly of by-reference code where it is.
    #[test]
    fn disassembly_names_a_closures_copied_registers_only_when_it_has_some() {
        let block = Rc::new(CompiledCode::new(
            CodeKind::Block,
            vec![Instruction::ImplicitReturn { register: 0 }],
            0,
            3,
            2,
            vec![(0, 0..1)],
            Span::new(0, 5),
        ));
        let copying = format_instruction(&Instruction::MakeClosure {
            destination: 1,
            parameters: Rc::new(vec![Symbol::intern("x")]),
            code: block.clone(),
            copied: vec![0, 3],
        });
        assert_eq!(copying, "MakeClosure r1, [:x] -> copying r0 r3");
        let plain = format_instruction(&Instruction::MakeClosure {
            destination: 1,
            parameters: Rc::new(vec![Symbol::intern("x")]),
            code: block,
            copied: Vec::new(),
        });
        assert_eq!(plain, "MakeClosure r1, [:x] ->");
    }

    #[test]
    fn format_literal_covers_every_variant() {
        let literals = [
            Literal::Nil,
            Literal::Boolean(true),
            Literal::SmallInteger(-3),
            Literal::Float(2.5),
            Literal::String("s".to_string()),
            Literal::Symbol(Symbol::intern("sym")),
            Literal::LargeInteger(
                crate::ArbitraryInteger::parse("99999999999999999999", 10).unwrap(),
            ),
        ];
        let rendered: Vec<String> = literals.iter().map(format_literal).collect();
        assert_eq!(
            rendered,
            vec![
                "nil",
                "true",
                "-3",
                "2.5",
                "'s'",
                "#sym",
                "large-int 99999999999999999999"
            ]
        );
    }

    /// `disasm` shows a literal a binary names, and a binary is untrusted:
    /// a literal of 100,000 digits would take the quadratic decimal
    /// conversion hours, so it is rendered elided instead.
    #[test]
    fn a_huge_large_integer_literal_renders_elided() {
        let digits = (1..=100_000).collect();
        let literal = Literal::LargeInteger(crate::ArbitraryInteger::from_parts(false, digits));
        assert_eq!(
            format_literal(&literal),
            "large-int <100000 words> 0x186a0000000000001869f...00000000000000020000000000000001"
        );
    }

    /// A body from `instructions`, emitted into `builder`.
    fn built(builder: CodeBuilder, instructions: &[Instruction]) -> CompiledCode {
        let mut builder = builder;
        for instruction in instructions {
            builder.emit(instruction.clone());
        }
        CompiledCode::from_builder(
            CodeKind::Method,
            builder,
            1,
            300,
            0,
            300,
            vec![(0, 0..instructions.len())],
            Span::new(0, 10),
        )
    }

    fn load_float(value: f64) -> Instruction {
        Instruction::LoadLiteral {
            destination: 1,
            literal: Literal::Float(value),
        }
    }

    /// Equality is about what a body means, so the encoding a builder chose
    /// does not enter it -- a NaN literal, sends, a block and a register
    /// past a byte included.
    #[test]
    fn a_body_built_narrow_equals_the_same_body_built_wide() {
        let block = Rc::new(CompiledCode::new(
            CodeKind::Block,
            vec![Instruction::ImplicitReturn { register: 0 }],
            0,
            1,
            0,
            vec![(0, 0..1)],
            Span::new(0, 1),
        ));
        let instructions = [
            load_float(f64::from_bits(0x7ff8_0000_0000_0001)),
            load_float(1.5),
            Instruction::SendPublic {
                selector: Symbol::intern("at:put:"),
                receiver_register: 1,
                argument_base: 2,
                argument_sources: vec![2, 3],
                destination: 4,
                call_site: CallSite { number: 0 },
            },
            Instruction::MakeClosure {
                destination: 5,
                parameters: Rc::new(vec![]),
                code: block,
                copied: vec![],
            },
            Instruction::Move {
                destination: 299,
                source: 1,
            },
            Instruction::Return { register: 1 },
        ];
        let narrow = built(CodeBuilder::never_wide_by_stress(), &instructions);
        let wide = built(CodeBuilder::always_wide(), &instructions);
        assert!(!narrow.is_wide(0) && wide.is_wide(0));
        assert_eq!(narrow, wide);
        assert_eq!(wide, narrow);
    }

    /// A `Float` literal compares by its bits: the same NaN is the same
    /// body, a NaN of another payload or a zero of the other sign is not.
    #[test]
    fn a_float_literal_compares_by_its_bits() {
        let body = |value: f64| {
            built(
                CodeBuilder::never_wide_by_stress(),
                &[load_float(value), Instruction::Return { register: 1 }],
            )
        };
        let payload = f64::from_bits(0x7ff8_0000_0000_0001);
        assert_eq!(body(payload), body(payload));
        assert_ne!(body(payload), body(f64::NAN));
        assert_ne!(body(0.0), body(-0.0));
        assert_ne!(body(1.5), body(2.5));
    }
}

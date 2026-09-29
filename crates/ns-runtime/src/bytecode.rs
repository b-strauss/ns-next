//! The bytecode instruction set and compiled-code data shapes. Pure data -- see
//! `docs/design/compiler.md`'s "The instruction set" section. `Instruction` is
//! a plain Rust enum, `match`-dispatched by `vm.rs`; "architecture-neutral,
//! versioned" describes `image::encode`'s wire format for a `CompiledCode`, not
//! this in-memory type.

use std::ops::Range;
use std::rc::Rc;

use ns_syntax::Span;

use crate::Symbol;

/// A compiled instruction-set literal -- a lowered form of `ast::Literal` that
/// keeps only what execution needs. Int/float text is parsed once at compile
/// time; `String`/`Symbol`/`Array` still allocate a fresh heap
/// object each time they execute (a literal string is a fresh `Object` on every
/// evaluation).
///
/// `LargeInteger` is an integer literal beyond `i64` (§5.1.1 bounds none), and
/// never holds one that fits: that is a `SmallInteger`. Its digits live in the
/// code, which holds no handle and so survives every move and every heap;
/// `LoadLiteral` allocates the `LargeInteger` object each time it runs, which
/// is sound because a value has no identity.
///
/// `UnresolvedIdentifier` is the one program-level error a *resolved* module
/// can still carry: an identifier with no binding -- a module root's clause
/// receiver, which the root rule decides and which never runs (see
/// `interpreter::literals::unresolved_identifier_error`). Compilation cannot
/// raise a `NewspeakError`, so it is deferred: it is no lowered
/// `ast::Literal` but the payload of a `LoadLiteral` that raises instead of
/// producing a value.
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
}

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
/// is computed with a `checked_*` operation, so the cases that have no
/// `i64` result -- one that does not fit, a zero divisor -- answer `None` and
/// take the fallback, leaving the `LargeInteger` promotion and every error
/// and its text to `builtins/numbers.rs`.
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
/// index into the owning `CompiledCode.instructions`.
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
        argument_registers: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    /// An ordinary, explicit-receiver send (`lookupPublic`).
    SendPublic {
        selector: Symbol,
        receiver_register: u16,
        argument_registers: Vec<u16>,
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
        argument_registers: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    /// `Binding::SuperSend`: receiver is `self`, lookup starts at the
    /// current activation's `current_class`'s superclass.
    SendSuper {
        selector: Symbol,
        argument_registers: Vec<u16>,
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
    /// the kernel method's `self`, `argument_registers` the leaf's own
    /// arguments; the failure block is not an argument, because it is code
    /// here, not a value.
    ///
    /// Reads and writes only registers of the activation executing it, so
    /// `outer_reach_of` does not name it.
    SendMirror {
        primitive: crate::builtins::primitive::MirrorPrimitive,
        mirror_register: u16,
        subject_register: u16,
        argument_registers: Vec<u16>,
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
    /// `MixinApplication.own_slot_base` of its `current_class`, which is the
    /// level an initializer activation initializes -- one activation per
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
    /// Unconditional jump. Emitted only by control-flow inlining.
    Jump {
        target: u32,
    },
    /// Control-flow inlining's runtime guard: jumps if `register` does not hold
    /// a `Value::Boolean` at all.
    JumpIfNotBoolean {
        register: u16,
        target: u32,
    },
    /// Jumps if `register` is exactly `Value::Boolean(false)`; falls through
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
    /// `right` into `destination` when both hold `Value::SmallInteger` and the
    /// result fits `i64`; otherwise jumps to `target`, where the ordinary
    /// send stands. A result that does not fit is promoted to a `LargeInteger`,
    /// and every error -- a non-integer operand, a zero divisor, a user
    /// subclass -- is raised, by that send's primitive, never here, so there
    /// is one source for each of them.
    ///
    /// Reads and writes only registers of the activation executing it -- never
    /// another frame's by reference -- so `outer_reach_of` does not name it.
    IntegerBinary {
        operation: IntegerOperation,
        destination: u16,
        left: u16,
        right: u16,
        target: u32,
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
    /// The arguments of a class declaration's superclass clause's deferred
    /// factory message (`7` in `= Base withValue: 7`), compiled as a body
    /// whose `argument_count` is the *declaring* class's own factory parameter
    /// count and whose value is the argument list as a dynamic array.
    /// `newspeak-spec.pdf` §6.2 evaluates that message fresh on every
    /// instantiation, in that instantiation's own initializer context, so
    /// -- unlike `SuperclassClause`, which runs once per enclosing object
    /// and on the enclosing object -- this body sees the subclass's own
    /// factory arguments. A kind of its own for the same reason
    /// `SuperclassClause` is one: `image::decode` tells a decoded body's
    /// purpose by its kind, so the two must not be confusable there.
    SuperclassMessage,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledCode {
    pub kind: CodeKind,
    pub instructions: Vec<Instruction>,
    pub argument_count: u16,
    /// Total register count, in the order the registers are laid out:
    /// `argument_count`, then `copied_count`, then the named temporaries,
    /// then one hidden register for the indirection vector when the body has
    /// vector variables, then the compiler-allocated scratch registers
    /// (`docs/design/compiler.md`, "Registers"). An activation is pushed with
    /// `register_count - argument_count`.
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
    /// How many frames out the deepest by-reference read or write this body
    /// or a block it creates can reach: `0` for converted code, `1` for a
    /// fallback closure that reads its own creator's registers, one more per
    /// level above that. `MakeClosure` marks the creating activation when
    /// this is non-zero, which is what makes a returning frame keep its
    /// registers (`docs/design/execution.md`, "Converted representation").
    /// Derived from the instructions, never on the wire.
    ///
    /// **Every instruction that reads or writes another frame's registers by
    /// reference has to be named in `outer_reach_of`.** One that is not
    /// leaves the creating activation unmarked, its frame drops its
    /// registers when it returns, and the reads answer `nil` -- silent wrong
    /// output at exit status 0, not a crash.
    pub outer_reach: u16,
    /// Whether this is the body of `Block>>on:do:`, `ensure:` or
    /// `ifCurtailed:`, and where its parameters and locals live: recognized
    /// only when the kernel is compiled (`compiler::unwind_marker`), so no
    /// other body carries one. The unwinder reads it off each activation's
    /// code as it pops (`interpreter::unwind`). Never on the wire: a decoded
    /// body carries none.
    pub unwind_marker: Option<UnwindMarker>,
    pub source: Span,
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
    /// site rather than reads one (`image::decode::number_call_sites`).
    /// Shares `call_site_variants!` with `call_site` above.
    pub fn call_site_mut(&mut self) -> Option<&mut CallSite> {
        match self {
            call_site_variants!(site) => Some(site),
            _ => None,
        }
    }
}

/// One past the highest call-site number among `instructions`' sends and
/// the `family_size` of every block they create; 0 when there is none.
/// `UNCACHED` sends are not numbered. The compiler and the decoder both
/// compute it here, so the two cannot disagree about a body.
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
/// compiler and the decoder both compute it here, so the two cannot disagree
/// about a body.
///
/// **The match below has to name every instruction that reaches another
/// frame's registers by reference.** An omitted one contributes `0`, so
/// `MakeClosure` never marks the creating activation, the frame loses its
/// registers when it returns, and every read through it answers `nil`
/// without a word (`docs/design/execution.md`, "The divorce"). Adding such
/// an instruction is therefore a change to this function too.
pub(crate) fn outer_reach_of(instructions: &[Instruction]) -> u16 {
    instructions
        .iter()
        .map(|instruction| match instruction {
            Instruction::LoadOuterLocal { depth, .. }
            | Instruction::StoreOuterLocal { depth, .. } => *depth,
            Instruction::MakeClosure { code, .. } => code.outer_reach.saturating_sub(1),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

/// A human-readable listing of `code` for `ns-next disasm`. Nested block
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
        CodeKind::SuperclassMessage => "superclass clause message",
    };
    writeln!(
        out,
        "{pad}{kind} {name} (args {}, registers {})",
        code.argument_count, code.register_count
    )
    .expect("writing to a String cannot fail");
    for (index, instruction) in code.instructions.iter().enumerate() {
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
            argument_registers,
            destination,
            call_site,
        } => format!(
            "SendLexical r{destination} <- depth {depth} #{selector} ({}) {}",
            registers(argument_registers),
            format_call_site(*call_site)
        ),
        Instruction::SendPublic {
            selector,
            receiver_register,
            argument_registers,
            destination,
            call_site,
        } => format!(
            "SendPublic r{destination} <- r{receiver_register} #{selector} ({}) {}",
            registers(argument_registers),
            format_call_site(*call_site)
        ),
        Instruction::SendInlinable {
            selector,
            receiver_register,
            argument_registers,
            destination,
            call_site,
        } => format!(
            "SendInlinable r{destination} <- r{receiver_register} #{selector} ({}) {}",
            registers(argument_registers),
            format_call_site(*call_site)
        ),
        Instruction::SendSuper {
            selector,
            argument_registers,
            destination,
            call_site,
        } => format!(
            "SendSuper r{destination} <- #{selector} ({}) {}",
            registers(argument_registers),
            format_call_site(*call_site)
        ),
        Instruction::SendMirror {
            primitive,
            mirror_register,
            subject_register,
            argument_registers,
            destination,
            failure,
        } => {
            // The subject leads the list, as it leads the mirror selector's
            // arguments; the failure block is the target, not an operand.
            let operands: Vec<u16> = std::iter::once(*subject_register)
                .chain(argument_registers.iter().copied())
                .collect();
            format!(
                "SendMirror r{destination} <- r{mirror_register} #{} ({}) else -> {failure}",
                crate::builtins::primitive::mirror_entry(*primitive).selector,
                registers(&operands),
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
        Literal::Float(float) => format!("{float:?}"),
        Literal::String(string) => format!("'{string}'"),
        Literal::Symbol(symbol) => format!("#{symbol}"),
        Literal::LargeInteger(integer) => format!("large-int {}", integer.to_decimal_string()),
        Literal::UnresolvedIdentifier(name) => format!("unresolved-ident '{name}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_code_construction_smoke_test() {
        let code = CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: Literal::SmallInteger(42),
                },
                Instruction::Return { register: 1 },
            ],
            argument_count: 1,
            register_count: 2,
            statement_program_counter: vec![(0, 0..2)],
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 10),
        };
        assert_eq!(code.instructions.len(), 2);
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
            Rc::new(CompiledCode {
                kind: CodeKind::Block,
                instructions,
                argument_count: 0,
                register_count: 1,
                statement_program_counter: vec![],
                family_size: 0,
                copied_count: 0,
                outer_reach,
                unwind_marker: None,
                source: Span::new(0, 0),
            })
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
            argument_registers: vec![0],
            destination: 3,
            failure: 4,
        };
        assert_eq!(
            format_instruction(&instruction),
            "SendMirror r3 <- r1 #string:concatenate:ifFail: (r2, r0) else -> 4"
        );
        assert_eq!(instruction.call_site(), None);
        assert_eq!(outer_reach_of(&[instruction]), 0);
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
            argument_registers: vec![1],
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
        let block = Rc::new(CompiledCode {
            kind: CodeKind::Block,
            instructions: vec![Instruction::ImplicitReturn { register: 0 }],
            argument_count: 1,
            register_count: 1,
            statement_program_counter: vec![(0, 0..1)],
            family_size: 0,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 5),
        });
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
        let block = Rc::new(CompiledCode {
            kind: CodeKind::Block,
            instructions: vec![
                Instruction::LoadOuterLocal {
                    destination: 0,
                    depth: 1,
                    index: 0,
                },
                Instruction::NonLocalReturn { register: 0 },
            ],
            argument_count: 0,
            register_count: 1,
            statement_program_counter: vec![(0, 0..2)],
            family_size: 0,
            copied_count: 0,
            outer_reach: 1,
            unwind_marker: None,
            source: Span::new(0, 5),
        });
        let code = CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![
                Instruction::MakeClosure {
                    destination: 1,
                    parameters: Rc::new(vec![]),
                    code: block,
                    copied: Vec::new(),
                },
                Instruction::SendPublic {
                    selector: Symbol::intern("value"),
                    receiver_register: 1,
                    argument_registers: vec![],
                    destination: 2,
                    call_site: CallSite { number: 1 },
                },
                Instruction::LoadSelf { destination: 3 },
                Instruction::ImplicitReturn { register: 3 },
            ],
            argument_count: 1,
            register_count: 4,
            statement_program_counter: vec![(0, 0..2)],
            family_size: 2,
            copied_count: 0,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 10),
        };
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
        let block = Rc::new(CompiledCode {
            kind: CodeKind::Block,
            instructions: vec![Instruction::ImplicitReturn { register: 0 }],
            argument_count: 0,
            register_count: 3,
            statement_program_counter: vec![(0, 0..1)],
            family_size: 0,
            copied_count: 2,
            outer_reach: 0,
            unwind_marker: None,
            source: Span::new(0, 5),
        });
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
}

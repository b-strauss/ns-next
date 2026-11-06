//! The VM's idioms, written once in `ns-assembler`'s operations: what a
//! template, a stub and every later tier emit to touch a frame, call a
//! Rust leaf, poll or leave machine code, so that none of them reaches an
//! encoder and none can drift from another (`docs/design/execution.md`,
//! "The foundation and the one extension point"). Which machine register
//! plays which role is decided here, per ABI ("Fixed registers and the
//! calling convention").

use ns_assembler::aarch64::{
    SP, X0, X1, X19, X2, X20, X21, X22, X23, X24, X25, X26, X27, X28, X29, X3, X4, X5, X6, X7, X8,
    X9,
};
use ns_assembler::x64::{
    DIVIDEND, R10, R11, R12, R13, R14, R15, R8, R9, RAX, RBP, RBX, RCX, RDI, RDX, REMAINDER, RSI,
    RSP,
};
use ns_assembler::{
    Aarch64, Assembled, Assembler, AssemblyError, BranchForm, BranchKind, Condition, Encoder,
    Label, LabelReference, Memory, Operation, Register, Width, X64,
};

use super::stubs::{BlockWord, ExitKind};
use crate::bytecode::IntegerOperation;
use crate::heap::machine_words::{MachineWord, MachineWords};

/// The platform calling convention the stubs and leaf calls cross.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Abi {
    /// Microsoft's x64 convention.
    Windows64,
    /// The System V AMD64 convention of Linux and macOS.
    SystemV,
    /// The Arm 64-bit procedure call standard, AAPCS64, of Linux and of
    /// Apple's platforms, which differ in nothing the JIT relies on: the
    /// encoder never names `x18`, the register Apple reserves.
    Aapcs64,
}

/// `length` bytes that stop the program wherever execution enters them:
/// what a compaction of the code zone writes over the room a moved or
/// freed body left (`jit::zone`), so that an address into it something
/// failed to remap traps at once rather than running stale code, as Cog
/// fills its freed method zone with stop instructions (`stopsFromto`).
pub(crate) fn stop_bytes(abi: Abi, length: usize) -> Vec<u8> {
    let mut code = Vec::with_capacity(length);
    abi.encoding().fill_with_stops(length, &mut code);
    code
}

/// The encoder of an ABI's architecture, which the macro assembler
/// assembles through: one type over every encoder, so that the idioms,
/// the templates and the stubs are written once and the ABI alone decides
/// which machine code they become.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Encoding {
    X64(X64),
    Aarch64(Aarch64),
}

impl Encoder for Encoding {
    fn encode(&self, operation: Operation, code: &mut Vec<u8>) {
        match self {
            Encoding::X64(encoder) => encoder.encode(operation, code),
            Encoding::Aarch64(encoder) => encoder.encode(operation, code),
        }
    }

    fn encode_branch(
        &self,
        kind: BranchKind,
        form: BranchForm,
        distance: i64,
        code: &mut Vec<u8>,
    ) -> bool {
        match self {
            Encoding::X64(encoder) => encoder.encode_branch(kind, form, distance, code),
            Encoding::Aarch64(encoder) => encoder.encode_branch(kind, form, distance, code),
        }
    }

    fn encode_label_reference(
        &self,
        reference: LabelReference,
        distance: i64,
        code: &mut Vec<u8>,
    ) -> bool {
        match self {
            Encoding::X64(encoder) => encoder.encode_label_reference(reference, distance, code),
            Encoding::Aarch64(encoder) => encoder.encode_label_reference(reference, distance, code),
        }
    }

    fn fill_with_stops(&self, length: usize, code: &mut Vec<u8>) {
        match self {
            Encoding::X64(encoder) => encoder.fill_with_stops(length, code),
            Encoding::Aarch64(encoder) => encoder.fill_with_stops(length, code),
        }
    }

    fn stack_slot_bytes(&self) -> i32 {
        match self {
            Encoding::X64(encoder) => encoder.stack_slot_bytes(),
            Encoding::Aarch64(encoder) => encoder.stack_slot_bytes(),
        }
    }

    fn instruction_alignment(&self) -> u32 {
        match self {
            Encoding::X64(encoder) => encoder.instruction_alignment(),
            Encoding::Aarch64(encoder) => encoder.instruction_alignment(),
        }
    }
}

impl Abi {
    /// The convention of the platform this build runs on.
    pub(crate) const fn host() -> Abi {
        if cfg!(target_arch = "aarch64") {
            Abi::Aapcs64
        } else if cfg!(windows) {
            Abi::Windows64
        } else {
            Abi::SystemV
        }
    }

    /// The encoder of this convention's architecture.
    pub(crate) fn encoding(self) -> Encoding {
        match self {
            Abi::Windows64 | Abi::SystemV => Encoding::X64(X64),
            Abi::Aapcs64 => Encoding::Aarch64(Aarch64),
        }
    }

    /// The bytes a return address, or a pushed register, takes on the
    /// native stack under this convention: its encoder's stack slot
    /// (`Encoder::stack_slot_bytes`), as a constant, since the room an
    /// entry into machine code checks for is one (`jit::NATIVE_STACK_NEED`).
    pub(crate) const fn stack_slot_bytes(self) -> i32 {
        match self {
            Abi::Windows64 | Abi::SystemV => 8,
            Abi::Aapcs64 => 16,
        }
    }

    /// The registers a callee of this convention preserves, in the order
    /// the entry stub pushes them. Under AAPCS64 `x29`, the frame pointer,
    /// is among them though no role takes it: Apple requires it to address
    /// a valid frame record at all times, so machine code never writes it
    /// and the save is only the convention's due.
    pub(crate) fn callee_saved(self) -> &'static [Register] {
        match self {
            Abi::Windows64 => &[RBX, RBP, RDI, RSI, R12, R13, R14, R15],
            Abi::SystemV => &[RBX, RBP, R12, R13, R14, R15],
            Abi::Aapcs64 => &[X19, X20, X21, X22, X23, X24, X25, X26, X27, X28, X29],
        }
    }

    /// The register a function of this convention takes its first
    /// argument in.
    pub(crate) fn first_argument(self) -> Register {
        match self {
            Abi::Windows64 => RCX,
            Abi::SystemV => RDI,
            Abi::Aapcs64 => X0,
        }
    }
}

/// The roles machine registers play while a body runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Roles {
    /// The absolute address of the frame's register 0.
    pub(crate) frame: Register,
    /// The entry block's address.
    pub(crate) context: Register,
    /// The object space's base address.
    pub(crate) object_space: Register,
    /// The idioms' own scratch register, which no template keeps a value
    /// in across an idiom.
    pub(crate) idiom_scratch: Register,
    /// Where a leaf call leaves the leaf's answer.
    pub(crate) leaf_result: Register,
    /// The registers templates may use freely; none survives a leaf call
    /// or an exit.
    pub(crate) scratch: &'static [Register],
    /// The scratch register a division divides and leaves its quotient
    /// in, where the encoder fixes it (`Assembler::divide_signed`).
    pub(crate) dividend: Register,
    /// The scratch register a division leaves its remainder in.
    pub(crate) remainder: Register,
    /// The registers a send's argument placement holds the moved values
    /// in, every source loaded before any slot is stored; the first is
    /// also the one a placement made move after move goes through. None
    /// is `callee_receiver` or `callee_class`, which hold the send's
    /// receiver, and a `super` send's S, across the placement.
    pub(crate) placement: [Register; 6],
    /// A send's cells' address (`BlockWord::Cells`), from its load before
    /// the guard's compare to the load of the cell's entry, across
    /// `load_guard`, `load_class`, `load_lexical_class` and
    /// `walk_superclasses`; then its site's own cell's address, which a
    /// hit enters the callee with and a miss the polymorphic stub; in the
    /// stubs the block cell or the global row they read -- a row's
    /// address moved on its hit, so that its chain start word lies where a
    /// cell's does -- and 0 after the miss leaf. A private or `super` slot
    /// accessor's stub reads the recorded guard there; the leaf stub hands
    /// it to its leaf, which runs a slot accessor straight from the site's
    /// entry when it is a site's cell.
    pub(crate) send_cells: Register,
    /// The guard a send compares with its cell's: the receiver's, or a
    /// class object's application word; kept into the stubs, which compare
    /// it with a block's cells, and in the megamorphic stub the dispatch
    /// id it keys the global cache with.
    pub(crate) send_guard: Register,
    /// A lexical send's cell's chain start, then the cell's level, which
    /// `walk_superclasses` counts down.
    pub(crate) send_level: Register,
    /// On a send's miss, its key past the guard, which the stubs are
    /// entered with: the selector's index above the rule's two-bit tag,
    /// the tag of `inline_cache::rule_word`; in the megamorphic stub the
    /// selector alone, once the tag is split off. The stubs' hit then
    /// counts the level down in it, as `send_level`.
    pub(crate) send_key: Register,
    /// The address a send calls: the polymorphic stub's on a miss, the
    /// cell's entry on a hit, from its load to the call; in the stubs the
    /// multiplier of the megamorphic stub's hash, then the entry it jumps
    /// to.
    pub(crate) send_entry: Register,
    /// The stubs' rule word, as a global row holds it
    /// (`inline_cache::rule_word`): the tag the stubs are entered with in
    /// `send_key`'s low two bits, and for a lexical rule N's dispatch id
    /// above it.
    pub(crate) lookup_rule: Register,
    /// The megamorphic stub's hash of its key, then the group it picks.
    pub(crate) lookup_hash: Register,
    /// In the getter stub, the field its entry names, from the entry to
    /// the field's load: entered after the send, whose level and key it
    /// shares a register with, is done with both.
    pub(crate) accessor_field: Register,
    /// The activation count (`MachineWord::DepthCount`) while a prologue,
    /// a return or the entry stub counts it.
    pub(crate) depth_count: Register,
    /// The end of the frame the prologue pushes, as a byte offset from the
    /// activation stack's base.
    pub(crate) frame_extent: Register,
    /// The prologue's bound on `frame_extent`, the ordinary stack's end.
    pub(crate) stack_end: Register,
    /// The dead register's word a debug build stores into the registers a
    /// frame does not read before it writes them.
    pub(crate) poison: Register,
    /// A return address a stub reads off the native stack and hands a
    /// leaf as its argument; free once the leaf is called.
    pub(crate) return_address: Register,
    /// The native stack pointer the processor's `call` and `ret` move,
    /// which the entry stub saves and the exit stub resets, and through
    /// which a stub reads the return address on top of the stack.
    pub(crate) stack_pointer: Register,
    /// The registers optimized code keeps its values in between two
    /// interrupt points (`jit::optimizing::allocate`), those a leaf call
    /// preserves first. Every one but the first two is a scratch register
    /// some role above takes, which is sound because a role is played
    /// only inside a frame node's template, and every frame node clobbers
    /// every allocatable register: what optimized code holds there it has
    /// written back before, and it reads every register from its frame
    /// word after. So nothing optimized code emits between two frame
    /// nodes -- a simple node's code, a write-back, a reload, a phi's move
    /// -- may take one of them as a temporary: only `idiom_scratch`,
    /// `dividend`, `remainder`, `leaf_result` and the other registers no
    /// value is allocated to. `MacroAssembler::hold_values` lets a debug
    /// build refuse a temporary or a leaf call that breaks it.
    pub(crate) allocatable: &'static [Register],
    /// How many of `allocatable`'s first registers keep their values
    /// across a leaf call (`MacroAssembler::call_leaf`): those the
    /// convention has its callee preserve and the call passes no argument
    /// in. Windows64 preserves `rsi` and `rdi` and SystemV passes the
    /// shim's block and leaf index in them, so the count differs.
    pub(crate) survive_leaf_calls: usize,
    /// The registers a stub or a call is entered with, which the methods
    /// below name.
    entered_with: EnteredWith,
}

/// The registers the exit stub and a compiled call's callee are entered
/// with (`Roles::exit_program_counter` and its neighbours).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct EnteredWith {
    exit_program_counter: Register,
    exit_code_address: Register,
    callee_receiver: Register,
    callee_class: Register,
    callee_closure: Register,
    callee_counted: Register,
    callee_argument: Register,
}

impl Roles {
    /// The roles under `abi`. They are the same under both x64
    /// conventions: the pinned roles sit in registers both have their
    /// callees preserve, so a leaf call keeps them, and the entry stub
    /// saves every general-purpose register either convention preserves,
    /// so a body may use the rest of them freely. It saves no vector
    /// register: Win64 has its callee preserve XMM6-XMM15 as well, so a
    /// template that uses one of those requires the entry stub to save
    /// them first.
    ///
    /// Two registers play no role and are no scratch registers, `r13` and
    /// `rbp`: preserved by both conventions and saved by the entry stub,
    /// they are the free registers a value survives a leaf call in, `r13`
    /// left for road step 9's register allocation: the baseline keeps no
    /// frame register in a machine register (`docs/design/execution.md`,
    /// "The register cache").
    ///
    /// Under AAPCS64 each role takes the register x64's does, one to one
    /// (`rax` `x0`, `rcx` `x1`, `rdx` `x2`, `rsi` `x3`, `rdi` `x4`, `r8`
    /// to `r10` `x5` to `x7`, `r12` `x8`, `r11` `x9`, the pinned `rbx`,
    /// `r14` and `r15` the preserved `x19` to `x21`, and `r13` and `rbp`'s
    /// place `x22` and `x23`), so that every register two roles share
    /// under x64 they share here, and no other. `x24` to `x29` take no
    /// role; the encoder keeps `x16` to `x18` and `x30` for itself.
    pub(crate) fn of(abi: Abi) -> Roles {
        match abi {
            Abi::Windows64 | Abi::SystemV => Roles {
                frame: RBX,
                context: R14,
                object_space: R15,
                idiom_scratch: R11,
                leaf_result: RAX,
                scratch: &[RAX, RCX, RDX, RSI, RDI, R8, R9, R10, R12],
                dividend: DIVIDEND,
                remainder: REMAINDER,
                placement: [RAX, RDX, R8, R9, R10, R12],
                send_cells: R8,
                send_guard: R9,
                send_level: R10,
                send_key: R10,
                send_entry: RCX,
                lookup_rule: RDX,
                lookup_hash: RAX,
                accessor_field: R10,
                depth_count: RCX,
                frame_extent: RAX,
                stack_end: RDX,
                poison: RDX,
                return_address: RSI,
                stack_pointer: RSP,
                allocatable: &[R13, RBP, R12, RSI, RDI, R8, R9, R10],
                survive_leaf_calls: if abi == Abi::Windows64 { 5 } else { 3 },
                entered_with: EnteredWith {
                    exit_program_counter: RCX,
                    exit_code_address: RDX,
                    callee_receiver: RSI,
                    callee_class: RDI,
                    callee_closure: R9,
                    callee_counted: R10,
                    callee_argument: R12,
                },
            },
            Abi::Aapcs64 => Roles {
                frame: X19,
                context: X20,
                object_space: X21,
                idiom_scratch: X9,
                leaf_result: X0,
                scratch: &[X0, X1, X2, X3, X4, X5, X6, X7, X8],
                dividend: X0,
                remainder: X2,
                placement: [X0, X2, X5, X6, X7, X8],
                send_cells: X5,
                send_guard: X6,
                send_level: X7,
                send_key: X7,
                send_entry: X1,
                lookup_rule: X2,
                lookup_hash: X0,
                accessor_field: X7,
                depth_count: X1,
                frame_extent: X0,
                stack_end: X2,
                poison: X2,
                return_address: X3,
                stack_pointer: SP,
                allocatable: &[X22, X23, X24, X25, X26, X27, X28, X3, X4, X5, X6, X7, X8],
                survive_leaf_calls: 7,
                entered_with: EnteredWith {
                    exit_program_counter: X1,
                    exit_code_address: X2,
                    callee_receiver: X3,
                    callee_class: X4,
                    callee_closure: X6,
                    callee_counted: X7,
                    callee_argument: X8,
                },
            },
        }
    }

    /// The register the exit stub takes the exit's kind in: the stub's
    /// own answer, so it needs no move.
    pub(crate) fn exit_kind(self) -> Register {
        self.leaf_result
    }

    /// The register the exit stub takes the exit's program counter in.
    pub(crate) fn exit_program_counter(self) -> Register {
        self.entered_with.exit_program_counter
    }

    /// The register the exit stub takes an address inside the exiting code
    /// in, which names the body an exit leaves (`jit::materialize`).
    pub(crate) fn exit_code_address(self) -> Register {
        self.entered_with.exit_code_address
    }

    /// The register a compiled call hands its callee's receiver in, as the
    /// word its header place 0 stores. The call writes nothing into the
    /// callee's frame: the callee's machine entry stores it there once its
    /// checks have passed (`jit::sends::prologue`), so that a refused call
    /// leaves the sender's registers -- a source of the send among them --
    /// as they were. A scratch register everywhere else.
    pub(crate) fn callee_receiver(self) -> Register {
        self.entered_with.callee_receiver
    }

    /// The register a compiled call hands its callee's current class in,
    /// as the word its header place 1 stores, under `callee_receiver`'s
    /// rule. A scratch register everywhere else.
    pub(crate) fn callee_class(self) -> Register {
        self.entered_with.callee_class
    }

    /// The register a call of a block's body hands the closure in, which
    /// its prologue stores as the frame's operand (`jit::sends::prologue`):
    /// a closure stub's (`jit::targets`) or the closure call stub's, the
    /// only ways into a block's prologue. A scratch register everywhere
    /// else -- the send's guard while a send compares it -- since no cell
    /// names a block's body.
    pub(crate) fn callee_closure(self) -> Register {
        self.entered_with.callee_closure
    }

    /// The register a call of a block's body hands 1 in when the block's
    /// activation counts against the depth limit and 0 when the control
    /// flow exempts it, which its prologue writes into the frame's flags
    /// word, under `callee_closure`'s rule.
    pub(crate) fn callee_counted(self) -> Register {
        self.entered_with.callee_counted
    }

    /// The register a call of a block's body of one parameter hands the
    /// argument it places itself in -- `cull:`'s receiver -- once the
    /// prologue's checks have passed, so that a refused call leaves the
    /// send's slots as they were; the word no value is for a call whose
    /// argument already lies in its slot. Under `callee_closure`'s rule.
    pub(crate) fn callee_argument(self) -> Register {
        self.entered_with.callee_argument
    }

    /// The register a body's dispatching entry hands the shared
    /// dispatching stub the address of the body's dispatching rows in
    /// (`targets::assemble_dispatching_stub`): the send's guard, which the
    /// cell's compare is done with once the call is made.
    pub(crate) fn dispatching_rows(self) -> Register {
        self.send_guard
    }

    /// The register a body's dispatching entry hands the shared
    /// dispatching stub the body's machine entry in, where the stub goes
    /// on when no row matches: the send's key, which only a miss reads.
    pub(crate) fn dispatching_baseline(self) -> Register {
        self.send_key
    }

    /// What a prologue reads after its checks, of what the call handed it
    /// and of what it counted: the machine words' address it stores the
    /// count through, the depth count it is about to store, the receiver,
    /// the current class, and a block's closure, `counted` word and
    /// argument. None is pinned, so the stack's growth from a prologue
    /// (`MacroAssembler::grow_stack_if`) keeps them on the native stack
    /// across its leaf call.
    pub(crate) fn call_handover(self) -> [Register; 7] {
        [
            self.idiom_scratch,
            self.depth_count,
            self.callee_receiver(),
            self.callee_class(),
            self.callee_closure(),
            self.callee_counted(),
            self.callee_argument(),
        ]
    }

    /// The registers optimized code computes a `SmallInteger` fast path in
    /// (`MacroAssembler::small_integer_operation`): the four that no value
    /// is allocated to and no pinned role takes -- the division's two, the
    /// one scratch register besides them that is not allocatable, and the
    /// idioms' own -- so that the operands, which stay in their allocated
    /// registers for the fallback, are copied in and nothing a value is
    /// held in is written before the fast path is sure to apply.
    pub(crate) fn integer_temporaries(self) -> IntegerRegisters {
        let third = self
            .scratch
            .iter()
            .copied()
            .find(|register| {
                !self.allocatable.contains(register)
                    && *register != self.dividend
                    && *register != self.remainder
            })
            .expect("internal: a scratch register besides the division's is not allocatable");
        IntegerRegisters {
            left: self.dividend,
            right: third,
            remainder: self.remainder,
            first_spare: self.idiom_scratch,
            second_spare: self.remainder,
        }
    }

    /// The two registers under `abi` that play no role and are no scratch
    /// registers (`of`): preserved by the convention and saved by the
    /// entry stub, the free registers a value survives a leaf call in,
    /// which the tests that run machine code hold to that.
    #[cfg(test)]
    pub(crate) fn free_survivors(abi: Abi) -> [Register; 2] {
        match abi {
            Abi::Windows64 | Abi::SystemV => [R13, RBP],
            Abi::Aapcs64 => [X22, X23],
        }
    }
}

/// An index into the leaf table `CodeZone::enter` is given.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct LeafIndex(pub(crate) u32);

/// An argument of a leaf call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Operand {
    /// The word a register holds.
    Register(Register),
    /// A constant word.
    Immediate(u64),
    /// A word of the entry block: what differs from one entry to the
    /// next, as the running activation does.
    Block(BlockWord),
    /// An address inside the code making the call, which names its body
    /// to the leaf (`JitState::body_at`) without the code holding an
    /// absolute address.
    CodeAddress,
    /// The address of the body's data word at this index
    /// (`MacroAssembler::take_body_words`).
    BodyWord(usize),
}

/// The size of an object `bump_allocate` places.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ObjectSize {
    /// An object of this many words after its header, a constant: the
    /// block is the header, those words and, when the header cannot hold
    /// the count, an overflow word before it (`Heap::machine_block`).
    Constant(usize),
    /// The bytes of the block, its header included, in a register, for an
    /// object whose header holds its count -- fewer than 255 words after
    /// it -- and that lies below the large-object threshold: what the
    /// caller has checked before.
    Register(Register),
}

/// Code placed after the body's last instruction, out of the straight
/// line: the paths a check takes only when it fails.
#[derive(Clone, Copy, Debug)]
enum ColdPath {
    Exit {
        label: Label,
        kind: ExitKind,
        program_counter: u32,
    },
    /// A leaf's call (`call_leaf_out_of_line`), its answer moved into
    /// `result`, then back into the straight line at `rejoin`.
    LeafCall {
        label: Label,
        leaf: LeafIndex,
        arguments: [Operand; 3],
        result: Register,
        rejoin: Label,
    },
    /// A leaf's call (`call_leaf_out_of_line_or_exit`), then back into the
    /// straight line at `rejoin` when it answered 0, and an exit that
    /// re-executes the instruction at `program_counter` otherwise.
    CheckedLeafCall {
        label: Label,
        leaf: LeafIndex,
        arguments: [Operand; 3],
        program_counter: u32,
        rejoin: Label,
    },
    /// A prologue's growth of the activation stack (`grow_stack_if`): the
    /// call's handover kept on the native stack across the growth leaf's
    /// call, then back into the straight line at `rejoin` when it made the
    /// room, and the exit `kind` otherwise.
    StackGrowth {
        label: Label,
        leaf: LeafIndex,
        extent: Register,
        kind: ExitKind,
        rejoin: Label,
    },
    #[cfg(debug_assertions)]
    DeadRegister {
        label: Label,
        register: u16,
        program_counter: u32,
    },
}

/// The assembler of one piece of machine code, speaking in the VM's idioms.
///
/// A template reaches the frame's registers, the entry block, a leaf, the
/// poll and an exit only through the idioms below, never by emitting their
/// operations again through `assembler()`, which is for what no idiom
/// covers: an idiom is the one place its layout, its debug check and its
/// calling sequence are decided, so a copy would drift from it.
pub(crate) struct MacroAssembler {
    assembler: Assembler<Encoding>,
    roles: Roles,
    abi: Abi,
    /// The instruction the code being emitted lowers, at which a debug
    /// build's refusal of a dead register exits.
    #[cfg(debug_assertions)]
    program_counter: u32,
    cold_paths: Vec<ColdPath>,
    /// Whether a poll takes its exit unconditionally, as under
    /// `NS_NEXT_STRESS_EXIT`: a safepoint is always requested.
    exit_at_every_poll: bool,
    /// Whether optimized code counts its interrupt points down to a
    /// forced exit (`count_down_to_deoptimize`), as under
    /// `NS_NEXT_STRESS_DEOPT`.
    deoptimize_by_countdown: bool,
    /// Whether a back edge counts itself (`count_back_edge`).
    count_back_edges: bool,
    /// The body's data words, which `finish` places after its code, and
    /// the label they start at (`take_body_words`).
    body_words: usize,
    body_words_label: Option<Label>,
    /// The allocatable registers optimized code holds values in where the
    /// code being emitted runs (`hold_values`), which a debug build
    /// refuses as a temporary and across a leaf call that clobbers them.
    #[cfg(debug_assertions)]
    values_held: Vec<Register>,
    /// For each cold path, by its index, the registers values were held
    /// in where the straight line branches off to it, which its code is
    /// checked against when `finish` emits it after the straight line.
    #[cfg(debug_assertions)]
    cold_path_holds: Vec<Vec<Register>>,
}

/// The alignment of a body's data words, a cache line's, which they share
/// with no instruction: a store into a line the processor is executing
/// from costs it a self-modifying-code clear on x64, and a refill of a
/// word is such a store (Intel SDM Vol. 3A, 11.6 Self-Modifying Code).
pub(crate) const BODY_WORDS_ALIGNMENT: u32 = 64;

impl MacroAssembler {
    pub(crate) fn new(abi: Abi) -> MacroAssembler {
        MacroAssembler {
            assembler: Assembler::with_encoder(abi.encoding()),
            roles: Roles::of(abi),
            abi,
            #[cfg(debug_assertions)]
            program_counter: 0,
            cold_paths: Vec::new(),
            exit_at_every_poll: super::stress_exit_from_env(),
            deoptimize_by_countdown: false,
            count_back_edges: false,
            body_words: 0,
            body_words_label: None,
            #[cfg(debug_assertions)]
            values_held: Vec::new(),
            #[cfg(debug_assertions)]
            cold_path_holds: Vec::new(),
        }
    }

    /// Queues `cold_path` for `finish`, with the registers values are
    /// held in here, where the straight line branches off to it.
    fn push_cold_path(&mut self, cold_path: ColdPath) {
        #[cfg(debug_assertions)]
        self.cold_path_holds.push(self.values_held.clone());
        self.cold_paths.push(cold_path);
    }

    /// Records that optimized code holds values in `registers`, all of
    /// them `Roles::allocatable`, where the code emitted next runs, until
    /// the next `hold_values`; the baseline holds none, and a frame node's
    /// template is emitted after `hold_values(&[])`, since every value is
    /// written back before it. A cold path queued meanwhile keeps the set
    /// held where it was queued. Only a debug build keeps it, to refuse
    /// what would clobber one of them -- and only at the entry points that
    /// check it: `temporary`, `call_leaf`, and the out-of-line leaf calls
    /// (`call_leaf_out_of_line`, `call_leaf_out_of_line_or_exit`), whose
    /// result register is taken as a temporary. An idiom that writes a
    /// register through `assembler()`, or a scratch register a template
    /// names through its roles, is not checked: every idiom optimized code
    /// emits between two frame nodes takes each register it writes,
    /// other than the one its own value is allocated to, through
    /// `temporary`.
    pub(crate) fn hold_values(&mut self, registers: &[Register]) {
        debug_assert!(
            registers
                .iter()
                .all(|register| self.roles.allocatable.contains(register)),
            "internal: values are held only in allocatable registers"
        );
        #[cfg(debug_assertions)]
        {
            self.values_held = registers.to_vec();
        }
    }

    /// `register`, which the code about to be emitted writes as a
    /// temporary: every idiom optimized code emits between two frame
    /// nodes takes its temporaries through here, so that a debug build
    /// refuses one that holds a value (`Roles::allocatable`).
    pub(crate) fn temporary(&self, register: Register) -> Register {
        #[cfg(debug_assertions)]
        assert!(
            !self.values_held.contains(&register),
            "internal: {register:?} holds a value and cannot be a temporary"
        );
        register
    }

    /// Takes `count` more of the body's data words -- words the body reads
    /// between leaf calls and a leaf writes, kept with its code, so that
    /// they live and move with it -- and answers the index of the first.
    /// `finish` places them after the code, zeroed, on a cache line of
    /// their own (`BODY_WORDS_ALIGNMENT`), each reached relative to the
    /// instruction that reads it, so the code holds no absolute address.
    pub(crate) fn take_body_words(&mut self, count: usize) -> usize {
        if self.body_words_label.is_none() {
            self.body_words_label = Some(self.assembler.label());
        }
        let first = self.body_words;
        self.body_words += count;
        first
    }

    /// The label the body's data words start at, and word `index`'s byte
    /// offset from it.
    fn body_word(&self, index: usize) -> (Label, i32) {
        let Some(label) = self.body_words_label else {
            panic!("internal: a body word is read before any is taken");
        };
        assert!(
            index < self.body_words,
            "internal: body word {index} was never taken"
        );
        let displacement = i32::try_from(index)
            .ok()
            .and_then(|index| index.checked_mul(WORD_BYTES))
            .expect("internal: a body's data words lie within 2 GiB of their start");
        (label, displacement)
    }

    /// The label the body's data words start at, which `finish` binds, if
    /// any was taken.
    pub(crate) fn body_words_label(&self) -> Option<Label> {
        self.body_words_label
    }

    /// How many data words the body has taken.
    pub(crate) fn body_word_count(&self) -> usize {
        self.body_words
    }

    /// `target :=` the body's data word `index`. Keeps the flags.
    pub(crate) fn load_body_word(&mut self, target: Register, index: usize) {
        let (label, displacement) = self.body_word(index);
        self.assembler.load_labelled(target, label, displacement);
    }

    /// Makes `count_back_edge` emit its count (`true`) or nothing, so that
    /// code compiled while nobody counts pays nothing for the count.
    pub(crate) fn set_count_back_edges(&mut self, count_back_edges: bool) {
        self.count_back_edges = count_back_edges;
    }

    /// Makes every poll emitted from now on exit at once (`true`) or only
    /// on a requested safepoint, whatever `NS_NEXT_STRESS_EXIT` says.
    #[cfg(test)]
    pub(crate) fn set_exit_at_every_poll(&mut self, exit_at_every_poll: bool) {
        self.exit_at_every_poll = exit_at_every_poll;
    }

    /// Makes every interrupt point of the optimized code emitted from now
    /// on count down to a forced exit (`true`), as under the deopt stress
    /// mode, or emits nothing for the count (`count_down_to_deoptimize`).
    pub(crate) fn set_deoptimize_by_countdown(&mut self, deoptimize_by_countdown: bool) {
        self.deoptimize_by_countdown = deoptimize_by_countdown;
    }

    pub(crate) fn roles(&self) -> Roles {
        self.roles
    }

    /// The abstract operations, for what no idiom covers.
    pub(crate) fn assembler(&mut self) -> &mut Assembler<Encoding> {
        &mut self.assembler
    }

    /// Says which instruction the code emitted next lowers, so that an
    /// idiom's own exit -- a debug build's refusal of a dead register --
    /// resumes the interpreter there.
    pub(crate) fn set_program_counter(&mut self, program_counter: u32) {
        #[cfg(debug_assertions)]
        {
            self.program_counter = program_counter;
        }
        #[cfg(not(debug_assertions))]
        let _ = program_counter;
    }

    /// The address of `word` of the entry block.
    pub(crate) fn block_word(&self, word: BlockWord) -> Memory {
        Memory {
            base: self.roles.context,
            displacement: word.displacement(),
        }
    }

    /// Tells apart, where the native stack pointer stands at a body's
    /// base -- at an instruction boundary, or in the exit stub, which a
    /// body jumps to -- the frame the stub that made the entry called
    /// from every frame machine code pushed: jumps to `above` when a
    /// compiled frame's return address lies above the entry's, and to
    /// `entered_at_push` when the body is the entry stub's -- the run
    /// loop's entry frame, whose header its push wrote -- and falls
    /// through for the call stub's callee, a frame its prologue pushed.
    /// The one test of which frame began the stretch, shared by the exit
    /// stub (`stubs::assemble_exit_stub`) and a lean body's transfer
    /// (`closures::transfer_to_twin`), so that the two cannot drift. Clobbers
    /// the flags and `scratch`.
    pub(crate) fn branch_on_stretch_entry(
        &mut self,
        scratch: Register,
        above: Label,
        entered_at_push: Label,
    ) {
        let saved_stack_pointer = self.block_word(BlockWord::SavedStackPointer);
        let entered_by_call = self.block_word(BlockWord::EnteredByCall);
        let slot_bytes = self.abi.stack_slot_bytes();
        let assembler = &mut self.assembler;
        // The body was called with the stack pointer one slot below the
        // saved one, the entry stub's return address; a compiled frame
        // above it pushed one more each.
        assembler.move_register(scratch, self.roles.stack_pointer);
        assembler.add_immediate(scratch, slot_bytes);
        assembler.compare_memory(scratch, saved_stack_pointer);
        assembler.jump_if(Condition::NotEqual, above);
        assembler.load(Width::Word64, scratch, entered_by_call);
        assembler.compare_immediate(scratch, 0);
        assembler.jump_if(Condition::Equal, entered_at_push);
    }

    /// The address of the frame's register `register`.
    fn frame_slot(&self, register: u16) -> Memory {
        Memory {
            base: self.roles.frame,
            displacement: i32::from(register) * 8,
        }
    }

    /// `target :=` the frame's register `register`. Clobbers the flags
    /// and the idiom scratch register in every build, though only a debug
    /// build uses them: there it refuses a dead register as the
    /// interpreter's read does (`heap.rs`'s `assert_live_register`), with
    /// an exit of its own at the current program counter. So a template
    /// never relies on the flags across it, in either build.
    pub(crate) fn load_frame_register(&mut self, target: Register, register: u16) {
        debug_assert_ne!(
            target, self.roles.idiom_scratch,
            "the check of a dead register needs the idiom scratch register"
        );
        let slot = self.frame_slot(register);
        self.assembler.load(Width::Word64, target, slot);
        #[cfg(debug_assertions)]
        {
            let label = self.assembler.label();
            let scratch = self.roles.idiom_scratch;
            self.assembler
                .move_immediate(scratch, crate::heap::Word::DEAD_REGISTER_BITS);
            self.assembler.compare(target, scratch);
            self.assembler.jump_if(Condition::Equal, label);
            self.push_cold_path(ColdPath::DeadRegister {
                label,
                register,
                program_counter: self.program_counter,
            });
        }
    }

    /// `target :=` the receiver of a frame whose code takes
    /// `argument_count` arguments: a header word, which is never dead, so
    /// nothing is checked. Keeps the flags.
    pub(crate) fn load_receiver(&mut self, target: Register, argument_count: u16) {
        let slot = self.frame_slot(crate::heap::Heap::receiver_register(argument_count));
        self.assembler.load(Width::Word64, target, slot);
    }

    /// The frame's register `register := source`. Keeps the flags.
    pub(crate) fn store_frame_register(&mut self, register: u16, source: Register) {
        let slot = self.frame_slot(register);
        self.assembler.store(Width::Word64, slot, source);
    }

    /// `target := word`. Keeps the flags.
    pub(crate) fn load_immediate_word(&mut self, target: Register, word: u64) {
        self.assembler.move_immediate(target, word);
    }

    /// `target := source`. Keeps the flags.
    pub(crate) fn move_register(&mut self, target: Register, source: Register) {
        self.assembler.move_register(target, source);
    }

    /// `register :=` the integer the `SmallInteger` in it holds, its tag
    /// shifted out.
    pub(crate) fn untag_small_integer(&mut self, register: Register) {
        self.assembler
            .shift_right_arithmetic(register, small_integer_tag_bits());
    }

    /// `register :=` the `SmallInteger` of an integer already shifted into
    /// place above its tag: the tag set.
    pub(crate) fn tag_small_integer(&mut self, register: Register) {
        self.assembler
            .or_immediate(register, small_integer_tag_word());
    }

    /// `decline`s with `Condition::Equal` unless `left` holds a
    /// `SmallInteger`, then likewise for `right`: `integer_into`'s decline
    /// of any other operand. Each decline is its own branch, so that a
    /// baseline body's backward one polls on its own way.
    pub(crate) fn test_small_integer_tags(
        &mut self,
        left: Register,
        right: Register,
        decline: &mut dyn FnMut(&mut MacroAssembler, Condition),
    ) {
        let tag = small_integer_tag_word();
        self.assembler.test_immediate(left, tag);
        decline(self, Condition::Equal);
        self.assembler.test_immediate(right, tag);
        decline(self, Condition::Equal);
    }

    /// `IntegerBinary`'s `SmallInteger` fast path, `integer_into`'s, on the
    /// words in `registers.left` and `registers.right`, answering the
    /// register the result is left in: every case it does not compute --
    /// an operand that is no `SmallInteger`, an overflow, a zero divisor --
    /// calls `decline` with the condition under which the code branches
    /// away, before any register but `registers`' own is written. The
    /// baseline and optimized code both emit it, each with its own
    /// registers and its own way to decline, so the two make the same
    /// choice of instructions. Clobbers the flags and every register of
    /// `registers`, the operands' included.
    pub(crate) fn small_integer_operation(
        &mut self,
        operation: IntegerOperation,
        registers: IntegerRegisters,
        decline: &mut dyn FnMut(&mut MacroAssembler, Condition),
    ) -> Register {
        debug_assert!(
            distinct(&[
                registers.left,
                registers.right,
                registers.remainder,
                registers.first_spare
            ]) && distinct(&[
                registers.left,
                registers.right,
                registers.first_spare,
                registers.second_spare
            ]),
            "internal: a fast path's registers are distinct"
        );
        self.test_small_integer_tags(registers.left, registers.right, decline);
        let tag = small_integer_tag_word();
        let (left, right) = (registers.left, registers.right);
        match operation {
            // `(a << 3 | 1) + ((b << 3 | 1) - 1)` is `(a + b) << 3 | 1`,
            // and overflows exactly when `a + b` leaves 61 bits:
            // `Value::small_integer_sum`. The difference likewise.
            IntegerOperation::Add | IntegerOperation::Subtract => {
                let untagged = registers.first_spare;
                let assembler = &mut self.assembler;
                assembler.move_register(untagged, right);
                assembler.add_immediate(untagged, -tag);
                if operation == IntegerOperation::Add {
                    assembler.add(left, untagged);
                } else {
                    assembler.subtract(left, untagged);
                }
                decline(self, Condition::Overflow);
                left
            }
            // `a * (b << 3)` is `(a * b) << 3`, overflowing exactly when
            // `a * b` leaves 61 bits: `Value::small_integer_product`.
            IntegerOperation::Multiply => {
                let untagged = registers.first_spare;
                self.untag_small_integer(left);
                let assembler = &mut self.assembler;
                assembler.move_register(untagged, right);
                assembler.add_immediate(untagged, -tag);
                assembler.multiply_checked(left, untagged);
                decline(self, Condition::Overflow);
                self.tag_small_integer(left);
                left
            }
            IntegerOperation::FloorDivide => self.small_integer_floor_quotient(registers, decline),
            // Tagging keeps the order, so the words compare as the
            // integers do: `Value::small_integer_order`.
            IntegerOperation::Less => self.small_integer_compare(registers, Condition::Less),
            IntegerOperation::Greater => self.small_integer_compare(registers, Condition::Greater),
            IntegerOperation::LessOrEqual => {
                self.small_integer_compare(registers, Condition::LessOrEqual)
            }
            IntegerOperation::GreaterOrEqual => {
                self.small_integer_compare(registers, Condition::GreaterOrEqual)
            }
            IntegerOperation::Equal => self.small_integer_compare(registers, Condition::Equal),
        }
    }

    /// The Boolean of comparing the `SmallInteger`s in `registers.left`
    /// and `registers.right` by `condition`, in the register it answers.
    fn small_integer_compare(
        &mut self,
        registers: IntegerRegisters,
        condition: Condition,
    ) -> Register {
        let (answer, truth) = (registers.first_spare, registers.second_spare);
        let assembler = &mut self.assembler;
        assembler.move_immediate(answer, crate::heap::Immediate::boolean(false).bits());
        assembler.move_immediate(truth, crate::heap::Immediate::boolean(true).bits());
        assembler.compare(registers.left, registers.right);
        assembler.move_if(condition, answer, truth);
        answer
    }

    /// The floor quotient of the `SmallInteger`s in `registers.left` and
    /// `registers.right`, tagged, in the register it answers:
    /// `Value::small_integer_floor_quotient`, declining for a zero divisor
    /// and for the one quotient that leaves 61 bits, the smallest
    /// `SmallInteger` over -1. `registers.left` is the division's dividend
    /// and `registers.remainder` its remainder, where the encoder fixes
    /// them.
    fn small_integer_floor_quotient(
        &mut self,
        registers: IntegerRegisters,
        decline: &mut dyn FnMut(&mut MacroAssembler, Condition),
    ) -> Register {
        let (dividend, divisor) = (registers.left, registers.right);
        self.untag_small_integer(dividend);
        self.untag_small_integer(divisor);
        self.assembler.compare_immediate(divisor, 0);
        decline(self, Condition::Equal);
        // Both are 61-bit integers, so the division cannot fault.
        let assembler = &mut self.assembler;
        assembler.divide_signed(dividend, registers.remainder, divisor);
        // `checked_floor_divide`: no remainder, or one of the divisor's
        // sign, leaves the truncated quotient; any other rounds it down.
        // The second spare may be the remainder's register, which is read
        // before it is written.
        let (remainder_sign, divisor_sign) = (registers.first_spare, registers.second_spare);
        let floored = assembler.label();
        assembler.compare_immediate(registers.remainder, 0);
        assembler.jump_if(Condition::Equal, floored);
        assembler.move_register(remainder_sign, registers.remainder);
        assembler.shift_right_arithmetic(remainder_sign, SIGN_SHIFT);
        assembler.move_register(divisor_sign, divisor);
        assembler.shift_right_arithmetic(divisor_sign, SIGN_SHIFT);
        assembler.compare(remainder_sign, divisor_sign);
        assembler.jump_if(Condition::Equal, floored);
        assembler.add_immediate(dividend, -1);
        assembler.bind(floored);
        // Tagged, the quotient comes back unchanged from its shift unless
        // it leaves 61 bits.
        let (tagged, untagged) = (registers.first_spare, registers.second_spare);
        assembler.move_register(tagged, dividend);
        assembler.shift_left(tagged, small_integer_tag_bits());
        assembler.move_register(untagged, tagged);
        self.untag_small_integer(untagged);
        self.assembler.compare(untagged, dividend);
        decline(self, Condition::NotEqual);
        self.tag_small_integer(tagged);
        tagged
    }

    /// Calls `leaf` through the shim with `arguments`, leaving its answer
    /// in the roles' `leaf_result`. Only the pinned roles survive it: the
    /// caller has flushed every value it caches in a scratch register.
    /// Clobbers the flags.
    ///
    /// The native stack is aligned here, at run time, rather than from a
    /// depth the compiler tracks, since later stages call leaves from
    /// depths it cannot know -- as Cog aligns the C stack before each call
    /// into the runtime. Every operand held in a register is copied to the
    /// stack before any argument register is written, so an operand may
    /// sit in any register, an argument register included; a block word is
    /// read through the context register, which no argument overwrites.
    ///
    /// The stack pointer the call aligns away from waits in the block's
    /// `LeafStackPointer`, so that no register is pinned for it: a leaf
    /// never enters machine code, so no second leaf call overwrites the
    /// word before the first restores it.
    ///
    /// # Panics
    ///
    /// On an operand in the stack pointer, which the alignment overwrites
    /// before the operand is read.
    pub(crate) fn call_leaf(&mut self, leaf: LeafIndex, arguments: [Operand; 3]) {
        let roles = self.roles;
        // A value held past the survivors would be clobbered by the call
        // or by its arguments' placement.
        #[cfg(debug_assertions)]
        if let Some(register) = self
            .values_held
            .iter()
            .find(|register| !roles.allocatable[..roles.survive_leaf_calls].contains(register))
        {
            panic!("internal: a leaf call clobbers {register:?}, which holds a value");
        }
        let stack_pointer = roles.stack_pointer;
        let layout = LeafCallLayout::of(self.abi);
        for argument in arguments {
            assert!(
                argument != Operand::Register(stack_pointer),
                "a leaf call's operand cannot be the stack pointer it realigns"
            );
        }
        // A byte offset into the frame the call carves below the aligned
        // stack pointer, whatever the architecture's stack slot.
        let stack_slot = |displacement| Memory {
            base: stack_pointer,
            displacement,
        };
        let saved_stack_pointer = self.block_word(BlockWord::LeafStackPointer);
        self.assembler
            .store(Width::Word64, saved_stack_pointer, stack_pointer);
        self.assembler.and_immediate(stack_pointer, -16);
        self.assembler.add_stack(-layout.frame_bytes);
        let assignments = arguments.into_iter().zip(layout.places).zip(layout.copies);
        for ((argument, place), copy) in assignments.clone() {
            if let Operand::Register(source) = argument {
                let displacement = match place {
                    Place::Register(_) => copy,
                    Place::Stack(displacement) => displacement,
                };
                self.assembler
                    .store(Width::Word64, stack_slot(displacement), source);
            }
        }
        for ((argument, place), copy) in assignments {
            match (argument, place) {
                (Operand::Register(_), Place::Register(target)) => {
                    self.assembler.load(Width::Word64, target, stack_slot(copy));
                }
                (Operand::Register(_), Place::Stack(_)) => {}
                (Operand::Block(word), Place::Register(target)) => {
                    let address = self.block_word(word);
                    self.assembler.load(Width::Word64, target, address);
                }
                (Operand::Block(word), Place::Stack(displacement)) => {
                    let address = self.block_word(word);
                    self.assembler
                        .load(Width::Word64, roles.idiom_scratch, address);
                    self.assembler.store(
                        Width::Word64,
                        stack_slot(displacement),
                        roles.idiom_scratch,
                    );
                }
                (Operand::CodeAddress, Place::Register(target)) => {
                    self.assembler.load_code_address(target);
                }
                (Operand::BodyWord(index), Place::Register(target)) => {
                    let (label, displacement) = self.body_word(index);
                    self.assembler
                        .load_label_address(target, label, displacement);
                }
                (Operand::BodyWord(index), Place::Stack(displacement)) => {
                    let (label, word) = self.body_word(index);
                    self.assembler
                        .load_label_address(roles.idiom_scratch, label, word);
                    self.assembler.store(
                        Width::Word64,
                        stack_slot(displacement),
                        roles.idiom_scratch,
                    );
                }
                (Operand::CodeAddress, Place::Stack(displacement)) => {
                    self.assembler.load_code_address(roles.idiom_scratch);
                    self.assembler.store(
                        Width::Word64,
                        stack_slot(displacement),
                        roles.idiom_scratch,
                    );
                }
                (Operand::Immediate(word), Place::Register(target)) => {
                    self.assembler.move_immediate(target, word);
                }
                (Operand::Immediate(word), Place::Stack(displacement)) => {
                    if let Ok(value) = i32::try_from(word.cast_signed()) {
                        self.assembler
                            .store_immediate32(stack_slot(displacement), value);
                    } else {
                        self.assembler.move_immediate(roles.idiom_scratch, word);
                        self.assembler.store(
                            Width::Word64,
                            stack_slot(displacement),
                            roles.idiom_scratch,
                        );
                    }
                }
            }
        }
        self.assembler.move_register(layout.block, roles.context);
        self.assembler
            .move_immediate(layout.leaf, u64::from(leaf.0));
        let shim = self.block_word(BlockWord::Shim);
        self.assembler
            .load(Width::Word64, roles.idiom_scratch, shim);
        self.assembler.call_register(roles.idiom_scratch);
        self.assembler
            .load(Width::Word64, stack_pointer, saved_stack_pointer);
    }

    /// Calls the code at the address in `target`, and answers a label
    /// bound right after the `call` instruction: its return address, the
    /// address the processor pushes, with nothing emitted between the two.
    /// A send's call goes through here so that the exit's walk can map
    /// the return address it finds on the native stack back to the send.
    pub(crate) fn call_returning(&mut self, target: Register) -> Label {
        self.assembler.call_register(target);
        let return_address = self.assembler.label();
        self.assembler.bind(return_address);
        return_address
    }

    /// Leaves machine code through the exit stub, which writes `kind`,
    /// `program_counter` and an address inside this code -- naming the
    /// body the exit leaves, the innermost frame's -- into the block,
    /// materializes every compiled frame and returns to `enter`'s caller.
    /// A jump rather than a call: the stub never returns here, and a call
    /// that never returns would leave the processor's return-address
    /// predictor unbalanced on every exit.
    pub(crate) fn exit(&mut self, kind: ExitKind, program_counter: u32) {
        let roles = self.roles;
        self.assembler.load_code_address(roles.exit_code_address());
        self.assembler
            .move_immediate(roles.exit_kind(), kind as u64);
        self.assembler
            .move_immediate(roles.exit_program_counter(), u64::from(program_counter));
        let stub = self.block_word(BlockWord::ExitStub);
        self.assembler
            .load(Width::Word64, roles.idiom_scratch, stub);
        self.assembler.jump_register(roles.idiom_scratch);
    }

    /// Takes `exit(kind, program_counter)` when `condition` holds, out of
    /// line, and falls through otherwise. Keeps the flags on the path
    /// that falls through.
    pub(crate) fn exit_if(&mut self, condition: Condition, kind: ExitKind, program_counter: u32) {
        let label = self.assembler.label();
        self.assembler.jump_if(condition, label);
        self.push_cold_path(ColdPath::Exit {
            label,
            kind,
            program_counter,
        });
    }

    /// Calls `leaf` with `arguments` out of line, at `label`, which the
    /// straight line jumps to, then moves its answer into `result` and
    /// jumps back to `rejoin`: a slow path that rejoins the fast one, as
    /// an allocation's leaf does once the inline bump found no room. The
    /// call keeps only the pinned roles, as every leaf call does
    /// (`call_leaf`), so the code at `rejoin` reads no other scratch
    /// register the straight line set before the jump.
    pub(crate) fn call_leaf_out_of_line(
        &mut self,
        label: Label,
        leaf: LeafIndex,
        arguments: [Operand; 3],
        result: Register,
        rejoin: Label,
    ) {
        self.push_cold_path(ColdPath::LeafCall {
            label,
            leaf,
            arguments,
            result,
            rejoin,
        });
    }

    /// Calls `leaf` with `arguments` out of line, at `label`, which the
    /// straight line jumps to; on its answer 0 jumps back to `rejoin`, and
    /// on any other takes the exit that re-executes the instruction at
    /// `program_counter`: a cold check that either vouches for what the
    /// straight line does next or, having changed nothing, hands the
    /// instruction to the interpreter. The call keeps only the pinned
    /// roles (`call_leaf_out_of_line`).
    pub(crate) fn call_leaf_out_of_line_or_exit(
        &mut self,
        label: Label,
        leaf: LeafIndex,
        arguments: [Operand; 3],
        program_counter: u32,
        rejoin: Label,
    ) {
        self.push_cold_path(ColdPath::CheckedLeafCall {
            label,
            leaf,
            arguments,
            program_counter,
            rejoin,
        });
    }

    /// When `condition` holds -- the frame a prologue pushes, ending
    /// `extent` bytes from the activation stack's base, ends past the
    /// ordinary end -- calls `leaf`, the growth leaf
    /// (`jit::leaves::grow_stack`), out of line with `extent`, keeping the
    /// call's handover (`Roles::call_handover`) on the native stack across
    /// it: back here when it made the room, so that the push goes on in
    /// machine code, and the exit `kind` when the ordinary room cannot hold
    /// the frame, nothing changed. Falls through otherwise.
    pub(crate) fn grow_stack_if(
        &mut self,
        condition: Condition,
        leaf: LeafIndex,
        extent: Register,
        kind: ExitKind,
    ) {
        let label = self.assembler.label();
        let rejoin = self.assembler.label();
        self.assembler.jump_if(condition, label);
        self.assembler.bind(rejoin);
        self.push_cold_path(ColdPath::StackGrowth {
            label,
            leaf,
            extent,
            kind,
            rejoin,
        });
    }

    /// `header :=` the header word of an `Array` of `word_count` words
    /// after its header (`MachineWord::ArrayHeader`, the count's bits
    /// added), and `class_word :=` the `Array` class's word
    /// (`MachineWord::ArrayClass`): what `MakeVector`'s allocation writes,
    /// as `Heap::allocate_nil_array` writes it. Clobbers the flags and the
    /// idiom scratch register.
    pub(crate) fn load_array_header_and_class(
        &mut self,
        header: Register,
        class_word: Register,
        word_count: usize,
    ) {
        self.load_header_and_class(
            (header, MachineWord::ArrayHeader),
            (class_word, MachineWord::ArrayClass),
            word_count,
        );
    }

    /// `header.0 :=` the machine word `header.1`, the header word of an
    /// object of no words, with the bits of `word_count` words after it
    /// added, and `class_word.0 :=` the machine word `class_word.1`, its
    /// class's word: `load_array_header_and_class` for any class whose
    /// header and class word the machine words hold -- a closure's and a
    /// frame object's (`jit::closures`). Clobbers the flags and the idiom
    /// scratch register.
    pub(crate) fn load_header_and_class(
        &mut self,
        header: (Register, MachineWord),
        class_word: (Register, MachineWord),
        word_count: usize,
    ) {
        let machine_words = self.roles.idiom_scratch;
        self.load_machine_words(machine_words);
        let count_bits = crate::heap::Heap::machine_word_count_bits(word_count);
        let assembler = &mut self.assembler;
        assembler.move_immediate(class_word.0, count_bits);
        assembler.load(
            Width::Word64,
            header.0,
            Self::machine_word(machine_words, header.1),
        );
        // The machine word's count is 0, so the addition sets the count.
        assembler.add(header.0, class_word.0);
        assembler.load(
            Width::Word64,
            class_word.0,
            Self::machine_word(machine_words, class_word.1),
        );
    }

    /// Bumps eden's cursor (`BlockWord::EdenCursor`) in place by the
    /// block of an object of `size`, as `Space::bump_object` bumps it:
    /// jumps to `slow`, nothing written, when the block would end past the
    /// cursor's limit -- below which no allocation reaches eden's
    /// scavenge threshold, so the slow path's leaf decides every request
    /// of a scavenge -- or when the inline bump may not place it at all;
    /// otherwise stores the new top, writes the overflow word if the
    /// block has one, `header` and `class_word`, and counts the object
    /// (`MachineWord::MachineAllocations`). `object :=` the object's
    /// offset, which is its word, as `Value::object` makes it; its fields
    /// hold whatever eden held, which the caller writes before anything
    /// reads them. Clobbers the flags, the idiom scratch register and,
    /// past the jump to `slow`, `header`, which the slow path still finds
    /// as it was.
    pub(crate) fn bump_allocate(
        &mut self,
        object: Register,
        size: ObjectSize,
        header: Register,
        class_word: Register,
        slow: Label,
    ) {
        let layout = crate::heap::Heap::machine_layout();
        let (scratch, object_space) = (self.roles.idiom_scratch, self.roles.object_space);
        debug_assert!(
            object != header
                && object != class_word
                && header != class_word
                && ![object, header, class_word].contains(&scratch),
            "internal: an allocation's registers are distinct and none is the idiom scratch register"
        );
        let (bytes, overflow_word) = match size {
            ObjectSize::Constant(word_count) => {
                let block = crate::heap::Heap::machine_block(word_count);
                if !block.inline {
                    self.assembler.jump(slow);
                    return;
                }
                (Some(block.bytes), block.overflow_word)
            }
            ObjectSize::Register(_) => (None, None),
        };
        let cursor = self.block_word(BlockWord::EdenCursor);
        let top = Memory {
            base: scratch,
            displacement: 0,
        };
        let limit = Memory {
            base: scratch,
            displacement: EDEN_LIMIT_DISPLACEMENT,
        };
        let assembler = &mut self.assembler;
        assembler.load(Width::Word64, scratch, cursor);
        assembler.load(Width::Word64, object, top);
        match (size, bytes) {
            (ObjectSize::Register(bytes), _) => assembler.add(object, bytes),
            (ObjectSize::Constant(_), Some(bytes)) => assembler.add_immediate(object, bytes),
            (ObjectSize::Constant(_), None) => unreachable!("internal: a constant block's bytes"),
        }
        // The end against the limit, unsigned: a limit below the top, as
        // while the decoder allocates old, refuses every bump, as
        // `Reservation::bump`'s saturating subtraction does.
        assembler.compare_memory(object, limit);
        assembler.jump_if(Condition::Above, slow);
        assembler.store(Width::Word64, top, object);
        match (size, bytes) {
            (ObjectSize::Register(bytes), _) => assembler.subtract(object, bytes),
            (_, Some(bytes)) => assembler.add_immediate(object, -bytes),
            (ObjectSize::Constant(_), None) => unreachable!("internal: a constant block's bytes"),
        }
        assembler.add(object, object_space);
        if let Some(overflow_word) = overflow_word {
            assembler.move_immediate(scratch, overflow_word);
            assembler.store(
                Width::Word64,
                Memory {
                    base: object,
                    displacement: 0,
                },
                scratch,
            );
            assembler.add_immediate(object, WORD_BYTES);
        }
        assembler.store(
            Width::Word64,
            Memory {
                base: object,
                displacement: layout.header_displacement,
            },
            header,
        );
        assembler.store(
            Width::Word64,
            Memory {
                base: object,
                displacement: layout.class_word_displacement,
            },
            class_word,
        );
        assembler.subtract(object, object_space);
        self.load_machine_words(scratch);
        let count = Self::machine_word(scratch, MachineWord::MachineAllocations);
        let assembler = &mut self.assembler;
        assembler.load(Width::Word64, header, count);
        assembler.add_immediate(header, 1);
        assembler.store(Width::Word64, count, header);
    }

    /// Writes `nil` into `count` fields of the object whose word is in
    /// `object`, from field `from` on, as `allocate_object` fills a new
    /// object's: a straight line of stores for a few, a loop for more.
    /// `object` is kept. Clobbers the flags and the idiom scratch
    /// register.
    pub(crate) fn nil_fill(&mut self, object: Register, from: usize, count: usize) {
        if count == 0 {
            return;
        }
        let layout = crate::heap::Heap::machine_layout();
        let (scratch, object_space) = (self.roles.idiom_scratch, self.roles.object_space);
        let nil = layout.nil_word;
        let first = from
            .checked_mul(8)
            .and_then(|bytes| i32::try_from(bytes).ok())
            .and_then(|bytes| bytes.checked_add(layout.fields_displacement))
            .expect("internal: an object's fields lie within 2 GiB of its start");
        let bytes = count
            .checked_mul(8)
            .and_then(|bytes| i32::try_from(bytes).ok())
            .and_then(|bytes| bytes.checked_add(first).map(|_| bytes))
            .expect("internal: an object's fields lie within 2 GiB of its start");
        let assembler = &mut self.assembler;
        if count <= NIL_STORES_IN_LINE {
            assembler.move_register(scratch, object);
            assembler.add(scratch, object_space);
            for index in 0..count {
                let index = i32::try_from(index).expect("internal: a few stores");
                let displacement = first + index * WORD_BYTES;
                assembler.store_immediate32(
                    Memory {
                        base: scratch,
                        displacement,
                    },
                    nil,
                );
            }
            return;
        }
        // The end, then `object` walked up to it field by field and back.
        assembler.move_register(scratch, object);
        assembler.add(scratch, object_space);
        assembler.add_immediate(scratch, first + bytes);
        assembler.add(object, object_space);
        assembler.add_immediate(object, first);
        let step = assembler.label();
        assembler.bind(step);
        assembler.store_immediate32(
            Memory {
                base: object,
                displacement: 0,
            },
            nil,
        );
        assembler.add_immediate(object, WORD_BYTES);
        assembler.compare(object, scratch);
        assembler.jump_if(Condition::Below, step);
        assembler.subtract(object, object_space);
        assembler.add_immediate(object, -(first + bytes));
    }

    /// Writes `nil` into every field of the object whose word is in
    /// `object` and whose block, its header included, takes the bytes in
    /// `bytes`, a header that holds its count: `nil_fill` for a count
    /// known only at run time, a loop that writes nothing for an object of
    /// no fields. `object` and `bytes` are kept. Clobbers the flags,
    /// `end` and the idiom scratch register.
    pub(crate) fn nil_fill_block(&mut self, object: Register, bytes: Register, end: Register) {
        debug_assert!(
            distinct(&[object, bytes, end, self.roles.idiom_scratch]),
            "internal: the fill's registers are distinct"
        );
        let layout = crate::heap::Heap::machine_layout();
        let (field, object_space) = (self.roles.idiom_scratch, self.roles.object_space);
        let assembler = &mut self.assembler;
        let (step, done) = (assembler.label(), assembler.label());
        assembler.move_register(end, object);
        assembler.add(end, object_space);
        assembler.move_register(field, end);
        assembler.add(end, bytes);
        assembler.add_immediate(field, layout.fields_displacement);
        assembler.bind(step);
        assembler.compare(field, end);
        assembler.jump_if(Condition::AboveOrEqual, done);
        assembler.store_immediate32(
            Memory {
                base: field,
                displacement: 0,
            },
            layout.nil_word,
        );
        assembler.add_immediate(field, WORD_BYTES);
        assembler.jump(step);
        assembler.bind(done);
    }

    /// Jumps to `refused` unless the object `class` names -- an object,
    /// which the caller has checked -- is a class object whose instances
    /// `Heap::allocate_instance_with` makes as an ordinary instance would
    /// be made: its format a class object's, its builtin field and its
    /// value class refusal nil, both read afresh, as
    /// `allocate_for_factory` reads them at every instantiation; then
    /// `header :=` the instance header word the application id table
    /// records for its application (`heap/application_ids.rs`), and a jump
    /// to `refused` for an application the table does not record or whose
    /// word is 0. Clobbers the flags and the idiom scratch register.
    pub(crate) fn load_instance_header(
        &mut self,
        class: Register,
        header: Register,
        refused: Label,
    ) {
        debug_assert!(
            distinct(&[class, header, self.roles.idiom_scratch]),
            "internal: the instance header's registers are distinct"
        );
        let layout = crate::heap::Heap::machine_layout();
        let (object_space, scratch) = (self.roles.object_space, self.roles.idiom_scratch);
        let field = |displacement| Memory {
            base: header,
            displacement,
        };
        let assembler = &mut self.assembler;
        assembler.move_register(header, class);
        assembler.add(header, object_space);
        // The format first: only a class object has the fields read next.
        assembler.load(Width::Word64, scratch, field(layout.header_displacement));
        assembler.and_immediate(scratch, layout.instance_format_bits);
        assembler.compare_immediate(scratch, layout.class_format);
        assembler.jump_if(Condition::NotEqual, refused);
        for displacement in [
            layout.builtin_displacement,
            layout.value_class_refusal_displacement,
        ] {
            assembler.load(Width::Word64, scratch, field(displacement));
            assembler.compare_immediate(scratch, layout.nil_word);
            assembler.jump_if(Condition::NotEqual, refused);
        }
        self.load_application_word(header, class);
        let assembler = &mut self.assembler;
        assembler.test_immediate(header, layout.small_integer_tag);
        assembler.jump_if(Condition::Equal, refused);
        assembler.shift_right_arithmetic(header, layout.tag_bits);
        self.load_application_row(header, refused);
        let assembler = &mut self.assembler;
        assembler.load(
            Width::Word64,
            header,
            Memory {
                base: header,
                displacement: layout.application_instance_header_displacement,
            },
        );
        assembler.compare_immediate(header, 0);
        assembler.jump_if(Condition::Equal, refused);
    }

    /// Stores `value` into the word at `displacement` from the offset of
    /// the object `holder` names, an object machine code did not just
    /// allocate, then applies the write barrier as `Heap::write_barrier`
    /// does (`barrier`). Clobbers the flags, `scratch` and, on the path
    /// that calls the leaf, every scratch register but `holder`, as every
    /// leaf call does.
    pub(crate) fn store_with_barrier(
        &mut self,
        holder: Register,
        displacement: i32,
        value: Register,
        scratch: Register,
    ) {
        debug_assert!(
            holder != value && holder != scratch && value != scratch,
            "internal: a barrier's registers are distinct"
        );
        let object_space = self.roles.object_space;
        let assembler = &mut self.assembler;
        assembler.move_register(scratch, holder);
        assembler.add(scratch, object_space);
        assembler.store(
            Width::Word64,
            Memory {
                base: scratch,
                displacement,
            },
            value,
        );
        self.barrier(holder, value, scratch, true);
    }

    /// `store_with_barrier`, into the word at `target`, an address inside
    /// the object `holder` names that the caller computed -- an element
    /// or a field at an index known only at run time. `target`'s base is
    /// kept on the straight line; clobbers what `store_with_barrier` does.
    pub(crate) fn store_with_barrier_at(
        &mut self,
        holder: Register,
        target: Memory,
        value: Register,
        scratch: Register,
    ) {
        debug_assert!(
            holder != value
                && holder != scratch
                && value != scratch
                && target.base != scratch
                && target.base != value,
            "internal: a barrier's registers are distinct"
        );
        self.assembler.store(Width::Word64, target, value);
        self.barrier(holder, value, scratch, false);
    }

    /// The write barrier after `value` was stored into the object
    /// `holder` names, with Cog's early outs in its order
    /// (`genStoreCheckReceiverReg:valueReg:scratchReg:inFrame:`): a
    /// `value` that names no object -- an immediate, `nil` among the
    /// constants, or the word 0 -- a young `holder`, an old `value`, and a
    /// `holder` whose remembered bit is set already. Only a store that
    /// passes all four calls the leaf that remembers `holder`
    /// (`Leaf::Remember`, `Heap::remember`), out of line, which answers
    /// `holder` again, so that `holder` is kept on both paths and `value`
    /// on the straight line. `scratch` holds `holder`'s address when
    /// `holds_holder_address` says so, and is computed afresh otherwise,
    /// past the three early outs that need no header.
    fn barrier(
        &mut self,
        holder: Register,
        value: Register,
        scratch: Register,
        holds_holder_address: bool,
    ) {
        let layout = crate::heap::Heap::machine_layout();
        let object_space = self.roles.object_space;
        let assembler = &mut self.assembler;
        let (remember, done) = (assembler.label(), assembler.label());
        assembler.test_immediate(value, layout.tag_mask);
        assembler.jump_if(Condition::NotEqual, done);
        assembler.compare_immediate(value, 0);
        assembler.jump_if(Condition::Equal, done);
        assembler.compare_immediate(holder, layout.nursery_end);
        assembler.jump_if(Condition::Below, done);
        assembler.compare_immediate(value, layout.nursery_end);
        assembler.jump_if(Condition::AboveOrEqual, done);
        if !holds_holder_address {
            assembler.move_register(scratch, holder);
            assembler.add(scratch, object_space);
        }
        assembler.load(
            Width::Word64,
            scratch,
            Memory {
                base: scratch,
                displacement: layout.header_displacement,
            },
        );
        assembler.test_immediate_with_width(Width::Word32, scratch, layout.remembered_bit);
        assembler.jump_if(Condition::Equal, remember);
        assembler.bind(done);
        self.call_leaf_out_of_line(
            remember,
            super::leaves::Leaf::Remember.index(),
            [
                Operand::Register(holder),
                Operand::Immediate(0),
                Operand::Immediate(0),
            ],
            holder,
            done,
        );
    }

    /// Jumps to `failed` unless `object` holds an object of an `Array`'s
    /// format (`Format::IndexablePointers`) with an element `index`, as
    /// `Heap::array_element` and `Heap::array_set_element` find it: what
    /// `interpreter::locals::load_vector` and `store_vector` check of an
    /// indirection vector before they read or write it. The count is the
    /// header's, or the overflow word's when the header says it is there.
    /// `object` is kept; clobbers the flags and `scratch`.
    pub(crate) fn check_array_index(
        &mut self,
        object: Register,
        index: u16,
        scratch: Register,
        failed: Label,
    ) {
        debug_assert!(object != scratch, "internal: the check keeps its object");
        let object_space = self.roles.object_space;
        let layout = crate::heap::Heap::machine_layout();
        // The words after the header an element `index` needs: the class
        // word and the elements up to it.
        let needed = i32::from(index) + 2;
        self.jump_unless_object_word(object, failed);
        let assembler = &mut self.assembler;
        assembler.move_register(scratch, object);
        assembler.add(scratch, object_space);
        assembler.load(
            Width::Word64,
            scratch,
            Memory {
                base: scratch,
                displacement: layout.header_displacement,
            },
        );
        self.jump_unless_format(scratch, layout.indexable_pointers_format, failed);
        self.assembler
            .shift_right_logical(scratch, layout.word_count_shift);
        // A count in the overflow word is at least the header's mark, so
        // when that is past `needed` the header's alone decides.
        if needed >= layout.word_count_overflow {
            self.load_overflow_count(object, scratch);
        }
        let assembler = &mut self.assembler;
        assembler.compare_immediate(scratch, needed);
        assembler.jump_if(Condition::Below, failed);
    }

    /// Jumps to `failed` unless the header word in `header` is of exactly
    /// the format whose bits are `format`, one bit of the format's five
    /// set: two tests, which keep `header`, where a mask would clobber it.
    /// Clobbers the flags.
    fn jump_unless_format(&mut self, header: Register, format: i32, failed: Label) {
        let layout = crate::heap::Heap::machine_layout();
        debug_assert!(
            format.count_ones() == 1 && format & !layout.instance_format_bits == 0,
            "internal: the format tested is one bit of the format's"
        );
        let assembler = &mut self.assembler;
        assembler.test_immediate(header, layout.instance_format_bits & !format);
        assembler.jump_if(Condition::NotEqual, failed);
        assembler.test_immediate(header, format);
        assembler.jump_if(Condition::Equal, failed);
    }

    /// Jumps to `failed` unless the word in `value` names an object whose
    /// header's format bits are `format`'s, as `Heap::object_frame` and
    /// the closure accessors tell a frame object and a closure: an
    /// immediate, `nil` among them, jumps too. `value` is kept; clobbers
    /// the flags and `scratch`.
    pub(crate) fn jump_unless_object_of_format(
        &mut self,
        value: Register,
        format: i32,
        scratch: Register,
        failed: Label,
    ) {
        debug_assert!(value != scratch, "internal: the check keeps its value");
        let layout = crate::heap::Heap::machine_layout();
        self.jump_unless_object_word(value, failed);
        let object_space = self.roles.object_space;
        let assembler = &mut self.assembler;
        assembler.move_register(scratch, value);
        assembler.add(scratch, object_space);
        assembler.load(
            Width::Word64,
            scratch,
            Memory {
                base: scratch,
                displacement: layout.header_displacement,
            },
        );
        assembler.and_immediate(scratch, layout.instance_format_bits);
        assembler.compare_immediate(scratch, format);
        assembler.jump_if(Condition::NotEqual, failed);
    }

    /// `count :=` the count of the words after the header of the object
    /// `object` names -- its class word and its fields -- the header's,
    /// or the overflow word's when the header says it is there. `object`
    /// is kept; clobbers the flags.
    pub(crate) fn load_word_count(&mut self, object: Register, count: Register) {
        debug_assert!(object != count, "internal: the count keeps its object");
        let layout = crate::heap::Heap::machine_layout();
        let object_space = self.roles.object_space;
        let assembler = &mut self.assembler;
        assembler.move_register(count, object);
        assembler.add(count, object_space);
        assembler.load(
            Width::Word64,
            count,
            Memory {
                base: count,
                displacement: layout.header_displacement,
            },
        );
        assembler.shift_right_logical(count, layout.word_count_shift);
        self.load_overflow_count(object, count);
    }

    /// `count :=` the count of the words after the header of the object
    /// `object` names, `count` holding the header's count when called:
    /// the overflow word's, below its mark, when the header's says it is
    /// there, and the header's otherwise. Clobbers the flags.
    fn load_overflow_count(&mut self, object: Register, count: Register) {
        let layout = crate::heap::Heap::machine_layout();
        let object_space = self.roles.object_space;
        let assembler = &mut self.assembler;
        let in_header = assembler.label();
        assembler.compare_immediate(count, layout.word_count_overflow);
        assembler.jump_if(Condition::NotEqual, in_header);
        assembler.move_register(count, object);
        assembler.add(count, object_space);
        assembler.load(
            Width::Word64,
            count,
            Memory {
                base: count,
                displacement: layout.header_displacement - WORD_BYTES,
            },
        );
        // The overflow word's count lies below its mark.
        let mark_bits = 64 - layout.word_count_shift;
        assembler.shift_left(count, mark_bits);
        assembler.shift_right_logical(count, mark_bits);
        assembler.bind(in_header);
    }

    /// `address :=` the address of the element the one-based `index`
    /// names of the `Array` `object` names, as `collections::element_at`
    /// and `element_at_put` find it: jumps to `failed`, nothing written,
    /// unless `object` names an object of an `Array`'s format
    /// (`Format::IndexablePointers`) and `index` holds a `SmallInteger`
    /// from 1 to its element count -- the header's count, or the overflow
    /// word's, less its class word. `object` is kept; clobbers the flags,
    /// `index` and the idiom scratch register.
    pub(crate) fn load_element_address(
        &mut self,
        object: Register,
        index: Register,
        address: Register,
        failed: Label,
    ) {
        let layout = crate::heap::Heap::machine_layout();
        let (scratch, object_space) = (self.roles.idiom_scratch, self.roles.object_space);
        debug_assert!(
            distinct(&[object, index, address, scratch]),
            "internal: an element's registers are distinct and none is the idiom scratch register"
        );
        self.jump_unless_object_word(object, failed);
        let assembler = &mut self.assembler;
        assembler.test_immediate(index, layout.small_integer_tag);
        assembler.jump_if(Condition::Equal, failed);
        assembler.move_register(address, object);
        assembler.add(address, object_space);
        assembler.load(
            Width::Word64,
            scratch,
            Memory {
                base: address,
                displacement: layout.header_displacement,
            },
        );
        self.jump_unless_format(scratch, layout.indexable_pointers_format, failed);
        self.assembler
            .shift_right_logical(scratch, layout.word_count_shift);
        self.load_overflow_count(object, scratch);
        // The elements' bytes, the class word's taken off the count, and
        // the index's byte offset from the first element: a
        // `SmallInteger`'s word is its value times eight plus its tag, so
        // the word less eight and the tag is the offset, negative -- past
        // every offset, unsigned -- for an index below 1.
        let assembler = &mut self.assembler;
        assembler.shift_left(scratch, WORD_SHIFT);
        assembler.add_immediate(scratch, -WORD_BYTES);
        assembler.add_immediate(index, -(WORD_BYTES + layout.small_integer_tag));
        assembler.compare(index, scratch);
        assembler.jump_if(Condition::AboveOrEqual, failed);
        assembler.add(address, index);
        assembler.add_immediate(address, layout.fields_displacement);
    }

    /// `address :=` the address of the byte the one-based `index` names
    /// of the `ByteArray` `object` names, as `bytes::element_at` and
    /// `element_at_put` find it: jumps to `failed`, nothing written,
    /// unless `object` names an object of a ByteArray's format
    /// (`Format::AlignedBytes`) and `index` holds a `SmallInteger` from 1
    /// to its size -- its data words' bytes, counted from the header's
    /// count or the overflow word's, less the unused bytes of the last.
    /// The data begin at the first `byte_data_alignment` offset at or
    /// after field 0's, computed from the object's offset as
    /// `object::aligned_data_start` computes it, never stored, which is
    /// why every mover realigns them. `object` is kept; clobbers the
    /// flags, `index`, `size` and the idiom scratch register.
    pub(crate) fn load_byte_address(
        &mut self,
        object: Register,
        index: Register,
        address: Register,
        size: Register,
        failed: Label,
    ) {
        let layout = crate::heap::Heap::machine_layout();
        let (scratch, object_space) = (self.roles.idiom_scratch, self.roles.object_space);
        debug_assert!(
            distinct(&[object, index, address, size, scratch]),
            "internal: a byte's registers are distinct and none is the idiom scratch register"
        );
        self.jump_unless_object_word(object, failed);
        let assembler = &mut self.assembler;
        assembler.test_immediate(index, layout.small_integer_tag);
        assembler.jump_if(Condition::Equal, failed);
        assembler.move_register(address, object);
        assembler.add(address, object_space);
        assembler.load(
            Width::Word64,
            scratch,
            Memory {
                base: address,
                displacement: layout.header_displacement,
            },
        );
        // Each bit of the format's code a ByteArray's codes share, tested
        // alone, so that the header is kept.
        let format = layout.aligned_bytes_format;
        let low_bit = format.isolate_lowest_one();
        for bit in [low_bit, format & !low_bit] {
            assembler.test_immediate(scratch, bit);
            assembler.jump_if(Condition::Equal, failed);
        }
        assembler.move_register(size, scratch);
        assembler.shift_right_logical(size, layout.unused_bytes_shift);
        assembler.and_immediate(size, layout.unused_bytes_mask);
        assembler.shift_right_logical(scratch, layout.word_count_shift);
        self.load_overflow_count(object, scratch);
        // The size: the data words' bytes less the unused ones; and the
        // index's position, below 0 -- past every size, unsigned -- for an
        // index below 1.
        let assembler = &mut self.assembler;
        assembler.shift_left(scratch, WORD_SHIFT);
        assembler.add_immediate(scratch, -layout.byte_data_reserve_bytes);
        assembler.subtract(scratch, size);
        assembler.shift_right_arithmetic(index, layout.tag_bits);
        assembler.add_immediate(index, -1);
        assembler.compare(index, scratch);
        assembler.jump_if(Condition::AboveOrEqual, failed);
        // The data's offset: field 0's rounded up to the alignment.
        let alignment = layout.byte_data_alignment;
        assembler.move_register(address, object);
        assembler.add_immediate(address, layout.fields_displacement + alignment - 1);
        assembler.and_immediate(address, -alignment);
        assembler.add(address, object_space);
        assembler.add(address, index);
    }

    /// Jumps to `failed` unless `value` holds the word of an object: an
    /// object's tag, and not the word 0, which `Word::object_offset`
    /// refuses. Clobbers the flags.
    fn jump_unless_object_word(&mut self, value: Register, failed: Label) {
        self.jump_unless_object(value, failed);
        let assembler = &mut self.assembler;
        assembler.compare_immediate(value, 0);
        assembler.jump_if(Condition::Equal, failed);
    }

    /// Takes `exit(kind, program_counter)` at `label`, out of line: the
    /// target of a check's jump to `failed`.
    pub(crate) fn exit_at(&mut self, label: Label, kind: ExitKind, program_counter: u32) {
        self.push_cold_path(ColdPath::Exit {
            label,
            kind,
            program_counter,
        });
    }

    /// Adds one to the entry block's `BackEdges` when the code counts its
    /// back edges (`set_count_back_edges`), and emits nothing otherwise.
    /// Clobbers the flags and the idiom scratch register.
    pub(crate) fn count_back_edge(&mut self) {
        self.count(BlockWord::BackEdges);
    }

    /// Spends `amount` of the body's optimization budget
    /// (`heap/optimization_budgets.rs`): subtracts it from the word whose
    /// address the body's data word `word` holds, and out of line, when
    /// the word fell below zero, calls `Leaf::Budget` with the address,
    /// the frame's -- whose header names the key a method's body is
    /// optimized for (`jit::budget_spent`) -- and the code's own address,
    /// then goes on where the spend ends, since the leaf changes nothing
    /// the code reads. Clobbers the flags and the idiom scratch register.
    ///
    /// The data word names the row rather than the code holding a code
    /// id, since a full collection may free a code id and mint it again
    /// for another body: the JIT points the word at the body's row, and
    /// afresh after every full collection. Until it is pointed the word
    /// names the unpointed budget (`MachineWord::UnpointedBudget`), which
    /// is negative, so that the hot path is the load of the address, the
    /// subtract and one branch, with no test for a word naming none.
    ///
    /// # Panics
    ///
    /// On an amount past `i32::MAX`, which no body's length reaches: the
    /// compile refuses a body past `u16::MAX` instructions.
    pub(crate) fn spend_budget(&mut self, word: usize, amount: u32) {
        let scratch = self.roles.idiom_scratch;
        let amount = i32::try_from(amount).expect("internal: a body's length fits an immediate");
        let (cold, rejoin) = (self.assembler.label(), self.assembler.label());
        self.load_body_word(scratch, word);
        self.assembler.subtract_from_memory(
            Memory {
                base: scratch,
                displacement: 0,
            },
            amount,
        );
        self.assembler.jump_if(Condition::Less, cold);
        self.assembler.bind(rejoin);
        self.call_leaf_out_of_line(
            cold,
            super::leaves::Leaf::Budget.index(),
            [
                Operand::Register(scratch),
                Operand::Register(self.roles.frame),
                Operand::CodeAddress,
            ],
            scratch,
            rejoin,
        );
    }

    /// Adds one to the entry block's `CompiledCalls` when the code counts
    /// (`set_count_back_edges`), and emits nothing otherwise. Clobbers the
    /// flags and the idiom scratch register.
    pub(crate) fn count_compiled_call(&mut self) {
        self.count(BlockWord::CompiledCalls);
    }

    /// Adds one to the entry block's `DispatchedCalls` when the code
    /// counts, and emits nothing otherwise. Clobbers the flags and the
    /// idiom scratch register.
    pub(crate) fn count_dispatched_call(&mut self) {
        self.count(BlockWord::DispatchedCalls);
    }

    /// Adds one to the block's `word` when the code counts.
    fn count(&mut self, word: BlockWord) {
        if !self.count_back_edges {
            return;
        }
        let scratch = self.roles.idiom_scratch;
        let count = self.block_word(word);
        self.assembler.load(Width::Word64, scratch, count);
        self.assembler.add_immediate(scratch, 1);
        self.assembler.store(Width::Word64, count, scratch);
    }

    /// `target :=` the machine words' address (`BlockWord::MachineWords`).
    /// Keeps the flags.
    pub(crate) fn load_machine_words(&mut self, target: Register) {
        let machine_words = self.block_word(BlockWord::MachineWords);
        self.assembler.load(Width::Word64, target, machine_words);
    }

    /// The machine word `word`, given the machine words' address in
    /// `machine_words`.
    pub(crate) fn machine_word(machine_words: Register, word: MachineWord) -> Memory {
        Memory {
            base: machine_words,
            displacement: MachineWords::offset(word),
        }
    }

    /// `target :=` the guard a cell is compared with for the value in
    /// `value` (`interpreter::inline_cache::cells::Cell`), as
    /// `Heap::dispatch_id` finds it: an object's header word's low 22
    /// bits, an immediate's `MachineWord::ImmediateGuard`. Clobbers the
    /// flags and the idiom scratch register; `value` is kept.
    pub(crate) fn load_guard(&mut self, target: Register, value: Register) {
        let layout = crate::heap::Heap::machine_layout();
        self.load_by_tag(
            target,
            value,
            layout.header_displacement,
            MachineWord::ImmediateGuard(0),
            Some(layout.guard_mask),
        );
    }

    /// `target :=` the class object of the value in `value`, as the word
    /// a header place stores it (`Heap::store`), as `Mutator::class_of`
    /// finds it: an object's class word, an immediate's
    /// `MachineWord::ImmediateClass`. Clobbers the flags and the idiom
    /// scratch register; `value` is kept.
    pub(crate) fn load_class(&mut self, target: Register, value: Register) {
        let layout = crate::heap::Heap::machine_layout();
        self.load_by_tag(
            target,
            value,
            layout.class_word_displacement,
            MachineWord::ImmediateClass(0),
            None,
        );
    }

    /// `target :=` the word at `object_displacement` of the object `value`
    /// names, masked by `mask` when one is given; or, for an immediate,
    /// the machine word `first_immediate` is the first of a family of, at
    /// the immediate's index (`Value::immediate_index`).
    fn load_by_tag(
        &mut self,
        target: Register,
        value: Register,
        object_displacement: i32,
        first_immediate: MachineWord,
        mask: Option<i32>,
    ) {
        debug_assert!(
            target != value
                && target != self.roles.idiom_scratch
                && value != self.roles.idiom_scratch,
            "internal: the guard and class idioms keep their value and use the idiom scratch register"
        );
        let layout = crate::heap::Heap::machine_layout();
        let object_space = self.roles.object_space;
        let scratch = self.roles.idiom_scratch;
        let machine_words = self.block_word(BlockWord::MachineWords);
        let assembler = &mut self.assembler;
        let (immediate, have_index, done) =
            (assembler.label(), assembler.label(), assembler.label());
        assembler.test_immediate(value, layout.tag_mask);
        assembler.jump_if(Condition::NotEqual, immediate);
        assembler.move_register(target, value);
        assembler.add(target, object_space);
        assembler.load(
            Width::Word64,
            target,
            Memory {
                base: target,
                displacement: object_displacement,
            },
        );
        if let Some(mask) = mask {
            assembler.and_immediate(target, mask);
        }
        assembler.jump(done);
        assembler.bind(immediate);
        // The immediate's index: a `SmallInteger`'s, a float's, else the
        // constant's payload.
        assembler.move_immediate(target, layout.small_integer_index);
        assembler.test_immediate(value, layout.small_integer_tag);
        assembler.jump_if(Condition::NotEqual, have_index);
        assembler.move_immediate(target, layout.small_float_index);
        assembler.test_immediate(value, layout.small_float_tag);
        assembler.jump_if(Condition::NotEqual, have_index);
        assembler.move_register(target, value);
        assembler.shift_right_arithmetic(target, layout.tag_bits);
        assembler.bind(have_index);
        assembler.shift_left(target, WORD_SHIFT);
        assembler.load(Width::Word64, scratch, machine_words);
        assembler.add(target, scratch);
        assembler.load(
            Width::Word64,
            target,
            Memory {
                base: target,
                displacement: MachineWords::offset(first_immediate),
            },
        );
        assembler.bind(done);
    }

    /// Steps the class object in `class` up its superclass chain as many
    /// times as `level` says, counting `level` down to 0, as
    /// `inline_cache::level_above` steps it; jumps to `failed` when a
    /// superclass is missing before the level is reached, which makes the
    /// send a miss as it makes the interpreter's. Clobbers the flags.
    pub(crate) fn walk_superclasses(&mut self, class: Register, level: Register, failed: Label) {
        let layout = crate::heap::Heap::machine_layout();
        let object_space = self.roles.object_space;
        let assembler = &mut self.assembler;
        let (step, done) = (assembler.label(), assembler.label());
        assembler.bind(step);
        assembler.compare_immediate(level, 0);
        assembler.jump_if(Condition::Equal, done);
        assembler.add(class, object_space);
        assembler.load(
            Width::Word64,
            class,
            Memory {
                base: class,
                displacement: layout.superclass_displacement,
            },
        );
        assembler.test_immediate(class, layout.tag_mask);
        assembler.jump_if(Condition::NotEqual, failed);
        assembler.add_immediate(level, -1);
        assembler.jump(step);
        assembler.bind(done);
    }

    /// `target :=` §3.3's enclosing object at lexical `depth` of the frame
    /// whose code takes `argument_count` arguments, as
    /// `dispatch::kth_enclosing_object` finds it from
    /// `LexicalOrigin::Class`, instruction for instruction: the receiver,
    /// header place 0, at depth 0; otherwise the walk out from the current
    /// class, header place 1, along the declaring-class fields, each
    /// level's enclosing-object field the object one step out, and `nil`
    /// where a level, or the object, is missing -- a top-level
    /// activation's, a module root's. Both header places are written by
    /// the time the frame's code runs. Clobbers the flags and the idiom
    /// scratch register.
    pub(crate) fn load_enclosing_object(
        &mut self,
        target: Register,
        argument_count: u16,
        depth: u16,
    ) {
        let receiver_place = crate::heap::Heap::receiver_register(argument_count);
        let receiver = self.frame_slot(receiver_place);
        let current_class =
            self.frame_slot(crate::heap::Heap::current_class_register(argument_count));
        let layout = crate::heap::Heap::machine_layout();
        let object_space = self.roles.object_space;
        let scratch = self.roles.idiom_scratch;
        let assembler = &mut self.assembler;
        if depth == 0 {
            assembler.load(Width::Word64, target, receiver);
            return;
        }
        let (missing, done) = (assembler.label(), assembler.label());
        assembler.load(Width::Word64, target, current_class);
        for step in 1..=depth {
            // The level: the current class, then each declaring class.
            assembler.test_immediate(target, layout.tag_mask);
            assembler.jump_if(Condition::NotEqual, missing);
            assembler.move_register(scratch, target);
            assembler.add(scratch, object_space);
            assembler.load(
                Width::Word64,
                target,
                Memory {
                    base: scratch,
                    displacement: layout.enclosing_displacement,
                },
            );
            assembler.test_immediate(target, layout.tag_mask);
            assembler.jump_if(Condition::NotEqual, missing);
            if step < depth {
                assembler.load(
                    Width::Word64,
                    target,
                    Memory {
                        base: scratch,
                        displacement: layout.declaring_class_displacement,
                    },
                );
            }
        }
        assembler.jump(done);
        assembler.bind(missing);
        assembler.move_immediate(target, crate::heap::Immediate::NIL.bits());
        assembler.bind(done);
    }

    /// `target :=` §5.9's N at lexical `depth` of the frame whose code
    /// takes `argument_count` arguments, as `dispatch::kth_lexical_class`
    /// finds it from `LexicalOrigin::Class`, instruction for instruction:
    /// the current class, header place 1, then `depth` steps out along the
    /// declaring-class fields; a jump to `missing` where the chain is
    /// shorter, where the Rust answers `None`, with `target` then holding
    /// no class -- the word that ended the walk -- which the code at
    /// `missing` must not read. Clobbers the flags.
    pub(crate) fn load_lexical_class(
        &mut self,
        target: Register,
        argument_count: u16,
        depth: u16,
        missing: Label,
    ) {
        let current_class =
            self.frame_slot(crate::heap::Heap::current_class_register(argument_count));
        let layout = crate::heap::Heap::machine_layout();
        let object_space = self.roles.object_space;
        let assembler = &mut self.assembler;
        assembler.load(Width::Word64, target, current_class);
        for _ in 0..depth {
            assembler.test_immediate(target, layout.tag_mask);
            assembler.jump_if(Condition::NotEqual, missing);
            assembler.add(target, object_space);
            assembler.load(
                Width::Word64,
                target,
                Memory {
                    base: target,
                    displacement: layout.declaring_class_displacement,
                },
            );
        }
        assembler.test_immediate(target, layout.tag_mask);
        assembler.jump_if(Condition::NotEqual, missing);
    }

    /// `target :=` S, the superclass of the current class of the frame
    /// whose code takes `argument_count` arguments, as
    /// `interpreter::sends::super_start` finds it, exiting at the send,
    /// `program_counter`, where there is no current class or no
    /// superclass, so that the interpreter raises the error the uncached
    /// `super` send raises. Clobbers the flags.
    pub(crate) fn load_superclass(
        &mut self,
        target: Register,
        argument_count: u16,
        program_counter: u32,
    ) {
        let layout = crate::heap::Heap::machine_layout();
        let current_class =
            self.frame_slot(crate::heap::Heap::current_class_register(argument_count));
        let object_space = self.roles.object_space;
        self.assembler.load(Width::Word64, target, current_class);
        self.assembler.test_immediate(target, layout.tag_mask);
        self.exit_if(Condition::NotEqual, ExitKind::Reexecute, program_counter);
        self.assembler.add(target, object_space);
        self.assembler.load(
            Width::Word64,
            target,
            Memory {
                base: target,
                displacement: layout.superclass_displacement,
            },
        );
        self.assembler.test_immediate(target, layout.tag_mask);
        self.exit_if(Condition::NotEqual, ExitKind::Reexecute, program_counter);
    }

    /// Jumps to `missed` unless the guard of the value in `value`
    /// (`load_guard`) is the word at `cell_guard`, `guard` its register.
    /// Clobbers the flags and the idiom scratch register; `value` is kept.
    pub(crate) fn compare_receiver_guard(
        &mut self,
        guard: Register,
        value: Register,
        cell_guard: Memory,
        missed: Label,
    ) {
        self.load_guard(guard, value);
        self.assembler.compare_memory(guard, cell_guard);
        self.assembler.jump_if(Condition::NotEqual, missed);
    }

    /// Jumps to `missed` unless the application field of the class object
    /// in `class` (`Heap::class_application_word`) holds the word at
    /// `cell_guard`, `guard` its register. Clobbers the flags; `class` is
    /// kept.
    pub(crate) fn compare_application_word(
        &mut self,
        guard: Register,
        class: Register,
        cell_guard: Memory,
        missed: Label,
    ) {
        self.load_application_word(guard, class);
        self.assembler.compare_memory(guard, cell_guard);
        self.assembler.jump_if(Condition::NotEqual, missed);
    }

    /// `target :=` the instances dispatch id of the application whose class
    /// objects hold `word`, the register's application word
    /// (`Heap::class_application_word`), as the application id table
    /// records it (`heap/application_ids.rs`) -- `DispatchId::NONE`'s 0 for
    /// a row that holds no application; `target` may be `word`. Jumps to
    /// `not_an_application` for a word that is no `SmallInteger`, which
    /// names no application -- `nil`, a crafted image's class -- and to
    /// `unrecorded` for an index at or past the table's bound, which no
    /// class object of the heap holds unless the table ran out of room.
    /// Clobbers the flags and the idiom scratch register.
    pub(crate) fn load_application_id(
        &mut self,
        target: Register,
        word: Register,
        not_an_application: Label,
        unrecorded: Label,
    ) {
        let layout = crate::heap::Heap::machine_layout();
        debug_assert!(
            target != self.roles.idiom_scratch && word != self.roles.idiom_scratch,
            "internal: the application id's idiom uses the idiom scratch register"
        );
        let assembler = &mut self.assembler;
        assembler.test_immediate(word, layout.small_integer_tag);
        assembler.jump_if(Condition::Equal, not_an_application);
        assembler.move_register(target, word);
        assembler.shift_right_arithmetic(target, layout.tag_bits);
        self.load_application_row(target, unrecorded);
        self.assembler.load(
            Width::Word64,
            target,
            Memory {
                base: target,
                displacement: layout.application_dispatch_id_displacement,
            },
        );
    }

    /// `row :=` the address of the application id table's row
    /// (`heap/application_ids.rs`) of the application whose index `row`
    /// holds, untagged; jumps to `unrecorded` for an index at or past the
    /// table's bound, a negative one included. Clobbers the flags and the
    /// idiom scratch register.
    fn load_application_row(&mut self, row: Register, unrecorded: Label) {
        let layout = crate::heap::Heap::machine_layout();
        let scratch = self.roles.idiom_scratch;
        debug_assert_ne!(
            row, scratch,
            "internal: the row's idiom uses the idiom scratch register"
        );
        let machine_words = self.block_word(BlockWord::MachineWords);
        let assembler = &mut self.assembler;
        assembler.load(Width::Word64, scratch, machine_words);
        // Unsigned, so that a negative index is past the bound too.
        assembler.compare_memory(
            row,
            Memory {
                base: scratch,
                displacement: MachineWords::offset(MachineWord::ApplicationIdRows),
            },
        );
        assembler.jump_if(Condition::AboveOrEqual, unrecorded);
        assembler.load(
            Width::Word64,
            scratch,
            Memory {
                base: scratch,
                displacement: MachineWords::offset(MachineWord::ApplicationIds),
            },
        );
        assembler.shift_left(row, layout.application_row_shift);
        assembler.add(row, scratch);
    }

    /// Jumps to `cold` unless the receiver of the frame, whose code takes
    /// `argument_count` arguments, is an object whose header's guard is
    /// the one the frame recorded (`frame_header::GUARD`) and not
    /// `DispatchId::NONE`: the hot path of
    /// `interpreter::locals::check_own_slots`, one header load and one
    /// compare. `receiver :=` the receiver; clobbers the flags and `guard`.
    pub(crate) fn check_own_slot_guard(
        &mut self,
        argument_count: u16,
        receiver: Register,
        guard: Register,
        cold: Label,
    ) {
        debug_assert_ne!(
            receiver, guard,
            "internal: the guard check keeps its receiver"
        );
        let layout = crate::heap::Heap::machine_layout();
        let (guard_register, _) = crate::heap::Heap::own_slot_registers(argument_count);
        let recorded = self.frame_slot(guard_register);
        let object_space = self.roles.object_space;
        self.load_receiver(receiver, argument_count);
        self.jump_unless_object(receiver, cold);
        let assembler = &mut self.assembler;
        assembler.move_register(guard, receiver);
        assembler.add(guard, object_space);
        assembler.load(
            Width::Word64,
            guard,
            Memory {
                base: guard,
                displacement: layout.header_displacement,
            },
        );
        // `NONE`, which a frame records when it verified nothing, is never
        // a match, whatever guard a header holds: a header's guard bits of
        // 0 are `NONE`'s (asserted below `WORD_BYTES`).
        assembler.test_immediate(guard, layout.guard_mask);
        assembler.jump_if(Condition::Equal, cold);
        // The frame holds the guard as a `SmallInteger`.
        assembler.and_immediate(guard, layout.guard_mask);
        assembler.shift_left(guard, layout.tag_bits);
        assembler.or_immediate(guard, layout.small_integer_tag);
        assembler.compare_memory(guard, recorded);
        assembler.jump_if(Condition::NotEqual, cold);
    }

    /// `address :=` the address of the receiver's own slot `own_index` --
    /// mixin-relative, offset by the own slot base of the level the frame
    /// was entered at (`frame_header::ENTERED_LEVEL`), read from that
    /// level's row of the application id table, as
    /// `Heap::activation_slot_base` reads it -- for a receiver in
    /// `receiver` whose guard `check_own_slot_guard` found the frame's.
    /// Jumps to `failed`, nothing written, unless the entered level has a
    /// row and the receiver is an instance with that field
    /// (`Heap::instance_field`'s checks), the count the header's or the
    /// overflow word's. `receiver` is kept; clobbers the flags, `count`
    /// and the idiom scratch register.
    pub(crate) fn load_own_slot_address(
        &mut self,
        argument_count: u16,
        own_index: i32,
        receiver: Register,
        address: Register,
        count: Register,
        failed: Label,
    ) {
        debug_assert!(
            distinct(&[receiver, address, count, self.roles.idiom_scratch]),
            "internal: the own slot's idiom takes distinct registers"
        );
        let layout = crate::heap::Heap::machine_layout();
        let (_, entered_level) = crate::heap::Heap::own_slot_registers(argument_count);
        let entered_level = self.frame_slot(entered_level);
        let object_space = self.roles.object_space;
        let assembler = &mut self.assembler;
        assembler.load(Width::Word64, address, entered_level);
        assembler.shift_right_arithmetic(address, layout.tag_bits);
        self.load_application_row(address, failed);
        let assembler = &mut self.assembler;
        assembler.load(
            Width::Word64,
            address,
            Memory {
                base: address,
                displacement: layout.application_slot_base_displacement,
            },
        );
        assembler.add_immediate(address, own_index);
        assembler.move_register(count, receiver);
        assembler.add(count, object_space);
        assembler.load(
            Width::Word64,
            count,
            Memory {
                base: count,
                displacement: layout.header_displacement,
            },
        );
        assembler.test_immediate(count, layout.instance_format_bits);
        assembler.jump_if(Condition::NotEqual, failed);
        assembler.shift_right_logical(count, layout.word_count_shift);
        self.load_overflow_count(receiver, count);
        // The fields are the words after the header less the class word;
        // unsigned, so that a field index past them fails, however large.
        let assembler = &mut self.assembler;
        assembler.add_immediate(count, -1);
        assembler.compare(count, address);
        assembler.jump_if(Condition::BelowOrEqual, failed);
        assembler.shift_left(address, WORD_SHIFT);
        assembler.add(address, receiver);
        assembler.add(address, object_space);
    }

    /// Jumps to `checked` when the slots of the receiver, the object in
    /// `receiver`, take §3.1.1's check of what a value's slot may
    /// hold (`interpreter::instantiate::check_value_slot`): its class's
    /// application's row says so (`ROW_FLAG_VALUE_SLOTS`), or its class
    /// names no application the table records, which the check decides.
    /// `receiver` is kept; clobbers the flags, `scratch` and the idiom
    /// scratch register.
    pub(crate) fn jump_if_value_slots(
        &mut self,
        receiver: Register,
        scratch: Register,
        checked: Label,
    ) {
        let layout = crate::heap::Heap::machine_layout();
        let object_space = self.roles.object_space;
        let assembler = &mut self.assembler;
        assembler.move_register(scratch, receiver);
        assembler.add(scratch, object_space);
        assembler.load(
            Width::Word64,
            scratch,
            Memory {
                base: scratch,
                displacement: layout.class_word_displacement,
            },
        );
        self.load_application_word(scratch, scratch);
        let assembler = &mut self.assembler;
        assembler.test_immediate(scratch, layout.small_integer_tag);
        assembler.jump_if(Condition::Equal, checked);
        assembler.shift_right_arithmetic(scratch, layout.tag_bits);
        self.load_application_row(scratch, checked);
        let assembler = &mut self.assembler;
        assembler.load(
            Width::Word64,
            scratch,
            Memory {
                base: scratch,
                displacement: layout.application_flags_displacement,
            },
        );
        assembler.test_immediate(scratch, layout.application_value_slots_flag);
        assembler.jump_if(Condition::NotEqual, checked);
    }

    /// Jumps to `label` unless the word in `value` names an object -- an
    /// immediate, `nil` among them. Clobbers the flags.
    pub(crate) fn jump_unless_object(&mut self, value: Register, label: Label) {
        let layout = crate::heap::Heap::machine_layout();
        self.assembler.test_immediate(value, layout.tag_mask);
        self.assembler.jump_if(Condition::NotEqual, label);
    }

    /// `target :=` the application word of the class object in `class`
    /// (`Heap::class_application_word`). Clobbers the flags; `class` is
    /// kept unless it is `target`.
    pub(crate) fn load_application_word(&mut self, target: Register, class: Register) {
        let layout = crate::heap::Heap::machine_layout();
        let object_space = self.roles.object_space;
        let assembler = &mut self.assembler;
        assembler.move_register(target, class);
        assembler.add(target, object_space);
        assembler.load(
            Width::Word64,
            target,
            Memory {
                base: target,
                displacement: layout.application_displacement,
            },
        );
    }

    /// A debug build's flags word of a frame a compiled prologue pushes,
    /// for code that takes `argument_count` arguments: the generation one
    /// past the machine words' `NextGeneration`, which it advances, as the
    /// heap's push takes it, so that a handle to a frame since returned
    /// and pushed again is refused. Clobbers the flags, the idiom scratch
    /// register, `poison` and `depth_count`, whose values are dead here.
    #[cfg(debug_assertions)]
    pub(crate) fn store_frame_generation(&mut self, argument_count: u16) {
        let (generation, constant) = crate::heap::Heap::machine_flags(argument_count);
        let (word, constant_word) = (self.roles.poison, self.roles.depth_count);
        let machine_words = self.roles.idiom_scratch;
        self.load_machine_words(machine_words);
        let next = Self::machine_word(machine_words, MachineWord::NextGeneration);
        let flags = self.frame_slot(argument_count + generation.flags_register);
        let assembler = &mut self.assembler;
        assembler.load(Width::Word64, word, next);
        assembler.add_immediate(word, 1);
        assembler.and_immediate(word, generation.mask);
        assembler.store(Width::Word64, next, word);
        assembler.shift_left(word, generation.shift);
        assembler.move_immediate(constant_word, constant);
        assembler.add(word, constant_word);
        assembler.store(Width::Word64, flags, word);
    }

    /// Writes the kind the initializer leaf handed over
    /// (`MachineWord::CalleeKind`) into the flags word of a frame whose
    /// code takes `argument_count` arguments, as the walk reads it back
    /// (`Heap::write_machine_frame`): added to the flags a debug build's
    /// generation was just written with, or to a method's constant flags.
    /// Clobbers `kind` and the idiom scratch register.
    pub(crate) fn record_frame_kind(&mut self, argument_count: u16, kind: Register) {
        let machine_words = self.roles.idiom_scratch;
        self.load_machine_words(machine_words);
        let flags = self.frame_slot(argument_count + crate::heap::Heap::flags_register());
        let assembler = &mut self.assembler;
        assembler.load(
            Width::Word64,
            kind,
            Self::machine_word(machine_words, MachineWord::CalleeKind),
        );
        #[cfg(debug_assertions)]
        assembler.load(Width::Word64, machine_words, flags);
        #[cfg(not(debug_assertions))]
        assembler.move_immediate(
            machine_words,
            crate::heap::Heap::machine_flags_word(argument_count),
        );
        assembler.add(kind, machine_words);
        assembler.store(Width::Word64, flags, kind);
    }

    /// Takes `exit(Safepoint, program_counter)` when the heap's safepoint
    /// request (`MachineWord::SafepointRequested`) is set, and falls
    /// through otherwise. Clobbers the flags and
    /// the idiom scratch register. The exit is out of line, so a poll that
    /// finds nothing costs two loads, a compare and a branch not taken.
    ///
    /// Under the exit stress mode the exit is unconditional, so that every
    /// poll's exit, and the interpreter's resumption after it, run under
    /// the whole suite without waiting for a safepoint to be requested.
    pub(crate) fn poll(&mut self, program_counter: u32) {
        if self.exit_at_every_poll {
            self.exit(ExitKind::Safepoint, program_counter);
            return;
        }
        let label = self.assembler.label();
        self.jump_if_safepoint_requested(label);
        self.push_cold_path(ColdPath::Exit {
            label,
            kind: ExitKind::Safepoint,
            program_counter,
        });
    }

    /// A poll whose exit is the caller's: jumps to `requested` when the
    /// heap's safepoint request is set -- always under the exit stress
    /// mode, as `poll` exits -- and falls through otherwise, so that
    /// optimized code writes back the values it holds on that way alone
    /// before its exit ("A poll writes nothing back", `execution.md`).
    /// Clobbers the flags and the idiom scratch register.
    pub(crate) fn poll_to(&mut self, requested: Label) {
        if self.exit_at_every_poll {
            self.assembler.jump(requested);
            return;
        }
        self.jump_if_safepoint_requested(requested);
    }

    /// Under the deopt stress mode, an interrupt point's countdown: one
    /// taken off `MachineWord::DeoptCountdown`, and the exit that
    /// re-executes the instruction at `program_counter`, out of line, when
    /// that brought it to zero, so that the frame -- written back, as at
    /// every interrupt point -- goes on interpreted there as at any of the
    /// point's own exits. Emits nothing outside the mode. Clobbers the
    /// flags and the idiom scratch register.
    pub(crate) fn count_down_to_deoptimize(&mut self, program_counter: u32) {
        if !self.deoptimize_by_countdown {
            return;
        }
        let label = self.assembler.label();
        self.count_down_to(label);
        self.push_cold_path(ColdPath::Exit {
            label,
            kind: ExitKind::Reexecute,
            program_counter,
        });
    }

    /// Under the deopt stress mode, an interrupt point's countdown whose
    /// exit is the caller's: a jump to `exhausted` when the count reached
    /// zero, as `count_down_to_deoptimize` exits; nothing outside the
    /// mode. A back edge's poll goes to its own exit, the one way out of
    /// it. Clobbers the flags and the idiom scratch register.
    pub(crate) fn count_down_to(&mut self, exhausted: Label) {
        if !self.deoptimize_by_countdown {
            return;
        }
        let scratch = self.roles.idiom_scratch;
        self.load_machine_words(scratch);
        // The flags compare the word before the subtraction with 1: equal
        // exactly when this point brought it to zero, so that a countdown
        // already spent, negative until the safepoint sets it afresh,
        // never exits again.
        self.assembler
            .subtract_from_memory(Self::machine_word(scratch, MachineWord::DeoptCountdown), 1);
        self.assembler.jump_if(Condition::Equal, exhausted);
    }

    /// The poll's test: a jump to `label` when the safepoint request is
    /// set. Clobbers the flags and the idiom scratch register.
    fn jump_if_safepoint_requested(&mut self, label: Label) {
        let scratch = self.roles.idiom_scratch;
        let machine_words = self.block_word(BlockWord::MachineWords);
        self.assembler.load(Width::Word64, scratch, machine_words);
        let requested = Memory {
            base: scratch,
            displacement: MachineWords::offset(MachineWord::SafepointRequested),
        };
        self.assembler.load(Width::Word64, scratch, requested);
        self.assembler.compare_immediate(scratch, 0);
        self.assembler.jump_if(Condition::NotEqual, label);
    }

    /// Lays the code out, the cold paths after the straight line, so a
    /// check that passes falls through rather than jumping over its
    /// failure.
    pub(crate) fn finish(mut self) -> Result<Assembled, AssemblyError> {
        #[cfg(debug_assertions)]
        let mut holds = std::mem::take(&mut self.cold_path_holds).into_iter();
        for cold_path in std::mem::take(&mut self.cold_paths) {
            // A cold path runs where the straight line branched off to it,
            // so it is checked against the values held there.
            #[cfg(debug_assertions)]
            {
                self.values_held = holds
                    .next()
                    .expect("internal: every cold path records what was held");
            }
            match cold_path {
                ColdPath::Exit {
                    label,
                    kind,
                    program_counter,
                } => {
                    self.assembler.bind(label);
                    self.exit(kind, program_counter);
                }
                ColdPath::LeafCall {
                    label,
                    leaf,
                    arguments,
                    result,
                    rejoin,
                } => {
                    self.assembler.bind(label);
                    self.call_leaf(leaf, arguments);
                    let result = self.temporary(result);
                    self.assembler.move_register(result, self.roles.leaf_result);
                    self.assembler.jump(rejoin);
                }
                ColdPath::CheckedLeafCall {
                    label,
                    leaf,
                    arguments,
                    program_counter,
                    rejoin,
                } => {
                    self.assembler.bind(label);
                    self.call_leaf(leaf, arguments);
                    self.assembler.compare_immediate(self.roles.leaf_result, 0);
                    self.assembler.jump_if(Condition::Equal, rejoin);
                    self.exit(ExitKind::Reexecute, program_counter);
                }
                ColdPath::StackGrowth {
                    label,
                    leaf,
                    extent,
                    kind,
                    rejoin,
                } => {
                    self.assembler.bind(label);
                    let handover = self.roles.call_handover();
                    for register in handover {
                        self.assembler.push(register);
                    }
                    self.call_leaf(
                        leaf,
                        [
                            Operand::Register(extent),
                            Operand::Immediate(0),
                            Operand::Immediate(0),
                        ],
                    );
                    // A debug build's check of the handover: every scratch
                    // register it does not keep, the idioms' own among
                    // them -- but the leaf's answer,
                    // read below -- holds the dead register's word, whatever
                    // the convention's callee preserved, so that a register
                    // a prologue reads after its check and the handover
                    // misses fails at once on the growth path, as it would
                    // only by chance in a release build.
                    #[cfg(debug_assertions)]
                    for &register in self.roles.scratch.iter().chain([&self.roles.idiom_scratch]) {
                        if register != self.roles.leaf_result && !handover.contains(&register) {
                            self.assembler
                                .move_immediate(register, crate::heap::Word::DEAD_REGISTER_BITS);
                        }
                    }
                    for register in handover.into_iter().rev() {
                        self.assembler.pop(register);
                    }
                    self.assembler.compare_immediate(self.roles.leaf_result, 0);
                    self.assembler.jump_if(Condition::Equal, rejoin);
                    self.exit(kind, 0);
                }
                #[cfg(debug_assertions)]
                ColdPath::DeadRegister {
                    label,
                    register,
                    program_counter,
                } => {
                    self.assembler.bind(label);
                    let word = self.block_word(BlockWord::ExitRegister);
                    self.assembler.store_immediate32(word, i32::from(register));
                    self.exit(ExitKind::DeadRegister, program_counter);
                }
            }
        }
        if let Some(label) = self.body_words_label {
            let bytes = u32::try_from(self.body_words)
                .ok()
                .and_then(|words| words.checked_mul(WORD_BYTES.unsigned_abs()))
                .ok_or(AssemblyError::CodeTooLarge)?;
            self.assembler.align_data(BODY_WORDS_ALIGNMENT);
            self.assembler.bind(label);
            self.assembler.zeroed_data(bytes);
            // The line after the words holds no instruction either: the
            // next body starts at the next line.
            self.assembler.align_data(BODY_WORDS_ALIGNMENT);
        }
        self.assembler.finish()
    }
}

/// How far an index into the machine words is shifted to be a byte
/// offset: a word is eight bytes.
const WORD_SHIFT: u8 = 3;

/// A word's bytes, as an immediate operand.
const WORD_BYTES: i32 = 8;

/// The sign of a 64-bit word spread over all of it by an arithmetic
/// shift: 0 for a word of zero or more, -1 for a negative one.
const SIGN_SHIFT: u8 = 63;

/// The `SmallInteger` tag, as an immediate operand.
fn small_integer_tag_word() -> i32 {
    i32::try_from(crate::heap::value_bits::SMALL_INTEGER_TAG)
        .expect("internal: the tag is a low bit")
}

/// How far a `SmallInteger`'s payload lies above its tag, as a shift
/// count.
fn small_integer_tag_bits() -> u8 {
    u8::try_from(crate::heap::value_bits::TAG_BITS).expect("internal: the tag takes a few bits")
}

/// The registers a `SmallInteger` fast path computes in
/// (`MacroAssembler::small_integer_operation`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct IntegerRegisters {
    /// The left operand; the result of a sum, a difference or a product;
    /// a division's dividend, where the encoder fixes it.
    pub(crate) left: Register,
    /// The right operand; a division's divisor.
    pub(crate) right: Register,
    /// A division's remainder, where the encoder fixes it.
    pub(crate) remainder: Register,
    /// Every operation's first spare register, and the result of a
    /// comparison and of a division.
    pub(crate) first_spare: Register,
    /// A comparison's and a division's second spare register, which may
    /// be `remainder`: the division reads its remainder before it writes
    /// it.
    pub(crate) second_spare: Register,
}

// `check_own_slot_guard` takes a header's guard bits of 0 for `NONE`.
const _: () = assert!(crate::mixin::DispatchId::NONE.bits() == 0);

// `load_element_address` takes a `SmallInteger`'s word, less its tag, for
// its value's offset in words.
const _: () = assert!(1 << crate::heap::value_bits::TAG_BITS == WORD_BYTES);

/// Whether no two of `registers` are the same, for an idiom's check of
/// its operands.
fn distinct(registers: &[Register]) -> bool {
    registers
        .iter()
        .enumerate()
        .all(|(position, register)| !registers[position + 1..].contains(register))
}

/// Where eden's limit word lies past its top word
/// (`ns_memory::Reservation::bump_cursor_address`).
const EDEN_LIMIT_DISPLACEMENT: i32 = 8;

/// The most fields `nil_fill` writes in a straight line; more take a
/// loop, so that a large vector's code stays small.
const NIL_STORES_IN_LINE: usize = 8;

/// Where an argument of the leaf shim is passed.
#[derive(Clone, Copy)]
enum Place {
    Register(Register),
    /// The stack word at this displacement from the aligned stack pointer.
    Stack(i32),
}

/// How a leaf call passes the shim's arguments under one ABI. The shim
/// takes five: the block, the leaf's index, then the leaf's three.
struct LeafCallLayout {
    block: Register,
    leaf: Register,
    /// Where the leaf's three arguments go.
    places: [Place; 3],
    /// Where each operand held in a register is copied before any argument
    /// register is written.
    copies: [i32; 3],
    /// What the call reserves below the aligned stack pointer, a multiple
    /// of 16 so that the call is made aligned.
    frame_bytes: i32,
}

impl LeafCallLayout {
    fn of(abi: Abi) -> LeafCallLayout {
        match abi {
            // The fifth argument sits above the 32 bytes of shadow space,
            // which the callee owns only once it is called, so the copies
            // may use them until then.
            Abi::Windows64 => LeafCallLayout {
                block: RCX,
                leaf: RDX,
                places: [Place::Register(R8), Place::Register(R9), Place::Stack(32)],
                copies: [0, 8, 16],
                frame_bytes: 48,
            },
            Abi::SystemV => LeafCallLayout {
                block: RDI,
                leaf: RSI,
                places: [
                    Place::Register(RDX),
                    Place::Register(RCX),
                    Place::Register(R8),
                ],
                copies: [0, 8, 16],
                frame_bytes: 32,
            },
            Abi::Aapcs64 => LeafCallLayout {
                block: X0,
                leaf: X1,
                places: [
                    Place::Register(X2),
                    Place::Register(X3),
                    Place::Register(X4),
                ],
                copies: [0, 8, 16],
                frame_bytes: 32,
            },
        }
    }
}

/// The registers a leaf call under `abi` passes the shim's arguments in
/// -- the block, the leaf's index and the leaf's arguments that go in a
/// register -- for the tests that move operands out of them.
#[cfg(test)]
pub(crate) fn leaf_call_argument_registers(abi: Abi) -> Vec<Register> {
    let layout = LeafCallLayout::of(abi);
    let mut registers = vec![layout.block, layout.leaf];
    registers.extend(layout.places.iter().filter_map(|place| match place {
        Place::Register(register) => Some(*register),
        Place::Stack(_) => None,
    }));
    registers
}

/// The encoding's delegation, which holds on every host.
#[cfg(test)]
mod encoding_tests {
    use ns_assembler::aarch64::{X22, X23};
    use ns_assembler::{Aarch64, Assembler, Encoder, Register, X64};

    use super::{Abi, Encoding, Roles};

    /// Every encoding encodes what its encoder does: the operations, the
    /// label references, the stops, the stack slot and the alignment.
    #[test]
    fn an_encoding_encodes_as_its_encoder() {
        fn assembled<E: Encoder>(encoder: E) -> Vec<u8> {
            let mut assembler = Assembler::with_encoder(encoder);
            let target = Register(2);
            let label = assembler.label();
            assembler.enter_called();
            assembler.bind(label);
            assembler.add_immediate(target, 1);
            assembler.jump(label);
            assembler.jump_into_called(target);
            assembler.return_();
            // A word of data after the code, which a label reference
            // reaches relative to its own position.
            let data = assembler.label();
            assembler.load_labelled(target, data, 8);
            assembler.load_label_address(target, data, 0);
            assembler.return_();
            assembler.align_data(8);
            assembler.bind(data);
            assembler.zeroed_data(16);
            match assembler.finish() {
                Ok(assembled) => assembled.bytes,
                Err(error) => panic!("the code lays out: {error}"),
            }
        }
        fn stops(encoder: &impl Encoder) -> Vec<u8> {
            let mut code = Vec::new();
            encoder.fill_with_stops(16, &mut code);
            code
        }
        assert_eq!(assembled(Encoding::X64(X64)), assembled(X64));
        assert_eq!(assembled(Encoding::Aarch64(Aarch64)), assembled(Aarch64));
        assert_eq!(stops(&Encoding::X64(X64)), stops(&X64));
        assert_eq!(stops(&Encoding::Aarch64(Aarch64)), stops(&Aarch64));
        assert_eq!(
            Encoding::X64(X64).stack_slot_bytes(),
            X64.stack_slot_bytes()
        );
        assert_eq!(
            Encoding::Aarch64(Aarch64).stack_slot_bytes(),
            Aarch64.stack_slot_bytes()
        );
        assert_eq!(
            Encoding::Aarch64(Aarch64).instruction_alignment(),
            Aarch64.instruction_alignment()
        );
        assert_eq!(
            Encoding::X64(X64).instruction_alignment(),
            X64.instruction_alignment()
        );
    }

    /// The register each role of `roles` plays, in one order, then the
    /// scratch registers and the placement's.
    fn played(roles: Roles) -> Vec<Register> {
        let mut registers = vec![
            roles.frame,
            roles.context,
            roles.object_space,
            roles.idiom_scratch,
            roles.leaf_result,
            roles.dividend,
            roles.remainder,
            roles.send_cells,
            roles.send_guard,
            roles.send_level,
            roles.send_key,
            roles.send_entry,
            roles.lookup_rule,
            roles.lookup_hash,
            roles.accessor_field,
            roles.depth_count,
            roles.frame_extent,
            roles.stack_end,
            roles.poison,
            roles.return_address,
            roles.stack_pointer,
            roles.exit_kind(),
            roles.exit_program_counter(),
            roles.exit_code_address(),
            roles.callee_receiver(),
            roles.callee_class(),
            roles.callee_closure(),
            roles.callee_counted(),
            roles.callee_argument(),
        ];
        registers.extend(roles.scratch);
        registers.extend(roles.placement);
        registers
    }

    /// The roles under AAPCS64 share a register exactly where x64's do,
    /// so that every argument about which role a value survives in carries
    /// over; each pinned role sits in a register the convention preserves,
    /// so that a leaf call keeps it; and the two free survivors, `x22` and
    /// `x23` as `r13` and `rbp` under x64, are preserved too and play no
    /// role.
    #[test]
    fn the_aapcs64_roles_alias_as_x64_s_do() {
        let mut x64 = played(Roles::of(Abi::SystemV));
        let mut aarch64 = played(Roles::of(Abi::Aapcs64));
        x64.extend(Roles::free_survivors(Abi::SystemV));
        aarch64.extend(Roles::free_survivors(Abi::Aapcs64));
        assert_eq!(x64.len(), aarch64.len());
        let pairs: Vec<(Register, Register)> = x64.into_iter().zip(aarch64).collect();
        for (first, (x64_first, aarch64_first)) in pairs.iter().enumerate() {
            for (second, (x64_second, aarch64_second)) in pairs.iter().enumerate() {
                assert_eq!(
                    x64_first == x64_second,
                    aarch64_first == aarch64_second,
                    "roles {first} and {second}"
                );
            }
        }
        let roles = Roles::of(Abi::Aapcs64);
        for pinned in [roles.frame, roles.context, roles.object_space] {
            assert!(Abi::Aapcs64.callee_saved().contains(&pinned));
        }
        assert_eq!(Roles::free_survivors(Abi::Aapcs64), [X22, X23]);
        for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
            let roles = played(Roles::of(abi));
            for survivor in Roles::free_survivors(abi) {
                assert!(abi.callee_saved().contains(&survivor), "{abi:?}");
                assert!(!roles.contains(&survivor), "{abi:?}");
            }
        }
    }

    /// The stack slot an ABI states as a constant, which the room an entry
    /// checks for is computed from, is its encoder's.
    #[test]
    fn an_abi_s_stack_slot_is_its_encoder_s() {
        for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
            assert_eq!(abi.stack_slot_bytes(), abi.encoding().stack_slot_bytes());
        }
    }

    /// No register a value is allocated to is one an idiom between two
    /// frame nodes needs as it is -- a pinned role, the idioms' scratch,
    /// a leaf's answer, a division's, the exit's or the encoder's own --
    /// and the survivors are exactly the first registers a leaf call
    /// leaves alone: preserved by the convention, and no argument of the
    /// call's.
    #[test]
    fn the_allocatable_registers_take_no_fixed_role_and_the_survivors_come_first() {
        use ns_assembler::aarch64::{X16, X17, X18, X29, X30};

        for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
            let roles = Roles::of(abi);
            let fixed = [
                roles.frame,
                roles.context,
                roles.object_space,
                roles.idiom_scratch,
                roles.leaf_result,
                roles.dividend,
                roles.remainder,
                roles.stack_pointer,
                roles.exit_kind(),
                roles.exit_program_counter(),
                roles.exit_code_address(),
            ];
            let arguments = super::leaf_call_argument_registers(abi);
            for (index, register) in roles.allocatable.iter().enumerate() {
                assert!(!fixed.contains(register), "{abi:?} {register:?}");
                assert!(
                    !roles.allocatable[..index].contains(register),
                    "{abi:?} {register:?} twice"
                );
                if abi == Abi::Aapcs64 {
                    assert!(
                        ![X16, X17, X18, X29, X30].contains(register),
                        "{abi:?} {register:?}"
                    );
                }
                let survives =
                    abi.callee_saved().contains(register) && !arguments.contains(register);
                assert_eq!(
                    survives,
                    index < roles.survive_leaf_calls,
                    "{abi:?} {register:?}"
                );
            }
            assert_eq!(roles.allocatable[..2], Roles::free_survivors(abi));
        }
        assert_eq!(Roles::of(Abi::Windows64).survive_leaf_calls, 5);
        assert_eq!(Roles::of(Abi::SystemV).survive_leaf_calls, 3);
        assert_eq!(Roles::of(Abi::Aapcs64).survive_leaf_calls, 7);
        assert_eq!(Roles::of(Abi::Windows64).allocatable.len(), 8);
        assert_eq!(Roles::of(Abi::Aapcs64).allocatable.len(), 13);
    }

    /// Optimized code's fast path computes in registers no value is
    /// allocated to and no pinned role holds, the division's where the
    /// encoder fixes them.
    #[test]
    fn the_integer_temporaries_hold_no_value_and_no_pinned_role() {
        for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
            let roles = Roles::of(abi);
            let temporaries = roles.integer_temporaries();
            let registers = [
                temporaries.left,
                temporaries.right,
                temporaries.remainder,
                temporaries.first_spare,
                temporaries.second_spare,
            ];
            for register in registers {
                assert!(
                    !roles.allocatable.contains(&register),
                    "{abi:?} {register:?}"
                );
                assert!(
                    ![
                        roles.frame,
                        roles.context,
                        roles.object_space,
                        roles.stack_pointer
                    ]
                    .contains(&register),
                    "{abi:?} {register:?}"
                );
            }
            assert_eq!(
                (temporaries.left, temporaries.remainder),
                (roles.dividend, roles.remainder)
            );
            assert!(super::distinct(&registers[..4]), "{abi:?}");
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "cannot be a temporary")]
    fn a_debug_build_refuses_a_held_register_as_a_temporary() {
        let mut macro_assembler = super::MacroAssembler::new(Abi::Windows64);
        let held = Roles::of(Abi::Windows64).allocatable[3];
        macro_assembler.hold_values(&[held]);
        macro_assembler.temporary(held);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "clobbers")]
    fn a_debug_build_refuses_a_leaf_call_over_a_value_past_the_survivors() {
        let roles = Roles::of(Abi::SystemV);
        let mut macro_assembler = super::MacroAssembler::new(Abi::SystemV);
        macro_assembler.hold_values(&roles.allocatable[..=roles.survive_leaf_calls]);
        macro_assembler.call_leaf(super::LeafIndex(0), [super::Operand::Immediate(0); 3]);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "clobbers")]
    fn a_debug_build_refuses_an_out_of_line_leaf_call_over_a_value_held_where_it_branches_off() {
        let roles = Roles::of(Abi::Windows64);
        let mut macro_assembler = super::MacroAssembler::new(Abi::Windows64);
        let label = macro_assembler.assembler().label();
        let rejoin = macro_assembler.assembler().label();
        // r8, not a survivor, holds a value where the slow path branches
        // off; nothing is held by the time `finish` emits it.
        macro_assembler.hold_values(&[roles.allocatable[roles.survive_leaf_calls]]);
        macro_assembler.call_leaf_out_of_line(
            label,
            super::LeafIndex(0),
            [super::Operand::Immediate(0); 3],
            roles.leaf_result,
            rejoin,
        );
        macro_assembler.hold_values(&[]);
        macro_assembler.assembler().bind(rejoin);
        let _ = macro_assembler.finish();
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "cannot be a temporary")]
    fn a_debug_build_refuses_an_out_of_line_leaf_call_s_result_in_a_held_register() {
        let roles = Roles::of(Abi::Windows64);
        let mut macro_assembler = super::MacroAssembler::new(Abi::Windows64);
        let label = macro_assembler.assembler().label();
        let rejoin = macro_assembler.assembler().label();
        let held = roles.allocatable[0];
        macro_assembler.hold_values(&[held]);
        macro_assembler.call_leaf_out_of_line(
            label,
            super::LeafIndex(0),
            [super::Operand::Immediate(0); 3],
            held,
            rejoin,
        );
        macro_assembler.hold_values(&[]);
        macro_assembler.assembler().bind(rejoin);
        let _ = macro_assembler.finish();
    }

    #[test]
    fn an_out_of_line_leaf_call_keeps_the_values_the_survivors_hold() {
        for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
            let roles = Roles::of(abi);
            let mut macro_assembler = super::MacroAssembler::new(abi);
            let label = macro_assembler.assembler().label();
            let rejoin = macro_assembler.assembler().label();
            macro_assembler.hold_values(&roles.allocatable[..roles.survive_leaf_calls]);
            macro_assembler.call_leaf_out_of_line(
                label,
                super::LeafIndex(0),
                [super::Operand::Immediate(0); 3],
                roles.leaf_result,
                rejoin,
            );
            macro_assembler.assembler().bind(rejoin);
            assert!(macro_assembler.finish().is_ok(), "{abi:?}");
        }
    }

    #[test]
    fn a_leaf_call_keeps_the_values_the_survivors_hold() {
        for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
            let roles = Roles::of(abi);
            let mut macro_assembler = super::MacroAssembler::new(abi);
            macro_assembler.hold_values(&roles.allocatable[..roles.survive_leaf_calls]);
            macro_assembler.call_leaf(super::LeafIndex(0), [super::Operand::Immediate(0); 3]);
            let unheld = roles.allocatable[roles.survive_leaf_calls];
            assert_eq!(macro_assembler.temporary(unheld), unheld);
        }
    }
}

/// The idioms run as machine code, on every architecture with an encoder:
/// each register a test names is one of the host's scratch registers.
#[cfg(all(test, ns_jit))]
mod tests {
    use ns_assembler::{Memory, Register};

    use super::{Abi, ObjectSize, Roles};
    use crate::jit::stubs::testing::Machine;
    use crate::jit::stubs::{ExitKind, MachineExit};

    /// The host's first `COUNT` scratch registers, in the roles' order:
    /// x64's `rax`, `rcx`, `rdx` and `rsi`, AAPCS64's `x0` to `x3`.
    fn scratch<const COUNT: usize>() -> [Register; COUNT] {
        let roles = Roles::of(Abi::host());
        std::array::from_fn(|index| roles.scratch[index])
    }

    #[test]
    fn a_body_moves_one_frame_register_into_another() {
        let [first] = scratch();
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.set_program_counter(0);
            macro_assembler.load_frame_register(first, 2);
            macro_assembler.store_frame_register(5, first);
            macro_assembler.exit(ExitKind::Reexecute, 1);
        });
        let mut frame = [10, 11, 12, 13, 14, 15, 16, 17];
        let (exit, _) = machine.run(body, &mut frame, 0, false, &mut (), &[]);
        assert_eq!(exit, MachineExit::Reexecute { program_counter: 1 });
        assert_eq!(frame, [10, 11, 12, 13, 14, 12, 16, 17]);
    }

    #[test]
    fn the_register_base_places_the_frame_in_the_stack() {
        let [first] = scratch();
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.load_frame_register(first, 0);
            macro_assembler.store_frame_register(1, first);
            macro_assembler.exit(ExitKind::Reexecute, 1);
        });
        let mut frame = [10, 11, 12, 13, 14, 15, 16, 17];
        machine.run(body, &mut frame, 3, false, &mut (), &[]);
        assert_eq!(frame, [10, 11, 12, 13, 13, 15, 16, 17]);
    }

    #[test]
    fn an_immediate_word_of_every_width_reaches_a_register() {
        let [first] = scratch();
        let words = [0, 7, 0xFFFF_FFFF, u64::MAX, 0x1234_5678_9ABC_DEF0];
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            for (register, word) in (0u16..).zip(words) {
                macro_assembler.load_immediate_word(first, word);
                macro_assembler.store_frame_register(register, first);
            }
            macro_assembler.exit(ExitKind::Reexecute, 0);
        });
        let mut frame = [1u64; 8];
        machine.run(body, &mut frame, 0, false, &mut (), &[]);
        assert_eq!(frame[..5], words);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_register_load_traps_on_poison_in_debug() {
        let [first] = scratch();
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.set_program_counter(9);
            macro_assembler.load_frame_register(first, 3);
            macro_assembler.store_frame_register(5, first);
            macro_assembler.exit(ExitKind::Reexecute, 10);
        });
        let mut frame = [0u64; 8];
        frame[3] = crate::heap::Word::DEAD_REGISTER_BITS;
        let (exit, _) = machine.run(body, &mut frame, 0, false, &mut (), &[]);
        assert_eq!(
            exit,
            MachineExit::DeadRegister {
                program_counter: 9,
                register: 3
            }
        );
        assert_eq!(frame[5], 0, "the read stopped the body");
    }

    /// A fake class object for the walks below: words the machine code
    /// reads at the layout's displacements from its handle, which is its
    /// address, since the tests' object space starts at 0.
    struct FakeClass {
        words: Vec<u64>,
    }

    impl FakeClass {
        fn new() -> FakeClass {
            FakeClass {
                words: vec![crate::heap::Immediate::NIL.bits(); 64],
            }
        }

        fn handle(&self) -> u64 {
            self.words.as_ptr().expose_provenance() as u64
        }

        fn set(&mut self, displacement: i32, word: u64) {
            let index = usize::try_from(displacement / 8).expect("a field lies after the start");
            self.words[index] = word;
        }
    }

    const NIL: u64 = crate::heap::Immediate::NIL.bits();

    /// A frame of a body taking no arguments, its receiver `receiver` and
    /// its current class `current_class`.
    fn frame_with(receiver: u64, current_class: u64) -> [u64; 16] {
        let mut frame = [0u64; 16];
        frame[usize::from(crate::heap::Heap::receiver_register(0))] = receiver;
        frame[usize::from(crate::heap::Heap::current_class_register(0))] = current_class;
        frame
    }

    /// Runs `load_enclosing_object` at `depth` on `frame`, answering what
    /// it left in register 15.
    fn enclosing_object(frame: &mut [u64; 16], depth: u16) -> u64 {
        let [first] = scratch();
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.load_enclosing_object(first, 0, depth);
            macro_assembler.store_frame_register(15, first);
            macro_assembler.exit(ExitKind::Reexecute, 0);
        });
        machine.run(body, frame, 0, false, &mut (), &[]);
        frame[15]
    }

    /// `dispatch::kth_enclosing_object`'s ends: `self` at depth 0; `nil`
    /// at every positive depth for a frame with no current class, a
    /// top-level activation's; the enclosing object one level out; and
    /// `nil` where the declaring chain ends before the depth.
    #[test]
    fn the_enclosing_object_walk_answers_nil_where_the_chain_ends() {
        let layout = crate::heap::Heap::machine_layout();
        let enclosing = FakeClass::new();
        let mut class = FakeClass::new();
        class.set(layout.enclosing_displacement, enclosing.handle());
        let receiver = 0x1235;
        assert_eq!(
            enclosing_object(&mut frame_with(receiver, NIL), 0),
            receiver
        );
        assert_eq!(enclosing_object(&mut frame_with(receiver, NIL), 1), NIL);
        assert_eq!(enclosing_object(&mut frame_with(receiver, NIL), 3), NIL);
        let mut frame = frame_with(receiver, class.handle());
        assert_eq!(enclosing_object(&mut frame, 1), enclosing.handle());
        let mut frame = frame_with(receiver, class.handle());
        assert_eq!(enclosing_object(&mut frame, 2), NIL, "no declaring class");
    }

    /// `load_lexical_class` jumps to its `missing` label where
    /// `dispatch::kth_lexical_class` answers `None` -- no current class,
    /// a declaring chain shorter than the depth -- and answers N otherwise.
    #[test]
    fn the_lexical_class_walk_takes_its_missing_label_where_the_chain_ends() {
        let [first] = scratch();
        let layout = crate::heap::Heap::machine_layout();
        let declaring = FakeClass::new();
        let mut class = FakeClass::new();
        class.set(layout.declaring_class_displacement, declaring.handle());
        let lexical_class = |frame: &mut [u64; 16], depth: u16| {
            let mut machine = Machine::new();
            let body = machine.body(|macro_assembler| {
                let missing = macro_assembler.assembler().label();
                macro_assembler.load_lexical_class(first, 0, depth, missing);
                macro_assembler.store_frame_register(15, first);
                macro_assembler.exit(ExitKind::Reexecute, 1);
                macro_assembler.assembler().bind(missing);
                macro_assembler.exit(ExitKind::Reexecute, 2);
            });
            let (exit, _) = machine.run(body, frame, 0, false, &mut (), &[]);
            (exit, frame[15])
        };
        let missing = MachineExit::Reexecute { program_counter: 2 };
        let found = MachineExit::Reexecute { program_counter: 1 };
        assert_eq!(lexical_class(&mut frame_with(0, NIL), 0).0, missing);
        assert_eq!(lexical_class(&mut frame_with(0, NIL), 1).0, missing);
        assert_eq!(
            lexical_class(&mut frame_with(0, class.handle()), 0),
            (found, class.handle())
        );
        assert_eq!(
            lexical_class(&mut frame_with(0, class.handle()), 1),
            (found, declaring.handle())
        );
        assert_eq!(
            lexical_class(&mut frame_with(0, class.handle()), 2).0,
            missing
        );
    }

    /// `load_superclass` exits at the send where `decide_super_send`
    /// raises its errors -- no current class, no superclass -- before it
    /// writes anything, and answers S otherwise.
    #[test]
    fn the_superclass_load_exits_where_there_is_no_current_class_or_superclass() {
        let [first] = scratch();
        let layout = crate::heap::Heap::machine_layout();
        let superclass = FakeClass::new();
        let without_superclass = FakeClass::new();
        let mut class = FakeClass::new();
        class.set(layout.superclass_displacement, superclass.handle());
        let load = |frame: &mut [u64; 16]| {
            let mut machine = Machine::new();
            let body = machine.body(|macro_assembler| {
                macro_assembler.load_superclass(first, 0, 7);
                macro_assembler.store_frame_register(15, first);
                macro_assembler.exit(ExitKind::Reexecute, 99);
            });
            let (exit, _) = machine.run(body, frame, 0, false, &mut (), &[]);
            (exit, frame[15])
        };
        let at_the_send = MachineExit::Reexecute { program_counter: 7 };
        assert_eq!(load(&mut frame_with(0, NIL)), (at_the_send, 0));
        assert_eq!(
            load(&mut frame_with(0, without_superclass.handle())),
            (at_the_send, 0)
        );
        assert_eq!(
            load(&mut frame_with(0, class.handle())),
            (
                MachineExit::Reexecute {
                    program_counter: 99
                },
                superclass.handle()
            )
        );
    }

    /// A fake eden for the allocation idioms: `words` of space, which the
    /// tests' object space, starting at 0, addresses by their own
    /// addresses, and a cursor over them, its top at the first word and
    /// its limit `limit_words` past it.
    struct FakeEden {
        words: Vec<u64>,
        cursor: Box<[u64; 2]>,
    }

    /// What eden held before an allocation, so that a test sees which
    /// words it wrote.
    const UNWRITTEN: u64 = 0xDEAD_BEEF;

    impl FakeEden {
        fn new(size: usize, limit_words: usize) -> FakeEden {
            let words = vec![UNWRITTEN; size];
            let start = words.as_ptr().expose_provenance() as u64;
            let cursor = Box::new([start, start + 8 * limit_words as u64]);
            FakeEden { words, cursor }
        }

        fn start(&self) -> u64 {
            self.words.as_ptr().expose_provenance() as u64
        }

        fn machine(&self) -> Machine {
            let mut machine = Machine::new();
            machine.set_block_word(
                crate::jit::stubs::BlockWord::EdenCursor,
                self.cursor.as_ptr().expose_provenance() as u64,
            );
            machine
        }

        fn top(&self) -> u64 {
            self.cursor[0]
        }
    }

    const HEADER: u64 = 0x0300_0000_0040_0005;
    const CLASS: u64 = 0x1230;

    /// Allocates an object of `size` in `eden` with `HEADER` and `CLASS`
    /// and fills `fields` of its fields with nil, the word in register 0
    /// and an exit at 1, or an exit at 2 when it takes the slow path; the
    /// register form's bytes in register 1 of the frame.
    fn allocate(eden: &FakeEden, size: ObjectSize, fields: usize) -> (MachineExit, u64) {
        let [first, second, third] = scratch();
        let mut machine = eden.machine();
        let body = machine.body(|macro_assembler| {
            let slow = macro_assembler.assembler().label();
            if let ObjectSize::Register(bytes) = size {
                macro_assembler.load_frame_register(bytes, 1);
            }
            macro_assembler.load_immediate_word(second, HEADER);
            macro_assembler.load_immediate_word(third, CLASS);
            macro_assembler.bump_allocate(first, size, second, third, slow);
            macro_assembler.nil_fill(first, 0, fields);
            macro_assembler.store_frame_register(0, first);
            macro_assembler.exit(ExitKind::Reexecute, 1);
            macro_assembler.assembler().bind(slow);
            macro_assembler.exit(ExitKind::Reexecute, 2);
        });
        let mut frame = [0u64; 4];
        if let ObjectSize::Register(_) = size {
            frame[1] = 8 * (fields as u64 + 2);
        }
        let (exit, _) = machine.run(body, &mut frame, 0, false, &mut (), &[]);
        (exit, frame[0])
    }

    const BUMPED: MachineExit = MachineExit::Reexecute { program_counter: 1 };
    const SLOW: MachineExit = MachineExit::Reexecute { program_counter: 2 };

    /// The inline bump places the block at the cursor's top, moves the
    /// top past it, and writes the header, the class word and, by
    /// `nil_fill`, the fields -- a straight line of stores for a few, a
    /// loop for more -- and nothing past them.
    #[test]
    fn an_allocation_bumps_the_cursor_and_writes_its_words() {
        for fields in [0, 2, 20] {
            let eden = FakeEden::new(64, 64);
            let (exit, object) = allocate(&eden, ObjectSize::Constant(fields + 1), fields);
            assert_eq!((exit, object), (BUMPED, eden.start()), "{fields} fields");
            assert_eq!(eden.top(), eden.start() + 8 * (fields as u64 + 2));
            assert_eq!(eden.words[..2], [HEADER, CLASS]);
            assert!(eden.words[2..2 + fields].iter().all(|word| *word == NIL));
            assert_eq!(eden.words[2 + fields], UNWRITTEN, "{fields} fields");
        }
    }

    /// The bytes in a register bump as a constant's do.
    #[test]
    fn an_allocation_of_a_size_in_a_register_bumps_as_a_constant_one() {
        let [_, _, _, fourth] = scratch();
        let eden = FakeEden::new(16, 16);
        let (exit, object) = allocate(&eden, ObjectSize::Register(fourth), 3);
        assert_eq!((exit, object), (BUMPED, eden.start()));
        assert_eq!(eden.top(), eden.start() + 8 * 5);
        assert_eq!(eden.words[..5], [HEADER, CLASS, NIL, NIL, NIL]);
        assert_eq!(eden.words[5], UNWRITTEN);
    }

    /// A count the header cannot hold puts the overflow word before the
    /// header, and the object's word is the header's.
    #[test]
    fn a_large_count_takes_an_overflow_word_before_the_header() {
        let fields = 300;
        let eden = FakeEden::new(400, 400);
        let (exit, object) = allocate(&eden, ObjectSize::Constant(fields + 1), fields);
        assert_eq!((exit, object), (BUMPED, eden.start() + 8));
        assert_eq!(eden.top(), eden.start() + 8 * (fields as u64 + 3));
        assert_eq!(eden.words[0], (0xFF << 56) | (fields as u64 + 1));
        assert_eq!(eden.words[1..3], [HEADER, CLASS]);
        assert!(eden.words[3..3 + fields].iter().all(|word| *word == NIL));
        assert_eq!(eden.words[3 + fields], UNWRITTEN);
    }

    /// A block that would end past the limit, and one at or past the
    /// large-object threshold, which the inline bump never places, take
    /// the slow path with nothing written; one that ends at the limit is
    /// placed.
    #[test]
    fn a_block_past_the_limit_or_large_takes_the_slow_path() {
        let eden = FakeEden::new(16, 3);
        assert_eq!(allocate(&eden, ObjectSize::Constant(3), 2).0, SLOW);
        assert_eq!(eden.top(), eden.start());
        assert!(eden.words.iter().all(|word| *word == UNWRITTEN));
        assert_eq!(allocate(&eden, ObjectSize::Constant(2), 1).0, BUMPED);
        let eden = FakeEden::new(16, 16);
        assert_eq!(allocate(&eden, ObjectSize::Constant(4096), 0).0, SLOW);
        assert_eq!(eden.top(), eden.start());
    }

    /// A limit below the top refuses every bump, as `Reservation::bump`'s
    /// does.
    #[test]
    fn a_limit_below_the_top_refuses_the_inline_bump() {
        let mut eden = FakeEden::new(16, 16);
        eden.cursor[1] = eden.start() - 8;
        assert_eq!(allocate(&eden, ObjectSize::Constant(1), 0).0, SLOW);
        assert_eq!(eden.top(), eden.start());
    }

    /// A leaf the tests' tables hold where no leaf is expected.
    fn no_leaf(_: &mut Vec<u64>, _: [u64; 3]) -> u64 {
        panic!("no leaf is called")
    }

    /// The write barrier's leaf, standing in for `Heap::remember`: records
    /// the holder it is handed, and answers it, as the leaf does.
    fn record_holder(remembered: &mut Vec<u64>, arguments: [u64; 3]) -> u64 {
        remembered.push(arguments[0]);
        arguments[0]
    }

    /// The leaves the barrier's tests run with: only `Leaf::Remember`'s
    /// records anything.
    fn barrier_leaves() -> Vec<ns_memory::Leaf<Vec<u64>>> {
        let remember = crate::jit::leaves::Leaf::Remember as usize;
        (0..=remember)
            .map(|index| {
                if index == remember {
                    record_holder as ns_memory::Leaf<Vec<u64>>
                } else {
                    no_leaf as ns_memory::Leaf<Vec<u64>>
                }
            })
            .collect()
    }

    /// Stores `value` into field 0 of a holder of the header `header` that
    /// lies `holder` bytes into the object space, through the barrier --
    /// `store_with_barrier`, or, when `computed` says so,
    /// `store_with_barrier_at` an address computed beforehand: answers
    /// the field and the holders the leaf was asked to remember, having
    /// checked that the holder's register still holds it after the store,
    /// on either path.
    fn store_through_barrier(
        holder: u64,
        header: u64,
        value: u64,
        computed: bool,
    ) -> (u64, Vec<u64>) {
        let layout = crate::heap::Heap::machine_layout();
        let [first, second, third, fourth] = scratch();
        let word_index =
            |displacement: i32| usize::try_from(displacement / 8).expect("after the start");
        let mut words = [0u64; 4];
        words[word_index(layout.header_displacement)] = header;
        let address = words.as_mut_ptr().expose_provenance() as u64;
        let mut machine = Machine::new();
        machine.set_block_word(
            crate::jit::stubs::BlockWord::ObjectSpaceBase,
            address.wrapping_sub(holder),
        );
        let body = machine.body(|macro_assembler| {
            macro_assembler.load_frame_register(first, 0);
            macro_assembler.load_frame_register(second, 1);
            if computed {
                let object_space = macro_assembler.roles().object_space;
                let assembler = macro_assembler.assembler();
                assembler.move_register(fourth, first);
                assembler.add(fourth, object_space);
                macro_assembler.store_with_barrier_at(
                    first,
                    Memory {
                        base: fourth,
                        displacement: layout.fields_displacement,
                    },
                    second,
                    third,
                );
            } else {
                macro_assembler.store_with_barrier(
                    first,
                    layout.fields_displacement,
                    second,
                    third,
                );
            }
            macro_assembler.store_frame_register(2, first);
            macro_assembler.exit(ExitKind::Reexecute, 1);
        });
        let mut frame = [holder, value, 0, 0];
        let mut remembered = Vec::new();
        let (exit, _) = machine.run(
            body,
            &mut frame,
            0,
            false,
            &mut remembered,
            &barrier_leaves(),
        );
        assert_eq!(exit, MachineExit::Reexecute { program_counter: 1 });
        assert_eq!(frame[2], holder, "the barrier keeps its holder");
        (words[word_index(layout.fields_displacement)], remembered)
    }

    /// The barrier stores the word in every case, and calls the leaf only
    /// past its four early outs, as `Heap::write_barrier` remembers only an
    /// old holder that came to name a young object and `Heap::remember`
    /// only one whose bit is clear.
    #[test]
    fn the_barrier_remembers_only_an_old_holder_of_a_young_object_not_yet_remembered() {
        let layout = crate::heap::Heap::machine_layout();
        let nursery_end = u64::try_from(layout.nursery_end).expect("a positive offset");
        let remembered_bit = u64::from(layout.remembered_bit.cast_unsigned());
        let (old, young) = (nursery_end + 0x100, 0x100);
        let (young_value, old_value) = (0x2000, nursery_end + 0x800);
        let integer = crate::heap::Immediate::small_integer(5)
            .expect("a small integer")
            .bits();
        // A header whose bits above and below the remembered bit are set
        // -- a word count, an identity hash, the mark, the age, a guard --
        // but not that bit: only the bit decides.
        let busy_header = !remembered_bit;
        for computed in [false, true] {
            assert_eq!(
                store_through_barrier(old, 0, young_value, computed),
                (young_value, vec![old]),
                "an old holder of a young object"
            );
            assert_eq!(
                store_through_barrier(old, busy_header, young_value, computed),
                (young_value, vec![old]),
                "an old holder whose header has every other bit set"
            );
            for (holder, header, value, case) in [
                (old, 0, integer, "an immediate"),
                (old, 0, NIL, "nil"),
                (old, 0, 0, "the word 0"),
                (young, 0, young_value, "a young holder"),
                (old, 0, old_value, "an old value"),
                (old, remembered_bit, young_value, "a remembered holder"),
            ] {
                assert_eq!(
                    store_through_barrier(holder, header, value, computed),
                    (value, Vec::new()),
                    "{case}"
                );
            }
        }
    }

    /// The offset from `words`' start of the address `load_element_address`
    /// or, when `bytes` says so, `load_byte_address` computes for the
    /// `SmallInteger` word `index` of the object whose word is `object`,
    /// the object space starting at `words`; `None` where it fails.
    fn indexed_offset(words: &[u64], object: u64, index: u64, bytes: bool) -> Option<u64> {
        let [first, second, third, fourth] = scratch();
        let address = words.as_ptr().expose_provenance() as u64;
        let mut machine = Machine::new();
        machine.set_block_word(crate::jit::stubs::BlockWord::ObjectSpaceBase, address);
        let body = machine.body(|macro_assembler| {
            let failed = macro_assembler.assembler().label();
            macro_assembler.load_frame_register(first, 0);
            macro_assembler.load_frame_register(second, 1);
            if bytes {
                macro_assembler.load_byte_address(first, second, third, fourth, failed);
            } else {
                macro_assembler.load_element_address(first, second, third, failed);
            }
            macro_assembler.store_frame_register(2, third);
            macro_assembler.store_frame_register(3, first);
            macro_assembler.exit(ExitKind::Reexecute, 1);
            macro_assembler.assembler().bind(failed);
            macro_assembler.store_frame_register(3, first);
            macro_assembler.exit(ExitKind::Reexecute, 2);
        });
        let mut frame = [object, index, 0, 0];
        let (exit, _) = machine.run(body, &mut frame, 0, false, &mut (), &[]);
        assert_eq!(frame[3], object, "the check keeps its object");
        (exit == MachineExit::Reexecute { program_counter: 1 }).then(|| frame[2] - address)
    }

    /// The word of the `SmallInteger` `value`.
    fn integer(value: i64) -> u64 {
        crate::heap::Immediate::small_integer(value)
            .expect("a small integer")
            .bits()
    }

    /// An `Array`'s element's address is its field's, for every index
    /// from 1 to its count -- its header's, or its overflow word's -- and
    /// nothing else passes: an index out of range, below 1 or not a
    /// `SmallInteger`, a receiver of another format, an immediate, `nil`.
    #[test]
    fn an_element_address_is_found_exactly_for_an_index_in_range() {
        let layout = crate::heap::Heap::machine_layout();
        let array = layout.indexable_pointers_format;
        let fields = u64::try_from(layout.fields_displacement).expect("a positive displacement");
        let mut words = vec![0u64; 4];
        // The object at offset 8, its header there, after its overflow word.
        words[1] = header_of(array, 4);
        let element = |index: i64| Some(8 + fields + (index.unsigned_abs() - 1) * 8);
        for index in 1..=3 {
            assert_eq!(
                indexed_offset(&words, 8, integer(index), false),
                element(index)
            );
        }
        for index in [0, 4, -1, i64::from(i32::MAX), -(1 << 40)] {
            assert_eq!(
                indexed_offset(&words, 8, integer(index), false),
                None,
                "{index}"
            );
        }
        assert_eq!(
            indexed_offset(&words, 8, NIL, false),
            None,
            "nil as the index"
        );
        assert_eq!(
            indexed_offset(&words, 8, 16, false),
            None,
            "an object as the index"
        );
        assert_eq!(
            indexed_offset(&words, integer(8), integer(1), false),
            None,
            "an immediate"
        );
        assert_eq!(indexed_offset(&words, NIL, integer(1), false), None, "nil");
        words[1] = header_of(0, 4);
        assert_eq!(
            indexed_offset(&words, 8, integer(1), false),
            None,
            "an instance"
        );
        words[1] = header_of(array, 255);
        words[0] = (0xFF << 56) | 301;
        assert_eq!(
            indexed_offset(&words, 8, integer(300), false),
            element(300),
            "the overflow's last"
        );
        assert_eq!(
            indexed_offset(&words, 8, integer(301), false),
            None,
            "past the overflow's count"
        );
        words[1] = header_of(array, 255) | 0xFFF_FFFF_FFC0_0000 & !(0xFF << 56) & !0x07C0_0000;
        assert_eq!(
            indexed_offset(&words, 8, integer(300), false),
            element(300),
            "a header with its hash, age and other bits set"
        );
    }

    /// A ByteArray's byte lies at its data's aligned start plus the index
    /// less 1, wherever the object lies, for every index from 1 to its
    /// size -- its data words' bytes less the unused ones, its count its
    /// header's or its overflow word's -- and nothing else passes.
    #[test]
    fn a_byte_address_is_found_exactly_for_an_index_in_range() {
        let layout = crate::heap::Heap::machine_layout();
        let format = layout.aligned_bytes_format;
        let unused = |count: u64| {
            i32::try_from(count << layout.unused_bytes_shift).expect("the unused bytes fit 32 bits")
        };
        // A ByteArray of `size` bytes: its class word, the alignment
        // reserve and its data words.
        let count = |size: u64| 4 + size.div_ceil(8);
        for object in [8u64, 16, 24, 32] {
            let mut words = vec![0u64; 512];
            let header = (object / 8) as usize;
            let data = (object + 16).next_multiple_of(32);
            for size in [1u64, 8, 13, 16] {
                let unused_bytes = (8 - size % 8) % 8;
                words[header] = header_of(format | unused(unused_bytes), count(size));
                for index in 1..=size {
                    assert_eq!(
                        indexed_offset(&words, object, integer(index as i64), true),
                        Some(data + index - 1),
                        "object {object}, size {size}, index {index}"
                    );
                }
                for index in [0, size as i64 + 1, -1] {
                    assert_eq!(
                        indexed_offset(&words, object, integer(index), true),
                        None,
                        "object {object}, size {size}, index {index}"
                    );
                }
            }
            // A size past what the header counts, from the overflow word.
            let size = 2001;
            words[header - 1] = (0xFF << 56) | count(size);
            words[header] = header_of(format | unused(7), 255);
            assert_eq!(
                indexed_offset(&words, object, integer(2001), true),
                Some(data + 2000),
                "the overflow's last byte"
            );
            assert_eq!(indexed_offset(&words, object, integer(2002), true), None);
            assert_eq!(
                indexed_offset(&words, object, NIL, true),
                None,
                "nil as the index"
            );
        }
        let mut words = vec![0u64; 8];
        for other in [
            0,
            layout.indexable_pointers_format,
            16 << layout.unused_bytes_shift,
        ] {
            words[1] = header_of(other, 6);
            assert_eq!(
                indexed_offset(&words, 8, integer(1), true),
                None,
                "format bits {other}"
            );
        }
        assert_eq!(
            indexed_offset(&words, integer(8), integer(1), true),
            None,
            "an immediate"
        );
    }

    /// Whether `check_array_index` lets element `index` of `object`
    /// through: the object whose header is `words[1]` when `object` is
    /// `None`, the word `object` otherwise.
    fn index_checked(words: &[u64], object: Option<u64>, index: u16) -> bool {
        let [first, second] = scratch();
        let address = words.as_ptr().expose_provenance() as u64;
        let mut machine = Machine::new();
        machine.set_block_word(crate::jit::stubs::BlockWord::ObjectSpaceBase, address);
        let body = machine.body(|macro_assembler| {
            let failed = macro_assembler.assembler().label();
            macro_assembler.load_frame_register(first, 0);
            macro_assembler.check_array_index(first, index, second, failed);
            macro_assembler.store_frame_register(1, first);
            macro_assembler.exit(ExitKind::Reexecute, 1);
            macro_assembler.assembler().bind(failed);
            macro_assembler.store_frame_register(1, first);
            macro_assembler.exit(ExitKind::Reexecute, 2);
        });
        let word = object.unwrap_or(8);
        let mut frame = [word, 0];
        let (exit, _) = machine.run(body, &mut frame, 0, false, &mut (), &[]);
        assert_eq!(frame[1], word, "the check keeps its object");
        exit == MachineExit::Reexecute { program_counter: 1 }
    }

    /// The header of an object of the format bits `format` with `count`
    /// words after it.
    fn header_of(format: i32, count: u64) -> u64 {
        let layout = crate::heap::Heap::machine_layout();
        u64::try_from(format).expect("format bits") | (count << layout.word_count_shift)
    }

    /// The check lets through exactly the elements an `Array` has --
    /// counted by its header or, past 253, by its overflow word -- and
    /// refuses any other format, an immediate and `nil`.
    #[test]
    fn the_array_check_lets_through_exactly_an_arrays_elements() {
        let layout = crate::heap::Heap::machine_layout();
        let array = layout.indexable_pointers_format;
        let mut words = vec![0u64; 4];
        words[1] = header_of(array, 3);
        assert!(index_checked(&words, None, 0));
        assert!(index_checked(&words, None, 1));
        assert!(!index_checked(&words, None, 2), "past the elements");
        assert!(!index_checked(&words, None, 300), "far past the elements");
        words[1] = header_of(0, 3);
        assert!(!index_checked(&words, None, 0), "an instance's fields");
        let integer = crate::heap::Immediate::small_integer(8)
            .expect("a small integer")
            .bits();
        assert!(!index_checked(&words, Some(integer), 0), "an immediate");
        assert!(!index_checked(&words, Some(NIL), 0), "nil");
        words[1] = header_of(array, 254);
        assert!(
            index_checked(&words, None, 252),
            "the header's last element"
        );
        assert!(!index_checked(&words, None, 253), "past the header's count");
        words[1] = header_of(array, 255);
        words[0] = (0xFF << 56) | 300;
        assert!(index_checked(&words, None, 253), "below the overflow mark");
        assert!(
            index_checked(&words, None, 298),
            "the overflow's last element"
        );
        assert!(
            !index_checked(&words, None, 299),
            "past the overflow's count"
        );
    }

    /// A frame number word, as a frame's header holds its guard and its
    /// entered level.
    fn number(value: u64) -> u64 {
        let layout = crate::heap::Heap::machine_layout();
        (value << layout.tag_bits) | u64::try_from(layout.small_integer_tag).expect("a low bit")
    }

    /// An application id table of `rows`, each a slot base and flags, as
    /// `heap/application_ids.rs` lays it out, each row's other words 0.
    fn application_rows(rows: &[(u64, u64)]) -> Vec<u64> {
        let layout = crate::heap::Heap::machine_layout();
        let words = 1 << (layout.application_row_shift - 3);
        let mut table = vec![0u64; rows.len() * words];
        let word = |displacement: i32| usize::try_from(displacement / 8).expect("in the row");
        for (row, (slot_base, flags)) in rows.iter().enumerate() {
            table[row * words + word(layout.application_slot_base_displacement)] = *slot_base;
            table[row * words + word(layout.application_flags_displacement)] = *flags;
        }
        table
    }

    /// A machine whose application id table is `table`, of `rows` rows.
    fn machine_with_rows(table: &[u64], rows: u64) -> Machine {
        use crate::heap::machine_words::MachineWord;
        let mut machine = Machine::new();
        machine.set_machine_word(
            MachineWord::ApplicationIds,
            table.as_ptr().expose_provenance() as u64,
        );
        machine.set_machine_word(MachineWord::ApplicationIdRows, rows);
        machine
    }

    /// An object's words for the own-slot tests: an overflow word, the
    /// header and the class word `class`, then `fields` fields, field
    /// `index` holding `1000 + index`.
    fn instance_words(header: u64, overflow: u64, class: u64, fields: usize) -> Vec<u64> {
        let mut words = vec![overflow, header, class];
        words.extend((0..fields).map(|index| 1000 + index as u64));
        words
    }

    /// The handle of the object `instance_words` laid out, its header its
    /// second word, in the tests' object space, which starts at 0.
    fn instance_handle(words: &[u64]) -> u64 {
        let layout = crate::heap::Heap::machine_layout();
        let header = words.as_ptr().expose_provenance() as u64 + 8;
        header - u64::try_from(layout.header_displacement).expect("the header follows the start")
    }

    /// The header of an instance with guard `guard` and `fields` fields.
    fn instance_header(guard: u64, fields: u64) -> u64 {
        header_of(0, fields + 1) | guard
    }

    /// Runs `check_own_slot_guard` and `load_own_slot_address` for
    /// `own_index` on a frame whose receiver is `receiver` and whose guard
    /// and entered level are `recorded.0` and `recorded.1`, against
    /// `table` of `rows` rows: the field read, or the program counter of
    /// the exit taken, 2 for the guard's cold path, 3 for a field that is
    /// not there.
    fn own_slot_read(
        receiver: u64,
        recorded: (u64, u64),
        own_index: i32,
        table: &[u64],
        rows: u64,
    ) -> Result<u64, u32> {
        let layout = crate::heap::Heap::machine_layout();
        let [first, second, third, fourth] = scratch();
        let mut machine = machine_with_rows(table, rows);
        let body = machine.body(|macro_assembler| {
            let (cold, failed) = (
                macro_assembler.assembler().label(),
                macro_assembler.assembler().label(),
            );
            macro_assembler.check_own_slot_guard(0, first, second, cold);
            macro_assembler.load_own_slot_address(0, own_index, first, third, second, failed);
            macro_assembler.assembler().load(
                ns_assembler::Width::Word64,
                fourth,
                Memory {
                    base: third,
                    displacement: layout.fields_displacement,
                },
            );
            macro_assembler.store_frame_register(15, fourth);
            macro_assembler.exit(ExitKind::Reexecute, 1);
            macro_assembler.assembler().bind(cold);
            macro_assembler.exit(ExitKind::Reexecute, 2);
            macro_assembler.assembler().bind(failed);
            macro_assembler.exit(ExitKind::Reexecute, 3);
        });
        let (guard_register, level_register) = crate::heap::Heap::own_slot_registers(0);
        let mut frame = frame_with(receiver, NIL);
        frame[usize::from(guard_register)] = number(recorded.0);
        frame[usize::from(level_register)] = number(recorded.1);
        let (exit, _) = machine.run(body, &mut frame, 0, false, &mut (), &[]);
        match exit {
            MachineExit::Reexecute { program_counter: 1 } => Ok(frame[15]),
            MachineExit::Reexecute { program_counter } => Err(program_counter),
            other => panic!("unexpected exit {other:?}"),
        }
    }

    /// The own-slot idioms read exactly the field `check_own_slots` and
    /// `Heap::activation_slot_base` address: the receiver's guard must be
    /// the frame's and not `NONE`, the entered level must have a row, and
    /// the slot base plus the index must be one of the instance's fields,
    /// counted by its header or, past 254 fields, its overflow word.
    #[test]
    fn an_own_slot_is_read_exactly_where_the_guard_and_the_level_place_it() {
        const COLD: Result<u64, u32> = Err(2);
        const MISSING: Result<u64, u32> = Err(3);
        let layout = crate::heap::Heap::machine_layout();
        let table = application_rows(&[(0, 0), (2, 0)]);
        let words = instance_words(instance_header(5, 4), 0, NIL, 4);
        let object = instance_handle(&words);
        assert_eq!(own_slot_read(object, (5, 1), 1, &table, 2), Ok(1003));
        assert_eq!(own_slot_read(object, (5, 1), 0, &table, 2), Ok(1002));
        assert_eq!(
            own_slot_read(object, (5, 0), 3, &table, 2),
            Ok(1003),
            "level 0's base"
        );
        assert_eq!(
            own_slot_read(object, (5, 1), 2, &table, 2),
            MISSING,
            "past the fields"
        );
        assert_eq!(
            own_slot_read(object, (5, 1), -3, &table, 2),
            MISSING,
            "below them"
        );
        assert_eq!(
            own_slot_read(object, (6, 1), 1, &table, 2),
            COLD,
            "another guard"
        );
        assert_eq!(
            own_slot_read(object, (5, 2), 1, &table, 2),
            MISSING,
            "no row"
        );
        assert_eq!(
            own_slot_read(object, (5, 1), 1, &table, 1),
            MISSING,
            "past the bound"
        );
        assert_eq!(
            own_slot_read(object, (5, u64::from(u32::MAX)), 1, &table, 2),
            MISSING,
            "UNREGISTERED"
        );
        assert_eq!(
            own_slot_read(number(7), (5, 1), 1, &table, 2),
            COLD,
            "an immediate"
        );
        assert_eq!(own_slot_read(NIL, (5, 1), 1, &table, 2), COLD, "nil");
        let unguarded = instance_words(instance_header(0, 4), 0, NIL, 4);
        let unguarded = instance_handle(&unguarded);
        assert_eq!(
            own_slot_read(unguarded, (0, 1), 1, &table, 2),
            COLD,
            "NONE never matches"
        );
        let array = instance_words(
            header_of(layout.indexable_pointers_format, 5) | 5,
            0,
            NIL,
            4,
        );
        let array = instance_handle(&array);
        assert_eq!(
            own_slot_read(array, (5, 1), 1, &table, 2),
            MISSING,
            "no instance"
        );
        let overflow = u64::try_from(layout.word_count_overflow).expect("a count");
        let large = instance_words(header_of(0, overflow) | 5, 301, NIL, 300);
        let large = instance_handle(&large);
        assert_eq!(
            own_slot_read(large, (5, 1), 297, &table, 2),
            Ok(1299),
            "the overflow's"
        );
        assert_eq!(
            own_slot_read(large, (5, 1), 298, &table, 2),
            MISSING,
            "past it"
        );
    }

    /// `jump_if_value_slots` jumps exactly where `check_value_slot` may
    /// refuse: for a receiver whose class's row carries the value flag,
    /// and whose class names no row the table records.
    #[test]
    fn the_value_check_is_taken_exactly_for_a_value_classs_instance() {
        let [first, second] = scratch();
        let layout = crate::heap::Heap::machine_layout();
        let flag = u64::try_from(layout.application_value_slots_flag).expect("a flag");
        let table = application_rows(&[(0, 0), (0, flag)]);
        let checked = |application_word: u64, rows: u64| {
            let mut class = FakeClass::new();
            class.set(layout.application_displacement, application_word);
            let words = instance_words(instance_header(5, 1), 0, class.handle(), 1);
            let mut machine = machine_with_rows(&table, rows);
            let body = machine.body(|macro_assembler| {
                let label = macro_assembler.assembler().label();
                macro_assembler.load_frame_register(first, 0);
                macro_assembler.jump_if_value_slots(first, second, label);
                macro_assembler.store_frame_register(1, first);
                macro_assembler.exit(ExitKind::Reexecute, 1);
                macro_assembler.assembler().bind(label);
                macro_assembler.store_frame_register(1, first);
                macro_assembler.exit(ExitKind::Reexecute, 2);
            });
            let receiver = instance_handle(&words);
            let mut frame = [receiver, 0];
            let (exit, _) = machine.run(body, &mut frame, 0, false, &mut (), &[]);
            assert_eq!(frame[1], receiver, "the receiver is kept");
            exit == MachineExit::Reexecute { program_counter: 2 }
        };
        assert!(!checked(number(0), 2), "no value class");
        assert!(checked(number(1), 2), "a value class");
        assert!(checked(number(2), 2), "no row");
        assert!(checked(number(1), 1), "past the bound");
        assert!(checked(NIL, 2), "no application");
    }
}

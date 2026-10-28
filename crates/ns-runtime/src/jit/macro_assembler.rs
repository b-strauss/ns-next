//! The VM's idioms, written once in `ns-assembler`'s operations: what a
//! template, a stub and every later tier emit to touch a frame, call a
//! Rust leaf, poll or leave machine code, so that none of them reaches an
//! encoder and none can drift from another (`docs/design/execution.md`,
//! "The foundation and the one extension point"). Which machine register
//! plays which role is decided here, per ABI ("Fixed registers and the
//! calling convention").

use ns_assembler::x64::{
    DIVIDEND, R10, R11, R12, R13, R14, R15, R8, R9, RAX, RBP, RBX, RCX, RDI, RDX, REMAINDER, RSI,
    RSP,
};
use ns_assembler::{
    Assembled, Assembler, AssemblyError, Condition, Label, Memory, Register, Width, X64,
};

use super::stubs::{BlockWord, ExitKind};
use crate::heap::machine_words::{MachineWord, MachineWords};

/// The platform calling convention the stubs and leaf calls cross.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Abi {
    /// Microsoft's x64 convention.
    Windows64,
    /// The System V AMD64 convention of Linux and macOS.
    SystemV,
}

impl Abi {
    /// The convention of the platform this build runs on.
    pub(crate) fn host() -> Abi {
        if cfg!(windows) {
            Abi::Windows64
        } else {
            Abi::SystemV
        }
    }

    /// The registers a callee of this convention preserves, in the order
    /// the entry stub pushes them.
    pub(crate) fn callee_saved(self) -> &'static [Register] {
        match self {
            Abi::Windows64 => &[RBX, RBP, RDI, RSI, R12, R13, R14, R15],
            Abi::SystemV => &[RBX, RBP, R12, R13, R14, R15],
        }
    }

    /// The register a function of this convention takes its first
    /// argument in.
    pub(crate) fn first_argument(self) -> Register {
        match self {
            Abi::Windows64 => RCX,
            Abi::SystemV => RDI,
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
    /// `r13` plays no role and is no scratch register: preserved by both
    /// conventions, it is the one free register a value survives a leaf
    /// call in, which the register cache is to have.
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
        RCX
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
    assembler: Assembler<X64>,
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
    /// Whether a back edge counts itself (`count_back_edge`).
    count_back_edges: bool,
}

impl MacroAssembler {
    pub(crate) fn new(abi: Abi) -> MacroAssembler {
        MacroAssembler {
            assembler: Assembler::new(),
            roles: Roles::of(abi),
            abi,
            #[cfg(debug_assertions)]
            program_counter: 0,
            cold_paths: Vec::new(),
            exit_at_every_poll: super::stress_exit_from_env(),
            count_back_edges: false,
        }
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

    pub(crate) fn roles(&self) -> Roles {
        self.roles
    }

    /// The abstract operations, for what no idiom covers.
    pub(crate) fn assembler(&mut self) -> &mut Assembler<X64> {
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
            self.cold_paths.push(ColdPath::DeadRegister {
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
        let layout = LeafCallLayout::of(self.abi);
        for argument in arguments {
            assert!(
                argument != Operand::Register(RSP),
                "a leaf call's operand cannot be the stack pointer it realigns"
            );
        }
        let saved_stack_pointer = self.block_word(BlockWord::LeafStackPointer);
        self.assembler
            .store(Width::Word64, saved_stack_pointer, RSP);
        self.assembler.and_immediate(RSP, -16);
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
        self.assembler.load(Width::Word64, RSP, saved_stack_pointer);
    }

    /// Leaves machine code through the exit stub, which writes `kind` and
    /// `program_counter` into the block and returns to `enter`'s caller.
    /// A jump rather than a call: the stub never returns here, and a call
    /// that never returns would leave the processor's return-address
    /// predictor unbalanced on every exit.
    pub(crate) fn exit(&mut self, kind: ExitKind, program_counter: u32) {
        let roles = self.roles;
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
        self.cold_paths.push(ColdPath::Exit {
            label,
            kind,
            program_counter,
        });
    }

    /// Adds one to the entry block's `BackEdges` when the code counts its
    /// back edges (`set_count_back_edges`), and emits nothing otherwise.
    /// Clobbers the flags and the idiom scratch register.
    pub(crate) fn count_back_edge(&mut self) {
        if !self.count_back_edges {
            return;
        }
        let scratch = self.roles.idiom_scratch;
        let count = self.block_word(BlockWord::BackEdges);
        self.assembler.load(Width::Word64, scratch, count);
        self.assembler.add_immediate(scratch, 1);
        self.assembler.store(Width::Word64, count, scratch);
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
        self.cold_paths.push(ColdPath::Exit {
            label,
            kind: ExitKind::Safepoint,
            program_counter,
        });
    }

    /// Lays the code out, the cold paths after the straight line, so a
    /// check that passes falls through rather than jumping over its
    /// failure.
    pub(crate) fn finish(mut self) -> Result<Assembled, AssemblyError> {
        for cold_path in std::mem::take(&mut self.cold_paths) {
            match cold_path {
                ColdPath::Exit {
                    label,
                    kind,
                    program_counter,
                } => {
                    self.assembler.bind(label);
                    self.exit(kind, program_counter);
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
        self.assembler.finish()
    }
}

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
        }
    }
}

/// The stack word at `displacement` from the stack pointer.
fn stack_slot(displacement: i32) -> Memory {
    Memory {
        base: RSP,
        displacement,
    }
}

/// These name x64's registers, so they are x64's whatever `cfg(ns_jit)`
/// says (`build.rs`).
#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use ns_assembler::x64::RAX;

    use crate::jit::stubs::testing::Machine;
    use crate::jit::stubs::{ExitKind, MachineExit};

    #[test]
    fn a_body_moves_one_frame_register_into_another() {
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.set_program_counter(0);
            macro_assembler.load_frame_register(RAX, 2);
            macro_assembler.store_frame_register(5, RAX);
            macro_assembler.exit(ExitKind::Reexecute, 1);
        });
        let mut frame = [10, 11, 12, 13, 14, 15, 16, 17];
        let (exit, _) = machine.run(body, &mut frame, 0, false, &mut (), &[]);
        assert_eq!(exit, MachineExit::Reexecute { program_counter: 1 });
        assert_eq!(frame, [10, 11, 12, 13, 14, 12, 16, 17]);
    }

    #[test]
    fn the_register_base_places_the_frame_in_the_stack() {
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.load_frame_register(RAX, 0);
            macro_assembler.store_frame_register(1, RAX);
            macro_assembler.exit(ExitKind::Reexecute, 1);
        });
        let mut frame = [10, 11, 12, 13, 14, 15, 16, 17];
        machine.run(body, &mut frame, 3, false, &mut (), &[]);
        assert_eq!(frame, [10, 11, 12, 13, 13, 15, 16, 17]);
    }

    #[test]
    fn an_immediate_word_of_every_width_reaches_a_register() {
        let words = [0, 7, 0xFFFF_FFFF, u64::MAX, 0x1234_5678_9ABC_DEF0];
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            for (register, word) in (0u16..).zip(words) {
                macro_assembler.load_immediate_word(RAX, word);
                macro_assembler.store_frame_register(register, RAX);
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
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.set_program_counter(9);
            macro_assembler.load_frame_register(RAX, 3);
            macro_assembler.store_frame_register(5, RAX);
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
}

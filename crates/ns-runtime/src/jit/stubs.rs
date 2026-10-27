//! The entry stub, which crosses from the platform's C calling convention
//! into machine code, and the exit stub, through which machine code leaves
//! (`docs/design/execution.md`, "The native stack carries the return
//! addresses"); the entry block they share with `CodeZone::enter`; and the
//! one decoding of an exit.

use ns_assembler::x64::RSP;
use ns_assembler::{Assembled, Width};
use ns_memory::ENTRY_BLOCK_WORDS;

use super::macro_assembler::{Abi, MacroAssembler};

/// The words of the entry block, by index. `CodeZone::enter` writes the
/// first four on every entry; the caller of `enter` writes the inputs
/// (`ObjectSpaceBase` … `MachineEntry`, `PollWord`, `ExitStub`,
/// `Activation`, and `BackEdges`' zero) before it, the exit stub the
/// outputs (`ExitProgramCounter`, `ExitKind`, and `ExitRegister` for a
/// dead register's exit), and code that counts its back edges `BackEdges`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BlockWord {
    /// The leaf shim's address, which `call_leaf` calls. Words 0, 2 and 3,
    /// the context, the leaf table and its length, only the shim reads.
    Shim = 1,
    /// The object space's base address, every handle's origin.
    ObjectSpaceBase = 4,
    /// The activation stack's base address.
    StackBase = 5,
    /// The frame's register base, in words from the stack's base.
    RegisterBase = 6,
    /// The address of the body to call.
    MachineEntry = 7,
    /// The program counter the interpreter resumes at.
    ExitProgramCounter = 8,
    /// Why the code left: an `ExitKind`, also `enter`'s answer.
    ExitKind = 9,
    /// The stack pointer the entry stub called the body with, which the
    /// exit stub returns to.
    SavedStackPointer = 10,
    /// The address of the heap's safepoint flag, one byte, a Rust `bool`.
    PollWord = 11,
    /// The register a dead register's exit read.
    ExitRegister = 12,
    /// The exit stub's address. A body reaches the stub through this word
    /// rather than an address of its own, so its code holds no absolute
    /// address and can be moved within the zone.
    ExitStub = 13,
    /// The running activation, as `Heap::activation_word` makes it, which
    /// a leaf that steps an instruction is handed.
    Activation = 14,
    /// The backward jumps code compiled to count them took since the entry
    /// (`JitStatistics::machine_back_edges`).
    BackEdges = 15,
}

impl BlockWord {
    /// The word's index into the block.
    pub(crate) fn index(self) -> usize {
        self as usize
    }

    /// The word's byte offset from the block's start.
    pub(crate) fn displacement(self) -> i32 {
        self as i32 * 8
    }
}

const _: () = assert!((BlockWord::BackEdges as usize) < ENTRY_BLOCK_WORDS);

/// Why machine code left, as the exit stub answers it in `ExitKind`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ExitKind {
    /// The interpreter re-executes the instruction at the exit's program
    /// counter: an instruction without a template or leaf, or a template's
    /// decline.
    Reexecute = 0,
    /// A poll found a safepoint requested; the interpreter takes it before
    /// the instruction at the exit's program counter.
    Safepoint = 1,
    /// A debug build's read of a dead register: the interpreter refuses it
    /// as its own read is refused.
    DeadRegister = 2,
}

/// An exit, decoded from the block and the kind `enter` answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MachineExit {
    Reexecute { program_counter: u32 },
    Safepoint { program_counter: u32 },
    DeadRegister { program_counter: u32, register: u16 },
}

/// The exit `kind` and the block an entry left behind describe.
///
/// # Panics
///
/// On a kind, program counter or register no exit writes: the stubs and
/// the macro assembler write only those, so any other is a defect of the
/// JIT.
pub(crate) fn decode_exit(block: &[u64; ENTRY_BLOCK_WORDS], kind: u64) -> MachineExit {
    let word = block[BlockWord::ExitProgramCounter.index()];
    let Ok(program_counter) = u32::try_from(word) else {
        panic!("internal: machine code exited at program counter {word}");
    };
    if kind == ExitKind::Reexecute as u64 {
        MachineExit::Reexecute { program_counter }
    } else if kind == ExitKind::Safepoint as u64 {
        MachineExit::Safepoint { program_counter }
    } else if kind == ExitKind::DeadRegister as u64 {
        let word = block[BlockWord::ExitRegister.index()];
        let Ok(register) = u16::try_from(word) else {
            panic!("internal: machine code exited reading register {word}");
        };
        MachineExit::DeadRegister {
            program_counter,
            register,
        }
    } else {
        panic!("internal: machine code exited with kind {kind}")
    }
}

/// The entry stub: a function of `abi`'s C calling convention taking the
/// block's address, as `CodeZone::enter` calls it. It saves every
/// general-purpose register the convention has its callee preserve, so a
/// body may use them all -- but no vector register: on Win64 XMM6-XMM15
/// are callee-preserved too, so a template that uses one of them requires
/// this stub to save them; pins the roles from the block; saves the stack pointer for the exit
/// stub; and calls the body. A body never returns to it: it leaves only
/// through the exit stub, which unwinds the native stack to the saved
/// pointer, so what follows the call traps.
pub(crate) fn assemble_entry_stub(abi: Abi) -> Assembled {
    let mut macro_assembler = MacroAssembler::new(abi);
    let roles = macro_assembler.roles();
    let stack_base = macro_assembler.block_word(BlockWord::StackBase);
    let register_base = macro_assembler.block_word(BlockWord::RegisterBase);
    let object_space_base = macro_assembler.block_word(BlockWord::ObjectSpaceBase);
    let machine_entry = macro_assembler.block_word(BlockWord::MachineEntry);
    let saved_stack_pointer = macro_assembler.block_word(BlockWord::SavedStackPointer);
    let assembler = macro_assembler.assembler();
    for register in abi.callee_saved() {
        assembler.push(*register);
    }
    assembler.move_register(roles.context, abi.first_argument());
    assembler.load(Width::Word64, roles.frame, stack_base);
    assembler.load(Width::Word64, roles.idiom_scratch, register_base);
    assembler.shift_left(roles.idiom_scratch, 3);
    assembler.add(roles.frame, roles.idiom_scratch);
    assembler.load(Width::Word64, roles.object_space, object_space_base);
    assembler.load(Width::Word64, roles.idiom_scratch, machine_entry);
    assembler.store(Width::Word64, saved_stack_pointer, RSP);
    assembler.call_register(roles.idiom_scratch);
    assembler.trap();
    finish_stub(macro_assembler)
}

/// A stub's code, which has no label and so always lays out.
fn finish_stub(macro_assembler: MacroAssembler) -> Assembled {
    match macro_assembler.finish() {
        Ok(assembled) => assembled,
        Err(error) => panic!("internal: a stub without labels failed to lay out: {error}"),
    }
}

/// The exit stub, which a body jumps to with the exit's kind and program
/// counter in the roles' `exit_kind` and `exit_program_counter`: it writes
/// both into the block, resets the stack pointer to the one the entry stub
/// saved -- dropping every return address a body pushed -- restores the registers the entry stub
/// saved and returns the kind to `enter`'s caller.
pub(crate) fn assemble_exit_stub(abi: Abi) -> Assembled {
    let mut macro_assembler = MacroAssembler::new(abi);
    let roles = macro_assembler.roles();
    let exit_kind = macro_assembler.block_word(BlockWord::ExitKind);
    let exit_program_counter = macro_assembler.block_word(BlockWord::ExitProgramCounter);
    let saved_stack_pointer = macro_assembler.block_word(BlockWord::SavedStackPointer);
    let assembler = macro_assembler.assembler();
    assembler.store(Width::Word64, exit_kind, roles.exit_kind());
    assembler.store(
        Width::Word64,
        exit_program_counter,
        roles.exit_program_counter(),
    );
    assembler.load(Width::Word64, RSP, saved_stack_pointer);
    for register in abi.callee_saved().iter().rev() {
        assembler.pop(*register);
    }
    assembler.return_();
    finish_stub(macro_assembler)
}

/// Running machine code on a fake frame, for the tests of the stubs and
/// the idioms: a zone holding both stubs, bodies installed after them.
#[cfg(test)]
pub(super) mod testing {
    use ns_assembler::Assembled;
    use ns_memory::{CodeOffset, CodeZone, Leaf, ENTRY_BLOCK_WORDS};

    use super::{assemble_entry_stub, assemble_exit_stub, decode_exit, BlockWord, MachineExit};
    use crate::jit::macro_assembler::{Abi, MacroAssembler};

    /// Every piece of code starts at a multiple of this.
    const ALIGNMENT: u32 = 64;

    pub(crate) struct Machine {
        zone: CodeZone,
        entry: CodeOffset,
        exit: CodeOffset,
        next: u32,
    }

    impl Machine {
        pub(crate) fn new() -> Machine {
            let zone = CodeZone::reserve(1 << 16).expect("a zone");
            let mut machine = Machine {
                zone,
                entry: CodeOffset(0),
                exit: CodeOffset(0),
                next: 0,
            };
            machine.entry = machine.install(&assemble_entry_stub(Abi::host()));
            machine.exit = machine.install(&assemble_exit_stub(Abi::host()));
            machine
        }

        pub(crate) fn install(&mut self, assembled: &Assembled) -> CodeOffset {
            let offset = CodeOffset(self.next);
            self.zone
                .writer()
                .write(offset, &assembled.bytes)
                .expect("written");
            let length = u32::try_from(assembled.bytes.len()).expect("small");
            self.next = (self.next + length).next_multiple_of(ALIGNMENT);
            offset
        }

        /// Installs the body `build` writes and finishes.
        pub(crate) fn body(&mut self, build: impl FnOnce(&mut MacroAssembler)) -> CodeOffset {
            let mut macro_assembler = MacroAssembler::new(Abi::host());
            build(&mut macro_assembler);
            let assembled = macro_assembler.finish().expect("assembled");
            self.install(&assembled)
        }

        /// Runs `body` on `frame`, its registers starting `register_base`
        /// words into it, and answers the exit and the block.
        pub(crate) fn run<C>(
            &self,
            body: CodeOffset,
            frame: &mut [u64],
            register_base: u64,
            safepoint_requested: &bool,
            context: &mut C,
            leaves: &[Leaf<C>],
        ) -> (MachineExit, [u64; ENTRY_BLOCK_WORDS]) {
            let mut block = [0u64; ENTRY_BLOCK_WORDS];
            block[BlockWord::StackBase.index()] = frame.as_mut_ptr().expose_provenance() as u64;
            block[BlockWord::RegisterBase.index()] = register_base;
            block[BlockWord::MachineEntry.index()] = self.zone.executable_address(body) as u64;
            block[BlockWord::ExitStub.index()] = self.zone.executable_address(self.exit) as u64;
            block[BlockWord::PollWord.index()] =
                std::ptr::from_ref(safepoint_requested).expose_provenance() as u64;
            let kind = self.zone.enter(self.entry, &mut block, context, leaves);
            assert_eq!(kind, block[BlockWord::ExitKind.index()]);
            (decode_exit(&block, kind), block)
        }
    }
}

#[cfg(test)]
mod tests {
    use ns_assembler::x64::{R12, R13, R8, RAX, RBP, RCX, RDI, RDX, RSI};
    use ns_memory::Leaf;

    use super::testing::Machine;
    use super::{BlockWord, ExitKind, MachineExit};
    use crate::jit::macro_assembler::{LeafIndex, Operand};

    #[test]
    fn the_entry_stub_runs_a_body_that_exits_at_once() {
        let mut machine = Machine::new();
        // The body overwrites every register the entry stub saves and the
        // roles do not already overwrite, so a register the stubs failed to
        // restore shows in the locals Rust keeps across the calls.
        let body = machine.body(|macro_assembler| {
            for register in [RBP, RSI, RDI, R12, R13] {
                macro_assembler.load_immediate_word(register, 0xDEAD_BEEF_0000_0000);
            }
            macro_assembler.exit(ExitKind::Reexecute, 7);
        });
        let mut frame = [0u64; 8];
        let mut live = std::hint::black_box(0u64);
        let mut other = std::hint::black_box(1u64);
        for iteration in 0..1000u64 {
            for _ in 0..2 {
                let (exit, block) = machine.run(body, &mut frame, 0, &false, &mut (), &[]);
                assert_eq!(exit, MachineExit::Reexecute { program_counter: 7 });
                assert_eq!(block[BlockWord::ExitProgramCounter.index()], 7);
                assert_eq!(
                    block[BlockWord::ExitKind.index()],
                    ExitKind::Reexecute as u64
                );
            }
            live += iteration;
            other = other.wrapping_mul(3);
        }
        assert_eq!(std::hint::black_box(live), 499_500);
        assert_eq!(std::hint::black_box(other), 3u64.wrapping_pow(1000));
    }

    fn add_arguments(counter: &mut u64, arguments: [u64; 3]) -> u64 {
        *counter += arguments[0] + arguments[1] + arguments[2];
        *counter
    }

    #[test]
    fn a_leaf_call_from_a_body_reaches_the_context() {
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.call_leaf(
                LeafIndex(0),
                [
                    Operand::Immediate(5),
                    Operand::Immediate(6),
                    Operand::Immediate(7),
                ],
            );
            let result = macro_assembler.roles().leaf_result;
            macro_assembler.store_frame_register(0, result);
            macro_assembler.exit(ExitKind::Reexecute, 1);
        });
        let mut frame = [0u64; 8];
        let mut counter = 100u64;
        let leaves: [Leaf<u64>; 1] = [add_arguments];
        let (exit, _) = machine.run(body, &mut frame, 0, &false, &mut counter, &leaves);
        assert_eq!(exit, MachineExit::Reexecute { program_counter: 1 });
        assert_eq!(counter, 118);
        assert_eq!(frame[0], 118, "the leaf's answer");
    }

    fn record_first_call(recorded: &mut [[u64; 3]; 2], arguments: [u64; 3]) -> u64 {
        recorded[0] = arguments;
        0
    }

    fn record_second_call(recorded: &mut [[u64; 3]; 2], arguments: [u64; 3]) -> u64 {
        recorded[1] = arguments;
        0
    }

    #[test]
    fn a_leaf_call_takes_its_operands_from_the_argument_registers_themselves() {
        // Each operand sits in a register another argument is passed in, on
        // both ABIs, so moving the arguments one by one would overwrite one
        // before it is read.
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.load_frame_register(RCX, 0);
            macro_assembler.load_frame_register(RDX, 1);
            macro_assembler.load_frame_register(R8, 2);
            macro_assembler.call_leaf(
                LeafIndex(0),
                [
                    Operand::Register(R8),
                    Operand::Register(RCX),
                    Operand::Register(RDX),
                ],
            );
            // Immediates beside a register, the third one too wide for a
            // sign-extended store.
            macro_assembler.load_frame_register(RDX, 1);
            macro_assembler.call_leaf(
                LeafIndex(1),
                [
                    Operand::Immediate(1 << 40),
                    Operand::Register(RDX),
                    Operand::Immediate(u64::MAX - (1 << 40)),
                ],
            );
            macro_assembler.exit(ExitKind::Reexecute, 2);
        });
        let mut frame = [11, 22, 33, 0, 0, 0, 0, 0];
        let mut recorded = [[0u64; 3]; 2];
        let leaves: [Leaf<[[u64; 3]; 2]>; 2] = [record_first_call, record_second_call];
        let (exit, _) = machine.run(body, &mut frame, 0, &false, &mut recorded, &leaves);
        assert_eq!(exit, MachineExit::Reexecute { program_counter: 2 });
        assert_eq!(recorded[0], [33, 11, 22]);
        assert_eq!(recorded[1], [1 << 40, 22, u64::MAX - (1 << 40)]);
    }

    /// Answers the misalignment of a local the compiler placed at a 16-byte
    /// boundary of its frame, which is 16-byte aligned only when the stack
    /// pointer was at the call.
    fn stack_misalignment(_context: &mut (), _arguments: [u64; 3]) -> u64 {
        #[repr(align(16))]
        struct Aligned([u8; 16]);
        let local = Aligned([0; 16]);
        let address = std::ptr::from_ref(std::hint::black_box(&local.0)).addr();
        (address % 16) as u64
    }

    #[test]
    fn a_leaf_call_aligns_the_native_stack_at_any_depth() {
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            for depth in 0..4u16 {
                for _ in 0..depth {
                    macro_assembler.assembler().push(RAX);
                }
                macro_assembler.call_leaf(LeafIndex(0), [Operand::Immediate(0); 3]);
                let result = macro_assembler.roles().leaf_result;
                macro_assembler.store_frame_register(depth, result);
                for _ in 0..depth {
                    macro_assembler.assembler().pop(RCX);
                }
            }
            macro_assembler.exit(ExitKind::Reexecute, 0);
        });
        let mut frame = [u64::MAX; 8];
        let leaves: [Leaf<()>; 1] = [stack_misalignment];
        machine.run(body, &mut frame, 0, &false, &mut (), &leaves);
        assert_eq!(frame[..4], [0, 0, 0, 0]);
    }

    #[test]
    fn a_poll_exits_only_when_a_safepoint_is_requested() {
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.set_exit_at_every_poll(false);
            macro_assembler.poll(3);
            macro_assembler.load_immediate_word(RAX, 42);
            macro_assembler.store_frame_register(0, RAX);
            macro_assembler.exit(ExitKind::Reexecute, 4);
        });
        let mut frame = [0u64; 8];
        let (exit, _) = machine.run(body, &mut frame, 0, &true, &mut (), &[]);
        assert_eq!(exit, MachineExit::Safepoint { program_counter: 3 });
        assert_eq!(frame[0], 0, "nothing after the poll ran");
        let (exit, _) = machine.run(body, &mut frame, 0, &false, &mut (), &[]);
        assert_eq!(exit, MachineExit::Reexecute { program_counter: 4 });
        assert_eq!(frame[0], 42);
    }

    #[test]
    fn a_poll_under_the_exit_stress_mode_exits_whatever_is_requested() {
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.set_exit_at_every_poll(true);
            macro_assembler.poll(3);
            macro_assembler.load_immediate_word(RAX, 42);
            macro_assembler.store_frame_register(0, RAX);
            macro_assembler.exit(ExitKind::Reexecute, 4);
        });
        let mut frame = [0u64; 8];
        let (exit, _) = machine.run(body, &mut frame, 0, &false, &mut (), &[]);
        assert_eq!(exit, MachineExit::Safepoint { program_counter: 3 });
        assert_eq!(frame[0], 0, "nothing after the poll ran");
    }
}

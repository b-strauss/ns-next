//! The entry stub, which crosses from the platform's C calling convention
//! into machine code and takes the return of the frame it called, the
//! exit stub, through which machine code leaves once every compiled frame
//! is materialized (`docs/design/execution.md`, "The native stack carries
//! the return addresses", "The exit materializes"), and the stubs a send's
//! cell names for a target without machine code of its own; the entry
//! block they share with `CodeZone::enter`; and the one decoding of an
//! exit.

use ns_assembler::{Assembled, Condition, Memory, Width};
use ns_memory::{EntryBlock, ENTRY_BLOCK_WORDS};

use super::leaves::Leaf;
use super::macro_assembler::{Abi, MacroAssembler, Operand};
use crate::heap::machine_words::MachineWord;

/// The words of the entry block, by index. `CodeZone::enter` writes the
/// first four on every entry; the caller of `enter` writes the inputs
/// before it -- `RegisterBase`, `MachineEntry` and, while counting, the
/// counters' zero at every entry, and `ObjectSpaceBase`, `StackBase`,
/// `MachineWords`, `ExitStub`, `Cells`, `PolymorphicStub`,
/// `PolymorphicBlocks` … `GlobalMask`, which name what lasts from one
/// entry to the next, only when the block is stale
/// (`JitState::mark_block_stale`) -- the exit stub the
/// outputs (`ExitProgramCounter`, `ExitKind`, and `ExitRegister` for a
/// dead register's exit), a leaf call `LeafStackPointer`, and code that
/// counts its back edges `BackEdges`.
///
/// Words 13 and 22 to 28 are free, kept for what the later stages enter
/// machine code with, so the three words only a debug build or a test
/// uses sit at the top.
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
    /// The address of the heap's machine words (`heap/machine_words.rs`),
    /// the words machine code reads and writes between leaf calls, among
    /// them the safepoint request a poll reads.
    MachineWords = 11,
    /// The exit stub's address. A body reaches the stub through this word
    /// rather than an address of its own, so its code holds no absolute
    /// address and can be moved within the zone.
    ExitStub = 12,
    /// The stack pointer a leaf call aligned away from, restored after
    /// it: a word rather than a register, since it is needed only across
    /// the call, and a callee-saved register is worth more to the register
    /// cache.
    LeafStackPointer = 14,
    /// The address of the site cells (`Cells::site_address` of site 0): a
    /// send's cell lies its site's index cells past it.
    Cells = 15,
    /// The address of the stub a send whose guard is not its cell's calls
    /// (`JitState::polymorphic_stub_address`), which a polymorphic site's
    /// cell names as its entry too.
    PolymorphicStub = 16,
    /// The exit's walk over the return addresses on the native stack: the
    /// address of the next one to materialize the frame of.
    WalkCursor = 17,
    /// An address inside the code an exit left, which names the body of
    /// the innermost frame (`Roles::exit_code_address`).
    ExitCodeAddress = 18,
    /// The address of polymorphic block 0's cells
    /// (`InlineCaches::block_cells_address`): a block lies its number of
    /// blocks past it.
    PolymorphicBlocks = 19,
    /// The address of the global cache's mirror
    /// (`InlineCaches::global_rows_address`), which the megamorphic stub
    /// probes.
    GlobalRows = 20,
    /// The global cache's group mask (`InlineCaches::global_mask`), which
    /// the megamorphic stub picks a group with: a word of the block rather
    /// than a constant of the code, since a test may resize the cache
    /// between two entries, which marks the block stale
    /// (`JitState::mark_block_stale`).
    GlobalMask = 21,
    /// The sends code compiled to count them called compiled code through
    /// (`JitStatistics::compiled_calls`).
    CompiledCalls = 29,
    /// The register a dead register's exit read, in a debug build.
    ExitRegister = 30,
    /// The backward jumps code compiled to count them took since the entry
    /// (`JitStatistics::machine_back_edges`).
    BackEdges = 31,
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
    /// A compiled call was refused before its callee was pushed -- by the
    /// callee's prologue, at the depth limit, past the stack's room or
    /// with a safepoint requested, or by a stub, for a target machine
    /// code does not run -- so the caller re-executes the send and the
    /// interpreter decides it.
    PushRefused = 3,
    /// The frame the entry stub called returned: its value is in the
    /// block's `ExitProgramCounter`.
    Returned = 4,
}

/// An exit, decoded from the block and the kind `enter` answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MachineExit {
    Reexecute { program_counter: u32 },
    Safepoint { program_counter: u32 },
    DeadRegister { program_counter: u32, register: u16 },
    PushRefused,
    Returned { value: u64 },
}

/// The exit `kind` and the block an entry left behind describe.
///
/// # Panics
///
/// On a kind, program counter or register no exit writes: the stubs and
/// the macro assembler write only those, so any other is a defect of the
/// JIT.
pub(crate) fn decode_exit(block: &EntryBlock, kind: u64) -> MachineExit {
    let word = block.get(BlockWord::ExitProgramCounter.index());
    if kind == ExitKind::Returned as u64 {
        return MachineExit::Returned { value: word };
    }
    if kind == ExitKind::PushRefused as u64 {
        return MachineExit::PushRefused;
    }
    let Ok(program_counter) = u32::try_from(word) else {
        panic!("internal: machine code exited at program counter {word}");
    };
    if kind == ExitKind::Reexecute as u64 {
        MachineExit::Reexecute { program_counter }
    } else if kind == ExitKind::Safepoint as u64 {
        MachineExit::Safepoint { program_counter }
    } else if kind == ExitKind::DeadRegister as u64 {
        let word = block.get(BlockWord::ExitRegister.index());
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
/// this stub to save them; pins the roles from the block; saves the stack
/// pointer for the exit stub; and calls the body. The body leaves either
/// through the exit stub, which unwinds the native stack to the saved
/// pointer, or by returning the frame it was entered at, by `ret` with its
/// value in `rax`: the stub then answers `ExitKind::Returned` with the
/// value in the block's `ExitProgramCounter`, so that the run loop pops
/// the frame and applies its continuation (`vm::finish`). That frame's
/// `Return` took one off the depth count, as every compiled frame's does,
/// but the run loop's pop takes it off too, so the stub adds it back.
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
    assembler.store(Width::Word64, saved_stack_pointer, roles.stack_pointer);
    assembler.call_register(roles.idiom_scratch);
    let returned_value = macro_assembler.block_word(BlockWord::ExitProgramCounter);
    let exit_kind = macro_assembler.block_word(BlockWord::ExitKind);
    let assembler = macro_assembler.assembler();
    assembler.store(Width::Word64, returned_value, roles.leaf_result);
    macro_assembler.load_machine_words(roles.idiom_scratch);
    let depth = MacroAssembler::machine_word(roles.idiom_scratch, MachineWord::DepthCount);
    let assembler = macro_assembler.assembler();
    let count = roles.depth_count;
    assembler.load(Width::Word64, count, depth);
    assembler.add_immediate(count, 1);
    assembler.store(Width::Word64, depth, count);
    assembler.move_immediate(roles.leaf_result, ExitKind::Returned as u64);
    assembler.store(Width::Word64, exit_kind, roles.leaf_result);
    for register in abi.callee_saved().iter().rev() {
        assembler.pop(*register);
    }
    assembler.return_();
    finish_stub(macro_assembler)
}

/// A stub's code, whose few short branches always lay out.
pub(super) fn finish_stub(macro_assembler: MacroAssembler) -> Assembled {
    match macro_assembler.finish() {
        Ok(assembled) => assembled,
        Err(error) => panic!("internal: a stub failed to lay out: {error}"),
    }
}

/// The exit stub, which a body jumps to with the exit's kind, program
/// counter and an address inside it in the roles' `exit_kind`,
/// `exit_program_counter` and `exit_code_address`: it writes the three
/// into the block; when compiled frames lie above the frame the entry stub
/// called -- the stack pointer below the entry's return address -- it
/// walks their return addresses, outermost first, materializing each
/// frame (`Leaf::MaterializeFrame`), then the innermost
/// (`Leaf::MaterializeInnermost`), so that every frame reads as an
/// interpreted one before the run loop or any observer runs; then it
/// resets the stack pointer to the one the entry stub saved -- dropping
/// every return address a body pushed -- restores the registers the entry
/// stub saved and returns the kind to `enter`'s caller.
pub(crate) fn assemble_exit_stub(abi: Abi) -> Assembled {
    let mut macro_assembler = MacroAssembler::new(abi);
    let roles = macro_assembler.roles();
    let exit_kind = macro_assembler.block_word(BlockWord::ExitKind);
    let exit_program_counter = macro_assembler.block_word(BlockWord::ExitProgramCounter);
    let exit_code_address = macro_assembler.block_word(BlockWord::ExitCodeAddress);
    let saved_stack_pointer = macro_assembler.block_word(BlockWord::SavedStackPointer);
    let cursor = macro_assembler.block_word(BlockWord::WalkCursor);
    let scratch = roles.idiom_scratch;
    let assembler = macro_assembler.assembler();
    assembler.store(Width::Word64, exit_kind, roles.exit_kind());
    assembler.store(
        Width::Word64,
        exit_program_counter,
        roles.exit_program_counter(),
    );
    assembler.store(Width::Word64, exit_code_address, roles.exit_code_address());
    // The body was called with the stack pointer one word below the saved
    // one, the entry stub's return address; a compiled frame above it
    // pushed one more each.
    let (walk, next, innermost, done) = (
        assembler.label(),
        assembler.label(),
        assembler.label(),
        assembler.label(),
    );
    assembler.move_register(scratch, roles.stack_pointer);
    assembler.add_immediate(scratch, RETURN_ADDRESS_BYTES);
    assembler.compare_memory(scratch, saved_stack_pointer);
    assembler.jump_if(Condition::Equal, done);
    assembler.bind(walk);
    // The outermost compiled return address, the entry's body's call, lies
    // two words below the saved stack pointer.
    assembler.load(Width::Word64, scratch, saved_stack_pointer);
    assembler.add_immediate(scratch, -2 * RETURN_ADDRESS_BYTES);
    assembler.store(Width::Word64, cursor, scratch);
    assembler.bind(next);
    assembler.load(Width::Word64, scratch, cursor);
    assembler.compare(scratch, roles.stack_pointer);
    assembler.jump_if(Condition::Below, innermost);
    assembler.load(
        Width::Word64,
        roles.return_address,
        Memory {
            base: scratch,
            displacement: 0,
        },
    );
    macro_assembler.call_leaf(
        Leaf::MaterializeFrame.index(),
        [
            Operand::Register(roles.return_address),
            Operand::Immediate(0),
            Operand::Immediate(0),
        ],
    );
    let assembler = macro_assembler.assembler();
    assembler.load(Width::Word64, scratch, cursor);
    assembler.add_immediate(scratch, -RETURN_ADDRESS_BYTES);
    assembler.store(Width::Word64, cursor, scratch);
    assembler.jump(next);
    assembler.bind(innermost);
    macro_assembler.call_leaf(
        Leaf::MaterializeInnermost.index(),
        [
            Operand::Block(BlockWord::ExitCodeAddress),
            Operand::Block(BlockWord::ExitKind),
            Operand::Block(BlockWord::ExitProgramCounter),
        ],
    );
    let assembler = macro_assembler.assembler();
    assembler.bind(done);
    assembler.load(Width::Word64, roles.exit_kind(), exit_kind);
    assembler.load(Width::Word64, roles.stack_pointer, saved_stack_pointer);
    for register in abi.callee_saved().iter().rev() {
        assembler.pop(*register);
    }
    assembler.return_();
    finish_stub(macro_assembler)
}

/// The bytes a return address takes on the native stack.
const RETURN_ADDRESS_BYTES: i32 = 8;

/// The miss stub, which a send's cell names for a method whose body has no
/// machine code yet, and which the polymorphic and megamorphic stubs end
/// in where they find no entry for the send (`emit_miss`).
pub(crate) fn assemble_miss_stub(abi: Abi) -> Assembled {
    let mut macro_assembler = MacroAssembler::new(abi);
    emit_miss(&mut macro_assembler);
    finish_stub(macro_assembler)
}

/// The miss: code entered as the callee is, the frame register at the
/// callee's register 0 and the send's return address on top of the
/// native stack, which names the send. The miss leaf decides it
/// (`Leaf::Miss`) and answers the callee's machine entry, which the code
/// jumps to as though the send had called it, the callee's receiver and
/// current class the leaf handed over in the roles' `callee_receiver` and
/// `callee_class`, or 0, at which it refuses the call so that the caller
/// re-executes the send.
pub(super) fn emit_miss(macro_assembler: &mut MacroAssembler) {
    let roles = macro_assembler.roles();
    macro_assembler.assembler().load(
        Width::Word64,
        roles.return_address,
        Memory {
            base: roles.stack_pointer,
            displacement: 0,
        },
    );
    macro_assembler.call_leaf(
        Leaf::Miss.index(),
        [
            Operand::Register(roles.frame),
            Operand::Register(roles.return_address),
            Operand::Immediate(0),
        ],
    );
    let answer = roles.leaf_result;
    macro_assembler.assembler().compare_immediate(answer, 0);
    macro_assembler.exit_if(Condition::Equal, ExitKind::PushRefused, 0);
    // No cell: after a miss the site's own entry need not be the decision
    // for this receiver, so the leaf stub, if that is what is entered,
    // decides the send rather than run the entry.
    macro_assembler
        .assembler()
        .move_immediate(roles.send_cells, 0);
    let machine_words = roles.idiom_scratch;
    macro_assembler.load_machine_words(machine_words);
    let receiver = MacroAssembler::machine_word(machine_words, MachineWord::CalleeReceiver);
    let class = MacroAssembler::machine_word(machine_words, MachineWord::CalleeClass);
    let assembler = macro_assembler.assembler();
    assembler.load(Width::Word64, roles.callee_receiver(), receiver);
    assembler.load(Width::Word64, roles.callee_class(), class);
    assembler.jump_register(answer);
}

/// The stub a send's cell names for a target machine code does not run: a
/// method with a fast path, a handler, a body the JIT left out or refused,
/// every target that is no method. It refuses the call, so that the
/// caller re-executes the send and the interpreter runs it.
pub(crate) fn assemble_refusing_stub(abi: Abi) -> Assembled {
    let mut macro_assembler = MacroAssembler::new(abi);
    macro_assembler.exit(ExitKind::PushRefused, 0);
    finish_stub(macro_assembler)
}

/// Running machine code on a fake frame, for the tests of the stubs and
/// the idioms: a zone holding both stubs, bodies installed after them.
#[cfg(test)]
pub(super) mod testing {
    use ns_assembler::Assembled;
    use ns_memory::{CodeOffset, CodeWriter, CodeZone, EntryBlock, Leaf, ENTRY_BLOCK_WORDS};

    use super::{assemble_entry_stub, assemble_exit_stub, decode_exit, BlockWord, MachineExit};
    use crate::heap::machine_words::{MachineWord, MachineWords};
    use crate::jit::macro_assembler::{Abi, MacroAssembler};

    /// Every piece of code starts at a multiple of this.
    const ALIGNMENT: u32 = 64;

    pub(crate) struct Machine {
        zone: CodeWriter,
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
            self.zone.write(offset, &assembled.bytes).expect("written");
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
            safepoint_requested: bool,
            context: &mut C,
            leaves: &[Leaf<C>],
        ) -> (MachineExit, [u64; ENTRY_BLOCK_WORDS]) {
            run_in(
                self.zone.zone(),
                [self.entry, self.exit],
                body,
                frame,
                register_base,
                safepoint_requested,
                context,
                leaves,
            )
        }
    }

    /// Runs `body` of `zone`, whose entry and exit stubs are `stubs`, on
    /// `frame`, its registers starting `register_base` words into it, and
    /// answers the exit and the block.
    #[expect(
        clippy::too_many_arguments,
        reason = "a test's one way to run code; each argument is a part of the fake frame"
    )]
    pub(crate) fn run_in<C>(
        zone: &CodeZone,
        stubs: [CodeOffset; 2],
        body: CodeOffset,
        frame: &mut [u64],
        register_base: u64,
        safepoint_requested: bool,
        context: &mut C,
        leaves: &[Leaf<C>],
    ) -> (MachineExit, [u64; ENTRY_BLOCK_WORDS]) {
        let [entry, exit] = stubs;
        let block = EntryBlock::new();
        let set = |word: BlockWord, value: u64| block.set(word.index(), value);
        set(
            BlockWord::StackBase,
            frame.as_mut_ptr().expose_provenance() as u64,
        );
        set(BlockWord::RegisterBase, register_base);
        set(
            BlockWord::MachineEntry,
            zone.executable_address(body) as u64,
        );
        set(BlockWord::ExitStub, zone.executable_address(exit) as u64);
        let machine_words = MachineWords::new();
        machine_words.set(
            MachineWord::SafepointRequested,
            u64::from(safepoint_requested),
        );
        set(BlockWord::MachineWords, machine_words.address() as u64);
        let kind = zone.enter(entry, &block, context, leaves);
        assert_eq!(kind, block.get(BlockWord::ExitKind.index()));
        let words = std::array::from_fn(|index| block.get(index));
        (decode_exit(&block, kind), words)
    }
}

/// These name x64's registers, so they are x64's whatever `cfg(ns_jit)`
/// says (`build.rs`).
#[cfg(all(test, target_arch = "x86_64"))]
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
                let (exit, block) = machine.run(body, &mut frame, 0, false, &mut (), &[]);
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
        let (exit, _) = machine.run(body, &mut frame, 0, false, &mut counter, &leaves);
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
        let (exit, _) = machine.run(body, &mut frame, 0, false, &mut recorded, &leaves);
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
        machine.run(body, &mut frame, 0, false, &mut (), &leaves);
        assert_eq!(frame[..4], [0, 0, 0, 0]);
    }

    #[test]
    fn a_value_in_the_free_callee_saved_register_survives_a_leaf_call() {
        // `r13` plays no role, so a leaf call keeps it, as the register
        // cache is to rely on: the call saves the stack pointer in the
        // block, not in a register.
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.load_immediate_word(R13, 0x1234_5678_9ABC);
            macro_assembler.assembler().push(RAX);
            macro_assembler.call_leaf(LeafIndex(0), [Operand::Immediate(0); 3]);
            macro_assembler.assembler().pop(RCX);
            macro_assembler.store_frame_register(0, R13);
            macro_assembler.exit(ExitKind::Reexecute, 0);
        });
        let mut frame = [0u64; 8];
        let leaves: [Leaf<()>; 1] = [stack_misalignment];
        machine.run(body, &mut frame, 0, false, &mut (), &leaves);
        assert_eq!(frame[0], 0x1234_5678_9ABC);
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
        let (exit, _) = machine.run(body, &mut frame, 0, true, &mut (), &[]);
        assert_eq!(exit, MachineExit::Safepoint { program_counter: 3 });
        assert_eq!(frame[0], 0, "nothing after the poll ran");
        let (exit, _) = machine.run(body, &mut frame, 0, false, &mut (), &[]);
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
        let (exit, _) = machine.run(body, &mut frame, 0, false, &mut (), &[]);
        assert_eq!(exit, MachineExit::Safepoint { program_counter: 3 });
        assert_eq!(frame[0], 0, "nothing after the poll ran");
    }
}

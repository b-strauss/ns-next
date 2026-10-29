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
    /// stubs the block cell or the global row they read, and 0 after the
    /// miss leaf. The leaf stub hands it to its leaf, which runs a slot
    /// accessor straight from the site's entry when it is a site's cell.
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
    /// The prologue's safepoint request word, then the end of the frame
    /// it pushes as a byte offset from the activation stack's base.
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

    /// The register the exit stub takes an address inside the exiting code
    /// in, which names the body an exit leaves (`jit::materialize`).
    pub(crate) fn exit_code_address(self) -> Register {
        RDX
    }

    /// The register a compiled call hands its callee's receiver in, as the
    /// word its header place 0 stores. The call writes nothing into the
    /// callee's frame: the callee's machine entry stores it there once its
    /// checks have passed (`jit::sends::prologue`), so that a refused call
    /// leaves the sender's registers -- a source of the send among them --
    /// as they were. A scratch register everywhere else.
    pub(crate) fn callee_receiver(self) -> Register {
        RSI
    }

    /// The register a compiled call hands its callee's current class in,
    /// as the word its header place 1 stores, under `callee_receiver`'s
    /// rule. A scratch register everywhere else.
    pub(crate) fn callee_class(self) -> Register {
        RDI
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
                (Operand::CodeAddress, Place::Register(target)) => {
                    self.assembler.load_code_address(target);
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
        self.assembler.load(Width::Word64, RSP, saved_stack_pointer);
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
        self.count(BlockWord::BackEdges);
    }

    /// Adds one to the entry block's `CompiledCalls` when the code counts
    /// (`set_count_back_edges`), and emits nothing otherwise. Clobbers the
    /// flags and the idiom scratch register.
    pub(crate) fn count_compiled_call(&mut self) {
        self.count(BlockWord::CompiledCalls);
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
        let scratch = self.roles.idiom_scratch;
        debug_assert!(
            target != scratch && word != scratch,
            "internal: the application id's idiom uses the idiom scratch register"
        );
        let machine_words = self.block_word(BlockWord::MachineWords);
        let assembler = &mut self.assembler;
        assembler.test_immediate(word, layout.small_integer_tag);
        assembler.jump_if(Condition::Equal, not_an_application);
        assembler.move_register(target, word);
        assembler.shift_right_arithmetic(target, layout.tag_bits);
        assembler.load(Width::Word64, scratch, machine_words);
        // Unsigned, so that a negative index is past the bound too.
        assembler.compare_memory(
            target,
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
        assembler.shift_left(target, WORD_SHIFT);
        assembler.add(target, scratch);
        assembler.load(
            Width::Word64,
            target,
            Memory {
                base: target,
                displacement: 0,
            },
        );
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

/// How far an index into the machine words is shifted to be a byte
/// offset: a word is eight bytes.
const WORD_SHIFT: u8 = 3;

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
        let mut machine = Machine::new();
        let body = machine.body(|macro_assembler| {
            macro_assembler.load_enclosing_object(RAX, 0, depth);
            macro_assembler.store_frame_register(15, RAX);
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
        let layout = crate::heap::Heap::machine_layout();
        let declaring = FakeClass::new();
        let mut class = FakeClass::new();
        class.set(layout.declaring_class_displacement, declaring.handle());
        let lexical_class = |frame: &mut [u64; 16], depth: u16| {
            let mut machine = Machine::new();
            let body = machine.body(|macro_assembler| {
                let missing = macro_assembler.assembler().label();
                macro_assembler.load_lexical_class(RAX, 0, depth, missing);
                macro_assembler.store_frame_register(15, RAX);
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
        let layout = crate::heap::Heap::machine_layout();
        let superclass = FakeClass::new();
        let without_superclass = FakeClass::new();
        let mut class = FakeClass::new();
        class.set(layout.superclass_displacement, superclass.handle());
        let load = |frame: &mut [u64; 16]| {
            let mut machine = Machine::new();
            let body = machine.body(|macro_assembler| {
                macro_assembler.load_superclass(RAX, 0, 7);
                macro_assembler.store_frame_register(15, RAX);
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
}

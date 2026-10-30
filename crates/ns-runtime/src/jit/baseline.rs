//! The baseline compiler: a body's instructions, one after another, each
//! lowered on its own through the one exhaustive `match` over
//! `Instruction` (`lower`), the extension point every instruction is
//! added through (`docs/design/execution.md`, "The foundation and the one
//! extension point"). A baseline frame's registers are the interpreter's
//! frame words, so each instruction's code starts and ends with every
//! value in its slot, and any instruction can hand the frame to the
//! interpreter at its own program counter.

use ns_assembler::{Condition, Label, Register};

use crate::builtins::primitive::{mirror_entry, MachineCall};
use crate::bytecode::{successors, CodeKind, CompiledCode, Instruction, IntegerOperation, Literal};
use crate::heap::{value_bits, Immediate};
use crate::interpreter::FeedbackBase;

use super::closures;
use super::leaves::Leaf;
use super::macro_assembler::{LeafIndex, MacroAssembler, ObjectSize, Operand};
use super::sends;
use super::stubs::ExitKind;
use super::{CallReturn, ProgramCounterMap, Refusal};

/// A compiled body: its machine code, and where each instruction's code
/// starts in it.
pub(crate) struct BaselineBody {
    pub(crate) code: ns_assembler::Assembled,
    pub(crate) map: ProgramCounterMap,
    /// Whether the code exits before it reaches a return, a compiled send
    /// or a backward jump: every path from the first instruction reaches an
    /// exit first, so the code runs at most a few straight-line
    /// instructions the interpreter would have run for less than the entry
    /// and the exit cost; such a body is neither installed nor entered
    /// (`jit::note_entry`).
    pub(crate) exits_at_once: bool,
    /// Whether the code reifies its frame (`reifies_its_frame`).
    pub(crate) reifies: bool,
    /// The offsets in the code of its first data word, past its last
    /// instruction, and of the byte past its last, if it has any
    /// (`MacroAssembler::take_body_words`): the alignment padding after
    /// them is no word's.
    pub(crate) words: Option<std::ops::Range<u32>>,
}

/// Whether `code` reifies its frame: whether it makes a closure, whose
/// `MakeClosure` reifies the frame the closure keeps, so that its prologue
/// writes the words the divorce at its return reads and its templates
/// keep its program counter where the interpreter stores it
/// (`MachineFrame::reifies`). No other instruction of a body reifies the
/// frame running it.
pub(crate) fn reifies_its_frame(code: &CompiledCode) -> bool {
    (0..code.instruction_count()).any(|program_counter| {
        matches!(
            code.instruction(program_counter),
            Instruction::MakeClosure { .. }
        )
    })
}

/// What a body runs as when a compiled call enters it, which its
/// prologue writes into its frame (`sends::prologue`): decided by whom it
/// was compiled for (`jit::compile`), since a body is one method's body,
/// one class's factory or one mixin's initializer for its life.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BodyRole {
    /// A method's body: its frame's kind and selector are its method's.
    Method,
    /// A factory's body (`compiler::factory_body`): its current class is
    /// the class object it runs on, its receiver.
    Factory,
    /// An initializer's body: its frame's kind is the body the initializer
    /// leaf decided, which the call hands over (`MachineWord::CalleeKind`).
    Initializer,
    /// A block's body: its receiver and its current class are those of the
    /// frame its closure captured, which the call hands over as a method's
    /// are, and the call hands the closure and whether the activation
    /// counts besides (`Roles::callee_closure`, `Roles::callee_counted`).
    Block,
    /// A superclass clause's body, which only the run loop's push enters,
    /// past its prologue.
    EnteredOnly,
}

impl BodyRole {
    /// The role of `code`, compiled as the body of `method` when it is
    /// one's.
    pub(crate) fn of(code: &CompiledCode, is_method: bool) -> BodyRole {
        match (is_method, code.kind) {
            (true, _) => BodyRole::Method,
            (false, CodeKind::Method) => BodyRole::Factory,
            (false, CodeKind::Initializer) => BodyRole::Initializer,
            (false, CodeKind::Block) => BodyRole::Block,
            (false, CodeKind::SuperclassClause) => BodyRole::EnteredOnly,
        }
    }
}

/// What `lower` did with one instruction, and what `compile` still emits
/// for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Lowering {
    /// The arm emitted the instruction's template itself.
    Template,
    /// The arm emitted the template of a return or of a compiled send: a
    /// template like `Template`, but one that does the work of a call, which
    /// pays for entering the body
    /// (`reaches_before_an_exit`).
    Transfer,
    /// A call of `leaf` with `arguments`, the instruction's own
    /// interpreter step, which answers 0 when it ran the instruction and
    /// anything else, having changed nothing, to have the interpreter run
    /// it instead.
    Leaf {
        leaf: LeafIndex,
        arguments: [Operand; 3],
    },
    /// An exit that hands the frame to the interpreter, which runs the
    /// instruction.
    Exit,
}

/// How far a word count is shifted to be a byte count.
const WORD_SHIFT: u8 = 3;

/// The sign of a 64-bit word spread over all of it by an arithmetic
/// shift: 0 for a word of zero or more, -1 for a negative one.
const SIGN_SHIFT: u8 = 63;

/// The body being compiled, as the arms of `lower` emit into it: the
/// macro assembler, every instruction's label -- all made before the first
/// instruction is emitted, so that a forward jump names its target -- and
/// the instruction being lowered.
pub(super) struct BodyAssembler {
    pub(super) macro_assembler: MacroAssembler,
    /// Each instruction's label, by program counter.
    starts: Vec<Label>,
    /// The body's argument count, which places its receiver in the frame.
    pub(super) argument_count: u16,
    /// The instruction being lowered.
    pub(super) program_counter: u32,
    /// Whether the body is a body-less declaration's own code
    /// (`CompiledCode::is_declaration_code`), whose lexical walk starts at
    /// its declaration's scope.
    pub(super) declaration_code: bool,
    /// The backward jumps taken out of a conditional, each a label bound
    /// after the last instruction and the target it polls before and
    /// jumps to, so that the branch not taken falls straight through.
    backward_branches: Vec<(Label, u32)>,
    /// Each call's return address, a label bound right after the call,
    /// and the program counter of the send it is the call of, in the order
    /// the calls are emitted (`call_send`).
    call_returns: Vec<(Label, u32)>,
    /// The feedback base the body's sends' cells are found from, embedded
    /// in the code: its machine code is entered only by an activation with
    /// this base (`JitState::interpreter_entry`).
    pub(super) feedback_base: FeedbackBase,
    /// The body's family's size, past which a site is uncached.
    pub(super) family_size: u32,
    /// One past the frame's last register: its window's end, above which
    /// `InitializeInstance`'s callee lies.
    pub(super) window_end: u16,
    /// What the body runs as when a compiled call enters it.
    pub(super) role: BodyRole,
    /// Whether the body reifies its frame (`reifies_its_frame`).
    pub(super) reifies: bool,
    /// How many registers a frame object of the body holds
    /// (`CompiledCode::frame_register_count`).
    pub(super) frame_register_count: usize,
}

/// The scratch registers a template uses, by role.
#[derive(Clone, Copy)]
struct TemplateRegisters {
    dividend: Register,
    remainder: Register,
    first: Register,
    second: Register,
    third: Register,
}

impl BodyAssembler {
    /// A jump to the instruction at `target`. A backward one closes a
    /// loop, so it polls first, as the interpreter's backward jump tests
    /// the safepoint, so that a loop that makes no send still reaches one;
    /// its poll exits at `target`, where the interpreter takes the
    /// safepoint (`vm::run_stretch`'s `jump_to!`).
    fn jump_to(&mut self, target: u32) {
        if target <= self.program_counter {
            self.macro_assembler.count_back_edge();
            self.macro_assembler.poll(target);
        }
        let label = self.starts[target as usize];
        self.macro_assembler.assembler().jump(label);
    }

    /// A jump to the instruction at `target` when `condition` holds, a
    /// backward one polling as `jump_to`'s does; the code falls through
    /// when it does not hold. A backward jump's poll lies after the last
    /// instruction (`emit_backward_branches`), so that the branch not
    /// taken costs what a forward one's does.
    fn jump_to_if(&mut self, condition: Condition, target: u32) {
        if target <= self.program_counter {
            let label = self.macro_assembler.assembler().label();
            self.macro_assembler.assembler().jump_if(condition, label);
            self.backward_branches.push((label, target));
        } else {
            let label = self.starts[target as usize];
            self.macro_assembler.assembler().jump_if(condition, label);
        }
    }

    /// The backward jumps `jump_to_if` set aside, each bound and taken.
    fn emit_backward_branches(&mut self) {
        for (label, target) in std::mem::take(&mut self.backward_branches) {
            self.macro_assembler.assembler().bind(label);
            self.macro_assembler.count_back_edge();
            self.macro_assembler.poll(target);
            let start = self.starts[target as usize];
            self.macro_assembler.assembler().jump(start);
        }
    }

    /// The scratch registers a template takes: the two a division fixes,
    /// then three others in the roles' order.
    fn registers(&self) -> TemplateRegisters {
        let roles = self.macro_assembler.roles();
        let mut others = roles
            .scratch
            .iter()
            .copied()
            .filter(|register| *register != roles.dividend && *register != roles.remainder);
        let mut next = || {
            others
                .next()
                .expect("internal: the roles leave a template three scratch registers")
        };
        let (first, second, third) = (next(), next(), next());
        TemplateRegisters {
            dividend: roles.dividend,
            remainder: roles.remainder,
            first,
            second,
            third,
        }
    }

    /// Loads the frame's registers `left.1` and `right.1` into `left.0`
    /// and `right.0`, jumping to `target` unless both hold a
    /// `SmallInteger`: `integer_into`'s decline of any other operand.
    fn load_small_integers(&mut self, left: (Register, u16), right: (Register, u16), target: u32) {
        let tag = small_integer_tag();
        self.macro_assembler.load_frame_register(left.0, left.1);
        self.macro_assembler.load_frame_register(right.0, right.1);
        self.macro_assembler.assembler().test_immediate(left.0, tag);
        self.jump_to_if(Condition::Equal, target);
        self.macro_assembler
            .assembler()
            .test_immediate(right.0, tag);
        self.jump_to_if(Condition::Equal, target);
    }

    /// `IntegerBinary`: `integer_into`'s `SmallInteger` fast path. Every
    /// case it declines jumps to `target` before the destination is
    /// written, so the send there runs on the operands as they were.
    fn integer_binary(
        &mut self,
        operation: IntegerOperation,
        destination: u16,
        left: u16,
        right: u16,
        target: u32,
    ) {
        let registers = self.registers();
        // The left operand sits where a division wants its dividend.
        let (left_value, right_value) = (registers.dividend, registers.first);
        self.load_small_integers((left_value, left), (right_value, right), target);
        let tag = small_integer_tag();
        let result = match operation {
            // `(a << 3 | 1) + ((b << 3 | 1) - 1)` is `(a + b) << 3 | 1`,
            // and overflows exactly when `a + b` leaves 61 bits:
            // `Value::small_integer_sum`. The difference likewise.
            IntegerOperation::Add | IntegerOperation::Subtract => {
                let untagged = registers.second;
                let assembler = self.macro_assembler.assembler();
                assembler.move_register(untagged, right_value);
                assembler.add_immediate(untagged, -tag);
                if operation == IntegerOperation::Add {
                    assembler.add(left_value, untagged);
                } else {
                    assembler.subtract(left_value, untagged);
                }
                self.jump_to_if(Condition::Overflow, target);
                left_value
            }
            // `a * (b << 3)` is `(a * b) << 3`, overflowing exactly when
            // `a * b` leaves 61 bits: `Value::small_integer_product`.
            IntegerOperation::Multiply => {
                let untagged = registers.second;
                let assembler = self.macro_assembler.assembler();
                assembler.shift_right_arithmetic(left_value, tag_bits());
                assembler.move_register(untagged, right_value);
                assembler.add_immediate(untagged, -tag);
                assembler.multiply_checked(left_value, untagged);
                self.jump_to_if(Condition::Overflow, target);
                self.macro_assembler
                    .assembler()
                    .or_immediate(left_value, tag);
                left_value
            }
            IntegerOperation::FloorDivide => self.floor_divide(registers, target),
            // Tagging keeps the order, so the words compare as the
            // integers do: `Value::small_integer_order`.
            IntegerOperation::Less => self.compare_into(registers, Condition::Less),
            IntegerOperation::Greater => self.compare_into(registers, Condition::Greater),
            IntegerOperation::LessOrEqual => self.compare_into(registers, Condition::LessOrEqual),
            IntegerOperation::GreaterOrEqual => {
                self.compare_into(registers, Condition::GreaterOrEqual)
            }
            IntegerOperation::Equal => self.compare_into(registers, Condition::Equal),
        };
        self.macro_assembler
            .store_frame_register(destination, result);
    }

    /// The Boolean of comparing the `SmallInteger`s in
    /// `registers.dividend` and `registers.first` by `condition`, in the
    /// register it answers.
    fn compare_into(&mut self, registers: TemplateRegisters, condition: Condition) -> Register {
        let (answer, truth) = (registers.second, registers.third);
        let assembler = self.macro_assembler.assembler();
        assembler.move_immediate(answer, Immediate::boolean(false).bits());
        assembler.move_immediate(truth, Immediate::boolean(true).bits());
        assembler.compare(registers.dividend, registers.first);
        assembler.move_if(condition, answer, truth);
        answer
    }

    /// The floor quotient of the `SmallInteger`s in `registers.dividend`
    /// and `registers.first`, tagged, in the register it answers:
    /// `Value::small_integer_floor_quotient`, declining to `target` for a
    /// zero divisor and for the one quotient that leaves 61 bits, the
    /// smallest `SmallInteger` over -1.
    fn floor_divide(&mut self, registers: TemplateRegisters, target: u32) -> Register {
        let (dividend, divisor) = (registers.dividend, registers.first);
        let assembler = self.macro_assembler.assembler();
        assembler.shift_right_arithmetic(dividend, tag_bits());
        assembler.shift_right_arithmetic(divisor, tag_bits());
        assembler.compare_immediate(divisor, 0);
        self.jump_to_if(Condition::Equal, target);
        // Both are 61-bit integers, so the division cannot fault.
        let assembler = self.macro_assembler.assembler();
        assembler.divide_signed(dividend, registers.remainder, divisor);
        // `checked_floor_divide`: no remainder, or one of the divisor's
        // sign, leaves the truncated quotient; any other rounds it down.
        let (remainder_sign, divisor_sign) = (registers.second, registers.third);
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
        let (tagged, untagged) = (registers.second, registers.third);
        assembler.move_register(tagged, dividend);
        assembler.shift_left(tagged, tag_bits());
        assembler.move_register(untagged, tagged);
        assembler.shift_right_arithmetic(untagged, tag_bits());
        assembler.compare(untagged, dividend);
        self.jump_to_if(Condition::NotEqual, target);
        self.macro_assembler
            .assembler()
            .or_immediate(tagged, small_integer_tag());
        tagged
    }

    /// `BranchIfFalse` and `BranchIfTrue`: to `target` when the frame's
    /// `register` holds `boolean`, on otherwise -- a non-Boolean included,
    /// which the instruction before has sent elsewhere
    /// (`vm::run_stretch`'s arms over `boolean_in`).
    fn branch_if(&mut self, register: u16, boolean: bool, target: u32) {
        let value = self.registers().first;
        self.macro_assembler.load_frame_register(value, register);
        self.macro_assembler
            .assembler()
            .compare_immediate(value, boolean_word(boolean));
        self.jump_to_if(Condition::Equal, target);
    }

    /// `JumpIfNotBoolean`: on when the frame's `register` holds either
    /// Boolean, to `target` otherwise, where the non-Boolean is
    /// signalled.
    fn jump_if_not_boolean(&mut self, register: u16, target: u32) {
        let value = self.registers().first;
        self.macro_assembler.load_frame_register(value, register);
        let assembler = self.macro_assembler.assembler();
        let boolean = assembler.label();
        assembler.compare_immediate(value, boolean_word(true));
        assembler.jump_if(Condition::Equal, boolean);
        assembler.compare_immediate(value, boolean_word(false));
        self.jump_to_if(Condition::NotEqual, target);
        self.macro_assembler.assembler().bind(boolean);
    }

    /// The frame's `destination :=` its `source`: `copy_register`.
    fn copy(&mut self, destination: u16, source: u16) {
        let value = self.registers().first;
        self.macro_assembler.load_frame_register(value, source);
        self.macro_assembler
            .store_frame_register(destination, value);
    }

    /// The frame's `destination :=` `immediate`: `load_literal_into` for a
    /// literal whose value is the word itself.
    fn load_immediate(&mut self, destination: u16, immediate: Immediate) {
        let value = self.registers().first;
        self.macro_assembler
            .load_immediate_word(value, immediate.bits());
        self.macro_assembler
            .store_frame_register(destination, value);
    }

    /// The frame's `destination :=` its receiver: `load_self_into`.
    fn load_self(&mut self, destination: u16) {
        let value = self.registers().first;
        self.macro_assembler
            .load_receiver(value, self.argument_count);
        self.macro_assembler
            .store_frame_register(destination, value);
    }

    /// `MakeVector`: `make_vector_into`'s `Array` of `size` nils, bumped
    /// inline in eden or, past the bump's limit, placed by the allocation
    /// leaf as the interpreter's cold path places it, then filled, its
    /// word into the frame's `destination`. Nothing declines or exits, so
    /// the store comes last only by the order of the work.
    fn make_vector(&mut self, destination: u16, size: u16) {
        let registers = self.registers();
        let (object, header, class_word) = (registers.first, registers.second, registers.third);
        let word_count = usize::from(size) + 1;
        let macro_assembler = &mut self.macro_assembler;
        let (slow, filled) = (
            macro_assembler.assembler().label(),
            macro_assembler.assembler().label(),
        );
        macro_assembler.load_array_header_and_class(header, class_word, word_count);
        macro_assembler.bump_allocate(
            object,
            ObjectSize::Constant(word_count),
            header,
            class_word,
            slow,
        );
        macro_assembler.assembler().bind(filled);
        macro_assembler.nil_fill(object, 0, usize::from(size));
        macro_assembler.store_frame_register(destination, object);
        macro_assembler.call_leaf_out_of_line(
            slow,
            Leaf::Allocate.index(),
            [
                Operand::Immediate(word_count as u64),
                Operand::Register(header),
                Operand::Register(class_word),
            ],
            object,
            filled,
        );
    }

    /// `MakeArray`: `make_array_into`'s `Array` of the frame's
    /// `element_registers`, in order, allocated as `make_vector`'s, its
    /// word into the frame's `destination`. The elements are stored with
    /// no barrier, as `Heap::allocate_array_from_registers` stores them:
    /// the object is new, young, or old and born remembered. Nothing
    /// declines or exits.
    fn make_array(&mut self, destination: u16, element_registers: &[u16]) {
        let registers = self.registers();
        let (object, header, class_word) = (registers.first, registers.second, registers.third);
        let (address, element) = (registers.dividend, registers.remainder);
        let word_count = element_registers.len() + 1;
        let fields = crate::heap::Heap::machine_layout().fields_displacement;
        let macro_assembler = &mut self.macro_assembler;
        let (slow, placed) = (
            macro_assembler.assembler().label(),
            macro_assembler.assembler().label(),
        );
        macro_assembler.load_array_header_and_class(header, class_word, word_count);
        macro_assembler.bump_allocate(
            object,
            ObjectSize::Constant(word_count),
            header,
            class_word,
            slow,
        );
        macro_assembler.assembler().bind(placed);
        if !element_registers.is_empty() {
            let object_space = macro_assembler.roles().object_space;
            macro_assembler.assembler().move_register(address, object);
            macro_assembler.assembler().add(address, object_space);
        }
        for (position, register) in element_registers.iter().enumerate() {
            let displacement = i32::try_from(position)
                .ok()
                .and_then(|position| position.checked_mul(8))
                .and_then(|bytes| bytes.checked_add(fields))
                .expect("internal: an array literal's elements lie within 2 GiB of its start");
            macro_assembler.load_frame_register(element, *register);
            macro_assembler.assembler().store(
                ns_assembler::Width::Word64,
                ns_assembler::Memory {
                    base: address,
                    displacement,
                },
                element,
            );
        }
        macro_assembler.store_frame_register(destination, object);
        macro_assembler.call_leaf_out_of_line(
            slow,
            Leaf::Allocate.index(),
            [
                Operand::Immediate(word_count as u64),
                Operand::Register(header),
                Operand::Register(class_word),
            ],
            object,
            placed,
        );
    }

    /// `AllocateInstance`: `allocate_instance_into`'s fresh instance of the
    /// class object the factory runs on, its receiver, bumped inline in
    /// eden and every field nil, its word into the frame's `destination`,
    /// where `allocate_for_factory` would make it with
    /// `Heap::allocate_instance_with`: the receiver a class object whose
    /// builtin field and value class refusal are nil, read afresh at every
    /// instantiation, and whose application's row records a header word
    /// (`MacroAssembler::load_instance_header`). Everything else -- a
    /// builtin's own instances, a refusal, an overflowing count, a bump
    /// past eden's limit -- is the generic leaf's, which runs the
    /// interpreter's `allocate_instance` and, for a refusal, exits with
    /// nothing written, so that the interpreter raises the error. The
    /// VM mirror's class is a builtin, so it never takes the inline path.
    fn allocate_instance(&mut self, destination: u16) -> Lowering {
        let layout = crate::heap::Heap::machine_layout();
        // The header's count is below the overflow mark, so the block lies
        // below the large-object threshold, which the inline bump of a
        // size in a register requires.
        let largest = usize::try_from(layout.word_count_overflow - 1)
            .expect("internal: the overflow mark is a small count");
        assert!(
            crate::heap::Heap::machine_block(largest).inline,
            "internal: an instance whose header holds its count is bumped inline"
        );
        let registers = self.registers();
        let (object, header, class_word) = (registers.first, registers.second, registers.third);
        let (bytes, end) = (registers.dividend, registers.remainder);
        let program_counter = self.program_counter;
        let argument_count = self.argument_count;
        let frame = self.macro_assembler.roles().frame;
        let macro_assembler = &mut self.macro_assembler;
        let (slow, done) = (
            macro_assembler.assembler().label(),
            macro_assembler.assembler().label(),
        );
        macro_assembler.load_receiver(class_word, argument_count);
        macro_assembler.jump_unless_object(class_word, slow);
        macro_assembler.load_instance_header(class_word, header, slow);
        // The block's bytes: the header's count of the words after it, and
        // the header itself.
        let assembler = macro_assembler.assembler();
        assembler.move_register(bytes, header);
        assembler.shift_right_logical(bytes, layout.word_count_shift);
        assembler.add_immediate(bytes, 1);
        assembler.shift_left(bytes, WORD_SHIFT);
        macro_assembler.bump_allocate(
            object,
            ObjectSize::Register(bytes),
            header,
            class_word,
            slow,
        );
        macro_assembler.nil_fill_block(object, bytes, end);
        macro_assembler.store_frame_register(destination, object);
        macro_assembler.assembler().bind(done);
        macro_assembler.call_leaf_out_of_line_or_exit(
            slow,
            Leaf::Step.index(),
            [
                Operand::Register(frame),
                Operand::Immediate(u64::from(program_counter)),
                Operand::CodeAddress,
            ],
            program_counter,
            done,
        );
        Lowering::Template
    }

    /// `LoadEnclosingObject`: the frame's `destination :=` §3.3's
    /// enclosing object at lexical `depth`
    /// (`MacroAssembler::load_enclosing_object`), as
    /// `load_enclosing_object_into` finds it. A body-less declaration's own
    /// code walks out from its declaration's scope, which the generic leaf
    /// runs (`interpreter::sends::enclosing_object`).
    fn load_enclosing_object(&mut self, destination: u16, depth: u16) -> Lowering {
        if self.declaration_code {
            return self.step();
        }
        let value = self.registers().first;
        let argument_count = self.argument_count;
        self.macro_assembler
            .load_enclosing_object(value, argument_count, depth);
        self.macro_assembler
            .store_frame_register(destination, value);
        Lowering::Template
    }

    /// `LoadVector`: `load_vector_into`, the frame's `destination :=`
    /// element `index` of the indirection vector in its `vector`. A
    /// register that holds no `Array` with that element exits before
    /// anything is written, and the interpreter raises the error
    /// (`interpreter::locals::load_vector`).
    fn load_vector(&mut self, destination: u16, vector: u16, index: u16) {
        let registers = self.registers();
        let (object, element) = (registers.first, registers.second);
        let displacement = element_displacement(index);
        let program_counter = self.program_counter;
        let macro_assembler = &mut self.macro_assembler;
        let failed = macro_assembler.assembler().label();
        macro_assembler.load_frame_register(object, vector);
        macro_assembler.check_array_index(object, index, element, failed);
        let object_space = macro_assembler.roles().object_space;
        let assembler = macro_assembler.assembler();
        assembler.add(object, object_space);
        assembler.load(
            ns_assembler::Width::Word64,
            element,
            ns_assembler::Memory {
                base: object,
                displacement,
            },
        );
        macro_assembler.store_frame_register(destination, element);
        macro_assembler.exit_at(failed, ExitKind::Reexecute, program_counter);
    }

    /// `StoreVector`: `store_vector`, element `index` of the indirection
    /// vector in the frame's `vector` `:=` its `source`, through the write
    /// barrier, since the vector may be old and the value young. A
    /// register that holds no `Array` with that element exits before
    /// anything is written -- the source is read only after the check, as
    /// the interpreter reads it -- and the interpreter raises the error.
    fn store_vector(&mut self, vector: u16, index: u16, source: u16) {
        let registers = self.registers();
        let (object, value, scratch) = (registers.first, registers.second, registers.third);
        let displacement = element_displacement(index);
        let program_counter = self.program_counter;
        let macro_assembler = &mut self.macro_assembler;
        let failed = macro_assembler.assembler().label();
        macro_assembler.load_frame_register(object, vector);
        macro_assembler.check_array_index(object, index, scratch, failed);
        macro_assembler.load_frame_register(value, source);
        macro_assembler.store_with_barrier(object, displacement, value, scratch);
        macro_assembler.exit_at(failed, ExitKind::Reexecute, program_counter);
    }

    /// The start of every own-slot instruction's template -- `InitializeSlot`,
    /// `InitializeSlotProvisionally`, `LoadOwnSlot` and
    /// `ReturnSlotIfFilled` -- which address `own_index`, mixin-relative,
    /// in the receiver with no send: `interpreter::locals::check_own_slots`,
    /// then the field's address (`MacroAssembler::load_own_slot_address`).
    /// The hot path compares the receiver's guard with the frame's
    /// (`MacroAssembler::check_own_slot_guard`); any other receiver calls
    /// `Leaf::OwnSlots` out of line, which runs the cold check and records
    /// the receiver's guard, after which the compare is made again and
    /// holds, or, having changed nothing, exits so that the interpreter
    /// raises the error. Answers the receiver's and the address's
    /// registers and the label a field the receiver lacks jumps to, which
    /// the caller binds to an exit that re-executes the instruction, and
    /// `registers.second` is free again; `None`, nothing emitted, for an
    /// index no displacement holds, which only a crafted image's code has.
    fn own_slot(
        &mut self,
        own_index: u32,
        registers: TemplateRegisters,
    ) -> Option<(Register, Register, Label)> {
        let own_index = i32::try_from(own_index).ok()?;
        let (receiver, spare, address) = (registers.first, registers.second, registers.third);
        let program_counter = self.program_counter;
        let argument_count = self.argument_count;
        let frame = self.macro_assembler.roles().frame;
        let macro_assembler = &mut self.macro_assembler;
        let assembler = macro_assembler.assembler();
        let (start, cold, failed) = (assembler.label(), assembler.label(), assembler.label());
        assembler.bind(start);
        macro_assembler.check_own_slot_guard(argument_count, receiver, spare, cold);
        macro_assembler.load_own_slot_address(
            argument_count,
            own_index,
            receiver,
            address,
            spare,
            failed,
        );
        macro_assembler.call_leaf_out_of_line_or_exit(
            cold,
            Leaf::OwnSlots.index(),
            [
                Operand::Register(frame),
                Operand::Immediate(u64::from(program_counter)),
                Operand::CodeAddress,
            ],
            program_counter,
            start,
        );
        Some((receiver, address, failed))
    }

    /// `InitializeSlot` and `InitializeSlotProvisionally`: the frame's
    /// `source` stored into the receiver's own slot `own_index`
    /// (`vm::initialize_slot`, `vm::initialize_slot_provisionally`),
    /// through the write barrier, since the receiver may be old and the
    /// value young. `InitializeSlot` first asks §3.1.1's check of a
    /// value's slot (`value_check`), which only an object stored into a
    /// slot of a value class's instance can fail (`check_value_slot`):
    /// such a store is the generic leaf's, which runs the interpreter's own
    /// `initialize_slot` and, where that refuses -- a value that is no
    /// value, or a future, which the interpreter forces first -- exits,
    /// nothing written, so that the interpreter takes its error or future
    /// path (`vm::initialize_slot_refused`). A future stored into any
    /// other slot is stored as it is, as the interpreter stores it.
    fn initialize_slot(&mut self, source: u16, own_index: u32, value_check: bool) -> Lowering {
        let registers = self.registers();
        let Some((receiver, address, failed)) = self.own_slot(own_index, registers) else {
            return Lowering::Exit;
        };
        let (value, scratch) = (registers.dividend, registers.second);
        let program_counter = self.program_counter;
        let frame = self.macro_assembler.roles().frame;
        let layout = crate::heap::Heap::machine_layout();
        let macro_assembler = &mut self.macro_assembler;
        let (store, done, checked) = (
            macro_assembler.assembler().label(),
            macro_assembler.assembler().label(),
            macro_assembler.assembler().label(),
        );
        macro_assembler.load_frame_register(value, source);
        if value_check {
            let assembler = macro_assembler.assembler();
            assembler.test_immediate(value, layout.tag_mask);
            assembler.jump_if(Condition::NotEqual, store);
            macro_assembler.jump_if_value_slots(receiver, scratch, checked);
        }
        macro_assembler.assembler().bind(store);
        macro_assembler.store_with_barrier_at(
            receiver,
            ns_assembler::Memory {
                base: address,
                displacement: layout.fields_displacement,
            },
            value,
            scratch,
        );
        macro_assembler.assembler().bind(done);
        macro_assembler.exit_at(failed, ExitKind::Reexecute, program_counter);
        if value_check {
            macro_assembler.call_leaf_out_of_line_or_exit(
                checked,
                Leaf::Step.index(),
                [
                    Operand::Register(frame),
                    Operand::Immediate(u64::from(program_counter)),
                    Operand::CodeAddress,
                ],
                program_counter,
                done,
            );
        }
        Lowering::Template
    }

    /// `LoadOwnSlot`: the frame's `destination :=` the receiver's own slot
    /// `own_index` (`interpreter::locals::load_own_slot`).
    fn load_own_slot(&mut self, destination: u16, own_index: u32) -> Lowering {
        let registers = self.registers();
        let Some((_, address, failed)) = self.own_slot(own_index, registers) else {
            return Lowering::Exit;
        };
        let value = registers.second;
        let program_counter = self.program_counter;
        let fields = crate::heap::Heap::machine_layout().fields_displacement;
        let macro_assembler = &mut self.macro_assembler;
        macro_assembler.assembler().load(
            ns_assembler::Width::Word64,
            value,
            ns_assembler::Memory {
                base: address,
                displacement: fields,
            },
        );
        macro_assembler.store_frame_register(destination, value);
        macro_assembler.exit_at(failed, ExitKind::Reexecute, program_counter);
        Lowering::Template
    }

    /// `ReturnSlotIfFilled`: a lazy slot's getter returns the receiver's
    /// own slot `own_index` as `Return` returns a value once the slot is
    /// filled, anything but `nil` (`interpreter::locals::filled_own_slot`),
    /// and goes on to compute it otherwise. It is the getter's first
    /// instruction, before anything could reify the frame, so its return
    /// divorces nothing (`sends::return_from`).
    fn return_slot_if_filled(&mut self, own_index: u32) -> Lowering {
        let registers = self.registers();
        let Some((_, address, failed)) = self.own_slot(own_index, registers) else {
            return Lowering::Exit;
        };
        let program_counter = self.program_counter;
        let layout = crate::heap::Heap::machine_layout();
        let macro_assembler = &mut self.macro_assembler;
        let value = macro_assembler.roles().leaf_result;
        let assembler = macro_assembler.assembler();
        let unfilled = assembler.label();
        assembler.load(
            ns_assembler::Width::Word64,
            value,
            ns_assembler::Memory {
                base: address,
                displacement: layout.fields_displacement,
            },
        );
        assembler.compare_immediate(value, layout.nil_word);
        assembler.jump_if(Condition::Equal, unfilled);
        sends::return_value(macro_assembler);
        macro_assembler.assembler().bind(unfilled);
        macro_assembler.exit_at(failed, ExitKind::Reexecute, program_counter);
        Lowering::Transfer
    }

    /// A send's call of the code at the address in `target`, recorded so
    /// that its return address -- the address right after the `call`,
    /// which the processor pushes and the exit's walk over the native
    /// stack finds -- maps back to the send being lowered
    /// (`ProgramCounterMap::program_counter_of_return`). Every call a
    /// send's template emits goes through here, so none is left unmapped.
    pub(super) fn call_send(&mut self, target: Register) {
        let return_address = self.macro_assembler.call_returning(target);
        self.call_returns
            .push((return_address, self.program_counter));
    }

    /// `SendMirror` of a leaf-safe row: the mirror leaf's call, which runs
    /// the send as `vm::send_mirror` does and writes its destination itself
    /// (`leaves::mirror`), then on at the next instruction when it ran the
    /// send, at `failure` when the row's leaf declined, and an exit that
    /// re-executes the send, nothing written, for anything else.
    fn send_mirror(&mut self, failure: u32) {
        let program_counter = self.program_counter;
        let frame = self.macro_assembler.roles().frame;
        self.macro_assembler.call_leaf(
            Leaf::Mirror.index(),
            [
                Operand::Register(frame),
                Operand::Immediate(u64::from(program_counter)),
                Operand::CodeAddress,
            ],
        );
        let answer = self.macro_assembler.roles().leaf_result;
        // The program counter, which the interpreter stores before the
        // row's leaf decides, is stored on both paths that go on, after the
        // exit: no leaf-safe row reads it.
        self.macro_assembler
            .assembler()
            .compare_immediate(answer, MIRROR_FAILED);
        let declined = if self.reifies {
            let declined = self.macro_assembler.assembler().label();
            self.macro_assembler
                .assembler()
                .jump_if(Condition::Equal, declined);
            Some(declined)
        } else {
            self.jump_to_if(Condition::Equal, failure);
            None
        };
        self.macro_assembler
            .assembler()
            .compare_immediate(answer, 0);
        self.macro_assembler
            .exit_if(Condition::NotEqual, ExitKind::Reexecute, program_counter);
        if let Some(declined) = declined {
            sends::store_program_counter(self);
            let ran = self.macro_assembler.assembler().label();
            self.macro_assembler.assembler().jump(ran);
            self.macro_assembler.assembler().bind(declined);
            sends::store_program_counter(self);
            self.jump_to(failure);
            self.macro_assembler.assembler().bind(ran);
        }
    }

    /// The call of the generic leaf, which runs the instruction being
    /// lowered through the interpreter's own effect function in the frame
    /// whose register 0 the frame register points at, whichever frame of
    /// the stretch that is, and whose body the code address names.
    fn step(&self) -> Lowering {
        Lowering::Leaf {
            leaf: Leaf::Step.index(),
            arguments: [
                Operand::Register(self.macro_assembler.roles().frame),
                Operand::Immediate(u64::from(self.program_counter)),
                Operand::CodeAddress,
            ],
        }
    }
}

/// The mirror leaf's answer for a row's decline (`leaves::FAILED`), as an
/// immediate operand.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the answer is a small constant"
)]
const MIRROR_FAILED: i32 = super::leaves::FAILED as i32;

/// The displacement of an `Array`'s element `index` from its offset.
fn element_displacement(index: u16) -> i32 {
    crate::heap::Heap::machine_layout().fields_displacement + i32::from(index) * 8
}

/// The `SmallInteger` tag, as an immediate operand.
fn small_integer_tag() -> i32 {
    i32::try_from(value_bits::SMALL_INTEGER_TAG).expect("internal: the tag is a low bit")
}

/// How far a `SmallInteger`'s payload lies above its tag, as a shift
/// count.
fn tag_bits() -> u8 {
    u8::try_from(value_bits::TAG_BITS).expect("internal: the tag takes a few bits")
}

/// The word of `true` or `false`, as an immediate operand.
fn boolean_word(boolean: bool) -> i32 {
    i32::try_from(Immediate::boolean(boolean).bits())
        .expect("internal: a Boolean's word is a small constant")
}

/// The word a literal is when `make_value` makes it without allocating:
/// `nil`, a Boolean, a `SmallInteger` and an immediate float. `None` for
/// every other literal.
fn immediate_literal(literal: &Literal) -> Option<Immediate> {
    match literal {
        Literal::Nil => Some(Immediate::NIL),
        Literal::Boolean(boolean) => Some(Immediate::boolean(*boolean)),
        Literal::SmallInteger(integer) => Immediate::small_integer(*integer),
        Literal::Float(float) => Immediate::small_float(*float),
        Literal::String(_)
        | Literal::Symbol(_)
        | Literal::LargeInteger(_)
        | Literal::UnresolvedIdentifier(_)
        | Literal::BodyTooLarge(_) => None,
    }
}

/// Whether the generic leaf may load `literal`, one `make_value`
/// allocates and that cannot fail. A literal that raises an error, and a
/// `SmallInteger` literal outside 61 bits, which no lowering makes and
/// `Value::small_integer` refuses with a panic a leaf could not unwind
/// from, are left to the interpreter.
fn allocated_literal(literal: &Literal) -> bool {
    match literal {
        Literal::String(_) | Literal::Symbol(_) | Literal::LargeInteger(_) | Literal::Float(_) => {
            true
        }
        Literal::Nil
        | Literal::Boolean(_)
        | Literal::SmallInteger(_)
        | Literal::UnresolvedIdentifier(_)
        | Literal::BodyTooLarge(_) => false,
    }
}

/// Compiles `code` with `macro_assembler`, its sends' cells found from
/// `feedback_base`: the prologue a compiled call enters at, then each
/// instruction in turn, at a label whose offset the program-counter map
/// records, and after the code the body's data words, its `MakeClosure`s'
/// sites (`MacroAssembler::take_body_words`).
pub(crate) fn compile(
    code: &CompiledCode,
    mut macro_assembler: MacroAssembler,
    feedback_base: FeedbackBase,
    role: BodyRole,
) -> Result<BaselineBody, Refusal> {
    // A program counter of the map, and an exit's, is a `u32`; the bound
    // is the zone's layout's, which a later stage's tables index by
    // `u16`.
    let count =
        u16::try_from(code.instruction_count()).map_err(|_| Refusal::TooManyInstructions)?;
    let starts = (0..count)
        .map(|_| macro_assembler.assembler().label())
        .collect();
    let mut body = BodyAssembler {
        macro_assembler,
        starts,
        argument_count: code.argument_count,
        program_counter: 0,
        declaration_code: code.is_declaration_code(),
        backward_branches: Vec::new(),
        call_returns: Vec::new(),
        feedback_base,
        family_size: code.family_size,
        window_end: sends::window_end(code),
        role,
        reifies: reifies_its_frame(code),
        frame_register_count: code.frame_register_count(),
    };
    sends::prologue(&mut body, code);
    let mut exits = Vec::with_capacity(usize::from(count));
    let mut transfers = Vec::with_capacity(usize::from(count));
    for program_counter in 0..u32::from(count) {
        let start = body.starts[program_counter as usize];
        body.macro_assembler.assembler().bind(start);
        body.program_counter = program_counter;
        body.macro_assembler.set_program_counter(program_counter);
        let instruction = code.instruction(program_counter as usize);
        // What every lowering ends with, not a second dispatch on the
        // instruction: the instruction was decided by `lower`'s arm.
        let lowering = lower(&instruction, &mut body);
        exits.push(lowering == Lowering::Exit);
        transfers.push(lowering == Lowering::Transfer);
        match lowering {
            Lowering::Template | Lowering::Transfer => {}
            Lowering::Leaf { leaf, arguments } => {
                let macro_assembler = &mut body.macro_assembler;
                macro_assembler.call_leaf(leaf, arguments);
                let answer = macro_assembler.roles().leaf_result;
                macro_assembler.assembler().compare_immediate(answer, 0);
                macro_assembler.exit_if(Condition::NotEqual, ExitKind::Reexecute, program_counter);
            }
            Lowering::Exit => {
                body.macro_assembler
                    .exit(ExitKind::Reexecute, program_counter);
            }
        }
    }
    let exits_at_once = !reaches_before_an_exit(code, &exits, &transfers);
    body.emit_backward_branches();
    let words = body.macro_assembler.body_words_label();
    let word_bytes =
        u32::try_from(body.macro_assembler.body_word_count() * 8).map_err(|_| Refusal::ZoneFull)?;
    let assembled = body
        .macro_assembler
        .finish()
        .map_err(|_| Refusal::Unassembled)?;
    let words = words
        .and_then(|label| assembled.label_offsets[label.index()])
        .map(|start| start..start + word_bytes);
    let map = program_counter_map(&assembled, &body.starts, &body.call_returns);
    Ok(BaselineBody {
        code: assembled,
        map,
        exits_at_once,
        reifies: body.reifies,
        words,
    })
}

/// The map of a body laid out as `assembled`: each instruction's start,
/// the label of `starts` by program counter, and each call's return
/// address, a label of `call_returns` with its send's program counter.
fn program_counter_map(
    assembled: &ns_assembler::Assembled,
    starts: &[Label],
    call_returns: &[(Label, u32)],
) -> ProgramCounterMap {
    let offset_of = |label: Label| {
        assembled.label_offsets[label.index()].expect("internal: every recorded label is bound")
    };
    ProgramCounterMap::new(
        starts.iter().map(|label| offset_of(*label)).collect(),
        call_returns
            .iter()
            .map(|(label, program_counter)| CallReturn {
                machine_offset: offset_of(*label),
                program_counter: *program_counter,
            })
            .collect(),
    )
}

/// Whether the code of `code`, whose instructions at the program counters
/// `exits` marks lower to exits and `transfers` marks lower to a return or
/// a compiled send, can reach a transfer or take a backward jump before
/// its first exit: whether some instruction reachable from the first
/// through templates and leaves is a transfer or jumps back. Only such
/// code does work in machine code that pays for an entry; a body that
/// exits after a few straight-line instructions costs an entry and an
/// exit on every push. A leaf that declines exits, but is counted as
/// falling through, since it declines only what the interpreter must run.
fn reaches_before_an_exit(code: &CompiledCode, exits: &[bool], transfers: &[bool]) -> bool {
    let mut reached = vec![false; exits.len()];
    let mut pending = Vec::new();
    if let Some(first) = reached.first_mut() {
        *first = true;
        pending.push(0_u32);
    }
    while let Some(program_counter) = pending.pop() {
        if exits[program_counter as usize] {
            continue;
        }
        if transfers[program_counter as usize] {
            return true;
        }
        let instruction = code.instruction(program_counter as usize);
        for successor in successors(program_counter, &instruction) {
            if successor <= program_counter {
                return true;
            }
            if let Some(seen) = reached.get_mut(successor as usize) {
                if !*seen {
                    *seen = true;
                    pending.push(successor);
                }
            }
        }
    }
    false
}

/// The lowering of `instruction`. One of three, by what the instruction
/// may do (`docs/design/execution.md`, "The foundation and the one
/// extension point"):
///
/// 1. **A template** -- machine code written against the macro
///    assembler's idioms and the register cache's operand interface,
///    never against an encoder -- for an instruction whose work machine
///    code does itself.
/// 2. **A leaf call** -- the instruction's own interpreter step, a Rust
///    function shared with `vm::run`, called through the leaf stub with
///    the frame's registers flushed -- only for an instruction that runs
///    no Newspeak code, cannot collect and reads no other activation.
/// 3. **An exit that re-executes it** -- the frame handed to the
///    interpreter at that instruction -- for every other one. Always
///    correct, so every instruction has a lowering from the day it is
///    added.
///
/// The match has an arm per instruction and no `_` arm, so a new
/// instruction cannot compile until it chooses one, and it is the only
/// dispatch on the instruction: a template's arm emits the template
/// through `body` and answers `Lowering::Template`, a leaf's answers the
/// leaf and its operands, an exit's emits nothing.
///
/// A template leaves the frame as the interpreter's arm does, and writes
/// it only once nothing can decline or exit any more: an exit or a
/// decline hands the interpreter an instruction no part of which ran, so
/// it never applies the instruction's effect twice.
fn lower(instruction: &Instruction, body: &mut BodyAssembler) -> Lowering {
    match instruction {
        Instruction::LoadLiteral {
            destination,
            literal,
        } => match immediate_literal(literal) {
            Some(immediate) => {
                body.load_immediate(*destination, immediate);
                Lowering::Template
            }
            None if allocated_literal(literal) => body.step(),
            None => Lowering::Exit,
        },
        Instruction::LoadSelf { destination } => {
            body.load_self(*destination);
            Lowering::Template
        }
        Instruction::Move {
            destination,
            source,
        }
        | Instruction::StoreLocal {
            source,
            slot: destination,
        } => {
            body.copy(*destination, *source);
            Lowering::Template
        }
        Instruction::MakeVector { destination, size } => {
            body.make_vector(*destination, *size);
            Lowering::Template
        }
        Instruction::MakeArray {
            destination,
            element_registers,
        } => {
            body.make_array(*destination, element_registers);
            Lowering::Template
        }
        Instruction::LoadVector {
            destination,
            vector,
            index,
        } => {
            body.load_vector(*destination, *vector, *index);
            Lowering::Template
        }
        Instruction::StoreVector {
            vector,
            index,
            source,
        } => {
            body.store_vector(*vector, *index, *source);
            Lowering::Template
        }
        Instruction::Jump { target } => {
            body.jump_to(*target);
            Lowering::Template
        }
        Instruction::JumpIfNotBoolean { register, target } => {
            body.jump_if_not_boolean(*register, *target);
            Lowering::Template
        }
        Instruction::BranchIfFalse { register, target } => {
            body.branch_if(*register, false, *target);
            Lowering::Template
        }
        Instruction::BranchIfTrue { register, target } => {
            body.branch_if(*register, true, *target);
            Lowering::Template
        }
        Instruction::IntegerBinary {
            operation,
            destination,
            left,
            right,
            target,
        } => {
            body.integer_binary(*operation, *destination, *left, *right, *target);
            Lowering::Template
        }
        Instruction::SendPublic {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => sends::send_public(
            body,
            *selector,
            *receiver_register,
            *argument_base,
            argument_sources,
            *destination,
            *call_site,
        ),
        // Only the reference lowering emits it, always uncached: an exit,
        // so that the oracle stays interpreted at its sends.
        Instruction::SendInlinable { .. } => Lowering::Exit,
        Instruction::Return { register } | Instruction::ImplicitReturn { register } => {
            sends::return_from(body, *register)
        }
        Instruction::SendLexical {
            selector,
            depth,
            argument_base,
            argument_sources,
            destination,
            call_site,
            ..
        } => sends::send_lexical(
            body,
            *selector,
            *depth,
            *argument_base,
            argument_sources,
            *destination,
            *call_site,
        ),
        Instruction::SendSuper {
            selector,
            argument_base,
            argument_sources,
            destination,
            call_site,
            ..
        } => sends::send_super(
            body,
            *selector,
            *argument_base,
            argument_sources,
            *destination,
            *call_site,
        ),
        // A leaf-safe row's leaf through the shim -- a row's own stub
        // serves only a method whose fast path it is; every other row's
        // send the interpreter's.
        Instruction::SendMirror {
            primitive, failure, ..
        } => match mirror_entry(*primitive).machine_call {
            MachineCall::Leaf | MachineCall::Stub(_) => {
                body.send_mirror(*failure);
                Lowering::Transfer
            }
            MachineCall::Exit => Lowering::Exit,
        },
        // The initializers' invocations, each a compiled call of the body
        // the initializer leaf decides (`sends::initialize`), the check of
        // a `<:` clause, and the lexical walk an implicit receiver makes.
        Instruction::InitializeInstance { destination, .. } => {
            sends::initialize(body, None, *destination)
        }
        Instruction::InitializeSuperclass {
            argument_base,
            argument_sources,
            destination,
            ..
        }
        | Instruction::InitializeAppliedLevel {
            argument_base,
            argument_sources,
            destination,
            ..
        } => sends::initialize(body, Some((*argument_base, argument_sources)), *destination),
        Instruction::CheckAppliedClause { .. } => body.step(),
        Instruction::LoadEnclosingObject { destination, depth } => {
            body.load_enclosing_object(*destination, *depth)
        }
        // A closure, its frame reified inline (`closures::make_closure`).
        Instruction::MakeClosure {
            destination,
            code,
            copied,
            ..
        } => closures::make_closure(body, *destination, code, copied),
        // A register of the frame `depth` closure hops out, on the
        // activation stack or in its divorced frame object
        // (`closures::outer_local`).
        Instruction::LoadOuterLocal {
            destination,
            depth,
            index,
        } => closures::outer_local(
            body,
            *depth,
            *index,
            closures::OuterAccess::Load {
                destination: *destination,
            },
        ),
        Instruction::StoreOuterLocal {
            source,
            depth,
            index,
        } => closures::outer_local(
            body,
            *depth,
            *index,
            closures::OuterAccess::Store { source: *source },
        ),
        // Non-local return: stage 4.
        Instruction::NonLocalReturn { .. } => Lowering::Exit,
        // The own-slot instructions, each through the recorded guard
        // (`BodyAssembler::own_slot`).
        Instruction::InitializeSlot { source, own_index } => {
            body.initialize_slot(*source, *own_index, true)
        }
        Instruction::InitializeSlotProvisionally { source, own_index } => {
            body.initialize_slot(*source, *own_index, false)
        }
        Instruction::LoadOwnSlot {
            destination,
            own_index,
        } => body.load_own_slot(*destination, *own_index),
        Instruction::ReturnSlotIfFilled { own_index } => body.return_slot_if_filled(*own_index),
        Instruction::AllocateInstance { destination } => body.allocate_instance(*destination),
        // The futures: stage 4.
        Instruction::MakeFuture { .. } => Lowering::Exit,
        Instruction::ResolveFuture { .. } => Lowering::Exit,
        // An error, which the interpreter raises: an exit in every stage.
        Instruction::SignalNonBoolean { .. } => Lowering::Exit,
    }
}

#[cfg(test)]
mod tests {
    use crate::builtins::numbers::checked_floor_divide;
    use crate::bytecode::{CodeKind, CompiledCode, Instruction, IntegerOperation, Literal};
    use crate::heap::{fits_small_integer, Immediate, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};
    use crate::interpreter::FeedbackBase;
    use crate::jit::macro_assembler::{Abi, MacroAssembler};
    use crate::jit::stubs::testing::Machine;
    use crate::jit::stubs::MachineExit;

    fn method(instructions: Vec<Instruction>, argument_count: u16) -> CompiledCode {
        CompiledCode::new(
            CodeKind::Method,
            instructions,
            argument_count,
            argument_count + 20,
            0,
            Vec::new(),
            ns_syntax::Span::new(0, 0),
        )
    }

    fn three_instructions() -> CompiledCode {
        method(
            vec![
                Instruction::LoadLiteral {
                    destination: 13,
                    literal: Literal::SmallInteger(3),
                },
                Instruction::Move {
                    destination: 14,
                    source: 13,
                },
                Instruction::SignalNonBoolean { register: 14 },
            ],
            0,
        )
    }

    /// Compiles `code`, polls exiting only on a requested safepoint
    /// whatever `NS_NEXT_STRESS_EXIT` says, and runs it on `frame` with
    /// `safepoint_requested`.
    fn run(code: &CompiledCode, frame: &mut [u64], safepoint_requested: bool) -> MachineExit {
        let mut macro_assembler = MacroAssembler::new(Abi::host());
        macro_assembler.set_exit_at_every_poll(false);
        let body = compiled(code, macro_assembler);
        let mut machine = Machine::new();
        let entry = interpreter_entry(&body, machine.install(&body.code));
        machine
            .run(entry, frame, 0, safepoint_requested, &mut (), &[])
            .0
    }

    /// `code` compiled with `macro_assembler`, its sends uncached.
    fn compiled(code: &CompiledCode, macro_assembler: MacroAssembler) -> super::BaselineBody {
        super::compile(
            code,
            macro_assembler,
            FeedbackBase::NOT_ASSIGNED,
            super::BodyRole::Method,
        )
        .expect("compiled")
    }

    /// Where the run loop enters `body`, installed at `start`: its first
    /// instruction, past the prologue a compiled call enters at.
    fn interpreter_entry(
        body: &super::BaselineBody,
        start: ns_memory::CodeOffset,
    ) -> ns_memory::CodeOffset {
        ns_memory::CodeOffset(start.0 + body.map.machine_offset(0).expect("mapped"))
    }

    /// The bytes of `call rax` and `call rcx`: `FF /2` with the register
    /// in the ModRM byte's r/m field and mode 3 (Intel SDM, Volume 2A,
    /// "CALL -- Call Procedure"), neither needing a REX prefix.
    const CALL_RAX: [u8; 2] = [0xFF, 0xD0];
    const CALL_RCX: [u8; 2] = [0xFF, 0xD1];

    /// Where a call whose encoding is `call` ends in `bytes`, the one place
    /// it occurs.
    fn end_of_the_one(bytes: &[u8], call: [u8; 2]) -> u32 {
        let ends: Vec<usize> = bytes
            .windows(2)
            .enumerate()
            .filter(|(_, window)| *window == call)
            .map(|(start, _)| start + 2)
            .collect();
        assert_eq!(ends.len(), 1, "the call occurs once");
        u32::try_from(ends[0]).expect("small")
    }

    #[test]
    fn a_sends_call_maps_its_return_address_to_the_send() {
        use crate::jit::stubs::ExitKind;
        use ns_assembler::x64::{RAX, RCX};

        let mut macro_assembler = MacroAssembler::new(Abi::host());
        let starts = (0..2)
            .map(|_| macro_assembler.assembler().label())
            .collect();
        let mut body = super::BodyAssembler {
            macro_assembler,
            starts,
            argument_count: 0,
            program_counter: 0,
            declaration_code: false,
            backward_branches: Vec::new(),
            call_returns: Vec::new(),
            feedback_base: FeedbackBase::NOT_ASSIGNED,
            family_size: 0,
            window_end: 0,
            role: super::BodyRole::Method,
            reifies: false,
            frame_register_count: 0,
        };
        // Each call is followed by more code of its instruction, so that a
        // label bound anywhere but right after the call would show.
        for (program_counter, target) in [(0u32, RAX), (1, RCX)] {
            body.program_counter = program_counter;
            let start = body.starts[program_counter as usize];
            body.macro_assembler.assembler().bind(start);
            body.call_send(target);
            body.macro_assembler.store_frame_register(3, target);
        }
        body.macro_assembler.exit(ExitKind::Reexecute, 1);
        let assembled = body.macro_assembler.finish().expect("assembled");
        let map = super::program_counter_map(&assembled, &body.starts, &body.call_returns);
        let first = end_of_the_one(&assembled.bytes, CALL_RAX);
        let second = end_of_the_one(&assembled.bytes, CALL_RCX);
        assert_eq!(map.program_counter_of_return(first), Some(0));
        assert_eq!(map.program_counter_of_return(second), Some(1));
        assert_eq!(map.program_counter_of_return(first + 1), None, "past it");
        assert_eq!(
            map.program_counter_of_return(first - 1),
            None,
            "inside the call"
        );
        assert_eq!(map.program_counter_of_return(0), None);
    }

    fn integer(value: i64) -> u64 {
        Immediate::small_integer(value)
            .expect("a SmallInteger")
            .bits()
    }

    fn boolean(value: bool) -> u64 {
        Immediate::boolean(value).bits()
    }

    #[test]
    fn every_instruction_starts_at_its_own_offset_in_order() {
        let body = compiled(&three_instructions(), MacroAssembler::new(Abi::host()));
        let starts: Vec<u32> = (0..3)
            .map(|program_counter| body.map.machine_offset(program_counter).expect("mapped"))
            .collect();
        assert!(starts[0] > 0, "the prologue comes first");
        assert!(starts[0] < starts[1] && starts[1] < starts[2]);
        assert!(starts[2] < u32::try_from(body.code.bytes.len()).expect("small"));
        for (program_counter, start) in (0..).zip(&starts) {
            assert_eq!(body.map.program_counter_at(*start), Some(program_counter));
        }
    }

    #[test]
    fn a_compiled_body_runs_its_templates_and_exits_at_the_first_without_one() {
        let mut frame = [0u64; 16];
        let exit = run(&three_instructions(), &mut frame, false);
        assert_eq!(exit, MachineExit::Reexecute { program_counter: 2 });
        let mut expected = [0u64; 16];
        expected[13] = integer(3);
        expected[14] = integer(3);
        assert_eq!(frame, expected);
    }

    #[test]
    fn every_immediate_literal_is_its_word() {
        let literals = [
            (Literal::Nil, Immediate::NIL),
            (Literal::Boolean(true), Immediate::boolean(true)),
            (Literal::Boolean(false), Immediate::boolean(false)),
            (
                Literal::SmallInteger(SMALL_INTEGER_MIN),
                Immediate::small_integer(SMALL_INTEGER_MIN).expect("fits"),
            ),
            (
                Literal::Float(2.5),
                Immediate::small_float(2.5).expect("immediate"),
            ),
        ];
        let mut instructions: Vec<Instruction> = (0u16..)
            .zip(&literals)
            .map(|(destination, (literal, _))| Instruction::LoadLiteral {
                destination,
                literal: literal.clone(),
            })
            .collect();
        instructions.push(Instruction::SignalNonBoolean { register: 0 });
        let mut frame = [0u64; 24];
        let exit = run(&method(instructions, 0), &mut frame, false);
        assert_eq!(exit, MachineExit::Reexecute { program_counter: 5 });
        for (index, (_, immediate)) in literals.iter().enumerate() {
            assert_eq!(frame[index], immediate.bits());
        }
    }

    #[test]
    fn a_literal_that_raises_exits_before_it_writes() {
        let code = method(
            vec![Instruction::LoadLiteral {
                destination: 3,
                literal: Literal::UnresolvedIdentifier("missing".into()),
            }],
            0,
        );
        let mut frame = [0u64; 24];
        assert_eq!(
            run(&code, &mut frame, false),
            MachineExit::Reexecute { program_counter: 0 }
        );
        assert_eq!(frame, [0u64; 24]);
    }

    #[test]
    fn load_self_reads_the_receiver_above_the_arguments() {
        let code = method(
            vec![
                Instruction::LoadSelf { destination: 16 },
                Instruction::SignalNonBoolean { register: 16 },
            ],
            2,
        );
        let mut frame = [0u64; 24];
        let receiver = crate::heap::Heap::receiver_register(2);
        frame[usize::from(receiver)] = 0x1230;
        assert_eq!(
            run(&code, &mut frame, false),
            MachineExit::Reexecute { program_counter: 1 }
        );
        assert_eq!(frame[16], 0x1230);
    }

    /// A body that runs `instruction` at 0 and exits at 1 when it falls
    /// through and at 2 when it jumps to its target, 2: at an instruction
    /// that always exits.
    fn branching(instruction: Instruction) -> CompiledCode {
        method(
            vec![
                instruction,
                Instruction::SignalNonBoolean { register: 0 },
                Instruction::SignalNonBoolean { register: 0 },
            ],
            0,
        )
    }

    /// Where `instruction` went with `word` in register 0: 1 on, 2 to its
    /// target.
    fn branch_taken(instruction: Instruction, word: u64) -> u32 {
        let mut frame = [0u64; 24];
        frame[0] = word;
        match run(&branching(instruction), &mut frame, false) {
            MachineExit::Reexecute { program_counter } => program_counter,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn the_conditional_jumps_test_their_register_as_the_interpreter_does() {
        let words = [
            boolean(true),
            boolean(false),
            Immediate::NIL.bits(),
            integer(1),
            0x40,
        ];
        for word in words {
            let as_boolean = (word == boolean(true))
                .then_some(true)
                .or((word == boolean(false)).then_some(false));
            let target = 2;
            let jump_if_not_boolean = Instruction::JumpIfNotBoolean {
                register: 0,
                target,
            };
            assert_eq!(
                branch_taken(jump_if_not_boolean, word),
                if as_boolean.is_some() { 1 } else { 2 }
            );
            let branch_if_false = Instruction::BranchIfFalse {
                register: 0,
                target,
            };
            assert_eq!(
                branch_taken(branch_if_false, word),
                if as_boolean == Some(false) { 2 } else { 1 }
            );
            let branch_if_true = Instruction::BranchIfTrue {
                register: 0,
                target,
            };
            assert_eq!(
                branch_taken(branch_if_true, word),
                if as_boolean == Some(true) { 2 } else { 1 }
            );
        }
    }

    #[test]
    fn a_backward_jump_polls_and_exits_at_its_target() {
        let code = method(
            vec![
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: Literal::SmallInteger(7),
                },
                Instruction::Jump { target: 0 },
            ],
            0,
        );
        let mut frame = [0u64; 24];
        assert_eq!(
            run(&code, &mut frame, true),
            MachineExit::Safepoint { program_counter: 0 }
        );
        assert_eq!(
            frame[1],
            integer(7),
            "the loop's body ran before its back edge"
        );
    }

    #[test]
    fn a_backward_branch_taken_polls_and_one_not_taken_falls_through() {
        let code = method(
            vec![
                Instruction::SignalNonBoolean { register: 0 },
                Instruction::BranchIfTrue {
                    register: 0,
                    target: 0,
                },
                Instruction::SignalNonBoolean { register: 0 },
            ],
            0,
        );
        let compiled = |word: u64| {
            let mut macro_assembler = MacroAssembler::new(Abi::host());
            macro_assembler.set_exit_at_every_poll(false);
            let body = compiled(&code, macro_assembler);
            let start = body.map.machine_offset(1).expect("mapped");
            let mut machine = Machine::new();
            let entry = machine.install(&body.code);
            let mut frame = [0u64; 24];
            frame[0] = word;
            machine
                .run(
                    ns_memory::CodeOffset(entry.0 + start),
                    &mut frame,
                    0,
                    true,
                    &mut (),
                    &[],
                )
                .0
        };
        assert_eq!(
            compiled(boolean(true)),
            MachineExit::Safepoint { program_counter: 0 }
        );
        assert_eq!(
            compiled(boolean(false)),
            MachineExit::Reexecute { program_counter: 2 }
        );
    }

    /// The operands at the edges of every `SmallInteger` operation, and
    /// words that are no `SmallInteger`.
    fn operands() -> Vec<u64> {
        let integers = [
            0,
            1,
            -1,
            2,
            -2,
            3,
            -3,
            7,
            -7,
            8,
            1 << 30,
            -(1 << 30),
            (1 << 31) + 5,
            1 << 40,
            -(1 << 40) - 3,
            SMALL_INTEGER_MAX,
            SMALL_INTEGER_MIN,
            SMALL_INTEGER_MAX - 1,
            SMALL_INTEGER_MIN + 1,
            SMALL_INTEGER_MAX / 2,
            SMALL_INTEGER_MIN / 2,
        ];
        let mut words: Vec<u64> = integers.into_iter().map(integer).collect();
        words.extend([Immediate::NIL.bits(), boolean(true), 0x40]);
        words
    }

    /// What `integer_into` answers for `operation` on two words: the word
    /// it writes, or `None` where it declines.
    fn expected(operation: IntegerOperation, left: u64, right: u64) -> Option<u64> {
        let payload = |word: u64| {
            (word & 1 == 1).then(|| word.cast_signed() >> crate::heap::value_bits::TAG_BITS)
        };
        let (left, right) = (payload(left)?, payload(right)?);
        let integer_word = |value: Option<i64>| {
            value
                .filter(|value| fits_small_integer(*value))
                .map(integer)
        };
        match operation {
            IntegerOperation::Add => integer_word(left.checked_add(right)),
            IntegerOperation::Subtract => integer_word(left.checked_sub(right)),
            IntegerOperation::Multiply => integer_word(left.checked_mul(right)),
            IntegerOperation::FloorDivide => integer_word(checked_floor_divide(left, right)),
            IntegerOperation::Less => Some(boolean(left < right)),
            IntegerOperation::Greater => Some(boolean(left > right)),
            IntegerOperation::LessOrEqual => Some(boolean(left <= right)),
            IntegerOperation::GreaterOrEqual => Some(boolean(left >= right)),
            IntegerOperation::Equal => Some(boolean(left == right)),
        }
    }

    #[test]
    fn integer_arithmetic_answers_what_the_interpreter_answers_or_declines() {
        let operations = [
            IntegerOperation::Add,
            IntegerOperation::Subtract,
            IntegerOperation::Multiply,
            IntegerOperation::FloorDivide,
            IntegerOperation::Less,
            IntegerOperation::Greater,
            IntegerOperation::LessOrEqual,
            IntegerOperation::GreaterOrEqual,
            IntegerOperation::Equal,
        ];
        let operands = operands();
        for operation in operations {
            let code = branching(Instruction::IntegerBinary {
                operation,
                destination: 2,
                left: 0,
                right: 1,
                target: 2,
            });
            let mut macro_assembler = MacroAssembler::new(Abi::host());
            macro_assembler.set_exit_at_every_poll(false);
            let body = compiled(&code, macro_assembler);
            let mut machine = Machine::new();
            let entry = interpreter_entry(&body, machine.install(&body.code));
            for &left in &operands {
                for &right in &operands {
                    let mut frame = [0u64; 24];
                    frame[0] = left;
                    frame[1] = right;
                    frame[2] = 0x5A5A;
                    let (exit, _) = machine.run(entry, &mut frame, 0, false, &mut (), &[]);
                    let what = format!("{operation:?} {left:#x} {right:#x}");
                    match expected(operation, left, right) {
                        Some(word) => {
                            assert_eq!(
                                exit,
                                MachineExit::Reexecute { program_counter: 1 },
                                "{what}"
                            );
                            assert_eq!(frame[2], word, "{what}");
                        }
                        None => {
                            assert_eq!(
                                exit,
                                MachineExit::Reexecute { program_counter: 2 },
                                "{what}"
                            );
                            assert_eq!(frame[2], 0x5A5A, "{what}: a decline writes nothing");
                        }
                    }
                    assert_eq!((frame[0], frame[1]), (left, right), "{what}: operands kept");
                }
            }
        }
    }

    /// A loop whose head lies past an instruction that exits: its back edge
    /// is reached only by the interpreter, after the exit.
    fn loop_past_an_instruction(exiting: bool) -> (CompiledCode, Vec<bool>) {
        let code = method(
            vec![
                Instruction::LoadLiteral {
                    destination: 13,
                    literal: Literal::Boolean(false),
                },
                Instruction::LoadLiteral {
                    destination: 14,
                    literal: Literal::SmallInteger(1),
                },
                Instruction::BranchIfTrue {
                    register: 13,
                    target: 1,
                },
                Instruction::Return { register: 14 },
            ],
            0,
        );
        (code, vec![false, exiting, false, true])
    }

    #[test]
    fn a_loop_reachable_only_past_an_exit_is_no_reason_to_enter() {
        let (code, exits) = loop_past_an_instruction(true);
        assert!(!super::reaches_before_an_exit(&code, &exits, &[false; 6]));
        let (code, exits) = loop_past_an_instruction(false);
        assert!(
            super::reaches_before_an_exit(&code, &exits, &[false; 6]),
            "the same loop through a template is"
        );
    }

    #[test]
    fn a_forward_diamond_without_a_loop_is_no_reason_to_enter() {
        let code = method(
            vec![
                Instruction::LoadLiteral {
                    destination: 13,
                    literal: Literal::Boolean(true),
                },
                Instruction::BranchIfTrue {
                    register: 13,
                    target: 4,
                },
                Instruction::LoadLiteral {
                    destination: 14,
                    literal: Literal::SmallInteger(1),
                },
                Instruction::Jump { target: 5 },
                Instruction::LoadLiteral {
                    destination: 14,
                    literal: Literal::SmallInteger(2),
                },
                Instruction::Return { register: 14 },
            ],
            0,
        );
        let exits = [false, false, false, false, false, true];
        assert!(!super::reaches_before_an_exit(&code, &exits, &[false; 6]));
    }

    /// A body that runs straight to its return reaches a transfer, which
    /// pays for its entry at a send, a push or a frame returned to as a
    /// loop does.
    #[test]
    fn a_return_is_a_reason_to_enter() {
        let (code, _) = loop_past_an_instruction(true);
        let transfers = [false, false, false, true];
        let exits_but_the_return = [false, false, false, false];
        assert!(super::reaches_before_an_exit(
            &code,
            &exits_but_the_return,
            &transfers
        ));
        let straight = method(
            vec![
                Instruction::LoadLiteral {
                    destination: 13,
                    literal: Literal::SmallInteger(1),
                },
                Instruction::Return { register: 13 },
            ],
            0,
        );
        assert!(super::reaches_before_an_exit(
            &straight,
            &[false, false],
            &[false, true]
        ));
        assert!(
            !super::reaches_before_an_exit(&straight, &[false, true], &[false, true]),
            "a return the interpreter runs is no reason"
        );
    }
}

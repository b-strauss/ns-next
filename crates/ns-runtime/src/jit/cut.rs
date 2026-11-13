//! The cut (`docs/design/execution.md`, "Stage 4: closures, non-local
//! return, allocation and the barrier"): a non-local return machine code
//! carries out itself, as Cog's `ceNonLocalReturn` does, when its home
//! lies in the current stretch of compiled frames and no activation with
//! an unwind marker lies between -- and an exit at the `NonLocalReturn`
//! otherwise, so that the interpreter's one unwinder carries the return
//! on (`interpreter::unwind`).
//!
//! The cut stub (`stubs::assemble_cut_stub`) walks the native stack's
//! return addresses from the innermost frame down, handing each to Rust:
//! `cut` takes the block's home as `Mutator::raise_return` takes it, then
//! `cut_frame` names the caller of one frame after another by its return
//! address, as the exit's walk does (`jit::materialize`), until it reaches
//! the home. Nothing is changed before the home is found: every answer of
//! `EXIT` leaves the frames as machine code left them. Once it is found,
//! every frame above it and the home itself are popped as their returns
//! pop them -- a reified one divorced, each one's count taken off the depth
//! count -- and machine code resumes at the home's own return address with
//! the value, as though the home had returned it.
//!
//! The cut runs no Newspeak code, never collects, never pushes, and reads
//! and pops only the compiled frames of the current stretch: the one
//! routine that reads and pops activations other than the one it runs in,
//! which no leaf may.

use crate::bytecode::{CodeKind, Instruction};
use crate::heap::Heap;
use crate::interpreter::Mutator;

use super::materialize::machine_frame;
use super::sends::call_slots;
use super::stubs::BlockWord;
use super::Installed;

/// What `cut` and `cut_frame` answer when the cut cannot carry the return
/// out exactly: no frame changed, the template exits at the
/// `NonLocalReturn` (`stubs::ExitKind::Reexecute`), and the interpreter's
/// `non_local_return` raises it.
pub(crate) const EXIT: u8 = 0;

/// What `cut` and `cut_frame` answer while the home is still to be found:
/// the stub hands over the next return address down the native stack.
pub(crate) const CONTINUE: u8 = 1;

/// Where machine code resumes once the cut popped the home: the home's own
/// return address -- after the call of the send that called it, in its
/// caller's code, or in the entry or the call stub for the frame the
/// stretch began at -- the native stack pointer that address's `ret`
/// would leave, and the frame register as the home's caller left it for
/// the home, at the home's register 0, from which the code after the call
/// takes the send's argument base off again.
pub(crate) struct CutAnswer {
    pub(crate) resume_address: u64,
    pub(crate) stack_pointer: u64,
    pub(crate) frame_register: u64,
}

/// A frame above the home the walk passed: its header's word offset, its
/// body, and the program counter it stopped at -- the `NonLocalReturn` for
/// the innermost, a send for every other.
struct PassedFrame {
    offset: u32,
    installed: Installed,
    program_counter: u32,
}

/// The cut's walk while the stub hands it return addresses, from `cut` to
/// the `cut_frame` that answers. Rust's alone; machine code never reads it.
#[derive(Default)]
pub(crate) struct CutWalk {
    /// The frame whose return address the stub hands over next: its
    /// register 0's word offset from the stack's base, and its body.
    current: Option<(u32, Installed)>,
    /// Every frame above the home passed so far, innermost first.
    passed: Vec<PassedFrame>,
    /// The header offset of the home's activation.
    home: u32,
    /// The stretch's entry frame, which the run loop pushed or resumed --
    /// or, for a stretch an interpreter's send began, that send's sender:
    /// its register 0's word offset and its header's.
    entry_base: u32,
    entry_offset: u32,
    /// The value the `^` returns, as a frame register holds it.
    value: u64,
}

impl CutWalk {
    /// Forgets the walk, keeping the room of its list.
    fn reset(&mut self) {
        self.current = None;
        self.passed.clear();
    }
}

/// `Leaf::Cut`: the start of the cut of the `NonLocalReturn` at
/// `arguments[1]`, the program counter, of the block whose register 0
/// machine code holds at `arguments[0]` and whose body `arguments[2]`, an
/// address inside it, names. Takes the home as `raise_return` does, from
/// the closure the block's frame holds as its operand: `EXIT` for a body
/// that is no block's -- an initializer's or a clause's `^` from an
/// inlined block, which the interpreter returns from the activation
/// itself -- for a block that is the stretch's entry frame, whose home
/// lies below the stretch, and for a home that is not running, whose
/// `cannotReturn` the interpreter raises, suspended, or below the
/// stretch's entry frame; `CONTINUE` otherwise, the walk set out from the
/// block.
pub(crate) fn cut(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [frame_address, program_counter, code_address] = arguments;
    mutator.jit.cut.reset();
    let Ok(program_counter) = u32::try_from(program_counter) else {
        unreachable!("internal: a program counter fits 32 bits");
    };
    let Some(installed) = usize::try_from(code_address)
        .ok()
        .and_then(|address| mutator.jit.body_of(address))
    else {
        unreachable!("internal: the cut is called from an installed body");
    };
    // No inlined scope's `NonLocalReturn` is a template: an inlined
    // block's is a jump to its home's return, and the plan inlines no
    // method that holds one.
    debug_assert!(
        !installed.optimized
            || usize::try_from(code_address)
                .ok()
                .and_then(|address| mutator.jit.inlined_at(address))
                .is_none_or(|inlined| !inlined.inlines()),
        "internal: the cut is called from no inlined scope"
    );
    let code = &installed.code;
    let Instruction::NonLocalReturn { register } = code.instruction(program_counter as usize)
    else {
        unreachable!("internal: the cut is called at a NonLocalReturn");
    };
    if code.kind != CodeKind::Block {
        return u64::from(EXIT);
    }
    // The markers the walk below exits at are its frames'; only a method
    // of the kernel's `Closure` carries one (`compiler::unwind_marker`).
    debug_assert!(
        code.unwind_marker.is_none(),
        "internal: no block's body carries an unwind marker"
    );
    let heap = &mutator.heap;
    let Some(register_base) = heap.machine_register_base_index(frame_address) else {
        unreachable!("internal: a frame's registers lie in the activation stack");
    };
    let Some(entry) = heap.top_activation() else {
        unreachable!("internal: machine code runs above its entry's frame");
    };
    let entry_base = Heap::register_base_index(heap.activation_register_base(entry));
    let entry_offset = Heap::activation_offset(entry);
    if register_base == entry_base {
        return u64::from(EXIT);
    }
    let register_address = |register: u16| frame_address + u64::from(register) * register_bytes();
    let closure = heap
        .machine_frame_value(register_address(Heap::operand_register(
            code.argument_count,
        )))
        .and_then(|operand| operand.as_object())
        .filter(|object| heap.is_closure(*object));
    let Some(home) = closure.and_then(|closure| heap.closure_home(closure)) else {
        return u64::from(EXIT);
    };
    if !heap.frame_is_running(home) {
        return u64::from(EXIT);
    }
    let Some(home_offset) = heap.married_offset_of(home) else {
        return u64::from(EXIT);
    };
    if home_offset < entry_offset {
        return u64::from(EXIT);
    }
    let Some(value) = heap.machine_frame_value(register_address(register)) else {
        unreachable!("internal: a NonLocalReturn's register lies in its frame");
    };
    let value = heap.machine_word_of(value);
    let offset = register_base + u32::from(code.argument_count);
    let walk = &mut mutator.jit.cut;
    walk.home = home_offset;
    walk.entry_base = entry_base;
    walk.entry_offset = entry_offset;
    walk.value = value;
    walk.passed.push(PassedFrame {
        offset,
        installed: installed.clone(),
        program_counter,
    });
    walk.current = Some((register_base, installed));
    u64::from(CONTINUE)
}

/// `Leaf::CutFrame`: one step of the walk `cut` set out on, handed the
/// return address of the frame the walk stands at, `arguments[0]`, the
/// native stack's word it lies in, `arguments[1]`, and the word above it,
/// `arguments[2]`, the next return address down -- the caller's own, when
/// the caller is the home. The return address names the caller and the
/// send it is stopped at (`JitState::return_site`), whose callee lies on
/// the send's argument slots, a block past them (`materialize`'s
/// `Callee`). A caller that is the home and catches a return -- a method's
/// activation, or a superclass clause's -- is where the walk ends: the cut
/// pops every frame above it and the home, and answers the home's return
/// address (`CutAnswer`). `EXIT` for a home that catches none -- an
/// initializer's, whose `^` escapes -- for a return address into a stub,
/// whose caller is the interpreter's, for the entry's frame reached
/// without finding the home, and for a caller whose code carries an unwind
/// marker -- `ensure:`, `ifCurtailed:`, `on:do:` -- whose pending block or
/// handler the interpreter's unwinder decides; `CONTINUE` for any other
/// caller, which the walk passes.
pub(crate) fn cut_frame(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [return_address, slot, home_return_address] = arguments;
    let Some((current_base, current)) = mutator.jit.cut.current.take() else {
        unreachable!("internal: the cut's walk stands at a frame");
    };
    let Some((caller, program_counter)) = usize::try_from(return_address)
        .ok()
        .and_then(|address| mutator.jit.return_site(address))
    else {
        mutator.jit.cut.reset();
        return u64::from(EXIT);
    };
    // A caller that called inside an inlined scope stands for several
    // activations, which the exit's walk materializes and the unwinder
    // pops one by one; one that called where a block its code eliminated
    // is live is left to the exit too, which makes the block.
    if caller.optimized
        && usize::try_from(return_address)
            .ok()
            .and_then(|address| mutator.jit.inlined_at(address))
            .is_some()
    {
        mutator.jit.cut.reset();
        return u64::from(EXIT);
    }
    // A method a method breakpoint names reports its exit by an unwind,
    // which only the interpreter's unwinder makes. Its entry's patch makes
    // its body exit at once, but the JIT stress mode installs such a body
    // all the same, and an on-stack replacement at a loop's head runs it
    // as the stretch's entry frame, the home this walk would return from
    // when its own block returns non-locally.
    if mutator.breakpoints.reports_exits_of(&caller.code) {
        mutator.jit.cut.reset();
        mutator.jit.cut_declined_for_a_method_exit = true;
        return u64::from(EXIT);
    }
    let Some(slots) = call_slots(&caller.code, program_counter) else {
        unreachable!("internal: a compiled call's return address names a send or an initializer's");
    };
    // A block a compiled send called lies past the send's slots, where its
    // closure stub laid it (`targets::ClosureStub`); every other callee on
    // them.
    let shift = if current.code.kind == CodeKind::Block {
        u32::from(slots.argument_count)
    } else {
        0
    };
    let Some(caller_base) = current_base.checked_sub(shift + u32::from(slots.argument_base)) else {
        unreachable!("internal: a callee lies above its caller's slots");
    };
    let walk = &mut mutator.jit.cut;
    let (home, entry_base, entry_offset) = (walk.home, walk.entry_base, walk.entry_offset);
    let at_entry = caller_base == entry_base;
    let caller_offset = if at_entry {
        entry_offset
    } else {
        caller_base + u32::from(caller.code.argument_count)
    };
    if caller_offset == home {
        let catches = if at_entry {
            let Some(entry) = mutator.heap.top_activation() else {
                unreachable!("internal: machine code runs above its entry's frame");
            };
            mutator.catches_return(entry)
        } else {
            caller.method.is_some() || caller.code.kind == CodeKind::SuperclassClause
        };
        if !catches {
            mutator.jit.cut.reset();
            return u64::from(EXIT);
        }
        let answer = CutAnswer {
            resume_address: home_return_address,
            stack_pointer: slot + 2 * return_address_bytes(),
            frame_register: mutator.heap.activation_stack_base_address() as u64
                + u64::from(caller_base) * register_bytes(),
        };
        let home = (!at_entry).then_some(PassedFrame {
            offset: caller_offset,
            installed: caller,
            program_counter,
        });
        return pop_to_the_home(mutator, home, answer);
    }
    if caller_base <= entry_base || caller.code.unwind_marker.is_some() {
        walk.reset();
        return u64::from(EXIT);
    }
    walk.passed.push(PassedFrame {
        offset: caller_offset,
        installed: caller.clone(),
        program_counter,
    });
    walk.current = Some((caller_base, caller));
    u64::from(CONTINUE)
}

/// Pops every frame the walk passed and the home -- a compiled frame
/// `home` describes, or the stretch's entry frame for `None` -- as their
/// returns pop them: each reified one divorced, recording the program
/// counter its frame stopped at, as the interpreter's unwind and the
/// home's return divorce it, and each one's count taken off the depth
/// count, which every compiled frame's prologue took and the home's own
/// return takes, whatever the stub it returns into does next. Writes the
/// answer into the entry block for the stub and answers the address it
/// resumes at.
fn pop_to_the_home(mutator: &mut Mutator<'_>, home: Option<PassedFrame>, answer: CutAnswer) -> u64 {
    let passed = std::mem::take(&mut mutator.jit.cut.passed);
    for frame in passed.iter().chain(home.iter()) {
        divorce(&mut mutator.heap, frame);
    }
    if home.is_none() {
        let Some(entry) = mutator.heap.top_activation() else {
            unreachable!("internal: machine code runs above its entry's frame");
        };
        // The entry's frame has its header; its return divorces it inline,
        // and the run loop's pop then finds it unreified.
        if mutator.heap.activation_reified(entry).is_some() {
            mutator.heap.divorce_at_machine_return(entry);
        }
    }
    // The frames are bounded by the depth limit and its reserve.
    let Ok(popped) = i64::try_from(passed.len()) else {
        unreachable!("internal: a stretch holds fewer frames than the depth limit allows");
    };
    mutator.heap.adjust_counted_depth(-(popped + 1));
    let walk = &mut mutator.jit.cut;
    walk.passed = passed;
    let value = walk.value;
    walk.reset();
    let block = &mutator.jit.block;
    block.set(BlockWord::CutStackPointer.index(), answer.stack_pointer);
    block.set(BlockWord::CutFrameRegister.index(), answer.frame_register);
    block.set(BlockWord::CutValue.index(), value);
    mutator.jit.statistics.cuts += 1;
    answer.resume_address
}

/// The divorce of `frame`, a frame machine code pushed, as its return
/// divorces it, when its code reifies it and it is reified: its header
/// written as a leaf's view writes it, its program counter the one it
/// stopped at.
fn divorce(heap: &mut Heap<'_>, frame: &PassedFrame) {
    if !frame.installed.reifies {
        return;
    }
    let written = machine_frame(
        heap,
        frame.offset,
        &frame.installed,
        frame.program_counter,
        None,
    );
    let activation = heap.write_machine_frame(&written);
    // Read as the innermost for the divorce's length, as a leaf's view
    // reads it (`materialize::leaf_frame`): a frame's words are read
    // below the stack's top alone.
    let extent = heap.stack_extent();
    heap.view_machine_frame(activation);
    if heap.activation_reified(activation).is_some() {
        heap.mark_stopped_in_machine_code(activation, frame.program_counter);
        heap.divorce_at_machine_return(activation);
    }
    heap.restore_stack_extent(extent);
}

/// The bytes of a frame register (`sends::REGISTER_BYTES`), as an
/// address's addend.
fn register_bytes() -> u64 {
    u64::from(super::sends::REGISTER_BYTES.unsigned_abs())
}

/// The bytes of a return address on the native stack, the host
/// convention's stack slot (`Abi::stack_slot_bytes`), as an address's
/// addend.
fn return_address_bytes() -> u64 {
    u64::from(
        super::macro_assembler::Abi::host()
            .stack_slot_bytes()
            .unsigned_abs(),
    )
}

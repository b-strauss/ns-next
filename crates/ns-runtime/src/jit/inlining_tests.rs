//! What the exit leaves of a frame of optimized code that inlines, read
//! where only the crate can read it (`docs/design/execution.md`,
//! "Inlining"): the activations each inlined scope stands for at an exit,
//! the frame a leaf called inside one runs in, and the prologue's refusal
//! of a call its deepest chain of scopes would take past the depth limit.

use indoc::indoc;

use crate::heap::{ActivationReference, CodeId, Heap};
use crate::interpreter::inline_cache::MethodSide;
use crate::interpreter::Mutator;
use crate::types::ActivationKind;
use crate::{HostResult, HostSignal, Interpreter, Lowering, Root, SourceOrigin, Symbol};

/// What a test records of every exit's stack and every leaf's frame
/// (`JitState::recorder`).
#[derive(Default)]
pub(crate) struct Recorder {
    exits: Vec<RecordedExit>,
    /// Each leaf's frame, and whether it is an inlined scope's.
    leaves: Vec<(RecordedFrame, bool)>,
}

/// The stack an exit left, innermost first, as the run loop found it, how
/// many activations the heap counted live, and whether the exit left an
/// inlined scope of optimized code (`JitState::exited_physical`).
struct RecordedExit {
    frames: Vec<RecordedFrame>,
    depth: usize,
    inlined: bool,
}

/// One activation as an observer reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecordedFrame {
    selector: Option<Symbol>,
    code: CodeId,
    program_counter: usize,
    compiled: bool,
}

fn describe(heap: &Heap<'_>, activation: ActivationReference) -> RecordedFrame {
    let selector = match heap.activation_kind(activation) {
        ActivationKind::Method(selector) => Some(selector),
        _ => None,
    };
    RecordedFrame {
        selector,
        code: heap.activation_code_id(activation),
        program_counter: heap.activation_program_counter(activation),
        compiled: heap.activation_is_compiled(activation),
    }
}

/// Records the stack an exit left, from `innermost` down to the bottom;
/// `inlined` when it left an inlined scope.
pub(crate) fn record_exit(
    mutator: &mut Mutator<'_>,
    innermost: ActivationReference,
    inlined: bool,
) {
    if mutator.jit.recorder.is_none() {
        return;
    }
    let mut frames = Vec::new();
    let mut frame = Some(innermost);
    while let Some(activation) = frame {
        frames.push(describe(&mutator.heap, activation));
        frame = mutator.heap.activation_sender(activation);
    }
    let depth = mutator.heap.activation_depth();
    if let Some(recorder) = mutator.jit.recorder.as_mut() {
        recorder.exits.push(RecordedExit {
            frames,
            depth,
            inlined,
        });
    }
}

/// Records the frame a leaf runs in, written for it (`leaf_frame`);
/// `inlined` when it is an inlined scope's.
pub(crate) fn record_leaf(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    inlined: bool,
) {
    if mutator.jit.recorder.is_none() {
        return;
    }
    let frame = describe(&mutator.heap, activation);
    if let Some(recorder) = mutator.jit.recorder.as_mut() {
        recorder.leaves.push((frame, inlined));
    }
}

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// `outer:` sends `middle:`, which the plan inlines into `outer:`'s
/// optimized code, and `middle:` sends `probe:`, which weighs more than
/// the plan inlines, so that its send is a frame node of the inlined
/// scope: an interrupt point inside it.
const NESTED: &str = indoc! {"
    class Subject = Object new () (
      probe: value = (
        ^value + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0
      )
      middle: value = (
        ^self probe: value
      )
      outer: value = (
        ^self middle: value
      )
      public run = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 3000] whileTrue: [
          sum:: sum + (self outer: index).
          index:: index + 1.
        ].
        ^sum
      )
    )
"};

/// `middle:` reads a string literal, which machine code loads through the
/// generic leaf (`leaves::step`), inside its scope inlined into `outer:`.
const LEAF_IN_A_CALLEE: &str = indoc! {"
    class Subject = Object new () (
      middle: value = (
        ^'abc' size + value
      )
      outer: value = (
        ^self middle: value
      )
      public run = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 3000] whileTrue: [
          sum:: sum + (self outer: index).
          index:: index + 1.
        ].
        ^sum
      )
    )
"};

/// `source`'s `run`, every body compiled at its first entry and optimized
/// at its first spend -- and, under `deopt`, its optimized code leaving
/// at every n-th interrupt point -- with the recorder on: the answer, what
/// was recorded, and the code ids and the program counter of the one send
/// of each of `sends`, a selector of a body and the selector it sends.
fn run_recording(
    source: &str,
    deopt: bool,
    sends: &[(&str, &str)],
) -> (i64, Recorder, Vec<(CodeId, usize)>) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = crate::resolve(module, source.into(), SourceOrigin::File("test.ns".into()))
        .unwrap_or_else(|_| panic!("the module resolves"));
    let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
    // The runs' feedback is what the compiles read: the cache stress mode
    // would vacate it at the runs' safepoints.
    #[cfg(debug_assertions)]
    interpreter.set_stress_caches(false);
    interpreter.set_jit_stress(true);
    if deopt {
        interpreter.set_deopt_stress(true);
    } else {
        interpreter.set_optimize_stress(true);
    }
    let class = interpreter.load_module(&resolved);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    interpreter.enter(|mutator| mutator.jit.recorder = Some(Recorder::default()));
    let answer = expect_ok(
        interpreter.send(&instance, Symbol::intern("run"), &[]),
        "run",
    );
    let answer = interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
        crate::ValueKind::SmallInteger(integer) => integer,
        _ => panic!("run: expected a small integer"),
    });
    let (recorder, sends) = interpreter.enter(|mutator| {
        let recorder = mutator.jit.recorder.take().expect("recording");
        let sends = sends
            .iter()
            .map(|(body, sent)| {
                let code = crate::interpreter::method_code(
                    mutator,
                    &class,
                    MethodSide::Instance,
                    Symbol::intern(body),
                )
                .expect("the method");
                let program_counter = code
                    .instructions()
                    .iter()
                    .position(|instruction| match instruction {
                        crate::bytecode::Instruction::SendPublic { selector, .. }
                        | crate::bytecode::Instruction::SendLexical { selector, .. } => {
                            *selector == Symbol::intern(sent)
                        }
                        _ => false,
                    })
                    .expect("the send");
                (mutator.heap.code_id(&code), program_counter)
            })
            .collect();
        (recorder, sends)
    });
    (answer, recorder, sends)
}

/// The sum `NESTED` and `LEAF_IN_A_CALLEE` answer, `addend` added to
/// every index below 3,000.
fn sum_of_indices(addend: i64) -> i64 {
    (0..3000).map(|index| index + addend).sum()
}

/// Under the deopt stress mode optimized code leaves at its interrupt
/// points, among them the send of `probe:` inside `middle:`'s scope
/// inlined into `outer:`: there the exit leaves two activations of the
/// one physical frame, each read as an interpreter's -- `middle:` the
/// innermost, at its `probe:` send, its caller `outer:`, at its `middle:`
/// send, each its own body -- and the heap counts every activation the
/// stack holds, the scope among them.
#[test]
fn an_exit_inside_an_inlined_callee_leaves_two_interpreted_frames() {
    let (answer, recorder, sends) = run_recording(
        NESTED,
        true,
        &[("middle:", "probe:"), ("outer:", "middle:")],
    );
    assert_eq!(answer, sum_of_indices(0));
    let [(middle, probe_send), (outer, middle_send)] = sends[..] else {
        panic!("two sends");
    };
    let inside: Vec<&RecordedExit> = recorder
        .exits
        .iter()
        .filter(|exit| {
            exit.inlined
                && exit.frames.len() >= 2
                && exit.frames[0].selector == Some(Symbol::intern("middle:"))
                && exit.frames[1].selector == Some(Symbol::intern("outer:"))
        })
        .collect();
    assert!(
        !inside.is_empty(),
        "an exit inside the inlined scope, of {} exits",
        recorder.exits.len()
    );
    for exit in inside {
        let (innermost, caller) = (exit.frames[0], exit.frames[1]);
        assert_eq!(innermost.code, middle);
        assert_eq!(innermost.program_counter, probe_send);
        assert_eq!(caller.code, outer);
        assert_eq!(caller.program_counter, middle_send);
        assert_eq!(
            exit.depth,
            exit.frames.len(),
            "every activation of the stack is counted, the scope among them"
        );
    }
}

/// After an exit inside an inlined scope the scope is an activation of
/// its own, going on interpreted from the exit, and the physical frame
/// below it stands in machine code at its send, so that the scope's
/// return resumes it in its body's baseline code, as any frame an exit
/// left below the innermost: the answer is the interpreter's.
#[test]
fn each_scope_goes_on_in_its_own_baseline_code_after_an_exit() {
    let (answer, recorder, _) = run_recording(NESTED, true, &[]);
    assert_eq!(answer, sum_of_indices(0));
    let exit = recorder
        .exits
        .iter()
        .find(|exit| {
            exit.inlined
                && exit.frames.len() >= 2
                && exit.frames[0].selector == Some(Symbol::intern("middle:"))
                && exit.frames[1].selector == Some(Symbol::intern("outer:"))
        })
        .expect("an exit inside the inlined scope");
    assert!(
        exit.frames[1].compiled,
        "the physical frame goes on in machine code once its scope returns"
    );
}

/// A leaf called inside an inlined scope runs in that scope's frame, its
/// header written as the walk writes it: the generic leaf loading
/// `middle:`'s string literal reads `middle:`'s body at its own program
/// counter, never `outer:`'s, and the answer is the interpreter's.
#[test]
fn a_leaf_called_from_an_inlined_scope_reads_that_scope() {
    let (answer, recorder, sends) = run_recording(LEAF_IN_A_CALLEE, false, &[("middle:", "size")]);
    assert_eq!(answer, sum_of_indices(3));
    let [(middle, size_send)] = sends[..] else {
        panic!("one send");
    };
    let in_middle: Vec<&RecordedFrame> = recorder
        .leaves
        .iter()
        .filter(|(_, inlined)| *inlined)
        .map(|(frame, _)| frame)
        .collect();
    assert!(!in_middle.is_empty(), "a leaf ran in an inlined scope");
    for frame in in_middle {
        assert_eq!(frame.selector, Some(Symbol::intern("middle:")));
        assert_eq!(frame.code, middle);
        assert!(
            frame.program_counter < size_send,
            "the literal's load, before the send"
        );
    }
}

/// Optimized code whose inlining nests a scope below its own refuses a
/// call whose push would leave the depth count within that one scope of
/// the depth limit word, naming the refusal as its inlining's, which
/// counts against no budget; one further from the word passes the
/// prologue, and one at the word is refused at the bound, as any call.
#[test]
fn the_prologue_refuses_within_the_deepest_chain_of_the_limit() {
    use crate::heap::machine_words::MachineWord;
    use crate::jit::baseline::BodyRole;
    use crate::jit::macro_assembler::{Abi, MacroAssembler};
    use crate::jit::optimizing::{compile_tree, tree_of_run, Speculation};
    use crate::jit::stubs::testing::Machine;
    use crate::jit::stubs::MachineExit;

    let tree = tree_of_run(
        indoc! {"
            class Monomorphic = () (
              public run = (
                ^self helper: 3
              )
              public helper: number = (
                ^number + 1
              )
            )
        "},
        "run",
    );
    let optimized = compile_tree(
        &tree,
        MacroAssembler::new(Abi::host()),
        BodyRole::Method,
        false,
        Speculation::Traps,
    )
    .unwrap_or_else(|refusal| panic!("refused: {refusal:?}"));
    assert_eq!(optimized.deepest_chain, 1, "`helper:` is inlined");
    let limit = 1000;
    let run_at = |depth: u64| {
        let mut machine = Machine::new();
        machine.set_machine_word(MachineWord::DepthCount, depth);
        machine.set_machine_word(MachineWord::DepthLimit, limit);
        machine.set_machine_word(MachineWord::OrdinaryStackEnd, 1 << 12);
        // An immediate receiver, whose guard no arm names: past the
        // prologue the type-case traps.
        machine.set_machine_word(
            MachineWord::CalleeReceiver,
            crate::heap::Immediate::small_integer(5)
                .expect("a SmallInteger")
                .bits(),
        );
        let body = machine.install(&optimized.code);
        let leaves: [ns_memory::Leaf<()>; 4] = [|_, _| 0, |_, _| 0, |_, _| 0, |_, _| 0];
        let mut frame = vec![0u64; 1 << 12];
        machine.run_called(body, &mut frame, &mut (), &leaves).0
    };
    assert_eq!(
        run_at(limit - 1),
        MachineExit::PushRefused {
            at_the_bound: false,
            for_its_inlining: true,
        }
    );
    assert_eq!(
        run_at(limit),
        MachineExit::PushRefused {
            at_the_bound: true,
            for_its_inlining: false,
        }
    );
    assert!(
        matches!(run_at(limit - 2), MachineExit::Trap { .. }),
        "past the prologue"
    );
}

//! What the exit leaves of a frame of optimized code that inlines, read
//! where only the crate can read it (`docs/design/execution.md`,
//! "Inlining"): the activations each inlined scope stands for at an exit,
//! the frame a leaf called inside one runs in, and the prologue's refusal
//! of a call its deepest chain of scopes would take past the depth limit.

use indoc::indoc;

use crate::heap::{ActivationReference, CodeId, Heap};
use crate::interpreter::Mutator;
use crate::interpreter::inline_cache::MethodSide;
use crate::{HostResult, HostSignal, Interpreter, Lowering, Root, SourceOrigin, Symbol};

/// The object a frame word named when the frame left its optimized code
/// at a poll and when it went back in, by their offsets.
pub(crate) type ObjectsAcross = (Option<u64>, Option<u64>);

/// What a test records of every exit's stack and every leaf's frame
/// (`JitState::recorder`).
#[derive(Default)]
pub(crate) struct Recorder {
    exits: Vec<RecordedExit>,
    /// Each leaf's frame, and whether it is an inlined scope's.
    leaves: Vec<(RecordedFrame, bool)>,
    /// Each frame that went on in its body's baseline code -- returned to
    /// (`jit::resumed`), or switched into it at a backward jump
    /// (`jit::back_edge_ready`): how many exits were recorded before it, its body,
    /// the program counter it went on at and whether the code it entered
    /// is its body's twin, which a reified frame goes on in.
    resumptions: Vec<(usize, CodeId, u32, bool)>,
    /// Each block an exit made (`materialize::materialize_blocks`), with
    /// how many resumptions were recorded before it.
    blocks: Vec<(usize, RecordedBlock)>,
    /// Each frame that went back into its optimized code at the poll it
    /// left (`jit::resumed_at_poll`): where the copy started when the frame
    /// left it and where it started when the frame went back in, and
    /// whether the poll's entry wrote a value back into a frame word the
    /// collector's pre-pass may have nilled.
    poll_reentries: Vec<(u32, u32, bool)>,
    /// For each of those re-entries, each object a frame word the entry
    /// loads a machine register from named when the frame left, and when
    /// it went back in, by their offsets.
    reentered_objects: Vec<Vec<ObjectsAcross>>,
}

/// What an exit made of one block optimized code eliminated, as the
/// stack reads right after.
#[derive(Debug, Clone, Copy)]
struct RecordedBlock {
    /// How many registers it was written into, and of how many scopes.
    registers: usize,
    scopes: usize,
    /// Whether every one of them reads back the one closure.
    one_object: bool,
    /// Whether the closure's captured frame is its home's activation's
    /// frame object, married to it.
    home_married: bool,
    /// The body of its home's activation.
    home: CodeId,
    /// Whether a scope of the chain runs the block, its activation's
    /// closure the one made.
    run_by_a_scope: bool,
    /// Whether its frame lies below the innermost the exit left.
    below: bool,
}

/// Records the block the exit made, `made`, its id and its closure, over
/// `home`, one of `scopes`' activations -- a point's chain of scopes and
/// each one's activation -- in a frame `below` the innermost or not, as
/// the stack reads it.
pub(crate) fn record_block<'gc>(
    mutator: &mut Mutator<'gc>,
    (chain, activations): (
        &[crate::jit::optimizing::metadata::ScopeAtPoint],
        &[Option<ActivationReference>],
    ),
    made: (
        crate::jit::optimizing::plan::EliminatedId,
        crate::heap::ObjectReference<'gc>,
    ),
    home: ActivationReference,
    below: bool,
) {
    use crate::jit::optimizing::allocate::Location;

    if mutator.jit.recorder.is_none() {
        return;
    }
    let (block, closure) = made;
    let heap = &mutator.heap;
    let mut recorded = RecordedBlock {
        registers: 0,
        scopes: 0,
        one_object: true,
        home_married: false,
        home: heap.activation_code_id(home),
        run_by_a_scope: false,
        below,
    };
    for stopped in chain {
        let Some(activation) = activations[stopped.scope.0 as usize] else {
            continue;
        };
        let base = heap.activation_register_base(activation);
        let held: Vec<u16> = stopped
            .locations
            .iter()
            .filter(|(_, location)| *location == Location::EliminatedBlock(block))
            .map(|(register, _)| *register)
            .collect();
        if !held.is_empty() {
            recorded.scopes += 1;
        }
        for register in held {
            recorded.registers += 1;
            recorded.one_object &= heap
                .register(base, register)
                .as_object()
                .is_some_and(|object| object.same(closure));
        }
        if heap
            .activation_closure(activation)
            .is_some_and(|running| running.same(closure))
        {
            recorded.run_by_a_scope = true;
        }
    }
    recorded.home_married = heap
        .activation_reified(home)
        .zip(heap.closure_captured_frame(closure))
        .is_some_and(|(married, captured)| {
            heap.frame_object(married).same(heap.frame_object(captured))
        });
    if let Some(recorder) = mutator.jit.recorder.as_mut() {
        let resumed_before = recorder.resumptions.len();
        recorder.blocks.push((resumed_before, recorded));
    }
}

/// The stack an exit left, innermost first, as the run loop found it, how
/// many activations the heap counted live, and whether the exit left an
/// inlined scope of optimized code (`JitState::exited_physical`).
struct RecordedExit {
    frames: Vec<RecordedFrame>,
    depth: usize,
    /// The activations the depth limit counts (`Heap::counted_depth`).
    counted: usize,
    inlined: bool,
    /// Whether the exit is a trap.
    trapped: bool,
}

/// One activation as an observer reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecordedFrame {
    selector: Option<Symbol>,
    code: CodeId,
    program_counter: usize,
    compiled: bool,
    /// Whether its current class is its receiver's class; `None` for a
    /// receiver that is no object.
    class_is_the_receiver_s: Option<bool>,
}

fn describe(heap: &Heap<'_>, activation: ActivationReference) -> RecordedFrame {
    // A leaf's view of an inlined block's activation names no closure yet:
    // its kind is read only of a method's.
    let selector = heap.activation_method_selector(activation);
    let (receiver, current_class) = heap.activation_receiver_and_current_class(activation);
    RecordedFrame {
        selector,
        code: heap.activation_code_id(activation),
        program_counter: heap.activation_program_counter(activation),
        compiled: heap.activation_is_compiled(activation),
        class_is_the_receiver_s: receiver
            .as_object()
            .map(|object| current_class.is_some_and(|class| class.same(heap.object_class(object)))),
    }
}

/// Records the stack an exit left, from `innermost` down to the bottom;
/// `inlined` when it left an inlined scope, `trapped` when it is a trap.
pub(crate) fn record_exit(
    mutator: &mut Mutator<'_>,
    innermost: ActivationReference,
    inlined: bool,
    trapped: bool,
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
    let counted = mutator.heap.counted_depth();
    if let Some(recorder) = mutator.jit.recorder.as_mut() {
        recorder.exits.push(RecordedExit {
            frames,
            depth,
            counted,
            inlined,
            trapped,
        });
    }
}

/// Records the frame `activation` going on in its body's baseline code at
/// `program_counter`, entering it at `entry` -- returned to
/// (`jit::resumed`) or switched into at a backward jump
/// (`jit::back_edge_ready`) -- and whether that code is the body's twin.
pub(crate) fn record_resumption(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    program_counter: u32,
    entry: ns_memory::CodeOffset,
) {
    let code = mutator.heap.activation_code_id(activation);
    let twin = mutator
        .jit
        .zone
        .as_ref()
        .map(|zone| zone.writer.zone().executable_address(entry))
        .and_then(|address| mutator.jit.body_of(address))
        .is_some_and(|installed| installed.reifies);
    if let Some(recorder) = mutator.jit.recorder.as_mut() {
        let exits = recorder.exits.len();
        recorder
            .resumptions
            .push((exits, code, program_counter, twin));
    }
}

/// Records a frame going back into its optimized code at the poll it
/// left, the copy having started at `exit_start` then and at
/// `entry_start` now, its entry rewriting a lost word or not, and the
/// objects its entry's machine registers are loaded from then and now.
pub(crate) fn record_poll_reentry(
    mutator: &mut Mutator<'_>,
    (exit_start, entry_start): (u32, u32),
    (rewrites_lost_words, objects): (bool, Vec<ObjectsAcross>),
) {
    if let Some(recorder) = mutator.jit.recorder.as_mut() {
        recorder
            .poll_reentries
            .push((exit_start, entry_start, rewrites_lost_words));
        recorder.reentered_objects.push(objects);
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

/// Whether the zone stress mode is on, as the JIT reads it: in a debug
/// build only, where the mode exists (`jit::stress_zone_from_env`).
fn the_zone_stress_mode_is_on() -> bool {
    crate::jit::stress_zone_from_env()
}

/// What `run_recording` answers: the run's answer, what was recorded, the
/// code id and program counter of each send asked for, and the
/// activations the depth limit counted before the run.
struct Recording {
    answer: i64,
    recorder: Recorder,
    sends: Vec<(CodeId, usize)>,
    counted_before: usize,
}

/// `source`'s `run`, the JIT on whatever the environment says, every body
/// compiled at its first entry and optimized at its first spend -- and,
/// under `deopt`, its optimized code leaving at every n-th interrupt
/// point -- with the recorder on: the answer, what was recorded, the code
/// ids and the program counter of the one send of each of `sends`, a
/// selector of a body and the selector it sends, and the counted depth
/// before the run.
fn run_recording(source: &str, deopt: bool, sends: &[(&str, &str)]) -> Recording {
    run_recording_after(source, (false, deopt), sends)
}

/// `run_recording`, `run` run once before with no stress mode of the
/// test's own and nothing recorded when `warmed`, so that the caches hold
/// what every send of a run reaches before the first body is optimized.
fn run_recording_after(
    source: &str,
    (warmed, deopt): (bool, bool),
    sends: &[(&str, &str)],
) -> Recording {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = crate::resolve(module, source.into(), SourceOrigin::File("test.ns".into()))
        .unwrap_or_else(|_| panic!("the module resolves"));
    let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
    // The runs' feedback is what the compiles read: the cache stress mode
    // would vacate it at the runs' safepoints.
    #[cfg(debug_assertions)]
    interpreter.set_stress_caches(false);
    // A body the breakpoint stress mode patched is neither optimized
    // nor inlined, and its baseline code exits at every statement.
    #[cfg(debug_assertions)]
    interpreter.set_stress_breakpoints(false);
    // The step stress mode is off for the same reason: a body a step's one-shot
    // breakpoints patched loses its machine code and every copy that inlined
    // it.
    #[cfg(debug_assertions)]
    interpreter.set_stress_step(false);
    interpreter.set_jit_enabled(true);
    interpreter.set_jit_stress(true);
    let class = interpreter.load_module(&resolved);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    if warmed {
        expect_ok(
            interpreter.send(&instance, Symbol::intern("run"), &[]),
            "run",
        );
    }
    if deopt {
        interpreter.set_deopt_stress(true);
    } else {
        interpreter.set_optimize_stress(true);
    }
    let counted_before = interpreter.enter(|mutator| {
        mutator.jit.recorder = Some(Recorder::default());
        mutator.heap.counted_depth()
    });
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
    Recording {
        answer,
        recorder,
        sends,
        counted_before,
    }
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
/// stack holds, the scope among them: live, and against the depth limit
/// exactly as the interpreter counts there, `run`, `outer:` and `middle:`
/// above what counted before the run, whatever a stress mode suspended.
#[test]
fn an_exit_inside_an_inlined_callee_leaves_two_interpreted_frames() {
    let Recording {
        answer,
        recorder,
        sends,
        counted_before,
    } = run_recording(
        NESTED,
        true,
        &[("middle:", "probe:"), ("outer:", "middle:")],
    );
    assert_eq!(answer, sum_of_indices(0));
    // The zone stress mode compacts the zone at every safepoint: every
    // other compaction spares optimized code and slides it, but the
    // freeing at the others is constant enough that no exit falls inside
    // the inlined scope to observe. The answer is still the interpreter's.
    if the_zone_stress_mode_is_on() {
        return;
    }
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
            "every activation of the stack is live, the scope among them"
        );
        assert_eq!(
            exit.counted,
            counted_before + 3,
            "the depth limit counts `run`, `outer:` and the scope `middle:`"
        );
    }
}

/// `outer:` inlines `adding:`, which sends nothing: its scope has no frame
/// node, so its copy never computes its current class. `run` adds
/// `delta` to every index below 3,000; `useFloats` makes it 3 / 2, no
/// `SmallInteger`.
const NO_FRAME_NODE: &str = indoc! {"
    class Subject = Object new (
      | delta ::= 1. |
    ) (
      adding: value = (
        ^value + delta
      )
      outer: value = (
        ^self adding: value
      )
      public useFloats = (
        delta:: 3 / 2.
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

/// An inlined scope with no frame node has its current class written by
/// the exit's walk alone: once `delta` is no `SmallInteger`, the addition
/// in `adding:`'s scope, inlined into `outer:`'s inlined into `run`,
/// traps, and the exit leaves both scopes' activations with their
/// receiver's class as their current class, as the interpreter's pushes
/// would have it.
#[test]
fn the_exit_writes_the_current_class_of_a_scope_without_a_frame_node() {
    let module = ns_syntax::parse(NO_FRAME_NODE).expect("parse");
    let resolved = crate::resolve(
        module,
        NO_FRAME_NODE.into(),
        SourceOrigin::File("class.ns".into()),
    )
    .unwrap_or_else(|_| panic!("the module resolves"));
    let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
    #[cfg(debug_assertions)]
    interpreter.set_stress_caches(false);
    // A body the breakpoint stress mode patched is neither optimized
    // nor inlined, and its baseline code exits at every statement.
    #[cfg(debug_assertions)]
    interpreter.set_stress_breakpoints(false);
    interpreter.set_jit_enabled(true);
    interpreter.set_jit_stress(true);
    let class = interpreter.load_module(&resolved);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let run = Symbol::intern("run");
    expect_ok(interpreter.send(&instance, run, &[]), "run");
    interpreter.set_optimize_stress(true);
    expect_ok(interpreter.send(&instance, run, &[]), "run");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("useFloats"), &[]),
        "useFloats",
    );
    interpreter.enter(|mutator| mutator.jit.recorder = Some(Recorder::default()));
    let answer = expect_ok(interpreter.send(&instance, run, &[]), "run");
    let printed = interpreter.enter(|mutator| {
        let answer = mutator.fetch(&answer);
        mutator.heap.describe(answer)
    });
    assert_eq!(printed, "4503000");
    let stressed = std::env::vars_os().any(|(name, value)| {
        name.to_string_lossy().starts_with("NS_NEXT_STRESS_") && !value.is_empty() && value != "0"
    });
    if stressed {
        return;
    }
    let recorder = interpreter.enter(|mutator| mutator.jit.recorder.take().expect("recording"));
    let inside: Vec<&RecordedExit> = recorder
        .exits
        .iter()
        .filter(|exit| {
            exit.inlined
                && exit
                    .frames
                    .first()
                    .is_some_and(|frame| frame.selector == Some(Symbol::intern("adding:")))
        })
        .collect();
    assert!(
        !inside.is_empty(),
        "an exit inside the scope, of {} exits",
        recorder.exits.len()
    );
    for exit in inside {
        assert_eq!(exit.frames[0].class_is_the_receiver_s, Some(true));
        assert_eq!(exit.frames[1].class_is_the_receiver_s, Some(true));
    }
}

/// After an exit inside an inlined scope each scope the exit left is an
/// activation of its own going on in its own body's code: `middle:`, the
/// innermost, interpreted from the exit at its `probe:` send; `outer:`,
/// the scope it was inlined into, stopped at its `middle:` send and
/// resumed, once `middle:` returns, in `outer:`'s own baseline code right
/// past that send, as any frame an exit left below the innermost. The
/// answer is the interpreter's.
#[test]
fn each_scope_goes_on_in_its_own_baseline_code_after_an_exit() {
    let Recording {
        answer,
        recorder,
        sends,
        ..
    } = run_recording(
        NESTED,
        true,
        &[("middle:", "probe:"), ("outer:", "middle:")],
    );
    assert_eq!(answer, sum_of_indices(0));
    // The zone stress mode compacts the zone at every safepoint: every
    // other compaction spares optimized code and slides it, but the
    // freeing at the others is constant enough that no exit falls inside
    // the inlined scope to observe. The answer is still the interpreter's.
    if the_zone_stress_mode_is_on() {
        return;
    }
    let [(_, probe_send), (outer, middle_send)] = sends[..] else {
        panic!("two sends");
    };
    let after_send = u32::try_from(middle_send + 1).expect("a small body");
    let mut checked = 0;
    for (index, exit) in recorder.exits.iter().enumerate() {
        if !(exit.inlined
            && exit.frames.len() >= 2
            && exit.frames[0].selector == Some(Symbol::intern("middle:"))
            && exit.frames[1].selector == Some(Symbol::intern("outer:")))
        {
            continue;
        }
        assert_eq!(exit.frames[0].program_counter, probe_send);
        assert!(exit.frames[1].compiled, "`outer:` stands in machine code");
        // A resumption after this exit is `outer:`'s, past its send; a
        // stress mode's collections and suspensions resume other frames
        // between.
        assert!(
            recorder
                .resumptions
                .iter()
                .any(|(exits, code, program_counter, _)| *exits > index
                    && (*code, *program_counter) == (outer, after_send)),
            "`outer:` goes on in its own baseline code past its send"
        );
        checked += 1;
    }
    assert!(checked > 0, "an exit inside the inlined scope");
}

/// A leaf called inside an inlined scope runs in that scope's frame, its
/// header written as the walk writes it: the generic leaf loading
/// `middle:`'s string literal reads `middle:`'s body at its own program
/// counter, never `outer:`'s, and the answer is the interpreter's.
#[test]
fn a_leaf_called_from_an_inlined_scope_reads_that_scope() {
    let Recording {
        answer,
        recorder,
        sends,
        ..
    } = run_recording(LEAF_IN_A_CALLEE, false, &[("middle:", "size")]);
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
    use crate::jit::optimizing::{Speculation, compile_tree, tree_of_run};
    use crate::jit::stubs::MachineExit;
    use crate::jit::stubs::testing::Machine;

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

/// What the growth leaf is handed and answers, for the prologue's test.
#[derive(Default)]
struct Growth {
    extents: Vec<u64>,
}

fn refuse_growth(growth: &mut Growth, arguments: [u64; 3]) -> u64 {
    growth.extents.push(arguments[0]);
    1
}

fn no_leaf(_: &mut Growth, _: [u64; 3]) -> u64 {
    0
}

/// Optimized code's prologue holds the stack's ordinary room to the
/// deepest physical register its inlined scopes reach, not only to its
/// own window: with the room's end between the two, the call asks the
/// growth leaf for the inlined extent, and is refused when the leaf
/// makes no room; with room past it, the call goes on.
#[test]
fn the_prologue_holds_the_room_to_the_deepest_extent() {
    use crate::heap::machine_words::MachineWord;
    use crate::jit::baseline::BodyRole;
    use crate::jit::leaves::Leaf;
    use crate::jit::macro_assembler::{Abi, MacroAssembler};
    use crate::jit::optimizing::{Speculation, compile_tree, tree_of_run};
    use crate::jit::stubs::MachineExit;
    use crate::jit::stubs::testing::Machine;

    let tree = tree_of_run(
        indoc! {"
            class Monomorphic = () (
              public run = (
                ^self helper: 3
              )
              public helper: number = (
                | a b c d e f g h |
                a:: number.
                b:: a.
                c:: b.
                d:: c.
                e:: d.
                f:: e.
                g:: f.
                h:: g.
                ^h + 1
              )
            )
        "},
        "run",
    );
    let window_end = u32::from(tree.root.facts.window_end());
    let optimized = compile_tree(
        &tree,
        MacroAssembler::new(Abi::host()),
        BodyRole::Method,
        false,
        Speculation::Traps,
    )
    .unwrap_or_else(|refusal| panic!("refused: {refusal:?}"));
    assert_eq!(optimized.deepest_chain, 1, "`helper:` is inlined");
    let extent = optimized.deepest_extent;
    assert!(extent > window_end + 1, "the scope reaches past the window");
    let run_with_end = |end: u32| {
        let mut machine = Machine::new();
        machine.set_machine_word(MachineWord::DepthCount, 0);
        machine.set_machine_word(MachineWord::DepthLimit, 1000);
        machine.set_machine_word(MachineWord::OrdinaryStackEnd, u64::from(end));
        machine.set_machine_word(
            MachineWord::CalleeReceiver,
            crate::heap::Immediate::small_integer(5)
                .expect("a SmallInteger")
                .bits(),
        );
        let body = machine.install(&optimized.code);
        let mut leaves: Vec<ns_memory::Leaf<Growth>> = vec![no_leaf; Leaf::GrowStack as usize + 1];
        leaves[Leaf::GrowStack as usize] = refuse_growth;
        let mut frame = vec![0u64; 1 << 12];
        let mut growth = Growth::default();
        let exit = machine.run_called(body, &mut frame, &mut growth, &leaves).0;
        (exit, growth.extents)
    };
    let (exit, extents) = run_with_end(window_end + 1);
    assert_eq!(
        exit,
        MachineExit::PushRefused {
            at_the_bound: false,
            for_its_inlining: false,
        },
        "no room made for the inlined scope"
    );
    assert_eq!(
        extents,
        vec![u64::from(extent) * 8],
        "the inlined extent asked for"
    );
    let (exit, extents) = run_with_end(extent);
    assert!(
        matches!(exit, MachineExit::Trap { .. }),
        "past the prologue"
    );
    assert!(extents.is_empty(), "room enough");
}

/// `outer:` inlines `helper:`, whose locals reach past `outer:`'s own
/// window, so that its copy's deepest extent lies above the window end
/// of the frame a push of `outer:` makes.
const ROOM_AT_A_PUSH: &str = indoc! {"
    class Subject = Object new () (
      helper: number = (
        | a b c d e f g h |
        a:: number.
        b:: a.
        c:: b.
        d:: c.
        e:: d.
        f:: e.
        g:: f.
        h:: g.
        ^h + 1
      )
      public outer: number = (
        ^self helper: number
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

/// The run loop's push of a body whose optimized copy inlines enters the
/// copy only where the frame has the room its inlined scopes reach
/// (`jit::entered`): with every push seating the ordinary end at the
/// pushed frame's window end, each host send of `outer:` goes on in its
/// baseline code, whose call of `helper:` grows the stack first, and
/// nothing is written past the seated end -- where entering the copy
/// would write `helper:`'s registers -- and every answer is the
/// interpreter's. Without the seat the same sends run the copy, which
/// grows nothing.
#[test]
fn a_push_enters_no_copy_whose_inlined_scopes_pass_the_stack_s_end() {
    let module = ns_syntax::parse(ROOM_AT_A_PUSH).expect("parse");
    let resolved = crate::resolve(
        module,
        ROOM_AT_A_PUSH.into(),
        SourceOrigin::File("room.ns".into()),
    )
    .unwrap_or_else(|_| panic!("the module resolves"));
    let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
    #[cfg(debug_assertions)]
    interpreter.set_stress_caches(false);
    // A body the breakpoint stress mode patched is neither optimized
    // nor inlined, and its baseline code exits at every statement.
    #[cfg(debug_assertions)]
    interpreter.set_stress_breakpoints(false);
    interpreter.set_jit_enabled(true);
    interpreter.set_jit_stress(true);
    let class = interpreter.load_module(&resolved);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let outer = Symbol::intern("outer:");
    let send_outer = |interpreter: &mut Interpreter, number: i64| {
        let argument = interpreter.stash(crate::Value::small_integer(number));
        let answer = expect_ok(interpreter.send(&instance, outer, &[argument]), "outer:");
        interpreter.enter(|mutator| mutator.fetch(&answer).as_small_integer())
    };
    // The caches fill before `outer:` is optimized, so that its copy
    // inlines what its send reached.
    let run = |interpreter: &mut Interpreter| {
        let answer = expect_ok(
            interpreter.send(&instance, Symbol::intern("run"), &[]),
            "run",
        );
        interpreter.enter(|mutator| mutator.fetch(&answer).as_small_integer())
    };
    let sum: i64 = (1..=3000).sum();
    assert_eq!(run(&mut interpreter), Some(sum));
    interpreter.set_optimize_stress(true);
    assert_eq!(run(&mut interpreter), Some(sum));
    let stressed = std::env::vars_os().any(|(name, value)| {
        name.to_string_lossy().starts_with("NS_NEXT_STRESS_") && !value.is_empty() && value != "0"
    });
    let inlined = interpreter.jit_inlined_bodies(&class, MethodSide::Instance, outer);
    if !stressed {
        assert_eq!(inlined, 1, "`outer:`'s copy inlines `helper:`");
        let before = interpreter.jit_statistics();
        assert_eq!(send_outer(&mut interpreter, 41), Some(42));
        let after = interpreter.jit_statistics();
        assert_eq!(
            after.stack_growths, before.stack_growths,
            "without the seat the copy runs, inlining the call"
        );
    }
    interpreter.enter(|mutator| mutator.heap.set_seat_ordinary_end_at_top(true));
    let before = interpreter.jit_statistics();
    for number in 0..10 {
        assert_eq!(send_outer(&mut interpreter, number), Some(number + 1));
    }
    let after = interpreter.jit_statistics();
    let disturbances = interpreter.enter(|mutator| {
        mutator.heap.set_seat_ordinary_end_at_top(false);
        mutator.heap.seat_disturbances()
    });
    assert_eq!(
        disturbances, 0,
        "nothing is written past a seated ordinary end before the stack grows"
    );
    if !stressed {
        assert!(
            after.stack_growths > before.stack_growths,
            "baseline code's call of `helper:` grows the stack: {} to {}",
            before.stack_growths,
            after.stack_growths
        );
    }
}

/// An inlined site that only ever saw one receiver meets another: the
/// type-case traps, the trap sets the site's trapped bit, the copy is
/// retired at its second trap there, and the copy compiled again has the
/// site's send as the type-case's way for receivers without an arm, so
/// that later runs with both receivers trap there no more. Every answer
/// is the interpreter's.
#[test]
fn an_inlined_site_meeting_an_unseen_receiver_traps_and_is_recompiled_with_its_send() {
    use crate::jit::optimizing::plan::{FallThrough, Limits, plan};
    use crate::jit::optimizing::tree::SnapshotTree;

    let source = indoc! {"
        class Subject = Object new (
          | other = Other new. |
        ) (
          class Other = () (
            public value: number = (
              ^number * 2
            )
          )
          public value: number = (
            ^number + 1
          )
          outer: target at: number = (
            ^target value: number
          )
          public run = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < 3000] whileTrue: [
              sum:: sum + (self outer: (index < 2000 ifTrue: [self] ifFalse: [other]) at: index).
              index:: index + 1.
            ].
            ^sum
          )
        )
    "};
    let expected: i64 = (0..3000)
        .map(|index| if index < 2000 { index + 1 } else { index * 2 })
        .sum();
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = crate::resolve(module, source.into(), SourceOrigin::File("test.ns".into()))
        .unwrap_or_else(|_| panic!("the module resolves"));
    let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
    #[cfg(debug_assertions)]
    interpreter.set_stress_caches(false);
    // A body the breakpoint stress mode patched is neither optimized
    // nor inlined, and its baseline code exits at every statement.
    #[cfg(debug_assertions)]
    interpreter.set_stress_breakpoints(false);
    // The step stress mode is off for the same reason: a body a step's one-shot
    // breakpoints patched loses its machine code and every copy that inlined
    // it.
    #[cfg(debug_assertions)]
    interpreter.set_stress_step(false);
    interpreter.set_jit_enabled(true);
    interpreter.set_jit_stress(true);
    interpreter.set_optimize_stress(true);
    let class = interpreter.load_module(&resolved);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    interpreter.enter(|mutator| mutator.jit.recorder = Some(Recorder::default()));
    // The traps recorded so far in `outer:at:`'s code, the inlined
    // `value:` among it.
    let traps_of_outer = |interpreter: &mut Interpreter| {
        interpreter.enter(|mutator| {
            let recorder = mutator.jit.recorder.as_ref().expect("recording");
            recorder
                .exits
                .iter()
                .filter(|exit| exit.trapped)
                .filter(|exit| {
                    exit.frames
                        .iter()
                        .any(|frame| frame.selector == Some(Symbol::intern("outer:at:")))
                })
                .count()
        })
    };
    let run = |interpreter: &mut Interpreter| {
        let answer = expect_ok(
            interpreter.send(&instance, Symbol::intern("run"), &[]),
            "run",
        );
        interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
            crate::ValueKind::SmallInteger(integer) => integer,
            _ => panic!("run: expected a small integer"),
        })
    };
    assert_eq!(run(&mut interpreter), expected);
    // The zone stress mode compacts the zone at every safepoint: every
    // other compaction spares optimized code and slides it, but the
    // freeing at the others leaves no copy compiled for one receiver to
    // meet the other and trap. Every answer is still the interpreter's.
    if the_zone_stress_mode_is_on() {
        assert_eq!(run(&mut interpreter), expected);
        assert_eq!(run(&mut interpreter), expected);
        return;
    }
    let first = interpreter.jit_statistics();
    let trapped = traps_of_outer(&mut interpreter);
    assert!(trapped >= 1, "the unseen receiver traps: {first:?}");
    assert!(
        first.made_not_entrant >= 1,
        "the copy is retired: {first:?}"
    );
    // The site trapped: the plan now sends to a receiver without an arm.
    let site_plan = interpreter.enter(|mutator| {
        let code = crate::interpreter::method_code(
            mutator,
            &class,
            MethodSide::Instance,
            Symbol::intern("outer:at:"),
        )
        .expect("the method");
        let base = mutator.caches.known_base(&code).expect("it ran");
        let tree = SnapshotTree::take(
            &code,
            &mut mutator.heap,
            &mutator.caches,
            base,
            None,
            &Limits::STANDARD,
        );
        let send = tree
            .root
            .instructions
            .iter()
            .position(|instruction| instruction.argument_slots().is_some())
            .expect("the send");
        let facts = tree.root.sites[send].expect("its site");
        let planned = plan(&tree, crate::jit::optimizing::Speculation::Traps);
        let fall_through = planned
            .sites
            .get(&(
                crate::jit::optimizing::plan::ScopeId::ROOT,
                u32::try_from(send).expect("small"),
            ))
            .map(|site| site.fall_through);
        (facts.trapped, fall_through)
    });
    assert!(site_plan.0, "the trap set the site's bit");
    assert_ne!(
        site_plan.1,
        Some(FallThrough::Trap),
        "no trap for an unseen receiver"
    );
    // `run`'s own copy, compiled before it saw the other receiver, traps
    // at its own speculations; `outer:at:`'s, compiled again, never.
    assert_eq!(run(&mut interpreter), expected);
    assert_eq!(run(&mut interpreter), expected);
    assert_eq!(
        traps_of_outer(&mut interpreter),
        trapped,
        "the copy compiled again traps no more"
    );
}

/// `store:` stores a young `Array` into the `item` slot of `holder`
/// through the setter its optimized copy inlines, which `run` compiles;
/// `storeLast` stores one more, and `check` reads it back.
#[cfg(debug_assertions)]
const AN_OLD_HOLDER: &str = indoc! {"
    class Subject = Object new (
      | holder = Holder new. |
    ) (
      class Holder = Object new (
        | public item ::= nil. |
      ) ()
      store: index = (
        | element = Array new: 1. |
        element at: 1 put: index.
        holder item: element.
        ^index
      )
      public run = (
        | index ::= 0. |
        [index < 3000] whileTrue: [
          self store: index.
          index:: index + 1.
        ].
        ^0
      )
      public storeLast = (
        ^self store: 4242
      )
      public check = (
        ^holder item at: 1
      )
    )
"};

/// The setter `store:`'s optimized copy inlines is a store through the
/// barrier: once `run` has compiled the copy, a full collection makes the
/// holder old and leaves it unremembered, and the copy's store of a young
/// value into it remembers it, so that the remembered set verifies and
/// the scavenge after it keeps the value the old object names -- one the
/// barrier missed would leave it naming the vacated nursery, read back as
/// poison, or at exit status 0 as another object. A debug build's: the
/// collections on demand, the verification and the poison are its own.
#[cfg(debug_assertions)]
#[test]
fn a_setter_arm_stores_through_the_barrier() {
    use crate::jit::optimizing::build::build_inlined;
    use crate::jit::optimizing::graph::Node;
    use crate::jit::optimizing::plan::plan;
    use crate::jit::optimizing::{Speculation, tree_of_run};

    // What the copy compiles: the setter's store, which calls the barrier's
    // leaf.
    let tree = tree_of_run(AN_OLD_HOLDER, "store:");
    let graph = build_inlined(&tree, &plan(&tree, Speculation::Traps), Speculation::Traps);
    assert!(
        graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .any(|node| matches!(node, Node::FieldStore { .. })),
        "{graph:#?}"
    );

    let module = ns_syntax::parse(AN_OLD_HOLDER).expect("parse");
    let resolved = crate::resolve(
        module,
        AN_OLD_HOLDER.into(),
        SourceOrigin::File("test.ns".into()),
    )
    .unwrap_or_else(|_| panic!("the module resolves"));
    let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
    // The run's feedback is what the copy is compiled from: the cache
    // stress mode would vacate it at the run's safepoints.
    #[cfg(debug_assertions)]
    interpreter.set_stress_caches(false);
    // A body the breakpoint stress mode patched is neither optimized
    // nor inlined, and its baseline code exits at every statement.
    #[cfg(debug_assertions)]
    interpreter.set_stress_breakpoints(false);
    interpreter.set_jit_enabled(true);
    interpreter.set_jit_stress(true);
    interpreter.set_optimize_stress(true);
    let class = interpreter.load_module(&resolved);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("run"), &[]),
        "run",
    );
    assert!(
        interpreter.jit_customized_copies(&class, MethodSide::Instance, Symbol::intern("store:"))
            >= 1,
        "`store:` runs as a copy"
    );
    // The holder is old and unremembered from here on.
    interpreter.collect_now();
    expect_ok(
        interpreter.send(&instance, Symbol::intern("storeLast"), &[]),
        "storeLast",
    );
    interpreter.enter(|mutator| mutator.heap.verify_remembered_set());
    interpreter.scavenge_now();
    let checked = expect_ok(
        interpreter.send(&instance, Symbol::intern("check"), &[]),
        "check",
    );
    let checked = interpreter.enter(|mutator| match mutator.fetch(&checked).kind() {
        crate::ValueKind::SmallInteger(integer) => integer,
        _ => panic!("check: expected a small integer"),
    });
    assert_eq!(checked, 4242);
}

/// `step:with:` hands `check:at:with:`, which its copy inlines, the block
/// `double`, which both value only through inlined sends, so that the copy
/// eliminates it; `check:at:with:` first sends its prober `probe:`, which
/// the copy inlines for the `Plain` prober every pass is handed -- until,
/// from its second run on, `run` hands the last pass an `Other`, which no
/// arm of the type-case takes: a trap inside the inlined scope where the
/// block lies in two registers, `step:with:`'s local and
/// `check:at:with:`'s parameter. `step:with:` then counts in a loop of its
/// own, whose backward jumps take its frame back into machine code. `run`
/// answers three times the sum of 0 to 2,999.
const A_BLOCK_IN_TWO_SCOPES: &str = indoc! {"
    class Subject = Object new (
      | runs ::= 0. |
    ) (
      class Plain = () (
        public probe: value = (
          ^value
        )
      )
      class Other = () (
        public probe: value = (
          ^value
        )
      )
      check: action at: index with: prober = (
        | probed |
        probed:: prober probe: index.
        ^probed + (action value: index)
      )
      step: index with: prober = (
        |
          double = [:value | value * 2].
          result = self check: double at: index with: prober.
          count ::= 0.
        |
        [count < 10] whileTrue: [count:: count + 1].
        ^result
      )
      public run = (
        |
          sum ::= 0.
          index ::= 0.
          plain = Plain new.
          other = Other new.
        |
        runs:: runs + 1.
        [index < 3000] whileTrue: [
          sum:: sum + (self step: index with: ((runs > 1 and: [index = 2999]) ifTrue: [other] ifFalse: [plain])).
          index:: index + 1.
        ].
        ^sum
      )
    )
"};

/// `step:with:` holds the block `double`, which it values only through
/// `apply:to:`, inlined, so that its copy eliminates it, and sends
/// `probe:`, which weighs more than the plan inlines and is optimized on
/// its own, its additions speculating on `SmallInteger`s: from its second
/// run on, `run` hands the last ten passes a `Float`, so that `probe:`'s
/// copy traps, leaving `step:with:`'s frame below it, standing in its
/// copy at the send with the block in its local. `run` answers twice the
/// sum of 0 to 2,999.
const A_BLOCK_BELOW_AN_EXIT: &str = indoc! {"
    class Subject = Object new (
      | runs ::= 0. |
    ) (
      probe: value = (
        ^value + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0 + 0
      )
      apply: action to: value = (
        ^action value: value
      )
      step: index with: value = (
        | double = [:each | each * 2]. |
        self probe: value.
        ^self apply: double to: index
      )
      public run = (
        |
          sum ::= 0.
          index ::= 0.
        |
        runs:: runs + 1.
        [index < 3000] whileTrue: [
          sum:: sum + (self step: index with: ((runs > 1 and: [index >= 2990]) ifTrue: [index + 0.5] ifFalse: [index])).
          index:: index + 1.
        ].
        ^sum
      )
    )
"};

/// The trap inside `check:at:with:`'s scope, where the block
/// `step:with:` eliminated lies in two scopes' registers, makes one closure
/// of it, which every register that holds it answers, its captured frame
/// `step:with:`'s activation's frame object, married to it. The answer is
/// the interpreter's.
#[test]
fn an_eliminated_block_is_one_object_after_an_exit() {
    let Recording {
        answer, recorder, ..
    } = run_recording_after(A_BLOCK_IN_TWO_SCOPES, (true, false), &[]);
    assert_eq!(answer, sum_of_indices(0) * 3);
    // The zone stress mode compacts the zone at every safepoint: every
    // other compaction spares optimized code and slides it, but the
    // freeing at the others is constant enough that no exit falls inside
    // the inlined scope to observe. The answer is still the interpreter's.
    if the_zone_stress_mode_is_on() {
        return;
    }
    assert!(!recorder.blocks.is_empty(), "an exit made a block");
    for (_, block) in &recorder.blocks {
        assert!(block.one_object, "{block:?}");
        assert!(block.home_married, "{block:?}");
    }
    assert!(
        recorder
            .blocks
            .iter()
            .any(|(_, block)| block.scopes >= 2 && block.registers >= 2),
        "a block held in `step:with:`'s local and `check:at:with:`'s parameter: {:?}",
        recorder.blocks
    );
}

/// The block an exit makes captures its home's activation, which it
/// reifies: `step:with:`, whose frame goes back into machine code, once
/// the inlined `check:at:with:` the trap left returns, in its body's twin,
/// the copy that keeps a reified frame's words, never in its lean code.
/// The answer is the interpreter's.
#[test]
fn the_home_of_a_materialized_block_goes_on_in_its_twin() {
    let Recording {
        answer,
        recorder,
        sends,
        ..
    } = run_recording_after(
        A_BLOCK_IN_TWO_SCOPES,
        (true, false),
        &[("step:with:", "check:at:with:")],
    );
    assert_eq!(answer, sum_of_indices(0) * 3);
    // The zone stress mode compacts the zone at every safepoint: every
    // other compaction spares optimized code and slides it, but the
    // freeing at the others is constant enough that no exit falls inside
    // the inlined scope to observe. The answer is still the interpreter's.
    if the_zone_stress_mode_is_on() {
        return;
    }
    let [(step, _)] = sends[..] else {
        panic!("one send");
    };
    assert!(
        recorder.blocks.iter().any(|(resumed_before, block)| {
            block.home == step
                && recorder.resumptions[*resumed_before..]
                    .iter()
                    .any(|(_, resumed, _, twin)| *resumed == step && *twin)
        }),
        "`step:with:` goes on in its twin after an exit made its block: {:?} {:?}",
        recorder.blocks,
        recorder.resumptions
    );
}

/// An exit deeper than a frame of optimized code makes the blocks that
/// frame's code eliminated too: `probe:`'s copy, which `step:with:`'s
/// copy called, traps, and `step:with:`'s frame, which goes on in
/// baseline code once `probe:` returns, finds its block made, one closure
/// in its local and married to its frame. The answer is the
/// interpreter's.
#[test]
fn a_block_in_a_frame_below_an_exit_is_made_there() {
    let Recording {
        answer,
        recorder,
        sends,
        ..
    } = run_recording_after(
        A_BLOCK_BELOW_AN_EXIT,
        (true, false),
        &[("step:with:", "probe:")],
    );
    assert_eq!(answer, sum_of_indices(0) * 2);
    // The zone stress mode compacts the zone at every safepoint: every
    // other compaction spares optimized code and slides it, but the
    // freeing at the others is constant enough that no exit falls inside
    // the inlined scope to observe. The answer is still the interpreter's.
    if the_zone_stress_mode_is_on() {
        return;
    }
    let [(step, _)] = sends[..] else {
        panic!("one send");
    };
    let below: Vec<&RecordedBlock> = recorder
        .blocks
        .iter()
        .map(|(_, block)| block)
        .filter(|block| block.below && block.home == step)
        .collect();
    assert!(
        !below.is_empty(),
        "an exit above `step:with:` made its block: {:?}",
        recorder.blocks
    );
    for block in below {
        assert!(block.one_object && block.home_married, "{block:?}");
        assert!(block.registers >= 1, "{block:?}");
    }
}

/// `countOf:` hands `values do:` a block that counts, which its copy
/// eliminates where it inlines `Triple`'s `do:` for the `Triple` `run`
/// hands it; from its second run on, `run` hands the last pass a `Pair`,
/// which no arm of the type-case takes, so that the copy traps at the send
/// of `do:` with the block in its argument slot. `run` answers 2,999 times
/// 3, then 2 for the `Pair`.
const A_BLOCK_HANDED_TO_A_TRAPPING_SEND: &str = indoc! {"
    class Subject = Object new (
      | runs ::= 0. |
    ) (
      class Pair = () (
        public do: action = (
          action value: 1.
          action value: 2.
        )
      )
      class Triple = () (
        public do: action = (
          action value: 1.
          action value: 2.
          action value: 3.
        )
      )
      countOf: values = (
        | count ::= 0. |
        values do: [:each | count:: count + 1].
        ^count
      )
      public run = (
        |
          sum ::= 0.
          index ::= 0.
          small = Triple new.
        |
        runs:: runs + 1.
        [index < 3000] whileTrue: [
          sum:: sum + (self countOf: ((runs > 1 and: [index = 2999]) ifTrue: [Pair new] ifFalse: [small])).
          index:: index + 1.
        ].
        ^sum
      )
    )
"};

/// A type-case that finds a receiver none of its arms takes traps, and
/// the interpreter runs its send again, reading its arguments where the
/// send placed them: the exit makes the block the copy eliminated in its
/// argument slot, so that `Pair`'s `do:` is handed a closure, which
/// counts. The answer is the interpreter's.
#[test]
fn a_block_handed_to_a_send_a_type_case_traps_at_is_made_in_its_slot() {
    let Recording {
        answer,
        recorder,
        sends,
        ..
    } = run_recording_after(
        A_BLOCK_HANDED_TO_A_TRAPPING_SEND,
        (true, false),
        &[("countOf:", "do:")],
    );
    assert_eq!(answer, 2999 * 3 + 2);
    // The zone stress mode compacts the zone at every safepoint: every
    // other compaction spares optimized code and slides it, but the
    // freeing at the others leaves no copy compiled for the Arrays to
    // meet the `Pair` and trap. The answer is still the interpreter's.
    if the_zone_stress_mode_is_on() {
        return;
    }
    let [(count_of, _)] = sends[..] else {
        panic!("one send");
    };
    assert!(
        recorder.blocks.iter().any(|(_, block)| {
            block.home == count_of && !block.below && block.one_object && block.home_married
        }),
        "the trap made `countOf:`'s block: {:?}",
        recorder.blocks
    );
}

/// `sumOf:` hands its block to an `Empty`'s `do:`, `Array`'s, whose
/// mirror cannot read the size of an instance of a subclass, which holds
/// no elements: its failure way hands the block to `super do:`,
/// `Collection`'s, whose `subclassResponsibility` `Empty` answers with 0,
/// and `sumOf:` answers the 100 it began its `sum` with.
const A_BLOCK_HANDED_ON_ON_A_FAILURE_WAY: &str = indoc! {"
    class Subject = Object new (
      | empty = Empty new. |
    ) (
      class Empty = Array new () (
        public subclassResponsibility = (
          ^0
        )
      )
      sumOf: collection = (
        | sum ::= 100. |
        collection do: [:element | sum:: sum + element].
        ^sum
      )
      public run = (
        |
          total ::= 0.
          index ::= 0.
        |
        [index < 3000] whileTrue: [
          total:: total + (self sumOf: empty).
          index:: index + 1.
        ].
        ^total
      )
    )
"};

/// The way of `Array`'s `do:` for a receiver whose size the mirror cannot
/// read, read as never run (`SUPER_SENDS_READ_AS_NEVER_RUN`), is a trap
/// on its `super do:` in a copy that speculates, and lets the block
/// `sumOf:` hands it escape nowhere: the copy of `sumOf:` inlines `do:`
/// for `Empty` and eliminates the block. Its first entry takes the way and
/// traps, the exit makes the block in `sumOf:`'s frame, and the
/// interpreter runs `super do:` with it; the trap marks the site, the
/// next one at the site retires the copy, and the copy compiled after it
/// sends `super do:`, the block created: no trap more, and every answer
/// the interpreter's.
#[test]
fn a_failure_way_that_starts_running_traps_and_its_block_is_made() {
    use crate::jit::optimizing::plan::{BlockPlan, Limits, plan};
    use crate::jit::optimizing::snapshot::SUPER_SENDS_READ_AS_NEVER_RUN;
    use crate::jit::optimizing::tree::SnapshotTree;

    SUPER_SENDS_READ_AS_NEVER_RUN.set(true);
    let _reset = core::mem::DropGuard::new((), |()| SUPER_SENDS_READ_AS_NEVER_RUN.set(false));
    let source = A_BLOCK_HANDED_ON_ON_A_FAILURE_WAY;
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = crate::resolve(module, source.into(), SourceOrigin::File("test.ns".into()))
        .unwrap_or_else(|_| panic!("the module resolves"));
    let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
    #[cfg(debug_assertions)]
    interpreter.set_stress_caches(false);
    // A body the breakpoint stress mode patched is neither optimized
    // nor inlined, and its baseline code exits at every statement.
    #[cfg(debug_assertions)]
    interpreter.set_stress_breakpoints(false);
    interpreter.set_jit_enabled(true);
    interpreter.set_jit_stress(true);
    let class = interpreter.load_module(&resolved);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let run = |interpreter: &mut Interpreter| {
        let answer = expect_ok(
            interpreter.send(&instance, Symbol::intern("run"), &[]),
            "run",
        );
        interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
            crate::ValueKind::SmallInteger(integer) => integer,
            _ => panic!("run: expected a small integer"),
        })
    };
    // A run that fills the caches before anything is optimized.
    assert_eq!(run(&mut interpreter), 300_000);
    interpreter.set_optimize_stress(true);
    interpreter.set_jit_counting(true);
    let sum_of = interpreter.enter(|mutator| {
        mutator.jit.recorder = Some(Recorder::default());
        let code = crate::interpreter::method_code(
            mutator,
            &class,
            MethodSide::Instance,
            Symbol::intern("sumOf:"),
        )
        .expect("the method");
        mutator.heap.code_id(&code)
    });
    let before = interpreter.jit_statistics();
    assert_eq!(run(&mut interpreter), 300_000);
    // The zone stress mode compacts the zone at every safepoint, and the
    // freeing at every other compaction leaves no copy for the way to
    // trap in; the optimize, deopt and invalidate modes change which copy
    // runs when, compiling one before the run above filled the caches,
    // leaving one before it reaches the way, or freeing it. The answer is
    // still the interpreter's.
    let changes_the_copy = ["OPTIMIZE", "DEOPT", "INVALIDATE"].iter().any(|mode| {
        std::env::var_os(format!("NS_NEXT_STRESS_{mode}"))
            .is_some_and(|value| !value.is_empty() && value != "0")
    });
    if the_zone_stress_mode_is_on() || changes_the_copy {
        assert_eq!(run(&mut interpreter), 300_000);
        assert_eq!(run(&mut interpreter), 300_000);
        return;
    }
    let first = interpreter.jit_statistics();
    let trapped_in_do = |interpreter: &mut Interpreter| {
        interpreter.enter(|mutator| {
            let recorder = mutator.jit.recorder.as_ref().expect("recording");
            recorder
                .exits
                .iter()
                .filter(|exit| exit.trapped)
                .filter(|exit| {
                    exit.frames
                        .first()
                        .is_some_and(|frame| frame.selector == Some(Symbol::intern("do:")))
                        && exit.frames.iter().any(|frame| frame.code == sum_of)
                })
                .count()
        })
    };
    let traps = trapped_in_do(&mut interpreter);
    assert!(traps >= 1, "the failure way traps in `do:`: {first:?}");
    assert!(
        first.eliminated_blocks - before.eliminated_blocks >= 1,
        "the copy eliminated the block: {first:?}"
    );
    let made = interpreter.enter(|mutator| {
        let recorder = mutator.jit.recorder.as_ref().expect("recording");
        recorder
            .blocks
            .iter()
            .filter(|(_, block)| block.home == sum_of && block.one_object && block.home_married)
            .count()
    });
    assert!(made >= 1, "the trap made `sumOf:`'s block");
    // The first trap marks the site, and the copy goes on being entered
    // until the second there retires it: every copy retired took two.
    let retired = first.made_not_entrant - before.made_not_entrant;
    assert!(
        retired >= 1,
        "the second trap at the site retires the copy: {first:?}"
    );
    assert!(
        first.traps - before.traps >= 2,
        "the first trap retires no copy: {first:?}"
    );
    assert!(
        u64::try_from(traps).unwrap_or(u64::MAX) > retired,
        "a copy retired only at its second trap: {traps} traps, {retired} retired"
    );
    // The site trapped: a compile of `sumOf:` now sends `super do:`, and
    // creates the block it is handed.
    let created = interpreter.enter(|mutator| {
        let code = crate::interpreter::method_code(
            mutator,
            &class,
            MethodSide::Instance,
            Symbol::intern("sumOf:"),
        )
        .expect("the method");
        let base = mutator.caches.known_base(&code).expect("it ran");
        let tree = SnapshotTree::take(
            &code,
            &mut mutator.heap,
            &mutator.caches,
            base,
            None,
            &Limits::STANDARD,
        );
        let planned = plan(&tree, crate::jit::optimizing::Speculation::Traps);
        planned
            .blocks
            .values()
            .all(|block| *block == BlockPlan::Created)
    });
    assert!(created, "the block is created once the way ran");
    // `run`'s copy, which inlines `sumOf:`, may have been compiled before
    // the site trapped, and traps there once; every copy compiled since
    // sends `super do:`.
    assert_eq!(run(&mut interpreter), 300_000);
    let settled = trapped_in_do(&mut interpreter);
    assert_eq!(run(&mut interpreter), 300_000);
    assert_eq!(run(&mut interpreter), 300_000);
    assert_eq!(
        trapped_in_do(&mut interpreter),
        settled,
        "the copies compiled again trap no more"
    );
}

/// A poll's light exit and the frame going back into its optimized code
/// there, or not, once the safepoint is served. Debug builds only, where
/// a test sets the scavenge threshold.
#[cfg(debug_assertions)]
mod polls {
    use super::*;

    /// `run: count` sums 1 to `count` twice, each time through `to:do:`,
    /// whose block allocates an `Array` at every pass: once `run:`'s copy
    /// inlines `to:do:` and the block, the inner loop -- `to:do:`'s own, in
    /// its inlined scope -- polls with the block eliminated and live in
    /// `to:do:`'s parameter, and the scavenges the allocations ask for stop
    /// it there. `run: count` answers `count * (count + 1)`.
    const AN_ALLOCATING_INLINED_LOOP: &str = indoc! {"
        class Subject = Object new () (
          public run: count = (
            |
              sum ::= 0.
              round ::= 0.
              last
            |
            [round < 2] whileTrue: [
              1 to: count do: [:step |
                last:: {step. step. step. step. step. step. step. step}.
                sum:: sum + (last at: 8).
              ].
              round:: round + 1.
            ].
            ^sum
          )
        )
    "};

    /// What `run_polling` answers: the run's answer, what was recorded, what
    /// the JIT counted while it ran, `run:`'s code id, the activations the
    /// depth limit counted before the run and after it, and the scavenges
    /// it took.
    struct Polled {
        answer: i64,
        recorder: Recorder,
        statistics: crate::jit::JitStatistics,
        run: CodeId,
        counted: (usize, usize),
        scavenges: usize,
    }

    /// `AN_ALLOCATING_INLINED_LOOP`'s `run: count`, the JIT on whatever the
    /// environment says, every body compiled at its first entry and
    /// optimized at its first spend, run six times for 100 first, so that
    /// `run:`'s copy inlines `to:do:` and its block, then once for `count`
    /// with a scavenge asked for at every 64 KiB eden takes, the recorder
    /// on, and `hook` run wherever a frame that left its optimized code at a
    /// poll is about to go back in (`JitState::before_poll_reentry`).
    fn run_polling(count: i64, hook: Option<Hook>) -> Polled {
        run_polling_with(
            (AN_ALLOCATING_INLINED_LOOP, 10100),
            count,
            hook,
            (|_| {}, |_| {}),
        )
    }

    /// What a test runs wherever a frame that left its optimized code at a
    /// poll is about to go back in.
    type Hook = fn(&mut Mutator<'_>, &crate::jit::StoppedAtPoll);

    /// What a test sets the interpreter up with before a run.
    type SetUp = fn(&mut Interpreter);

    /// `run_polling` for `source`, whose `run: 100` answers `warmed`, the
    /// interpreter set up by `before_warming` once the module is loaded
    /// and by `before_measuring` before the measured run.
    fn run_polling_with(
        (source, warmed): (&str, i64),
        count: i64,
        hook: Option<Hook>,
        (before_warming, before_measuring): (SetUp, SetUp),
    ) -> Polled {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(module, source.into(), SourceOrigin::File("test.ns".into()))
            .unwrap_or_else(|_| panic!("the module resolves"));
        let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
        // The runs' feedback is what the compiles read: the cache stress mode
        // would vacate it at the runs' safepoints.
        #[cfg(debug_assertions)]
        interpreter.set_stress_caches(false);
        // A body the breakpoint stress mode patched is neither optimized
        // nor inlined, and its baseline code exits at every statement.
        #[cfg(debug_assertions)]
        interpreter.set_stress_breakpoints(false);
        interpreter.set_jit_enabled(true);
        interpreter.set_jit_stress(true);
        interpreter.set_optimize_stress(true);
        let class = interpreter.load_module(&resolved);
        before_warming(&mut interpreter);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let send_run = |interpreter: &mut Interpreter, count: i64| {
            let argument = interpreter.stash(crate::Value::small_integer(count));
            let answer = expect_ok(
                interpreter.send(&instance, Symbol::intern("run:"), &[argument]),
                "run:",
            );
            interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
                crate::ValueKind::SmallInteger(integer) => integer,
                _ => panic!("run:: expected a small integer"),
            })
        };
        for _ in 0..6 {
            assert_eq!(send_run(&mut interpreter, 100), warmed);
        }
        interpreter.set_scavenge_threshold(64 * 1024);
        before_measuring(&mut interpreter);
        let counted_before = interpreter.enter(|mutator| {
            mutator.jit.recorder = Some(Recorder::default());
            mutator.jit.before_poll_reentry = hook;
            mutator.heap.counted_depth()
        });
        let scavenges = interpreter.collector_statistics().scavenges;
        let before = interpreter.jit_statistics();
        let answer = send_run(&mut interpreter, count);
        let after = interpreter.jit_statistics();
        let scavenges = interpreter.collector_statistics().scavenges - scavenges;
        let (recorder, run, counted_after) = interpreter.enter(|mutator| {
            mutator.jit.before_poll_reentry = None;
            let recorder = mutator.jit.recorder.take().expect("recording");
            let code = crate::interpreter::method_code(
                mutator,
                &class,
                MethodSide::Instance,
                Symbol::intern("run:"),
            )
            .expect("the method");
            (
                recorder,
                mutator.heap.code_id(&code),
                mutator.heap.counted_depth(),
            )
        });
        Polled {
            answer,
            recorder,
            statistics: crate::jit::JitStatistics {
                poll_reentries: after.poll_reentries - before.poll_reentries,
                poll_deoptimizations: after.poll_deoptimizations - before.poll_deoptimizations,
                safepoint_exits: after.safepoint_exits - before.safepoint_exits,
                materialized_blocks: after.materialized_blocks - before.materialized_blocks,
                polls_without_an_entry: after.polls_without_an_entry
                    - before.polls_without_an_entry,
                ..after
            },
            run,
            counted: (counted_before, counted_after),
            scavenges,
        }
    }

    /// Whether the environment turns a stress mode on, each of which changes
    /// which code runs, or leaves it at every poll.
    fn a_stress_mode_is_on() -> bool {
        std::env::vars_os().any(|(name, value)| {
            name.to_string_lossy().starts_with("NS_NEXT_STRESS_")
                && !value.is_empty()
                && value != "0"
        })
    }

    /// The exits recorded inside `to:do:`'s scope inlined into `run:`: those
    /// whose innermost frame is `to:do:`'s and whose next is `run:`'s.
    fn exits_inside_to_do(recorder: &Recorder, run: CodeId) -> Vec<&RecordedExit> {
        recorder
            .exits
            .iter()
            .filter(|exit| {
                exit.inlined
                    && exit.frames.len() >= 2
                    && exit.frames[0].selector == Some(Symbol::intern("to:do:"))
                    && exit.frames[1].code == run
            })
            .collect()
    }

    /// A frame that goes back into its optimized code at the poll it left
    /// gives back what the exit counted of its inlined scopes, exactly as
    /// their pushes would have counted them: at every exit inside `to:do:`'s
    /// scope -- the first, and every one after a frame went back in -- the
    /// stack holds `run:` and `to:do:` live, the depth limit counts the two
    /// above what it counted before the run, as the interpreter counts
    /// there, and once the run is done it counts what it did before. With no
    /// stress mode the frame goes back in at every poll, so that a re-entry
    /// precedes every exit but the first; the block the copy eliminated is
    /// never made, and `run:` never goes on in its baseline code.
    #[test]
    fn a_poll_re_entry_counts_the_depth_the_interpreter_counts() {
        let Polled {
            answer,
            recorder,
            statistics,
            run,
            counted: (counted_before, counted_after),
            ..
        } = run_polling(20_000, None);
        assert_eq!(answer, 20_000 * 20_001);
        assert_eq!(counted_after, counted_before);
        let inside = exits_inside_to_do(&recorder, run);
        for exit in &inside {
            assert_eq!(exit.depth, exit.frames.len(), "every activation is live");
            assert_eq!(
                exit.counted,
                counted_before + 2,
                "the depth limit counts `run:` and the scope `to:do:`"
            );
        }
        if a_stress_mode_is_on() {
            return;
        }
        assert!(inside.len() >= 2, "{} exits inside `to:do:`", inside.len());
        assert!(statistics.poll_reentries >= 1, "{statistics:?}");
        assert_eq!(statistics.poll_deoptimizations, 0, "{statistics:?}");
        assert_eq!(statistics.materialized_blocks, 0, "{statistics:?}");
        assert!(
            recorder
                .resumptions
                .iter()
                .all(|(_, code, _, _)| *code != run),
            "`run:` never goes on in its baseline code: {:?}",
            recorder.resumptions
        );
    }

    /// Reifies the innermost activation: a frame among the scopes of the
    /// frame about to go back into its optimized code.
    fn reify_the_innermost(mutator: &mut Mutator<'_>, _stopped: &crate::jit::StoppedAtPoll) {
        if let Some(innermost) = mutator.heap.top_activation() {
            let _ = mutator.heap.reify(innermost);
        }
    }

    /// A frame that left its optimized code at a poll does not go back in
    /// once a frame among its scopes is reified, since optimized code keeps
    /// no reified word: the block the copy eliminated is made, in the frame
    /// that left, and every scope goes on in its baseline code, answering
    /// the interpreter's sum.
    #[test]
    fn a_reified_scope_s_poll_deoptimizes() {
        let Polled {
            answer,
            recorder,
            statistics,
            ..
        } = run_polling(2_000, Some(reify_the_innermost));
        assert_eq!(answer, 2_000 * 2_001);
        if a_stress_mode_is_on() {
            return;
        }
        assert!(statistics.poll_deoptimizations >= 1, "{statistics:?}");
        assert_eq!(statistics.poll_reentries, 0, "{statistics:?}");
        assert!(
            recorder
                .blocks
                .iter()
                .any(|(_, block)| !block.below && block.one_object && block.home_married),
            "the poll's frame made its block: {:?}",
            recorder.blocks
        );
    }

    /// Frees every optimized copy there is.
    fn free_every_copy(mutator: &mut Mutator<'_>, _stopped: &crate::jit::StoppedAtPoll) {
        crate::jit::invalidate_all_optimized(mutator);
    }

    /// A frame that left its optimized code at a poll does not go back in
    /// once the copy is freed -- found by its body and key, never by an
    /// address -- and goes on in baseline code with the block the copy
    /// eliminated made, answering the interpreter's sum.
    #[test]
    fn a_freed_copy_s_poll_deoptimizes() {
        let Polled {
            answer,
            recorder,
            statistics,
            ..
        } = run_polling(2_000, Some(free_every_copy));
        assert_eq!(answer, 2_000 * 2_001);
        if a_stress_mode_is_on() {
            return;
        }
        assert!(statistics.poll_deoptimizations >= 1, "{statistics:?}");
        assert_eq!(statistics.poll_reentries, 0, "{statistics:?}");
        assert!(
            recorder
                .blocks
                .iter()
                .any(|(_, block)| !block.below && block.one_object && block.home_married),
            "the poll's frame made its block: {:?}",
            recorder.blocks
        );
    }

    /// `run: count` adds 3 `count` times in a loop of its own, each turn
    /// checking that `holder`'s one element, which a collection rewrites
    /// like any field, is `current`, a young `Array` the loop keeps
    /// choosing again with no frame node between, so that the copy holds
    /// `holder` and `current` in machine registers across the loop's
    /// poll. `run: count` answers `3 * count`, and a big negative number
    /// for any turn that found `current` not what `holder` holds.
    const A_MOVED_OBJECT_ACROSS_A_POLL: &str = indoc! {"
        class Subject = Object new () (
          public run: count = (
            |
              sum ::= 0.
              index ::= 0.
              table
              holder
              current
            |
            table:: {3. 0. 0. 0}.
            holder:: {table}.
            current:: table.
            [index < count] whileTrue: [
              ((holder at: 1) == current)
                ifTrue: [sum:: sum + 3]
                ifFalse: [sum:: sum - 1000000].
              current:: index >= 0 ifTrue: [table] ifFalse: [holder].
              index:: index + 1.
            ].
            ^sum
          )
        )
    "};

    /// A frame going back into its optimized code at a poll finds the heap
    /// objects it holds in machine registers across the poll where the
    /// scavenge moved them: under the scavenge stress mode, which
    /// scavenges at every safepoint, an object a frame word the poll's
    /// entry loads a machine register from named lay elsewhere when the
    /// frame went back in than when it left, every turn finds `current`
    /// the object `holder` holds -- `nil` restored, `holder at:` would trap
    /// and fail -- and the sum is the interpreter's.
    #[test]
    fn a_moved_object_held_across_a_poll_is_the_same_after_re_entry() {
        let Polled {
            answer,
            recorder,
            statistics,
            scavenges,
            ..
        } = run_polling_with(
            (A_MOVED_OBJECT_ACROSS_A_POLL, 300),
            200,
            None,
            (|_| {}, |interpreter| interpreter.set_stress_scavenge(true)),
        );
        assert_eq!(answer, 3 * 200);
        if a_stress_mode_is_on() {
            return;
        }
        assert!(scavenges >= 2, "{scavenges} scavenges");
        assert!(statistics.poll_reentries >= 1, "{statistics:?}");
        assert!(
            recorder.reentered_objects.iter().flatten().any(
                |(left, entered)| matches!((left, entered), (Some(before), Some(after)) if before != after)
            ),
            "an object a machine register is restored with moved during the safepoint: {:?}",
            recorder.reentered_objects
        );
    }

    /// `run: limit` counts the slot `index` up to `limit` in a loop whose
    /// head's first instruction is `self done`, a send the copy inlines,
    /// each turn allocating: as `tests/jit.rs`'s program of the same name.
    /// `run: limit` answers `2 * limit - 1`.
    const A_LOOP_HEADED_BY_AN_INLINED_SEND: &str = indoc! {"
        class Subject = Object new (
          |
            index ::= 0.
            count ::= 0.
          |
        ) (
          done = (
            ^index >= count
          )

          public run: limit = (
            | last |
            index:: 0.
            count:: limit.
            [self done] whileFalse: [
              last:: {index. index. index. index. index. index. index. index}.
              index:: index + 1.
            ].
            ^index + (last at: 8)
          )
        )
    "};

    /// A poll's entry writes a value the code holds back into a frame word
    /// the collector's pre-pass nils, where the code after the poll reads
    /// it -- a clean binding of a register the leftover analysis taints at
    /// the loop's head -- and the frame that goes back in there after the
    /// scavenge stress mode's scavenges answers the interpreter's count.
    #[test]
    fn a_poll_s_entry_writes_back_what_the_collector_nilled() {
        let Polled {
            answer,
            recorder,
            statistics,
            ..
        } = run_polling_with(
            (A_LOOP_HEADED_BY_AN_INLINED_SEND, 199),
            2_000,
            None,
            (|_| {}, |interpreter| interpreter.set_stress_scavenge(true)),
        );
        assert_eq!(answer, 3_999);
        if a_stress_mode_is_on() {
            return;
        }
        assert!(statistics.poll_reentries >= 1, "{statistics:?}");
        assert!(
            recorder
                .poll_reentries
                .iter()
                .any(|(_, _, rewrote)| *rewrote),
            "an entry rewrote a lost word: {:?}",
            recorder.poll_reentries
        );
    }

    /// A frame goes back into its optimized code at the poll it left after
    /// a compaction of the code zone moved the copy: under the zone stress
    /// mode, which compacts the zone at every safepoint and every other
    /// time spares the bodies with optimized code, sliding them, a frame
    /// went back in at a copy that starts elsewhere than where it left it,
    /// the poll's entry looked up afresh, and the sum is the interpreter's.
    #[test]
    fn a_poll_re_enters_the_copy_a_compaction_moved() {
        let Polled {
            answer,
            recorder,
            statistics,
            ..
        } = run_polling_with(
            (AN_ALLOCATING_INLINED_LOOP, 10100),
            100,
            None,
            (|_| {}, |interpreter| interpreter.set_stress_zone(true)),
        );
        assert_eq!(answer, 100 * 101);
        if a_stress_mode_is_on() {
            return;
        }
        assert!(
            recorder
                .poll_reentries
                .iter()
                .any(|(exit_start, entry_start, _)| exit_start != entry_start),
            "a frame went back in at a moved copy: {:?} {statistics:?}",
            recorder.poll_reentries
        );
    }

    /// Frees every optimized copy and compiles the one the frame about to
    /// go back in ran afresh, for the same body and key: what a safepoint
    /// that frees a copy and then installs what was requested since can
    /// do. Under no stress mode the copy is installed again.
    fn install_the_copy_again(mutator: &mut Mutator<'_>, stopped: &crate::jit::StoppedAtPoll) {
        let physical = stopped.physical();
        let code = mutator.heap.activation_code(physical);
        let frame = mutator.heap.activation_receiver_and_current_class(physical);
        crate::jit::invalidate_all_optimized(mutator);
        crate::jit::request_optimization(mutator, &code, Some(frame));
        crate::jit::optimize_requested(mutator);
        let key = crate::jit::activation_key(&mutator.heap, &code, physical);
        let installed = match key {
            Some(key) => mutator.jit.copy_of(&code, key).is_some(),
            None => mutator.jit.optimized(&code).is_some(),
        };
        assert!(
            a_stress_mode_is_on() || installed,
            "the copy is installed again"
        );
    }

    /// A frame that left its optimized code at a poll does not go back in
    /// at a copy installed for the same body and key since its own was
    /// freed, whose poll of the same index is another compile's: the copy
    /// is found by its install, and the frame goes on in baseline code
    /// with the block its copy eliminated made, answering the
    /// interpreter's sum.
    #[test]
    fn a_copy_installed_again_in_its_place_is_not_entered() {
        let Polled {
            answer,
            recorder,
            statistics,
            ..
        } = run_polling(2_000, Some(install_the_copy_again));
        assert_eq!(answer, 2_000 * 2_001);
        if a_stress_mode_is_on() {
            return;
        }
        assert!(statistics.poll_deoptimizations >= 1, "{statistics:?}");
        assert_eq!(statistics.poll_reentries, 0, "{statistics:?}");
        assert!(
            recorder.blocks.iter().any(|(_, block)| !block.below),
            "the poll's frame made its block: {:?}",
            recorder.blocks
        );
    }

    /// Makes the optimized code compiled from now on give no poll an entry.
    fn refuse_poll_entries(interpreter: &mut Interpreter) {
        interpreter.enter(|mutator| mutator.jit.refuse_poll_entries = true);
    }

    /// A poll the compile gave no entry keeps the exit any interrupt point
    /// takes: the exit makes the block the copy eliminated, the frame goes
    /// on in baseline code, the exit is counted as one at a poll without
    /// an entry -- neither a re-entry nor a deoptimization of one -- and
    /// the sum is the interpreter's.
    #[test]
    fn a_poll_without_an_entry_keeps_the_full_exit() {
        let Polled {
            answer, statistics, ..
        } = run_polling_with(
            (AN_ALLOCATING_INLINED_LOOP, 10100),
            2_000,
            None,
            (refuse_poll_entries, |_| {}),
        );
        assert_eq!(answer, 2_000 * 2_001);
        if a_stress_mode_is_on() {
            return;
        }
        assert!(statistics.polls_without_an_entry >= 1, "{statistics:?}");
        assert_eq!(statistics.poll_reentries, 0, "{statistics:?}");
        assert_eq!(statistics.poll_deoptimizations, 0, "{statistics:?}");
        assert!(statistics.materialized_blocks >= 1, "{statistics:?}");
    }
}

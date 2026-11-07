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
    /// Each frame returned to that went on in its body's baseline code
    /// (`jit::resumed`): how many exits were recorded before it, its body
    /// and the program counter it went on at.
    resumptions: Vec<(usize, CodeId, u32)>,
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

/// Records the frame `activation`, returned to, going on in its body's
/// baseline code at `program_counter` (`jit::resumed`).
pub(crate) fn record_resumption(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    program_counter: u32,
) {
    let code = mutator.heap.activation_code_id(activation);
    if let Some(recorder) = mutator.jit.recorder.as_mut() {
        let exits = recorder.exits.len();
        recorder.resumptions.push((exits, code, program_counter));
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
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = crate::resolve(module, source.into(), SourceOrigin::File("test.ns".into()))
        .unwrap_or_else(|_| panic!("the module resolves"));
    let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
    // The runs' feedback is what the compiles read: the cache stress mode
    // would vacate it at the runs' safepoints.
    #[cfg(debug_assertions)]
    interpreter.set_stress_caches(false);
    interpreter.set_jit_enabled(true);
    interpreter.set_jit_stress(true);
    if deopt {
        interpreter.set_deopt_stress(true);
    } else {
        interpreter.set_optimize_stress(true);
    }
    let class = interpreter.load_module(&resolved);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
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
                .any(|(exits, code, program_counter)| *exits > index
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

/// An inlined site that only ever saw one receiver meets another: the
/// type-case traps, the trap sets the site's trapped bit, the copy is
/// retired at its second trap there, and the copy compiled again has the
/// site's send as the type-case's way for receivers without an arm, so
/// that later runs with both receivers trap there no more. Every answer
/// is the interpreter's.
#[test]
fn an_inlined_site_meeting_an_unseen_receiver_traps_and_is_recompiled_with_its_send() {
    use crate::jit::optimizing::plan::{plan, FallThrough, Limits};
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
    use crate::jit::optimizing::{tree_of_run, Speculation};

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

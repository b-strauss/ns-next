//! The baseline JIT's compile trigger and its state per interpreter
//! (`docs/design/execution.md`, "Planned: the baseline JIT", "When a body
//! is compiled"). What a compiled body computes is the goldens' to pin,
//! under both lowerings; these pin when a body is compiled, whose zone it
//! runs from, and what happens when the zone is full.
#![cfg(ns_jit)]

use indoc::{formatdoc, indoc};
use ns_runtime::{
    HostResult, HostSignal, Interpreter, JitStatistics, Lowering, Root, SourceOrigin, Symbol,
    ValueKind,
};

fn load_with(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(module, source.into(), SourceOrigin::File("test.ns".into()))
        .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    // On whatever the environment says: these pin the JIT, and the stress
    // gate runs the suite once with `NS_NEXT_NO_JIT=1`.
    interpreter.set_jit_enabled(true);
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
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

/// Instantiates `class` and sends the instance `run`, answering the
/// `SmallInteger` it answers.
fn run(interpreter: &mut Interpreter, class: &Root) -> i64 {
    let instance = expect_ok(interpreter.instantiate(class, &[]), "instantiate");
    let answer = expect_ok(
        interpreter.send(&instance, Symbol::intern("run"), &[]),
        "run",
    );
    interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("run: expected a small integer"),
    })
}

const THREE_ANSWERS: &str = indoc! {"
    class Subject = Object new () (
      answer = (
        ^14
      )
      public run = (
        ^self answer + self answer + self answer
      )
    ) : ()
"};

/// What the JIT has done since `before`.
fn since(after: JitStatistics, before: JitStatistics) -> JitStatistics {
    JitStatistics {
        compiled_bodies: after.compiled_bodies - before.compiled_bodies,
        refused_bodies: after.refused_bodies - before.refused_bodies,
        bodies_leaving_before_a_loop: after.bodies_leaving_before_a_loop
            - before.bodies_leaving_before_a_loop,
        machine_back_edges: after.machine_back_edges - before.machine_back_edges,
        safepoint_exits: after.safepoint_exits - before.safepoint_exits,
        machine_entries: after.machine_entries - before.machine_entries,
        compiled_calls: after.compiled_calls - before.compiled_calls,
        miss_leaf_calls: after.miss_leaf_calls - before.miss_leaf_calls,
        resumed_entries: after.resumed_entries - before.resumed_entries,
        materialized_frames: after.materialized_frames - before.materialized_frames,
    }
}

/// The bodies the JIT compiled or found leaving before a loop: those it
/// took up at their second entry.
fn taken_up(statistics: JitStatistics) -> u64 {
    statistics.compiled_bodies + statistics.bodies_leaving_before_a_loop
}

ns_runtime::for_each_lowering! {
    fn a_body_is_taken_up_at_its_second_entry(lowering) {
        let (mut interpreter, class) = load_with(THREE_ANSWERS, lowering);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 42);
        let done = since(interpreter.jit_statistics(), before);
        assert!(taken_up(done) >= 1, "`answer`, entered three times, is taken up");
        assert_eq!(done.refused_bodies, 0);
    }
}

ns_runtime::for_each_lowering! {
    /// The JIT stress mode's setting compiles every body at its first
    /// entry and enters machine code that leaves before a loop, so that a program
    /// runs the entry stub, the exit and the resumption on every push and
    /// still answers right. The optimized lowering's `self answer` is a
    /// cached lexical send, which `run`'s machine code makes itself, so
    /// that the interpreter pushes `answer` only where the reference
    /// lowering leaves every send uncached.
    fn the_stress_setting_enters_machine_code_on_every_push(lowering) {
        let (mut interpreter, class) = load_with(THREE_ANSWERS, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_jit_counting(true);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 42);
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized {
            assert_eq!(
                done.machine_entries, 2,
                "the two base activations the host pushes, the factory's and `run`'s, run \
                 their machine code from the first"
            );
            if !a_stress_mode_is_on() {
                assert_eq!(
                    done.miss_leaf_calls + done.compiled_calls,
                    3,
                    "`run` calls `answer`'s machine code three times, the first through the miss"
                );
            }
        } else {
            assert_eq!(
                done.machine_entries, 5,
                "`answer`, entered three times, runs its machine code from the first, and so \
                 do the two base activations the host pushes, the factory's and `run`'s"
            );
        }
        assert_eq!(done.bodies_leaving_before_a_loop, 0, "installed, not left out");
        assert!(done.compiled_bodies >= 1);
        assert_eq!(done.refused_bodies, 0);
    }
}

ns_runtime::for_each_lowering! {
    /// A body entered while the JIT is off is refused at once, so that
    /// its later pushes never reach the JIT again: a second run refuses
    /// nothing more, and takes up fewer bodies than a run of an
    /// interpreter whose JIT was never off.
    fn a_body_entered_while_the_jit_is_off_is_refused_for_good(lowering) {
        let (mut interpreter, class) = load_with(THREE_ANSWERS, lowering);
        interpreter.set_jit_enabled(false);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 42);
        let done = since(interpreter.jit_statistics(), before);
        assert!(done.refused_bodies >= 1, "`run` and `answer` are refused");
        assert_eq!(taken_up(done), 0, "the JIT is off");

        interpreter.set_jit_enabled(true);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 42);
        let revived = since(interpreter.jit_statistics(), before);
        assert_eq!(revived.refused_bodies, 0, "refused once, at the first entry");

        // The same program with the JIT on from the start: whatever else
        // either run takes up -- a kernel body entered for the first
        // time -- the control also takes up `answer`, which the first
        // interpreter refused for good.
        let (mut control, control_class) = load_with(THREE_ANSWERS, lowering);
        let before = control.jit_statistics();
        assert_eq!(run(&mut control, &control_class), 42);
        let controlled = since(control.jit_statistics(), before);
        assert!(
            taken_up(revived) < taken_up(controlled),
            "not revived by turning the JIT on"
        );
    }
}

ns_runtime::for_each_lowering! {
    fn a_second_interpreter_takes_up_its_own_bodies(lowering) {
        let (mut first, first_class) = load_with(THREE_ANSWERS, lowering);
        let (mut second, second_class) = load_with(THREE_ANSWERS, lowering);
        assert_eq!(run(&mut first, &first_class), 42);
        assert_eq!(run(&mut first, &first_class), 42);
        let before = second.jit_statistics();
        assert_eq!(run(&mut second, &second_class), 42);
        let done = since(second.jit_statistics(), before);
        assert!(taken_up(done) >= 1, "the second interpreter takes up `answer` itself");
    }
}

ns_runtime::for_each_lowering! {
    /// A program with more bodies than a zone of one page holds, each
    /// starting with a template and installed by the stress setting, fills
    /// the zone, and the bodies refused once it is full run interpreted
    /// with the output unchanged.
    fn a_zone_of_one_page_leaves_the_output_right(lowering) {
        let count: i64 = 300;
        let methods: String = (0..count)
            .map(|index| format!("  m{index} = (\n    ^{index}\n  )\n"))
            .collect();
        let sends: String = (0..count)
            .map(|index| format!("    sum:: sum + self m{index} + self m{index}.\n"))
            .collect();
        let source = formatdoc! {"
            class Subject = Object new () (
            {methods}  public run = (
                | sum |
                sum:: 0.
            {sends}    ^sum
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(&source, lowering);
        interpreter.set_jit_zone_bytes(4096);
        interpreter.set_jit_stress(true);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), count * (count - 1));
        assert!(since(interpreter.jit_statistics(), before).refused_bodies >= 1, "the page fills");
    }
}

/// Instantiates `class`, sends the instance `run` and answers what it
/// answers as the run loop's errors describe a value, each element of an
/// `Array` on its own.
fn run_described(interpreter: &mut Interpreter, class: &Root) -> Vec<String> {
    let instance = expect_ok(interpreter.instantiate(class, &[]), "instantiate");
    let answer = expect_ok(
        interpreter.send(&instance, Symbol::intern("run"), &[]),
        "run",
    );
    interpreter.enter(|mutator| {
        let value = mutator.fetch(&answer);
        let heap = mutator.heap();
        match value.kind() {
            ValueKind::Object(object) => match heap.array_elements(object) {
                Some(elements) => elements
                    .into_iter()
                    .map(|element| heap.describe(element))
                    .collect(),
                None => vec![heap.describe(value)],
            },
            _ => vec![heap.describe(value)],
        }
    })
}

ns_runtime::for_each_lowering! {
    /// `goldens/runtime/jit/counting_loop.ns`'s loop, compiled at its
    /// first entry: its moves, constants, compares, adds and jumps are
    /// templates, so the loop runs in machine code, every iteration a
    /// back edge. The reference lowering sends `whileTrue:`, whose loop
    /// starts with a send and so runs interpreted.
    fn a_counting_loop_runs_in_machine_code(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              public run = (
                |
                  sum ::= 0.
                  index ::= 1.
                |
                [index <= 1000000] whileTrue: [
                  sum:: sum + index.
                  index:: index + 1.
                ].
                ^sum.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_jit_counting(true);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 500_000_500_000);
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized {
            assert!(
                done.machine_back_edges > 0,
                "the loop closes in machine code"
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// The default trigger, without the stress setting: `run`'s counting
    /// loop is all templates, so its code can jump back before it leaves,
    /// and the second run -- `run`'s second entry -- compiles it and enters
    /// its machine code. The reference lowering's loop starts with a send,
    /// so there `run` leaves before a loop and is never entered.
    fn a_body_that_loops_is_entered_at_its_second_entry(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              public run = (
                |
                  sum ::= 0.
                  index ::= 1.
                |
                [index <= 1000] whileTrue: [
                  sum:: sum + index.
                  index:: index + 1.
                ].
                ^sum.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        assert_eq!(run(&mut interpreter, &class), 500_500);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 500_500);
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized {
            assert!(done.machine_entries > 0, "the loop runs in machine code");
            assert!(done.compiled_bodies >= 1, "`run`, at its second entry");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// `goldens/runtime/jit/overflow.ns` in brief: the largest
    /// `SmallInteger` plus one, inside a loop body run compiled, declines
    /// to the send, which answers the `LargeInteger`.
    fn integer_overflow_in_machine_code_answers_a_large_integer(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              public run = (
                |
                  value ::= 1152921504606846973.
                  index ::= 0.
                |
                [index < 4] whileTrue: [
                  value:: value + 1.
                  index:: index + 1.
                ].
                ^value.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        interpreter.set_jit_stress(true);
        assert_eq!(
            run_described(&mut interpreter, &class),
            ["1152921504606846977"]
        );
    }
}

ns_runtime::for_each_lowering! {
    /// Templated moves, then a send, at which the machine code exits, then
    /// more moves, which the interpreter runs: the frame the exit hands
    /// over is the interpreter's own, so every local the program reads
    /// afterwards is what the interpreter alone computes.
    fn a_body_exiting_mid_way_matches_the_interpreter(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              triple: value = (
                ^value * 3.
              )
              public run = (
                |
                  first ::= 5.
                  second ::= first.
                  third
                  fourth
                  fifth ::= true.
                |
                third:: self triple: second.
                fourth:: third.
                second:: nil.
                ^{first. second. third. fourth. fifth}.
              )
            ) : ()
        "};
        let (mut compiled, compiled_class) = load_with(source, lowering);
        compiled.set_jit_stress(true);
        let before = compiled.jit_statistics();
        let in_machine_code = run_described(&mut compiled, &compiled_class);
        assert!(since(compiled.jit_statistics(), before).machine_entries >= 1);

        let (mut interpreted, interpreted_class) = load_with(source, lowering);
        interpreted.set_jit_enabled(false);
        let before = interpreted.jit_statistics();
        let by_the_interpreter = run_described(&mut interpreted, &interpreted_class);
        assert_eq!(since(interpreted.jit_statistics(), before).machine_entries, 0);

        assert_eq!(in_machine_code, by_the_interpreter);
        assert_eq!(in_machine_code, ["5", "nil", "15", "15", "true"]);
    }
}

ns_runtime::for_each_lowering! {
    /// Each iteration allocates an `Array` through the generic leaf, and
    /// the allocations fill eden, whose request for a scavenge only a back
    /// edge's poll can notice in a loop that makes no send: the machine
    /// code exits there, and the interpreter takes the safepoint and runs
    /// the rest of the loop.
    fn a_back_edge_poll_takes_the_safepoint(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              public run = (
                |
                  index ::= 0.
                  last
                |
                [index < 100000] whileTrue: [
                  last:: {index. index. index. index. index. index. index. index. index. index. index. index. index. index. index. index}.
                  index:: index + 1.
                ].
                ^(last at: 16) + index.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        interpreter.set_jit_stress(true);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 199_999);
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized {
            assert!(done.safepoint_exits >= 1, "a back edge found the scavenge requested");
        }
    }
}

/// A counting loop all of templates, so that its body is entered at its
/// second entry wherever the thread has the room.
const COUNTING_LOOP: &str = indoc! {"
    class Subject = Object new () (
      public run = (
        |
          sum ::= 0.
          index ::= 1.
        |
        [index <= 1000] whileTrue: [
          sum:: sum + index.
          index:: index + 1.
        ].
        ^sum.
      )
    ) : ()
"};

/// Runs `COUNTING_LOOP` twice on a thread of `stack_bytes`, answering the
/// second run's answer and its entries into machine code.
fn run_the_loop_on_a_thread_of(stack_bytes: usize) -> (i64, u64) {
    std::thread::Builder::new()
        .stack_size(stack_bytes)
        .spawn(|| {
            let (mut interpreter, class) = load_with(COUNTING_LOOP, Lowering::Optimized);
            assert_eq!(run(&mut interpreter, &class), 500_500);
            let before = interpreter.jit_statistics();
            let answer = run(&mut interpreter, &class);
            (
                answer,
                since(interpreter.jit_statistics(), before).machine_entries,
            )
        })
        .expect("a thread")
        .join()
        .expect("the run finished")
}

/// A compiled frame's return address lies on the native stack, so an
/// entry first checks that the thread has the room the depth limit can
/// take (`jit::NATIVE_STACK_NEED`, about 0.86 MB): a thread of 512 KiB
/// runs the same program interpreted, with the same answer, where one of
/// 8 MiB, the size `ns-next` runs programs on, enters machine code.
#[test]
fn a_thread_with_too_little_stack_runs_interpreted() {
    let (answer, entries) = run_the_loop_on_a_thread_of(512 << 10);
    assert_eq!(answer, 500_500);
    assert_eq!(entries, 0, "no room for the compiled frames");
    let (answer, entries) = run_the_loop_on_a_thread_of(8 << 20);
    assert_eq!(answer, 500_500);
    assert!(entries > 0, "the loop runs in machine code");
}

ns_runtime::for_each_lowering! {
    /// A loop sending `add:` to an object whose `add:` adds: the send's
    /// cell names the callee's machine entry once the miss has compiled
    /// it, so the loop's calls reach machine code by `call` and come back
    /// by `ret`, and the sum is the interpreter's. The reference
    /// lowering's sends are uncached and exit.
    fn a_compiled_send_calls_compiled_code(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              class Adder = () (
                public add: value = (
                  ^value + 1
                )
                public sumWith: adder = (
                  |
                    sum ::= 0.
                    index ::= 0.
                  |
                  [index < 10000] whileTrue: [
                    sum:: sum + (adder add: index).
                    index:: index + 1.
                  ].
                  ^sum.
                )
              )
              public run = (
                |
                  adder = Adder new.
                |
                ^adder sumWith: adder.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_jit_counting(true);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 50_005_000);
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized {
            // A stress mode that exits at every poll, or that holds the
            // safepoint request so that every compiled push is refused,
            // leaves the loop to the interpreter after its first round.
            let expected = if a_stress_mode_is_on() { 0 } else { 9_000 };
            assert!(
                done.compiled_calls >= expected,
                "the loop's sends call compiled code: {}",
                done.compiled_calls
            );
            assert!(done.miss_leaf_calls >= 1, "the first send missed");
        }
    }
}

/// Whether the environment turns a stress mode on (`NS_NEXT_STRESS_*`
/// set, and neither empty nor `0`), under which machine code leaves for
/// the interpreter far more often than a program needs it to.
fn a_stress_mode_is_on() -> bool {
    std::env::vars_os().any(|(name, value)| {
        name.to_string_lossy().starts_with("NS_NEXT_STRESS_") && !value.is_empty() && value != "0"
    })
}

/// A chain of compiled sends three deep -- `run:`, `first:with:`,
/// `second:with:`, `third:with:` -- whose innermost body sends a message
/// nothing understands,
/// unhandled: the exit at that send materializes every compiled frame, so
/// the stack the unhandled exception ends the run on reads as the
/// interpreter's.
const THREE_DEEP: &str = indoc! {"
    class Subject = Object new () (
      class Chain = () (
        public first: value with: chain = (
          ^(chain second: value + 1 with: chain) + 1
        )
        public second: value with: chain = (
          ^(chain third: value + 1 with: chain) + 1
        )
        public third: value with: chain = (
          |
            index ::= 0.
          |
          [index < value] whileTrue: [index:: index + 1].
          index > 100 ifTrue: [^nil frobnicate].
          ^index
        )
        public run: chain = (
          |
            sum ::= 0.
            index ::= 0.
          |
          [index < 200] whileTrue: [
            sum:: sum + (chain first: index with: chain).
            index:: index + 1.
          ].
          ^sum.
        )
      )
      public run = (
        |
          chain = Chain new.
        |
        ^chain run: chain.
      )
    ) : ()
"};

/// The stack `THREE_DEEP`'s unhandled exception ended the run on, with the
/// JIT on (under its stress setting, so that every body is compiled at its
/// first entry) or off.
fn three_deep_stack(lowering: Lowering, jit: bool) -> (Vec<String>, JitStatistics) {
    let (mut interpreter, class) = load_with(THREE_DEEP, lowering);
    interpreter.set_jit_enabled(jit);
    interpreter.set_jit_stress(jit);
    let before = interpreter.jit_statistics();
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    match interpreter.send(&instance, Symbol::intern("run"), &[]) {
        Err(HostSignal::Error(error)) => {
            assert!(error.message.contains("frobnicate"), "{}", error.message);
        }
        _ => panic!("run: expected the unhandled error"),
    }
    let done = since(interpreter.jit_statistics(), before);
    (interpreter.stack_trace(), done)
}

ns_runtime::for_each_lowering! {
    fn an_exit_in_a_callee_materializes_every_frame(lowering) {
        let (compiled, statistics) = three_deep_stack(lowering, true);
        let (interpreted, _) = three_deep_stack(lowering, false);
        assert_eq!(compiled, interpreted);
        assert!(
            compiled.iter().any(|frame| frame.starts_with("third:with:")),
            "the innermost body's frame: {compiled:?}"
        );
        assert!(
            compiled.iter().any(|frame| frame.starts_with("run:")),
            "the entry's frame: {compiled:?}"
        );
        if lowering == Lowering::Optimized {
            assert!(
                statistics.materialized_frames >= 3,
                "`first:with:`, `second:with:` and `third:with:` were compiled frames: {}",
                statistics.materialized_frames
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// `step:`'s send of `printString`, a primitive machine code does not
    /// run, leaves machine code inside the callee at every call: the exit
    /// materializes `loop:`'s frame too, and once `step:` returns, `loop:`
    /// is resumed in its machine code, so that its loop goes on there
    /// rather than in the interpreter.
    fn a_returned_to_frame_resumes_in_machine_code(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              class Stepper = () (
                public step: value = (
                  ^value printString size
                )
                public loop: stepper = (
                  |
                    sum ::= 0.
                    index ::= 0.
                  |
                  [index < 1000] whileTrue: [
                    sum:: sum + (stepper step: index).
                    index:: index + 1.
                  ].
                  ^sum.
                )
              )
              public run = (
                |
                  stepper = Stepper new.
                |
                ^stepper loop: stepper.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_jit_counting(true);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 2890);
        let done = since(interpreter.jit_statistics(), before);
        // The exit stress mode leaves at every back edge, so that `loop:`'s
        // loop goes on in the interpreter after its first exit.
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                done.resumed_entries >= 900,
                "`loop:` resumed in machine code after each call: {}",
                done.resumed_entries
            );
            assert!(
                done.machine_back_edges >= 900,
                "its loop closes in machine code: {}",
                done.machine_back_edges
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// The default trigger, without the stress setting: `add:` has run
    /// once, interpreted, when `sumWith:count:`'s machine code first sends
    /// it, so the miss leaf counts its second entry, compiles it and
    /// installs it from inside the entry, and the send calls it; every
    /// later send of the loop calls it through the cell the miss rewrote,
    /// and the interpreter enters no other body.
    fn a_callee_the_miss_compiles_is_entered_at_its_next_call(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              class Adder = () (
                public add: value = (
                  |
                    index ::= 0.
                  |
                  [index < 1] whileTrue: [index:: index + 1].
                  ^value + index
                )
                public sumWith: adder count: count = (
                  |
                    sum ::= 0.
                    index ::= 0.
                  |
                  [index < count] whileTrue: [
                    sum:: sum + (adder add: index).
                    index:: index + 1.
                  ].
                  ^sum.
                )
              )
              public run: count = (
                |
                  adder = Adder new.
                |
                ^adder sumWith: adder count: count.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        interpreter.set_jit_counting(true);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let run_with = |interpreter: &mut Interpreter, count: i64| {
            let count = interpreter.enter(|mutator| {
                mutator.stash(ns_runtime::Value::small_integer(count))
            });
            let answer = expect_ok(
                interpreter.send(&instance, Symbol::intern("run:"), &[count]),
                "run:",
            );
            interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
                ValueKind::SmallInteger(integer) => integer,
                _ => panic!("run: expected a small integer"),
            })
        };
        assert_eq!(run_with(&mut interpreter, 1), 1);
        let before = interpreter.jit_statistics();
        assert_eq!(run_with(&mut interpreter, 10_000), 50_005_000);
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(done.compiled_bodies >= 2, "`sumWith:count:`, then `add:` by the miss");
            assert_eq!(done.miss_leaf_calls, 1, "the one send that found no machine entry");
            assert!(done.compiled_calls >= 9_000, "{}", done.compiled_calls);
            assert_eq!(done.machine_entries, 1, "only `sumWith:count:` is entered by the loop");
        }
    }
}

/// Implicit-receiver sends at lexical depths 0 to 3 -- `self` included --
/// a private one and an explicit `outer` one, each callee's body taking
/// its argument, so that it has no fast path and a compiled call enters
/// its machine code; a second `Middle` makes the depth-1 sites see
/// another enclosing object.
const LEXICAL_DEPTHS: &str = indoc! {"
    class Subject = Object new () (
      class Outer = () (
        class Middle offset: offset = (
          |
            middleOffset = offset.
          |
        ) (
          class Inner = () (
            atOwn: value = (
              ^value + 1
            )
            private secret: value = (
              ^value + 1000000
            )
            public sumOver: count = (
              |
                total ::= 0.
                index ::= 0.
              |
              [index < count] whileTrue: [
                total:: total + (atOwn: index) + (self atOwn: index)
                    + (atMiddle: index) + (outer Middle atMiddle: index)
                    + (atOuter: index) + (atModule: index) + (secret: index).
                index:: index + 1.
              ].
              ^total.
            )
          )
          atMiddle: value = (
            ^value + middleOffset
          )
          public inner = (
            ^Inner new
          )
        )
        atOuter: value = (
          ^value + 100
        )
        public middle: offset = (
          ^Middle offset: offset
        )
      )
      atModule: value = (
        ^value + 1000
      )
      public run = (
        |
          container = Outer new.
        |
        ^((container middle: 10) inner sumOver: 1000)
            + ((container middle: 20) inner sumOver: 1000)
      )
    ) : ()
"};

/// `source`'s `run` with the JIT on, every body compiled at its first
/// entry and the compiled calls counted, or with the JIT off; and what
/// the JIT did meanwhile.
fn run_compiled(source: &str, lowering: Lowering, jit: bool) -> (i64, JitStatistics) {
    let (mut interpreter, class) = load_with(source, lowering);
    interpreter.set_jit_enabled(jit);
    interpreter.set_jit_stress(jit);
    interpreter.set_jit_counting(jit);
    let before = interpreter.jit_statistics();
    let answer = run(&mut interpreter, &class);
    (answer, since(interpreter.jit_statistics(), before))
}

ns_runtime::for_each_lowering! {
    /// Every lexical send of `sumOver:`'s loop calls its callee through
    /// its site's cell once the miss leaf has decided its first send --
    /// seven sends an iteration, two thousand iterations -- and the sum is
    /// the interpreter's.
    fn a_lexical_send_in_machine_code_matches_the_interpreter_at_every_depth(lowering) {
        let (compiled, statistics) = run_compiled(LEXICAL_DEPTHS, lowering, true);
        let (interpreted, _) = run_compiled(LEXICAL_DEPTHS, lowering, false);
        assert_eq!(interpreted, 2_009_257_000);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                statistics.compiled_calls >= 13_000,
                "the lexical sends call through their cells: {}",
                statistics.compiled_calls
            );
        }
    }
}

/// A mixin applied twice in one chain, whose method's `super` send starts
/// at the Middle level from the upper application and at Base from the
/// lower one, and whose `outer` send reaches the module from both.
const SUPER_AT_TWO_LEVELS: &str = indoc! {"
    class Subject = Object new () (
      class Base = Object new () (
        public step: value = (
          ^value + 1
        )
      )
      class Marker = Object new () (
        public step: value = (
          ^(super step: value) * 2 + (bonus: value)
        )
      )
      class Middle = Object new () (
        public step: value = (
          ^(super step: value) + 1000
        )
      )
      class Twice = Base new <: Marker <: Middle <: Marker () ()
      bonus: value = (
        ^value + 3
      )
      sumWith: twice = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 1000] whileTrue: [
          sum:: sum + (twice step: index).
          index:: index + 1.
        ].
        ^sum
      )
      public run = (
        ^self sumWith: Twice new
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// `Marker`'s `bonus:` send calls its callee through its cell at both
    /// of its levels, and `Middle`'s `super step:` through its own; the
    /// `super` site of `Marker`, which sees two superclasses, is
    /// polymorphic and decided by the miss leaf at every send. The sum is
    /// the interpreter's.
    fn a_super_send_in_machine_code_matches_the_interpreter_at_both_levels(lowering) {
        let (compiled, statistics) = run_compiled(SUPER_AT_TWO_LEVELS, lowering, true);
        let (interpreted, _) = run_compiled(SUPER_AT_TWO_LEVELS, lowering, false);
        assert_eq!(interpreted, 5_509_500);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                statistics.compiled_calls >= 3_900,
                "`twice step:`, `Middle`'s `super` send and `Marker`'s `bonus:` call through                  their cells: {}",
                statistics.compiled_calls
            );
        }
    }
}

/// Three compiled frames deep, `third:` allocates in a loop: under the
/// scavenge stress mode its back edge polls a held request, exits, the
/// exit materializes `second:`, `first:` and `third:` above `run:`, and
/// the scavenge -- its remembered set and cells verified first -- traces
/// them; each frame goes on, and the sum is the interpreter's.
#[cfg(debug_assertions)]
const COLLECTED_DEEP: &str = indoc! {"
    class Subject = Object new () (
      class Chain = () (
        public first: value with: chain = (
          ^(chain second: value with: chain) + 1
        )
        public second: value with: chain = (
          ^(chain third: value with: chain) + 1
        )
        public third: value with: chain = (
          |
            index ::= 0.
            sum ::= 0.
            pair
          |
          [index < 20] whileTrue: [
            pair:: {index. value}.
            sum:: sum + (pair at: 1) + (pair at: 2).
            index:: index + 1.
          ].
          ^sum
        )
        public run: chain = (
          |
            sum ::= 0.
            index ::= 0.
          |
          [index < 30] whileTrue: [
            sum:: sum + (chain first: index with: chain).
            index:: index + 1.
          ].
          ^sum.
        )
      )
      public run = (
        |
          chain = Chain new.
        |
        ^chain run: chain.
      )
    ) : ()
"};

/// `COLLECTED_DEEP` under the scavenge stress mode, with the JIT on (its
/// stress setting) or off: the answer and what the JIT did.
#[cfg(debug_assertions)]
fn collected_deep(lowering: Lowering, jit: bool) -> (i64, JitStatistics) {
    let (mut interpreter, class) = load_with(COLLECTED_DEEP, lowering);
    interpreter.set_jit_enabled(jit);
    interpreter.set_jit_stress(jit);
    interpreter.set_stress_scavenge(true);
    let before = interpreter.jit_statistics();
    let answer = run(&mut interpreter, &class);
    interpreter.set_stress_scavenge(false);
    (answer, since(interpreter.jit_statistics(), before))
}

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    fn a_scavenge_inside_a_three_deep_compiled_stretch_matches_the_interpreter(lowering) {
        let (compiled, statistics) = collected_deep(lowering, true);
        let (interpreted, _) = collected_deep(lowering, false);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized {
            assert!(statistics.safepoint_exits >= 1, "a back edge took the held request");
            assert!(
                statistics.materialized_frames >= 3,
                "the stretch was three compiled frames deep: {}",
                statistics.materialized_frames
            );
        }
    }
}

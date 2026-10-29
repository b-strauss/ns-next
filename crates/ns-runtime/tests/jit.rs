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
        send_exits: after.send_exits - before.send_exits,
        act_leaf_calls: after.act_leaf_calls - before.act_leaf_calls,
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

/// Whether the environment turns on a stress mode under which every poll
/// leaves machine code: the exit stress mode, whose polls exit
/// unconditionally, and every mode that holds the safepoint request,
/// which a poll takes (`Interpreter::hold_safepoints_for_stress`). The
/// JIT's and the wide encoding's stress modes leave the polls alone.
fn a_stress_mode_polls_out() -> bool {
    ["EXIT", "MOVES", "CACHES", "COLLECT", "SCAVENGE", "SUSPEND"]
        .iter()
        .any(|mode| {
            std::env::var_os(format!("NS_NEXT_STRESS_{mode}"))
                .is_some_and(|value| !value.is_empty() && value != "0")
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
    /// `step:`'s block, a closure machine code does not make yet, leaves
    /// machine code inside the callee at every call: the exit
    /// materializes `loop:`'s frame too, and once `step:` returns, `loop:`
    /// is resumed in its machine code, so that its loop goes on there
    /// rather than in the interpreter.
    fn a_returned_to_frame_resumes_in_machine_code(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              class Stepper = () (
                public step: value = (
                  ^[value printString size] value
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
        // A stress mode whose polls leave at every back edge sends `loop:`'s
        // loop on in the interpreter after its first exit.
        if lowering == Lowering::Optimized && !a_stress_mode_polls_out() {
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
                "`twice step:`, `Middle`'s `super` send and `Marker`'s `bonus:` call through \
                 their cells: {}",
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

/// One site, `sendStep:to:`'s, sent receivers of `count` classes in turn
/// from `sumWith:`'s loop, each class's `step:` its argument plus a
/// distinct power of ten, so that a receiver sent the wrong method changes
/// the sum.
fn polymorphic_source(count: usize) -> String {
    let names = ["First", "Second", "Third", "Fourth", "Fifth", "Sixth"];
    let classes: String = names[..count]
        .iter()
        .zip([1, 10, 100, 1000, 10_000, 100_000])
        .map(|(name, offset)| {
            formatdoc! {"
                  class {name} = () (
                    public step: value = (
                      ^value + {offset}
                    )
                  )
            "}
        })
        .collect();
    let keyword = |position: usize| if position == 0 { "sumWith" } else { "with" };
    let parameters: Vec<String> = names[..count]
        .iter()
        .enumerate()
        .map(|(position, name)| format!("{}: {}", keyword(position), name.to_lowercase()))
        .collect();
    let sends: Vec<String> = names[..count]
        .iter()
        .map(|name| format!("(sendStep: {} to: index)", name.to_lowercase()))
        .collect();
    let arguments: Vec<String> = names[..count]
        .iter()
        .enumerate()
        .map(|(position, name)| format!("{}: {name} new", keyword(position)))
        .collect();
    formatdoc! {"
        class Subject = Object new () (
        {classes}  sendStep: receiver to: value = (
            ^receiver step: value
          )
          {parameters} = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < 1000] whileTrue: [
              sum:: sum + {sends}.
              index:: index + 1.
            ].
            ^sum.
          )
          public run = (
            ^{arguments}
          )
        ) : ()
        ",
        parameters = parameters.join(" "),
        sends = sends.join(" + "),
        arguments = arguments.join(" "),
    }
}

/// `source`'s `run` twice with the JIT on, every body compiled at its
/// first entry and the compiled calls counted, or with the JIT off: the
/// answers, and what the JIT did in each run.
fn run_twice_compiled(
    source: &str,
    lowering: Lowering,
    jit: bool,
) -> ([i64; 2], [JitStatistics; 2]) {
    let (mut interpreter, class) = load_with(source, lowering);
    interpreter.set_jit_enabled(jit);
    interpreter.set_jit_stress(jit);
    interpreter.set_jit_counting(jit);
    let before = interpreter.jit_statistics();
    let first = run(&mut interpreter, &class);
    let middle = interpreter.jit_statistics();
    let second = run(&mut interpreter, &class);
    let after = interpreter.jit_statistics();
    (
        [first, second],
        [since(middle, before), since(after, middle)],
    )
}

/// `polymorphic_source(count)` compiled and interpreted: the answers are
/// the interpreter's; in the first run the polymorphic site meets the
/// miss leaf once per receiver class, beside `run`'s first send, which
/// leaves machine code, and the first send of each of `sumWith:`'s
/// `count` sites; and in the second, which sends each class a thousand
/// times more, not at all -- every send hits in machine code -- while
/// `sumWith:` calls `sendStep:to:`'s machine code at every send.
fn assert_hits_in_machine_code(count: usize, lowering: Lowering) {
    let source = polymorphic_source(count);
    let (compiled, statistics) = run_twice_compiled(&source, lowering, true);
    let (interpreted, _) = run_twice_compiled(&source, lowering, false);
    let offsets: i64 = [1, 10, 100, 1000, 10_000, 100_000][..count].iter().sum();
    let classes = i64::try_from(count).expect("a few classes");
    let expected = classes * 499_500 + 1000 * offsets;
    assert_eq!(interpreted, [expected, expected]);
    assert_eq!(compiled, interpreted);
    if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
        let [first, second] = statistics;
        let classes = classes.unsigned_abs();
        assert_eq!(
            first.miss_leaf_calls,
            classes + 1 + classes,
            "the polymorphic site met the miss leaf once per class"
        );
        assert_eq!(
            second.miss_leaf_calls, 0,
            "every send of the second run hit in machine code"
        );
        assert!(
            second.compiled_calls >= 1000 * classes,
            "`sumWith:` calls `sendStep:to:` through its cells: {}",
            second.compiled_calls
        );
    }
}

ns_runtime::for_each_lowering! {
    /// Four receiver classes at one site: its block of four cells, which
    /// the polymorphic stub scans.
    fn a_polymorphic_site_hits_in_machine_code(lowering) {
        assert_hits_in_machine_code(4, lowering);
    }
}

ns_runtime::for_each_lowering! {
    /// Six receiver classes at one site, which has gone megamorphic: the
    /// global cache's mirror, which the megamorphic stub probes.
    fn a_megamorphic_site_hits_in_machine_code(lowering) {
        assert_hits_in_machine_code(6, lowering);
    }
}

/// A mixin applied over six superclasses, whose one compiled `step:`
/// makes a `super` send to six S, a private lexical send under six N and
/// a public lexical one to receivers of six classes: three megamorphic
/// sites, keyed by S's application, by N's, and by the receiver's with
/// N's in the rule.
const MARKED_SIX: &str = indoc! {"
    class Subject = Object new () (
      class First = () (
        public step: value = (
          ^value + 1
        )
      )
      class Second = () (
        public step: value = (
          ^value + 10
        )
      )
      class Third = () (
        public step: value = (
          ^value + 100
        )
      )
      class Fourth = () (
        public step: value = (
          ^value + 1000
        )
      )
      class Fifth = () (
        public step: value = (
          ^value + 10000
        )
      )
      class Sixth = () (
        public step: value = (
          ^value + 100000
        )
      )
      class Marker = () (
        private secret: value = (
          ^value * 2
        )
        public bump: value = (
          ^value + 7
        )
        public step: value = (
          ^(super step: value) + (secret: value) + (bump: value)
        )
      )
      class MarkedFirst = First new <: Marker () ()
      class MarkedSecond = Second new <: Marker () ()
      class MarkedThird = Third new <: Marker () ()
      class MarkedFourth = Fourth new <: Marker () ()
      class MarkedFifth = Fifth new <: Marker () ()
      class MarkedSixth = Sixth new <: Marker () ()
      sumWith: first with: second with: third with: fourth with: fifth with: sixth = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 1000] whileTrue: [
          sum:: sum + (first step: index) + (second step: index) + (third step: index)
              + (fourth step: index) + (fifth step: index) + (sixth step: index).
          index:: index + 1.
        ].
        ^sum.
      )
      public run = (
        ^sumWith: MarkedFirst new with: MarkedSecond new with: MarkedThird new
            with: MarkedFourth new with: MarkedFifth new with: MarkedSixth new
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// `Marker`'s `super`, private and public lexical sites, each
    /// megamorphic, hit in machine code: the megamorphic stub finds S's
    /// and N's dispatch ids from their application words, and the second
    /// run meets the miss leaf nowhere.
    fn a_megamorphic_super_or_lexical_site_hits_in_machine_code(lowering) {
        let (compiled, statistics) = run_twice_compiled(MARKED_SIX, lowering, true);
        let (interpreted, _) = run_twice_compiled(MARKED_SIX, lowering, false);
        // Per index 4 index + 7 plus its class's offset, for six classes.
        let expected = 6 * (4 * 499_500 + 7000) + 111_111_000;
        assert_eq!(interpreted, [expected, expected]);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            let [_, second] = statistics;
            assert_eq!(
                second.miss_leaf_calls, 0,
                "every send of the second run hit in machine code"
            );
            assert!(
                second.compiled_calls >= 6000,
                "`sumWith:with:with:with:with:with:` calls `step:` through its cells: {}",
                second.compiled_calls
            );
        }
    }
}

/// `run` sent three times to one instance of `source`'s `Subject`, with the
/// JIT on -- every body compiled at its first entry, the compiled calls
/// counted -- or off: the answers, and what the JIT did in the third run,
/// once every site the program reaches was filled by the two before. The
/// instance is made once, so that no factory's exit falls into the runs.
fn run_thrice_on_one_instance(
    source: &str,
    lowering: Lowering,
    jit: bool,
) -> ([i64; 3], JitStatistics) {
    let (mut interpreter, class) = load_with(source, lowering);
    interpreter.set_jit_enabled(jit);
    interpreter.set_jit_stress(jit);
    interpreter.set_jit_counting(jit);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let mut answers = [0; 3];
    let mut before = interpreter.jit_statistics();
    for answer in &mut answers {
        before = interpreter.jit_statistics();
        let result = expect_ok(
            interpreter.send(&instance, Symbol::intern("run"), &[]),
            "run",
        );
        *answer = interpreter.enter(|mutator| match mutator.fetch(&result).kind() {
            ValueKind::SmallInteger(integer) => integer,
            _ => panic!("run: expected a small integer"),
        });
    }
    (answers, since(interpreter.jit_statistics(), before))
}

/// A getter sent a thousand times from a compiled loop, and a quick
/// method of the kernel's -- `yourself`, answering the receiver -- beside
/// it: its cell names the getter stub, which loads the field.
const GETTERS: &str = indoc! {"
    class Subject = Object new (
      | holder = Holder new. |
    ) (
      class Holder = Object new (
        | public count ::= 3. |
      ) ()
      sumOver: aHolder = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 1000] whileTrue: [
          sum:: sum + aHolder yourself count.
          index:: index + 1.
        ].
        ^sum.
      )
      public run = (
        ^sumOver: holder
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A public getter runs in its stub: the third run's thousand sends of
    /// `count` and `yourself` leave machine code for nothing and call no
    /// leaf, and the sum is the interpreter's.
    fn a_getter_runs_in_its_stub(lowering) {
        let (compiled, statistics) = run_thrice_on_one_instance(GETTERS, lowering, true);
        let (interpreted, _) = run_thrice_on_one_instance(GETTERS, lowering, false);
        assert_eq!(interpreted, [3000, 3000, 3000]);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(statistics.send_exits, 0, "every send stayed in machine code");
            assert_eq!(statistics.act_leaf_calls, 0, "the getter and `yourself` called no leaf");
            assert_eq!(statistics.miss_leaf_calls, 0);
            assert!(
                statistics.compiled_calls >= 2000,
                "`sumOver:` calls through its cells: {}",
                statistics.compiled_calls
            );
        }
    }
}

/// `Array>>do:` over a thousand elements, filled by `at:put:` first: the
/// size is `do:`'s `SendMirror` of a leaf-safe row, each element `at:`, a
/// kernel method forwarding to one, and each `value:` a compiled method
/// whose getter and setter run in their stubs.
const ROWS: &str = indoc! {"
    class Subject = Object new (
      | elements = Array new: 1000. summer = Summer new. |
    ) (
      class Summer = Object new (
        | public total ::= 0. |
      ) (
        public value: element = (
          self total: self total + element.
        )
      )
      fill: array summing: aSummer = (
        |
          index ::= 1.
        |
        [index <= 1000] whileTrue: [
          array at: index put: index.
          index:: index + 1.
        ].
        aSummer total: 0.
        array do: aSummer.
        ^aSummer total.
      )
      public run = (
        ^fill: elements summing: summer
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A leaf-safe row runs without an exit: in the third run `do:`,
    /// `at:`, `at:put:`, `value:` and the accessors all stay in machine
    /// code, and the sum is the interpreter's.
    fn a_leaf_row_runs_without_an_exit(lowering) {
        let (compiled, statistics) = run_thrice_on_one_instance(ROWS, lowering, true);
        let (interpreted, _) = run_thrice_on_one_instance(ROWS, lowering, false);
        assert_eq!(interpreted, [500_500, 500_500, 500_500]);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(statistics.send_exits, 0, "every send stayed in machine code");
            assert_eq!(statistics.miss_leaf_calls, 0);
            assert!(
                statistics.act_leaf_calls >= 3000,
                "`at:`, `at:put:` and the setter ran through the leaf stub: {}",
                statistics.act_leaf_calls
            );
        }
    }
}

/// The rows marked `MachineCall::Leaf` are exactly those this list names,
/// each with why its leaf is leaf-safe: a row marked without being read
/// fails here, and so does one the list names that is not marked.
#[test]
fn every_leaf_row_is_audited() {
    use ns_runtime::builtins::primitive::{MachineCall, MIRROR};
    let marked: std::collections::BTreeSet<&str> = MIRROR
        .iter()
        .filter(|entry| entry.machine_call == MachineCall::Leaf)
        .map(|entry| entry.selector)
        .collect();
    let mut audited = std::collections::BTreeSet::new();
    for (why, selectors) in AUDITED_LEAF_ROWS {
        assert!(!why.is_empty());
        for selector in *selectors {
            assert!(audited.insert(*selector), "{selector} is listed twice");
        }
    }
    let unaudited: Vec<_> = marked.difference(&audited).collect();
    let unmarked: Vec<_> = audited.difference(&marked).collect();
    assert!(
        unaudited.is_empty(),
        "marked Leaf but not audited: {unaudited:?}"
    );
    assert!(
        unmarked.is_empty(),
        "audited but not marked Leaf: {unmarked:?}"
    );
}

/// Every mirror row machine code runs through the leaf shim, by why its
/// leaf is leaf-safe (`builtins::primitive::MachineCall::Leaf`): each was
/// read before it was marked, and a row marked since must be read and
/// added here.
const AUDITED_LEAF_ROWS: &[(&str, &[&str])] = &[
    (
        "answers the class, an identity, the identity hash or a new String, or declines for a future, having written nothing",
        &[
            "objectClass:ifFail:",
            "object:identicalTo:ifFail:",
            "object:equals:ifFail:",
            "objectHash:ifFail:",
            "objectPrintString:ifFail:",
        ],
    ),
    (
        "a number's leaf reads its operands and allocates at most what it answers",
        &[
            "integer:add:ifFail:",
            "integer:subtract:ifFail:",
            "integer:multiply:ifFail:",
            "integer:floorDivide:ifFail:",
            "integer:lessThan:ifFail:",
            "integer:greaterThan:ifFail:",
            "integer:lessOrEqual:ifFail:",
            "integer:greaterOrEqual:ifFail:",
            "integer:equals:ifFail:",
            "integerNegated:ifFail:",
            "integerAbsolute:ifFail:",
            "integerPrintString:ifFail:",
            "integerAsString:ifFail:",
            "largeInteger:add:ifFail:",
            "largeInteger:subtract:ifFail:",
            "largeInteger:multiply:ifFail:",
            "largeInteger:floorDivide:ifFail:",
            "largeInteger:lessThan:ifFail:",
            "largeInteger:greaterThan:ifFail:",
            "largeInteger:lessOrEqual:ifFail:",
            "largeInteger:greaterOrEqual:ifFail:",
            "largeInteger:equals:ifFail:",
            "largeIntegerHash:ifFail:",
            "largeIntegerNegated:ifFail:",
            "largeIntegerAbsolute:ifFail:",
            "largeIntegerPrintString:ifFail:",
            "largeIntegerAsString:ifFail:",
            "float:add:ifFail:",
            "float:subtract:ifFail:",
            "float:multiply:ifFail:",
            "float:divide:ifFail:",
            "float:lessThan:ifFail:",
            "float:greaterThan:ifFail:",
            "float:equals:ifFail:",
            "floatHash:ifFail:",
            "floatPrintString:ifFail:",
            "floatAsString:ifFail:",
            "float:lessOrEqual:ifFail:",
            "float:greaterOrEqual:ifFail:",
            "integer:modulo:ifFail:",
            "largeInteger:modulo:ifFail:",
            "integer:bitAnd:ifFail:",
            "largeInteger:bitAnd:ifFail:",
            "integerAsFloat:ifFail:",
            "largeIntegerAsFloat:ifFail:",
            "float:asStringFixed:ifFail:",
            "float:asStringExponential:ifFail:",
            "float:asStringPrecision:ifFail:",
            "floatClass:parse:ifFail:",
            "integer:bitOr:ifFail:",
            "largeInteger:bitOr:ifFail:",
            "integer:bitXor:ifFail:",
            "largeInteger:bitXor:ifFail:",
            "integer:shiftLeft:ifFail:",
            "largeInteger:shiftLeft:ifFail:",
            "integer:gcd:ifFail:",
            "integer:shiftRight:ifFail:",
            "integer:digitAt:ifFail:",
            "integerDigitLength:ifFail:",
            "integer:asStringRadix:ifFail:",
            "integerClass:parse:radix:ifFail:",
            "floatSquareRoot:ifFail:",
            "floatSine:ifFail:",
            "floatCosine:ifFail:",
            "floatTangent:ifFail:",
            "floatArcSine:ifFail:",
            "floatArcCosine:ifFail:",
            "floatArcTangent:ifFail:",
            "float:arcTangent:ifFail:",
            "floatExponential:ifFail:",
            "floatNaturalLogarithm:ifFail:",
            "floatLogarithm:ifFail:",
            "float:raisedTo:ifFail:",
            "floatFloor:ifFail:",
            "floatCeiling:ifFail:",
            "floatRounded:ifFail:",
            "floatTruncated:ifFail:",
            "floatNegated:ifFail:",
            "floatAbsolute:ifFail:",
            "floatIsNaN:ifFail:",
            "floatIsFinite:ifFail:",
            "floatIsInfinite:ifFail:",
            "floatAsInteger:ifFail:",
            "fractionNumerator:ifFail:",
            "fractionDenominator:ifFail:",
            "fractionClass:numerator:denominator:ifFail:",
            "fractionClass:reducedNumerator:denominator:ifFail:",
            "number:lessThan:ifFail:",
            "number:greaterThan:ifFail:",
            "number:lessOrEqual:ifFail:",
            "number:greaterOrEqual:ifFail:",
            "number:equals:ifFail:",
            "numberHash:ifFail:",
            "number:add:ifFail:",
            "number:subtract:ifFail:",
            "number:multiply:ifFail:",
            "number:divide:ifFail:",
            "number:floorDivide:ifFail:",
            "number:modulo:ifFail:",
            "number:quotient:ifFail:",
            "number:remainder:ifFail:",
            "numberAsFloat:ifFail:",
        ],
    ),
    (
        "reads the text and allocates at most what it answers, writing no object",
        &[
            "stringSize:ifFail:",
            "string:concatenate:ifFail:",
            "string:equals:ifFail:",
            "stringHash:ifFail:",
            "stringAsSymbol:ifFail:",
            "stringPrintString:ifFail:",
            "symbolAsString:ifFail:",
            "symbol:equals:ifFail:",
            "symbolHash:ifFail:",
            "symbolPrintString:ifFail:",
            "string:at:ifFail:",
            "string:copyFrom:to:ifFail:",
            "string:indexOf:startingAt:ifFail:",
            "string:lastIndexOf:startingAt:ifFail:",
            "stringClass:withAll:ifFail:",
            "stringClass:with:ifFail:",
            "stringAsUtf8:ifFail:",
            "stringClass:fromUtf8:from:to:ifFail:",
            "stringUtf8Size:ifFail:",
        ],
    ),
    (
        "reads or allocates an Array, and stores into one only once the index or the range is checked",
        &[
            "collectionSize:ifFail:",
            "collection:at:ifFail:",
            "collection:at:put:ifFail:",
            "collection:copyFrom:to:ifFail:",
            "collection:replaceFrom:to:with:startingAt:ifFail:",
            "arrayClass:new:ifFail:",
        ],
    ),
    (
        "reads a closure's parameter count",
        &[
            "blockNumArgs:ifFail:",
        ],
    ),
    (
        "reads a class object's parts, allocating at most the name's String",
        &[
            "className:ifFail:",
            "metaclassThisClass:ifFail:",
            "behavior:sharesApplicationWith:ifFail:",
            "behaviorEnclosing:ifFail:",
            "behaviorSuperclass:ifFail:",
            "behaviorLevelEnclosing:ifFail:",
        ],
    ),
    (
        "reads or allocates a ByteArray, and writes its bytes only once every index, range and value is checked",
        &[
            "byteArrayClass:new:ifFail:",
            "byteArrayClass:fits:ifFail:",
            "byteArraySize:ifFail:",
            "byteArray:at:ifFail:",
            "byteArray:at:put:ifFail:",
            "byteArray:copyFrom:to:ifFail:",
            "byteArray:replaceFrom:to:with:startingAt:ifFail:",
            "byteArray:concatenate:ifFail:",
            "byteArray:indexOf:startingAt:ifFail:",
            "byteArray:lastIndexOf:startingAt:ifFail:",
            "byteArray:startsWith:ifFail:",
            "byteArray:endsWith:ifFail:",
            "byteArray:at:putCodePoint:ifFail:",
        ],
    ),
    (
        "reads or writes one element of a ByteArray once the offset and the element are checked",
        &[
            "byteArray:int8ElementAt:ifFail:",
            "byteArray:int8ElementAt:put:ifFail:",
            "byteArray:int16ElementAt:ifFail:",
            "byteArray:int16ElementAt:put:ifFail:",
            "byteArray:uint16ElementAt:ifFail:",
            "byteArray:uint16ElementAt:put:ifFail:",
            "byteArray:int32ElementAt:ifFail:",
            "byteArray:int32ElementAt:put:ifFail:",
            "byteArray:uint32ElementAt:ifFail:",
            "byteArray:uint32ElementAt:put:ifFail:",
            "byteArray:int64ElementAt:ifFail:",
            "byteArray:int64ElementAt:put:ifFail:",
            "byteArray:uint64ElementAt:ifFail:",
            "byteArray:uint64ElementAt:put:ifFail:",
            "byteArray:float32ElementAt:ifFail:",
            "byteArray:float32ElementAt:put:ifFail:",
            "byteArray:float64ElementAt:ifFail:",
            "byteArray:float64ElementAt:put:ifFail:",
            "byteArray:int8At:ifFail:",
            "byteArray:int8At:put:ifFail:",
            "byteArray:uint8At:ifFail:",
            "byteArray:uint8At:put:ifFail:",
            "byteArray:littleEndianInt16At:ifFail:",
            "byteArray:littleEndianInt16At:put:ifFail:",
            "byteArray:littleEndianUint16At:ifFail:",
            "byteArray:littleEndianUint16At:put:ifFail:",
            "byteArray:littleEndianInt32At:ifFail:",
            "byteArray:littleEndianInt32At:put:ifFail:",
            "byteArray:littleEndianUint32At:ifFail:",
            "byteArray:littleEndianUint32At:put:ifFail:",
            "byteArray:littleEndianInt64At:ifFail:",
            "byteArray:littleEndianInt64At:put:ifFail:",
            "byteArray:littleEndianUint64At:ifFail:",
            "byteArray:littleEndianUint64At:put:ifFail:",
            "byteArray:littleEndianFloat32At:ifFail:",
            "byteArray:littleEndianFloat32At:put:ifFail:",
            "byteArray:littleEndianFloat64At:ifFail:",
            "byteArray:littleEndianFloat64At:put:ifFail:",
            "byteArray:bigEndianInt16At:ifFail:",
            "byteArray:bigEndianInt16At:put:ifFail:",
            "byteArray:bigEndianUint16At:ifFail:",
            "byteArray:bigEndianUint16At:put:ifFail:",
            "byteArray:bigEndianInt32At:ifFail:",
            "byteArray:bigEndianInt32At:put:ifFail:",
            "byteArray:bigEndianUint32At:ifFail:",
            "byteArray:bigEndianUint32At:put:ifFail:",
            "byteArray:bigEndianInt64At:ifFail:",
            "byteArray:bigEndianInt64At:put:ifFail:",
            "byteArray:bigEndianUint64At:ifFail:",
            "byteArray:bigEndianUint64At:put:ifFail:",
            "byteArray:bigEndianFloat32At:ifFail:",
            "byteArray:bigEndianFloat32At:put:ifFail:",
            "byteArray:bigEndianFloat64At:ifFail:",
            "byteArray:bigEndianFloat64At:put:ifFail:",
        ],
    ),
    (
        "reads, allocates or stores into a WeakArray or an Ephemeron, a store only once it is checked",
        &[
            "weakArrayClass:new:ifFail:",
            "weakArraySize:ifFail:",
            "weakArray:at:ifFail:",
            "weakArray:at:put:ifFail:",
            "objectIsValue:ifFail:",
            "ephemeronKey:ifFail:",
            "ephemeron:key:ifFail:",
            "ephemeronValue:ifFail:",
            "ephemeron:value:ifFail:",
            "ephemeronFinalizer:ifFail:",
            "ephemeron:finalizer:ifFail:",
        ],
    ),
];

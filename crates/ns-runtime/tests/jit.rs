//! The baseline JIT's compile trigger and its state per interpreter
//! (`docs/design/execution.md`, "Planned: the baseline JIT", "When a body
//! is compiled"). What a compiled body computes is the goldens' to pin,
//! under both lowerings; these pin when a body is compiled, whose zone it
//! runs from, and what happens when the zone is full.
#![cfg(ns_jit)]

use indoc::{formatdoc, indoc};
use ns_runtime::{
    CallSiteState, HostResult, HostSignal, Interpreter, JitStatistics, Lowering, MethodSide, Root,
    SourceOrigin, Symbol, Value, ValueKind,
};

/// `source` loaded under `lowering`, with every body's optimization
/// budget parked: these pin what baseline code does, which a request for
/// an optimization -- the leaf's call and the safepoint's exit -- would
/// change in every long loop. The budget's own tests load with
/// `load_spending_with`.
fn load_with(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let (mut interpreter, class) = load_spending_with(source, lowering);
    interpreter.set_optimization_budget(i64::MAX);
    (interpreter, class)
}

/// `source` loaded under `lowering`, every body's optimization budget the
/// JIT's own.
fn load_spending_with(source: &str, lowering: Lowering) -> (Interpreter, Root) {
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

/// `run`, but what the JIT did while `run` ran, after the instance was
/// made, whose initializers call leaves of their own.
fn run_measured(interpreter: &mut Interpreter, class: &Root) -> (i64, JitStatistics) {
    let instance = expect_ok(interpreter.instantiate(class, &[]), "instantiate");
    let before = interpreter.jit_statistics();
    let answer = expect_ok(
        interpreter.send(&instance, Symbol::intern("run"), &[]),
        "run",
    );
    let done = since(interpreter.jit_statistics(), before);
    let answer = interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("run: expected a small integer"),
    });
    (answer, done)
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
        compiled_twins: after.compiled_twins - before.compiled_twins,
        refused_bodies: after.refused_bodies - before.refused_bodies,
        bodies_exiting_at_once: after.bodies_exiting_at_once - before.bodies_exiting_at_once,
        machine_back_edges: after.machine_back_edges - before.machine_back_edges,
        safepoint_exits: after.safepoint_exits - before.safepoint_exits,
        machine_entries: after.machine_entries - before.machine_entries,
        compiled_calls: after.compiled_calls - before.compiled_calls,
        miss_leaf_calls: after.miss_leaf_calls - before.miss_leaf_calls,
        resumed_entries: after.resumed_entries - before.resumed_entries,
        materialized_frames: after.materialized_frames - before.materialized_frames,
        send_exits: after.send_exits - before.send_exits,
        act_leaf_calls: after.act_leaf_calls - before.act_leaf_calls,
        abandoned_bodies: after.abandoned_bodies - before.abandoned_bodies,
        site_slot_hits: after.site_slot_hits - before.site_slot_hits,
        called_entries: after.called_entries - before.called_entries,
        exits: after.exits - before.exits,
        pushes_refused: after.pushes_refused - before.pushes_refused,
        stack_growths: after.stack_growths - before.stack_growths,
        stack_growth_refusals: after.stack_growth_refusals - before.stack_growth_refusals,
        leaf_calls: after.leaf_calls - before.leaf_calls,
        cuts: after.cuts - before.cuts,
        zone_compactions: after.zone_compactions - before.zone_compactions,
        freed_bodies: after.freed_bodies - before.freed_bodies,
        freed_twins: after.freed_twins - before.freed_twins,
        moved_bytes: after.moved_bytes - before.moved_bytes,
        revived_bodies: after.revived_bodies - before.revived_bodies,
        abandoned_entries_out_of_line: after.abandoned_entries_out_of_line
            - before.abandoned_entries_out_of_line,
        back_edge_switches: after.back_edge_switches - before.back_edge_switches,
        back_edge_stretch_exits: after.back_edge_stretch_exits - before.back_edge_stretch_exits,
        back_edge_compiles: after.back_edge_compiles - before.back_edge_compiles,
        safepoint_resumptions: after.safepoint_resumptions - before.safepoint_resumptions,
        compile_nanoseconds: after.compile_nanoseconds - before.compile_nanoseconds,
        optimization_requests: after.optimization_requests - before.optimization_requests,
        optimized_bodies: after.optimized_bodies - before.optimized_bodies,
        optimized_exits: after.optimized_exits - before.optimized_exits,
        abandoned_optimized_bodies: after.abandoned_optimized_bodies
            - before.abandoned_optimized_bodies,
        optimize_nanoseconds: after.optimize_nanoseconds - before.optimize_nanoseconds,
        optimized_frames_materialized: after.optimized_frames_materialized
            - before.optimized_frames_materialized,
        optimized_bodies_resumed: after.optimized_bodies_resumed - before.optimized_bodies_resumed,
        optimized_twin_requests: after.optimized_twin_requests - before.optimized_twin_requests,
        customized_copies: after.customized_copies - before.customized_copies,
        dispatched_calls: after.dispatched_calls - before.dispatched_calls,
        forced_exits: after.forced_exits - before.forced_exits,
        deepest_forced_exit: after.deepest_forced_exit,
        traps: after.traps - before.traps,
        made_not_entrant: after.made_not_entrant - before.made_not_entrant,
        refused_for_good: after.refused_for_good - before.refused_for_good,
        invalidated_copies: after.invalidated_copies - before.invalidated_copies,
    }
}

/// The bodies the JIT compiled or found exiting at once: those it
/// took up at their second entry.
fn taken_up(statistics: JitStatistics) -> u64 {
    statistics.compiled_bodies + statistics.bodies_exiting_at_once
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
    /// The wall time of the baseline compiler is counted, so that a
    /// compiler thread of its own can be judged by what it would take off
    /// the mutator.
    fn compiling_takes_time_that_is_counted(lowering) {
        let (mut interpreter, class) = load_with(THREE_ANSWERS, lowering);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 42);
        let done = since(interpreter.jit_statistics(), before);
        assert!(done.compiled_bodies >= 1, "`answer` is compiled");
        assert!(done.compile_nanoseconds > 0);
    }
}

ns_runtime::for_each_lowering! {
    /// The JIT stress mode's setting compiles every body at its first
    /// entry and enters machine code that exits at once, so that a program
    /// runs the entry stub, the exit and the resumption on every push and
    /// still answers right. The optimized lowering's `self answer` is a
    /// cached lexical send, which `run`'s machine code makes itself; the
    /// reference lowering leaves every send uncached, so that `run` leaves
    /// its machine code at its first send and the interpreter's sends call
    /// `answer`'s (`jit::called`).
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
        } else if !the_zone_stress_mode_is_on() {
            assert_eq!(
                done.machine_entries, 3,
                "the two base activations the host pushes, the factory's and `run`'s, and \
                 `answer`'s first push, which compiles it, run their machine code from the first"
            );
            assert_eq!(
                done.called_entries, 2,
                "the interpreter's next two sends of `answer` call its machine code"
            );
        }
        assert_eq!(done.bodies_exiting_at_once, 0, "installed, not left out");
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
    /// A program with more bodies than a zone holds that has room for only
    /// a few past its stubs, each starting with a template and installed
    /// by the stress setting, fills the zone, which is compacted rather
    /// than any body refused, with the output unchanged.
    fn a_zone_with_room_for_a_few_bodies_leaves_the_output_right(lowering) {
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
        // The room past the stubs, not the zone's size, is what the test
        // fixes, since the stubs' size is the architecture's: 1,344 bytes,
        // which on x64 makes a zone of three pages.
        interpreter.set_jit_zone_bytes(1 << 20);
        let stubs = interpreter.table_sizes().zone_used_bytes;
        interpreter.set_jit_zone_bytes(stubs + 1_344);
        interpreter.set_jit_stress(true);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), count * (count - 1));
        let done = since(interpreter.jit_statistics(), before);
        assert!(done.compiled_bodies >= 1, "the zone holds the stubs and a body");
        assert!(done.zone_compactions >= 1, "the zone fills");
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
    /// loop is all templates, so its code can jump back before it exits,
    /// and the second run -- `run`'s second entry -- compiles it and enters
    /// its machine code. The reference lowering's loop starts with a send,
    /// so there `run` exits at once and is never entered. The backward
    /// jumps' trigger is set past the loop's 1,000 turns, so that the
    /// first run's loop does not compile `run` first (`jit::at_back_edge`).
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
        interpreter.set_jit_compile_at_back_jump(u32::MAX);
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
    /// The default trigger, without the stress setting: `run` has no
    /// loop, but its code runs to a compiled send and a return before it
    /// exits, so the second run compiles it and the third, a push by the
    /// host, enters its machine code. The reference lowering sends
    /// uncached, so there `run` exits at once.
    fn a_straight_line_method_with_a_send_is_entered_at_a_push(lowering) {
        let (mut interpreter, class) = load_with(THREE_ANSWERS, lowering);
        interpreter.set_jit_counting(true);
        assert_eq!(run(&mut interpreter, &class), 42);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 42);
        assert_eq!(run(&mut interpreter, &class), 42);
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(done.compiled_bodies, 1, "`run`, at its second entry");
            assert_eq!(done.bodies_exiting_at_once, 0);
            assert!(done.machine_entries >= 2, "a push enters `run`: {}", done.machine_entries);
        }
    }
}

/// How many elements `a_straight_line_body_is_entered_at_a_push`'s `do:`
/// visits.
const ELEMENTS: i64 = 1000;

ns_runtime::for_each_lowering! {
    /// The block `[:each | each]` runs straight to its return: `do:`'s
    /// `value:` activates it once per element, and from its second
    /// activation on, compiled, every one runs its machine code, which
    /// returns at once -- entered at an interpreter's push or called by an
    /// interpreted send or a compiled one.
    fn a_straight_line_body_is_entered_at_a_push(lowering) {
        let source = formatdoc! {"
            class Subject = Object new () (
              public run = (
                |
                  elements = Array new: {ELEMENTS}.
                |
                elements do: [:each | each].
                ^elements size.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(&source, lowering);
        interpreter.set_jit_counting(true);
        assert_eq!(run(&mut interpreter, &class), ELEMENTS);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), ELEMENTS);
        let done = since(interpreter.jit_statistics(), before);
        if !a_stress_mode_is_on() {
            let ran = done.machine_entries + done.called_entries + done.compiled_calls;
            assert!(
                ran >= ELEMENTS as u64,
                "every activation of the block runs its machine code: {ran}"
            );
            if lowering == Lowering::Optimized {
                assert_eq!(done.send_exits, 0, "no closure stub declined");
            }
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

/// A loop of 100,000 turns, each allocating an `Array` through the
/// generic leaf, so that the allocations fill eden, whose request for a
/// scavenge only a back edge's poll can notice in a loop that makes no
/// send.
const ALLOCATING_LOOP: &str = indoc! {"
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

ns_runtime::for_each_lowering! {
    /// The machine code exits at the poll that found the scavenge
    /// requested, the interpreter takes the safepoint, and the frame goes
    /// back into the machine code at the loop's head, where the rest of
    /// the loop runs.
    fn a_back_edge_poll_takes_the_safepoint(lowering) {
        let (mut interpreter, class) = load_with(ALLOCATING_LOOP, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_jit_counting(true);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 199_999);
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized {
            assert!(done.safepoint_exits >= 1, "a back edge found the scavenge requested");
        }
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                done.safepoint_resumptions >= 1,
                "the frame went back into machine code after a safepoint: {done:?}"
            );
            assert!(
                done.machine_back_edges >= 99_000,
                "the loop closes in machine code after its scavenges: {done:?}"
            );
            assert_eq!(
                done.safepoint_resumptions, done.safepoint_exits,
                "every poll's exit went back into machine code: {done:?}"
            );
        }
    }
}

/// `ALLOCATING_LOOP` with a send to a compiled method in each turn, right
/// after the allocation that may request a scavenge.
const ALLOCATING_LOOP_WITH_A_CALL: &str = indoc! {"
    class Subject = Object new () (
      successorOf: value = (
        | successor |
        successor:: value + 1.
        ^successor.
      )
      public run = (
        |
          index ::= 0.
          last
        |
        [index < 100000] whileTrue: [
          last:: {index. index. index. index. index. index. index. index. index. index. index. index. index. index. index. index}.
          index:: self successorOf: index.
        ].
        ^(last at: 16) + index.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A compiled call made while a scavenge is requested is refused by
    /// its callee's one depth compare, the request having lowered the
    /// depth limit word, and the interpreter takes the push.
    fn a_compiled_call_at_a_requested_safepoint_is_refused(lowering) {
        let (mut interpreter, class) = load_with(ALLOCATING_LOOP_WITH_A_CALL, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_jit_counting(true);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 199_999);
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                done.pushes_refused >= 1,
                "a call found the scavenge requested: {done:?}"
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// An interpreted backward jump that finds a safepoint requested stops
    /// the frame at its loop's head, and once the safepoint is served the
    /// frame switches into machine code there, as the jump would have
    /// switched it: the counting loop sent once, its body compiled at its
    /// first interpreted backward jump, run while the cache stress mode
    /// holds the request, so that no interpreted backward jump gets past
    /// the safepoint's test -- every compile at a backward jump, and every
    /// switch, is the safepoint's. The allocating loop, with no request
    /// held, switches at its first backward jump, its scavenges' requests
    /// found by machine code's polls.
    fn an_interpreted_back_jump_at_a_safepoint_switches_after_it(lowering) {
        let (mut interpreter, class) = load_with(ALLOCATING_LOOP, lowering);
        interpreter.set_jit_compile_at_back_jump(1);
        let (answer, done) = run_measured(&mut interpreter, &class);
        assert_eq!(answer, 199_999);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(done.back_edge_switches >= 1, "the frame switches: {done:?}");
        }
        #[cfg(debug_assertions)]
        {
            let (mut interpreter, class) = load_with(COUNTING_LOOP, lowering);
            interpreter.set_jit_compile_at_back_jump(1);
            interpreter.set_stress_caches(true);
            let (answer, done) = run_measured(&mut interpreter, &class);
            assert_eq!(answer, 500_500);
            if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
                assert_eq!(
                    done.back_edge_compiles, 1,
                    "compiled after the first jump's safepoint: {done:?}"
                );
                assert_eq!(
                    done.back_edge_stretch_exits, 0,
                    "no interpreted jump got past the request: {done:?}"
                );
                assert!(
                    done.safepoint_resumptions >= 1,
                    "the frame went into machine code after the safepoint: {done:?}"
                );
            }
        }
    }
}

/// `ALLOCATING_LOOP` in a method of its own, `allocate:`, sent by
/// `allocateFrom:`, which `run` sends forty times from a loop the
/// interpreter runs: `allocate:`'s machine code runs below
/// `allocateFrom:`'s, called through its cell, or below the interpreter's
/// send. From the twentieth round on, `allocate:` first sends `start`,
/// whose machine code exits at its send of a message the class's
/// `doesNotUnderstand:` answers with 0, so that `allocate:`'s frame,
/// materialized with it, goes on in its machine code when `start`
/// returns to it, and its loop runs there.
const ALLOCATING_CALLEE: &str = indoc! {"
    class Subject = Object new () (
      allocate: round = (
        |
          index ::= round < 20 ifTrue: [0] ifFalse: [self start].
          last
        |
        [index < 10000] whileTrue: [
          last:: {index. index. index. index. index. index. index. index. index. index. index. index. index. index. index. index}.
          index:: index + 1.
        ].
        ^(last at: 16) + index.
      )
      allocateFrom: round = (
        ^self allocate: round
      )
      start = (
        ^self unknownStart
      )
      public doesNotUnderstand: message = (
        ^0
      )
      public run = (
        |
          sum ::= 0.
          round ::= 0.
        |
        [round < 40] whileTrue: [
          sum:: sum + (self allocateFrom: round).
          round:: round + 1.
        ].
        ^sum.
      )
    ) : ()
"};

/// A poll that finds a scavenge requested in a called body stops the
/// body's frame at its loop's head, and once the safepoint is served the
/// frame goes back into machine code there, wherever the stop reached the
/// run loop: as the stop of a body called below a compiled sender, the
/// exit materializing both frames, or below the interpreter's send, in the
/// first twenty rounds, and as the stop of the machine code a frame went
/// on in when a send returned to it, in the last twenty. The safepoint
/// clears the request it served, so that the turn after it finds no poll
/// to stop at: a scavenge takes one poll's exit at most, and each such
/// exit is a called body's stop that goes back in here, since no request
/// left standing stops the frame again for another way back to take it.
/// Only the optimized lowering: the reference lowering's sends exit, and
/// its answer is the same.
#[test]
fn a_called_body_goes_back_into_machine_code_after_its_safepoint() {
    let (mut interpreter, class) = load_with(ALLOCATING_CALLEE, Lowering::Optimized);
    // `run` stays interpreted, its sends calling machine code.
    interpreter.set_jit_compile_at_back_jump(u32::MAX);
    interpreter.set_jit_counting(true);
    let scavenges = interpreter.collector_statistics().scavenges;
    let (answer, done) = run_measured(&mut interpreter, &class);
    let scavenges = interpreter.collector_statistics().scavenges - scavenges;
    assert_eq!(answer, 40 * 19_999);
    if !a_stress_mode_is_on() {
        assert!(
            done.safepoint_exits >= 1,
            "a called body's poll found the scavenge requested: {done:?}"
        );
        assert!(
            u64::try_from(scavenges).expect("a small count") >= done.safepoint_exits,
            "no poll found a served request still standing: {scavenges} scavenges, {done:?}"
        );
        assert_eq!(
            done.safepoint_resumptions, done.safepoint_exits,
            "every poll's exit went back into machine code: {done:?}"
        );
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

/// `source`'s `run` as `run_compiled` runs it with the JIT on, every
/// body's optimization budget the JIT's own.
fn run_spending(source: &str, lowering: Lowering) -> (i64, JitStatistics) {
    let (mut interpreter, class) = load_spending_with(source, lowering);
    interpreter.set_jit_stress(true);
    interpreter.set_jit_counting(true);
    let before = interpreter.jit_statistics();
    let answer = run(&mut interpreter, &class);
    (answer, since(interpreter.jit_statistics(), before))
}

/// A loop of 200,000 turns, which spends far more than a body's budget
/// at its back jumps.
const HOT_LOOP: &str = indoc! {"
    class Subject = Object new () (
      public run = (
        |
          sum ::= 0.
          index ::= 1.
        |
        [index <= 200000] whileTrue: [
          sum:: sum + index.
          index:: index + 1.
        ].
        ^sum.
      )
    ) : ()
"};

/// A body that runs once and returns.
const RETURN_THREE: &str = indoc! {"
    class Subject = Object new () (
      public run = (
        ^3
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// The loop's back jumps spend `run`'s budget until it runs out: its
    /// optimization is requested, once, and its row parked, so that the
    /// rest of the loop requests nothing more. The reference lowering
    /// inlines neither block: each turn sends `value` to both, whose
    /// bodies are requested as well -- the condition's and the loop's,
    /// `run`'s own back jump spending less than either.
    fn a_hot_loop_requests_its_body_s_optimization(lowering) {
        let (answer, statistics) = run_spending(HOT_LOOP, lowering);
        assert_eq!(answer, 20_000_100_000);
        // The invalidate stress mode frees the copies the requests made,
        // and each body asks for its copy again at its next spend.
        if the_stress_mode_is_on("INVALIDATE") {
            return;
        }
        let requested = match lowering {
            Lowering::Optimized => 1,
            Lowering::Reference => 2,
        };
        if spends_no_budget_in_a_loop(lowering) {
            assert!(statistics.optimization_requests <= requested);
        } else {
            assert_eq!(statistics.optimization_requests, requested);
        }
    }
}

/// Whether a stress mode keeps a hot loop's bodies from running a budget
/// out: only machine code spends one. The suspend stress mode's held
/// request stops the loop at every backward jump, and the safepoint
/// suspends its frame and resumes it, a frame that goes on interpreted
/// rather than switch back into machine code at the loop's head
/// (`vm::run`), so that after its first poll the loop never runs in
/// machine code again. Under the zone stress mode the reference
/// lowering's `run` runs each turn in machine code only up to its first
/// send, uncached, which exits, so that it never reaches its backward
/// jump there, while its blocks, which no activation runs at a
/// safepoint, are freed at every compaction, which clears their budgets
/// (`docs/OPEN-WORK.md`, "A compaction that frees a live body forgets
/// the optimization budget it spent").
fn spends_no_budget_in_a_loop(lowering: Lowering) -> bool {
    let suspending = std::env::var_os("NS_NEXT_STRESS_SUSPEND")
        .is_some_and(|value| !value.is_empty() && value != "0");
    suspending || (lowering == Lowering::Reference && the_zone_stress_mode_is_on())
}

/// A loop whose every turn sends a method without a loop of its own,
/// which spends its budget only at its entries.
#[cfg(debug_assertions)]
const STEPPING: &str = indoc! {"
    class Subject = Object new () (
      step: value = (
        ^value + 1
      )
      public run = (
        |
          index ::= 0.
        |
        [index < 100] whileTrue: [
          index:: self step: index.
        ].
        ^index.
      )
    ) : ()
"};

/// The budget `STEPPING`'s test arms every body with: a few dozen runs'
/// worth.
#[cfg(debug_assertions)]
const STEPPING_BUDGET: i64 = 50_000;

/// An interpreter with `STEPPING` loaded under `lowering`, every body's
/// budget `STEPPING_BUDGET`, and an instance to send `run` to: the
/// optimize stress mode off, whatever the environment says, since it arms
/// every budget at 0 by design, and these pin what a budget carries.
#[cfg(debug_assertions)]
fn stepping(lowering: Lowering) -> (Interpreter, Root) {
    let (mut interpreter, class) = load_spending_with(STEPPING, lowering);
    interpreter.set_optimize_stress(false);
    interpreter.set_optimization_budget(STEPPING_BUDGET);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    (interpreter, instance)
}

/// Sends `instance` `run` once, answering whether the JIT has requested
/// an optimization since `before`.
#[cfg(debug_assertions)]
fn run_stepping(interpreter: &mut Interpreter, instance: &Root, before: JitStatistics) -> bool {
    expect_ok(
        interpreter.send(instance, Symbol::intern("run"), &[]),
        "run",
    );
    since(interpreter.jit_statistics(), before).optimization_requests > 0
}

/// A full collection frees the code ids of the bodies no activation runs
/// -- `run` and `step:` between two runs -- and mints new ones for them,
/// whose rows are armed with what was left of the old: a budget spent
/// across the collection runs out at the same total as one spent without
/// it, so that a body called a little between every two collections is
/// still requested. The optimized lowering's: under the reference
/// lowering every body `STEPPING` runs would exit at once, so that none
/// is compiled and none spends. A debug build's, as `collect_now` is.
#[cfg(debug_assertions)]
#[test]
fn a_full_collection_keeps_what_is_left_of_a_budget() {
    let lowering = Lowering::Optimized;
    {
        if the_zone_stress_mode_is_on() {
            // No budget runs out: the compactions free `step:`, which no
            // activation runs between two of its calls, and clear its
            // budget, so that its next compile arms a full one
            // (`docs/OPEN-WORK.md`, "A compaction that frees a live body
            // forgets the optimization budget it spent"); the exemption
            // goes with that item. `run` spends nothing either: its first
            // call of `step:` after each compaction misses and finds it
            // uncompiled, counting its entries afresh, so that the miss
            // refuses the call (`jit::note_entry_by_call`) and `run`
            // exits at the send, and is abandoned at its sixty-fourth
            // such exit.
            return;
        }
        let (mut interpreter, instance) = stepping(lowering);
        let before = interpreter.jit_statistics();
        let runs_to_request = (1..=10_000)
            .find(|_| run_stepping(&mut interpreter, &instance, before))
            .expect("the budget runs out");
        assert!(runs_to_request >= 10, "{runs_to_request} runs");
        let (mut interpreter, instance) = stepping(lowering);
        let before = interpreter.jit_statistics();
        let half = runs_to_request * 6 / 10 + 1;
        for _ in 0..half {
            assert!(
                !run_stepping(&mut interpreter, &instance, before),
                "requested before the collection"
            );
        }
        interpreter.collect_now();
        let requested = (0..half).any(|_| run_stepping(&mut interpreter, &instance, before));
        assert!(
            requested,
            "{half} runs on either side of the collection, {runs_to_request} running out"
        );
    }
}

ns_runtime::for_each_lowering! {
    /// A body run once spends a few instructions of its budget, which
    /// requests nothing.
    fn a_body_run_once_requests_nothing(lowering) {
        let (answer, statistics) = run_spending(RETURN_THREE, lowering);
        assert_eq!(answer, 3);
        assert_eq!(statistics.optimization_requests, 0);
    }
}

/// The budget the optimizing tests arm every body with: a few thousand
/// instructions, so that a body a loop calls a few hundred times runs out.
const OPTIMIZING_BUDGET: i64 = 5_000;

/// `source`'s `run` sent `runs` times to one instance, as `run_compiled`
/// runs it with the JIT on -- every body compiled at its first entry, so
/// that the reference lowering's, which mostly exit at once, are too --
/// and every body's budget `OPTIMIZING_BUDGET`: a body whose budget runs
/// out is optimized at the safepoint its request asks for, and the pushes
/// and calls after it enter its optimized code. Answers each run's answer,
/// what the JIT did over all of them, and what it did in the last, which
/// runs after the first run's optimizations.
fn run_optimizing(
    source: &str,
    lowering: Lowering,
    runs: usize,
) -> (Vec<i64>, JitStatistics, JitStatistics) {
    let (mut interpreter, class) = load_spending_with(source, lowering);
    interpreter.set_optimization_budget(OPTIMIZING_BUDGET);
    interpreter.set_jit_stress(true);
    interpreter.set_jit_counting(true);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let before = interpreter.jit_statistics();
    let mut last = before;
    let mut answers = Vec::new();
    for _ in 0..runs {
        last = interpreter.jit_statistics();
        answers.push(send_unary(&mut interpreter, &subject, "run"));
    }
    let after = interpreter.jit_statistics();
    (answers, since(after, before), since(after, last))
}

/// Whether the decisions about optimized code are pinned here: under the
/// optimized lowering with no stress mode that changes which copy of a
/// body runs when (`MODES_THAT_CHANGE_THE_COPY`). The wide stress mode
/// keeps the pins, so that it holds the tiers' decisions too.
fn pins_the_tiers(lowering: Lowering) -> bool {
    lowering == Lowering::Optimized
        && !MODES_THAT_CHANGE_THE_COPY
            .iter()
            .any(|mode| the_stress_mode_is_on(mode))
}

/// The stress modes that change which copy of a body runs when: by their
/// compactions, collections, suspensions and moves, by compiling or
/// optimizing every body at once, or by leaving machine code at every
/// poll or interrupt point -- the cache stress mode among them, since it
/// holds the safepoint request, which every poll takes, and vacates the
/// caches there -- and by freeing the copies at every tenth interrupt
/// point optimized code passes, the invalidate stress mode. The wide
/// stress mode changes no machine code's decision.
const MODES_THAT_CHANGE_THE_COPY: [&str; 11] = [
    "MOVES",
    "COLLECT",
    "SCAVENGE",
    "SUSPEND",
    "ZONE",
    "JIT",
    "EXIT",
    "OPTIMIZE",
    "DEOPT",
    "CACHES",
    "INVALIDATE",
];

/// The squares of 1 to 100,000 summed in one loop: `run`'s back jumps
/// spend its budget in its first run, so that the second and third push
/// `run` into its optimized code.
const SUM_OF_SQUARES_LOOP: &str = indoc! {"
    class Subject = Object new () (
      public run = (
        |
          sum ::= 0.
          index ::= 1.
        |
        [index <= 100000] whileTrue: [
          sum:: sum + (index * index).
          index:: index + 1.
        ].
        ^sum.
      )
    ) : ()
"};

/// The sum of the squares of 1 to 100,000.
fn expected_sum_of_squares() -> i64 {
    let count: i64 = 100_000;
    count * (count + 1) * (2 * count + 1) / 6
}

ns_runtime::for_each_lowering! {
    /// A body whose budget ran out is optimized at the next safepoint,
    /// once, and the runs that push it afterwards answer what its first
    /// run, in baseline code, answered. The optimized lowering inlines the
    /// loop's blocks, so that `run` is the only body optimized; the
    /// reference lowering's blocks are optimized as well.
    fn a_hot_body_runs_optimized_and_answers_the_same(lowering) {
        let (answers, statistics, _) = run_optimizing(SUM_OF_SQUARES_LOOP, lowering, 3);
        assert_eq!(answers, vec![expected_sum_of_squares(); 3]);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(statistics.optimized_bodies, 1);
        } else {
            assert_some_optimized(&statistics, lowering);
        }
    }
}

/// `run` calling `three` twice: under the optimize stress mode `three` is
/// compiled at its first entry, a push, which spends nothing, and its
/// second, a call through its prologue, spends its budget, armed at 0.
const RETURN_THREE_TWICE: &str = indoc! {"
    class Subject = Object new () (
      three = (
        ^3
      )
      public run = (
        ^self three + self three
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// The optimize stress mode arms every budget at 0 whatever the test
    /// parked it at, so that a body run twice is optimized: at the
    /// safepoint its request asks for, which the reference lowering's
    /// `run`, interpreted and with no loop, first reaches in its second
    /// run, at its push.
    fn under_the_optimize_stress_a_body_run_twice_is_optimized(lowering) {
        let (mut interpreter, class) = load_with(RETURN_THREE_TWICE, lowering);
        interpreter.set_optimize_stress(true);
        assert_eq!(run(&mut interpreter, &class), 6);
        assert_eq!(run(&mut interpreter, &class), 6);
        let statistics = interpreter.jit_statistics();
        // Under the zone stress mode the safepoint between the reference
        // lowering's sends frees `three`, which no activation runs, so
        // that each send compiles it afresh and enters it at a push,
        // which spends nothing.
        if lowering == Lowering::Reference && the_zone_stress_mode_is_on() {
            return;
        }
        assert!(statistics.optimized_bodies >= 1, "{statistics:?}");
    }
}

/// Whether the copies a test counts are pinned here: under no stress mode
/// that frees or compiles again the code a run made -- by its
/// compactions, collections, suspensions and moves, or by leaving machine
/// code at every poll -- beside the optimize, JIT and deopt stress modes,
/// which these tests set or which change no copy's key.
fn counts_the_copies() -> bool {
    !MODES_THAT_CHANGE_THE_COPY
        .iter()
        .filter(|mode| !matches!(**mode, "OPTIMIZE" | "JIT" | "DEOPT"))
        .any(|mode| the_stress_mode_is_on(mode))
}

/// `source` loaded under `lowering` with every body compiled at its first
/// entry, installed whatever it exits at, and optimized at the first
/// spend of its budget, so that a body's every key is optimized once it
/// has run.
fn load_customizing(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let (mut interpreter, class) = load_with(source, lowering);
    interpreter.set_jit_stress(true);
    interpreter.set_optimize_stress(true);
    (interpreter, class)
}

/// The class `subject`'s unary `selector` answers.
fn class_answered(interpreter: &mut Interpreter, subject: &Root, selector: &str) -> Root {
    expect_ok(
        interpreter.send(subject, Symbol::intern(selector), &[]),
        selector,
    )
}

/// `total`, a method of `A`, summing `A`'s slot in a loop, run on an `A`
/// and on a `B`, a subclass of `A` with a slot of its own: one body, run
/// on two receivers' applications and entered at `A`'s for both.
const ONE_METHOD_ON_TWO_APPLICATIONS: &str = indoc! {"
    class Subject = Object new () (
      class A step: value = Object new (
        |
          step = value.
        |
      ) (
        public total = (
          |
            sum ::= 0.
            index ::= 0.
          |
          [index < 100] whileTrue: [
            sum:: sum + step.
            index:: index + 1.
          ].
          ^sum.
        )
      )

      class B = A step: 3 (
        |
          own = 5.
        |
      ) ()

      public aClass = (
        ^A.
      )

      public run = (
        ^(A step: 2) total + B new total.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A method body run on receivers of two applications has a copy for
    /// each: its key is the receiver's application and the one it was
    /// entered at, and the copy made for an `A` is never entered for a
    /// `B`, which runs the baseline code until its own copy is made.
    fn a_method_run_on_two_applications_has_two_copies(lowering) {
        let (mut interpreter, class) = load_customizing(ONE_METHOD_ON_TWO_APPLICATIONS, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        assert_eq!(send_unary(&mut interpreter, &subject, "run"), 500);
        assert_eq!(send_unary(&mut interpreter, &subject, "run"), 500);
        if !counts_the_copies() {
            return;
        }
        let base_class = class_answered(&mut interpreter, &subject, "aClass");
        let total = Symbol::intern("total");
        assert_eq!(
            interpreter.jit_customized_copies(&base_class, MethodSide::Instance, total),
            2
        );
        assert!(interpreter.jit_statistics().customized_copies >= 2);
    }
}

/// `M`'s `total` summing `M`'s private lazy slot in a loop, `M` applied
/// twice in `Twice`'s chain: sent publicly it runs at the upper level,
/// whose slot holds 10, and sent by `super` from `Middle` at the lower
/// one, whose slot holds 1 -- one body on one receiver at two levels, and
/// one getter, which reads its level's own field.
const ONE_MIXIN_APPLIED_TWICE: &str = indoc! {"
    class Subject = Object new () (
      class Base = Object new () ()

      class M step: value = Object new (
        |
          private seed = value.
        |
      ) (
        lazy private step = seed.

        public total = (
          |
            sum ::= 0.
            index ::= 0.
          |
          [index < 100] whileTrue: [
            sum:: sum + step.
            index:: index + 1.
          ].
          ^sum.
        )
      )

      class Middle = Object new () (
        public lowerTotal = (
          ^super total.
        )
      )

      class Twice = Base new <: M step: 1 <: Middle <: M step: 10.

      public mClass = (
        ^M.
      )

      public run = (
        |
          twice = Twice new.
        |
        ^twice total + twice lowerTotal.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A mixin applied twice in one chain runs its method's one body on
    /// one receiver at two levels, and each level has a copy of its own:
    /// the key's entered application tells them apart, so that neither
    /// level runs the copy made for the other. The lazy slot's getter
    /// reads its slot at the field its copy's level fixes, so a level that
    /// ran the other's copy would read the other's slot: the sum of 100
    /// tens and 100 ones holds at every run only if each runs its own.
    fn a_mixin_applied_twice_has_a_copy_per_level(lowering) {
        let (mut interpreter, class) = load_customizing(ONE_MIXIN_APPLIED_TWICE, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), 1100);
        }
        if !counts_the_copies() {
            return;
        }
        let mixin_class = class_answered(&mut interpreter, &subject, "mClass");
        for selector in ["total", "step"] {
            assert_eq!(
                interpreter.jit_customized_copies(
                    &mixin_class,
                    MethodSide::Instance,
                    Symbol::intern(selector)
                ),
                2,
                "{selector}: a copy per level"
            );
        }
    }
}

/// `run` reading `Holder`'s lazy slot a hundred times: its getter, whose
/// initializer counts its evaluations, computes it at the first read and
/// answers the slot's value at every later one.
const A_LAZY_SLOT_READ_IN_A_LOOP: &str = indoc! {"
    class Subject = Object new () (
      class Holder = Object new (
        |
          public evaluations ::= 0.
        |
      ) (
        lazy public cached = count.

        count = (
          evaluations:: evaluations + 1.
          ^evaluations * 7.
        )
      )

      public holderClass = (
        ^Holder.
      )

      public run = (
        |
          holder = Holder new.
          sum ::= 0.
          index ::= 0.
        |
        [index < 100] whileTrue: [
          sum:: sum + holder cached.
          index:: index + 1.
        ].
        ^sum * 1000 + holder evaluations.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A lazy slot's getter run as a customized copy, which reads the
    /// slot at a fixed field with no guard, computes the slot once and
    /// answers its value from then on: a hundred reads of 7, one
    /// evaluation.
    fn a_lazy_slot_read_by_a_copy_answers_its_value_once_filled(lowering) {
        let (mut interpreter, class) = load_customizing(A_LAZY_SLOT_READ_IN_A_LOOP, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), 700_001);
        }
        if !counts_the_copies() {
            return;
        }
        let holder_class = class_answered(&mut interpreter, &subject, "holderClass");
        assert_eq!(
            interpreter.jit_customized_copies(
                &holder_class,
                MethodSide::Instance,
                Symbol::intern("cached")
            ),
            1,
            "the getter runs as a copy"
        );
    }
}

/// `A` and an unrelated `D`, each summing its slot in its `total`, and
/// `edit`, which adds a slot to `A` through a MirrorBuilder and gives
/// `step` another initial value: `run` sums both totals of fresh
/// instances, 500 before the edit and 700 after it.
const AN_EDIT_OF_ONE_OF_TWO_CLASSES: &str = indoc! {"
    class Subject usingPlatform: platform = Object new (
      |
        ClassMirror = platform mirrors ClassMirror.
      |
    ) (
      class A = Object new (
        |
          public step ::= 2.
        |
      ) (
        public total = (
          |
            sum ::= 0.
            index ::= 0.
          |
          [index < 100] whileTrue: [
            sum:: sum + step.
            index:: index + 1.
          ].
          ^sum.
        )
      )

      class D = Object new (
        |
          public step ::= 3.
        |
      ) (
        public total = (
          |
            sum ::= 0.
            index ::= 0.
          |
          [index < 100] whileTrue: [
            sum:: sum + step.
            index:: index + 1.
          ].
          ^sum.
        )
      )

      public aClass = (
        ^A.
      )

      public dClass = (
        ^D.
      )

      public run = (
        ^A new total + D new total.
      )

      public edit = (
        | builder |
        builder:: (ClassMirror reflecting: A) mixin declaration asBuilder.
        builder header source: 'class A = Object new (| public added ::= 5. public step ::= 4. |)'.
        builder install.
        ^0.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// An install frees the copies keyed on the applications it rebuilds,
    /// and only those: `A`'s `total` has its copy freed, `D`'s keeps its
    /// own, and the program answers what the edit says.
    fn an_edit_frees_the_copies_of_the_applications_it_rebuilds(lowering) {
        let (mut interpreter, class) = load_customizing(AN_EDIT_OF_ONE_OF_TWO_CLASSES, lowering);
        let platform = interpreter.platform();
        let subject = expect_ok(
            interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
            "usingPlatform:",
        );
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), 500);
        }
        let total = Symbol::intern("total");
        let d_class = class_answered(&mut interpreter, &subject, "dClass");
        let counted = counts_the_copies();
        if counted {
            let a_class = class_answered(&mut interpreter, &subject, "aClass");
            assert_eq!(
                interpreter.jit_customized_copies(&a_class, MethodSide::Instance, total),
                1
            );
            assert_eq!(
                interpreter.jit_customized_copies(&d_class, MethodSide::Instance, total),
                1
            );
        }
        // Only the statistic shows the copy freed: no receiver carries the
        // old application's id after the install, so a copy left keyed on
        // it would never be entered again. `invalidate_applications` frees
        // its memory early; it prevents no wrong answer, which the
        // reclaim's dropping of the rows does
        // (`a_copy_keyed_on_a_reclaimed_id_is_freed`).
        let statistics_before = interpreter.jit_statistics();
        assert_eq!(send_unary(&mut interpreter, &subject, "edit"), 0);
        let statistics_after = since(interpreter.jit_statistics(), statistics_before);
        if counted {
            assert_eq!(statistics_after.invalidated_copies, 1, "{statistics_after:?}");
            assert_eq!(
                interpreter.jit_customized_copies(&d_class, MethodSide::Instance, total),
                1,
                "`D`'s copy stays"
            );
        }
        let expected_after_edit = 700;
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), expected_after_edit);
        }
    }
}

ns_runtime::for_each_lowering! {
    /// Under the invalidate stress mode optimized code leaves for a
    /// safepoint at every tenth interrupt point it passes, which frees
    /// the copies before it installs what was requested since, and the
    /// bodies are optimized afresh at their next run-out: copies are
    /// installed and freed run after run, and the answers hold.
    fn under_the_invalidate_stress_copies_are_freed_and_the_answer_holds(lowering) {
        let (mut interpreter, class) = load_with(ONE_METHOD_ON_TWO_APPLICATIONS, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_invalidate_stress(true);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..5 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), 500);
        }
        if counts_the_copies() {
            let statistics = interpreter.jit_statistics();
            assert!(statistics.customized_copies >= 2, "{statistics:?}");
            assert!(statistics.invalidated_copies >= 1, "{statistics:?}");
            assert_eq!(statistics.made_not_entrant, 0, "{statistics:?}");
        }
    }
}

/// `Cell`'s lazy slot `step`, whose getter reads the slot and fills it
/// with 2 when it is nil, `Cell` applied over whichever class `Box`'s
/// factory is given: over `Narrow`, which has no slot, the slot is the
/// instance's first field; over `Wide`, whose three slots hold 100, its
/// fourth. Every `Box` makes its own class object of `Cell`, and the
/// application over a base dies with the last of them.
const ONE_MIXIN_OVER_TWO_BASES: &str = indoc! {"
    class Subject = Object new () (
      class Narrow = Object new () ()

      class Wide = Object new (
        |
          public first ::= 100.
          public second ::= 100.
          public third ::= 100.
        |
      ) ()

      class Box on: base = Object new (
        |
          base = base.
        |
      ) (
        public class Cell = base new () (
          lazy public step = 2.
        )
      )

      public bases = (
        ^Narrow == Wide ifTrue: [1] ifFalse: [0].
      )

      public narrow = (
        ^(Box on: Narrow) Cell new.
      )

      public wide = (
        ^(Box on: Wide) Cell new.
      )

      public sumOf: cell = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 100] whileTrue: [
          sum:: sum + cell step.
          index:: index + 1.
        ].
        ^sum.
      )
    ) : ()
"};

/// `subject`'s `sumOf:` of `cell`, a `SmallInteger`.
#[cfg(debug_assertions)]
fn sum_of(interpreter: &mut Interpreter, subject: &Root, cell: &Root) -> i64 {
    let answer = expect_ok(
        interpreter.send(
            subject,
            Symbol::intern("sumOf:"),
            std::slice::from_ref(cell),
        ),
        "sumOf:",
    );
    interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("sumOf:: expected a small integer"),
    })
}

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    /// A full collection that reclaims the id of an application frees
    /// the copies keyed on it and drops their rows: `Cell` over `Narrow`
    /// has a copy of `step`'s getter, which reads its slot at the first
    /// field; the application dies with its last `Box`, the collection
    /// frees its id, and `Cell` over `Wide` is minted it again -- a copy
    /// left keyed on it would answer `Wide`'s first slot, 100, for
    /// `Cell`'s 2.
    fn a_copy_keyed_on_a_reclaimed_id_is_freed(lowering) {
        let (mut interpreter, class) = load_customizing(ONE_MIXIN_OVER_TWO_BASES, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        // Both bases' applications minted before either `Cell`'s, and the
        // ids that died before collected, so that `Cell` over `Narrow`
        // takes the smallest free id, which its death frees for `Cell`
        // over `Wide` to take (`DispatchIdAllocator::mint`).
        assert_eq!(send_unary(&mut interpreter, &subject, "bases"), 0);
        interpreter.collect_now();
        for _ in 0..3 {
            let cell = class_answered(&mut interpreter, &subject, "narrow");
            assert_eq!(sum_of(&mut interpreter, &subject, &cell), 200);
        }
        interpreter.collect_now();
        let expected_of_the_new_class = 200;
        for _ in 0..3 {
            let cell = class_answered(&mut interpreter, &subject, "wide");
            assert_eq!(
                sum_of(&mut interpreter, &subject, &cell),
                expected_of_the_new_class
            );
        }
    }
}

/// `total`, a method of `A`, adding `A`'s slot in a block it hands
/// `repeat:`, which runs it a hundred times: one block body, run by the
/// closures of an `A` and of a `B`.
const ONE_BLOCK_ON_TWO_RECEIVERS: &str = indoc! {"
    class Subject = Object new () (
      class A step: value = Object new (
        |
          step = value.
        |
      ) (
        repeat: block = (
          |
            index ::= 0.
          |
          [index < 100] whileTrue: [
            block value.
            index:: index + 1.
          ].
        )

        public total = (
          |
            sum ::= 0.
          |
          self repeat: [sum:: sum + step].
          ^sum.
        )
      )

      class B = A step: 3 (
        |
          own = 5.
        |
      ) ()

      public aClass = (
        ^A.
      )

      public run = (
        ^(A step: 2) total + B new total.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A block body has one optimized copy whatever its closures'
    /// receivers: it is never customized, so that the block `total`
    /// makes, run by an `A`'s closure and a `B`'s, is optimized once and
    /// has no copies.
    fn a_block_has_one_copy_whatever_its_receivers(lowering) {
        let (mut interpreter, class) = load_customizing(ONE_BLOCK_ON_TWO_RECEIVERS, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        assert_eq!(send_unary(&mut interpreter, &subject, "run"), 500);
        assert_eq!(send_unary(&mut interpreter, &subject, "run"), 500);
        if !counts_the_copies() {
            return;
        }
        let base_class = class_answered(&mut interpreter, &subject, "aClass");
        let total = Symbol::intern("total");
        let blocks = interpreter.jit_block_tiers(&base_class, MethodSide::Instance, total);
        assert_eq!(blocks, vec![(true, 0)], "optimized once, no copies");
        assert!(interpreter.jit_statistics().optimized_bodies >= 1);
    }
}

/// `sum`, a private method of `A` summing `A`'s slot in a loop, sent
/// lexically ten times by the public `total`, run on an `A` and on a `B`,
/// a subclass of `A` with a slot of its own: a private lexical site,
/// whose cell's guard is `A`'s application word, reaching one body on two
/// receivers' applications.
const A_PRIVATE_METHOD_ON_TWO_APPLICATIONS: &str = indoc! {"
    class Subject = Object new () (
      class A step: value = Object new (
        |
          step = value.
        |
      ) (
        private sum = (
          |
            total ::= 0.
            index ::= 0.
          |
          [index < 100] whileTrue: [
            total:: total + step.
            index:: index + 1.
          ].
          ^total.
        )

        public total = (
          |
            result ::= 0.
            index ::= 0.
          |
          [index < 10] whileTrue: [
            result:: result + sum.
            index:: index + 1.
          ].
          ^result.
        )
      )

      class B = A step: 3 (
        |
          own = 5.
        |
      ) ()

      public aClass = (
        ^A.
      )

      public run = (
        ^(A step: 2) total + B new total.
      )
    ) : ()
"};

/// The machine entries `selector`'s body's dispatching rows name, and
/// those of its customized copies, sorted alike, for a comparison of the
/// two sets.
fn dispatching_entries(
    interpreter: &mut Interpreter,
    class: &Root,
    selector: &str,
) -> (Vec<u64>, Vec<u64>) {
    let (mut rows, copies) =
        interpreter.jit_dispatching_entries(class, MethodSide::Instance, Symbol::intern(selector));
    rows.sort_unstable();
    (rows, copies)
}

ns_runtime::for_each_lowering! {
    /// A private method sent lexically reaches each receiver's copy: the
    /// site's cell, guarded by `A`'s application word, names the body's
    /// dispatching entry once it has a copy, whose rows hold the copy of
    /// an `A`'s and of a `B`'s.
    fn a_private_method_reaches_each_receiver_s_copy(lowering) {
        let (mut interpreter, class) =
            load_customizing(A_PRIVATE_METHOD_ON_TWO_APPLICATIONS, lowering);
        interpreter.set_jit_counting(true);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), 5000);
        }
        if !counts_the_copies() {
            return;
        }
        let base_class = class_answered(&mut interpreter, &subject, "aClass");
        assert_eq!(
            interpreter.jit_customized_copies(&base_class, MethodSide::Instance, Symbol::intern("sum")),
            2
        );
        let (rows, copies) = dispatching_entries(&mut interpreter, &base_class, "sum");
        assert_eq!(copies.len(), 2);
        assert_eq!(rows, copies, "a row for each copy");
        // The optimized lowering's cell names the entry; the reference
        // lowering's sends are uncached, and its interpreted send enters
        // the entry through the call stub (`jit::called`).
        assert!(interpreter.jit_statistics().dispatched_calls > 0);
    }
}

/// `total`, a method of `A` summing `A`'s slot in a loop, which `B`, a
/// subclass of `A` with a slot of its own, overrides by a `super` send:
/// the `super` site's cell, guarded by `A`'s application word, reaches
/// `A`'s body on a `B`, whose copy is customized for a `B` entered at `A`.
const A_SUPER_SEND_ON_A_SUBCLASS: &str = indoc! {"
    class Subject = Object new () (
      class A step: value = Object new (
        |
          step = value.
        |
      ) (
        public total = (
          |
            sum ::= 0.
            index ::= 0.
          |
          [index < 100] whileTrue: [
            sum:: sum + step.
            index:: index + 1.
          ].
          ^sum.
        )
      )

      class B = A step: 3 (
        |
          own = 5.
        |
      ) (
        public total = (
          ^super total + 1.
        )
      )

      public aClass = (
        ^A.
      )

      public run = (
        |
          b = B new.
          result ::= 0.
          index ::= 0.
        |
        [index < 10] whileTrue: [
          result:: result + b total.
          index:: index + 1.
        ].
        ^(A step: 2) total + result.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A `super` send reaches its copy: `A`'s `total` run on a `B` by
    /// `B`'s `super total` has the copy for a `B` entered at `A`, which the
    /// body's dispatching rows name, as they name an `A`'s own.
    fn a_super_send_reaches_its_copy(lowering) {
        let (mut interpreter, class) = load_customizing(A_SUPER_SEND_ON_A_SUBCLASS, lowering);
        interpreter.set_jit_counting(true);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), 3210);
        }
        if !counts_the_copies() {
            return;
        }
        let base_class = class_answered(&mut interpreter, &subject, "aClass");
        let (rows, copies) = dispatching_entries(&mut interpreter, &base_class, "total");
        assert!(!copies.is_empty(), "the body has a copy");
        assert_eq!(rows, copies, "a row for each copy");
        // The optimized lowering's cell names the entry; the reference
        // lowering's sends are uncached, and its interpreted send enters
        // the entry through the call stub (`jit::called`).
        assert!(interpreter.jit_statistics().dispatched_calls > 0);
    }
}

/// `pick:`, a method of `A` that answers `other` when its argument is
/// `true` and sums a loop otherwise, run on `B1` to `B9`, nine subclasses
/// of `A` and so nine receivers' applications: each its own copy, whose
/// `other` site, never executed while the argument was `false`, traps.
/// A method entered once, its first entry interpreted, sends `pick:` as
/// an interpreted send, which calls the body through the call stub
/// (`jit::called`); so does every send under the reference lowering,
/// whose sends are uncached.
const PICK_ON_NINE_APPLICATIONS: &str = indoc! {"
    class Subject = Object new (
      |
        receivers = {B1 new. B2 new. B3 new. B4 new. B5 new. B6 new. B7 new. B8 new. B9 new}.
        chosen ::= 1.
      |
    ) (
      class A = Object new () (
        public pick: flag = (
          |
            sum ::= 0.
            index ::= 0.
          |
          flag ifTrue: [^self other].
          [index < 100] whileTrue: [
            sum:: sum + index.
            index:: index + 1.
          ].
          ^sum.
        )

        public other = (
          ^7.
        )
      )

      class B1 = A new () ()

      class B2 = A new () ()

      class B3 = A new () ()

      class B4 = A new () ()

      class B5 = A new () ()

      class B6 = A new () ()

      class B7 = A new () ()

      class B8 = A new () ()

      class B9 = A new () ()

      class C = A new () ()

      public aClass = (
        ^A.
      )

      public pickTwo = (
        ^((receivers at: 1) pick: false) + ((receivers at: 2) pick: false).
      )

      public pickAll = (
        ^pickEach: false.
      )

      public choose: index = (
        chosen:: index.
      )

      public pickChosenTaking = (
        ^(receivers at: chosen) pick: true.
      )

      public pickFirstTaking = (
        ^(receivers at: 1) pick: true.
      )

      public pickOnAnother = (
        ^C new pick: true.
      )

      pickEach: flag = (
        |
          total ::= 0.
          index ::= 1.
        |
        [index <= receivers size] whileTrue: [
          total:: total + ((receivers at: index) pick: flag).
          index:: index + 1.
        ].
        ^total.
      )
    ) : ()
"};

/// Whether a copy's trap is what a send that runs it takes: no stress
/// mode changes the copy, and none forces the exits that would leave it
/// before its trap.
fn a_copy_runs_to_its_trap() -> bool {
    counts_the_copies() && !the_stress_mode_is_on("DEOPT")
}

/// Ends `load_customizing`'s stress once its copies are made: no key is
/// optimized from here on, and a body is compiled at its second entry
/// again, so that a method entered once runs interpreted.
fn stop_customizing(interpreter: &mut Interpreter) {
    interpreter.set_optimize_stress(false);
    interpreter.set_jit_stress(false);
}

/// An interpreted send into a body with a copy for its receiver enters
/// the body's dispatching entry, whose rows find the copy: the answer
/// holds, and the calls through the entry are counted.
#[test]
fn an_interpreted_send_reaches_its_copy_through_the_dispatching_entry() {
    let (mut interpreter, class) = load_customizing(PICK_ON_NINE_APPLICATIONS, Lowering::Reference);
    interpreter.set_jit_counting(true);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    for _ in 0..3 {
        assert_eq!(send_unary(&mut interpreter, &subject, "pickTwo"), 9900);
    }
    if !counts_the_copies() {
        return;
    }
    let base_class = class_answered(&mut interpreter, &subject, "aClass");
    assert_eq!(
        interpreter.jit_customized_copies(
            &base_class,
            MethodSide::Instance,
            Symbol::intern("pick:")
        ),
        2
    );
    let before = interpreter.jit_statistics().dispatched_calls;
    assert_eq!(send_unary(&mut interpreter, &subject, "pickTwo"), 9900);
    assert!(
        interpreter.jit_statistics().dispatched_calls >= before + 2,
        "each interpreted send enters its copy through the entry"
    );
}

/// An interpreted send whose receiver has no copy of a body that has
/// copies for others finds no row and runs the baseline code, with the
/// right answer, so that no copy traps at its `other` site; one whose
/// receiver has a copy runs it to that trap.
#[test]
fn an_interpreted_send_without_a_copy_runs_the_baseline_code() {
    let (mut interpreter, class) = load_customizing(PICK_ON_NINE_APPLICATIONS, Lowering::Optimized);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    for _ in 0..3 {
        assert_eq!(send_unary(&mut interpreter, &subject, "pickTwo"), 9900);
    }
    // No copy is made for a `C`, whose first send spends no budget now.
    stop_customizing(&mut interpreter);
    let before = interpreter.jit_statistics().traps;
    assert_eq!(send_unary(&mut interpreter, &subject, "pickOnAnother"), 7);
    let between = interpreter.jit_statistics().traps;
    assert_eq!(send_unary(&mut interpreter, &subject, "pickFirstTaking"), 7);
    if !a_copy_runs_to_its_trap() {
        return;
    }
    let base_class = class_answered(&mut interpreter, &subject, "aClass");
    assert_eq!(
        interpreter.jit_customized_copies(
            &base_class,
            MethodSide::Instance,
            Symbol::intern("pick:")
        ),
        2
    );
    assert_eq!(between, before, "no copy ran the send on a `C`");
    assert_eq!(
        interpreter.jit_statistics().traps,
        between + 1,
        "a `B1`'s copy ran the send on a `B1`"
    );
}

/// A body with more copies than its dispatching rows hold: an interpreted
/// send reaches every receiver's copy all the same, the one past the
/// eighth row too, by the key's lookup. Each receiver is tried in an
/// interpreter of its own, with all nine copies installed, since a
/// second trap at the `other` site makes the copy that takes it not
/// entrant: the copy runs to its trap there, which the baseline code a
/// send past the rows would run does not take.
#[test]
fn an_interpreted_send_reaches_a_copy_past_the_dispatching_rows() {
    for index in 1..=9 {
        let (mut interpreter, class) =
            load_customizing(PICK_ON_NINE_APPLICATIONS, Lowering::Optimized);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "pickAll"), 44550);
        }
        stop_customizing(&mut interpreter);
        let chosen = interpreter.enter(|mutator| mutator.stash(Value::small_integer(index)));
        expect_ok(
            interpreter.send(&subject, Symbol::intern("choose:"), &[chosen]),
            "choose:",
        );
        let base_class = class_answered(&mut interpreter, &subject, "aClass");
        let copies = interpreter.jit_customized_copies(
            &base_class,
            MethodSide::Instance,
            Symbol::intern("pick:"),
        );
        let before = interpreter.jit_statistics().traps;
        assert_eq!(
            send_unary(&mut interpreter, &subject, "pickChosenTaking"),
            7
        );
        if !a_copy_runs_to_its_trap() {
            continue;
        }
        assert_eq!(copies, 9, "a copy for each receiver");
        assert_eq!(
            interpreter.jit_statistics().traps,
            before + 1,
            "the send on receiver {index} ran its copy"
        );
    }
}

ns_runtime::for_each_lowering! {
    /// A compaction of the code zone moves the copies a body's
    /// dispatching rows name, and the rows move with them: under the zone
    /// stress mode, which compacts at its safepoints, the program's answer
    /// holds at every run, and after a compaction between runs the rows
    /// still name exactly the copies the zone holds.
    fn a_compaction_remaps_the_dispatching_rows(lowering) {
        let (mut interpreter, class) =
            load_customizing(A_PRIVATE_METHOD_ON_TWO_APPLICATIONS, lowering);
        interpreter.set_stress_zone(true);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..4 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), 5000);
        }
        interpreter.set_stress_zone(false);
        assert_eq!(send_unary(&mut interpreter, &subject, "run"), 5000);
        let base_class = class_answered(&mut interpreter, &subject, "aClass");
        let before = dispatching_entries(&mut interpreter, &base_class, "sum");
        interpreter.compact_jit_zone();
        let (rows, copies) = dispatching_entries(&mut interpreter, &base_class, "sum");
        assert_eq!(rows, copies, "the rows name the copies where they lie now");
        assert_eq!(send_unary(&mut interpreter, &subject, "run"), 5000);
        if counts_the_copies() {
            assert!(!before.1.is_empty(), "the body had copies before the compaction");
        }
    }
}

/// The squares of 1 to 100,000 summed by a loop that sends `square:` for
/// each, whose send is an interrupt point of its optimized code: every
/// call pushes `square:` into its optimized code afresh, so that the
/// deopt stress mode's countdown meets its point at every turn.
const SUM_OF_SQUARES_BY_SENDS: &str = indoc! {"
    class Subject = Object new () (
      square: value = (
        ^self times: value with: value
      )
      times: left with: right = (
        ^left * right
      )
      public run = (
        |
          sum ::= 0.
          index ::= 1.
        |
        [index <= 100000] whileTrue: [
          sum:: sum + (self square: index).
          index:: index + 1.
        ].
        ^sum.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// Under the deopt stress mode optimized code leaves at every n-th
    /// interrupt point it passes, writing its frame back and handing the
    /// instruction to the interpreter, and the answer is the
    /// interpreter's. With the JIT stress mode, as `run_optimizing`, so
    /// that the reference lowering's bodies, which exit at once at their
    /// uncached sends, are compiled and optimized too.
    fn under_the_deopt_stress_every_point_leaves_and_the_answer_holds(lowering) {
        let (mut interpreter, class) = load_with(SUM_OF_SQUARES_BY_SENDS, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_deopt_stress(true);
        assert_eq!(run(&mut interpreter, &class), expected_sum_of_squares());
        let statistics = interpreter.jit_statistics();
        // Under the zone stress mode the reference lowering's bodies spend
        // no budget: each is compiled afresh at its push, which spends
        // nothing, and exits at its first send, uncached, before it
        // spends at a back jump (`spends_no_budget_in_a_loop`).
        if lowering == Lowering::Reference && the_zone_stress_mode_is_on() {
            return;
        }
        assert!(
            statistics.forced_exits > 100,
            "{} forced exits of optimized code",
            statistics.forced_exits
        );
    }
}

/// The deopt stress mode's forced exits never count against the optimized
/// code's `EXIT_BUDGET`, as the exit stress mode's forced poll exits never
/// do: outside the JIT stress mode, which abandons nothing, `square:`'s
/// optimized code leaves thousands of times and stays installed. The
/// optimized lowering's: the reference lowering's uncached sends exit at
/// once, so that no body of it is compiled outside the JIT stress mode.
#[test]
fn the_deopt_stress_s_forced_exits_abandon_nothing() {
    let (mut interpreter, class) = load_with(SUM_OF_SQUARES_BY_SENDS, Lowering::Optimized);
    interpreter.set_deopt_stress(true);
    assert_eq!(run(&mut interpreter, &class), expected_sum_of_squares());
    let statistics = interpreter.jit_statistics();
    assert!(
        statistics.forced_exits > 100,
        "{} forced exits of optimized code",
        statistics.forced_exits
    );
    if !a_stress_mode_is_on() {
        assert_eq!(statistics.abandoned_optimized_bodies, 0, "{statistics:?}");
    }
}

/// `sweep:` runs a loop of `turns` turns whose every turn sends `step:`
/// three times -- three interrupt points of its optimized code, and its
/// back edge's poll a fourth -- while `step:`, which only answers its
/// argument, has none: `run` calls it a hundred times, each call one
/// long activation of `sweep:`'s optimized code, so that the points it
/// passes are the only ones counted down between two safepoints.
const LONG_OPTIMIZED_LOOP: &str = indoc! {"
    class Subject = Object new () (
      step: value = (
        ^value
      )
      sweep: turns = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < turns] whileTrue: [
          sum:: sum + (self step: index) + (self step: 1) + (self step: 2).
          index:: index + 1.
        ].
        ^sum
      )
      public run = (
        |
          total ::= 0.
          call ::= 0.
        |
        [call < 100] whileTrue: [
          total:: total + (self sweep: 300).
          call:: call + 1.
        ].
        ^total
      )
    ) : ()
"};

/// The deopt stress mode forces exits deep in one long activation of
/// optimized code, not only within its first few points: a forced exit
/// leaves the activation's optimized code for good, so that its n-th
/// point is forced only by a countdown of n, and the draw reaches
/// 987 -- the 247th turn of `sweep:`'s loop, past many a write-back of the
/// values its back edge carries. The optimized lowering's, which inlines
/// the loop's blocks into `sweep:`.
/// A first run, with nothing optimized, records every site, so that the
/// optimized code speculates on none: compiled at its first spend, before
/// its sends ran, it would trap at the first, and each activation would
/// leave it at its first turn.
#[test]
fn the_deopt_stress_forces_exits_deep_in_a_long_optimized_activation() {
    let (mut interpreter, class) = load_with(LONG_OPTIMIZED_LOOP, Lowering::Optimized);
    // Whatever stress mode the environment sets, the first run optimizes
    // nothing.
    interpreter.set_optimize_stress(false);
    let turn = (0..300).sum::<i64>() + 3 * 300;
    assert_eq!(run(&mut interpreter, &class), 100 * turn);
    interpreter.set_deopt_stress(true);
    assert_eq!(run(&mut interpreter, &class), 100 * turn);
    let statistics = interpreter.jit_statistics();
    assert!(statistics.forced_exits > 0, "no forced exit");
    // Under a mode beside it whose every poll leaves machine code, each
    // activation of `sweep:` leaves its optimized code for good at its
    // first back edge, and the safepoint there sets the next countdown,
    // so that no point past the first few is ever reached.
    if a_stress_mode_beside_deopt_polls_out() {
        return;
    }
    assert!(
        statistics.forced_exits >= 50,
        "{} forced exits",
        statistics.forced_exits
    );
    assert!(
        statistics.deepest_forced_exit >= 610,
        "the deepest forced exit at point {}",
        statistics.deepest_forced_exit
    );
}

/// `turn:` sending each of `CALLEES` loopless callees once in its first
/// two turns, which fill the sends' caches and compile the callees, and
/// once a turn from the `STAGGER * index`-th on, which `run`'s loop calls `TURNS` times,
/// so that `turn:` runs in machine code: with a budget of
/// `OPTIMIZING_BUDGET`, the callees' budgets run out one after another,
/// each in an activation of `turn:` of its own, each request raising a
/// real safepoint request, at which `turn:`'s next compiled send is
/// refused at the callee's prologue.
fn callees_optimized_one_after_another() -> String {
    let callees: String = (0..CALLEES)
        .map(|index| {
            formatdoc! {"
                  callee{index}: value = (
                    ^value + {index}
                  )
            "}
        })
        .collect();
    let sends: String = (0..CALLEES)
        .map(|index| {
            let from = STAGGER * index;
            format!(
                "    ((value < 2) | (value >= {from})) ifTrue: [sum:: sum + (self callee{index}: value)].\n"
            )
        })
        .collect();
    formatdoc! {"
        class Subject = Object new () (
        {callees}  turn: value = (
            | sum ::= 0. |
        {sends}    ^sum
          )
          public run = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < {TURNS}] whileTrue: [
              sum:: sum + (self turn: index).
              index:: index + 1.
            ].
            ^sum
          )
        ) : ()
    "}
}

/// More callees than a body's `EXIT_BUDGET` of 64 exits.
const CALLEES: i64 = 100;

/// The turns between the first sends of two callees.
const STAGGER: i64 = 30;

/// Enough turns for the last callee to run its budget out.
const TURNS: i64 = 6000;

/// A push refused at a callee's prologue because a real safepoint request
/// lowered the depth limit word -- here the optimization request of a
/// callee whose budget ran out -- is not counted against the caller,
/// whose code it says nothing of: a caller whose callees keep requesting
/// their optimization is not abandoned, however many there are.
#[test]
fn a_caller_whose_callees_keep_requesting_optimization_is_not_abandoned() {
    let source = callees_optimized_one_after_another();
    let (mut interpreter, class) = load_spending_with(&source, Lowering::Optimized);
    interpreter.set_optimization_budget(OPTIMIZING_BUDGET);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let expected: i64 = (0..CALLEES)
        .map(|index| {
            (0..TURNS)
                .filter(|value| *value < 2 || *value >= STAGGER * index)
                .map(|value| value + index)
                .sum::<i64>()
        })
        .sum();
    assert_eq!(send_unary(&mut interpreter, &subject, "run"), expected);
    let statistics = interpreter.jit_statistics();
    if a_stress_mode_is_on() {
        return;
    }
    assert!(
        statistics.optimization_requests > 64,
        "{} optimization requests",
        statistics.optimization_requests
    );
    assert!(statistics.pushes_refused > 64, "{statistics:?}");
    assert_eq!(statistics.abandoned_bodies, 0, "{statistics:?}");
    assert_eq!(statistics.abandoned_optimized_bodies, 0, "{statistics:?}");
}

/// `sweep:`, compiled by its first two calls, whose loops turn no turn,
/// then called once more to turn `CALLEES` turns, each of which sends a
/// callee it never sent before: every one of those calls finds a callee
/// that has no machine code yet, and every refusal leaves `sweep:`'s
/// frame to the interpreter until its loop's back edge enters the
/// machine code again.
fn a_caller_of_callees_never_compiled() -> String {
    let callees: String = (0..CALLEES)
        .map(|index| {
            formatdoc! {"
                  callee{index}: value = (
                    ^value + {index}
                  )
            "}
        })
        .collect();
    let sends: String = (0..CALLEES)
        .map(|index| {
            format!(
                "      index = {index} ifTrue: [sum:: sum + (self callee{index}: 1)].
"
            )
        })
        .collect();
    formatdoc! {"
        class Subject = Object new () (
        {callees}  sweep: turns = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < turns] whileTrue: [
        {sends}      index:: index + 1.
            ].
            ^sum
          )
          public run = (
            ^(self sweep: 0) + (self sweep: 0) + (self sweep: {CALLEES})
          )
        ) : ()
    "}
}

/// A call the miss leaf refuses because its callee has no machine code
/// yet -- never compiled, or freed by a compaction and counting its
/// entries again -- is not counted against the caller, whose code it
/// says nothing of: a caller of more such callees than its exit budget
/// is not abandoned. Under the zone stress mode, whose compactions free
/// every callee between two of its caller's calls, every call of a loop
/// was refused this way, and the loop's body abandoned.
#[test]
fn a_caller_of_callees_never_compiled_is_not_abandoned() {
    let source = a_caller_of_callees_never_compiled();
    let (mut interpreter, class) = load_with(&source, Lowering::Optimized);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let expected: i64 = (0..CALLEES).map(|index| 1 + index).sum();
    assert_eq!(send_unary(&mut interpreter, &subject, "run"), expected);
    let statistics = interpreter.jit_statistics();
    // The zone stress mode alone frees every callee between two calls,
    // the very case the rule is for.
    if !a_stress_mode_is_on() || only_the_zone_stress_mode_is_on() {
        assert_eq!(statistics.abandoned_bodies, 0, "{statistics:?}");
    }
    if a_stress_mode_is_on() {
        return;
    }
    assert!(statistics.pushes_refused >= 64, "{statistics:?}");
}

/// Whether the zone stress mode is the one stress mode the environment
/// turns on.
fn only_the_zone_stress_mode_is_on() -> bool {
    the_zone_stress_mode_is_on()
        && std::env::vars_os().all(|(name, value)| {
            let name = name.to_string_lossy();
            !name.starts_with("NS_NEXT_STRESS_")
                || name == "NS_NEXT_STRESS_ZONE"
                || value.is_empty()
                || value == "0"
        })
}

/// `sweep:`, compiled by its first two calls, whose loops turn no turn,
/// then called once more to turn `TURNS_PAST_THE_BUDGET` turns, each of
/// which calls `large:`, a body of many additions and no send to compile
/// beside it; `measureSweep` and `measureLarge` compile only `sweep:` and
/// only `large:`, which the second of their two calls compiles.
fn a_caller_of_a_callee_the_zone_cannot_hold() -> String {
    let additions: String = (0..200)
        .map(|index| format!("    sum:: sum + value + {index}.\n"))
        .collect();
    formatdoc! {"
        class Subject = Object new () (
          large: value = (
            | sum ::= 0. |
        {additions}    ^sum
          )
          sweep: turns = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < turns] whileTrue: [
              sum:: sum + (self large: index).
              index:: index + 1.
            ].
            ^sum
          )
          public measureSweep = (
            ^(self sweep: 0) + (self sweep: 0)
          )
          public measureLarge = (
            ^(self large: 0) + (self large: 0)
          )
          public run = (
            ^(self sweep: 0) + (self sweep: 0) + (self sweep: {TURNS_PAST_THE_BUDGET})
          )
        ) : ()
    "}
}

/// More turns than a body's `EXIT_BUDGET` of 64 exits.
const TURNS_PAST_THE_BUDGET: i64 = 200;

/// How many bytes of a fresh zone `selector` leaves used past its stubs,
/// sent to a fresh instance of `source`'s class.
fn zone_bytes_compiled_by(source: &str, selector: &str) -> usize {
    let (mut interpreter, class) = load_with(source, Lowering::Optimized);
    interpreter.set_jit_zone_bytes(1 << 20);
    let stubs = interpreter.table_sizes().zone_used_bytes;
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    send_unary(&mut interpreter, &subject, selector);
    interpreter.table_sizes().zone_used_bytes - stubs
}

/// A call refused for a callee whose compile the exhausted zone holds is
/// counted against its caller: the zone retries the compile only after a
/// growing number of held ones, so that a hot caller would exit at every
/// call, and its abandonment stops that, and gives the zone room back.
/// The zone holds its stubs, `large:` and half of `sweep:`: `large:`
/// fits the zone emptied -- a body larger than that is refused for good,
/// and its calls refused by the refusing stub -- but not beside
/// `sweep:`, which the compaction keeps while it runs.
#[test]
fn a_caller_of_a_callee_the_exhausted_zone_holds_is_abandoned() {
    let source = a_caller_of_a_callee_the_zone_cannot_hold();
    let expected: i64 = (0..TURNS_PAST_THE_BUDGET)
        .map(|value| (0..200).map(|index| value + index).sum::<i64>())
        .sum();
    let sweep_bytes = zone_bytes_compiled_by(&source, "measureSweep");
    let large_bytes = zone_bytes_compiled_by(&source, "measureLarge");
    let (mut interpreter, class) = load_with(&source, Lowering::Optimized);
    interpreter.set_jit_zone_bytes(1 << 20);
    let stubs = interpreter.table_sizes().zone_used_bytes;
    let checked = !a_stress_mode_is_on();
    if checked {
        interpreter.set_jit_zone_bytes(stubs + large_bytes + sweep_bytes / 2);
    }
    assert_eq!(run(&mut interpreter, &class), expected);
    if !checked {
        return;
    }
    let statistics = interpreter.jit_statistics();
    assert!(statistics.abandoned_bodies >= 1, "{statistics:?}");
    assert_eq!(statistics.refused_bodies, 0, "{statistics:?}");
}

/// Holds `statistics` to some body optimized -- but under a stress mode
/// that keeps a loop's bodies from spending their budgets
/// (`spends_no_budget_in_a_loop`).
fn assert_some_optimized(statistics: &JitStatistics, lowering: Lowering) {
    if spends_no_budget_in_a_loop(lowering) {
        return;
    }
    assert!(statistics.optimized_bodies >= 1, "a body is optimized");
}

/// A loop calling `guarded` 10,000 times, whose budget runs out after a
/// few hundred calls: from then on the loop's calls enter its optimized
/// code, whose `on:do:` send's callee signals a resumable exception, so
/// that the exit materializes `guarded`'s optimized frame, and the
/// handler's `resume:` returns into it.
const SIGNAL_IN_A_HOT_LOOP: &str = indoc! {"
    class Subject = Object new () (
      warn = (
        ^(Exception signal) + 1
      )
      guarded = (
        ^[self warn] on: Exception do: [:e | e resume: 0]
      )
      public run = (
        |
          total ::= 0.
          index ::= 0.
        |
        [index < 10000] whileTrue: [
          total:: total + self guarded.
          index:: index + 1.
        ].
        ^total
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A frame of optimized code that an exit materialized is returned to
    /// in its baseline code, the only code with an entry at every
    /// instruction, and the answer is the interpreter's: in the second
    /// run, after the first optimized `guarded` and `warn`, every signal
    /// materializes frames that stood in optimized code, and the frames
    /// returned to are those of bodies with optimized code, which go on in
    /// their baseline code.
    fn a_frame_returned_to_resumes_in_baseline_code(lowering) {
        let (answers, statistics, last) = run_optimizing(SIGNAL_IN_A_HOT_LOOP, lowering, 2);
        assert_eq!(answers, vec![10_000; 2]);
        assert_some_optimized(&statistics, lowering);
        if pins_the_tiers(lowering) {
            assert!(
                last.optimized_frames_materialized >= 10_000,
                "{} frames of optimized code materialized",
                last.optimized_frames_materialized
            );
            assert!(
                last.optimized_bodies_resumed >= 10_000,
                "{} frames of optimized bodies resumed in baseline code",
                last.optimized_bodies_resumed
            );
        }
    }
}

/// A loop whose every turn calls `add:to:`, which adds, and from its
/// 9,001st call on makes a block capturing its arguments and has
/// `evaluate:` evaluate it: by then its budget has run out and it is
/// optimized, so that its optimized code makes the first of its closures.
const CLOSURE_IN_A_HOT_LOOP: &str = indoc! {"
    class Subject = Object new () (
      evaluate: block = (
        ^block value
      )
      add: index to: total = (
        index > 9000 ifTrue: [^self evaluate: [total + index]].
        ^total + index
      )
      public run = (
        |
          total ::= 0.
          index ::= 1.
        |
        [index <= 10000] whileTrue: [
          total:: self add: index to: total.
          index:: index + 1.
        ].
        ^total
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// `add:to:`'s optimized code makes its block as its lean baseline
    /// code does, by transferring its frame into the body's twin, which
    /// makes the closure: its first transfer finds no twin and calls the
    /// twin leaf, which compiles the baseline body's twin -- once, the
    /// second run's transfers finding it -- and the answer is the
    /// interpreter's.
    fn a_closure_made_in_optimized_code_transfers_into_the_twin(lowering) {
        let (answers, statistics, last) = run_optimizing(CLOSURE_IN_A_HOT_LOOP, lowering, 2);
        assert_eq!(answers, vec![50_005_000; 2]);
        assert_some_optimized(&statistics, lowering);
        assert!(statistics.compiled_twins >= 1);
        if pins_the_tiers(lowering) {
            assert_eq!(statistics.optimized_twin_requests, 1);
            assert_eq!(last.optimized_twin_requests, 0);
            assert_eq!(last.compiled_twins, 0, "the twin is compiled once");
        }
    }
}

/// A loop calling `step:at:` 3,100 times, whose budget runs out after a
/// few hundred calls, so that the loop's calls enter its optimized code;
/// from the 3,001st call on `step:at:` sends `probe:`, a site that ran on
/// `self` alone, to a `Stranger`, which answers it in its own
/// `doesNotUnderstand:`, whose handler machine code leaves to the
/// interpreter: an exit of the optimized code at every call. The site
/// ran, so its send is no trap: a send never executed is one, which
/// counts against no budget.
const EXITING_HOT_LOOP: &str = indoc! {"
    class Subject = Object new (
      | stranger = Stranger new. |
    ) (
      class Stranger = () (
        public doesNotUnderstand: message = (
          ^1
        )
      )
      step: target at: value = (
        ^target probe: value
      )
      public probe: value = (
        ^value
      )
      public run = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 3100] whileTrue: [
          sum:: sum + (self step: (index < 3000 ifTrue: [self] ifFalse: [stranger]) at: index).
          index:: index + 1.
        ].
        ^sum
      )
    ) : ()
"};

/// `source`'s `run` with the JIT on and every body's budget
/// `OPTIMIZING_BUDGET`, outside the JIT stress mode, which never abandons
/// a body; and what the JIT did meanwhile.
fn run_abandoning(source: &str, lowering: Lowering) -> (i64, JitStatistics) {
    let (mut interpreter, class) = load_spending_with(source, lowering);
    interpreter.set_optimization_budget(OPTIMIZING_BUDGET);
    interpreter.set_jit_counting(true);
    let before = interpreter.jit_statistics();
    let answer = run(&mut interpreter, &class);
    if let Err(message) = interpreter.verify_cells() {
        panic!("after the run: {message}");
    }
    (answer, since(interpreter.jit_statistics(), before))
}

ns_runtime::for_each_lowering! {
    /// `step:at:`'s optimized code exits at each of the last hundred calls:
    /// the exits count against its own budget, never its baseline code's,
    /// so that at the sixty-fourth its optimized code alone is abandoned
    /// and the rest of the calls run its baseline code, whose thirty-six
    /// exits abandon nothing. The reference lowering's sends are uncached
    /// and exit, so that its bodies are never compiled.
    fn an_optimized_body_that_keeps_exiting_is_abandoned_and_baseline_stays(lowering) {
        let (answer, statistics) = run_abandoning(EXITING_HOT_LOOP, lowering);
        assert_eq!(answer, 2999 * 3000 / 2 + 100);
        if pins_the_tiers(lowering) {
            assert!(statistics.optimized_bodies >= 1, "{statistics:?}");
            assert_eq!(statistics.optimized_exits, 64, "{statistics:?}");
            assert_eq!(statistics.abandoned_optimized_bodies, 1, "{statistics:?}");
            assert_eq!(statistics.abandoned_bodies, 0, "{statistics:?}");
            assert_eq!(statistics.traps, 0, "{statistics:?}");
        }
    }
}

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

ns_runtime::for_each_lowering! {
    /// `fib:` recurses without a loop. `run`, entered once, stays
    /// interpreted, and so does the first `fib:`, whose sends' pushes
    /// compile the body; from then
    /// on an interpreted frame's send of `fib:` calls its machine code
    /// (`jit::called`), whose own sends call it through their cells, so
    /// that the recursion runs in machine code below an interpreted root.
    fn a_recursion_rooted_in_an_interpreted_frame_runs_in_machine_code(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              public fib: n = (
                n < 2 ifTrue: [^1].
                ^(self fib: n - 1) + (self fib: n - 2).
              )
              public run = (
                ^self fib: 20.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        interpreter.set_jit_counting(true);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 10_946);
        let done = since(interpreter.jit_statistics(), before);
        let (mut interpreted, class) = load_with(source, lowering);
        interpreted.set_jit_enabled(false);
        assert_eq!(run(&mut interpreted, &class), 10_946);
        // The reference lowering's sends are uncached and leave machine
        // code; a stress mode leaves it far more often than the program
        // needs to.
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                done.called_entries > 0,
                "an interpreted send called `fib:`'s machine code"
            );
            assert!(
                done.compiled_calls > 10_000,
                "the recursion ran in machine code: {}",
                done.compiled_calls
            );
        }
    }
}

/// An interpreted `run:`'s send of `first:with:`, called in machine code
/// once `first:with:` is compiled, whose own send of `second:with:`
/// reaches, at the run's last round, the closure `second:with:` makes
/// there and runs, which machine code does not run: the exit materializes
/// `second:with:`'s frame and `first:with:`'s above the interpreted
/// sender, and the unhandled error the closure raises records the stack.
const CALLED_THEN_CLOSURE: &str = indoc! {"
    class Subject = Object new () (
      class Chain = () (
        public first: value with: chain = (
          ^(chain second: value + 1 with: chain) + 1
        )
        public second: value with: chain = (
          value > 100 ifTrue: [^[nil frobnicate] value].
          ^value * 2
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

/// The stack `CALLED_THEN_CLOSURE`'s unhandled error recorded, with the JIT
/// on or off, and what the JIT did meanwhile.
fn called_then_closure_stack(lowering: Lowering, jit: bool) -> (Vec<String>, JitStatistics) {
    let (mut interpreter, class) = load_with(CALLED_THEN_CLOSURE, lowering);
    interpreter.set_jit_enabled(jit);
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
    fn an_exit_in_a_called_body_materializes_it_above_its_interpreted_sender(lowering) {
        let (compiled, statistics) = called_then_closure_stack(lowering, true);
        let (interpreted, _) = called_then_closure_stack(lowering, false);
        assert_eq!(compiled, interpreted);
        // The suspension stress mode suspends every activation into its
        // frame object at a safepoint and resumes only the top one, and
        // the trace (`Interpreter::stack_trace`) names only the
        // activations on the stack: there, with the JIT on as off, it
        // holds only those pushed since the last suspension --
        // `defaultAction`'s alone -- and none of the two frames.
        if std::env::var_os("NS_NEXT_STRESS_SUSPEND")
            .is_some_and(|value| !value.is_empty() && value != "0")
        {
            return;
        }
        assert!(
            compiled.iter().any(|frame| frame.starts_with("second:with:")),
            "the innermost method's frame: {compiled:?}"
        );
        assert!(
            compiled.iter().any(|frame| frame.starts_with("first:with:")),
            "the called body's frame: {compiled:?}"
        );
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                statistics.called_entries > 0,
                "`run:`'s send called `first:with:`'s machine code"
            );
            assert!(
                statistics.materialized_frames >= 2,
                "`first:with:` and `second:with:` were compiled frames: {}",
                statistics.materialized_frames
            );
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

/// Whether the environment turns the zone stress mode on, which frees and
/// compiles again every body no activation runs at its safepoints, so that
/// a body's entries and calls are not those of its one compile.
fn the_zone_stress_mode_is_on() -> bool {
    std::env::var_os("NS_NEXT_STRESS_ZONE").is_some_and(|value| !value.is_empty() && value != "0")
}

/// The stress modes beside the deopt stress mode under which every poll
/// leaves machine code (`a_stress_mode_polls_out`): the exit stress
/// mode, whose polls exit unconditionally, and every mode that holds the
/// safepoint request, which a poll takes
/// (`Interpreter::hold_safepoints_for_stress`).
const MODES_THAT_POLL_OUT: [&str; 7] = [
    "EXIT", "MOVES", "CACHES", "COLLECT", "SCAVENGE", "SUSPEND", "ZONE",
];

/// Whether the environment turns `mode`'s stress mode on.
fn the_stress_mode_is_on(mode: &str) -> bool {
    std::env::var_os(format!("NS_NEXT_STRESS_{mode}"))
        .is_some_and(|value| !value.is_empty() && value != "0")
}

/// Whether the environment turns on a stress mode, other than the deopt
/// stress mode a test sets itself, under which every poll leaves machine
/// code.
fn a_stress_mode_beside_deopt_polls_out() -> bool {
    MODES_THAT_POLL_OUT
        .iter()
        .any(|mode| the_stress_mode_is_on(mode))
}

/// Whether the environment turns on a stress mode under which every poll
/// leaves machine code: the exit stress mode, whose polls exit
/// unconditionally, and every mode that holds the safepoint request,
/// which a poll takes (`Interpreter::hold_safepoints_for_stress`), and the
/// deopt stress mode, whose every forced exit holds it until the next
/// safepoint resets the countdown. The JIT's, the optimize and the wide
/// encoding's stress modes leave the polls alone.
fn a_stress_mode_polls_out() -> bool {
    a_stress_mode_beside_deopt_polls_out() || the_stress_mode_is_on("DEOPT")
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
        // The suspension stress mode suspends the frames below a
        // safepoint into their frame objects, which the trace
        // (`Interpreter::stack_trace`, the activations on the stack) does
        // not walk, with the JIT off as on.
        if std::env::var_os("NS_NEXT_STRESS_SUSPEND").is_some_and(|value| !value.is_empty() && value != "0") {
            return;
        }
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
    /// `step:`'s send of a message its class answers in its own
    /// `doesNotUnderstand:`, whose handler machine code leaves to the
    /// interpreter, leaves machine code inside the callee at every call: the exit materializes
    /// `loop:`'s frame too, and once `step:` returns, `loop:` is resumed in
    /// its machine code, so that its loop goes on there rather than in the
    /// interpreter.
    fn a_returned_to_frame_resumes_in_machine_code(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              class Stepper = () (
                public step: value = (
                  ^value printString size + self nothing
                )
                public doesNotUnderstand: message = (
                  ^0
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
    /// and the interpreter enters no other body but at its two pushes.
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
            // `Adder`'s accessor answers the class in its stub; `new`, the
            // factory's first send from machine code, has the miss count its
            // body's first entry and refuse the call, so that `run:` leaves
            // there; `add:`'s miss compiles it.
            assert_eq!(
                done.miss_leaf_calls, 2,
                "the two sends that found no machine entry"
            );
            assert!(done.compiled_calls >= 9_000, "{}", done.compiled_calls);
            assert_eq!(done.send_exits, 1, "`run:` leaves at the one send it does not make");
            assert_eq!(
                done.machine_entries, 2,
                "`run:`, compiled at the host's push, is entered there, and `sumWith:count:`, \
                 compiled at the interpreter's push after `run:` left, is entered there; `add:` \
                 only through compiled calls"
            );
        }
        // The factory's second send from machine code: its miss counts the
        // body's second entry, which no interpreter's push counted, and
        // compiles it, so that nothing leaves machine code.
        let before = interpreter.jit_statistics();
        assert_eq!(run_with(&mut interpreter, 10), 55);
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(done.send_exits, 0, "the factory is compiled and called");
            assert_eq!(done.exits, 0, "nothing left machine code");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A callee whose send of a message its class answers in its own
    /// `doesNotUnderstand:`, whose handler machine code leaves to the
    /// interpreter, leaves its machine code on every call: the interpreted loop of the first
    /// run calls it once it is compiled (`jit::called`), the exit
    /// materializes it and the interpreter runs the rest of it. Once it
    /// has spent its exit budget the JIT abandons it, and the loop's call
    /// of it in the loop's own machine code, compiled at its second entry
    /// and refused since, leaves at that send on every run and spends the
    /// loop's budget, so that the program goes on interpreted rather than
    /// paying an exit and an entry per turn -- but for a revival of
    /// `step:` now and then, each waiting twice as many interpreted entries
    /// as the one before.
    fn a_body_that_keeps_leaving_is_abandoned(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              twice: value = (
                ^value + value
              )
              step: value = (
                |
                  doubled = self twice: value.
                |
                ^doubled + self one.
              )
              public doesNotUnderstand: message = (
                ^1
              )
              public run = (
                |
                  sum ::= 0.
                  index ::= 0.
                |
                [index < 1000] whileTrue: [
                  sum:: sum + (self step: index).
                  index:: index + 1.
                ].
                ^sum.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 1_000_000);
        for _ in 0..80 {
            assert_eq!(run(&mut interpreter, &class), 1_000_000);
        }
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                done.resumed_entries < 200,
                "resumed only until the budgets are spent: {}",
                done.resumed_entries
            );
            // `step:`, then `run`; and `step:` again after each revival
            // its interpreted entries earn it, at 4,096 of them and then
            // twice as many each time: at most five in 81,000.
            assert!(done.abandoned_bodies >= 2, "`step:`, then `run`");
            assert!(done.revived_bodies <= 5, "revived {} times", done.revived_bodies);
            assert!(
                done.abandoned_bodies <= done.revived_bodies + 2,
                "abandoned again only after a revival: {done:?}"
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A setter's call stores in the setter stub, which writes the field
    /// through the inline barrier and answers the receiver, so that the
    /// next message sent to the setter's answer goes to it.
    fn a_setter_runs_in_the_setter_stub(lowering) {
        let source = indoc! {"
            class Subject = Object new (
              |
                box = Box new.
              |
            ) (
              class Box = (
                |
                  public contents ::= 0.
                |
              ) ()
              public run = (
                |
                  index ::= 0.
                |
                [index < 100] whileTrue: [
                  (box contents: box contents + index) contents: box contents + 1.
                  index:: index + 1.
                ].
                ^box contents.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        interpreter.set_jit_counting(true);
        assert_eq!(run(&mut interpreter, &class), 5050);
        let (answer, done) = run_measured(&mut interpreter, &class);
        assert_eq!(answer, 5050);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(done.leaf_calls, 0, "both setters of every turn store in their stub");
            assert_eq!(done.send_exits, 0, "no setter's call is refused");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// `Array>>at:` and `at:put:`, methods whose fast path is a leaf-safe
    /// mirror row with a stub of its own, run in that stub without
    /// deciding the send again; an index out of range, the eleventh
    /// turn's, goes on to the row's leaf, which declines, and the
    /// interpreter, running the send, raises the error it raises with the
    /// JIT off.
    fn a_mirror_method_runs_in_its_stub(lowering) {
        let source = indoc! {"
            class Subject = Object new (
              |
                elements = Array new: 10.
              |
            ) (
              public run = (
                |
                  index ::= 1.
                  sum ::= 0.
                |
                [index <= 10] whileTrue: [
                  elements at: index put: index.
                  sum:: sum + (elements at: index).
                  index:: index + 1.
                ].
                ^sum.
              )
              public overrun = (
                |
                  index ::= 1.
                  sum ::= 0.
                |
                [index <= 11] whileTrue: [
                  sum:: sum + (elements at: index).
                  index:: index + 1.
                ].
                ^sum.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        interpreter.set_jit_counting(true);
        assert_eq!(run(&mut interpreter, &class), 55);
        let (answer, done) = run_measured(&mut interpreter, &class);
        assert_eq!(answer, 55);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(done.leaf_calls, 0, "both sends of every turn run in their stubs");
            assert_eq!(done.send_exits, 0, "no call is refused");
        }
        let overrun = |interpreter: &mut Interpreter| {
            let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            match interpreter.send(&instance, Symbol::intern("overrun"), &[]) {
                Err(HostSignal::Error(error)) => error.message,
                _ => panic!("overrun: expected the index error"),
            }
        };
        let first = overrun(&mut interpreter);
        let before = interpreter.jit_statistics();
        let compiled = overrun(&mut interpreter);
        let done = since(interpreter.jit_statistics(), before);
        assert_eq!(compiled, first);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(done.machine_entries >= 1, "`overrun`, which loops, is entered");
            // The bodies that raise the index error, compiled by the first
            // overrun, are entered at their pushes and leave at sends of
            // their own.
            assert!(
                done.send_exits >= 1,
                "the eleventh `at:` declines and its call is refused"
            );
        }
        let (mut interpreted, interpreted_class) = load_with(source, lowering);
        interpreted.set_jit_enabled(false);
        let instance = expect_ok(interpreted.instantiate(&interpreted_class, &[]), "instantiate");
        match interpreted.send(&instance, Symbol::intern("overrun"), &[]) {
            Err(HostSignal::Error(error)) => assert_eq!(error.message, compiled),
            _ => panic!("overrun: expected the index error"),
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

ns_runtime::for_each_lowering! {
    /// The entry block is kept from one entry to the next, its global
    /// cache's mask among its words: after the cache is resized between
    /// two runs, the megamorphic stub probes the new one -- the answers
    /// are the interpreter's, and once the resized cache is filled a run
    /// meets the miss leaf nowhere, which a stale mask probing the wrong
    /// group would not.
    fn the_entry_block_follows_a_resized_global_cache(lowering) {
        let source = polymorphic_source(6);
        let (mut interpreter, class) = load_with(&source, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_jit_counting(true);
        let offsets: i64 = [1, 10, 100, 1000, 10_000, 100_000].iter().sum();
        let expected = 6 * 499_500 + 1000 * offsets;
        assert_eq!(run(&mut interpreter, &class), expected);
        interpreter.set_global_cache_size(4096);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), expected, "filling the resized cache");
        let middle = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), expected, "hitting in it");
        let filled = since(middle, before);
        let hitting = since(interpreter.jit_statistics(), middle);
        assert!(filled.machine_entries >= 1, "machine code ran after the resize");
        assert!(hitting.machine_entries >= 1, "machine code ran after the resize");
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(
                hitting.miss_leaf_calls, 0,
                "every send hit in machine code, the megamorphic one in the resized cache"
            );
        }
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
                statistics.act_leaf_calls <= 1,
                "`at:`, `at:put:` and the setter ran in their stubs, and only `do:`'s one \
                 `size` in its row's leaf: {}",
                statistics.act_leaf_calls
            );
        }
    }
}

/// A public setter sent a thousand times from a compiled loop, of field
/// 0 and of a field holding Arrays: its cell names the setter stub, which
/// stores through the inline barrier.
const SETTERS: &str = indoc! {"
    class Subject = Object new (
      | holder = Holder new. |
    ) (
      class Holder = Object new (
        |
          public count ::= 0.
          public item ::= nil.
        |
      ) ()
      bump: aHolder = (
        |
          index ::= 0.
        |
        [index < 1000] whileTrue: [
          aHolder count: aHolder count + 1.
          aHolder item: {index}.
          index:: index + 1.
        ].
        ^aHolder count + (aHolder item at: 1).
      )
      public run = (
        ^bump: holder
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A setter stores in its stub: the third run's two thousand setter
    /// sends call no leaf, and what they stored reads back as the
    /// interpreter's.
    fn a_setter_stores_without_a_leaf_call(lowering) {
        let (compiled, statistics) = run_thrice_on_one_instance(SETTERS, lowering, true);
        let (interpreted, _) = run_thrice_on_one_instance(SETTERS, lowering, false);
        assert_eq!(interpreted, [1999, 2999, 3999]);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "bump:");
            assert_eq!(statistics.send_exits, 0, "every send stayed in machine code");
            assert_eq!(statistics.leaf_calls, 0, "no setter called a leaf");
            assert!(
                statistics.compiled_calls >= 2000,
                "`bump:` calls its setters through their cells: {}",
                statistics.compiled_calls
            );
        }
    }
}

/// A private slot's setter and getter sent from a nested class, and a
/// slot's setter and getter sent to `super`, a thousand times each from
/// compiled loops: their cells name the guarded stubs, which compare the
/// receiver's guard with the one the entry recorded and store through the
/// inline barrier.
const GUARDED_ACCESSORS: &str = indoc! {"
    class Subject = Object new (
      |
        private hidden ::= 0.
        reader = Reader new.
        doubling = Doubling new.
      |
    ) (
      class Reader = Object new () (
        public sum = (
          |
            index ::= 0.
            total ::= 0.
          |
          [index < 1000] whileTrue: [
            hidden: {index}.
            total:: total + (hidden at: 1).
            index:: index + 1.
          ].
          ^total
        )
      )
      class Base = Object new (
        |
          public count ::= 0.
        |
      ) ()
      class Doubling = Base new () (
        public count: value = (
          super count: value * 2.
        )
        public count = (
          ^super count + 1
        )
        public sum = (
          |
            index ::= 0.
            total ::= 0.
          |
          [index < 1000] whileTrue: [
            self count: index.
            total:: total + self count.
            index:: index + 1.
          ].
          ^total
        )
      )
      public run = (
        ^reader sum + doubling sum
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A private and a `super` accessor run in their guarded stubs: the
    /// third run's four thousand accessor sends call no leaf, and the
    /// sums are the interpreter's.
    fn a_private_setter_stores_without_a_leaf_call(lowering) {
        let (compiled, statistics) =
            run_thrice_on_one_instance(GUARDED_ACCESSORS, lowering, true);
        let (interpreted, _) = run_thrice_on_one_instance(GUARDED_ACCESSORS, lowering, false);
        assert_eq!(interpreted, [1_499_500; 3]);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "sum");
            assert_eq!(statistics.send_exits, 0, "every send stayed in machine code");
            assert_eq!(statistics.leaf_calls, 0, "no private or `super` accessor called a leaf");
            assert!(
                statistics.compiled_calls >= 4000,
                "the loops call the accessors through their cells: {}",
                statistics.compiled_calls
            );
        }
    }
}

/// `at:put:` and `at:` on an Array and on a ByteArray, each sent a
/// thousand times from a compiled loop at every index, the Array's
/// elements Arrays themselves.
const INDEXING: &str = indoc! {"
    class Subject = Object new (
      |
        elements = Array new: 1000.
        bytes = ByteArray new: 1000.
      |
    ) (
      fill: array and: byteArray = (
        |
          index ::= 1.
          byte ::= 1.
          sum ::= 0.
        |
        [index <= 1000] whileTrue: [
          array at: index put: {index}.
          byteArray at: index put: byte.
          sum:: sum + ((array at: index) at: 1) + (byteArray at: index).
          index:: index + 1.
          byte:: byte + 1.
          byte = 256 ifTrue: [byte:: 0].
        ].
        ^sum.
      )
      public run = (
        ^fill: elements and: bytes
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// `at:put:` and `at:` run in their stubs: the third run's four
    /// thousand sends to the Array and the ByteArray call no leaf, and
    /// the sum is the interpreter's.
    fn at_put_on_an_array_runs_inline(lowering) {
        let (compiled, statistics) = run_thrice_on_one_instance(INDEXING, lowering, true);
        let (interpreted, _) = run_thrice_on_one_instance(INDEXING, lowering, false);
        let expected = 500_500 + (0..1000).map(|index| (index + 1) % 256).sum::<i64>();
        assert_eq!(interpreted, [expected; 3]);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "fill:and:");
            assert_eq!(statistics.send_exits, 0, "every send stayed in machine code");
            assert_eq!(statistics.leaf_calls, 0, "no `at:` or `at:put:` called a leaf");
            assert!(
                statistics.compiled_calls >= 4000,
                "`fill:and:` calls `at:` and `at:put:` through their cells: {}",
                statistics.compiled_calls
            );
        }
    }
}

/// `store:at:` compiled by a thousand stores in range, then sent an index
/// past the Array and one past the ByteArray.
const OUT_OF_RANGE: &str = indoc! {"
    class Subject = Object new (
      |
        elements = Array new: 10.
        bytes = ByteArray new: 10.
      |
    ) (
      store: indexable at: index = (
        indexable at: index put: 7.
      )
      public run = (
        |
          index ::= 0.
        |
        [index < 1000] whileTrue: [
          store: elements at: index \\\\ 10 + 1.
          store: bytes at: index \\\\ 10 + 1.
          index:: index + 1.
        ].
        ^0
      )
      public storePastArray = (
        store: elements at: 11.
      )
      public storePastBytes = (
        store: bytes at: 0.
      )
    ) : ()
"};

/// The message of the error `selector` raises, sent to a `Subject` of
/// `OUT_OF_RANGE` once `run` has compiled `store:at:` -- with the JIT on
/// or off.
fn out_of_range_message(selector: &str, jit: bool) -> String {
    let (mut interpreter, class) = load_with(OUT_OF_RANGE, Lowering::Optimized);
    interpreter.set_jit_enabled(jit);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("run"), &[]),
        "run",
    );
    match interpreter.send(&instance, Symbol::intern(selector), &[]) {
        Err(HostSignal::Error(error)) => error.message,
        Err(HostSignal::Return) => panic!("{selector}: a non-local return escaped"),
        Ok(_) => panic!("{selector}: the store past the end succeeded"),
    }
}

/// A store past the end of an Array or a ByteArray from compiled code
/// raises the interpreter's error: the stub hands every index it does not
/// store at to the row's leaf, which refuses, and the interpreter runs
/// the send again and raises it.
#[test]
fn at_put_out_of_range_fails_as_the_interpreter_does() {
    for selector in ["storePastArray", "storePastBytes"] {
        let compiled = out_of_range_message(selector, true);
        assert_eq!(compiled, out_of_range_message(selector, false));
        assert!(compiled.contains("out of range"), "{selector}: {compiled}");
    }
}

/// The rows marked leaf-safe -- `MachineCall::Leaf`, or `Stub` with a
/// stub of their own -- are exactly those this list names, each with why
/// its leaf is leaf-safe: a row marked without being read
/// fails here, and so does one the list names that is not marked.
#[test]
fn every_leaf_row_is_audited() {
    use ns_runtime::builtins::primitive::{MachineCall, RowStub, MIRROR};
    let marked: std::collections::BTreeSet<&str> = MIRROR
        .iter()
        .filter(|entry| entry.machine_call.is_leaf_safe())
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
    // A row stub falls back to the row the first entry marked with it
    // names (`jit::targets`'s `emit_row_stub`), so a second would be run
    // as the first's.
    for stub in RowStub::ALL {
        let rows: Vec<&str> = MIRROR
            .iter()
            .filter(|entry| entry.machine_call == MachineCall::Stub(stub))
            .map(|entry| entry.selector)
            .collect();
        assert_eq!(rows.len(), 1, "{stub:?} marks exactly one row: {rows:?}");
    }
}

/// Every mirror row machine code runs through the leaf shim, by why its
/// leaf is leaf-safe (`builtins::primitive::MachineCall::Leaf`): each was
/// read before it was marked, and a row marked since must be read and
/// added here.
const AUDITED_LEAF_ROWS: &[(&str, &[&str])] = &[
    (
        "a value's derived equality and hash read the states on a bounded work stack of their own, allocate no object and send nothing, or decline having written nothing",
        &["value:sameAs:ifFail:", "valueHash:ifFail:"],
    ),
    (
        "reads its operand's class word and answers a Boolean, never declining and writing nothing",
        &["objectIsFuture:ifFail:"],
    ),
    (
        "reads or writes only its future's two fields, through the write barrier, allocates nothing and pushes nothing, or declines having written nothing",
        &[
            "futureState:ifFail:",
            "futureComputation:ifFail:",
            "futureTakeOver:ifFail:",
            "future:resolveTo:ifFail:",
            "futureAbandon:ifFail:",
        ],
    ),
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

ns_runtime::for_each_lowering! {
    /// A private slot's accessors, sent in a loop to an instance of a
    /// subclass of the class whose instance filled the site, miss their
    /// guarded stubs' compare -- the entry recorded the first receiver's
    /// guard -- and run in the leaf stub's leaf straight from the entry of
    /// the send's own site: the cell the send hit is its own, at a site of
    /// the body's family, never site 0. Two subjects with private slots at
    /// different fields share no site, so a stub or a leaf that read
    /// another site's cell or entry would answer the other's field.
    fn a_private_accessor_runs_from_its_own_sites_entry(lowering) {
        let source = indoc! {"
            class Subject = Object new (
              |
                counter = Counter new.
                derived = Derived new.
                other = Other new.
              |
            ) (
              class Counter = (
                |
                  private padding ::= 0.
                  private count ::= 0.
                |
              ) (
                public step: index = (
                  count: count + index.
                  ^count
                )
              )
              class Derived = Counter new () ()
              class Other = (
                |
                  private count ::= 1000.
                |
              ) (
                public peek = (
                  ^count
                )
              )
              public run = (
                |
                  index ::= 0.
                  sum ::= 0.
                |
                [index < 200] whileTrue: [
                  sum:: sum + (counter step: index) + (derived step: index) + other peek.
                  index:: index + 1.
                ].
                ^sum.
              )
            ) : ()
        "};
        // One instance throughout, so that no instantiation -- whose
        // superclass initializer still exits -- falls into the third run.
        let (compiled, done) = run_thrice_on_one_instance(source, lowering, true);
        let (interpreted, _) = run_thrice_on_one_instance(source, lowering, false);
        let first = (0..200i64).map(|index| index * (index + 1) + 1000).sum::<i64>();
        assert_eq!(interpreted[0], first);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                done.site_slot_hits >= 400,
                "the private getter and setter of every `derived` turn run from their sites' own \
                 entries: {}",
                done.site_slot_hits
            );
            assert_eq!(done.send_exits, 0, "no accessor's call is refused");
            assert_eq!(done.machine_entries, 1, "`run` runs whole in machine code");
        }
    }
}

/// `count` classes whose `step:` sends a message their
/// `doesNotUnderstand:` answers, whose handler machine code leaves to the
/// interpreter, sent at one site of a loop in turn: with two to four the site
/// is polymorphic, its callee reached through its block's cells; with five
/// or more it is megamorphic, reached through the global cache's rows.
fn leaving_through(count: usize) -> String {
    let fills: String = (1..=count)
        .map(|index| {
            format!(
                "      steppers at: {index} put: Stepper{index} new.
"
            )
        })
        .collect();
    let classes: String = (2..=count)
        .map(|index| {
            format!(
                "      class Stepper{index} = Stepper1 () ()
"
            )
        })
        .collect();
    formatdoc! {"
        class Subject = Object new (
          |
            steppers = Array new: {count}.
          |
        {fills}    ) (
              class Stepper1 = () (
                twice: value = (
                  ^value + value
                )
                public step: value = (
                  |
                    doubled = self twice: value.
                  |
                  ^doubled + self one.
                )
                public doesNotUnderstand: message = (
                  ^1
                )
              )
        {classes}      public run = (
                |
                  sum ::= 0.
                  index ::= 0.
                |
                [index < 1000] whileTrue: [
                  sum:: sum + ((steppers at: index \\\\ {count} + 1) step: index).
                  index:: index + 1.
                ].
                ^sum.
              )
            ) : ()
    "}
}

ns_runtime::for_each_lowering! {
    /// A callee that keeps leaving, sent from a polymorphic site (three
    /// classes) and from a megamorphic one (five), is abandoned while the
    /// site is in that state, like one sent from a site with a cell of its
    /// own, and every cell that named its machine entry -- the site's
    /// block cells, the global rows -- is written
    /// afresh from its entry, so that every cell still agrees with its
    /// entry afterwards (`Interpreter::verify_cells`, which reads every
    /// site's cell, every block's cells and every global row).
    fn abandoned_bodies_leave_no_cell_naming_them(lowering) {
        for (count, polymorphic) in [(3, true), (5, false)] {
            let source = leaving_through(count);
            let (mut interpreter, class) = load_with(&source, lowering);
            let before = interpreter.jit_statistics();
            assert_eq!(run(&mut interpreter, &class), 1_000_000);
            for _ in 0..10 {
                assert_eq!(run(&mut interpreter, &class), 1_000_000);
            }
            let done = since(interpreter.jit_statistics(), before);
            if let Err(message) = interpreter.verify_cells() {
                panic!("{count} classes, after the abandonment: {message}");
            }
            if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
                assert!(
                    done.abandoned_bodies >= 1,
                    "{count} classes: `step:`, which exits on every call"
                );
                let states = interpreter.call_site_states(
                    &class,
                    MethodSide::Instance,
                    Symbol::intern("run"),
                    Symbol::intern("step:"),
                );
                let expected = |state: &CallSiteState| {
                    if polymorphic {
                        matches!(state, CallSiteState::Polymorphic(entries) if entries.len() == count)
                    } else {
                        matches!(state, CallSiteState::Megamorphic)
                    }
                };
                assert!(
                    states.iter().any(expected),
                    "{count} classes: the site's state"
                );
            }
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A callee found never compilable -- its body makes a future at once,
    /// which the test hook has machine code leave to the interpreter
    /// (`Interpreter::set_jit_futures_exit`), so it exits at once -- is
    /// refused by the miss leaf, which writes the cells the decision went
    /// through afresh, naming the refusing stub: the site's later sends
    /// exit at once rather than calling the miss leaf again. `run` sends
    /// it once, before its loop, which only makes `run` worth compiling:
    /// a call in the loop would leave machine code at every turn, the
    /// loop's backward jump switching back in (`jit::at_back_edge`), until
    /// `run` had spent its exit budget and were abandoned.
    fn a_callee_never_compiled_is_refused_without_the_miss_leaf(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              step: value = (
                ||
                  next = value + 1.
                ||
                ^next.
              )
              public run = (
                |
                  sum ::= self step: 0.
                  index ::= 0.
                |
                [index < 100] whileTrue: [
                  sum:: sum + index.
                  index:: index + 1.
                ].
                ^sum.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        interpreter.set_jit_futures_exit(true);
        assert_eq!(run(&mut interpreter, &class), 4951);
        assert_eq!(run(&mut interpreter, &class), 4951);
        let (answer, done) = run_measured(&mut interpreter, &class);
        assert_eq!(answer, 4951);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(done.machine_entries >= 1, "`run`, which loops, is entered");
            assert_eq!(done.send_exits, 1, "the call of `step:` is refused");
            assert_eq!(done.miss_leaf_calls, 0, "by the refusing stub, not the miss leaf");
        }
    }
}

/// `count` classes sharing a `step:` whose body makes a future at once, so
/// that it exits at once and is never compiled once the test hook has
/// machine code leave a future's making to the interpreter
/// (`Interpreter::set_jit_futures_exit`) -- sent at one site once per run,
/// before `run`'s loop, each run to the next class: with two to four the
/// site is polymorphic, its callee reached through its block's slots; with
/// five or more megamorphic, reached through the global cache's rows. The
/// loop only makes `run` worth compiling: a call in it would leave machine
/// code at every turn, the loop's backward jump switching back in
/// (`jit::at_back_edge`), until `run` had spent its exit budget and were
/// abandoned.
fn refused_through(count: usize) -> String {
    let fills: String = (1..=count)
        .map(|index| format!("      steppers at: {index} put: Stepper{index} new.\n"))
        .collect();
    let classes: String = (2..=count)
        .map(|index| format!("      class Stepper{index} = Stepper1 () ()\n"))
        .collect();
    formatdoc! {"
        class Subject = Object new (
          |
            steppers = Array new: {count}.
            start ::= 0.
          |
        {fills}    ) (
              class Stepper1 = () (
                public step: value = (
                  ||
                    next = value + 1.
                  ||
                  ^next.
                )
              )
        {classes}      public run = (
                |
                  sum ::= 0.
                  index ::= 0.
                |
                start: start + 1.
                sum:: (steppers at: start \\\\ {count} + 1) step: 0.
                [index < 100] whileTrue: [
                  sum:: sum + index.
                  index:: index + 1.
                ].
                ^sum.
              )
            ) : ()
    "}
}

ns_runtime::for_each_lowering! {
    /// The refusal of a callee never compiled rewrites the cell the send
    /// reached it through when that is a polymorphic block's slot (three
    /// classes) or a global row (five), not only a site's own cell: once
    /// every class's slot or row has been refused once -- each run's first
    /// call refused, from a different class -- a full turn of runs, each
    /// first calling another class, is refused by the refusing stub
    /// alone, without the miss leaf, and every cell agrees with its entry.
    /// A slot filled while the callee could still be compiled names the
    /// miss stub until the refusal rewrites it.
    fn a_callee_never_compiled_is_refused_through_a_block_or_a_row(lowering) {
        for count in [3, 5] {
            let source = refused_through(count);
            let (mut interpreter, class) = load_with(&source, lowering);
            interpreter.set_jit_futures_exit(true);
            let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            let run_once = |interpreter: &mut Interpreter| {
                let answer = expect_ok(
                    interpreter.send(&instance, Symbol::intern("run"), &[]),
                    "run",
                );
                interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
                    ValueKind::SmallInteger(integer) => integer,
                    _ => panic!("run: expected a small integer"),
                })
            };
            for _ in 0..count + 2 {
                assert_eq!(run_once(&mut interpreter), 4951);
            }
            // A full turn of the classes, each run's first call another's.
            let before = interpreter.jit_statistics();
            for _ in 0..count {
                assert_eq!(run_once(&mut interpreter), 4951);
            }
            let done = since(interpreter.jit_statistics(), before);
            if let Err(message) = interpreter.verify_cells() {
                panic!("{count} classes: {message}");
            }
            if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
                assert!(done.machine_entries >= 1, "{count} classes: `run` is entered");
                assert_eq!(
                    done.send_exits, count as u64,
                    "{count} classes: each run's call is refused"
                );
                assert_eq!(
                    done.miss_leaf_calls, 0,
                    "{count} classes: by the refusing stub, not the miss leaf"
                );
                let states = interpreter.call_site_states(
                    &class,
                    MethodSide::Instance,
                    Symbol::intern("run"),
                    Symbol::intern("step:"),
                );
                assert!(
                    states.iter().any(|state| if count <= 4 {
                        matches!(state, CallSiteState::Polymorphic(entries) if entries.len() == count)
                    } else {
                        matches!(state, CallSiteState::Megamorphic)
                    }),
                    "{count} classes: the site's state"
                );
            }
        }
    }
}

/// How many locals `fill:` keeps in its indirection vector: an `Array` of
/// 1022 fields, whose block -- an overflow word, the header, the class
/// word and the fields -- is 1025 words, about 8 KiB.
const VECTOR_LOCALS: usize = 1022;

/// How many vectors `run` makes: about three times what eden holds.
const VECTORS: usize = 3000;

/// `fill:` makes an indirection vector at its every activation, since the
/// block of its branch never taken writes every local; it writes two of
/// them and answers their sum read back from the vector. `run` sends it
/// `VECTORS` times from a loop and answers what the first and the last
/// vector answered. `run` is entered once, and the backward jumps'
/// trigger is set past its loop's turns (`run_vectors`), so it stays
/// interpreted and takes every safepoint the allocations request in its
/// own loop, and
/// `fill:`, compiled at its second entry, runs from its first instruction
/// to its return without a poll.
fn vectors_source() -> String {
    let locals: Vec<String> = (0..VECTOR_LOCALS)
        .map(|index| format!("v{index}"))
        .collect();
    let declarations = locals.join(" ");
    let writes: String = locals
        .iter()
        .map(|local| format!("{local}:: n. "))
        .collect();
    let last = &locals[VECTOR_LOCALS - 1];
    formatdoc! {"
        class Subject = Object new () (
          fill: n = (
            | {declarations} |
            n < 0 ifTrue: [^[{writes}]].
            v0:: n.
            {last}:: n + n.
            ^v0 + {last}
          )
          public run = (
            | first last index |
            index:: 0.
            [index < {VECTORS}] whileTrue: [
              index:: index + 1.
              last:: self fill: index.
              index = 1 ifTrue: [first:: last]].
            ^first + last
          )
        ) : ()
    "}
}

/// `vectors_source`'s `run` with the JIT on or off: its answer, what the
/// JIT did, and the heap's occupancy and scavenges after it.
fn run_vectors(jit: bool) -> (i64, JitStatistics, ns_runtime::HeapStatistics, usize) {
    let (mut interpreter, class) = load_with(&vectors_source(), Lowering::Optimized);
    interpreter.set_jit_enabled(jit);
    interpreter.set_jit_compile_at_back_jump(u32::MAX);
    let before = interpreter.jit_statistics();
    let scavenges = interpreter.collector_statistics().scavenges;
    let answer = run(&mut interpreter, &class);
    (
        answer,
        since(interpreter.jit_statistics(), before),
        interpreter.heap_statistics(),
        interpreter.collector_statistics().scavenges - scavenges,
    )
}

/// `MakeVector` in machine code bumps eden's cursor inline and, past its
/// limit, calls the leaf that decides as the interpreter's cold path
/// does: the same objects young and old, the same scavenges requested
/// and taken at the same safepoints, the same counts. The optimized
/// lowering only: the reference one sends `ifTrue:` with a closure, whose
/// creation leaves machine code.
#[test]
fn machine_code_allocates_past_eden_as_the_interpreter_does() {
    let (compiled, statistics, compiled_heap, compiled_scavenges) = run_vectors(true);
    let (interpreted, _, interpreted_heap, interpreted_scavenges) = run_vectors(false);
    let last = i64::try_from(VECTORS).expect("a small count");
    assert_eq!(interpreted, 3 + 3 * last);
    assert_eq!(compiled, interpreted);
    assert!(statistics.compiled_bodies > 0, "`fill:` is compiled");
    // A stress mode's collections empty eden before the vectors fill it.
    if !a_stress_mode_is_on() {
        assert!(
            interpreted_scavenges >= 2,
            "the vectors fill eden more than once: {interpreted_scavenges}"
        );
        assert_eq!(compiled_scavenges, interpreted_scavenges);
        assert_eq!(compiled_heap, interpreted_heap);
        // `fill:` runs to its return at every call: the scavenge its
        // allocation requests is served at `run`'s next backward jump,
        // which clears the request before the next call, so no prologue
        // finds it standing and refuses the call.
        assert_eq!(
            statistics.exits, 0,
            "no call leaves machine code: {statistics:?}"
        );
        assert_eq!(statistics.safepoint_exits, 0, "`fill:` has no poll");
    }
}

/// A closure whose captured variables live in an indirection vector:
/// `writer`, which `prepare` answers, writes fresh arrays and a count
/// into them in a loop, which machine code runs once the block is
/// compiled at its second entry, and `reader` reads them back. The vector
/// outlives `prepare`'s activation in both closures, so a full collection
/// between two sends tenures it. The host sends the writer `value:`
/// itself, since a send of it from machine code leaves for the
/// interpreter until closures run in machine code.
#[cfg(debug_assertions)]
const BARRIER_SOURCE: &str = indoc! {"
    class Subject = Object new (
      | writer reader |
    ) (
      public prepare = (
        | first second count |
        count:: 0.
        writer:: [:limit |
          | index |
          index:: 0.
          [index < limit] whileTrue: [
            first:: {index. limit}.
            second:: {limit. index. index + 1}.
            count:: count + 1.
            index:: index + 1].
          count].
        reader:: [(first at: 1) + (first at: 2) + (second at: 3) + count].
        ^writer
      )
      public read = (
        ^reader value
      )
    ) : ()
"};

/// How many stores of each array `BARRIER_SOURCE`'s writer makes in one
/// measured send: far fewer bytes than eden's limit holds, so that no
/// allocation asks for a scavenge.
#[cfg(debug_assertions)]
const BARRIER_STORES: i64 = 1000;

/// `BARRIER_SOURCE`'s subject and its writer, prepared, the writer run
/// three times so that the block is compiled, and everything tenured by
/// a full collection.
#[cfg(debug_assertions)]
fn tenured_writer() -> (Interpreter, Root, Root) {
    let (mut interpreter, class) = load_with(BARRIER_SOURCE, Lowering::Optimized);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let writer = expect_ok(
        interpreter.send(&subject, Symbol::intern("prepare"), &[]),
        "prepare",
    );
    for _ in 0..3 {
        send_integer(&mut interpreter, &writer, "value:", 3);
    }
    // A full collection between entries compacts both generations into the
    // old space: the vector is old from here on, and remembered by nothing.
    interpreter.collect_now();
    (interpreter, subject, writer)
}

/// Sends `selector` to `subject` with the `SmallInteger` `argument`, or
/// with none for a unary selector, answering the `SmallInteger` it
/// answers.
#[cfg(debug_assertions)]
fn send_integer(
    interpreter: &mut Interpreter,
    subject: &Root,
    selector: &str,
    argument: i64,
) -> i64 {
    let arguments: Vec<Root> = if selector.ends_with(':') {
        vec![interpreter.stash(ns_runtime::Value::small_integer(argument))]
    } else {
        Vec::new()
    };
    let answer = expect_ok(
        interpreter.send(subject, Symbol::intern(selector), &arguments),
        selector,
    );
    interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("{selector}: expected a small integer"),
    })
}

/// Asserts that `statistics` show machine code entered and leaving only
/// at a poll that found a safepoint requested, which any stress mode
/// holding the request makes every poll do.
fn assert_ran_in_machine_code(statistics: &JitStatistics, what: &str) {
    assert!(
        statistics.machine_entries > 0,
        "{what}: the writer is entered"
    );
    assert_eq!(
        statistics.exits, statistics.safepoint_exits,
        "{what}: every exit is a safepoint's"
    );
}

/// A store machine code makes of a young array into the vector, which is
/// old, remembers the vector, as the interpreter's barrier does: the
/// scavenge after it finds the arrays through the remembered vector, and
/// they read back as written. A store that missed the barrier leaves the
/// vector naming nursery memory the scavenge vacated.
#[cfg(debug_assertions)]
#[test]
fn an_inline_store_of_a_young_value_into_an_old_object_remembers_it() {
    let (mut interpreter, subject, writer) = tenured_writer();
    let before = interpreter.jit_statistics();
    let count = send_integer(&mut interpreter, &writer, "value:", BARRIER_STORES);
    let statistics = since(interpreter.jit_statistics(), before);
    assert_eq!(count, 9 + BARRIER_STORES);
    interpreter.scavenge_now();
    let last = BARRIER_STORES - 1;
    assert_eq!(
        send_integer(&mut interpreter, &subject, "read", 0),
        last + BARRIER_STORES + (last + 1) + count
    );
    if !a_stress_mode_is_on() {
        assert_ran_in_machine_code(&statistics, "value:");
        assert_eq!(
            statistics.leaf_calls, 1,
            "the one store that remembered the vector"
        );
    }
}

/// Once the vector is remembered, a store into it calls no leaf: the
/// barrier's early out on the header's remembered bit takes every one,
/// and the arrays are bumped inline.
#[cfg(debug_assertions)]
#[test]
fn a_store_into_a_remembered_holder_calls_no_leaf() {
    let (mut interpreter, _, writer) = tenured_writer();
    send_integer(&mut interpreter, &writer, "value:", 1);
    let before = interpreter.jit_statistics();
    send_integer(&mut interpreter, &writer, "value:", BARRIER_STORES);
    let statistics = since(interpreter.jit_statistics(), before);
    if !a_stress_mode_is_on() {
        assert_ran_in_machine_code(&statistics, "value:");
        assert_eq!(statistics.leaf_calls, 0, "no store calls a leaf");
    }
}

/// A holder of a slot and an Array, written by `store:` through the
/// setter stub and the `at:put:` stub, and read back by `read`.
#[cfg(debug_assertions)]
const OLD_HOLDERS: &str = indoc! {"
    class Subject = Object new (
      |
        holder = Holder new.
        elements = Array new: 10.
      |
    ) (
      class Holder = Object new (
        |
          public item ::= nil.
        |
      ) ()
      public store: count = (
        |
          index ::= 0.
        |
        [index < count] whileTrue: [
          holder item: {index}.
          elements at: 10 put: {index + 1}.
          index:: index + 1.
        ].
        ^index
      )
      public read = (
        ^(holder item at: 1) + ((elements at: 10) at: 1)
      )
    ) : ()
"};

/// A store of a young Array into an old holder by the setter stub, and
/// into an old Array by the `at:put:` stub, remembers the holder, as the
/// interpreter's barrier does: the scavenge after them finds the young
/// Arrays through the remembered holders, and they read back as stored.
/// A store that missed the barrier leaves a holder naming nursery memory
/// the scavenge vacated.
#[cfg(debug_assertions)]
#[test]
fn a_stub_storing_into_an_old_holder_remembers_it() {
    let (mut interpreter, class) = load_with(OLD_HOLDERS, Lowering::Optimized);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    for _ in 0..3 {
        send_integer(&mut interpreter, &subject, "store:", 3);
    }
    // Everything old from here on, and remembered by nothing.
    interpreter.collect_now();
    let before = interpreter.jit_statistics();
    assert_eq!(
        send_integer(&mut interpreter, &subject, "store:", BARRIER_STORES),
        BARRIER_STORES
    );
    let statistics = since(interpreter.jit_statistics(), before);
    interpreter.scavenge_now();
    assert_eq!(
        send_integer(&mut interpreter, &subject, "read", 0),
        2 * BARRIER_STORES - 1
    );
    if !a_stress_mode_is_on() {
        assert_ran_in_machine_code(&statistics, "store:");
        assert_eq!(
            statistics.leaf_calls, 2,
            "the two stores that remembered the holder and the Array"
        );
    }
}

/// A lazy slot read and computed again a thousand times from a compiled
/// loop: `doubled`'s getter answers its filled slot, and `cell`'s, reset
/// to `nil` before each read, computes an Array and stores it through the
/// inline barrier.
const OWN_SLOTS: &str = indoc! {"
    class Subject = Object new (
      | holder = Holder new. |
    ) (
      class Holder = Object new (
        | public seed ::= 3. |
      ) (
        lazy public doubled = seed * 2.
        lazy public cell ::= {seed}.
      )
      readAll: aHolder = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 1000] whileTrue: [
          aHolder cell: nil.
          sum:: sum + aHolder doubled + (aHolder cell at: 1).
          index:: index + 1.
        ].
        ^sum.
      )
      public run = (
        ^readAll: holder
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// The own-slot instructions run in machine code: in the third run the
    /// two thousand lazy getters' `ReturnSlotIfFilled` and `InitializeSlot`
    /// leave machine code for nothing and call no leaf, since the prologue
    /// verified each receiver against the getter's level, and the sum is
    /// the interpreter's.
    fn own_slot_access_runs_without_an_exit(lowering) {
        let (compiled, statistics) = run_thrice_on_one_instance(OWN_SLOTS, lowering, true);
        let (interpreted, _) = run_thrice_on_one_instance(OWN_SLOTS, lowering, false);
        assert_eq!(interpreted, [9000; 3]);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "readAll:");
            assert_eq!(statistics.send_exits, 0, "every send stayed in machine code");
            assert_eq!(statistics.leaf_calls, 0, "no own-slot instruction called a leaf");
            assert!(
                statistics.compiled_calls >= 3000,
                "`readAll:` calls the lazy getters through their cells: {}",
                statistics.compiled_calls
            );
        }
    }
}

/// A lazy slot of `Base`, computed again a thousand times on a `Base` and
/// on a `Derived`, whose getter loads its String literal through the
/// generic leaf between the two own-slot instructions.
const INHERITED_OWN_SLOTS: &str = indoc! {"
    class Subject = Object new (
      |
        base = Base new.
        derived = Derived new.
      |
    ) (
      class Base = Object new () (
        lazy public label ::= 'label'.
      )
      class Derived = Base new () ()
      readAll: aHolder = (
        |
          index ::= 0.
        |
        [index < 1000] whileTrue: [
          aHolder label: nil.
          aHolder label.
          index:: index + 1.
        ].
        ^index.
      )
      public run = (
        ^(readAll: base) + (readAll: derived)
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// An own-slot instruction's cold check records the receiver's guard
    /// in its frame, and the generic leaf's view of the frame keeps it: on
    /// a `Base` each getter calls only the generic leaf, for its literal;
    /// on a `Derived`, which the prologue does not verify, the first
    /// own-slot instruction's cold check as well, and the second, after
    /// the literal's leaf, hits.
    fn an_own_slot_guard_survives_a_leaf_call(lowering) {
        let (compiled, statistics) =
            run_thrice_on_one_instance(INHERITED_OWN_SLOTS, lowering, true);
        let (interpreted, _) = run_thrice_on_one_instance(INHERITED_OWN_SLOTS, lowering, false);
        assert_eq!(interpreted, [2000; 3]);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "readAll:");
            assert_eq!(statistics.send_exits, 0, "every send stayed in machine code");
            assert_eq!(
                statistics.leaf_calls, 3000,
                "a literal's leaf per getter, and a cold check per `Derived` getter"
            );
        }
    }
}

/// A thousand `Point`s and `Point3`s made in a compiled loop: each factory
/// send a compiled call of the factory's body, which bumps the instance
/// inline and calls the class's initializer, `Point3`'s calling `Point`'s.
/// The classes are read from slots, so that the factories alone are sent
/// (`IMPLICIT_POINTS` sends the accessors too).
const POINTS: &str = indoc! {"
    class Subject = Object new (
      |
        pointClass = Point.
        point3Class = Point3.
      |
    ) (
      class Point x: ax y: ay = Object new (
        |
          public x = ax.
          public y = ay.
        |
      ) ()
      class Point3 x: ax y: ay z: az = Point x: ax y: ay (
        |
          public z = az.
        |
      ) ()
      public run = (
        |
          sum ::= 0.
          index ::= 0.
          point
          deep
        |
        [index < 1000] whileTrue: [
          point:: pointClass x: index y: 2.
          deep:: point3Class x: 1 y: index z: 3.
          sum:: sum + point x + point y + deep y + deep z.
          index:: index + 1.
        ].
        ^sum.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// Instantiation runs in machine code: in the third run every factory
    /// send of the loop calls the factory's body, whose `AllocateInstance`
    /// and `InitializeInstance`, and the initializers' own invocations,
    /// leave machine code for nothing, and the sum is the interpreter's.
    fn instantiation_runs_without_an_exit(lowering) {
        let (compiled, statistics) = run_thrice_on_one_instance(POINTS, lowering, true);
        let (interpreted, _) = run_thrice_on_one_instance(POINTS, lowering, false);
        assert_eq!(interpreted, [1_004_000; 3]);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "run");
            assert_eq!(statistics.send_exits, 0, "every send stayed in machine code");
            assert_eq!(
                statistics.materialized_frames, 0,
                "no exit materialized a factory's or an initializer's frame"
            );
            assert!(
                statistics.compiled_calls >= 2000,
                "`run` calls both factories through their cells: {}",
                statistics.compiled_calls
            );
            // Five initializer invocations a turn, and the cold check of
            // `Point`'s first own slot on a `Point3`, whose own class is not
            // the level its frame was entered at.
            assert_eq!(
                statistics.leaf_calls, 6000,
                concat!(
                    "each instance is bumped inline, and only the initializer leaf and one ",
                    "own-slot check a turn are called"
                )
            );
        }
    }
}

/// A lazy slot of `Base`, which has a slot before it, computed on a
/// `Derived`, which declares slots of its own after `Base`'s, both made
/// in the compiled loop: the lazy slot is `Base`'s level's, at `Base`'s
/// slot base, and `Derived`'s own slots lie past it.
const SUPERCLASS_SLOT_BASE: &str = indoc! {"
    class Subject = Object new (
      |
        derivedClass = Derived.
      |
    ) (
      class Base = Object new (
        |
          public seed ::= 5.
        |
      ) (
        lazy public doubled = seed * 2.
      )
      class Derived = Base new (
        |
          public extra ::= 100.
          public other ::= 7.
        |
      ) ()
      public run = (
        |
          sum ::= 0.
          index ::= 0.
          derived
        |
        [index < 1000] whileTrue: [
          derived:: derivedClass new.
          sum:: sum + derived doubled + derived extra + derived other + derived seed.
          index:: index + 1.
        ].
        ^sum.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A superclass level's own slots are found at that level's slot base,
    /// not at the receiver's class's: `Base`'s initializer and its lazy
    /// getter, run on a `Derived` in machine code, write and read `seed`
    /// and `doubled` where `Base`'s layout has them, and `Derived`'s
    /// initializer its own slots past them.
    fn a_superclass_levels_slots_are_at_its_own_slot_base(lowering) {
        let (compiled, statistics) =
            run_thrice_on_one_instance(SUPERCLASS_SLOT_BASE, lowering, true);
        let (interpreted, _) = run_thrice_on_one_instance(SUPERCLASS_SLOT_BASE, lowering, false);
        assert_eq!(interpreted, [122_000; 3]);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "run");
            assert_eq!(statistics.send_exits, 0, "every send stayed in machine code");
        }
    }
}

/// `POINTS` with a send in `Point`'s initializer of a message `Point`
/// answers in its own `doesNotUnderstand:`, whose handler machine code
/// leaves to the interpreter, and, past the hundredth instance, a message
/// nothing understands: every instantiation leaves machine code inside
/// the initializers the compiled factory calls, and the last one ends the
/// run.
const POINTS_LEAVING: &str = indoc! {"
    class Subject = Object new (
      |
        pointClass = Point.
        point3Class = Point3.
      |
    ) (
      class Point x: ax y: ay = Object new (
        |
          public x = ax + self zero.
          public y = ay.
          public checked = ax > 100 ifTrue: [nil frobnicate].
        |
      ) (
        public doesNotUnderstand: message = (
          ^0
        )
      )
      class Point3 x: ax y: ay z: az = Point x: ax y: ay (
        |
          public z = az.
        |
      ) ()
      public run = (
        |
          sum ::= 0.
          index ::= 0.
          deep
        |
        [index < 200] whileTrue: [
          deep:: point3Class x: index y: index z: 3.
          sum:: sum + deep x + deep y + deep z.
          index:: index + 1.
        ].
        ^sum.
      )
    ) : ()
"};

/// The stack `POINTS_LEAVING`'s unhandled exception ended the run on, with
/// the JIT on, every body compiled at its first entry, or off.
fn points_leaving_stack(lowering: Lowering, jit: bool) -> (Vec<String>, JitStatistics) {
    let (mut interpreter, class) = load_with(POINTS_LEAVING, lowering);
    interpreter.set_jit_enabled(jit);
    interpreter.set_jit_stress(jit);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let before = interpreter.jit_statistics();
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
    /// An exit inside an initializer a compiled factory called, which a
    /// compiled loop called, materializes the factory's frame and both
    /// initializers' frames with their kinds, so that the run goes on in
    /// the interpreter -- and back in machine code, at each frame it
    /// returns to -- and the stack its unhandled exception ends on reads
    /// as the interpreter's.
    fn an_exit_in_a_compiled_initializer_materializes_its_frames(lowering) {
        // The suspension stress mode suspends the frames below a
        // safepoint into their frame objects, which the trace
        // (`Interpreter::stack_trace`, the activations on the stack) does
        // not walk, with the JIT off as on: it would not name the
        // initializers' and the factory's frames this pins.
        if std::env::var_os("NS_NEXT_STRESS_SUSPEND").is_some_and(|value| !value.is_empty() && value != "0") {
            return;
        }
        let (compiled, statistics) = points_leaving_stack(lowering, true);
        let (interpreted, _) = points_leaving_stack(lowering, false);
        assert_eq!(compiled, interpreted);
        assert_eq!(
            compiled
                .iter()
                .filter(|frame| frame.starts_with("initializer"))
                .count(),
            2,
            "`Point3`'s and `Point`'s initializers: {compiled:?}"
        );
        assert!(
            compiled.iter().any(|frame| frame.starts_with("factory")),
            "the factory's frame: {compiled:?}"
        );
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                statistics.materialized_frames >= 3 * 100,
                "the factory's and both initializers' frames were compiled frames: {}",
                statistics.materialized_frames
            );
            assert!(
                statistics.resumed_entries >= 100,
                "the frames returned to went on in machine code: {}",
                statistics.resumed_entries
            );
        }
    }
}

/// How many slots `Wide` declares: more than a header's count holds, so
/// that its instances need an overflow word and its row records no
/// instance header.
const WIDE_SLOTS: usize = 260;

/// A module instantiating, from a compiled loop, every kind of class whose
/// instance `AllocateInstance`'s inline bump does not make -- `Array` and
/// `String`, builtins that make their own, an `Ephemeron`, a class whose
/// count needs an overflow word -- beside ones it does: a user subclass of
/// `Array` and of `Ephemeron`, which hold no payload, and a class whose
/// levels run no code, whose instance the factory answers with no
/// initializer called; and, sent alone, the instantiations that are
/// refused: `Symbol new`, a value class whose enclosing object is no
/// value, and a class whose application refuses instantiation. Every
/// class is read from a slot.
fn builtins_and_refusals_source() -> String {
    let slots: String = (0..WIDE_SLOTS)
        .map(|index| format!("slot{index} ::= {index}. "))
        .collect();
    let last = WIDE_SLOTS - 1;
    formatdoc! {"
        class Subject usingPlatform: platform = Object new (
          |
            arrayClass = Array.
            stringClass = String.
            myArrayClass = MyArray.
            myStringClass = MyString.
            symbolClass = Symbol.
            ephemeronClass = platform kernel Ephemeron.
            myEphemeronClass = MyEphemeron.
            wideClass = Wide.
            oddClass = Odd.
            emptyClass = Empty.
          |
        ) (
          class MyArray = Array new () ()
          class MyString = String new () ()
          class MyEphemeron = ephemeronClass new () ()
          class Wide = Object new (| {slots}|) (
            public last = (^slot{last})
          )
          class Odd = Value new () (
            public == other = (^true)
          )
          class Empty = Object new () ()
          public run = (
            |
              sum ::= 0.
              index ::= 0.
              empty
              previous
            |
            [index < 100] whileTrue: [
              sum:: sum + arrayClass new size + stringClass new size.
              myArrayClass new.
              ephemeronClass new.
              myEphemeronClass new.
              sum:: sum + wideClass new last.
              empty:: emptyClass new.
              empty == previous ifTrue: [sum:: sum + 1000000].
              previous:: empty.
              index:: index + 1.
            ].
            ^sum.
          )
          public symbol = (^symbolClass new)
          public myString = (^myStringClass new)
          public odd = (^oddClass new)
        ) : ()
    "}
}

/// `builtins_and_refusals_source`'s `run`, three times, with the JIT on --
/// every body compiled at its first entry, the compiled calls counted --
/// or off: the answers and what the JIT did in the third run; then the
/// error text of each refused instantiation, sent three times.
fn builtins_and_refusals(lowering: Lowering, jit: bool) -> ([i64; 3], JitStatistics, Vec<String>) {
    let source = builtins_and_refusals_source();
    let (mut interpreter, class) = load_with(&source, lowering);
    interpreter.set_jit_enabled(jit);
    interpreter.set_jit_stress(jit);
    interpreter.set_jit_counting(jit);
    let platform = interpreter.platform();
    let instance = expect_ok(interpreter.instantiate(&class, &[platform]), "instantiate");
    let mut answers = [0; 3];
    let mut statistics = interpreter.jit_statistics();
    for answer in &mut answers {
        let before = interpreter.jit_statistics();
        let result = expect_ok(
            interpreter.send(&instance, Symbol::intern("run"), &[]),
            "run",
        );
        statistics = since(interpreter.jit_statistics(), before);
        *answer = interpreter.enter(|mutator| match mutator.fetch(&result).kind() {
            ValueKind::SmallInteger(integer) => integer,
            _ => panic!("run: expected a small integer"),
        });
    }
    let mut errors = Vec::new();
    for selector in ["symbol", "myString", "odd"] {
        for _ in 0..3 {
            match interpreter.send(&instance, Symbol::intern(selector), &[]) {
                Err(HostSignal::Error(error)) => errors.push(error.message.to_string()),
                _ => panic!("{selector}: expected the instantiation refused"),
            }
        }
    }
    (answers, statistics, errors)
}

ns_runtime::for_each_lowering! {
    /// Each instantiation `AllocateInstance`'s inline bump does not make
    /// runs through the generic leaf, from the factory's machine code --
    /// a builtin's own instance, an ephemeron, an overflowing count -- or
    /// exits there so that the interpreter raises the refusal; a class
    /// whose levels run no code is answered by the stub answering its
    /// receiver, a fresh instance each time; all as with the JIT off.
    fn instantiations_the_inline_bump_does_not_make_answer_as_interpreted(lowering) {
        let (compiled, statistics, compiled_errors) = builtins_and_refusals(lowering, true);
        let (interpreted, _, interpreted_errors) = builtins_and_refusals(lowering, false);
        let last = i64::try_from(WIDE_SLOTS - 1).expect("small");
        assert_eq!(interpreted, [100 * last; 3]);
        assert_eq!(compiled, interpreted);
        assert_eq!(compiled_errors, interpreted_errors);
        assert!(
            interpreted_errors[0].contains("Symbol")
                && interpreted_errors[3].contains("enclosing objects must be values")
                && interpreted_errors[6].contains("Odd"),
            "{interpreted_errors:?}"
        );
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "run");
            assert_eq!(statistics.send_exits, 0, "every send stayed in machine code");
            assert!(
                statistics.compiled_calls >= 600,
                "`run` calls the factories through their cells: {}",
                statistics.compiled_calls
            );
        }
    }
}

/// `POINTS` with the classes named by their accessors, the implicit
/// receiver's sends `Point` and `Point3`: a nested class's accessor send
/// whose class is cached in its hidden slot answers it in its stub.
const IMPLICIT_POINTS: &str = indoc! {"
    class Subject = Object new () (
      class Point x: ax y: ay = Object new (
        |
          public x = ax.
          public y = ay.
        |
      ) ()
      class Point3 x: ax y: ay z: az = Point x: ax y: ay (
        |
          public z = az.
        |
      ) ()
      public run = (
        |
          sum ::= 0.
          index ::= 0.
          point
          deep
        |
        [index < 1000] whileTrue: [
          point:: Point x: index y: 2.
          deep:: Point3 x: 1 y: index z: 3.
          sum:: sum + point x + point y + deep y + deep z.
          index:: index + 1.
        ].
        ^sum.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// The nested class accessor's stub: in the third run every `Point`
    /// and `Point3` send answers the cached class in machine code, and
    /// nothing leaves it but a safepoint.
    fn a_nested_class_accessor_answers_its_cached_class_in_its_stub(lowering) {
        let (compiled, statistics) = run_thrice_on_one_instance(IMPLICIT_POINTS, lowering, true);
        let (interpreted, _) = run_thrice_on_one_instance(IMPLICIT_POINTS, lowering, false);
        assert_eq!(interpreted, [1_004_000; 3]);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "run");
            assert_eq!(statistics.send_exits, 0, "every send stayed in machine code");
            assert!(
                statistics.compiled_calls >= 4000,
                "`run` calls both accessors and both factories through their cells: {}",
                statistics.compiled_calls
            );
        }
    }
}

/// A thousand closures made in a compiled loop, each by a compiled call of
/// `make:`, whose `MakeClosure` reifies its frame and allocates the
/// closure in machine code, kept in an array the host then sums through:
/// `run` answers how many it made, `total` what they answer.
const CLOSURES: &str = indoc! {"
    class Subject = Object new (
      |
        blocks = Array new: 1000.
      |
    ) (
      make: value = (
        |
          doubled = value * 2.
        |
        ^[doubled + value]
      )
      public run = (
        |
          index ::= 0.
        |
        [index < 1000] whileTrue: [
          blocks at: index + 1 put: (self make: index).
          index:: index + 1.
        ].
        ^index.
      )
      public total = (
        |
          sum ::= 0.
        |
        blocks do: [:block | sum:: sum + block value].
        ^sum.
      )
    ) : ()
"};

/// Sends `selector` to `subject`, answering the `SmallInteger` it answers.
fn send_unary(interpreter: &mut Interpreter, subject: &Root, selector: &str) -> i64 {
    let answer = expect_ok(
        interpreter.send(subject, Symbol::intern(selector), &[]),
        selector,
    );
    interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("{selector}: expected a small integer"),
    })
}

ns_runtime::for_each_lowering! {
    /// `MakeClosure` runs in machine code: in the third run of the loop
    /// every closure is made with no exit, its frame reified and the
    /// closure allocated inline, and the closures answer what the
    /// interpreter's do.
    fn make_closure_runs_without_an_exit(lowering) {
        let run = |jit: bool| {
            let (mut interpreter, class) = load_with(CLOSURES, lowering);
            interpreter.set_jit_enabled(jit);
            interpreter.set_jit_counting(jit);
            let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            let mut statistics = interpreter.jit_statistics();
            for _ in 0..3 {
                let before = interpreter.jit_statistics();
                assert_eq!(send_unary(&mut interpreter, &subject, "run"), 1000);
                statistics = since(interpreter.jit_statistics(), before);
            }
            (send_unary(&mut interpreter, &subject, "total"), statistics)
        };
        let (compiled, statistics) = run(true);
        let (interpreted, _) = run(false);
        assert_eq!(interpreted, 3 * (999 * 1000 / 2));
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "run");
            assert_eq!(statistics.send_exits, 0, "every send stayed in machine code");
            assert!(
                statistics.compiled_calls >= 1000,
                "`run` calls `make:` through its cell: {}",
                statistics.compiled_calls
            );
            assert_eq!(
                statistics.leaf_calls, 0,
                "every frame object and closure is bumped inline, its site filled before"
            );
        }
    }
}

/// A hundred numbers summed by a block `do:` sends `value:` to, each
/// element in turn: `run` makes the block, which keeps its sum in an
/// `Array` it was handed as a copied value, and calls `do:`, whose loop
/// calls the block.
const SUMMED: &str = indoc! {"
    class Subject = Object new (
      |
        numbers = Array new: 100.
      |
    ) (
      public fill = (
        |
          index ::= 0.
        |
        [index < 100] whileTrue: [
          index:: index + 1.
          numbers at: index put: index.
        ].
        ^index.
      )
      public run = (
        |
          sum = Array new: 1.
        |
        sum at: 1 put: 0.
        numbers do: [:each | sum at: 1 put: (sum at: 1) + each].
        ^sum at: 1.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A block a compiled `do:` sends `value:` is called as a method is,
    /// through the closure stub its cell names, and runs as a compiled
    /// call: by the third run the loop over the elements leaves machine
    /// code at no send.
    fn a_closure_value_runs_without_an_exit(lowering) {
        let run = |jit: bool| {
            let (mut interpreter, class) = load_with(SUMMED, lowering);
            interpreter.set_jit_enabled(jit);
            interpreter.set_jit_counting(jit);
            let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            assert_eq!(send_unary(&mut interpreter, &subject, "fill"), 100);
            let mut statistics = interpreter.jit_statistics();
            for _ in 0..3 {
                let before = interpreter.jit_statistics();
                assert_eq!(send_unary(&mut interpreter, &subject, "run"), 5050);
                statistics = since(interpreter.jit_statistics(), before);
            }
            statistics
        };
        let statistics = run(true);
        let _ = run(false);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "run");
            assert_eq!(statistics.send_exits, 0, "every `value:` stayed in machine code");
            assert!(
                statistics.compiled_calls >= 100,
                "`do:` calls the block through its cell: {}",
                statistics.compiled_calls
            );
        }
    }
}

/// `find:` returns out of the block `do:` sends `value:` with `^`, once
/// per element `run` looks for: the home, `find:`, lies in the stretch
/// `run` was entered at, with only `do:` between it and the block.
const FOUND: &str = indoc! {"
    class Subject = Object new (
      |
        numbers = Array new: 100.
      |
    ) (
      public fill = (
        |
          index ::= 0.
        |
        [index < 100] whileTrue: [
          index:: index + 1.
          numbers at: index put: index.
        ].
        ^index.
      )
      find: target = (
        numbers do: [:each | each = target ifTrue: [^each]].
        ^0.
      )
      public run = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 100] whileTrue: [
          index:: index + 1.
          sum:: sum + (self find: index).
        ].
        ^sum.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A `^` out of the block a compiled `do:` calls is cut by machine
    /// code: by the third run every one of the hundred returns pops `do:`
    /// and the block and returns from `find:` into `run` with no exit,
    /// each a cut, and the depth count ends where it began.
    fn a_non_local_return_out_of_do_cuts_without_an_exit(lowering) {
        let run = |jit: bool| {
            let (mut interpreter, class) = load_with(FOUND, lowering);
            interpreter.set_jit_enabled(jit);
            let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            assert_eq!(send_unary(&mut interpreter, &subject, "fill"), 100);
            let depth = interpreter.enter(|mutator| mutator.heap().counted_depth());
            let mut statistics = interpreter.jit_statistics();
            for _ in 0..3 {
                let before = interpreter.jit_statistics();
                assert_eq!(send_unary(&mut interpreter, &subject, "run"), 5050);
                statistics = since(interpreter.jit_statistics(), before);
            }
            assert_eq!(
                interpreter.enter(|mutator| mutator.heap().counted_depth()),
                depth,
                "every frame the returns popped left the count"
            );
            statistics
        };
        let statistics = run(true);
        let _ = run(false);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "run");
            assert_eq!(statistics.exits, 0, "no return left machine code");
            assert_eq!(statistics.cuts, 100, "every return was cut");
        }
    }
}

/// A home an interpreted frame's send called through the call stub: `run`,
/// sent once, stays interpreted -- the backward jumps' trigger set past
/// its loop -- and calls `find:`, compiled at its second
/// entry, through the call stub, so that each later `^` out of `do:` is
/// cut into the call stub, which hands the value to `run`'s send as
/// `find:`'s return would.
#[test]
fn a_non_local_return_into_an_interpreted_caller_cuts_into_the_call_stub() {
    let (mut interpreter, class) = load_with(FOUND, Lowering::Optimized);
    interpreter.set_jit_compile_at_back_jump(u32::MAX);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    assert_eq!(send_unary(&mut interpreter, &subject, "fill"), 100);
    let depth = interpreter.enter(|mutator| mutator.heap().counted_depth());
    let before = interpreter.jit_statistics();
    assert_eq!(send_unary(&mut interpreter, &subject, "run"), 5050);
    let statistics = since(interpreter.jit_statistics(), before);
    assert_eq!(
        interpreter.enter(|mutator| mutator.heap().counted_depth()),
        depth,
        "every frame the returns popped left the count"
    );
    if !a_stress_mode_is_on() {
        assert!(
            statistics.called_entries >= 90,
            "`run` calls `find:` through the call stub: {}",
            statistics.called_entries
        );
        assert!(
            statistics.cuts >= 90,
            "the returns into `run` are cut: {}",
            statistics.cuts
        );
    }
}

/// `findSeven` returns out of the block `do:` sends `value:`: sent from
/// the host, it is the frame the run loop pushes and, once compiled,
/// enters machine code at, so that its `^` is cut to the stretch's entry
/// frame and returns through the entry stub.
const SEVEN: &str = indoc! {"
    class Subject = Object new (
      |
        numbers = {1. 2. 3. 4. 5. 6. 7. 8. 9. 10}.
      |
    ) (
      public findSeven = (
        numbers do: [:each | each = 7 ifTrue: [^each]].
        ^0.
      )
    ) : ()
"};

/// A home the run loop entered machine code at is cut to as well: the cut
/// divorces it and takes its count off as its return would, the entry
/// stub adds the count back for the run loop's pop, and the depth count
/// ends where it began.
#[test]
fn a_non_local_return_to_the_entry_frame_is_cut() {
    let (mut interpreter, class) = load_with(SEVEN, Lowering::Optimized);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let depth = interpreter.enter(|mutator| mutator.heap().counted_depth());
    for _ in 0..3 {
        assert_eq!(send_unary(&mut interpreter, &subject, "findSeven"), 7);
    }
    let before = interpreter.jit_statistics();
    for _ in 0..5 {
        assert_eq!(send_unary(&mut interpreter, &subject, "findSeven"), 7);
    }
    let statistics = since(interpreter.jit_statistics(), before);
    assert_eq!(
        interpreter.enter(|mutator| mutator.heap().counted_depth()),
        depth,
        "the cut and the entry stub left the count as they found it"
    );
    if !a_stress_mode_is_on() {
        assert_eq!(
            statistics.machine_entries, 5,
            "each send enters at the push"
        );
        assert_eq!(statistics.exits, 0, "no return left machine code");
        assert_eq!(statistics.cuts, 5, "every return was cut");
    }
}

/// `scan`, sent once, stays interpreted, and its `value:` calls the
/// block, compiled at its second entry, through the closure call stub.
const INTERPRETED_VALUE: &str = indoc! {"
    class Subject = Object new () (
      public scan = (
        |
          check = [:each | each = 30 ifTrue: [^each]. 0].
          index ::= 0.
        |
        [index < 50] whileTrue: [
          index:: index + 1.
          check value: index.
        ].
        ^0.
      )
    ) : ()
"};

/// A `^` from a block an interpreted `value:` called is not cut: its
/// return address lies in the closure call stub, so its home -- the
/// interpreted sender -- runs no machine code to resume, and the block
/// exits so that the interpreter's unwinder returns from `scan`.
#[test]
fn a_non_local_return_from_a_block_an_interpreted_value_called_exits() {
    let (mut interpreter, class) = load_with(INTERPRETED_VALUE, Lowering::Optimized);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let before = interpreter.jit_statistics();
    assert_eq!(send_unary(&mut interpreter, &subject, "scan"), 30);
    let statistics = since(interpreter.jit_statistics(), before);
    if !a_stress_mode_is_on() {
        assert!(
            statistics.called_entries > 0,
            "the interpreted `value:` calls the compiled block"
        );
        assert_eq!(statistics.cuts, 0, "the return is not cut");
        assert!(statistics.exits >= 1, "the return exits");
    }
}

/// A loop of block literals, whose blocks the reference lowering
/// activates, each sent `value` by a `SendInlinable` of `run` itself, and
/// so exempt from the depth count; each reads `count`, a local of `run`,
/// in machine code, then sends `<` or `+` from an uncached site, which
/// leaves machine code.
const LOOP_OF_EXEMPT_BLOCKS: &str = indoc! {"
    class Subject = Object new () (
      public run = (
        |
          count ::= 0.
        |
        [count < 5] whileTrue: [count:: count + 1].
        ^count.
      )
    ) : ()
"};

/// A block the interpreter's `SendInlinable` calls in machine code,
/// exempt from the depth count, and whose code leaves machine code at
/// its first send, is materialized exempt: the interpreter's pop of its frame takes
/// nothing off the count, which ends where it began.
#[test]
fn an_exit_inside_an_exempt_closure_call_materializes_it_exempt() {
    let (mut interpreter, class) = load_with(LOOP_OF_EXEMPT_BLOCKS, Lowering::Reference);
    interpreter.set_jit_stress(true);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let depth = interpreter.enter(|mutator| mutator.heap().counted_depth());
    let before = interpreter.jit_statistics();
    assert_eq!(send_unary(&mut interpreter, &subject, "run"), 5);
    let statistics = since(interpreter.jit_statistics(), before);
    assert_eq!(
        interpreter.enter(|mutator| mutator.heap().counted_depth()),
        depth,
        "the exempt frames left the count as they found it"
    );
    if the_zone_stress_mode_is_on() {
        return;
    }
    assert!(
        statistics.called_entries > 0,
        "the loop's blocks are called in machine code"
    );
    assert!(
        statistics.materialized_frames > 0,
        "an exit materialized a called block's frame"
    );
}

/// Every quick method that activates a block, sent blocks held in locals
/// -- which no lowering inlines -- of no parameter and of one, reading a
/// copied value: `ifTrue:`, `ifFalse:`, `ifTrue:ifFalse:`, `and:`,
/// `or:`, which hand a block nothing, and `ifNotNil:`, `ifNil:ifNotNil:`
/// and `ifNotNil:ifNil:`, which hand a block of one parameter the
/// receiver.
const SHAPES: &str = indoc! {"
    class Subject = Object new () (
      shapes: index = (
        |
          base = index * 2.
          none = [base].
          other = [base + 3].
          one = [:value | value + base].
          truth = [index > 50].
          even = index \\\\ 2 = 0.
        |
        ^(true ifTrue: none) + (false ifFalse: other) + (even ifTrue: none ifFalse: other)
          + ((even and: truth) ifTrue: [1] ifFalse: [0])
          + ((even or: truth) ifTrue: [1] ifFalse: [0]) + (index ifNotNil: one)
          + (index ifNotNil: none) + (index ifNil: none ifNotNil: one)
          + (index ifNotNil: none ifNil: one) + (index ifNil: one ifNotNil: none)
      )
      public run = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 100] whileTrue: [
          sum:: sum + (self shapes: index).
          index:: index + 1.
        ].
        ^sum.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A block a quick method activates is called by a closure stub, its
    /// frame shifted past all the send's slots: by the
    /// third run no send of `SHAPES` leaves machine code, and every one
    /// answers what the interpreter's does.
    fn every_block_a_quick_method_activates_runs_without_an_exit(lowering) {
        let run = |jit: bool| {
            let (mut interpreter, class) = load_with(SHAPES, lowering);
            interpreter.set_jit_enabled(jit);
            let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            let mut answer = 0;
            let mut statistics = interpreter.jit_statistics();
            for _ in 0..3 {
                let before = interpreter.jit_statistics();
                answer = send_unary(&mut interpreter, &subject, "run");
                statistics = since(interpreter.jit_statistics(), before);
            }
            (answer, statistics)
        };
        let (compiled, statistics) = run(true);
        let (interpreted, _) = run(false);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "run");
            assert_eq!(statistics.send_exits, 0, "every block is called in machine code");
        }
    }
}

/// A block only `ifNotNil:` activates, which the interpreter pushes above
/// its sender's window and never calls (`jit::called_closure`).
#[cfg(debug_assertions)]
const CULLED: &str = indoc! {"
    class Subject = Object new () (
      public run = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 100] whileTrue: [
          sum:: sum + (index ifNotNil: [:value | value + 1]).
          index:: index + 1.
        ].
        ^sum.
      )
    ) : ()
"};

/// A full collection frees the id of `CULLED`'s block, which nothing
/// names between runs, and the next run mints it again: the first push of
/// the block after it, the interpreter's once its closure stub found no
/// row, records the block's machine entry under the new id, so that the
/// run after it calls the block in machine code at every send.
#[cfg(debug_assertions)]
#[test]
fn a_culled_blocks_row_is_recorded_again_after_a_full_collection() {
    let (mut interpreter, class) = load_with(CULLED, Lowering::Optimized);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    for _ in 0..3 {
        assert_eq!(send_unary(&mut interpreter, &subject, "run"), 5050);
    }
    interpreter.collect_now();
    assert_eq!(send_unary(&mut interpreter, &subject, "run"), 5050);
    let before = interpreter.jit_statistics();
    assert_eq!(send_unary(&mut interpreter, &subject, "run"), 5050);
    let statistics = since(interpreter.jit_statistics(), before);
    if !a_stress_mode_is_on() {
        assert_eq!(
            statistics.send_exits, 0,
            "every `ifNotNil:` calls the block in machine code"
        );
    }
}

/// `assign` sends `ifNotNil:`, whose method in `Object` is the quick
/// method `^<parameter> cull: self`, as a setter: `ifNotNil:: [...]`'s
/// value is the setter's argument slot, the block, whatever `ifNotNil:`
/// answers (`compiler::lower`'s `lower_setter_assign`). The block takes
/// one argument, which `cull:` makes the receiver: never in the slot the
/// assignment's value is read from. `run` answers 1 for each block
/// `assign` answers, which answers 1 for its receiver.
const CULLED_BY_A_SETTER: &str = indoc! {"
    class Subject = Object new () (
      assign = (
        ^ifNotNil:: [:value | value == self ifTrue: [1] ifFalse: [-2]]
      )
      public run = (
        |
          index ::= 0.
          sum ::= 0.
          last
        |
        [index < 100] whileTrue: [
          last:: self assign.
          sum:: sum + (last == self ifTrue: [-1000] ifFalse: [last value: self]).
          index:: index + 1.
        ].
        ^sum
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A setter send `ifNotNil:: [...]` that the closure stub of `cull:`
    /// serves answers the block as the assignment's value, as the
    /// interpreter's does, whose push of a culled block lands above the
    /// sender's window: the stub places the block's frame past the send's
    /// slots, so that the receiver it hands the block as its argument
    /// lands in none of them. The reference lowering's sends are
    /// uncached, so no stub serves them there; its outcome is held to the
    /// interpreter's all the same.
    fn a_setter_send_culled_in_machine_code_keeps_its_value(lowering) {
        let (mut interpreted, interpreted_class) = load_with(CULLED_BY_A_SETTER, lowering);
        interpreted.set_jit_enabled(false);
        let interpreted_subject =
            expect_ok(interpreted.instantiate(&interpreted_class, &[]), "instantiate");
        let expected = send_unary(&mut interpreted, &interpreted_subject, "run");
        assert_eq!(expected, 100, "the interpreter answers the block's value");
        let (mut interpreter, class) = load_with(CULLED_BY_A_SETTER, lowering);
        interpreter.set_jit_counting(true);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), expected);
        }
        let before = interpreter.jit_statistics();
        assert_eq!(send_unary(&mut interpreter, &subject, "run"), expected);
        let statistics = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "the setter sends");
            assert_eq!(
                statistics.send_exits, 0,
                "every `ifNotNil:` is served by the closure stub"
            );
            assert!(
                statistics.compiled_calls >= 100,
                "every culled block is a compiled call"
            );
        }
    }
}

/// `make:` makes a closure, and `churn` registers bodies of its own --
/// each of its methods and blocks -- after a full collection freed the
/// ids of `make:`'s body and block, which nothing names once the closures
/// are gone.
#[cfg(debug_assertions)]
const CODE_IDS: &str = indoc! {"
    class Subject = Object new () (
      make: value = (
        ^[value + 1]
      )
      public run = (
        |
          index ::= 0.
          last
        |
        [index < 10] whileTrue: [
          last:: self make: index.
          index:: index + 1.
        ].
        ^last value.
      )
      first = (
        ^[:number | number * 3]
      )
      second = (
        ^[:left :right | left * right]
      )
      third = (
        ^[:number | number - 5]
      )
      fourth = (
        ^[nil]
      )
      public churn = (
        ^(self first value: 1) + (self second value: 2 value: 3) + (self third value: 4)
          + (self fourth value isNil ifTrue: [1] ifFalse: [2])
      )
    ) : ()
"};

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    /// A compiled `MakeClosure` names its body and its block by the ids
    /// the heap has for them now, not the ones it had when its site was
    /// first filled: a full collection frees the ids nothing names, and
    /// the bodies `churn` runs take them, but the closures `run` makes
    /// after it still run `make:`'s block.
    fn a_full_collection_keeps_a_compiled_closures_code_id(lowering) {
        let (mut interpreter, class) = load_with(CODE_IDS, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        assert_eq!(send_unary(&mut interpreter, &subject, "run"), 10);
        interpreter.collect_now();
        assert_eq!(send_unary(&mut interpreter, &subject, "churn"), 3 + 6 - 1 + 1);
        assert_eq!(send_unary(&mut interpreter, &subject, "run"), 10);
    }
}

/// `make:`, which makes a closure and sends after it, so that the
/// interpreter stores a later program counter than the `MakeClosure`'s,
/// and `nest:`, whose block makes a closure in turn, whose frame's home is
/// `nest:`'s: called from a loop, so that both bodies and the outer block
/// run in machine code by the last round, whose closures `run` answers.
const DIVORCED: &str = indoc! {"
    class Subject = Object new () (
      make: value = (
        |
          block = [value + 1].
        |
        self touch: value.
        ^block
      )
      nest: value = (
        ^[[value + 2]]
      )
      touch: value = (
        ^value
      )
      public run = (
        |
          index ::= 0.
          made
          nested
        |
        [index < 10] whileTrue: [
          made:: self make: index.
          nested:: (self nest: index) value.
          index:: index + 1.
        ].
        ^{made. nested}.
      )
    ) : ()
"};

/// The frame objects `DIVORCED`'s closures keep, each described by what
/// its divorce left -- its program counter, whether it is continuable,
/// running or has a sender, its kind and receiver -- and its home's, with
/// the JIT on or off, and what the JIT did meanwhile.
///
/// The program counter is left out under a stress mode that holds the
/// safepoint request: the interpreter stores the one it goes on at when
/// it takes a safepoint at a return, which machine code, polling at its
/// backward jumps and its calls' prologues alone, never takes there, so
/// that the counter a returned frame records depends on when a safepoint
/// fell, with the JIT on as off.
fn divorced_frames(lowering: Lowering, jit: bool) -> (Vec<String>, JitStatistics) {
    let (mut interpreter, class) = load_with(DIVORCED, lowering);
    interpreter.set_jit_enabled(jit);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let before = interpreter.jit_statistics();
    let closures = expect_ok(
        interpreter.send(&subject, Symbol::intern("run"), &[]),
        "run",
    );
    let statistics = since(interpreter.jit_statistics(), before);
    let described = interpreter.enter(|mutator| {
        let heap = mutator.heap();
        let array = mutator.fetch(&closures).as_object().expect("an Array");
        let describe = |frame| {
            let record = heap.frame_record(frame);
            let kind = match &record.kind {
                ns_runtime::FrameKind::Method(method) => method.selector.to_string(),
                ns_runtime::FrameKind::Closure(_) => "block".to_owned(),
                _ => "other".to_owned(),
            };
            let program_counter = if a_stress_mode_polls_out() {
                "-".to_owned()
            } else {
                record.program_counter.to_string()
            };
            format!(
                "{kind} at {program_counter} uncontinuable {} running {} sender {} receiver {}",
                record.uncontinuable,
                heap.frame_activation(frame).is_some(),
                record.sender.is_some(),
                heap.describe(record.receiver),
            )
        };
        (0..2)
            .flat_map(|index| {
                let closure = heap
                    .array_element(array, index)
                    .and_then(|element| element.as_object())
                    .expect("a closure");
                let frame = heap
                    .closure_captured_frame(closure)
                    .expect("a captured frame");
                let home = heap.frame_home(frame).map_or("no home".to_owned(), |home| {
                    format!("home {}", describe(home))
                });
                [describe(frame), home]
            })
            .collect()
    });
    (described, statistics)
}

ns_runtime::for_each_lowering! {
    /// A compiled callee that makes a closure -- its frame reified by its
    /// `MakeClosure` in machine code -- and returns by `ret` leaves its
    /// frame object divorced as the interpreter's pop leaves it: at the
    /// program counter the interpreter stored last, uncontinuable, married
    /// to nothing and without a sender; and a block's frame, reified in
    /// machine code too, has the home the interpreter's has.
    fn a_compiled_closures_frame_is_divorced_as_the_interpreter_divorces_it(lowering) {
        let (compiled, statistics) = divorced_frames(lowering, true);
        let (interpreted, _) = divorced_frames(lowering, false);
        assert_eq!(compiled, interpreted);
        assert_eq!(compiled.len(), 4);
        assert!(
            compiled[0].starts_with("make: at ")
                && compiled[0].contains("uncontinuable true running false sender false"),
            "{compiled:?}"
        );
        assert_eq!(compiled[1], "no home");
        assert!(compiled[2].starts_with("block at "), "{compiled:?}");
        assert!(compiled[3].starts_with("home nest: at "), "{compiled:?}");
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                statistics.called_entries > 0,
                "an interpreted send called a compiled body"
            );
        }
    }
}

/// Locals a block reaches by reference in the frame of the body that
/// made it -- under the optimized lowering a fallback block of an inlined
/// `ifTrue:`, which `Yes` and `Later` are sent since they are no Boolean,
/// and under the reference lowering every captured local: a block one
/// frame out and one two frames out, their creator running -- a method,
/// and a block a closure stub laid past its send's slots; and a block
/// `Later` keeps and runs once its creator has returned, its frame
/// divorced; each reaching a temporary and an argument.
const OUTER_LOCALS: &str = indoc! {"
    class Subject = Object new (
      |
        yes = Yes new.
        later = Later new.
      |
    ) (
      class Yes = Object new () (
        public ifTrue: block = (
          ^block value.
        )
      )
      class Later = Object new (
        |
          kept ::= nil.
        |
      ) (
        public ifTrue: block = (
          kept:: block.
          ^nil.
        )
        public run = (
          ^kept value.
        )
      )
      countUpTo: limit = (
        |
          count ::= 0.
          index ::= 0.
        |
        [index < limit] whileTrue: [
          yes ifTrue: [count:: count + index].
          yes ifTrue: [yes ifTrue: [count:: count + 1]].
          index:: index + 1.
        ].
        ^count.
      )
      keepFrom: start = (
        |
          count ::= start.
        |
        later ifTrue: [
          count:: count + start + 1.
          count.
        ].
        ^start.
      )
      sumShifted: numbers = (
        |
          total ::= 0.
        |
        numbers do: [:each |
          |
            local ::= each.
          |
          yes ifTrue: [local:: local + each + 1].
          total:: total + local.
        ].
        ^total.
      )
      public run = (
        |
          total ::= 0.
          index ::= 0.
        |
        [index < 100] whileTrue: [
          total:: total + (self countUpTo: 10) + (self sumShifted: {index. 2. 3}).
          self keepFrom: index.
          total:: total + later run + later run.
          index:: index + 1.
        ].
        ^total.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// `LoadOuterLocal` and `StoreOuterLocal` run in machine code, on a
    /// creator's frame still on the activation stack and on one divorced
    /// into its frame object alike: by the third run no exit leaves
    /// machine code under the optimized lowering, whose fallback blocks
    /// are the only ones reading by reference, and both lowerings answer
    /// what the interpreter does.
    fn outer_locals_run_without_an_exit(lowering) {
        let run = |jit: bool| {
            let (mut interpreter, class) = load_with(OUTER_LOCALS, lowering);
            interpreter.set_jit_enabled(jit);
            let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            let mut answer = 0;
            let mut statistics = interpreter.jit_statistics();
            for _ in 0..3 {
                let before = interpreter.jit_statistics();
                answer = send_unary(&mut interpreter, &subject, "run");
                statistics = since(interpreter.jit_statistics(), before);
            }
            (answer, statistics)
        };
        let (compiled, statistics) = run(true);
        let (interpreted, _) = run(false);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "run");
            assert_eq!(statistics.send_exits, 0, "every block is called in machine code");
        }
    }
}

/// A recursion whose base case returns from an inlined `ifTrue:`: under
/// the optimized lowering the conditional's fallback makes the arm's
/// closure, for a receiver that is no Boolean, and every receiver here is
/// one.
const RECURSION: &str = indoc! {"
    class Subject = Object new () (
      fib: count = (
        count < 2 ifTrue: [^1].
        ^(self fib: count - 1) + (self fib: count - 2)
      )
      public run = (
        ^self fib: 15
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A body with an inlined conditional compiles lean -- no reifying
    /// prologue, no program counter stored at its sends, no test of the
    /// reified word at its returns -- and its Boolean path never transfers
    /// into the twin, so that no twin is compiled: the closure the
    /// conditional's fallback makes costs only the path that makes it.
    fn a_conditional_costs_its_boolean_path_no_reification(lowering) {
        let (mut interpreter, class) = load_with(RECURSION, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let mut statistics = interpreter.jit_statistics();
        for _ in 0..3 {
            let before = interpreter.jit_statistics();
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), 987);
            statistics = since(interpreter.jit_statistics(), before);
        }
        let selector = Symbol::intern("fib:");
        let reifies = interpreter.jit_body_reifies(&class, MethodSide::Instance, selector);
        let twin = interpreter.jit_twin_offset(&class, MethodSide::Instance, selector);
        // The reference lowering inlines no conditional: its `ifTrue:` is a
        // send of a real closure, which every activation makes. The zone
        // stress mode may have freed `fib:`'s code by the end of the run.
        if lowering == Lowering::Optimized {
            assert_eq!(twin, None, "no transfer compiled its twin");
            assert_eq!(statistics.compiled_twins, 0);
            if !a_stress_mode_is_on() {
                assert_eq!(reifies, Some(false), "`fib:` is compiled lean");
                assert_ran_in_machine_code(&statistics, "run");
            }
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A conditional sent to a receiver that is no Boolean -- `Yes`'s and
    /// `Later`'s `ifTrue:` -- transfers its frame into the body's twin at
    /// the fallback's `MakeClosure`, which spends no exit: the twin is
    /// compiled once, at the first transfer, the later runs make no exit
    /// but a safepoint's, and both lowerings answer what the interpreter
    /// does.
    fn a_conditional_sent_to_a_non_boolean_runs_in_the_twin(lowering) {
        let run = |jit: bool| {
            let (mut interpreter, class) = load_with(OUTER_LOCALS, lowering);
            interpreter.set_jit_enabled(jit);
            let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            let mut answers = Vec::new();
            let mut runs = Vec::new();
            for _ in 0..3 {
                let before = interpreter.jit_statistics();
                answers.push(send_unary(&mut interpreter, &subject, "run"));
                runs.push(since(interpreter.jit_statistics(), before));
            }
            let twin = interpreter.jit_twin_offset(
                &class,
                MethodSide::Instance,
                Symbol::intern("countUpTo:"),
            );
            (answers, runs, twin)
        };
        let (compiled, runs, twin) = run(true);
        let (interpreted, _, _) = run(false);
        assert_eq!(compiled, interpreted);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(twin.is_some(), "`countUpTo:`'s transfer compiled its twin");
            assert!(runs[0].compiled_twins + runs[1].compiled_twins > 0);
            assert_eq!(runs[2].compiled_twins, 0, "each twin is compiled once");
            assert_ran_in_machine_code(&runs[2], "run");
            assert_eq!(runs[2].abandoned_bodies, 0);
        }
    }
}

/// A resumable exception signalled under a compiled `on:do:`: `signal`'s
/// mirror send exits and materializes the frames below it, the handler
/// search reifies the `on:do:` frame that exit left stopped in machine
/// code, and `resume:` returns through the protected block into it.
const RESUMED_HANDLER: &str = indoc! {"
    class Subject = Object new () (
      warn = (
        ^(Exception signal) + 1
      )
      guarded = (
        ^[self warn + 2] on: Exception do: [:e | e resume: 5]
      )
      public run = (
        |
          total ::= 0.
          index ::= 0.
        |
        [index < 20] whileTrue: [
          total:: total + self guarded.
          index:: index + 1.
        ].
        ^total
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A frame the interpreter reified -- the `on:do:` frame the handler
    /// search reifies -- goes on in machine code only in its body's twin,
    /// compiled for it though `on:do:` makes no closure, so that its return
    /// divorces the frame as the interpreter's pop would: the twin is
    /// compiled once, the frame is resumed in machine code, no body is
    /// abandoned, and both lowerings answer what the interpreter does.
    fn a_frame_the_handler_search_reified_resumes_in_the_twin(lowering) {
        let run = |jit: bool| {
            let (mut interpreter, class) = load_with(RESUMED_HANDLER, lowering);
            interpreter.set_jit_enabled(jit);
            let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            let mut answers = Vec::new();
            let mut runs = Vec::new();
            for _ in 0..3 {
                let before = interpreter.jit_statistics();
                answers.push(send_unary(&mut interpreter, &subject, "run"));
                runs.push(since(interpreter.jit_statistics(), before));
            }
            let closure_class = interpreter.enter(|mutator| {
                let class = mutator.builtin_class(ns_runtime::BuiltinClass::Closure);
                mutator.stash(Value::object(class))
            });
            let twin = interpreter.jit_twin_offset(
                &closure_class,
                MethodSide::Instance,
                Symbol::intern("on:do:"),
            );
            (answers, runs, twin)
        };
        let (compiled, runs, twin) = run(true);
        let (interpreted, _, _) = run(false);
        assert_eq!(compiled, interpreted);
        assert_eq!(compiled[0], 20 * 8);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                twin.is_some(),
                "the reified `on:do:` frame, which makes no closure, went on in its twin"
            );
            assert_eq!(runs[2].compiled_twins, 0, "each twin is compiled once");
            assert!(runs[2].resumed_entries > 0, "frames resumed in machine code");
            assert_eq!(runs[2].abandoned_bodies, 0);
        }
    }
}

/// `make:` makes six closures, so that its twin, which reifies its frame
/// and allocates each closure inline, is several times its lean body,
/// which only transfers at each `MakeClosure`; `run` makes no closure.
const TWIN_TOO_LARGE: &str = indoc! {"
    class Subject = Object new () (
      make: x = (
        | first second third fourth fifth sixth |
        first:: [x].
        second:: [x + 1].
        third:: [x + 2].
        fourth:: [x + 3].
        fifth:: [x + 4].
        sixth:: [x + 5].
        ^x
      )
      public run = (
        |
          total ::= 0.
          index ::= 0.
        |
        [index < 20] whileTrue: [
          total:: total + (self make: index).
          index:: index + 1.
        ].
        ^total
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A body whose twin the zone could not hold even empty is refused for
    /// good at the exit its transfer makes (`refuse_for_its_twin`): the
    /// zone is sized to end where a first interpreter placed `make:`'s
    /// twin, right after its lean body, so that the lean body fits and the
    /// twin does not. The lean body is compiled and transfers once; the
    /// body is then refused, its code freed, nothing abandoned, and no
    /// later run compiles it or a twin again -- though an abandoned body
    /// would be revived after one interpreted entry here -- while every
    /// run answers what the interpreter does.
    fn a_body_whose_twin_is_too_large_is_refused_for_good(lowering) {
        let make = Symbol::intern("make:");
        let (mut probe, probe_class) = load_with(TWIN_TOO_LARGE, lowering);
        assert_eq!(run(&mut probe, &probe_class), 190);
        let lean = probe.jit_body_offset(&probe_class, MethodSide::Instance, make);
        let twin = probe.jit_twin_offset(&probe_class, MethodSide::Instance, make);
        let (mut interpreted, interpreted_class) = load_with(TWIN_TOO_LARGE, lowering);
        interpreted.set_jit_enabled(false);
        let expected = run(&mut interpreted, &interpreted_class);
        let checked = lowering == Lowering::Optimized && !a_stress_mode_is_on();
        let zone_bytes = match (lean, twin) {
            (Some(lean), Some(twin)) if checked => {
                assert!(lean < twin, "the twin follows its lean body");
                usize::try_from(twin).expect("small")
            }
            _ => 1 << 20,
        };
        let (mut interpreter, class) = load_with(TWIN_TOO_LARGE, lowering);
        interpreter.set_jit_zone_bytes(zone_bytes);
        interpreter.set_jit_revival_entries(1);
        let mut runs = Vec::new();
        for _ in 0..3 {
            let before = interpreter.jit_statistics();
            assert_eq!(run(&mut interpreter, &class), expected);
            runs.push(since(interpreter.jit_statistics(), before));
            if checked {
                assert_eq!(
                    interpreter.jit_body_offset(&class, MethodSide::Instance, make),
                    None,
                    "the refused body's code is freed and never compiled again"
                );
                assert_eq!(
                    interpreter.jit_twin_offset(&class, MethodSide::Instance, make),
                    None
                );
            }
        }
        assert_eq!(expected, 190);
        if checked {
            assert!(runs[0].compiled_bodies >= 1, "the lean body is compiled");
            assert!(runs[0].exits >= 1, "its transfer exits");
            assert!(runs[0].refused_bodies >= 1, "the body is refused");
            for (index, done) in runs.iter().enumerate() {
                assert_eq!(done.compiled_twins, 0, "run {index} compiles no twin");
                assert_eq!(done.abandoned_bodies, 0, "run {index} abandons nothing");
                assert_eq!(done.revived_bodies, 0, "run {index} revives nothing");
            }
        }
    }
}

/// Simultaneous slot clauses whose ends force seven futures a turn: two
/// for each `Cycle`, whose `first` holds `second`'s future unforced; one
/// for `Assigned`, whose `second` writes `first`, so that the end then
/// finds a value there; two for `thrice:`'s temporaries.
const SIMULTANEOUS: &str = indoc! {"
    class Subject = Object new () (
      class Cycle = Object new (
        ||
          public first = {second}.
          public second = 2.
        ||
      ) ()
      class Assigned = Object new (
        ||
          public second = first:: 5.
          public first ::= 1.
        ||
      ) ()
      thrice: number = (
        ||
          doubled = {half. number * 2}.
          half = number.
        ||
        ^(doubled at: 2) + half.
      )
      public run = (
        |
          total ::= 0.
          index ::= 0.
        |
        [index < 10] whileTrue: [
          total:: total + Cycle new second + Cycle new first size.
          total:: total + Assigned new first + (self thrice: index).
          index:: index + 1.
        ].
        ^total.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// `MakeFuture` and `ResolveFuture` run in machine code, and so does
    /// the forcing of every future: `ResolveFuture` calls the future's
    /// `resolve` as a compiled send, so that by the fifth run, once `run`
    /// and `resolve` are compiled, no exit leaves machine code under the
    /// optimized lowering but a safepoint's, and no body is abandoned.
    /// Both lowerings answer what the interpreter does; the reference
    /// lowering's uncached sends exit, so that only the optimized one
    /// runs the forcings without an exit.
    fn simultaneous_slots_run_without_an_exit(lowering) {
        let run = |jit: bool| {
            let (mut interpreter, class) = load_with(SIMULTANEOUS, lowering);
            interpreter.set_jit_enabled(jit);
            let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            let mut answer = 0;
            let mut statistics = interpreter.jit_statistics();
            for _ in 0..5 {
                let before = interpreter.jit_statistics();
                answer = send_unary(&mut interpreter, &subject, "run");
                statistics = since(interpreter.jit_statistics(), before);
            }
            (answer, statistics)
        };
        let (compiled, statistics) = run(true);
        let (interpreted, _) = run(false);
        assert_eq!(compiled, interpreted);
        assert_eq!(compiled, 215);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "run");
            assert_eq!(statistics.abandoned_bodies, 0, "no body is abandoned");
        }
    }
}

/// A slot clause whose first slot's block answers the second slot's
/// future, unresolved, so that the clause's end forcing `first` meets a
/// future of a future, instantiated more often per run than a body's exit
/// budget allows exits.
const ALIASING: &str = indoc! {"
    class Subject = Object new () (
      class Alias = Object new (
        ||
          public first = second.
          public second = 2.
        ||
      ) ()
      public run = (
        |
          total ::= 0.
          index ::= 0.
        |
        [index < 100] whileTrue: [
          total:: total + Alias new first.
          index:: index + 1.
        ].
        ^total.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A future whose computation answers another, unresolved future is
    /// resolved in machine code: `resolve` takes the inner future over
    /// through leaf-safe mirror rows, so it never exits, and so its exit
    /// budget is never spent and it stays compiled however many such
    /// clauses run. Over five runs of a hundred instances each, no body is
    /// abandoned, and the last run exits only at safepoints.
    fn a_future_of_a_future_resolves_without_an_exit(lowering) {
        let run = |jit: bool| {
            let (mut interpreter, class) = load_with(ALIASING, lowering);
            interpreter.set_jit_enabled(jit);
            let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            let first = interpreter.jit_statistics();
            let mut answer = 0;
            let mut statistics = interpreter.jit_statistics();
            for _ in 0..5 {
                let before = interpreter.jit_statistics();
                answer = send_unary(&mut interpreter, &subject, "run");
                statistics = since(interpreter.jit_statistics(), before);
            }
            (answer, statistics, since(interpreter.jit_statistics(), first))
        };
        let (compiled, statistics, overall) = run(true);
        let (interpreted, _, _) = run(false);
        assert_eq!(compiled, interpreted);
        assert_eq!(compiled, 200);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_ran_in_machine_code(&statistics, "run");
            assert_eq!(overall.abandoned_bodies, 0, "no body is abandoned");
        }
    }
}

/// A local of `prepare` that two blocks `Later` keeps write and read once
/// `prepare` has returned, its frame divorced: the optimized lowering's
/// fallback blocks of an inlined `ifTrue:`, which reach it by reference.
#[cfg(debug_assertions)]
const DIVORCED_HOLDER: &str = indoc! {"
    class Subject = Object new (
      |
        writer = Later new.
        reader = Later new.
      |
    ) (
      class Later = Object new (
        |
          kept ::= nil.
        |
      ) (
        public ifTrue: block = (
          kept:: block.
          ^nil.
        )
        public run = (
          ^kept value.
        )
      )
      public prepare = (
        |
          held ::= nil.
        |
        writer ifTrue: [
          held:: Array new: 5.
          held at: 1 put: 42.
          0.
        ].
        reader ifTrue: [(held at: 1) + held size].
        ^0.
      )
      public write = (
        ^writer run.
      )
      public read = (
        ^reader run.
      )
    ) : ()
"};

/// A store machine code makes of a young array into a register of a
/// divorced frame object, which is old, remembers the frame object, as
/// `Heap::frame_set_local`'s barrier does: the scavenge after it finds the
/// array through the remembered frame, and it reads back as written. A
/// store that missed the barrier leaves the frame naming nursery memory
/// the scavenge vacated.
#[cfg(debug_assertions)]
#[test]
fn an_outer_store_into_an_old_divorced_frame_remembers_it() {
    let (mut interpreter, class) = load_with(DIVORCED_HOLDER, Lowering::Optimized);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    assert_eq!(send_unary(&mut interpreter, &subject, "prepare"), 0);
    for _ in 0..3 {
        assert_eq!(send_unary(&mut interpreter, &subject, "write"), 0);
        assert_eq!(send_unary(&mut interpreter, &subject, "read"), 47);
    }
    // A full collection between entries compacts both generations into the
    // old space: the frame object is old from here on, and remembered by
    // nothing.
    interpreter.collect_now();
    let before = interpreter.jit_statistics();
    assert_eq!(send_unary(&mut interpreter, &subject, "write"), 0);
    let statistics = since(interpreter.jit_statistics(), before);
    interpreter.scavenge_now();
    assert_eq!(send_unary(&mut interpreter, &subject, "read"), 47);
    if !a_stress_mode_is_on() {
        assert_ran_in_machine_code(&statistics, "write");
        assert!(
            statistics.leaf_calls >= 1,
            "the store that remembered the frame object"
        );
    }
}

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    /// `OUTER_LOCALS` with every safepoint suspending the run's
    /// activations into their frame objects and resuming only the top
    /// one: a creator below the running block is suspended, its married
    /// word the negative `SmallInteger` of its side-table row, and its
    /// registers lie in its frame object, where the outer-local templates
    /// read and write them as the interpreter's `frame_local` does --
    /// never on the activation stack the word does not name.
    fn outer_locals_reach_a_suspended_creator(lowering) {
        let run = |jit: bool| {
            let (mut interpreter, class) = load_with(OUTER_LOCALS, lowering);
            interpreter.set_jit_enabled(jit);
            interpreter.set_stress_suspend(true);
            let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            let suspensions = interpreter.stress_suspensions();
            let mut answer = 0;
            for _ in 0..3 {
                answer = send_unary(&mut interpreter, &subject, "run");
            }
            assert!(
                interpreter.stress_suspensions() > suspensions,
                "the run's activations were suspended"
            );
            answer
        };
        assert_eq!(run(true), run(false));
    }
}

/// `count` methods `m0`, `m1`, ..., each answering its index and one, and
/// for each a method `hot` sending it a hundred times from a loop of its
/// own, which `run` sends one after another: each `m` is hot for a while
/// and then never sent again.
fn hot_in_turn(count: usize) -> String {
    let methods: String = (0..count)
        .map(|index| {
            formatdoc! {"
                  m{index} = (
                    ^{index} + 1
                  )
                  hot{index} = (
                    |
                      sum ::= 0.
                      index ::= 0.
                    |
                    [index < 100] whileTrue: [
                      sum:: sum + self m{index}.
                      index:: index + 1.
                    ].
                    ^sum
                  )
            "}
        })
        .collect();
    let sends: String = (0..count)
        .map(|index| format!("    total:: total + self hot{index}.\n"))
        .collect();
    formatdoc! {"
        class Subject = Object new () (
        {methods}  public run = (
            |
              total ::= 0.
            |
        {sends}    ^total
          )
        ) : ()
    "}
}

ns_runtime::for_each_lowering! {
    /// A zone of four pages holds a few of two hundred methods hot in
    /// turn: a compile that finds it full asks for a compaction, which
    /// frees the methods no longer sent, and every method is compiled once
    /// it is hot, none refused for want of room, the output the
    /// interpreter's.
    fn a_full_zone_compacts_and_compiles_again(lowering) {
        let count = 200;
        let source = hot_in_turn(count);
        let (mut interpreter, class) = load_with(&source, lowering);
        interpreter.set_jit_zone_bytes(16_384);
        let (answer, done) = run_measured(&mut interpreter, &class);
        let expected: i64 = (1..=200).map(|index: i64| 100 * index).sum();
        assert_eq!(answer, expected, "the interpreter's");
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(done.zone_compactions > 0, "the zone filled");
            let held = interpreter.table_sizes().zone_bodies as u64;
            assert!(
                done.compiled_bodies >= count as u64,
                "every hot method compiled: {}",
                done.compiled_bodies
            );
            assert!(
                done.compiled_bodies > held,
                "more than the zone holds at once ({held})"
            );
            assert_eq!(done.refused_bodies, 0, "no room refuses no body");
            assert!(done.freed_bodies > 0);
        }
    }
}

/// `a` exits at its send of a message nothing understands, after a loop
/// that has its body compiled under either lowering; `b` runs to its
/// return; `callB` sends `b`, and `callBOnce` sends it from a body run
/// only once, and so interpreted; `apply:` sends `value:` to a block
/// `makeBlock` makes; `makeAndRun` makes a block in its own machine code
/// and sends it.
const MOVING: &str = indoc! {"
    class Subject = Object new () (
      public a = (
        |
          index ::= 0.
        |
        [index < 3] whileTrue: [
          index:: index + 1.
        ].
        ^self missing
      )
      public b = (
        |
          index ::= 0.
        |
        [index < 3] whileTrue: [
          index:: index + 1.
        ].
        ^index + 4
      )
      public callB = (
        ^self b + 1
      )
      public callBOnce = (
        ^self b + 2
      )
      public makeBlock = (
        ^[:value | value + 1]
      )
      public apply: block = (
        ^block value: 3
      )
      public spin = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 2000] whileTrue: [
          sum:: sum + index.
          index:: index + 1.
        ].
        ^sum
      )
      public makeAndRun = (
        |
          index ::= 0.
        |
        [index < 3] whileTrue: [
          index:: index + 1.
        ].
        ^[:value | value + index] value: 10
      )
      public doesNotUnderstand: message = (
        ^1
      )
    ) : ()
"};

/// Sends `a` to `subject` until its body, compiled first, is abandoned for
/// the exits it keeps making, so that it is freed.
fn abandon_a(interpreter: &mut Interpreter, subject: &Root) {
    for _ in 0..70 {
        assert_eq!(send_unary(interpreter, subject, "a"), 1);
    }
}

ns_runtime::for_each_lowering! {
    /// A compaction slides a body down over one freed below it, and the
    /// cell a compiled send calls it through, and an interpreter's send,
    /// still reach its machine code at its new place: no exit, no miss.
    fn a_compaction_moves_a_body_its_cells_call(lowering) {
        let (mut interpreter, class) = load_with(MOVING, lowering);
        interpreter.set_jit_counting(true);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..2 {
            assert_eq!(send_unary(&mut interpreter, &subject, "a"), 1);
        }
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "callB"), 8);
        }
        let offset = |interpreter: &mut Interpreter, selector: &str| {
            interpreter.jit_body_offset(&class, MethodSide::Instance, Symbol::intern(selector))
        };
        let freed_offset_before = offset(&mut interpreter, "a");
        let moved_offset_before = offset(&mut interpreter, "b");
        abandon_a(&mut interpreter, &subject);
        interpreter.compact_jit_zone();
        let moved_offset_after = offset(&mut interpreter, "b");
        if let Err(message) = interpreter.verify_cells() {
            panic!("after the compaction: {message}");
        }
        let before = interpreter.jit_statistics();
        assert_eq!(send_unary(&mut interpreter, &subject, "callB"), 8);
        assert_eq!(send_unary(&mut interpreter, &subject, "callBOnce"), 9);
        let done = since(interpreter.jit_statistics(), before);
        // The reference lowering leaves every send uncached, `<` and `+`
        // among them, so that its bodies exit at their first send and are
        // never compiled.
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            let (
                Some(freed_offset_before),
                Some(moved_offset_before),
                Some(moved_offset_after),
            ) = (freed_offset_before, moved_offset_before, moved_offset_after)
            else {
                panic!("`a` and `b` were compiled, and `b` survives");
            };
            assert!(freed_offset_before < moved_offset_before, "`a` compiled first");
            assert_eq!(offset(&mut interpreter, "a"), None, "`a` is freed");
            assert!(
                moved_offset_after < moved_offset_before,
                "`b` moved down: {moved_offset_before:#x} to {moved_offset_after:#x}"
            );
            assert!(
                done.called_entries >= 1,
                "`callBOnce`'s send called `b`'s machine code"
            );
            assert_eq!(done.exits, 0, "nothing left machine code");
            assert_eq!(done.miss_leaf_calls, 0, "the cell names `b`'s new place");
            assert!(done.compiled_calls >= 1, "`callB` calls `b` through its cell");
        }
    }
}

/// A compaction slides a body's optimized code down with its baseline
/// code over a body freed below both, and the next run pushes it at its
/// new place: no optimization again, the same answer. `a`, compiled
/// first, is abandoned for its exits; `spin`, compiled after it, is
/// optimized after it is abandoned, its budget armed only then.
#[test]
fn a_compaction_slides_optimized_code() {
    let (mut interpreter, class) = load_with(MOVING, Lowering::Optimized);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    for _ in 0..2 {
        assert_eq!(send_unary(&mut interpreter, &subject, "a"), 1);
    }
    assert_eq!(send_unary(&mut interpreter, &subject, "spin"), 1_999_000);
    abandon_a(&mut interpreter, &subject);
    interpreter.set_optimization_budget(OPTIMIZING_BUDGET);
    for _ in 0..2 {
        assert_eq!(send_unary(&mut interpreter, &subject, "spin"), 1_999_000);
    }
    let spin = Symbol::intern("spin");
    let optimized_before = interpreter.jit_optimized_offset(&class, MethodSide::Instance, spin);
    let before = interpreter.jit_statistics();
    interpreter.compact_jit_zone();
    let optimized_after = interpreter.jit_optimized_offset(&class, MethodSide::Instance, spin);
    if let Err(message) = interpreter.verify_cells() {
        panic!("after the compaction: {message}");
    }
    assert_eq!(send_unary(&mut interpreter, &subject, "spin"), 1_999_000);
    let done = since(interpreter.jit_statistics(), before);
    if !a_stress_mode_is_on() {
        let (Some(optimized_before), Some(optimized_after)) = (optimized_before, optimized_after)
        else {
            panic!("`spin` is optimized, and its optimized code survives the compaction");
        };
        assert!(
            optimized_after < optimized_before,
            "the optimized code moved down: {optimized_before:#x} to {optimized_after:#x}"
        );
        assert!(done.moved_bytes > 0);
        assert_eq!(done.optimized_bodies, 0, "not optimized again");
        assert_eq!(done.exits, 0, "nothing left machine code");
    }
}

ns_runtime::for_each_lowering! {
    /// A compaction that keeps a body keeps the exits its machine code
    /// made: `a`, which leaves at every call, is abandoned at its
    /// sixty-fourth exit, though forty of them came before the compaction,
    /// so that how often the zone fills never resets the exit budget.
    fn a_compaction_keeps_the_exits_of_a_surviving_body(lowering) {
        let (mut interpreter, class) = load_with(MOVING, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let start = interpreter.jit_statistics();
        let mut sends = 0;
        while since(interpreter.jit_statistics(), start).exits < 40 && sends < 200 {
            assert_eq!(send_unary(&mut interpreter, &subject, "a"), 1);
            sends += 1;
        }
        let before_compaction = since(interpreter.jit_statistics(), start);
        let offset = |interpreter: &mut Interpreter| {
            interpreter.jit_body_offset(&class, MethodSide::Instance, Symbol::intern("a"))
        };
        let offset_before = offset(&mut interpreter);
        interpreter.compact_jit_zone();
        let offset_after = offset(&mut interpreter);
        while since(interpreter.jit_statistics(), start).abandoned_bodies == 0 && sends < 200 {
            assert_eq!(send_unary(&mut interpreter, &subject, "a"), 1);
            sends += 1;
        }
        let done = since(interpreter.jit_statistics(), start);
        // The reference lowering leaves every send uncached, `<` among
        // them, so that its body exits at once and is never compiled.
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(before_compaction.abandoned_bodies, 0, "`a` still compiled");
            assert!(
                offset_before.is_some() && offset_after.is_some(),
                "`a` survives the compaction: {offset_before:?} to {offset_after:?}"
            );
            assert_eq!(done.zone_compactions, 1, "the one compaction");
            assert_eq!(done.abandoned_bodies, 1, "`a` is abandoned");
            assert_eq!(
                done.exits, 64,
                "at its sixty-fourth exit, those before the compaction counted"
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A compaction slides a block's body down, and its machine entry row
    /// moves with it: the closure stub, and an interpreter's `value:`,
    /// enter its machine code at its new place.
    fn a_moved_block_body_is_called_through_its_row(lowering) {
        let (mut interpreter, class) = load_with(MOVING, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..2 {
            assert_eq!(send_unary(&mut interpreter, &subject, "a"), 1);
        }
        let block = expect_ok(
            interpreter.send(&subject, Symbol::intern("makeBlock"), &[]),
            "makeBlock",
        );
        let apply = |interpreter: &mut Interpreter| {
            let answer = expect_ok(
                interpreter.send(&subject, Symbol::intern("apply:"), std::slice::from_ref(&block)),
                "apply:",
            );
            interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
                ValueKind::SmallInteger(integer) => integer,
                _ => panic!("apply: expected a small integer"),
            })
        };
        for _ in 0..4 {
            assert_eq!(apply(&mut interpreter), 4);
        }
        let rows = |interpreter: &mut Interpreter| {
            interpreter.jit_block_rows(&class, MethodSide::Instance, Symbol::intern("makeBlock"))
        };
        let before_rows = rows(&mut interpreter);
        abandon_a(&mut interpreter, &subject);
        interpreter.compact_jit_zone();
        let after_rows = rows(&mut interpreter);
        let before = interpreter.jit_statistics();
        assert_eq!(apply(&mut interpreter), 4);
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            let [(entry_before, Some(address_before))] = before_rows[..] else {
                panic!("the block is compiled and its row written: {before_rows:?}");
            };
            assert_eq!(entry_before, address_before, "the row names the block's code");
            let [(entry_after, Some(address_after))] = after_rows[..] else {
                panic!("the block survives the compaction: {after_rows:?}");
            };
            assert_eq!(entry_after, address_after, "the row moved with the block's code");
            assert!(address_after < address_before, "the block's code moved down");
            assert_eq!(done.exits, 0, "nothing left machine code");
            assert!(
                done.machine_entries + done.called_entries >= 1,
                "machine code ran"
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A twin's closure site moves with its code: filled before the
    /// compaction, it is not filled again after it -- its words, the code
    /// table's epoch among them, are what they were -- and the body still
    /// makes the closure right.
    fn a_moved_body_keeps_its_closure_sites(lowering) {
        let (mut interpreter, class) = load_with(MOVING, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..2 {
            assert_eq!(send_unary(&mut interpreter, &subject, "a"), 1);
        }
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "makeAndRun"), 13);
        }
        let selector = Symbol::intern("makeAndRun");
        let words_before = interpreter.jit_twin_words(&class, MethodSide::Instance, selector);
        let offset_before = interpreter.jit_twin_offset(&class, MethodSide::Instance, selector);
        abandon_a(&mut interpreter, &subject);
        interpreter.compact_jit_zone();
        let words_after = interpreter.jit_twin_words(&class, MethodSide::Instance, selector);
        let offset_after = interpreter.jit_twin_offset(&class, MethodSide::Instance, selector);
        assert_eq!(send_unary(&mut interpreter, &subject, "makeAndRun"), 13);
        let words_run = interpreter.jit_twin_words(&class, MethodSide::Instance, selector);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            let Some(words_before) = words_before else {
                panic!("`makeAndRun` is compiled");
            };
            assert!(
                words_before.first().is_some_and(|epoch| *epoch != 0),
                "its site was filled: {words_before:?}"
            );
            assert!(offset_after < offset_before, "it moved down");
            assert_eq!(
                words_after.as_ref(),
                Some(&words_before),
                "moved with its code"
            );
            assert_eq!(words_run, Some(words_before), "not filled again");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A compaction moves a lean body and its twin, each down over the
    /// code freed below it, and writes the lean body's transfer word
    /// afresh: the next transfer reaches the twin at its new place, with
    /// no exit and no twin compiled again.
    fn a_compaction_moves_a_body_and_its_twin(lowering) {
        let (mut interpreter, class) = load_with(MOVING, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..2 {
            assert_eq!(send_unary(&mut interpreter, &subject, "a"), 1);
        }
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "makeAndRun"), 13);
        }
        let selector = Symbol::intern("makeAndRun");
        let offsets = |interpreter: &mut Interpreter| {
            (
                interpreter.jit_body_offset(&class, MethodSide::Instance, selector),
                interpreter.jit_twin_offset(&class, MethodSide::Instance, selector),
            )
        };
        let (body_before, twin_before) = offsets(&mut interpreter);
        let transfer_before = interpreter.jit_body_words(&class, MethodSide::Instance, selector);
        abandon_a(&mut interpreter, &subject);
        interpreter.compact_jit_zone();
        let (body_after, twin_after) = offsets(&mut interpreter);
        let transfer_after = interpreter.jit_body_words(&class, MethodSide::Instance, selector);
        let before = interpreter.jit_statistics();
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "makeAndRun"), 13);
        }
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            let (Some(body_before), Some(twin_before), Some(body_after), Some(twin_after)) =
                (body_before, twin_before, body_after, twin_after)
            else {
                panic!("`makeAndRun` and its twin are compiled, and survive the compaction");
            };
            assert!(body_before < twin_before, "the twin compiled after its body");
            assert!(body_after < body_before, "the body moved down");
            assert!(twin_after < twin_before, "the twin moved down");
            assert_ne!(
                transfer_after, transfer_before,
                "the transfer word names the twin's new place"
            );
            assert_eq!(done.compiled_twins, 0, "the twin is not compiled again");
            assert_eq!(done.exits, 0, "nothing left machine code");
            assert!(done.machine_entries + done.called_entries > 0);
        }
    }
}

/// `count` methods, each sending the next and adding one to what it
/// answers, the last answering 0, each after a loop so that its body is
/// compiled under either lowering.
fn chain_of(count: usize) -> String {
    let methods: String = (0..count)
        .map(|index| {
            let next = if index + 1 == count {
                "0".to_string()
            } else {
                format!("(self m{}) + 1", index + 1)
            };
            formatdoc! {"
                  m{index} = (
                    |
                      index ::= 0.
                    |
                    [index < 1] whileTrue: [
                      index:: index + 1.
                    ].
                    ^{next}
                  )
            "}
        })
        .collect();
    formatdoc! {"
        class Subject = Object new () (
        {methods}  public run = (
            ^self m0
          )
        ) : ()
    "}
}

ns_runtime::for_each_lowering! {
    /// A recursion through more bodies than the zone holds fills it with
    /// active code: a compaction its compile asks for frees nothing it
    /// runs, and the zone, still full, compiles nothing more and is not
    /// compacted again at every safepoint -- each retry waits for twice
    /// the held compiles the last one did, so the compactions grow with
    /// the logarithm of the entries, not with them -- and a full
    /// collection lets it try once more.
    fn a_zone_full_of_active_code_does_not_compact_at_every_safepoint(lowering) {
        let count = 300;
        let source = chain_of(count);
        let (mut interpreter, class) = load_with(&source, lowering);
        interpreter.set_jit_zone_bytes(16_384);
        let before = interpreter.jit_statistics();
        let runs = 4;
        for _ in 0..runs {
            assert_eq!(run(&mut interpreter, &class), count as i64 - 1);
        }
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Reference || a_stress_mode_is_on() {
            return;
        }
        // Each run enters every body once, so at most that many compiles
        // are held; with the retries doubling from 64 held compiles, at
        // most log2(1,200 / 64) + 1 retries follow the first compaction.
        let held = (runs * count) as u64;
        let most = u64::from((held / 64).ilog2()) + 2;
        assert!(
            done.zone_compactions >= 1 && done.zone_compactions <= most,
            "compacted {} times, not at every safepoint",
            done.zone_compactions
        );
        #[cfg(debug_assertions)]
        {
            let compactions = done.zone_compactions;
            interpreter.collect_now();
            assert_eq!(run(&mut interpreter, &class), count as i64 - 1);
            let done = since(interpreter.jit_statistics(), before);
            assert!(
                done.zone_compactions > compactions,
                "tried once more after the collection"
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// The zone stress mode compacts at the safepoints its schedule picks,
    /// freeing every body no activation runs, and the program still
    /// answers what it answers without it: the mode the goldens run under
    /// (`NS_NEXT_STRESS_ZONE`) does what it says.
    #[cfg(debug_assertions)]
    fn the_zone_stress_mode_frees_and_moves_bodies(lowering) {
        let source = hot_in_turn(20);
        let (mut interpreter, class) = load_with(&source, lowering);
        interpreter.set_stress_zone(true);
        let (answer, done) = run_measured(&mut interpreter, &class);
        let expected: i64 = (1..=20).map(|index: i64| 100 * index).sum();
        assert_eq!(answer, expected, "the interpreter's");
        assert!(interpreter.stress_zone_compactions() > 0, "the mode compacted");
        assert!(done.zone_compactions > 0);
        if lowering == Lowering::Optimized {
            assert!(done.freed_bodies > 0, "the hot methods are freed once cold");
        }
        // `spin`, compiled right after `a`, runs its loop while the mode
        // compacts: it is active, so it survives, and slides down over
        // `a`, which no activation runs, freed below it. The mode holds
        // the safepoint request, so that every interpreted backward jump
        // of `spin`'s first send stops at a safepoint, and a backward jump
        // stopped by a safepoint counts toward the body's compile as any
        // other does: the loop's head is where the frame would have
        // switched had nothing been requested (`vm::after_safepoint`). The
        // backward jumps' trigger is set past `spin`'s turns here, so
        // that the first send leaves `spin` uncompiled and its second
        // entry compiles it right after `a`.
        let (mut interpreter, class) = load_with(MOVING, lowering);
        interpreter.set_stress_zone(true);
        interpreter.set_jit_compile_at_back_jump(u32::MAX);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let before = interpreter.jit_statistics();
        assert_eq!(send_unary(&mut interpreter, &subject, "spin"), 1999 * 1000);
        for _ in 0..2 {
            assert_eq!(send_unary(&mut interpreter, &subject, "a"), 1);
        }
        for _ in 0..2 {
            assert_eq!(send_unary(&mut interpreter, &subject, "spin"), 1999 * 1000);
        }
        let done = since(interpreter.jit_statistics(), before);
        // Another stress mode changes which bodies compile when.
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(done.moved_bytes > 0, "a survivor slid down over freed code");
        }
    }
}

/// A program whose `answer` an `edit:` replaces through a MirrorBuilder
/// with a body that loops and then answers `number` more, and whose `run`
/// sends `answer` to a fresh `Edited`: every edit installs a new version,
/// whose old body dies with it.
const EDITED_ANSWER: &str = indoc! {"
    class Subject usingPlatform: platform = Object new (
      |
        ClassMirror = platform mirrors ClassMirror.
      |
    ) (
      class Edited = Object new () (
        public answer = (
          |
            index ::= 0.
          |
          [index < 3] whileTrue: [
            index:: index + 1.
          ].
          ^index
        )
      )
      public edited = (
        ^Edited
      )
      public run = (
        ^Edited new answer
      )
      public edit: number = (
        | builder |
        builder:: (ClassMirror reflecting: Edited) mixin declaration asBuilder.
        builder instanceSide methods
            addFromSource: 'public answer = ( | index ::= 0. | [index < 3] whileTrue: [ index:: index + 1 ]. ^index + ' , number printString , ' )'.
        builder install.
        ^number
      )
    ) : ()
"};

/// `EDITED_ANSWER` loaded and instantiated with the platform, and its
/// `Edited` class.
fn edited_answer(lowering: Lowering) -> (Interpreter, Root, Root) {
    let (mut interpreter, class) = load_with(EDITED_ANSWER, lowering);
    let platform = interpreter.platform();
    let subject = expect_ok(
        interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
        "usingPlatform:",
    );
    let edited = expect_ok(
        interpreter.send(&subject, Symbol::intern("edited"), &[]),
        "edited",
    );
    (interpreter, subject, edited)
}

/// Sends `selector` to `subject` with the `SmallInteger` `number`,
/// answering the `SmallInteger` it answers.
fn send_number(interpreter: &mut Interpreter, subject: &Root, selector: &str, number: i64) -> i64 {
    let argument = interpreter.stash(Value::small_integer(number));
    let answer = expect_ok(
        interpreter.send(subject, Symbol::intern(selector), &[argument]),
        selector,
    );
    interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("{selector}: expected a small integer"),
    })
}

ns_runtime::for_each_lowering! {
    /// Thousands of edits of one hot method in a zone of 64 KiB: each
    /// edit's body dies with its version and is freed -- at a full
    /// collection, or by the compaction a full zone asks for -- so that
    /// every new version is compiled and runs in machine code, and the
    /// zone's bodies and returns stay within what the zone holds, not
    /// what the edits made.
    fn thousands_of_edits_keep_compiling_in_a_small_zone(lowering) {
        let zone_bytes = 65_536;
        let (mut interpreter, subject, edited) = edited_answer(lowering);
        interpreter.set_jit_zone_bytes(zone_bytes);
        let edits = 3_000;
        let checks = lowering == Lowering::Optimized && !a_stress_mode_is_on();
        let mut most_bodies = 0;
        let mut most_returns = 0;
        for number in 1..=edits {
            assert_eq!(send_number(&mut interpreter, &subject, "edit:", number), number);
            let before = interpreter.jit_statistics();
            for _ in 0..10 {
                assert_eq!(send_unary(&mut interpreter, &subject, "run"), 3 + number);
            }
            let done = since(interpreter.jit_statistics(), before);
            let sizes = interpreter.table_sizes();
            most_bodies = most_bodies.max(sizes.zone_bodies);
            most_returns = most_returns.max(sizes.zone_returns);
            assert!(sizes.zone_used_bytes <= zone_bytes, "within the zone");
            if checks {
                assert!(
                    done.machine_entries + done.called_entries >= 8,
                    "edit {number}: every run after the first two in machine code: {done:?}"
                );
                assert!(
                    interpreter
                        .jit_body_offset(&edited, MethodSide::Instance, Symbol::intern("answer"))
                        .is_some(),
                    "edit {number}: the new version is compiled"
                );
            }
        }
        // No body takes fewer than 64 bytes, so the zone holds at most a
        // thousand of them, whatever the number of edits.
        let bound = zone_bytes / 64;
        assert!(most_bodies <= bound, "{most_bodies} bodies at most");
        assert!(most_returns <= bound, "{most_returns} returns at most");
        if checks {
            assert!(
                interpreter.jit_statistics().freed_bodies >= 1_500,
                "the edited-away bodies were freed"
            );
        }
    }
}

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    /// A full collection frees the machine code of a body that died with
    /// its version, without waiting for the zone to fill: `freed_bodies`
    /// rises by the collection, which compacts nothing. Counted from
    /// before the edit, so that a collection a stress mode makes inside
    /// the edit, which frees the body first, counts as well.
    fn a_full_collection_frees_the_code_of_a_dead_body(lowering) {
        let (mut interpreter, subject, edited) = edited_answer(lowering);
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), 3);
        }
        let compiled = interpreter
            .jit_body_offset(&edited, MethodSide::Instance, Symbol::intern("answer"))
            .is_some();
        let before = interpreter.jit_statistics();
        assert_eq!(send_number(&mut interpreter, &subject, "edit:", 1), 1);
        interpreter.collect_now();
        let done = since(interpreter.jit_statistics(), before);
        assert_eq!(send_unary(&mut interpreter, &subject, "run"), 4);
        // The reference lowering's body exits at once and is never
        // installed. The zone stress mode compacts, and may have freed the
        // body before the edit and compiled it no more.
        if lowering == Lowering::Optimized {
            if !the_zone_stress_mode_is_on() {
                assert!(compiled, "`answer` was compiled");
                assert_eq!(done.zone_compactions, 0, "without a compaction");
            }
            if compiled {
                assert!(done.freed_bodies >= 1, "the dead body's code is freed");
            }
        }
    }
}

/// `count` methods `leave0`, `leave1`, ..., each looping and then sending
/// a message the class's `doesNotUnderstand:` answers, which machine code
/// leaves to the interpreter, and for each a `drive` sending it 70 times,
/// past the exit budget; and `hot`, which loops and leaves nowhere.
fn leaving_bodies(count: usize) -> String {
    let methods: String = (0..count)
        .map(|index| {
            formatdoc! {"
                  leave{index} = (
                    |
                      index ::= 0.
                    |
                    [index < 2] whileTrue: [
                      index:: index + 1.
                    ].
                    ^self missing{index} + index
                  )
                  drive{index} = (
                    |
                      sum ::= 0.
                      index ::= 0.
                    |
                    [index < 70] whileTrue: [
                      sum:: sum + self leave{index}.
                      index:: index + 1.
                    ].
                    ^sum
                  )
            "}
        })
        .collect();
    let sends: String = (0..count)
        .map(|index| format!("    total:: total + self drive{index}.\n"))
        .collect();
    formatdoc! {"
        class Subject = Object new () (
        {methods}  public run = (
            |
              total ::= 0.
            |
        {sends}    ^total
          )
          public hot = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < 10] whileTrue: [
              sum:: sum + index.
              index:: index + 1.
            ].
            ^sum
          )
          public doesNotUnderstand: message = (
            ^1
          )
        ) : ()
    "}
}

ns_runtime::for_each_lowering! {
    /// Five hundred bodies, each abandoned for leaving on every call, do
    /// not fill the zone: each is freed when it is abandoned, the zone is
    /// compacted when a compile finds it full, and a body compiled after
    /// them all still finds room.
    fn abandoned_bodies_do_not_fill_the_zone(lowering) {
        let count = 500;
        let source = leaving_bodies(count);
        let (mut interpreter, class) = load_with(&source, lowering);
        interpreter.set_jit_zone_bytes(32_768);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let before = interpreter.jit_statistics();
        assert_eq!(
            send_unary(&mut interpreter, &subject, "run"),
            (count * 70 * 3) as i64
        );
        // `hot` compiles at its second entry. Whether that compile finds
        // room depends on where the last compaction left the zone's end,
        // which depends on the size of every `leave` body: a debug build's
        // are larger, each frame register's load checked for the poison of
        // a dead register, so the two builds end `run` at different places.
        // A compile that finds no room asks for a compaction and counts its
        // entries again, so the worst case is two entries more: four.
        for _ in 0..4 {
            assert_eq!(send_unary(&mut interpreter, &subject, "hot"), 45);
        }
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                done.abandoned_bodies >= count as u64,
                "every `leave` abandoned: {}",
                done.abandoned_bodies
            );
            assert!(done.zone_compactions > 0, "the zone compacted");
            assert!(
                done.compiled_bodies >= count as u64,
                "every `leave` compiled"
            );
            assert!(
                interpreter
                    .jit_body_offset(&class, MethodSide::Instance, Symbol::intern("hot"))
                    .is_some(),
                "`hot`, compiled after them all, found room"
            );
        }
    }
}

/// `hot`, a loop that leaves nowhere, and `runHot:`, which sends it
/// `count` times from a loop of its own.
const HOT_AFTERWARDS: &str = indoc! {"
      public hot = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 10] whileTrue: [
          sum:: sum + index.
          index:: index + 1.
        ].
        ^sum
      )
      public runHot: count = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < count] whileTrue: [
          sum:: sum + self hot.
          index:: index + 1.
        ].
        ^sum
      )
"};

ns_runtime::for_each_lowering! {
    /// A zone exhausted by active code compiles again once that code has
    /// returned, in a program that allocates nothing and so never runs a
    /// full collection: the compiles the exhausted zone holds ask for a
    /// compaction again, which frees the recursion's idle bodies, and a
    /// method hot only afterwards is compiled and runs in machine code,
    /// every call of it counted (`Interpreter::set_jit_counting`), whether
    /// its caller calls it from machine code or from the interpreter.
    fn an_exhausted_zone_compiles_again_without_a_collection(lowering) {
        let count = 300;
        let source = chain_of(count).replacen(
            "  public run = (",
            &format!("{HOT_AFTERWARDS}  public run = ("),
            1,
        );
        let (mut interpreter, class) = load_with(&source, lowering);
        interpreter.set_jit_zone_bytes(16_384);
        interpreter.set_jit_counting(true);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let collections = interpreter.collector_statistics().collections;
        let before = interpreter.jit_statistics();
        // The second run compiles each body it enters, and so fills the
        // zone with the code of the frames it is in.
        for _ in 0..2 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), count as i64 - 1);
        }
        let filled = since(interpreter.jit_statistics(), before);
        assert_eq!(send_number(&mut interpreter, &subject, "runHot:", 1_000), 45_000);
        let after_hot = interpreter.jit_statistics();
        assert_eq!(send_number(&mut interpreter, &subject, "runHot:", 10), 450);
        let last = since(interpreter.jit_statistics(), after_hot);
        if lowering == Lowering::Reference || a_stress_mode_is_on() {
            return;
        }
        assert!(
            filled.zone_compactions >= 1,
            "the recursion filled the zone with active code"
        );
        assert_eq!(
            interpreter.collector_statistics().collections,
            collections,
            "no full collection ran"
        );
        assert!(
            interpreter
                .jit_body_offset(&class, MethodSide::Instance, Symbol::intern("hot"))
                .is_some(),
            "`hot` is compiled"
        );
        // `hot` is called from `runHot:`'s machine code through its cell
        // where the zone had room for `runHot:` at its loop's 40th backward
        // jump, and from the interpreter through the call stub otherwise:
        // a debug build's larger machine code leaves no room for it.
        assert!(
            last.compiled_calls + last.called_entries >= 10,
            "`hot` runs in machine code: {last:?}"
        );
        // A compiled call its callee refused would be counted too, and
        // would leave the machine code by an exit.
        assert_eq!(last.exits, 0, "no call was refused: {last:?}");
    }
}

/// `leave:`, which loops and then, when its argument is 0, sends a message
/// the class's `doesNotUnderstand:` answers, which machine code leaves to
/// the interpreter; `callLeave:`, which sends it ten times from a loop;
/// and `drive:`, which sends it a thousand times, its argument 0 -- an
/// exit -- only at the turn `drive:`'s argument names.
#[cfg(debug_assertions)]
const DECAYING: &str = indoc! {"
    class Subject = Object new () (
      public leave: flag = (
        |
          index ::= 0.
        |
        [index < 2] whileTrue: [
          index:: index + 1.
        ].
        flag = 0 ifTrue: [^self missing + index].
        ^index
      )
      public callLeave: flag = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 10] whileTrue: [
          sum:: sum + (self leave: flag).
          index:: index + 1.
        ].
        ^sum
      )
      public drive: exitAt = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 1000] whileTrue: [
          sum:: sum + (self leave: index - exitAt).
          index:: index + 1.
        ].
        ^sum
      )
      public doesNotUnderstand: message = (
        ^1
      )
    ) : ()
"};

/// Sends `leave: 0` to `subject` 70 times, each leaving machine code once
/// the body is compiled: past the exit budget, so that it is abandoned.
#[cfg(debug_assertions)]
fn spend_the_exit_budget(interpreter: &mut Interpreter, subject: &Root) {
    for _ in 0..70 {
        assert_eq!(send_number(interpreter, subject, "leave:", 0), 3);
    }
}

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    /// A body abandoned for its exits counts them down at every scavenge,
    /// and once two have halved them below half the budget it is revived:
    /// it counts its entries again, is compiled at its second, and runs in
    /// machine code.
    fn an_abandoned_body_compiles_again_once_its_exits_decay(lowering) {
        let (mut interpreter, class) = load_with(DECAYING, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let before = interpreter.jit_statistics();
        spend_the_exit_budget(&mut interpreter, &subject);
        let abandoned = since(interpreter.jit_statistics(), before);
        let before_scavenges = interpreter.jit_statistics();
        interpreter.scavenge_now();
        interpreter.scavenge_now();
        let decayed = since(interpreter.jit_statistics(), before_scavenges);
        if let Err(message) = interpreter.verify_cells() {
            panic!("after the revival: {message}");
        }
        for _ in 0..2 {
            assert_eq!(send_number(&mut interpreter, &subject, "leave:", 1), 2);
        }
        let before_hot = interpreter.jit_statistics();
        for _ in 0..10 {
            assert_eq!(send_number(&mut interpreter, &subject, "leave:", 1), 2);
        }
        let hot = since(interpreter.jit_statistics(), before_hot);
        // The reference lowering leaves every send uncached, `<` among
        // them, so that its body exits at once and is never compiled.
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(abandoned.abandoned_bodies, 1, "`leave:` is abandoned");
            assert_eq!(decayed.revived_bodies, 1, "two scavenges revive it");
            assert!(
                interpreter
                    .jit_body_offset(&class, MethodSide::Instance, Symbol::intern("leave:"))
                    .is_some(),
                "`leave:` is compiled again"
            );
            assert_eq!(hot.machine_entries, 10, "every later run is in machine code");
            assert_eq!(hot.exits, 0, "and leaves it only by its returns");
        }
    }
}

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    /// A caller's cached send of a callee the JIT abandoned names the
    /// refusing stub; once the callee is revived, the cell is written
    /// afresh from its entry and names the miss stub, which compiles the
    /// callee, and the send calls its machine code again rather than
    /// refusing the call and spending the caller's budget.
    fn a_revived_callee_is_called_from_machine_code_again(lowering) {
        let (mut interpreter, class) = load_with(DECAYING, lowering);
        interpreter.set_jit_counting(true);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..3 {
            assert_eq!(send_number(&mut interpreter, &subject, "callLeave:", 1), 20);
        }
        spend_the_exit_budget(&mut interpreter, &subject);
        let before_scavenges = interpreter.jit_statistics();
        interpreter.scavenge_now();
        interpreter.scavenge_now();
        let decayed = since(interpreter.jit_statistics(), before_scavenges);
        if let Err(message) = interpreter.verify_cells() {
            panic!("after the revival: {message}");
        }
        let before = interpreter.jit_statistics();
        assert_eq!(send_number(&mut interpreter, &subject, "callLeave:", 1), 20);
        let first = since(interpreter.jit_statistics(), before);
        let before = interpreter.jit_statistics();
        assert_eq!(send_number(&mut interpreter, &subject, "callLeave:", 1), 20);
        let second = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(decayed.revived_bodies, 1, "`leave:` is revived");
            // The miss refuses the first call, which counts nothing; the
            // interpreter's push of `leave:` counts its first entry, and the
            // loop's next backward jump switches back into `callLeave:`'s
            // machine code (`jit::at_back_edge`), whose next call through
            // the miss stub compiles `leave:` and calls it.
            assert!(
                first.back_edge_switches >= 1,
                "the rest of the run went back into machine code: {first:?}"
            );
            assert!(
                first.compiled_calls >= 1,
                "and called `leave:`'s machine code: {first:?}"
            );
            assert_eq!(second.send_exits, 0, "no call is refused: {second:?}");
            assert_eq!(second.exits, 0, "nothing leaves machine code");
            assert!(
                second.compiled_calls >= 10,
                "`callLeave:` calls `leave:` through its cell: {}",
                second.compiled_calls
            );
        }
    }
}

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    /// A body that exits on every run is abandoned, revived after two
    /// scavenges, compiled at its second entry and abandoned again: over
    /// twenty scavenges it is compiled at most once per two of them.
    fn a_body_that_always_exits_compiles_at_most_once_per_two_scavenges(lowering) {
        let (mut interpreter, class) = load_with(DECAYING, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        // Compiles `leave:` and the `doesNotUnderstand:` handler once, and
        // abandons `leave:`.
        spend_the_exit_budget(&mut interpreter, &subject);
        let before = interpreter.jit_statistics();
        for _ in 0..20 {
            interpreter.scavenge_now();
            spend_the_exit_budget(&mut interpreter, &subject);
        }
        let done = since(interpreter.jit_statistics(), before);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                done.compiled_bodies <= 11,
                "`leave:` compiled at most once per two scavenges: {}",
                done.compiled_bodies
            );
            assert_eq!(done.revived_bodies, 10, "revived at every second scavenge");
            assert_eq!(done.abandoned_bodies, 10, "and abandoned again");
        }
    }
}

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    /// A hot body that leaves machine code once in a thousand entries,
    /// two hundred thousand times with a scavenge after every thousand,
    /// never spends its exit budget: the scavenges halve its count, so that
    /// the budget bounds the exits of a stretch of the run, not of its
    /// lifetime, and the end of the run is in machine code.
    fn a_rare_exit_no_longer_ends_a_long_run_interpreted(lowering) {
        let (mut interpreter, class) = load_with(DECAYING, lowering);
        interpreter.set_jit_counting(true);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        // The long run only where its assertions are checked: the
        // reference lowering, whose body is never compiled, and a stress
        // mode, which collects or scavenges at every safepoint, check the
        // answers over a stretch of it.
        let measured = lowering == Lowering::Optimized && !a_stress_mode_is_on();
        let rounds = if measured { 200 } else { 10 };
        let before = interpreter.jit_statistics();
        for _ in 0..rounds {
            assert_eq!(send_number(&mut interpreter, &subject, "drive:", 999), 2001);
            interpreter.scavenge_now();
        }
        let long_run = since(interpreter.jit_statistics(), before);
        let before = interpreter.jit_statistics();
        assert_eq!(send_number(&mut interpreter, &subject, "drive:", -1), 2000);
        let last = since(interpreter.jit_statistics(), before);
        if measured {
            assert!(long_run.exits >= 200, "`leave:` left once per thousand");
            assert_eq!(long_run.abandoned_bodies, 0, "no body spent its budget");
            assert_eq!(last.exits, 0, "the last thousand entries stay in machine code");
            assert!(
                last.compiled_calls >= 1000,
                "`drive:` calls `leave:` through its cell: {}",
                last.compiled_calls
            );
        }
    }
}

/// `hot:`, which loops and then, when its argument is 0, sends `handles:`
/// to its class, whose mirror send leaves machine code: `handles:` exits
/// at every call until it is abandoned, and from then on its refused call
/// is an exit of `hot:`'s. The interpreter runs both without allocating,
/// so `hot: 0` exits and `hot: 1` does not, and neither allocates.
/// `run:exitEvery:` sends `hot:` `count` times from a loop, its argument 0
/// at every `period`th turn.
const ALLOCATION_FREE: &str = indoc! {"
    class Subject = Object new () (
      public hot: value = (
        |
          index ::= 0.
        |
        [index < 2] whileTrue: [
          index:: index + 1.
        ].
        ^value = 0
          ifTrue: [(self class handles: value) ifTrue: [0] ifFalse: [2]]
          ifFalse: [index]
      )
      public run: count exitEvery: period = (
        |
          sum ::= 0.
          index ::= 0.
          countdown ::= period.
        |
        [index < count] whileTrue: [
          countdown:: countdown - 1.
          sum:: sum + (self hot: (countdown = 0
            ifTrue: [
              countdown:: period.
              0
            ]
            ifFalse: [1])).
          index:: index + 1.
        ].
        ^sum
      )
    ) : ()
"};

/// Sends `run: count exitEvery: period` to `subject`, answering what the
/// JIT did meanwhile and whether a scavenge or a full collection ran.
fn run_allocation_free(
    interpreter: &mut Interpreter,
    subject: &Root,
    count: i64,
    period: i64,
) -> (JitStatistics, bool) {
    let collector = interpreter.collector_statistics();
    let before = interpreter.jit_statistics();
    let count_argument = interpreter.stash(Value::small_integer(count));
    let period_argument = interpreter.stash(Value::small_integer(period));
    let answer = expect_ok(
        interpreter.send(
            subject,
            Symbol::intern("run:exitEvery:"),
            &[count_argument, period_argument],
        ),
        "run:exitEvery:",
    );
    let answer = interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("run:exitEvery: expected a small integer"),
    });
    assert_eq!(answer, 2 * count);
    let done = since(interpreter.jit_statistics(), before);
    let after = interpreter.collector_statistics();
    let collected =
        after.scavenges != collector.scavenges || after.collections != collector.collections;
    (done, collected)
}

ns_runtime::for_each_lowering! {
    /// A run that allocates nothing never scavenges, so its exit counts
    /// never decay; a hot body that leaves machine code once in a thousand
    /// calls is abandoned at its sixty-fourth exit all the same, and
    /// revived once it has been entered interpreted often enough, so that
    /// a long run goes on calling its machine code. Its caller, entered
    /// once and compiled by on-stack replacement, spends its own exit
    /// budget on the refused calls while the callee is abandoned, and is
    /// abandoned for the rest of the test: the callee's machine code is
    /// then called from the interpreter through the call stub, which the
    /// last run's assertion admits beside a call through the caller's cell
    /// (`docs/OPEN-WORK.md`, "Known defects and gaps").
    fn a_rare_exit_without_allocation_does_not_end_a_long_run_interpreted(lowering) {
        let (mut interpreter, class) = load_with(ALLOCATION_FREE, lowering);
        interpreter.set_jit_counting(true);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let measured = lowering == Lowering::Optimized && !a_stress_mode_is_on();
        // `handles:` is compiled too, and leaves at its mirror send: the
        // first 64 rare exits are its own, and abandon it, and from then
        // on its refused call is `hot:`'s exit. `hot:` is abandoned at
        // about the 128,000th call and revived 4,096 interpreted calls
        // later; its next abandonment would come 64,000 calls after that.
        let count = if measured { 150_000 } else { 10_000 };
        let (long_run, collected) = run_allocation_free(&mut interpreter, &subject, count, 1_000);
        let (last, _) = run_allocation_free(&mut interpreter, &subject, 1_000, 2_000);
        if measured {
            assert!(!collected, "the run allocates nothing");
            assert!(
                long_run.abandoned_bodies >= 2,
                "`handles:` and `hot:` spent their budgets: {long_run:?}"
            );
            assert!(long_run.revived_bodies >= 1, "`hot:`'s entries revived it");
            assert_eq!(
                last.exits, 0,
                "the last thousand calls stay in machine code: {last:?}"
            );
            // `run:exitEvery:`, compiled at its loop's 40th backward jump,
            // spends its own budget on `hot:`'s refused calls while `hot:` is
            // abandoned, and is abandoned in turn: it calls `hot:`'s machine
            // code through its cell, or from the interpreter through the
            // call stub.
            assert!(
                last.compiled_calls + last.called_entries >= 1000,
                "every call of `hot:` enters its machine code: {last:?}"
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// Bodies that exit at every call, in a run that allocates nothing, are
    /// revived by their entries after a wait that doubles at each
    /// abandonment up to 65,536 entries, so that each is compiled at a rate
    /// that falls geometrically and then stays at once per 65,536 entries.
    fn an_always_exiting_body_recompiles_at_a_backed_off_rate(lowering) {
        let (mut interpreter, class) = load_with(ALLOCATION_FREE, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let measured = lowering == Lowering::Optimized && !a_stress_mode_is_on();
        let count = if measured { 300_000 } else { 10_000 };
        let (done, collected) = run_allocation_free(&mut interpreter, &subject, count, 1);
        if measured {
            assert!(!collected, "the run allocates nothing");
            // The waits are 4,096, 8,192, 16,384, 32,768 and then 65,536
            // entries each. Two bodies take turns: `handles:`, which leaves
            // at its mirror send, and `hot:`, whose call of `handles:` is
            // refused while that is abandoned.
            let mut most = 0u64;
            let mut waited = 0u64;
            let mut wait = 4096u64;
            while waited + wait <= count.unsigned_abs() {
                waited += wait;
                most += 1;
                wait = (wait * 2).min(65_536);
            }
            assert!(
                (3..=2 * most).contains(&done.revived_bodies),
                "revived {} times in {count} entries, at most {most} each: {done:?}",
                done.revived_bodies
            );
            assert!(
                done.compiled_bodies <= done.revived_bodies + 3,
                "one compile per revival: {} compiles",
                done.compiled_bodies
            );
            // The tens of thousands of interpreted pushes that waited are
            // counted inline: only the push that revives a body leaves the
            // run loop for it.
            assert!(
                (1..=done.revived_bodies).contains(&done.abandoned_entries_out_of_line),
                "{} pushes of an abandoned body out of line, {} revivals",
                done.abandoned_entries_out_of_line,
                done.revived_bodies
            );
        }
    }
}

/// `Thing`, whose factory makes its instance and runs its initializer,
/// whose slot's message its class answers in its own
/// `doesNotUnderstand:`, which machine code leaves to the interpreter;
/// `make`, which sends `Thing new`; and `makeAgain:`, which sends it from
/// a site of its own only when its argument is true.
const LEAVING_FACTORY: &str = indoc! {"
    class Subject = Object new () (
      class Thing = (
        |
          value = self missing.
        |
      ) (
        public doesNotUnderstand: message = (
          ^1
        )
      ) : ()
      public make = (
        ^Thing new
      )
      public makeAgain: making = (
        making ifTrue: [^Thing new].
        ^nil
      )
    ) : ()
"};
ns_runtime::for_each_lowering! {
    /// A body revived by an entry the miss leaf counts -- a factory's,
    /// which only a miss counts -- has every cell that refused its calls
    /// written afresh, not only the site that missed. The initializer
    /// leaves at its slot's message and is abandoned first; the factory's
    /// call of it is refused from then on, an exit of the factory's, until
    /// the factory is abandoned too, its wait lowered to one entry, and
    /// `make`'s cell names the refusing stub. `makeAgain:`'s site, never
    /// run before its body is compiled, misses: that entry revives the
    /// factory, and `make`'s cell must name the miss stub again.
    fn a_revival_by_a_miss_writes_every_cell_afresh(lowering) {
        let (mut interpreter, class) = load_with(LEAVING_FACTORY, lowering);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let start = interpreter.jit_statistics();
        let abandoned = |interpreter: &Interpreter| {
            since(interpreter.jit_statistics(), start).abandoned_bodies
        };
        let make_again = |interpreter: &mut Interpreter, making: bool| {
            let argument = interpreter.stash(Value::boolean(making));
            expect_ok(
                interpreter.send(&subject, Symbol::intern("makeAgain:"), &[argument]),
                "makeAgain:",
            );
        };
        let make = |interpreter: &mut Interpreter| {
            expect_ok(interpreter.send(&subject, Symbol::intern("make"), &[]), "make");
        };
        let mut sends = 0;
        while abandoned(&interpreter) < 1 && sends < 500 {
            make(&mut interpreter);
            sends += 1;
        }
        interpreter.set_jit_revival_entries(1);
        while abandoned(&interpreter) < 2 && sends < 1_000 {
            make(&mut interpreter);
            sends += 1;
        }
        // Compiled at its second entry, its site not yet run.
        for _ in 0..2 {
            make_again(&mut interpreter, false);
        }
        let before = interpreter.jit_statistics();
        make_again(&mut interpreter, true);
        let revived = since(interpreter.jit_statistics(), before);
        if let Err(message) = interpreter.verify_cells() {
            panic!("after the revival: {message}");
        }
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(abandoned(&interpreter), 2, "the initializer, then the factory");
            assert_eq!(revived.revived_bodies, 1, "`makeAgain:`'s miss revived the factory");
            assert!(revived.miss_leaf_calls >= 1, "counted by the miss leaf");
        }
    }
}

/// A module whose `Victim>>answer` is a family of a thousand sites, run
/// first so that its range lies below every other, and whose `editVictim`
/// replaces it through a MirrorBuilder, freeing that range at the next
/// full collection; `run` loops and sends, a family above it.
#[cfg(debug_assertions)]
fn moved_family_source() -> String {
    let victim = format!("^self{}", " yourself".repeat(1_000));
    formatdoc! {"
        class Subject usingPlatform: platform = Object new (
          |
            ClassMirror = platform mirrors ClassMirror.
          |
        ) (
          class Victim = Object new () (
            public answer = ( {victim} )
          )
          public victim = ( ^Victim new answer )
          public editVictim = (
            | builder |
            builder:: (ClassMirror reflecting: Victim) mixin declaration asBuilder.
            builder instanceSide methods addFromSource: 'public answer = ( ^0 )'.
            builder install.
          )
          public run = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < 10] whileTrue: [
              sum:: sum + index yourself.
              index:: index + 1.
            ].
            ^sum
          )
        ) : ()
    "}
}

ns_runtime::for_each_lowering! {
    /// The machine code of a family a slide moved is freed and compiled
    /// again: `run`'s body was compiled against its family's base, which
    /// the collection after the victim's edit moves down over the victim's
    /// freed range, so `freed_bodies` rises at the collection and the body
    /// is compiled against the new base at its second entry after it, and
    /// runs in machine code from then on. `collect_now` is a debug build's.
    #[cfg(debug_assertions)]
    fn machine_code_of_a_moved_family_compiles_again(lowering) {
        let (mut interpreter, class) = load_with(&moved_family_source(), lowering);
        let platform = interpreter.platform();
        let subject = expect_ok(
            interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
            "usingPlatform:",
        );
        expect_ok(
            interpreter.send(&subject, Symbol::intern("victim"), &[]),
            "victim",
        );
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), 45);
        }
        let run = Symbol::intern("run");
        let compiled = interpreter
            .jit_body_offset(&class, MethodSide::Instance, run)
            .is_some();
        // Counted from before the edit: the collect stress mode may slide
        // the ranges inside it, before `collect_now`.
        let slides = interpreter.cache_statistics().site_slides;
        let before = interpreter.jit_statistics();
        expect_ok(
            interpreter.send(&subject, Symbol::intern("editVictim"), &[]),
            "editVictim",
        );
        interpreter.collect_now();
        let collected = since(interpreter.jit_statistics(), before);
        let slid = interpreter.cache_statistics().site_slides > slides;
        let freed_at_once = interpreter
            .jit_body_offset(&class, MethodSide::Instance, run)
            .is_none();
        for _ in 0..2 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), 45);
        }
        let recompiled = interpreter
            .jit_body_offset(&class, MethodSide::Instance, run)
            .is_some();
        let before = interpreter.jit_statistics();
        assert_eq!(send_unary(&mut interpreter, &subject, "run"), 45);
        let third = since(interpreter.jit_statistics(), before);
        if let Err(message) = interpreter.verify_cells() {
            panic!("after the slide: {message}");
        }
        // The reference lowering's sends run uncached, so its families take
        // no range and nothing slides; its body exits at once. The zone
        // stress mode frees and compiles bodies at its own safepoints.
        if lowering == Lowering::Optimized && !the_zone_stress_mode_is_on() {
            assert!(compiled, "`run` was compiled");
            assert!(slid, "the collection slid the ranges");
            assert!(collected.freed_bodies >= 1, "the moved family's code is freed");
            assert!(freed_at_once, "no machine code is left for `run`");
            assert!(recompiled, "`run` is compiled again at its second entry");
            assert!(third.machine_entries > 0, "`run` runs in machine code again");
        }
    }
}

/// `count` blocks, each of its own body, kept in `kept` so that their code
/// ids stay named across collections, and `callBlock:`, which makes a
/// block of a body of its own, sends it `value:` and lets it go, so that
/// a collection frees its id.
#[cfg(debug_assertions)]
fn many_blocks(count: usize) -> String {
    let blocks: String = (0..count).map(|index| format!("[{index}]. ")).collect();
    formatdoc! {"
        class Subject = Object new (
          |
            kept ::= nil.
            held ::= nil.
          |
        ) (
          public keepAll = ( kept:: {{ {blocks}}}. ^kept size )
          public callBlock: value = (
            | answer |
            held:: [:number | number + 1].
            answer:: held value: value.
            held:: nil.
            ^answer
          )
        ) : ()
    "}
}

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    /// After a full collection lowered the machine entry table's committed
    /// rows to the code table's length -- the shrink pair at nothing, so
    /// that every page past it goes -- the code ids of two hundred kept
    /// blocks take the table past the lowered bound, and the block
    /// `callBlock:` makes takes an id past it: machine code's send of
    /// `value:` compares that id with the bound
    /// (`MachineWord::MachineEntryRows`), which was lowered before the
    /// pages went, finds no row, and leaves the call to the interpreter,
    /// which answers right; a read past the bound would fault on a page
    /// given back.
    fn a_machine_entry_row_past_the_lowered_bound_reads_as_none(lowering) {
        let (mut interpreter, class) = load_with(&many_blocks(200), lowering);
        interpreter.set_shrink_pairs(ns_runtime::ShrinkPairs {
            machine_entries: ns_runtime::Shrink {
                threshold: 0,
                headroom: 0,
            },
            // The band's end, not the damping's ramp, is what this pins.
            damped: false,
            ..ns_runtime::ShrinkPairs::default()
        });
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for value in 0..4 {
            assert_eq!(send_number(&mut interpreter, &subject, "callBlock:", value), value + 1);
        }
        let committed = interpreter.table_sizes().machine_entry_rows_committed;
        // The first collection sees the rows' growth, demand, and keeps
        // them; the next, after none, lowers them.
        interpreter.collect_now();
        interpreter.collect_now();
        let lowered = interpreter.table_sizes().machine_entry_rows_committed;
        assert_eq!(send_unary(&mut interpreter, &subject, "keepAll"), 200);
        let code_ids = interpreter.table_sizes().code_ids;
        for value in 10..14 {
            assert_eq!(send_number(&mut interpreter, &subject, "callBlock:", value), value + 1);
        }
        // The reference lowering's bodies exit at once and record no row;
        // a stress mode's collections free and mint the ids meanwhile.
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                lowered < committed,
                "the collection lowered the rows from {committed} to {lowered}"
            );
            assert!(
                code_ids > lowered,
                "{code_ids} code ids past {lowered} committed rows"
            );
        }
    }
}

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    /// Six hundred edits of a hot method fill the code zone with bodies
    /// that die with their versions; a full collection frees them, and a
    /// compaction slides the survivors down and gives the zone's pages
    /// back past their end and the headroom, the code zone's shrink pair
    /// lowered to 64 KiB and 16 KiB (`heap::limits::Shrink`). The next
    /// compile commits them again, and every run answers right.
    fn the_zone_gives_back_its_pages_after_a_compaction(lowering) {
        let threshold = 64 << 10;
        let headroom = 16 << 10;
        // The zone commits and gives back in its own unit: a page, or a
        // chunk of 1 MiB on Windows.
        let page = (64 << 10).max(ns_memory::CodeZone::commit_granularity());
        let (mut interpreter, subject, _) = edited_answer(lowering);
        interpreter.set_shrink_pairs(ns_runtime::ShrinkPairs {
            code_zone: ns_runtime::Shrink {
                threshold,
                headroom,
            },
            // The band's end, not the damping's ramp, is what this pins.
            damped: false,
            ..ns_runtime::ShrinkPairs::default()
        });
        let edits = 600;
        for number in 1..=edits {
            assert_eq!(send_number(&mut interpreter, &subject, "edit:", number), number);
            for _ in 0..3 {
                assert_eq!(send_unary(&mut interpreter, &subject, "run"), 3 + number);
            }
        }
        interpreter.collect_now();
        // After the collection, which frees the dead bodies but moves
        // none: only the compaction lowers the zone's next offset.
        let filled = interpreter.table_sizes().zone_committed_bytes;
        let before = interpreter.collector_statistics();
        interpreter.compact_jit_zone();
        let sizes = interpreter.table_sizes();
        let used = sizes.zone_used_bytes;
        // Shrunk, or within the threshold, or within one doubling of
        // what a shrink would keep (`heap::limits::Shrink`).
        assert!(
            sizes.zone_committed_bytes <= (used + threshold).max(2 * (used + headroom)) + 2 * page,
            "{} bytes committed past {used} used",
            sizes.zone_committed_bytes
        );
        let after = interpreter.collector_statistics();
        for _ in 0..3 {
            assert_eq!(send_unary(&mut interpreter, &subject, "run"), 3 + edits);
        }
        // The reference lowering's bodies exit at once and are never
        // installed; a stress mode compacts and collects on its own.
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                sizes.zone_committed_bytes <= used + headroom + page,
                "{} bytes committed past {used} used, {filled} before",
                sizes.zone_committed_bytes
            );
            assert!(sizes.zone_committed_bytes < filled, "the zone shrank");
            assert!(after.decommits > before.decommits, "the compaction decommitted");
        }
    }
}

/// A summing loop of 100,000 turns in `run`, which the host sends once:
/// its body is never entered a second time, so only its backward jumps can
/// bring its loop into machine code.
const LOOP_ENTERED_ONCE: &str = indoc! {"
    class Subject = Object new () (
      public run = (
        |
          sum ::= 0.
          index ::= 1.
        |
        [index <= 100000] whileTrue: [
          sum:: sum + index.
          index:: index + 1.
        ].
        ^sum.
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// On-stack replacement: `run`'s interpreted loop counts its backward
    /// jumps, the 40th compiles the body and the frame switches into its
    /// machine code at the loop's head, where the rest of the loop runs.
    /// The reference lowering sends `whileTrue:`, so its loop has no
    /// backward jump.
    fn a_loop_in_a_method_entered_once_runs_in_machine_code(lowering) {
        let (mut interpreter, class) = load_with(LOOP_ENTERED_ONCE, lowering);
        interpreter.set_jit_counting(true);
        let (answer, done) = run_measured(&mut interpreter, &class);
        assert_eq!(answer, 5_000_050_000);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(done.back_edge_compiles, 1, "the 40th backward jump compiles `run`");
            assert_eq!(done.back_edge_switches, 1, "and the frame switches once");
            assert!(
                done.machine_back_edges >= 99_000,
                "the rest of the loop closes in machine code: {}",
                done.machine_back_edges
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A body compiled while one of its frames runs interpreted: the
    /// outer activation of `run:` sends itself once before its loop, and
    /// that inner activation, the body's second entry, compiles it. The
    /// outer frame, interpreted, switches into that machine code at its
    /// loop's first backward jump, counting nothing and compiling nothing.
    fn a_body_compiled_meanwhile_switches_at_its_next_back_jump(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              public run: depth = (
                |
                  sum ::= 0.
                  index ::= 1.
                |
                depth > 0 ifTrue: [sum:: self run: depth - 1].
                [index <= 1000] whileTrue: [
                  sum:: sum + index.
                  index:: index + 1.
                ].
                ^sum.
              )
              public run = (
                ^self run: 1
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        let (answer, done) = run_measured(&mut interpreter, &class);
        assert_eq!(answer, 1_001_000);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(
                done.back_edge_switches >= 1,
                "the outer frame switches: {}",
                done.back_edge_switches
            );
            assert_eq!(done.back_edge_compiles, 0, "the inner entry compiled the body");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A loop whose every turn leaves machine code -- a send its receiver
    /// answers in its `doesNotUnderstand:`, which machine code hands to the
    /// interpreter -- switches back at each backward jump only until its
    /// body has spent its exit budget: the JIT then abandons it, and the
    /// rest of the 10,000 turns run interpreted, not an exit and an entry
    /// each.
    fn a_loop_that_always_exits_is_abandoned_not_reentered(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              public doesNotUnderstand: message = (
                ^1
              )
              public run = (
                |
                  sum ::= 0.
                  index ::= 0.
                |
                [index < 10000] whileTrue: [
                  sum:: sum + self one.
                  index:: index + 1.
                ].
                ^sum.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        let (answer, done) = run_measured(&mut interpreter, &class);
        assert_eq!(answer, 10_000);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(done.abandoned_bodies >= 1, "`run` is abandoned: {done:?}");
            assert!(
                done.back_edge_switches <= 2 * 64,
                "switches bounded by the exit budget: {}",
                done.back_edge_switches
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A frame the interpreter reified -- its loop makes a block that
    /// reads `index` on every turn -- switches into the body's twin, which
    /// stores its sends' program counters and divorces it at its return.
    fn a_reified_frame_switches_into_the_twin(lowering) {
        let source = indoc! {"
            class Subject = Object new (
              |
                last ::= nil.
              |
            ) (
              public run = (
                |
                  sum ::= 0.
                  index ::= 0.
                |
                [index < 10000] whileTrue: [
                  last:: [index].
                  sum:: sum + index.
                  index:: index + 1.
                ].
                ^sum + last value.
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        let (answer, done) = run_measured(&mut interpreter, &class);
        assert_eq!(answer, 50_005_000);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(done.compiled_twins >= 1, "the twin is compiled: {done:?}");
            assert!(done.back_edge_switches >= 1, "the frame switches: {done:?}");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A block's loop switches as a method's does: the block is entered
    /// once, its 40th backward jump compiles it, records its machine entry
    /// row and switches.
    fn a_block_with_a_loop_entered_once_switches(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              public run = (
                ^[
                  |
                    sum ::= 0.
                    index ::= 1.
                  |
                  [index <= 100000] whileTrue: [
                    sum:: sum + index.
                    index:: index + 1.
                  ].
                  sum
                ] value
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        let (answer, done) = run_measured(&mut interpreter, &class);
        assert_eq!(answer, 5_000_050_000);
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert_eq!(done.back_edge_switches, 1, "{done:?}");
            assert_eq!(done.back_edge_compiles, 1, "{done:?}");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// While the JIT is off a backward jump neither compiles nor switches:
    /// its body is refused, as a push refuses it.
    fn no_back_edge_switches_while_the_jit_is_off(lowering) {
        let (mut interpreter, class) = load_with(LOOP_ENTERED_ONCE, lowering);
        interpreter.set_jit_enabled(false);
        let (answer, done) = run_measured(&mut interpreter, &class);
        assert_eq!(answer, 5_000_050_000);
        assert_eq!(done.back_edge_switches, 0);
        assert_eq!(done.back_edge_compiles, 0);
    }
}

/// `chain_of`'s recursion through `count` bodies, each looping 50 turns
/// before it sends the next, so that its 40th backward jump compiles it,
/// and the last sending `loop`, a loop of 10,000 turns: the first few
/// bodies fill a small zone with code that stays active below the rest,
/// so that every later body, `loop` among them, finds no room to compile
/// into.
fn chain_ending_in_a_loop(count: usize) -> String {
    let methods: String = (0..count)
        .map(|index| {
            let next = if index + 1 == count {
                "self loop".to_string()
            } else {
                format!("(self m{}) + 1", index + 1)
            };
            formatdoc! {"
                  m{index} = (
                    |
                      index ::= 0.
                    |
                    [index < 50] whileTrue: [
                      index:: index + 1.
                    ].
                    ^{next}
                  )
            "}
        })
        .collect();
    formatdoc! {"
        class Subject = Object new () (
        {methods}  loop = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < 10000] whileTrue: [
              sum:: sum + 1.
              index:: index + 1.
            ].
            ^sum
          )
          public run = (
            ^self m0
          )
        ) : ()
    "}
}

ns_runtime::for_each_lowering! {
    /// A loop whose body cannot switch into machine code -- the zone, full
    /// of the active code of the chain below it, has no room to compile
    /// it into -- counts its backward jumps out of line and tries again
    /// only every 40th, and ends the run loop's stretch only to switch:
    /// never at every turn of its 10,000.
    fn a_loop_that_cannot_switch_never_leaves_its_stretch_at_every_turn(lowering) {
        let count = 300;
        let source = chain_ending_in_a_loop(count);
        let (mut interpreter, class) = load_with(&source, lowering);
        interpreter.set_jit_zone_bytes(16_384);
        let expected = 10_000 + count as i64 - 1;
        let before = interpreter.jit_statistics();
        for _ in 0..3 {
            assert_eq!(run(&mut interpreter, &class), expected);
        }
        let done = since(interpreter.jit_statistics(), before);
        assert_eq!(
            done.back_edge_stretch_exits + done.safepoint_resumptions,
            done.back_edge_switches,
            "every stretch that ended at a backward jump switched: {done:?}"
        );
        if lowering == Lowering::Optimized && !a_stress_mode_is_on() {
            assert!(done.zone_compactions >= 1, "the zone filled: {done:?}");
            assert!(
                done.back_edge_compiles < 10,
                "the zone holds a few of the bodies: {done:?}"
            );
            assert!(
                done.back_edge_stretch_exits <= 10,
                "not at every turn of the 30,000: {done:?}"
            );
        }
    }
}

/// Three methods `warm` runs a thousand times on `SmallInteger`s and
/// Booleans alone, so that each is optimized with its fast paths'
/// fallbacks and its conditional's non-Boolean way never run; each of
/// the others then gives one of them, once, what it never saw.
const SPECULATED: &str = indoc! {"
    class Subject = Object new () (
      class Maybe = () (
        public ifTrue: trueBlock ifFalse: falseBlock = (
          ^3
        )
      )
      total: step = (
        |
          sum ::= 0.
          index ::= 1.
        |
        [index <= 100] whileTrue: [
          sum:: sum + step.
          index:: index + 1.
        ].
        ^sum
      )
      double: number = (
        ^number + number
      )
      choose: flag = (
        ^flag ifTrue: [1] ifFalse: [2]
      )
      public warm = (
        |
          sum ::= 0.
          index ::= 1.
        |
        [index <= 1000] whileTrue: [
          sum:: sum + (self total: 1) + (self double: index) + (self choose: index < 500).
          index:: index + 1.
        ].
        ^sum
      )
      public withFloat = (
        ^((self total: 0.5) * 2) asInteger
      )
      public withOverflow = (
        | big |
        big:: 1152921504606846975.
        ^(self double: big) - big - big + 7
      )
      public withNonBoolean = (
        ^self choose: Maybe new
      )
    ) : ()
"};

/// The budget `SPECULATED`'s methods are armed with: each runs out
/// within `warm`'s thousand calls.
const SPECULATING_BUDGET: i64 = 1_000;

/// `SPECULATED` warmed up under `lowering`, then sent `selector`:
/// its answer, and what the JIT did while it ran.
fn after_warm_up(lowering: Lowering, selector: &str) -> (i64, JitStatistics) {
    let (mut interpreter, class) = load_spending_with(SPECULATED, lowering);
    interpreter.set_optimization_budget(SPECULATING_BUDGET);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    // 100 for `total: 1`, twice the index for `double:`, and 1 or 2 for
    // `choose:`, over a thousand indices.
    assert_eq!(send_unary(&mut interpreter, &subject, "warm"), 1_102_501);
    let before = interpreter.jit_statistics();
    let answer = send_unary(&mut interpreter, &subject, selector);
    (answer, since(interpreter.jit_statistics(), before))
}

/// That `statistics` counts one trap where the tiers are pinned, and
/// none under the reference lowering, whose sends run uncached and so
/// record no site to speculate on.
fn assert_trapped_once(statistics: &JitStatistics, lowering: Lowering) {
    if pins_the_tiers(lowering) {
        assert_eq!(statistics.traps, 1, "{statistics:?}");
        assert_eq!(
            statistics.abandoned_optimized_bodies, 0,
            "a trap counts against no exit budget"
        );
    } else if lowering == Lowering::Reference {
        assert_eq!(statistics.traps, 0, "{statistics:?}");
    }
}

ns_runtime::for_each_lowering! {
    /// `total:`'s sum, optimized on `SmallInteger`s, given a `Float`:
    /// the fast path's failure traps, and the interpreter, running the
    /// operation again, sends it, so that the sum is right.
    fn a_float_after_warm_up_traps_once_and_answers_right(lowering) {
        let (answer, statistics) = after_warm_up(lowering, "withFloat");
        assert_eq!(answer, 100);
        assert_trapped_once(&statistics, lowering);
    }
}

ns_runtime::for_each_lowering! {
    /// `double:`, optimized on `SmallInteger`s that never overflowed,
    /// given the largest: the overflow traps, and the interpreter's
    /// send answers the `LargeInteger`.
    fn an_overflow_after_warm_up_traps_once_and_answers_right(lowering) {
        let (answer, statistics) = after_warm_up(lowering, "withOverflow");
        assert_eq!(answer, 7);
        assert_trapped_once(&statistics, lowering);
    }
}

ns_runtime::for_each_lowering! {
    /// `choose:`, optimized on Booleans, given an object with an
    /// `ifTrue:ifFalse:` of its own: the conditional traps, and the
    /// interpreter sends it.
    fn a_non_boolean_after_warm_up_traps_once_and_answers_right(lowering) {
        let (answer, statistics) = after_warm_up(lowering, "withNonBoolean");
        assert_eq!(answer, 3);
        assert_trapped_once(&statistics, lowering);
    }
}

/// `over:and:and:and:and:and:` sums each of its six arguments a hundred
/// times into an accumulator of its own, so that each sum is a site of
/// its own; `phase:` calls it fifty times with every argument 1 but the
/// one `phase` names, which is 0.5. Phase 0 gives none a `Float`.
const ONE_FLOAT_A_PHASE: &str = indoc! {"
    class Subject = Object new () (
      over: first and: second and: third and: fourth and: fifth and: sixth = (
        |
          one ::= 0.
          two ::= 0.
          three ::= 0.
          four ::= 0.
          five ::= 0.
          six ::= 0.
          index ::= 1.
        |
        [index <= 100] whileTrue: [
          one:: one + first.
          two:: two + second.
          three:: three + third.
          four:: four + fourth.
          five:: five + fifth.
          six:: six + sixth.
          index:: index + 1.
        ].
        ^one asInteger + two asInteger + three asInteger + four asInteger
          + five asInteger + six asInteger
      )
      argument: position in: phase = (
        ^position = phase ifTrue: [0.5] ifFalse: [1]
      )
      public phase: phase = (
        |
          total ::= 0.
          index ::= 1.
        |
        [index <= 50] whileTrue: [
          total:: total
            + (self
              over: (self argument: 1 in: phase)
              and: (self argument: 2 in: phase)
              and: (self argument: 3 in: phase)
              and: (self argument: 4 in: phase)
              and: (self argument: 5 in: phase)
              and: (self argument: 6 in: phase)).
          index:: index + 1.
        ].
        ^total
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// Each phase of `ONE_FLOAT_A_PHASE` after the first feeds a `Float`
    /// to one more of `over:and:and:and:and:and:`'s sums, each long
    /// enough for the copy made not entrant to be compiled again: each
    /// phase's site traps twice and makes the copy not entrant, until the
    /// copy past the last version, compiled without a trap, takes the
    /// remaining phases as they come. The bound holds whatever the stress
    /// mode; where the tiers are pinned it is reached.
    fn alternating_types_settle_within_the_last_version(lowering) {
        let (mut interpreter, class) = load_spending_with(ONE_FLOAT_A_PHASE, lowering);
        interpreter.set_optimization_budget(SPECULATING_BUDGET);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let before = interpreter.jit_statistics();
        for phase in 0..=6 {
            // Every sum is 100 but the one fed 0.5, which is 50.
            let expected = if phase == 0 { 50 * 600 } else { 50 * 550 };
            for round in 0..10 {
                assert_eq!(
                    send_number(&mut interpreter, &subject, "phase:", phase),
                    expected,
                    "phase {phase}, round {round}"
                );
            }
        }
        let statistics = since(interpreter.jit_statistics(), before);
        let last = u64::from(ns_runtime::LAST_VERSION);
        assert!(statistics.made_not_entrant <= last + 1, "{statistics:?}");
        assert_eq!(statistics.refused_for_good, 0, "{statistics:?}");
        if pins_the_tiers(lowering) {
            assert_eq!(statistics.made_not_entrant, last + 1, "{statistics:?}");
            assert_eq!(statistics.traps, 2 * (last + 1), "{statistics:?}");
            assert_eq!(statistics.abandoned_optimized_bodies, 0, "{statistics:?}");
        }
    }
}

/// `step:at:` sends `probe:` to a `Stranger`, which answers it in its
/// own `doesNotUnderstand:`, whose handler machine code leaves to the
/// interpreter: an exit of its optimized code at every call, and of its
/// baseline code too. The site ran, so its send is no trap.
const ALWAYS_EXITING: &str = indoc! {"
    class Subject = Object new (
      | stranger = Stranger new. |
    ) (
      class Stranger = () (
        public doesNotUnderstand: message = (
          ^1
        )
      )
      step: target at: value = (
        ^target probe: value
      )
      public run: count = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < count] whileTrue: [
          sum:: sum + (self step: stranger at: index).
          index:: index + 1.
        ].
        ^sum
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// `step:at:`'s optimized code, compiled again after each
    /// abandonment for its exits, is abandoned at every version up to one
    /// past the last, which leaves its key in baseline code for good: a
    /// second run enters no optimized code of it, and so leaves none.
    fn a_copy_abandoned_past_its_last_version_stays_in_baseline_code(lowering) {
        let (mut interpreter, class) = load_spending_with(ALWAYS_EXITING, lowering);
        interpreter.set_optimization_budget(100);
        interpreter.set_jit_revival_entries(64);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let before = interpreter.jit_statistics();
        assert_eq!(send_number(&mut interpreter, &subject, "run:", 40_000), 40_000);
        let statistics = since(interpreter.jit_statistics(), before);
        assert!(statistics.refused_for_good <= 1, "{statistics:?}");
        let before = interpreter.jit_statistics();
        assert_eq!(send_number(&mut interpreter, &subject, "run:", 1_000), 1_000);
        let again = since(interpreter.jit_statistics(), before);
        if pins_the_tiers(lowering) {
            let last = u64::from(ns_runtime::LAST_VERSION);
            assert_eq!(statistics.refused_for_good, 1, "{statistics:?}");
            assert_eq!(
                statistics.abandoned_optimized_bodies,
                last + 2,
                "{statistics:?}"
            );
            assert_eq!(statistics.traps, 0, "{statistics:?}");
            assert_eq!(again.optimized_exits, 0, "{again:?}");
            assert_eq!(again.refused_for_good, 0, "{again:?}");
        }
    }
}

/// `step:with:probing:`, a method of `A`, sums `number` twenty times
/// and adds what `target` answers `probe: value`: on an `A` it is given a `Stranger`,
/// which answers in its own `doesNotUnderstand:`, an exit at every call;
/// on a `B`, a subclass of `A`, a `Prober`, which answers 1 with no
/// exit, `value` itself: a `Float` sum only where `runBFloat:` gives
/// one, and a `Float` answer only where `probeB:` gives one. `quietA:`
/// gives an `A` the `Prober` too, so that its frames run its baseline
/// code with no exit, spending the body's budget.
const ONE_KEY_REFUSED: &str = indoc! {"
    class Subject = Object new (
      |
        stranger = Stranger new.
        prober = Prober new.
        a = A new.
        b = B new.
      |
    ) (
      class Stranger = () (
        public doesNotUnderstand: message = (
          ^1
        )
      )
      class Prober = () (
        public probe: value = (
          ^value
        )
      )
      class A = () (
        public step: target with: number probing: value = (
          |
            sum ::= 0.
            index ::= 1.
          |
          [index <= 20] whileTrue: [
            sum:: sum + number.
            index:: index + 1.
          ].
          ^(sum asInteger + (target probe: value)) asInteger
        )
      )
      class B = A () ()
      public runA: count = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < count] whileTrue: [
          sum:: sum + (a step: stranger with: 1 probing: 1).
          index:: index + 1.
        ].
        ^sum
      )
      public runB: count = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < count] whileTrue: [
          sum:: sum + (b step: prober with: 1 probing: 1).
          index:: index + 1.
        ].
        ^sum
      )
      public runBFloat: count = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < count] whileTrue: [
          sum:: sum + (b step: prober with: 0.5 probing: 1).
          index:: index + 1.
        ].
        ^sum
      )
      public probeB: count = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < count] whileTrue: [
          sum:: sum + (b step: prober with: 1 probing: 0.5).
          index:: index + 1.
        ].
        ^sum
      )
      public quietA: count = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < count] whileTrue: [
          sum:: sum + (a step: prober with: 1 probing: 1).
          index:: index + 1.
        ].
        ^sum
      )
      public mixed: count = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < count] whileTrue: [
          sum:: sum + (self quietA: 10) + (b step: prober with: 1 probing: 1).
          index:: index + 1.
        ].
        ^sum
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// `step:with:probing:`'s copy for an `A` is refused for good, and
    /// its copy for a `B`, made not entrant by a trap at its sum
    /// afterwards, is compiled again while the frames on an `A`, ten for
    /// each on a `B` and none exiting, spend most of the body's budget: a
    /// run-out of the refused pair's leaves the budget armed for the
    /// other key's recompile, never parked. The recompiled copy is there
    /// to trap at the probe's sum, which it still speculates on.
    fn a_key_refused_for_good_does_not_starve_another_key(lowering) {
        let (mut interpreter, class) = load_spending_with(ONE_KEY_REFUSED, lowering);
        interpreter.set_optimization_budget(100);
        interpreter.set_jit_revival_entries(64);
        let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let start = interpreter.jit_statistics();
        assert_eq!(send_number(&mut interpreter, &subject, "runA:", 40_000), 40_000 * 21);
        let refused = since(interpreter.jit_statistics(), start);
        assert_eq!(send_number(&mut interpreter, &subject, "runB:", 2_000), 2_000 * 21);
        assert_eq!(send_number(&mut interpreter, &subject, "runBFloat:", 3), 3 * 11);
        let retired = since(interpreter.jit_statistics(), start);
        assert_eq!(
            send_number(&mut interpreter, &subject, "mixed:", 5_000),
            5_000 * (10 * 21 + 21)
        );
        let before = interpreter.jit_statistics();
        assert_eq!(send_number(&mut interpreter, &subject, "probeB:", 3), 3 * 20);
        let probed = since(interpreter.jit_statistics(), before);
        let after = since(interpreter.jit_statistics(), start);
        if pins_the_tiers(lowering) {
            assert_eq!(refused.refused_for_good, 1, "{refused:?}");
            assert_eq!(retired.made_not_entrant, 1, "{retired:?}");
            assert!(probed.traps >= 1, "the B copy compiled again: {probed:?}");
            assert_eq!(after.refused_for_good, 1, "{after:?}");
        }
    }
}

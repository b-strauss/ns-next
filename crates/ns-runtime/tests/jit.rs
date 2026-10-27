//! The baseline JIT's compile trigger and its state per interpreter
//! (`docs/design/execution.md`, "Planned: the baseline JIT", "When a body
//! is compiled"). What a compiled body computes is the goldens' to pin,
//! under both lowerings; these pin when a body is compiled, whose zone it
//! runs from, and what happens when the zone is full.
#![cfg(target_arch = "x86_64")]

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
        bodies_exiting_at_entry: after.bodies_exiting_at_entry - before.bodies_exiting_at_entry,
        machine_back_edges: after.machine_back_edges - before.machine_back_edges,
        safepoint_exits: after.safepoint_exits - before.safepoint_exits,
        machine_entries: after.machine_entries - before.machine_entries,
    }
}

/// The bodies the JIT compiled or found exiting at entry: those it took
/// up at their second entry.
fn taken_up(statistics: JitStatistics) -> u64 {
    statistics.compiled_bodies + statistics.bodies_exiting_at_entry
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
    /// entry and enters machine code that exits at once, so that a program
    /// runs the entry stub, the exit and the resumption on every push and
    /// still answers right.
    fn the_stress_setting_enters_machine_code_on_every_push(lowering) {
        let (mut interpreter, class) = load_with(THREE_ANSWERS, lowering);
        interpreter.set_jit_stress(true);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 42);
        let done = since(interpreter.jit_statistics(), before);
        assert_eq!(
            done.machine_entries, 5,
            "`answer`, entered three times, runs its machine code from the first, and so do \
             the two base activations the host pushes, the factory's and `run`'s"
        );
        assert_eq!(done.bodies_exiting_at_entry, 0, "installed, not left out");
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
    /// starting with a template, fills the zone, and the bodies refused
    /// once it is full run interpreted with the output unchanged.
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

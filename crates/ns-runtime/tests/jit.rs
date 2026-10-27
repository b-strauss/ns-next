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
        machine_back_edges: after.machine_back_edges - before.machine_back_edges,
        safepoint_exits: after.safepoint_exits - before.safepoint_exits,
    }
}

ns_runtime::for_each_lowering! {
    fn a_body_is_compiled_at_its_second_entry(lowering) {
        let (mut interpreter, class) = load_with(THREE_ANSWERS, lowering);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 42);
        let compiled = since(interpreter.jit_statistics(), before);
        assert!(
            compiled.compiled_bodies >= 1,
            "`answer`, entered three times, is compiled"
        );
        assert_eq!(compiled.refused_bodies, 0);

        let (mut interpreter, class) = load_with(THREE_ANSWERS, lowering);
        interpreter.set_jit_enabled(false);
        let before = interpreter.jit_statistics();
        assert_eq!(run(&mut interpreter, &class), 42);
        let compiled = since(interpreter.jit_statistics(), before);
        assert_eq!(compiled.compiled_bodies, 0, "the JIT is off");
        assert_eq!(compiled.refused_bodies, 0, "the JIT is off");
    }
}

ns_runtime::for_each_lowering! {
    fn a_second_interpreter_does_not_run_the_first_ones_code(lowering) {
        let (mut first, first_class) = load_with(THREE_ANSWERS, lowering);
        let (mut second, second_class) = load_with(THREE_ANSWERS, lowering);
        assert_eq!(run(&mut first, &first_class), 42);
        assert_eq!(run(&mut first, &first_class), 42);
        let before = second.jit_statistics();
        assert_eq!(run(&mut second, &second_class), 42);
        let compiled = since(second.jit_statistics(), before);
        assert!(
            compiled.compiled_bodies >= 1,
            "the second interpreter compiles `answer` into its own zone"
        );
    }
}

ns_runtime::for_each_lowering! {
    fn a_full_zone_leaves_bodies_interpreted(lowering) {
        let count = 300;
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
        let compiled = since(interpreter.jit_statistics(), before);
        assert!(compiled.refused_bodies >= 1, "the zone of one page fills");
    }
}

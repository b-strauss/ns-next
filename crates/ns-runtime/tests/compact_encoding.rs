//! Bodies past every sixteen-bit table index: a constant, a send site and
//! an allocated literal each past the 65,536 an eight-byte instruction can
//! name, so the last of each takes the wide form and runs. The sources are
//! generated, not written inline.
//!
//! They run under the optimized lowering only. The reference lowering gives
//! every value a register of its own and never reuses one, so each constant,
//! literal and send site costs it a register first, and a body reaches its
//! `u16::MAX` register ceiling before any table passes 65,536 entries
//! (`docs/OPEN-WORK.md`, "Known defects and gaps"). The wide send forms run
//! under both lowerings in `goldens/runtime/encoding/`.

use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    HostResult, HostSignal, Interpreter, Lowering, Root, SourceOrigin, Symbol, Value, ValueKind,
};

/// One past the 65,536 indices a table of an eight-byte instruction names:
/// the last statement's entry is the first a narrow form cannot reach.
const STATEMENTS: usize = 65_537;

fn load(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

/// `HostSignal` carries no `Debug` impl, so `Result::expect` is unusable on
/// a `HostResult` -- unwrap by hand.
fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

fn new_instance(interpreter: &mut Interpreter, class: &Root) -> Root {
    expect_ok(interpreter.instantiate(class, &[]), "instantiate")
}

fn send(
    interpreter: &mut Interpreter,
    receiver: &Root,
    selector: &str,
    arguments: &[Root],
) -> HostResult {
    interpreter.send(receiver, Symbol::intern(selector), arguments)
}

fn integer_of<'gc>(value: Value<'gc>) -> i64 {
    match value.kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("expected a SmallInteger"),
    }
}

/// A class whose `run` declares `x` and runs `statements` in order.
fn module_running(statements: &str) -> String {
    format!("class Generated = Object new () (\n    public run = ( | x | {statements} )\n) : ()\n")
}

/// Loads `source` under the optimized lowering, sends `run` to an instance,
/// and answers the integer it returns and the instruction count of `run`'s
/// body.
fn run_and_count(source: &str) -> (i64, usize) {
    let (mut interpreter, class) = load(source, Lowering::Optimized);
    let instruction_count = interpreter.enter(|mutator| {
        let Some(object) = mutator.fetch(&class).as_object() else {
            panic!("the module's class is a class object")
        };
        let application = mutator.heap().class_application(object).expect("a class");
        application
            .mixin
            .compiled_method(Symbol::intern("run"))
            .expect("run is compiled")
            .code
            .instruction_count()
    });
    let instance = new_instance(&mut interpreter, &class);
    let answer = expect_ok(send(&mut interpreter, &instance, "run", &[]), "run");
    let integer = interpreter.enter(|mutator| integer_of(mutator.fetch(&answer)));
    (integer, instruction_count)
}

/// Each statement is a `LoadLiteral` of its own constant and a
/// `StoreLocal`, then the `Return`: the 65,537th constant is in a `Wide`.
#[test]
fn a_constant_past_sixteen_bits_loads_from_the_wide_form() {
    let statements: String = (0..STATEMENTS)
        .map(|offset| format!("x:: {}. ", 3_000_000_000_u64 + offset as u64))
        .collect();
    let (answer, instruction_count) = run_and_count(&module_running(&format!("{statements}^x")));
    assert_eq!(answer, 3_000_065_536);
    assert_eq!(instruction_count, 2 * STATEMENTS + 1);
}

/// `x:: 0.` is a `LoadLiteral` and a `StoreLocal`; each `x:: x + 1.` a
/// `LoadLiteral`, an `IntegerBinary`, the `Jump` past its fallback, the
/// fallback `SendPublic` -- one send site per statement -- and a
/// `StoreLocal`; then the `Return`. The 65,537th site is in the wide table.
#[test]
fn a_send_site_past_sixteen_bits_sends_from_the_wide_form() {
    let statements = "x:: x + 1. ".repeat(STATEMENTS);
    let (answer, instruction_count) =
        run_and_count(&module_running(&format!("x:: 0. {statements}^x")));
    assert_eq!(answer, 65_537);
    assert_eq!(instruction_count, 2 + 5 * STATEMENTS + 1);
}

/// Each statement is a `LoadLiteral` of its own string and a `StoreLocal`,
/// then the `size` send and the `Return`: the 65,537th string is in a
/// `Wide`.
#[test]
fn an_allocated_literal_past_sixteen_bits_loads_from_the_wide_form() {
    let statements = "x:: 'a'. ".repeat(STATEMENTS);
    let (answer, instruction_count) =
        run_and_count(&module_running(&format!("{statements}^x size")));
    assert_eq!(answer, 1);
    assert_eq!(instruction_count, 2 * STATEMENTS + 2);
}

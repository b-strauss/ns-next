//! Bodies past every sixteen-bit table index: a constant, a send site and
//! an allocated literal each past the 65,536 an eight-byte instruction can
//! name, so the first of each takes the eight-byte form, the last the wide
//! form, and the body runs. The sources are generated, not written inline.
//!
//! Each answers under the optimized lowering. The reference lowering gives
//! every value a register of its own and never reuses one, so each constant,
//! literal and send site costs it a register first, and the same body passes
//! its `u16::MAX` register ceiling before any table passes 65,536 entries:
//! there it compiles to a body that raises the too-large error when it runs
//! (`docs/design/compiler.md`, "Compilation cannot fail"). The wide send
//! forms run under both lowerings in `goldens/runtime/encoding/`.

use std::path::PathBuf;
use std::rc::Rc;

use indoc::formatdoc;

use ns_runtime::bytecode::{CompiledCode, Instruction, Literal};
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

/// The error a body past the register ceiling raises, sent as `run`.
const METHOD_TOO_LARGE: &str = "method too large: it needs more than 65535 registers";

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

/// The other half of `expect_ok`: the `NewspeakError`'s message from a
/// `HostResult` expected to fail.
fn expect_error(result: HostResult, what: &str) -> String {
    match result {
        Ok(_) => panic!("{what}: expected a Newspeak error"),
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => error.message,
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
/// and answers the integer it returns and `run`'s body, which holds no
/// handle and so outlives the entry it is read in.
fn run_and_compile(source: &str) -> (i64, Rc<CompiledCode>) {
    let (mut interpreter, class) = load(source, Lowering::Optimized);
    let code = interpreter.enter(|mutator| {
        let Some(object) = mutator.fetch(&class).as_object() else {
            panic!("the module's class is a class object")
        };
        let application = mutator.heap().class_application(object).expect("a class");
        application
            .mixin
            .compiled_method(Symbol::intern("run"))
            .expect("run is compiled")
            .code
            .clone()
    });
    let instance = new_instance(&mut interpreter, &class);
    let answer = expect_ok(send(&mut interpreter, &instance, "run", &[]), "run");
    let integer = interpreter.enter(|mutator| integer_of(mutator.fetch(&answer)));
    (integer, code)
}

/// The first of a table's entries, at `first`, takes the eight-byte form
/// (unless the stress mode makes every instruction wide), and the
/// 65,537th, at `last`, the wide form; `is_entry` says each index holds
/// the instruction the table serves.
fn assert_forms(
    code: &CompiledCode,
    first: usize,
    last: usize,
    is_entry: impl Fn(&Instruction) -> bool,
) {
    assert!(
        is_entry(&code.instruction(first)),
        "the first entry is at {first}"
    );
    assert!(
        is_entry(&code.instruction(last)),
        "the last entry is at {last}"
    );
    assert_eq!(
        code.is_wide(first),
        CompiledCode::every_instruction_is_wide()
    );
    assert!(code.is_wide(last), "the 65,537th entry takes the wide form");
}

/// Loads `source` under `lowering` and answers the error `run` raises on an
/// instance: the module loads, so only running the body fails.
fn run_error(source: &str, lowering: Lowering) -> String {
    let (mut interpreter, class) = load(source, lowering);
    let instance = new_instance(&mut interpreter, &class);
    expect_error(send(&mut interpreter, &instance, "run", &[]), "run")
}

/// Each statement is a `LoadLiteral` of its own constant and a
/// `StoreLocal`, then the `Return`: the 65,537th constant is in a `Wide`.
#[test]
fn a_constant_past_sixteen_bits_loads_from_the_wide_form() {
    let statements: String = (0..STATEMENTS)
        .map(|offset| format!("x:: {}. ", 3_000_000_000_u64 + offset as u64))
        .collect();
    let source = module_running(&format!("{statements}^x"));
    let (answer, code) = run_and_compile(&source);
    assert_eq!(answer, 3_000_065_536);
    assert_eq!(code.instruction_count(), 2 * STATEMENTS + 1);
    assert_forms(&code, 0, 2 * (STATEMENTS - 1), |instruction| {
        matches!(
            instruction,
            Instruction::LoadLiteral {
                literal: Literal::SmallInteger(_),
                ..
            }
        )
    });
    assert_eq!(run_error(&source, Lowering::Reference), METHOD_TOO_LARGE);
}

/// `x:: 0.` is a `LoadLiteral` and a `StoreLocal`; each `x:: x + 1.` a
/// `LoadLiteral`, an `IntegerBinary`, the `Jump` past its fallback, the
/// fallback `SendPublic` -- one send site per statement -- and a
/// `StoreLocal`; then the `Return`. The 65,537th site is in the wide table.
#[test]
fn a_send_site_past_sixteen_bits_sends_from_the_wide_form() {
    let statements = "x:: x + 1. ".repeat(STATEMENTS);
    let source = module_running(&format!("x:: 0. {statements}^x"));
    let (answer, code) = run_and_compile(&source);
    assert_eq!(answer, 65_537);
    assert_eq!(code.instruction_count(), 2 + 5 * STATEMENTS + 1);
    assert_forms(&code, 2 + 3, 2 + 5 * (STATEMENTS - 1) + 3, |instruction| {
        matches!(instruction, Instruction::SendPublic { .. })
    });
    assert_eq!(run_error(&source, Lowering::Reference), METHOD_TOO_LARGE);
}

/// Each statement is a `LoadLiteral` of its own string and a `StoreLocal`,
/// then the `size` send and the `Return`: the 65,537th string is in a
/// `Wide`.
#[test]
fn an_allocated_literal_past_sixteen_bits_loads_from_the_wide_form() {
    let statements = "x:: 'a'. ".repeat(STATEMENTS);
    let source = module_running(&format!("{statements}^x size"));
    let (answer, code) = run_and_compile(&source);
    assert_eq!(answer, 1);
    assert_eq!(code.instruction_count(), 2 * STATEMENTS + 2);
    assert_forms(&code, 0, 2 * (STATEMENTS - 1), |instruction| {
        matches!(
            instruction,
            Instruction::LoadLiteral {
                literal: Literal::String(_),
                ..
            }
        )
    });
    assert_eq!(run_error(&source, Lowering::Reference), METHOD_TOO_LARGE);
}

/// More named temporaries than a `u16` numbers registers for: no lowering
/// can lay `run` out, so under both it compiles to the raising body. The
/// class still loads, and `answer`, which never needed the ceiling, runs --
/// a body never run never fails.
#[test]
fn a_body_with_more_temporaries_than_registers_raises_when_it_runs() {
    let temporaries: String = (0..=usize::from(u16::MAX))
        .map(|index| format!("t{index} "))
        .collect();
    let source = format!(
        "class Generated = Object new () (
    public run = ( | {temporaries}| t0:: 1. ^t0 )
    public answer = ( ^7 )
) : ()
"
    );
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(&source, lowering);
        let instance = new_instance(&mut interpreter, &class);
        assert_eq!(
            expect_error(send(&mut interpreter, &instance, "run", &[]), "run"),
            METHOD_TOO_LARGE
        );
        let answer = expect_ok(send(&mut interpreter, &instance, "answer", &[]), "answer");
        assert_eq!(
            interpreter.enter(|mutator| integer_of(mutator.fetch(&answer))),
            7
        );
    }
}

/// `count` copies of `element`, each followed by a period: a tuple's or a
/// body's generated contents.
fn repeated(element: &str, count: usize) -> String {
    (0..count).map(|_| format!("{element}. ")).collect()
}

/// Loads `nested` -- declarations nested in `Generated`, beside its
/// `public make = ( <make> )` -- under `lowering`, and answers the error
/// `make` raises on an instance.
fn make_error(nested: &str, make: &str, lowering: Lowering) -> String {
    let source = format!(
        "class Generated = Object new () (
{nested}
    public make = ( {make} )
) : ()
"
    );
    let (mut interpreter, class) = load(&source, lowering);
    let instance = new_instance(&mut interpreter, &class);
    expect_error(send(&mut interpreter, &instance, "make", &[]), "make")
}

/// Every kind of body past the ceiling raises with its own kind's name. An
/// instance initializer: a slot initialized to a tuple of 65,535 values,
/// each in a register of its own.
#[test]
fn an_initializer_past_the_register_ceiling_raises_when_it_runs() {
    let elements = repeated("1", usize::from(u16::MAX));
    let nested = format!("    class Big = Object new ( | tuple = {{{elements}}}. | ) () : ()");
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        assert_eq!(
            make_error(&nested, "^Big new", lowering),
            "initializer too large: it needs more than 65535 registers"
        );
    }
}

/// A clause message's arguments, one tuple of 65,535 values, which the
/// class's initializer evaluates: the initializer is past the ceiling.
#[test]
fn a_clause_message_past_the_register_ceiling_raises_when_it_runs() {
    let elements = repeated("1", usize::from(u16::MAX));
    let nested = format!(
        "    class Base x: value = Object new () () : ()
    class Big = Base x: {{{elements}}} () () : ()"
    );
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        assert_eq!(
            make_error(&nested, "^Big new", lowering),
            "initializer too large: it needs more than 65535 registers"
        );
    }
}

/// A factory of 65,536 parameters: its declared arity is 65,536, so the
/// host's send of 65,536 arguments passes the arity check and is refused at
/// the push, which a frame of that many arguments cannot record, with the
/// too-large error its body would raise -- not an arity mismatch, before
/// its initializer runs. No send instruction can hand it them: a send names
/// its arguments' contiguous slots, so `build:`, which sends the factory
/// 65,536 arguments, is itself past the ceiling under both lowerings. The
/// host sends them instead.
#[test]
fn a_factory_of_more_parameters_than_registers_raises_the_too_large_error() {
    let pattern: String = (0..=usize::from(u16::MAX))
        .map(|index| format!("k: p{index} "))
        .collect();
    let selector: String = (0..=usize::from(u16::MAX)).map(|_| "k:").collect();
    let arguments: String = (0..=usize::from(u16::MAX)).map(|_| "k: x ").collect();
    let nested = format!(
        "    class Marker y: value = Object new () () : ()
    class Wide {pattern}= Object <: Marker y: p0 () () : ()
    build: x = ( ^Wide {arguments})"
    );
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        assert_eq!(make_error(&nested, "^build: 1", lowering), METHOD_TOO_LARGE);
        let source = format!(
            "class Generated = Object new () (
{nested}
    public make = ( ^Wide )
) : ()
"
        );
        let (mut interpreter, class) = load(&source, lowering);
        let instance = new_instance(&mut interpreter, &class);
        let wide = expect_ok(send(&mut interpreter, &instance, "make", &[]), "make");
        let arguments: Vec<Root> = (0..=usize::from(u16::MAX))
            .map(|_| interpreter.stash(Value::small_integer(1)))
            .collect();
        assert_eq!(
            expect_error(
                send(&mut interpreter, &wide, &selector, &arguments),
                "the factory"
            ),
            "factory too large: it needs more than 65535 registers"
        );
    }
}

/// A method of 65,536 parameters sent its 65,536 arguments by a program's
/// `Message>>sendTo:`, the one way Newspeak code hands a body a list of
/// values longer than any send's window: refused at the push with the
/// method's too-large error, never an abort. The message comes from the
/// program's own `doesNotUnderstand:`; the host fills in its selector and
/// its arguments, which no body under the ceiling could build.
#[test]
fn a_message_of_more_arguments_than_registers_raises_the_too_large_error() {
    let pattern: String = (0..=usize::from(u16::MAX))
        .map(|index| format!("k: p{index} "))
        .collect();
    let selector: String = (0..=usize::from(u16::MAX)).map(|_| "k:").collect();
    let source = formatdoc! {"
        class Generated = Object new () (
          public {pattern}= ( ^p0 )
          public grab = ( ^self missing )
          public doesNotUnderstand: message = ( ^message )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(&source, lowering);
        let instance = new_instance(&mut interpreter, &class);
        let message = expect_ok(send(&mut interpreter, &instance, "grab", &[]), "grab");
        let (selector_root, arguments_root) = interpreter.enter(|mutator| {
            let symbol = mutator.new_symbol(Symbol::intern(&selector));
            let elements = vec![Value::small_integer(1); usize::from(u16::MAX) + 1];
            let array = Value::object(mutator.new_array(elements));
            (mutator.stash(symbol), mutator.stash(array))
        });
        expect_ok(
            send(&mut interpreter, &message, "selector:", &[selector_root]),
            "selector:",
        );
        expect_ok(
            send(&mut interpreter, &message, "arguments:", &[arguments_root]),
            "arguments:",
        );
        assert_eq!(
            expect_error(
                send(&mut interpreter, &message, "sendTo:", &[instance]),
                "sendTo:"
            ),
            METHOD_TOO_LARGE
        );
    }
}

/// A pattern of `count` keyword parts, `k: p0 k: p1 ...`: a method's or a
/// factory's parameters.
fn keyword_pattern(count: usize) -> String {
    (0..count).map(|index| format!("k: p{index} ")).collect()
}

/// A method of 256 parameters is past the argument limit: it compiles to
/// the raising body, which the host's send of 256 arguments runs, with the
/// limit in its message.
#[test]
fn a_method_of_256_parameters_raises_when_run() {
    let pattern = keyword_pattern(256);
    let source = formatdoc! {"
        class Generated = Object new () (
          public {pattern}= ( ^p0 )
          public answer = ( ^7 )
        ) : ()
    "};
    let selector = "k:".repeat(256);
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(&source, lowering);
        let instance = new_instance(&mut interpreter, &class);
        let arguments: Vec<Root> = (0..256)
            .map(|_| interpreter.stash(Value::small_integer(1)))
            .collect();
        assert_eq!(
            expect_error(
                send(&mut interpreter, &instance, &selector, &arguments),
                "the wide method"
            ),
            "method too large: it takes or sends more than 255 arguments",
            "{lowering:?}"
        );
        let answer = expect_ok(send(&mut interpreter, &instance, "answer", &[]), "answer");
        assert_eq!(
            interpreter.enter(|mutator| integer_of(mutator.fetch(&answer))),
            7
        );
    }
}

/// A send of 256 arguments makes the body that holds it too large: the
/// method raises when it runs, never when it compiles.
#[test]
fn a_send_of_256_arguments_raises_when_run() {
    let arguments = "k: 1 ".repeat(256);
    let source = formatdoc! {"
        class Generated = Object new () (
          public run = ( ^self {arguments})
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        assert_eq!(
            run_error(&source, lowering),
            "method too large: it takes or sends more than 255 arguments",
            "{lowering:?}"
        );
    }
}

/// At the limit, 255 parameters and a send of 255 arguments, the method
/// runs.
#[test]
fn a_method_of_255_parameters_runs() {
    let pattern = keyword_pattern(255);
    let arguments: String = (0..255).map(|index| format!("k: {index} ")).collect();
    let source = formatdoc! {"
        class Generated = Object new () (
          public {pattern}= ( ^p254 - p1 )
          public run = ( ^self {arguments})
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(&source, lowering);
        let instance = new_instance(&mut interpreter, &class);
        let answer = expect_ok(send(&mut interpreter, &instance, "run", &[]), "run");
        assert_eq!(
            interpreter.enter(|mutator| integer_of(mutator.fetch(&answer))),
            253,
            "{lowering:?}"
        );
    }
}

//! The trampoline (`docs/design/execution.md`, "The trampoline"): sends
//! run on the activation stack, not the Rust stack, so recursion depth is
//! bounded by a count rather than by the native stack, and running past it is
//! an ordinary Newspeak error under both lowerings.

use indoc::{formatdoc, indoc};
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    HostResult, HostSignal, Interpreter, Lowering, Root, SourceOrigin, Symbol, Value,
};

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

fn small_integer<'gc>(value: Value<'gc>) -> i64 {
    match value {
        Value::SmallInteger(integer) => integer,
        _ => panic!("expected a SmallInteger"),
    }
}

const DEPTH: &str = indoc! {"
    Newspeak3
    'T'
    class Depth = Object new (
    ) (
        public down: n = ( n = 0 ifTrue: [^0]. ^down: n - 1 )
        public logic: n = ( ^n = 0 or: [(logic: n - 1) and: [true]] )
        public loop: n = ( [n > 0] whileTrue: [^loop: n - 1]. ^0 )
    ) : ()
"};

/// Fifty thousand nested method activations: far past what the Rust stack of
/// a test thread would hold if every send recursed on it, and nothing but
/// activation-stack entries since none does. Under the optimized lowering no
/// native re-enters Newspeak here, since every control-flow send is inlined.
#[test]
fn a_deep_method_recursion_runs_on_the_activation_stack() {
    let (mut interpreter, class) = load(DEPTH, Lowering::Optimized);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    for selector in ["down:", "logic:", "loop:"] {
        let result = expect_ok(
            interpreter.send(
                &instance,
                Symbol::intern(selector),
                &[interpreter.stash(Value::SmallInteger(50_000))],
            ),
            selector,
        );
        assert!(
            interpreter.enter(|mutator| matches!(
                mutator.fetch(&result),
                Value::SmallInteger(0) | Value::Boolean(true)
            )),
            "{selector}"
        );
        assert_eq!(
            interpreter.enter(|mutator| mutator.heap().activation_depth()),
            0,
            "{selector}: nothing left behind"
        );
    }
}

/// A `^` out of a method several activations up, through a block, lands on the
/// method it names and leaves nothing behind it on the stack.
#[test]
fn a_non_local_return_unwinds_to_its_home_on_the_activation_stack() {
    let source = indoc! {"
        Newspeak3
        'T'
        class R = Object new (
        ) (
            public find = ( self each: [:e | e = 3 ifTrue: [^e * 10]]. ^0 )
            each: blk = ( self to: 5 do: blk )
            to: hi do: blk = ( | i | i:: 1. [i <= hi] whileTrue: [blk value: i. i:: i + 1] )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(source, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let result = expect_ok(
            interpreter.send(&instance, Symbol::intern("find"), &[]),
            "find",
        );
        assert_eq!(
            interpreter.enter(|mutator| small_integer(mutator.fetch(&result))),
            30,
            "{lowering:?}"
        );
        assert_eq!(
            interpreter.enter(|mutator| mutator.heap().activation_depth()),
            0,
            "{lowering:?}"
        );
    }
}

/// A `^` into an initializer that is still running escapes -- the known gap
/// in `docs/design/execution.md`, "Non-local return" -- and it escapes as the
/// signal it always was, not as a panic: an initializer catches no return,
/// so the unwind pops everything down to the floor of the host's run and
/// leaves it with the signal. Closing the gap changes what this expects;
/// nothing else should.
#[test]
fn a_return_into_a_running_initializer_escapes_the_run() {
    let source = indoc! {"
        Newspeak3
        'T'
        class E = Object new (
        ) (
            class Inner = Object new ( | a = [^1] value. | ) ( ) : ()
            public make = ( Inner new. ^0 )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(source, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        match interpreter.send(&instance, Symbol::intern("make"), &[]) {
            Err(HostSignal::Return) => {}
            Ok(_) => panic!("{lowering:?}: the `^` must escape, not be answered"),
            Err(HostSignal::Error(error)) => {
                panic!("{lowering:?}: unexpected error: {}", error.message)
            }
        }
        assert_eq!(
            interpreter.enter(|mutator| mutator.heap().activation_depth()),
            0,
            "{lowering:?}"
        );
    }
}

/// Instantiation is a trampoline step: a class whose slot initializer
/// instantiates it again, twenty thousand levels deep, is twenty thousand
/// initializer activations on the stack and nothing on the Rust stack.
#[test]
fn a_deep_recursion_through_instantiation_runs_on_the_activation_stack() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Tower = Object new (
        ) (
            public class Floor n: k = Object new (
                | below = k = 0 ifTrue: [nil] ifFalse: [Floor n: k - 1]. |
            ) (
                public height = ( ^below == nil ifTrue: [0] ifFalse: [1 + below height] )
            ) : ()
            public build: k = ( ^(Floor n: k) height )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(source, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let height = expect_ok(
            interpreter.send(
                &instance,
                Symbol::intern("build:"),
                &[interpreter.stash(Value::SmallInteger(20_000))],
            ),
            "build:",
        );
        assert_eq!(
            interpreter.enter(|mutator| small_integer(mutator.fetch(&height))),
            20_000,
            "{lowering:?}"
        );
        interpreter.enter(|mutator| {
            assert_eq!(mutator.heap().activation_depth(), 0, "{lowering:?}");
            assert_eq!(mutator.heap().counted_depth(), 0, "{lowering:?}");
        });
    }
}

/// A recursion through blocks the optimized lowering does *not* inline --
/// each held in a variable -- runs through the natives, and none of them
/// calls back into the loop, so forty thousand levels fit on a test thread's
/// stack under either lowering.
const THROUGH_NATIVES: &str = indoc! {"
    Newspeak3
    'T'
    class N = Object new (
    ) (
        public viaValue: n = ( | b | b:: [:k | k = 0 ifTrue: [0] ifFalse: [b value: k - 1]]. ^b value: n )
        public viaIf: n = ( | t f | t:: [0]. f:: [viaIf: n - 1]. ^n = 0 ifTrue: t ifFalse: f )
        public viaOr: n = ( | rest | rest:: [(viaOr: n - 1) = 0]. ^(n = 0 or: rest) ifTrue: [0] ifFalse: [1] )
        public viaWhile: n = ( | c b | c:: [n > 0]. b:: [^viaWhile: n - 1]. c whileTrue: b. ^0 )
    ) : ()
"};

#[test]
fn a_deep_recursion_through_native_block_activations_runs_on_the_activation_stack() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(THROUGH_NATIVES, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for selector in ["viaValue:", "viaIf:", "viaOr:", "viaWhile:"] {
            let result = expect_ok(
                interpreter.send(
                    &instance,
                    Symbol::intern(selector),
                    &[interpreter.stash(Value::SmallInteger(40_000))],
                ),
                selector,
            );
            assert_eq!(
                interpreter.enter(|mutator| small_integer(mutator.fetch(&result))),
                0,
                "{selector} under {lowering:?}"
            );
            assert_eq!(
                interpreter.enter(|mutator| mutator.heap().activation_depth()),
                0,
                "{selector} under {lowering:?}"
            );
        }
    }
}

const TOO_DEEP: &str = "stack overflow: recursion too deep";

fn expect_too_deep(result: HostResult, what: &str) {
    match result {
        Err(HostSignal::Error(error)) => assert_eq!(error.message, TOO_DEEP, "{what}"),
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Ok(_) => panic!("{what}: ran past the depth limit"),
    }
}

/// The limit is exactly `MAX_ACTIVATION_DEPTH` counted activations, and it
/// counts alike under both lowerings (`docs/design/compiler.md`, "How the
/// limit counts"). `down: k` sent from Rust makes k + 1 method activations,
/// so `down: 99_999` is the deepest that fits and `down: 100_000` one too
/// many. `logic:` and `loop:` recurse from inside an `or:` block and a
/// `whileTrue:` body, which the optimized lowering inlines and the reference
/// lowering activates -- exempt, so the boundary is the same.
#[test]
fn the_depth_limit_is_exact_and_counts_alike_under_both_lowerings() {
    assert_eq!(ns_runtime::interpreter::MAX_ACTIVATION_DEPTH, 100_000);
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(DEPTH, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for selector in ["down:", "logic:", "loop:"] {
            let what = format!("{selector} under {lowering:?}");
            expect_ok(
                interpreter.send(
                    &instance,
                    Symbol::intern(selector),
                    &[interpreter.stash(Value::SmallInteger(99_999))],
                ),
                &what,
            );
            expect_too_deep(
                interpreter.send(
                    &instance,
                    Symbol::intern(selector),
                    &[interpreter.stash(Value::SmallInteger(100_000))],
                ),
                &what,
            );
            interpreter.enter(|mutator| {
                assert_eq!(
                    mutator.heap().activation_depth(),
                    0,
                    "{what}: nothing left behind"
                );
                assert_eq!(
                    mutator.heap().counted_depth(),
                    0,
                    "{what}: the unwind uncounted every activation it popped"
                );
            });
            // An overflow is an ordinary error: the interpreter goes on.
            expect_ok(
                interpreter.send(
                    &instance,
                    Symbol::intern(selector),
                    &[interpreter.stash(Value::SmallInteger(10))],
                ),
                &what,
            );
        }
    }
}

/// `ensure:`, `ifCurtailed:` and `on:do:` are ordinary kernel methods, each
/// an activation and each activating its receiver block, so a level of
/// `marked:` is seven counted activations under either lowering -- the
/// method, the three kernel methods and their three blocks -- and the
/// deepest recursion that fits is the same under both: 7 x 14,285 + 1 is
/// 99,996 activations, 7 x 14,286 + 1 is past the limit.
#[test]
fn the_marked_block_methods_count_alike_under_both_lowerings() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Marked = Object new (
        ) (
            public marked: n = (
                n = 0 ifTrue: [^0].
                ^[[[marked: n - 1] on: nil do: nil] ifCurtailed: []] ensure: []
            )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(source, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let what = format!("marked: under {lowering:?}");
        expect_ok(
            interpreter.send(
                &instance,
                Symbol::intern("marked:"),
                &[interpreter.stash(Value::SmallInteger(14_285))],
            ),
            &what,
        );
        expect_too_deep(
            interpreter.send(
                &instance,
                Symbol::intern("marked:"),
                &[interpreter.stash(Value::SmallInteger(14_286))],
            ),
            &what,
        );
        interpreter.enter(|mutator| {
            assert_eq!(mutator.heap().activation_depth(), 0, "{what}");
            assert_eq!(mutator.heap().counted_depth(), 0, "{what}");
        });
    }
}

/// A block activated through `value` is counted like any other activation,
/// so a recursion through blocks alone ends in the same error.
#[test]
fn a_recursion_through_block_values_is_bounded_too() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(THROUGH_NATIVES, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        expect_too_deep(
            interpreter.send(
                &instance,
                Symbol::intern("viaValue:"),
                &[interpreter.stash(Value::SmallInteger(250_000))],
            ),
            &format!("viaValue: under {lowering:?}"),
        );
    }
}

/// The decoded code table of an artifact, read straight off its bytes: the
/// symbol table, which the code table follows.
fn code_table_of(bytes: &[u8]) -> Vec<Rc<ns_runtime::bytecode::CompiledCode>> {
    use ns_runtime::image;
    let report = image::inspect::inspect(bytes).expect("inspect");
    let symbols_start = report
        .sections
        .iter()
        .find(|section| section.name == "symbols")
        .expect("a symbol table")
        .start;
    let mut reader = image::decode::Reader::new(&bytes[symbols_start..]);
    let symbols = image::symbols::decode(&mut reader).expect("symbols");
    image::decode::decode_code_table(&mut reader, &symbols)
        .expect("code table")
        .codes
}

/// A crafted artifact can send `ifTrue:` as a `SendInlinable` with a block
/// the sending activation did not create -- here the block itself, so it
/// recurses. The exemption requires the block's captured frame to be the
/// sender's (`docs/design/compiler.md`, "How the limit counts"), so this is
/// counted and ends in the depth error; without that condition it would run
/// two hundred thousand levels uncounted and answer.
///
/// Twice: once with a sending block that creates nothing, so its activation
/// has no frame at all, and once with one that creates a block first, `[0]`,
/// which reifies its activation. The second is what keeps the condition from
/// being weakened to "the sender has a frame": its sender has one, just not
/// the frame the block captured.
#[test]
fn a_crafted_send_inlinable_cannot_recurse_uncounted() {
    for (block, sender_reified) in [
        ("[n:: n + 1. n < 200000 ifTrue: b]", false),
        ("[[0]. n:: n + 1. n < 200000 ifTrue: b]", true),
    ] {
        crafted_recursion_is_counted(block, sender_reified);
    }
}

fn crafted_recursion_is_counted(block: &str, sender_reified: bool) {
    use ns_runtime::bytecode::{CodeKind, Instruction};
    use ns_runtime::image;
    use ns_runtime::image::opcodes::{OPCODE_SEND_INLINABLE, OPCODE_SEND_PUBLIC};
    let source = formatdoc! {"
        Newspeak3
        'T'
        class Crafted = Object new (
        ) (
            public main = ( | b n | n:: 0. b:: {block}. ^b value )
        ) : ()
    "};
    // `n < 200000 ifTrue: b` is not a literal-block send, so even the
    // reference lowering compiles it to a `SendPublic`, which the mutation
    // below turns into a `SendInlinable` -- the two share one operand layout.
    let (mut interpreter, class) = load(&source, Lowering::Reference);
    // Nothing has run in this interpreter, so the class read from its root
    // is current, and building the artifact runs nothing either -- both
    // happen in one entry.
    let mut bytes = interpreter
        .enter(|mutator| {
            let Value::Object(class) = mutator.fetch(&class) else {
                panic!("{block}: load_module answers a class object")
            };
            image::build(
                mutator.heap(),
                None,
                &SourceOrigin::File(PathBuf::from("t.ns")),
                class,
                Symbol::intern("main"),
            )
        })
        .expect("build");
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let id = report
        .symbols
        .iter()
        .position(|symbol| symbol == "ifTrue:")
        .expect("ifTrue: is interned") as u32;
    let mut pattern = vec![OPCODE_SEND_PUBLIC];
    pattern.extend_from_slice(&id.to_le_bytes());
    let offsets: Vec<usize> = bytes
        .windows(pattern.len())
        .enumerate()
        .filter(|(_, window)| *window == pattern.as_slice())
        .map(|(offset, _)| offset)
        .collect();
    assert_eq!(
        offsets.len(),
        1,
        "{block}: exactly one `ifTrue:` send to mutate"
    );
    bytes[offsets[0]] = OPCODE_SEND_INLINABLE;

    // The mutation hit the instruction it meant to, in the body that recurses.
    let codes = code_table_of(&bytes);
    let sender = codes
        .iter()
        .find(|code| {
            code.instructions.iter().any(|instruction| {
                matches!(instruction, Instruction::SendInlinable { selector, .. }
                    if selector.as_str() == "ifTrue:")
            })
        })
        .unwrap_or_else(|| panic!("{block}: a `SendInlinable` of `ifTrue:`"));
    assert_eq!(sender.kind, CodeKind::Block, "{block}: sent from the block");
    assert_eq!(
        sender
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::MakeClosure { .. })),
        sender_reified,
        "{block}: whether the sending block reifies its own activation"
    );

    let mut loaded = image::load(&bytes).expect("the mutated artifact decodes");
    let instance = expect_ok(
        loaded.interpreter.instantiate(&loaded.root_class, &[]),
        "instantiate",
    );
    expect_too_deep(
        loaded
            .interpreter
            .send(&instance, Symbol::intern("main"), &[]),
        &format!("the crafted recursion through {block}"),
    );
    loaded.interpreter.enter(|mutator| {
        assert_eq!(mutator.heap().activation_depth(), 0, "{block}");
        assert_eq!(mutator.heap().counted_depth(), 0, "{block}");
    });
}

/// A control-flow step sent from Rust, not from the loop: its block is
/// pushed at the floor of the run `Interpreter::send` starts, so the
/// continuation that pops it -- `Deliver` for `and:` and `or:`, a loop's
/// `WhileCondition` and `WhileBody` -- runs with no caller in that run, and a loop's next block
/// replaces the popped one right at the floor. Each must answer the native's
/// value and leave both depths where they were.
#[test]
fn a_control_flow_native_sent_from_rust_continues_at_the_floor_of_its_run() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Floor = Object new (
            | n ::= 0. |
        ) (
            public below: k = ( ^[n < k] )
            public atLeast: k = ( ^[n >= k] )
            public step = ( ^[n:: n + 1] )
            public count = ( ^n )
            public constant: v = ( ^[v] )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(source, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        // Each send takes the interpreter mutably, so the literal receivers
        // and arguments are rooted before the first of them.
        let [integer_3, integer_5, integer_42, true_value, false_value] = [
            Value::SmallInteger(3),
            Value::SmallInteger(5),
            Value::SmallInteger(42),
            Value::Boolean(true),
            Value::Boolean(false),
        ]
        .map(|value| interpreter.stash(value));
        // The arguments are borrowed, so one root can be passed several times.
        let send = |interpreter: &mut Interpreter,
                    receiver: &Root,
                    selector: &str,
                    arguments: &[&Root]| {
            let arguments: Vec<Root> = arguments.iter().copied().cloned().collect();
            at_the_floor(interpreter, lowering, receiver, selector, &arguments)
        };
        let below_three = expect_ok(
            send(&mut interpreter, &instance, "below:", &[&integer_3]),
            "below:",
        );
        let at_least_five = expect_ok(
            send(&mut interpreter, &instance, "atLeast:", &[&integer_5]),
            "atLeast:",
        );
        let step = expect_ok(send(&mut interpreter, &instance, "step", &[]), "step");
        let three = expect_ok(
            send(&mut interpreter, &instance, "constant:", &[&integer_3]),
            "constant:",
        );
        let yes = expect_ok(
            send(&mut interpreter, &instance, "constant:", &[&true_value]),
            "constant:",
        );
        let answer = expect_ok(
            send(&mut interpreter, &instance, "constant:", &[&integer_42]),
            "constant:",
        );
        let what = |selector: &str| format!("{selector} under {lowering:?}");

        // `WhileCondition` and `WhileBody`, each replacing the other.
        let value = expect_ok(
            send(&mut interpreter, &below_three, "whileTrue:", &[&step]),
            &what("whileTrue:"),
        );
        assert!(
            interpreter.enter(|mutator| matches!(mutator.fetch(&value), Value::Nil)),
            "{}",
            what("whileTrue:")
        );
        let count = expect_ok(send(&mut interpreter, &instance, "count", &[]), "count");
        assert_eq!(
            interpreter.enter(|mutator| small_integer(mutator.fetch(&count))),
            3,
            "{}",
            what("whileTrue:")
        );
        let value = expect_ok(
            send(&mut interpreter, &at_least_five, "whileFalse:", &[&step]),
            &what("whileFalse:"),
        );
        assert!(
            interpreter.enter(|mutator| matches!(mutator.fetch(&value), Value::Nil)),
            "{}",
            what("whileFalse:")
        );
        let count = expect_ok(send(&mut interpreter, &instance, "count", &[]), "count");
        assert_eq!(
            interpreter.enter(|mutator| small_integer(mutator.fetch(&count))),
            5,
            "{}",
            what("whileFalse:")
        );

        // `and:`/`or:` answer the block's value as it is.
        let value = expect_ok(
            send(&mut interpreter, &true_value, "and:", &[&three]),
            &what("and:"),
        );
        assert!(
            interpreter.enter(|mutator| matches!(mutator.fetch(&value), Value::SmallInteger(3))),
            "{}",
            what("and:")
        );
        let value = expect_ok(
            send(&mut interpreter, &true_value, "and:", &[&yes]),
            &what("and:"),
        );
        assert!(
            interpreter.enter(|mutator| matches!(mutator.fetch(&value), Value::Boolean(true))),
            "{}",
            what("and:")
        );
        let value = expect_ok(
            send(&mut interpreter, &false_value, "or:", &[&yes]),
            &what("or:"),
        );
        assert!(
            interpreter.enter(|mutator| matches!(mutator.fetch(&value), Value::Boolean(true))),
            "{}",
            what("or:")
        );

        // `Deliver`, the continuation every other activation has too.
        let value = expect_ok(
            send(&mut interpreter, &true_value, "ifTrue:", &[&answer]),
            &what("ifTrue:"),
        );
        assert_eq!(
            interpreter.enter(|mutator| small_integer(mutator.fetch(&value))),
            42,
            "{}",
            what("ifTrue:")
        );

        // A condition that answers a non-Boolean: the continuation decides a
        // `mustBeBoolean` send, which nothing here understands, and the error
        // leaves the floor as clean as a value does.
        match send(&mut interpreter, &three, "whileTrue:", &[&step]) {
            Err(HostSignal::Error(error)) => assert!(
                error.message.contains("mustBeBoolean"),
                "{}: {}",
                what("whileTrue:"),
                error.message
            ),
            Err(HostSignal::Return) => panic!("{}: a non-local return escaped", what("whileTrue:")),
            Ok(_) => panic!("{}: a non-Boolean condition must fail", what("whileTrue:")),
        }
    }
}

/// `do:` is a trampoline step, not a callback: a recursion through it is
/// activations on the stack and nothing on the Rust stack, so twenty thousand
/// levels -- far past what a test thread's stack holds when each level
/// re-enters the loop from Rust -- run under either lowering. Each level
/// counts two activations, the method and the block.
#[test]
fn a_deep_recursion_through_do_runs_on_the_activation_stack() {
    let source = indoc! {"
        Newspeak3
        'T'
        class DoDeep = Object new (
        ) (
            public d: n = ( n = 0 ifTrue: [^0]. #(1) do: [:e | ^d: n - 1]. ^1 )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(source, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let result = expect_ok(
            interpreter.send(
                &instance,
                Symbol::intern("d:"),
                &[interpreter.stash(Value::SmallInteger(20_000))],
            ),
            "d:",
        );
        assert_eq!(
            interpreter.enter(|mutator| small_integer(mutator.fetch(&result))),
            0,
            "{lowering:?}"
        );
        interpreter.enter(|mutator| {
            assert_eq!(mutator.heap().activation_depth(), 0, "{lowering:?}");
            assert_eq!(mutator.heap().counted_depth(), 0, "{lowering:?}");
        });
    }
}

/// The nested-class accessor is a trampoline step: a recursion through it --
/// each level a fresh `Link` whose `Next` clause reaches the accessor again --
/// ends at the activation limit, and the unwind unmarks every creation in
/// progress (`Interpreter::run_from_host` debug-asserts that).
#[test]
fn a_recursion_through_the_nested_class_accessor_ends_at_the_activation_limit() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Chain = Object new (
        ) (
            public class Link = Object new (
            ) (
                public class Next = self deeper (
                ) (
                ) : ()
                deeper = ( ^Link new Next )
            ) : ()
            public start = ( ^Link new Next )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(source, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        expect_too_deep(
            interpreter.send(&instance, Symbol::intern("start"), &[]),
            &format!("start under {lowering:?}"),
        );
        interpreter.enter(|mutator| {
            assert_eq!(mutator.heap().activation_depth(), 0, "{lowering:?}");
            assert_eq!(mutator.heap().counted_depth(), 0, "{lowering:?}");
        });
    }
}

/// A `^` whose home is a method below a superclass clause's activation pops
/// the clause on its way to that method (`interpreter::unwind`), and
/// the creation the clause had begun ends with it: the next access evaluates
/// the clause again rather than reporting the class as depending on itself.
#[test]
fn a_return_through_a_superclass_clause_abandons_its_creation() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Outer = Object new (
            | escape ::= nil. armed ::= true. |
        ) (
            public class Inner = self pick (
            ) (
            ) : ()
            pick = ( armed ifTrue: [armed:: false. escape value: 7]. ^Object )
            public probe = ( escape:: [:v | ^v]. ^Inner )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(source, lowering);
        let outer = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let escaped = expect_ok(
            at_the_floor(&mut interpreter, lowering, &outer, "probe", &[]),
            "the first probe",
        );
        assert_eq!(
            interpreter.enter(|mutator| small_integer(mutator.fetch(&escaped))),
            7,
            "{lowering:?}: the `^` lands on `probe`"
        );
        let inner = expect_ok(
            at_the_floor(&mut interpreter, lowering, &outer, "probe", &[]),
            "the second probe",
        );
        interpreter.enter(|mutator| {
            let Value::Object(inner_object) = mutator.fetch(&inner) else {
                panic!("{lowering:?}: the second access answers a class object");
            };
            assert!(mutator.heap().is_class(inner_object), "{lowering:?}");
        });
        let again = expect_ok(
            at_the_floor(&mut interpreter, lowering, &outer, "Inner", &[]),
            "the cached class",
        );
        // Both class objects are read after the run that answered the second.
        interpreter.enter(|mutator| {
            let Value::Object(inner) = mutator.fetch(&inner) else {
                panic!("{lowering:?}: the second access answers a class object");
            };
            assert!(
                matches!(mutator.fetch(&again), Value::Object(again) if again.same(inner)),
                "{lowering:?}: the class the second access built is the one cached"
            );
        });
    }
}

/// A `^` in a block of a superclass clause's message returns from the
/// message's own activation, which catches it (`Mutator::catches_return`):
/// the run loop pops the activation there and hands the value to the
/// instantiation's walk as the superclass's arguments, exactly as if the body
/// had answered it. The clause message is the one clause body a block can be
/// written in -- the clause receiver is a single name.
#[test]
fn a_return_in_a_clause_messages_block_hands_its_value_to_the_walk() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Outer = Object new (
        ) (
            class Base x: a = Object new (
                | uValue = a. |
            ) (
                public u = ( ^uValue )
            ) : ()
            class Cell = Base x: [^{3}] value (
                | vValue = 4. |
            ) (
                public v = ( ^vValue )
            ) : ()
            public make = ( | c | c:: Cell new. ^c u * 10 + c v )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(source, lowering);
        let outer = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let made = expect_ok(
            at_the_floor(&mut interpreter, lowering, &outer, "make", &[]),
            "make",
        );
        assert_eq!(
            interpreter.enter(|mutator| small_integer(mutator.fetch(&made))),
            34,
            "{lowering:?}: Base got the returned argument list, and Cell's own slot ran too"
        );
    }
}

/// A `^` into an initializer that has a level below it still to run -- its
/// activation carries the rest of the walk in its continuation -- escapes
/// like one into the last initializer (`a_return_into_a_running_initializer_escapes_the_run`),
/// and the unwind drops the walk's state with the activation: nothing is left
/// on either count, and the interpreter goes on.
#[test]
fn a_return_into_an_initializer_with_a_level_below_escapes_the_run() {
    let source = indoc! {"
        Newspeak3
        'T'
        class E = Object new (
        ) (
            class Base = Object new ( | a = [^1] value. | ) ( ) : ()
            class Cell = Base new ( | b = 2. | ) ( ) : ()
            public make = ( Cell new. ^0 )
            public fine = ( ^42 )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(source, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        match at_the_floor(&mut interpreter, lowering, &instance, "make", &[]) {
            Err(HostSignal::Return) => {}
            Ok(_) => panic!("{lowering:?}: the `^` must escape, not be answered"),
            Err(HostSignal::Error(error)) => {
                panic!("{lowering:?}: unexpected error: {}", error.message)
            }
        }
        let fine = expect_ok(
            at_the_floor(&mut interpreter, lowering, &instance, "fine", &[]),
            "fine",
        );
        assert_eq!(
            interpreter.enter(|mutator| small_integer(mutator.fetch(&fine))),
            42,
            "{lowering:?}"
        );
    }
}

/// An error in a superclass clause's message, while the walk of a `new` sent
/// from inside the run loop is still climbing: the unwind pops the message's
/// activation and the method that sent `new`, drops the walk's state, and
/// leaves the run with the error.
#[test]
fn an_error_in_a_clause_message_mid_climb_unwinds_the_run() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Outer = Object new (
        ) (
            class Base x: a = Object new ( | u = a. | ) ( ) : ()
            class Cell = Base x: nil foo ( ) ( ) : ()
            public make = ( ^Cell new )
            public fine = ( ^42 )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, class) = load(source, lowering);
        let outer = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        match at_the_floor(&mut interpreter, lowering, &outer, "make", &[]) {
            Err(HostSignal::Error(error)) => assert_eq!(
                error.message, "an object does not understand #foo",
                "{lowering:?}"
            ),
            Err(HostSignal::Return) => panic!("{lowering:?}: a non-local return escaped"),
            Ok(_) => panic!("{lowering:?}: `nil foo` must fail"),
        }
        let fine = expect_ok(
            at_the_floor(&mut interpreter, lowering, &outer, "fine", &[]),
            "fine",
        );
        assert_eq!(
            interpreter.enter(|mutator| small_integer(mutator.fetch(&fine))),
            42,
            "{lowering:?}"
        );
    }
}

/// `receiver selector arguments`, sent from Rust: whatever it answers, it must
/// leave the activation stack empty and the counted depth 0.
fn at_the_floor(
    interpreter: &mut Interpreter,
    lowering: Lowering,
    receiver: &Root,
    selector: &str,
    arguments: &[Root],
) -> HostResult {
    let result = interpreter.send(receiver, Symbol::intern(selector), arguments);
    interpreter.enter(|mutator| {
        assert_eq!(
            mutator.heap().activation_depth(),
            0,
            "{selector} under {lowering:?}"
        );
        assert_eq!(
            mutator.heap().counted_depth(),
            0,
            "{selector} under {lowering:?}"
        );
    });
    result
}

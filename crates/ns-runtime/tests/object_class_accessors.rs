//! §3.2's accessors for the predefined classes on `Object`
//! (`docs/design/core-library.md`, "The predefined classes are accessors
//! on `Object`").

use indoc::indoc;

use ns_runtime::{Heap, HostResult, HostSignal, Interpreter, Lowering, Root, Symbol, Value};

fn load(source: &str) -> (Interpreter, Root, ns_runtime::ResolvedModule) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    (interpreter, class, resolved)
}

/// `HostSignal` carries no `Debug` impl, so `Result::expect` isn't usable on
/// a `HostResult` -- unwrap by hand, naming which variant leaked. Copied
/// from `tests/mixin.rs`.
fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// See `expect_ok`: copied from `tests/mixin.rs` for the same reason.
fn expect_error(result: HostResult, what: &str) -> String {
    match result {
        Ok(_) => panic!("{what}: expected a Newspeak error"),
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => error.message,
    }
}

/// §3.2: `Object` provides accessors for the predefined classes. An
/// explicit `self String` reaches `Object`'s accessor and answers the very
/// class a string literal has.
#[test]
fn an_explicit_self_send_of_a_class_name_answers_the_builtin_class() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Probe = Object new () (
            public same = ( ^(self String) == 'text' class )
            public array = ( ^(self Array) new size )
        ) : ()
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let probe = expect_ok(interpreter.instantiate(&class, &[]), "Probe new");
    let same = expect_ok(
        interpreter.send(&probe, Symbol::intern("same"), &[]),
        "same",
    );
    let size = expect_ok(
        interpreter.send(&probe, Symbol::intern("array"), &[]),
        "array",
    );
    interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&same), Value::Boolean(true)));
        assert!(matches!(mutator.fetch(&size), Value::SmallInteger(0)));
    });
}

/// A class that declares a member of the same name reaches its own member:
/// the fast path is read off the method the lookup found, so it never
/// answers for a lookup that found something else.
#[test]
fn a_class_that_redefines_the_name_reaches_its_own_method() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Probe = Object new () (
            public String = ( ^42 )
            public answer = ( ^self String )
        ) : ()
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let probe = expect_ok(interpreter.instantiate(&class, &[]), "Probe new");
    let answer = expect_ok(
        interpreter.send(&probe, Symbol::intern("answer"), &[]),
        "answer",
    );
    interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&answer), Value::SmallInteger(42)));
    });
}

/// §3.2 lets only `Object` name `Top`, so `Object` declares no accessor for
/// it: the send is not understood.
#[test]
fn object_declares_no_accessor_for_top() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Probe = Object new () (
            public top = ( ^self Top )
        ) : ()
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let probe = expect_ok(interpreter.instantiate(&class, &[]), "Probe new");
    let message = expect_error(interpreter.send(&probe, Symbol::intern("top"), &[]), "top");
    assert!(message.contains("Top"), "{message}");
}

fn as_string<'gc>(heap: &Heap<'gc>, value: &Value<'gc>) -> String {
    match value {
        Value::Object(object) => heap
            .str_value(*object)
            .unwrap_or_else(|| panic!("expected a String, got {}", heap.describe(value))),
        other => panic!("expected a String, got {}", heap.describe(other)),
    }
}

/// A bare builtin name in a user method is `self String`, answered by
/// `Object`'s accessor, and the kernel's own methods -- which reach a
/// predefined class through the kernel's nested class, found lexically --
/// keep running: `3 printString` answers `'3'`, and a bare `String` is the
/// class of a string literal, under both lowerings.
#[test]
fn a_bare_builtin_name_and_the_kernel_answer_alike_under_both_lowerings() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Probe = Object new () (
            public printed = ( ^3 printString )
            public same = ( ^String == 'text' class )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = ns_runtime::resolve(
            module,
            source.into(),
            ns_runtime::SourceOrigin::File("test.ns".into()),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        let probe = expect_ok(interpreter.instantiate(&class, &[]), "Probe new");
        let printed = expect_ok(
            interpreter.send(&probe, Symbol::intern("printed"), &[]),
            "printed",
        );
        let same = expect_ok(
            interpreter.send(&probe, Symbol::intern("same"), &[]),
            "same",
        );
        interpreter.enter(|mutator| {
            assert_eq!(
                as_string(mutator.heap(), &mutator.fetch(&printed)),
                "3",
                "{lowering:?}"
            );
            assert!(
                matches!(mutator.fetch(&same), Value::Boolean(true)),
                "{lowering:?}"
            );
        });
    }
}

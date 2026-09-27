//! Unit-level tests for `Interpreter::platform` and `crate::platform`'s
//! dispatch. `crates/ns-cli/tests/run.rs` covers the full,
//! real-stdout end-to-end path through `ns-next run`; these tests exercise
//! the same dispatch chain directly via `Interpreter::send`, without
//! spawning a process, and check the singleton/identity behavior a
//! process-level test can't easily observe.

use indoc::indoc;
use ns_runtime::{BuiltinClass, HostResult, HostSignal, Interpreter, Root, Symbol, Value};

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

#[test]
fn platform_is_memoized_across_calls() {
    let mut interpreter = Interpreter::bootstrap();
    let first = interpreter.platform();
    let second = interpreter.platform();
    interpreter.enter(
        |mutator| match (mutator.fetch(&first), mutator.fetch(&second)) {
            (Value::Object(first_object), Value::Object(second_object)) => {
                assert!(first_object.same(second_object))
            }
            _ => panic!("expected Value::Object"),
        },
    );
}

#[test]
fn platform_transcript_show_and_new_line_round_trip() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();

    let transcript = expect_ok(
        interpreter.send(&platform, Symbol::intern("transcript"), &[]),
        "platform transcript",
    );

    let string = interpreter.enter(|mutator| {
        let object = mutator.new_string("hi".to_string());
        mutator.stash(Value::Object(object))
    });
    let after_show = expect_ok(
        interpreter.send(&transcript, Symbol::intern("show:"), &[string]),
        "transcript show: 'hi'",
    );
    // `show:` returns the receiver (the `Transcript`-style convention
    // `platform.rs`'s doc comment describes).
    interpreter.enter(
        |mutator| match (mutator.fetch(&after_show), mutator.fetch(&transcript)) {
            (Value::Object(after_show_object), Value::Object(transcript_object)) => {
                assert!(after_show_object.same(transcript_object))
            }
            _ => panic!("expected Value::Object"),
        },
    );

    expect_ok(
        interpreter.send(&transcript, Symbol::intern("newLine"), &[]),
        "transcript newLine",
    );
}

/// `platform Array` and the rest answer the builtin class through
/// `Object`'s accessors (§3.2), which the platform inherits like every
/// object: it declares none of its own.
#[test]
fn platform_class_accessors_return_the_matching_builtin_class() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();

    for (name, kind) in [
        ("Object", BuiltinClass::Object),
        ("Array", BuiltinClass::Array),
        ("OrderedCollection", BuiltinClass::OrderedCollection),
        ("String", BuiltinClass::String),
        ("Symbol", BuiltinClass::Symbol),
    ] {
        let got = expect_ok(
            interpreter.send(&platform, Symbol::intern(name), &[]),
            &format!("platform {name}"),
        );
        interpreter.enter(|mutator| {
            let expected = mutator.builtin_class(kind);
            match mutator.fetch(&got) {
                Value::Object(object) => assert!(object.same(expected), "platform {name}"),
                other => panic!(
                    "expected Value::Object, got {}",
                    mutator.heap().describe(&other)
                ),
            }
        });
    }
}

/// `platform collections List` exists because three whitelisted corpus
/// modules open with `platform collections List` in their module state
/// (`corpus/runnable.toml`). It names the Core Library's growable ordered
/// sequence, which this runtime already has as `OrderedCollection`.
#[test]
fn platform_collections_answers_list_with_the_ordered_collection_class() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();

    let collections = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    let got = expect_ok(
        interpreter.send(&collections, Symbol::intern("List"), &[]),
        "platform collections List",
    );
    interpreter.enter(|mutator| {
        let expected = mutator.builtin_class(BuiltinClass::OrderedCollection);
        match mutator.fetch(&got) {
            Value::Object(object) => assert!(object.same(expected)),
            _ => panic!("expected a class object"),
        }
    });
}

/// The namespace answers exactly what a whitelisted module asks for, and
/// nothing more: `Map` (and the rest of the corpus's collections protocol)
/// names a class this runtime does not have, and must fail loudly rather
/// than quietly alias onto some other collection.
#[test]
fn platform_collections_does_not_invent_a_map() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();

    let collections = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    match interpreter.send(&collections, Symbol::intern("Map"), &[]) {
        Err(HostSignal::Error(error)) => assert!(
            error.message.contains("does not understand"),
            "message was: {}",
            error.message
        ),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return"),
        Ok(_) => panic!("expected `collections Map` to be refused"),
    }
}

/// The transcript and the collections namespace are instances of classes of
/// their own, so neither answers the other's protocol.
#[test]
fn the_collections_namespace_is_not_a_transcript() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();

    let collections = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    match interpreter.send(&collections, Symbol::intern("newLine"), &[]) {
        Err(HostSignal::Error(error)) => assert!(
            error.message.contains("does not understand"),
            "message was: {}",
            error.message
        ),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return"),
        Ok(_) => panic!("expected `collections newLine` to be refused"),
    }
}

/// The platform is an instance of `PlatformFactory.ns`'s nested `Platform`,
/// built by the host at bootstrap, and its members are instances of that
/// module's other nested classes.
#[test]
fn the_platform_is_an_instance_of_the_shipped_platform_module() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();
    let platform_class = expect_ok(
        interpreter.send(&platform, Symbol::intern("class"), &[]),
        "platform class",
    );
    let platform_class_name = expect_ok(
        interpreter.send(&platform_class, Symbol::intern("name"), &[]),
        "platform class name",
    );
    let transcript = expect_ok(
        interpreter.send(&platform, Symbol::intern("transcript"), &[]),
        "platform transcript",
    );
    let transcript_class = expect_ok(
        interpreter.send(&transcript, Symbol::intern("class"), &[]),
        "platform transcript class",
    );
    let transcript_class_name = expect_ok(
        interpreter.send(&transcript_class, Symbol::intern("name"), &[]),
        "platform transcript class name",
    );
    interpreter.enter(|mutator| {
        for (root, expected) in [
            (&platform_class_name, "Platform"),
            (&transcript_class_name, "Transcript"),
        ] {
            let value = mutator.fetch(root);
            let Value::Object(object) = value else {
                panic!("expected a string, got {}", mutator.heap().describe(&value));
            };
            assert_eq!(mutator.heap().str_value(object).as_deref(), Some(expected));
        }
    });
}

/// The platform's members are lazy slots (§6.3.3): the host's boot runs the
/// factory and nothing more, so the transcript is built on the first
/// `platform transcript`, not at every start.
#[test]
fn the_platform_builds_its_members_on_first_use() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();
    let transcript_field = |interpreter: &mut Interpreter| {
        interpreter.enter(|mutator| {
            let Value::Object(platform) = mutator.fetch(&platform) else {
                panic!("the platform is an object");
            };
            let class = mutator.heap().class_of_object(platform);
            let application = mutator
                .heap()
                .class_application(class)
                .expect("the platform's class has an application");
            let position = application
                .mixin
                .slots
                .iter()
                .position(|slot| slot.name.as_str() == "transcript")
                .expect("the platform declares a transcript slot");
            let field = mutator
                .heap()
                .instance_field(platform, application.own_slot_base + position)
                .expect("the platform has a field per slot");
            matches!(field, Value::Nil)
        })
    };
    assert!(
        transcript_field(&mut interpreter),
        "the transcript is not built before its first use"
    );
    expect_ok(
        interpreter.send(&platform, Symbol::intern("transcript"), &[]),
        "platform transcript",
    );
    assert!(
        !transcript_field(&mut interpreter),
        "the transcript is kept once built"
    );
}

/// `Transcript` is a class of the platform module like any other, no
/// builtin, so a subclass of it inherits a working `show:`: the transcript
/// leaf reads only its string argument, and the capability is the mirror
/// the inherited method sends to. §6.1's superclass clause starts with a
/// name, so the module reaches `platform Transcript` through a method of
/// its own.
#[test]
fn a_subclass_of_the_transcript_class_writes_too() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Loudness usingPlatform: platform = Object <: Value (
            | private thePlatform = platform. |
        ) (
            private TranscriptClass = ( ^thePlatform Transcript )
            class Loud = TranscriptClass new () ()
            public speak = ( ^Loud new show: '' )
        ) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
        "usingPlatform:",
    );
    expect_ok(
        interpreter.send(&instance, Symbol::intern("speak"), &[]),
        "a Loud shows",
    );
}

/// The error message a refused send answers.
fn refusal(result: HostResult, what: &str) -> String {
    match result {
        Err(HostSignal::Error(error)) => error.message,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Ok(_) => panic!("{what}: was not refused"),
    }
}

/// The platform's class is `PlatformFactory`'s nested `Platform`, whose
/// factory is `new`: the factory that takes the mirror belongs to the module
/// root, which no program is handed, so the platform's class has no such
/// message to understand at all.
#[test]
fn the_platform_class_takes_no_mirror() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();
    let platform_class = expect_ok(
        interpreter.send(&platform, Symbol::intern("class"), &[]),
        "platform class",
    );
    let not_the_mirror = interpreter.stash(Value::SmallInteger(42));
    let not_the_kernel = interpreter.stash(Value::SmallInteger(7));
    let message = refusal(
        interpreter.send(
            &platform_class,
            Symbol::intern("usingVmMirror:kernel:"),
            &[not_the_mirror, not_the_kernel],
        ),
        "platform class usingVmMirror:kernel:",
    );
    assert!(
        message.contains("does not understand"),
        "message was: {message}"
    );
}

/// The platform's class is enclosed by the module instance the host built
/// with the mirror, and is no module root itself: nothing a program can
/// name answers that instance.
#[test]
fn the_platform_class_is_nested_in_the_platform_factory() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();
    interpreter.enter(|mutator| {
        let Value::Object(platform) = mutator.fetch(&platform) else {
            panic!("the platform is an object");
        };
        let heap = mutator.heap();
        let platform_class = heap.class_of_object(platform);
        let factory = heap
            .class_enclosing(platform_class)
            .expect("the platform's class is a nested class");
        let factory_class = heap.class_of_object(factory);
        assert_eq!(heap.class_display_name(factory_class), "PlatformFactory");
        assert!(
            heap.class_enclosing(factory_class).is_none(),
            "PlatformFactory is the module's root"
        );
    });
}

/// A second platform, and a subclass of the platform's class, are ordinary
/// instances of shipped classes: their members reach the module's classes
/// lexically, so their transcripts write like the first platform's.
#[test]
fn a_subclass_of_the_platform_class_writes_too() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Copier usingPlatform: platform = Object <: Value (
            | private thePlatform = platform. |
        ) (
            private PlatformClass = ( ^thePlatform class )
            class Copied = PlatformClass new () ()
            public speak = ( ^Copied new transcript show: '' )
            public speakAgain = ( ^PlatformClass new transcript show: '' )
        ) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
        "usingPlatform:",
    );
    expect_ok(
        interpreter.send(&instance, Symbol::intern("speak"), &[]),
        "a Copied platform shows",
    );
    expect_ok(
        interpreter.send(&instance, Symbol::intern("speakAgain"), &[]),
        "a second platform shows",
    );
}

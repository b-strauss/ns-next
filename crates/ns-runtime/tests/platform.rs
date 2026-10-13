//! Unit-level tests for `Interpreter::platform` and `crate::platform`'s
//! dispatch. `crates/ns-cli/tests/run.rs` covers the full,
//! real-stdout end-to-end path through `ns-next run`; these tests exercise
//! the same dispatch chain directly via `Interpreter::send`, without
//! spawning a process, and check the singleton/identity behavior a
//! process-level test can't easily observe.

use indoc::indoc;
use ns_runtime::image::shipped_names::{self, ShippedKind, ShippedName};
use ns_runtime::{
    BuiltinClass, HostResult, HostSignal, Interpreter, Root, Symbol, Value, ValueKind,
};

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// The class of `subject`, read on the host side, since `class` is
/// answered only to its receiver's own sends.
fn class_of(interpreter: &mut Interpreter, subject: &Root) -> Root {
    interpreter.enter(|mutator| {
        let class = mutator.class_of(mutator.fetch(subject));
        mutator.stash(Value::object(class))
    })
}

#[test]
fn platform_is_memoized_across_calls() {
    let mut interpreter = Interpreter::bootstrap();
    let first = interpreter.platform();
    let second = interpreter.platform();
    interpreter.enter(|mutator| {
        match (
            mutator.fetch(&first).as_object(),
            mutator.fetch(&second).as_object(),
        ) {
            (Some(first_object), Some(second_object)) => {
                assert!(first_object.same(second_object))
            }
            _ => panic!("expected an object"),
        }
    });
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
        mutator.stash(Value::object(object))
    });
    let after_show = expect_ok(
        interpreter.send(&transcript, Symbol::intern("show:"), &[string]),
        "transcript show: 'hi'",
    );
    // `show:` returns the receiver (the `Transcript`-style convention
    // `platform.rs`'s doc comment describes).
    interpreter.enter(|mutator| {
        match (
            mutator.fetch(&after_show).as_object(),
            mutator.fetch(&transcript).as_object(),
        ) {
            (Some(after_show_object), Some(transcript_object)) => {
                assert!(after_show_object.same(transcript_object))
            }
            _ => panic!("expected an object"),
        }
    });

    expect_ok(
        interpreter.send(&transcript, Symbol::intern("newLine"), &[]),
        "transcript newLine",
    );
}

/// A bare `Object`, `Array`, `String` and `Symbol` in a module's method
/// answers the matching builtin class through `Object`'s accessors
/// (§3.2), which the module inherits. The accessors are protected, so the
/// module sends them to itself, as a bare name does.
#[test]
fn a_bare_class_name_answers_the_matching_builtin_class() {
    let source = indoc! {"
        class Probe usingPlatform: platform = Object new () (
            public objectClass = ( ^Object )
            public arrayClass = ( ^Array )
            public stringClass = ( ^String )
            public symbolClass = ( ^Symbol )
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
    let probe = expect_ok(
        interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
        "usingPlatform:",
    );

    for (name, kind) in [
        ("objectClass", BuiltinClass::Object),
        ("arrayClass", BuiltinClass::Array),
        ("stringClass", BuiltinClass::String),
        ("symbolClass", BuiltinClass::Symbol),
    ] {
        let got = expect_ok(interpreter.send(&probe, Symbol::intern(name), &[]), name);
        interpreter.enter(|mutator| {
            let expected = mutator.builtin_class(kind);
            match mutator.fetch(&got).kind() {
                ValueKind::Object(object) => assert!(object.same(expected), "{name}"),
                _ => panic!(
                    "expected an object, got {}",
                    mutator.heap().describe(mutator.fetch(&got))
                ),
            }
        });
    }
}

/// `platform collections List` exists because three whitelisted corpus
/// modules open with `platform collections List` in their module state
/// (`corpus/runnable.toml`). It is `CollectionsFactory.ns`'s `List`, a class
/// compiled from shipped Newspeak source and no builtin, handed out by the
/// collections facet; every platform shares one collections module, so a
/// second platform, made by the host, answers the same `List`.
#[test]
fn platform_collections_answers_the_collections_modules_list() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();

    let collections = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    let list = expect_ok(
        interpreter.send(&collections, Symbol::intern("List"), &[]),
        "platform collections List",
    );
    let platform_class = class_of(&mut interpreter, &platform);
    let second = expect_ok(
        interpreter.send(&platform_class, Symbol::NEW, &[]),
        "platform class new",
    );
    let second_collections = expect_ok(
        interpreter.send(&second, Symbol::intern("collections"), &[]),
        "second collections",
    );
    let second_list = expect_ok(
        interpreter.send(&second_collections, Symbol::intern("List"), &[]),
        "second collections List",
    );
    interpreter.enter(|mutator| {
        let object_of = |root: &Root| match mutator.fetch(root).kind() {
            ValueKind::Object(object) => object,
            _ => panic!(
                "expected a class object, got {}",
                mutator.heap().describe(mutator.fetch(root))
            ),
        };
        let list = object_of(&list);
        let second_list = object_of(&second_list);
        let heap = mutator.heap();
        assert!(heap.is_class(list), "platform collections List is a class");
        assert_eq!(heap.class_builtin(list), None, "List is no builtin");
        assert!(heap.class_is_shipped(list), "List is shipped source");
        assert_eq!(heap.class_display_name(list), "List");
        assert!(
            second_list.same(list),
            "a second platform answers the same List"
        );
    });
}

/// Every shipped module has one instance, built at boot: the collections
/// module's instance is in the shipped world before anything asked for
/// `platform collections` or for the platform at all, and both platforms'
/// facets are of that very instance.
#[test]
fn the_collections_module_is_built_once_at_boot() {
    let mut interpreter = Interpreter::bootstrap();
    let at_boot = interpreter.enter(|mutator| {
        let name = ShippedName {
            module: Symbol::intern("CollectionsFactory"),
            declaration: Symbol::intern("CollectionsFactory"),
            kind: ShippedKind::ModuleInstance,
        };
        let resolved = match shipped_names::resolve_in(mutator, &[name], false) {
            Ok(resolved) => resolved,
            Err(error) => panic!("the collections module is built at boot: {error}"),
        };
        mutator.stash(Value::object(resolved[0].object))
    });
    let platform = interpreter.platform();
    let first = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    let second_platform = class_of(&mut interpreter, &platform);
    let second_platform = expect_ok(
        interpreter.send(&second_platform, Symbol::intern("new"), &[]),
        "platform class new",
    );
    let second = expect_ok(
        interpreter.send(&second_platform, Symbol::intern("collections"), &[]),
        "second platform collections",
    );
    interpreter.enter(|mutator| {
        let heap = mutator.heap();
        let module_of = |facet: &Root| {
            let facet = mutator
                .fetch(facet)
                .as_object()
                .expect("a facet is an object");
            heap.class_enclosing(heap.object_class(facet))
                .expect("a facet's class is nested")
        };
        let at_boot = mutator
            .fetch(&at_boot)
            .as_object()
            .expect("a module instance is an object");
        assert!(module_of(&first).same(at_boot));
        assert!(module_of(&second).same(at_boot));
    });
}

/// The namespace answers the collections module's classes and nothing
/// more: `IdentitySet`, which the corpus asks for, names a class this
/// runtime does not have, and must fail loudly rather than quietly alias
/// onto `Set` or `IdentityMap`.
#[test]
fn platform_collections_does_not_invent_an_identity_set() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();

    let collections = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    match interpreter.send(&collections, Symbol::intern("IdentitySet"), &[]) {
        Err(HostSignal::Error(error)) => assert!(
            error.message.contains("does not understand"),
            "message was: {}",
            error.message
        ),
        Err(HostSignal::Return) => panic!("unexpected HostSignal::Return"),
        Ok(_) => panic!("expected `collections IdentitySet` to be refused"),
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
    let platform_class = class_of(&mut interpreter, &platform);
    let platform_class_name = expect_ok(
        interpreter.send(&platform_class, Symbol::intern("name"), &[]),
        "platform class name",
    );
    let transcript = expect_ok(
        interpreter.send(&platform, Symbol::intern("transcript"), &[]),
        "platform transcript",
    );
    let transcript_class = class_of(&mut interpreter, &transcript);
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
            let Some(object) = value.as_object() else {
                panic!("expected a string, got {}", mutator.heap().describe(value));
            };
            assert_eq!(mutator.heap().str_value(object).as_deref(), Some(expected));
        }
    });
}

/// The platform's members are lazy slots (§6.3.3): the host's boot runs the
/// factory and nothing more, so the transcript is built on the first
/// `platform transcript`, not at every start, and `platform time` likewise.
#[test]
fn the_platform_builds_its_members_on_first_use() {
    for member in ["transcript", "time"] {
        a_platform_member_is_built_on_first_use(member);
    }
}

fn a_platform_member_is_built_on_first_use(member: &str) {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();
    let member_field = |interpreter: &mut Interpreter| {
        interpreter.enter(|mutator| {
            let Some(platform) = mutator.fetch(&platform).as_object() else {
                panic!("the platform is an object");
            };
            let class = mutator.heap().object_class(platform);
            let application = mutator
                .heap()
                .class_application(class)
                .expect("the platform's class has an application");
            let position = application
                .mixin
                .slots
                .iter()
                .position(|slot| slot.name.as_str() == member)
                .unwrap_or_else(|| panic!("the platform declares a {member} slot"));
            let field = mutator
                .heap()
                .instance_field(platform, application.own_slot_base + position)
                .expect("the platform has a field per slot");
            field.is_nil()
        })
    };
    assert!(
        member_field(&mut interpreter),
        "the {member} is not built before its first use"
    );
    expect_ok(
        interpreter.send(&platform, Symbol::intern(member), &[]),
        &format!("platform {member}"),
    );
    assert!(
        !member_field(&mut interpreter),
        "the {member} is kept once built"
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
    let platform_class = class_of(&mut interpreter, &platform);
    let not_the_mirror = interpreter.stash(Value::small_integer(42));
    let not_the_kernel = interpreter.stash(Value::small_integer(7));
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
        let Some(platform) = mutator.fetch(&platform).as_object() else {
            panic!("the platform is an object");
        };
        let heap = mutator.heap();
        let platform_class = heap.object_class(platform);
        let factory = heap
            .class_enclosing(platform_class)
            .expect("the platform's class is a nested class");
        let factory_class = heap.object_class(factory);
        assert_eq!(heap.class_display_name(factory_class), "PlatformFactory");
        assert!(
            heap.class_enclosing(factory_class).is_none(),
            "PlatformFactory is the module's root"
        );
    });
}

/// A second platform, made by the host from the platform's class, is an
/// ordinary instance of a shipped class: its members reach the module's
/// classes lexically, so its transcript writes like the first platform's.
/// No program makes one: the platform hands out none of its module's
/// classes, and `class` is protected.
#[test]
fn a_second_platform_writes_too() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();
    let platform_class = class_of(&mut interpreter, &platform);
    let second = expect_ok(
        interpreter.send(&platform_class, Symbol::NEW, &[]),
        "platform class new",
    );
    let transcript = expect_ok(
        interpreter.send(&second, Symbol::intern("transcript"), &[]),
        "second transcript",
    );
    let empty = interpreter.enter(|mutator| {
        let text = mutator.new_string(String::new());
        mutator.stash(Value::object(text))
    });
    expect_ok(
        interpreter.send(&transcript, Symbol::intern("show:"), &[empty]),
        "a second platform shows",
    );
}

/// Whether `left = right` answers true.
fn equals(interpreter: &mut Interpreter, left: &Root, right: &Root) -> bool {
    let answer = expect_ok(
        interpreter.send(left, Symbol::intern("="), std::slice::from_ref(right)),
        "=",
    );
    interpreter.enter(|mutator| mutator.fetch(&answer).as_boolean() == Some(true))
}

/// Whether `left == right` answers true.
fn identical(interpreter: &mut Interpreter, left: &Root, right: &Root) -> bool {
    let answer = expect_ok(
        interpreter.send(left, Symbol::intern("=="), std::slice::from_ref(right)),
        "==",
    );
    interpreter.enter(|mutator| mutator.fetch(&answer).as_boolean() == Some(true))
}

/// The platform, its transcript and its collections facet are values
/// (§3.1.1): a second of each, made by the host from the first one's class
/// or a second platform's members, is equal to the first, hashes alike and,
/// a value having no identity, is identical; and every platform's facet
/// comes from the one collections module, so it hands out the same `List`.
/// No program makes a second one (`goldens/library/platform/the_platform_and_its_parts_are_values.ns`).
#[test]
fn the_platform_and_its_parts_are_values() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();
    let platform_class = class_of(&mut interpreter, &platform);
    let second = expect_ok(
        interpreter.send(&platform_class, Symbol::NEW, &[]),
        "platform class new",
    );
    let member = |interpreter: &mut Interpreter, receiver: &Root, name: &str| {
        expect_ok(interpreter.send(receiver, Symbol::intern(name), &[]), name)
    };
    let transcript = member(&mut interpreter, &platform, "transcript");
    let transcript_class = class_of(&mut interpreter, &transcript);
    let fresh_transcript = expect_ok(
        interpreter.send(&transcript_class, Symbol::NEW, &[]),
        "transcript class new",
    );
    let second_transcript = member(&mut interpreter, &second, "transcript");
    let collections = member(&mut interpreter, &platform, "collections");
    let second_collections = member(&mut interpreter, &second, "collections");
    assert!(
        equals(&mut interpreter, &transcript, &fresh_transcript),
        "a fresh transcript equals the platform's"
    );
    assert!(
        equals(&mut interpreter, &second_transcript, &transcript),
        "a second platform's transcript equals the first's"
    );
    assert!(
        equals(&mut interpreter, &platform, &second),
        "a second platform equals the first"
    );
    let hash = member(&mut interpreter, &platform, "hash");
    let second_hash = member(&mut interpreter, &second, "hash");
    assert!(
        identical(&mut interpreter, &hash, &second_hash),
        "a second platform hashes as the first"
    );
    assert!(
        identical(&mut interpreter, &second_collections, &collections),
        "a second platform's facet is the first's, a value"
    );
    let list = member(&mut interpreter, &collections, "List");
    let second_list = member(&mut interpreter, &second_collections, "List");
    assert!(
        identical(&mut interpreter, &second_list, &list),
        "every platform's facet hands out the one List"
    );
}

/// The module instance behind the platform: the enclosing object of the
/// platform's class, which no program can name. Only a host test reaches it,
/// through the heap.
fn the_platform_factory(interpreter: &mut Interpreter) -> Root {
    let platform = interpreter.platform();
    interpreter.enter(|mutator| {
        let Some(platform) = mutator.fetch(&platform).as_object() else {
            panic!("the platform is an object");
        };
        let heap = mutator.heap();
        let factory = heap
            .class_enclosing(heap.object_class(platform))
            .expect("the platform's class is a nested class");
        mutator.stash(Value::object(factory))
    })
}

/// `PlatformFactory`'s nested classes are private, and its one public
/// member is `build`: a public send of a class's name to the factory
/// instance finds nothing, even for a host that holds it, while `build`
/// makes a platform. A second line of defence behind the factory instance
/// being unreachable.
#[test]
fn the_platform_factory_answers_only_build() {
    let mut interpreter = Interpreter::bootstrap();
    let factory = the_platform_factory(&mut interpreter);
    for name in [
        "Platform",
        "Transcript",
        "KernelNamespace",
        "Time",
        "Stopwatch",
    ] {
        let message = refusal(
            interpreter.send(&factory, Symbol::intern(name), &[]),
            &format!("factory {name}"),
        );
        assert!(
            message.contains("does not understand"),
            "factory {name}: {message}"
        );
    }
    let platform = expect_ok(
        interpreter.send(&factory, Symbol::intern("build"), &[]),
        "factory build",
    );
    let transcript = expect_ok(
        interpreter.send(&platform, Symbol::intern("transcript"), &[]),
        "build transcript",
    );
    let text = interpreter.enter(|mutator| {
        let text = Value::object(mutator.new_string(String::new()));
        mutator.stash(text)
    });
    expect_ok(
        interpreter.send(&transcript, Symbol::intern("show:"), &[text]),
        "build transcript show: ''",
    );
}

/// `CollectionsFactory` is written in the same factory pattern: its nested
/// classes are private and its one public member is `build`. A
/// host reaches its instance only as the enclosing object of the facet's
/// class, which no program can walk; even then the classes' names find
/// nothing, while `build` makes a facet whose `List` answers a
/// class. A second line of defence behind the factory instance being
/// unreachable, as for `PlatformFactory`.
#[test]
fn the_collections_factory_answers_only_build() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();
    let collections = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    let factory = interpreter.enter(|mutator| {
        let Some(collections) = mutator.fetch(&collections).as_object() else {
            panic!("the collections facet is an object");
        };
        let heap = mutator.heap();
        let factory = heap
            .class_enclosing(heap.object_class(collections))
            .expect("the facet's class is a nested class");
        assert_eq!(
            heap.class_display_name(heap.object_class(factory)),
            "CollectionsFactory"
        );
        mutator.stash(Value::object(factory))
    });
    for name in ["Collections", "List"] {
        let message = refusal(
            interpreter.send(&factory, Symbol::intern(name), &[]),
            &format!("collections factory {name}"),
        );
        assert!(
            message.contains("does not understand"),
            "collections factory {name}: {message}"
        );
    }
    let facet = expect_ok(
        interpreter.send(&factory, Symbol::intern("build"), &[]),
        "collections factory build",
    );
    let list = expect_ok(
        interpreter.send(&facet, Symbol::intern("List"), &[]),
        "build List",
    );
    interpreter.enter(|mutator| {
        let value = mutator.fetch(&list);
        let Some(list) = value.as_object() else {
            panic!("expected a class, got {}", mutator.heap().describe(value));
        };
        assert!(mutator.heap().is_class(list), "build List is a class");
    });
}

/// The platform hands out none of its module's classes, and the
/// collections facet not its own: an outside send of the class's name finds
/// nothing, and none of them may stand as a value in an image, since no
/// program reaches one.
#[test]
fn the_platform_hands_out_none_of_its_modules_classes() {
    let mut interpreter = Interpreter::bootstrap();
    let platform = interpreter.platform();
    let collections = expect_ok(
        interpreter.send(&platform, Symbol::intern("collections"), &[]),
        "platform collections",
    );
    for (receiver, module, name) in [
        (&platform, "PlatformFactory", "Platform"),
        (&platform, "TranscriptFactory", "Transcript"),
        (&platform, "PlatformFactory", "KernelNamespace"),
        (&platform, "TimeFactory", "Time"),
        (&platform, "TimeFactory", "Stopwatch"),
        (&collections, "CollectionsFactory", "Collections"),
    ] {
        let message = refusal(
            interpreter.send(receiver, Symbol::intern(name), &[]),
            &format!("{name} sent to a platform member"),
        );
        assert!(message.contains("does not understand"), "{name}: {message}");
        interpreter.enter(|mutator| {
            let shipped = ShippedName {
                module: Symbol::intern(module),
                declaration: Symbol::intern(name),
                kind: ShippedKind::Class,
            };
            let resolved = match shipped_names::resolve_in(mutator, &[shipped], false) {
                Ok(resolved) => resolved,
                Err(error) => panic!("{} is a shipped name: {error}", shipped.describe()),
            };
            assert_eq!(
                resolved[0].positions.value,
                name == "Stopwatch",
                "{} as a value",
                shipped.describe()
            );
        });
    }
}

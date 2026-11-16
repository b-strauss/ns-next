//! `ByteArray`'s builtin class and its leaves
//! (`crate::builtins::bytes`), through `Interpreter::send` -- same pattern
//! as `tests/array_leaves.rs`.

use indoc::indoc;
use ns_runtime::{BuiltinClass, HostResult, HostSignal, Interpreter, Root, Symbol};

/// Parses+resolves a one-class module and loads it into a freshly
/// bootstrapped `Interpreter`, as `tests/array_leaves.rs` does.
fn load(source: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
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

/// Sends the unary `selector` to a new instance of `source`'s class and
/// answers the interpreter and the answer.
fn answer(source: &str, selector: &str) -> (Interpreter, Root) {
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let result = interpreter.send(&receiver, Symbol::intern(selector), &[]);
    let result = expect_ok(result, selector);
    (interpreter, result)
}

#[test]
fn a_byte_array_is_an_instance_of_the_builtin_byte_array() {
    let source = indoc! {"
        class Foo = Object new () (
            public make = ( ^ByteArray new: 2 )
        ) : ()
    "};
    let (mut interpreter, result) = answer(source, "make");
    interpreter.enter(|mutator| {
        let value = mutator.fetch(&result);
        let byte_array = mutator.builtin_class(BuiltinClass::ByteArray);
        assert!(
            mutator.class_of(value).same(byte_array),
            "`ByteArray new: 2` is a ByteArray"
        );
        let object = value.as_object().expect("an object");
        assert_eq!(mutator.heap().byte_array_bytes(object), Some(vec![0, 0]));
    });
}

/// §3.2's accessor on `Object`: protected, so a program names the class
/// only from inside, and it answers the kernel's class itself.
#[test]
fn objects_byte_array_accessor_is_protected_and_answers_the_class() {
    let source = indoc! {"
        class Foo = Object new () (
            public mine = ( ^ByteArray )
            public theirs = ( ^Object new ByteArray )
        ) : ()
    "};
    let (mut interpreter, result) = answer(source, "mine");
    interpreter.enter(|mutator| {
        let value = mutator.fetch(&result);
        let byte_array = mutator.builtin_class(BuiltinClass::ByteArray);
        assert!(
            value
                .as_object()
                .is_some_and(|class| class.same(byte_array))
        );
    });
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    match interpreter.send(&receiver, Symbol::intern("theirs"), &[]) {
        Err(HostSignal::Error(error)) => assert!(
            error.message.contains("ByteArray"),
            "a send of the protected accessor from outside is not understood: {}",
            error.message
        ),
        Err(HostSignal::Return) => panic!("theirs: a non-local return escaped"),
        Ok(_) => panic!("another object's ByteArray accessor answered"),
    }
}

/// One of 32 KiB or more is allocated old, as every object of 32 KiB or
/// more is, and a new one holds zeros whichever generation it lies in.
#[test]
fn a_large_byte_array_is_allocated_old_and_holds_zeros() {
    let source = indoc! {"
        class Foo = Object new () (
            public make = ( ^ByteArray new: 40000 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let receiver = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let before = interpreter.heap_statistics();
    let result = expect_ok(
        interpreter.send(&receiver, Symbol::intern("make"), &[]),
        "make",
    );
    let after = interpreter.heap_statistics();
    assert_eq!(
        after.live_objects,
        before.live_objects + 1,
        "one object made"
    );
    assert_eq!(
        after.young_objects, before.young_objects,
        "the ByteArray is not in the nursery"
    );
    interpreter.enter(|mutator| {
        let value = mutator.fetch(&result);
        let object = value.as_object().expect("an object");
        let bytes = mutator
            .heap()
            .byte_array_bytes(object)
            .expect("a ByteArray");
        assert_eq!(bytes.len(), 40000);
        assert!(bytes.iter().all(|byte| *byte == 0));
    });
}

/// `Mutator::new_byte_array` makes a `ByteArray` of the kernel's class
/// holding the bytes given.
#[test]
fn new_byte_array_holds_its_bytes() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let object = mutator.new_byte_array(&[1, 2, 255]);
        let byte_array = mutator.builtin_class(BuiltinClass::ByteArray);
        assert!(mutator.heap().object_class(object).same(byte_array));
        assert_eq!(
            mutator.heap().byte_array_bytes(object),
            Some(vec![1, 2, 255])
        );
    });
}

/// The bytes of the `ByteArray` `selector` answers.
fn answered_bytes(source: &str, selector: &str) -> Vec<u8> {
    let (mut interpreter, result) = answer(source, selector);
    interpreter.enter(|mutator| {
        let value = mutator.fetch(&result);
        let byte_array = mutator.builtin_class(BuiltinClass::ByteArray);
        assert!(
            mutator.class_of(value).same(byte_array),
            "{selector} answers a ByteArray"
        );
        let object = value.as_object().expect("an object");
        mutator
            .heap()
            .byte_array_bytes(object)
            .expect("a ByteArray")
    })
}

/// `copyFrom:to:` and `,` answer new ByteArrays of the kernel's class, a
/// String argument counting as its UTF-8 bytes, and leave the receiver as
/// it was.
#[test]
fn a_copy_and_a_concatenation_are_new_byte_arrays() {
    let source = indoc! {"
        class Foo = Object new (
            | bytes = ByteArray withAll: {1. 2. 3}. |
        ) (
            public copied = ( ^bytes copyFrom: 2 to: 3 )
            public joined = ( ^bytes , 'aé' )
            public joinedBytes = ( ^bytes , bytes )
            public unchanged = ( bytes copyFrom: 1 to: 2. bytes , 'x'. ^bytes )
        ) : ()
    "};
    assert_eq!(answered_bytes(source, "copied"), vec![2, 3]);
    assert_eq!(
        answered_bytes(source, "joined"),
        vec![1, 2, 3, 97, 195, 169]
    );
    assert_eq!(
        answered_bytes(source, "joinedBytes"),
        vec![1, 2, 3, 1, 2, 3]
    );
    assert_eq!(answered_bytes(source, "unchanged"), vec![1, 2, 3]);
}

/// `replaceFrom:to:with:startingAt:` from the receiver itself is right
/// whichever way the ranges overlap, and from a String reads its UTF-8
/// bytes, the start counted in bytes.
#[test]
fn a_replacement_moves_overlapping_bytes_and_encodes_a_string() {
    let source = indoc! {"
        class Foo = Object new () (
            public up = (
                | bytes = ByteArray withAll: {1. 2. 3. 4. 5. 6. 7. 8. 9}. |
                ^bytes replaceFrom: 3 to: 9 with: bytes startingAt: 1
            )
            public down = (
                | bytes = ByteArray withAll: {1. 2. 3. 4. 5. 6. 7. 8. 9}. |
                ^bytes replaceFrom: 1 to: 7 with: bytes startingAt: 3
            )
            public fromText = ( ^(ByteArray new: 5) replaceFrom: 2 to: 5 with: 'zé€' startingAt: 2 )
        ) : ()
    "};
    assert_eq!(
        answered_bytes(source, "up"),
        vec![1, 2, 1, 2, 3, 4, 5, 6, 7]
    );
    assert_eq!(
        answered_bytes(source, "down"),
        vec![3, 4, 5, 6, 7, 8, 9, 8, 9]
    );
    assert_eq!(
        answered_bytes(source, "fromText"),
        vec![0, 195, 169, 226, 130]
    );
}

/// A refused replacement writes nothing: every operand is checked before
/// the first byte is stored.
#[test]
fn a_refused_replacement_leaves_the_receiver_as_it_was() {
    let source = indoc! {"
        class Foo = Object new (
            | bytes = ByteArray withAll: {1. 2. 3}. |
        ) (
            public shortSource = (
                [bytes replaceFrom: 1 to: 3 with: 'é' startingAt: 1] on: Error do: [:error | nil].
                ^bytes
            )
            public shortSelf = (
                [bytes replaceFrom: 1 to: 2 with: bytes startingAt: 3] on: Error do: [:error | nil].
                ^bytes
            )
            public badRange = (
                [bytes replaceFrom: 2 to: 4 with: bytes startingAt: 1] on: Error do: [:error | nil].
                ^bytes
            )
        ) : ()
    "};
    for selector in ["shortSource", "shortSelf", "badRange"] {
        assert_eq!(
            answered_bytes(source, selector),
            vec![1, 2, 3],
            "{selector}"
        );
    }
}

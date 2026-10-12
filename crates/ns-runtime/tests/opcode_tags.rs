//! Pins every wire tag in the image encoding to its value.
//!
//! These assertions exist to fail. A tag is assigned once and never changes
//! (`image/opcodes.rs`), and this is the mechanism that makes that a promise
//! rather than a comment: renumbering an instruction or a literal breaks this
//! test at the moment it happens.
//!
//! Without it the failure is silent and late. `FORMAT_VERSION` is deliberately
//! never bumped, so a stale image is not refused for being stale; if a tag
//! changed meaning, its bytes would decode into a *different, perfectly valid*
//! instruction and run. The untrusted-input guarantee does not cover that --
//! it promises no panic, not no misreading.
//!
//! Adding an instruction is expected to add a line here with the next free
//! number. Changing a line that already exists is the thing being prevented.

use ns_runtime::image::decode::builtin_class_from_tag;
use ns_runtime::image::encode::builtin_class_tag;
use ns_runtime::image::opcodes::*;
use ns_runtime::BuiltinClass;

#[test]
fn instruction_tags_are_fixed() {
    assert_eq!(OPCODE_LOAD_LITERAL, 0);
    assert_eq!(OPCODE_LOAD_SELF, 1);
    assert_eq!(OPCODE_MOVE, 3);
    assert_eq!(OPCODE_STORE_LOCAL, 4);
    assert_eq!(OPCODE_LOAD_OUTER_LOCAL, 5);
    assert_eq!(OPCODE_STORE_OUTER_LOCAL, 6);
    assert_eq!(OPCODE_MAKE_ARRAY, 12);
    assert_eq!(OPCODE_INITIALIZE_SLOT, 13);
    assert_eq!(OPCODE_RETURN, 14);
    assert_eq!(OPCODE_IMPLICIT_RETURN, 15);
    assert_eq!(OPCODE_NON_LOCAL_RETURN, 16);
    assert_eq!(OPCODE_JUMP, 17);
    assert_eq!(OPCODE_JUMP_IF_NOT_BOOLEAN, 18);
    assert_eq!(OPCODE_BRANCH_IF_FALSE, 19);
    assert_eq!(OPCODE_BRANCH_IF_TRUE, 20);
    assert_eq!(OPCODE_SEND_LEXICAL, 25);
    assert_eq!(OPCODE_SEND_PUBLIC, 26);
    assert_eq!(OPCODE_SEND_INLINABLE, 27);
    assert_eq!(OPCODE_SEND_SUPER, 28);
    assert_eq!(OPCODE_MAKE_CLOSURE, 29);
    assert_eq!(OPCODE_MAKE_VECTOR, 30);
    assert_eq!(OPCODE_LOAD_VECTOR, 31);
    assert_eq!(OPCODE_STORE_VECTOR, 32);
    assert_eq!(OPCODE_INTEGER_BINARY, 33);
    assert_eq!(OPCODE_RETURN_SLOT_IF_FILLED, 34);
    assert_eq!(OPCODE_LOAD_ENCLOSING_OBJECT, 35);
}

/// §6.3.2's simultaneous slots, through pull futures: the four instructions
/// that make, resolve and provisionally store a future.
#[test]
fn the_simultaneous_slot_instructions_keep_their_numbers() {
    assert_eq!(OPCODE_MAKE_FUTURE, 36);
    assert_eq!(OPCODE_RESOLVE_FUTURE, 37);
    assert_eq!(OPCODE_LOAD_OWN_SLOT, 38);
    assert_eq!(OPCODE_INITIALIZE_SLOT_PROVISIONALLY, 39);
}

/// An inlined loop's guard failure, which signals `NonBooleanReceiver`.
#[test]
fn the_non_boolean_signal_keeps_its_number() {
    assert_eq!(OPCODE_SIGNAL_NON_BOOLEAN, 40);
}

#[test]
fn the_integer_binary_opcode_keeps_its_tag() {
    // A tag is assigned once and never renumbered: a stale artifact must hit
    // an unknown tag and be refused, never decode into a different, valid
    // program.
    assert_eq!(ns_runtime::image::opcodes::OPCODE_INTEGER_BINARY, 33);
}

#[test]
fn literal_tags_are_fixed() {
    assert_eq!(LITERAL_NIL, 0);
    assert_eq!(LITERAL_BOOLEAN, 1);
    assert_eq!(LITERAL_SMALL_INTEGER, 2);
    assert_eq!(LITERAL_FLOAT, 3);
    assert_eq!(LITERAL_STRING, 4);
    assert_eq!(LITERAL_SYMBOL, 5);
    assert_eq!(LITERAL_UNRESOLVED_IDENTIFIER, 9);
    assert_eq!(LITERAL_LARGE_INTEGER, 10);
    assert_eq!(LITERAL_BODY_TOO_LARGE, 11);
}

/// Two tags sharing a number would make the encoder ambiguous and the decoder
/// wrong, and neither the pinning tests above nor the compiler would notice:
/// the constants are independent `u8`s, so a copy-paste that repeats a number
/// is well-formed Rust.
#[test]
fn no_two_tags_share_a_number() {
    let opcodes = [
        OPCODE_LOAD_LITERAL,
        OPCODE_LOAD_SELF,
        OPCODE_MOVE,
        OPCODE_STORE_LOCAL,
        OPCODE_LOAD_OUTER_LOCAL,
        OPCODE_STORE_OUTER_LOCAL,
        OPCODE_SEND_LEXICAL,
        OPCODE_SEND_PUBLIC,
        OPCODE_SEND_INLINABLE,
        OPCODE_SEND_SUPER,
        OPCODE_MAKE_CLOSURE,
        OPCODE_MAKE_ARRAY,
        OPCODE_MAKE_VECTOR,
        OPCODE_LOAD_VECTOR,
        OPCODE_STORE_VECTOR,
        OPCODE_INITIALIZE_SLOT,
        OPCODE_RETURN,
        OPCODE_IMPLICIT_RETURN,
        OPCODE_NON_LOCAL_RETURN,
        OPCODE_JUMP,
        OPCODE_JUMP_IF_NOT_BOOLEAN,
        OPCODE_BRANCH_IF_FALSE,
        OPCODE_BRANCH_IF_TRUE,
        OPCODE_INTEGER_BINARY,
        OPCODE_RETURN_SLOT_IF_FILLED,
        OPCODE_LOAD_ENCLOSING_OBJECT,
        OPCODE_MAKE_FUTURE,
        OPCODE_RESOLVE_FUTURE,
        OPCODE_LOAD_OWN_SLOT,
        OPCODE_INITIALIZE_SLOT_PROVISIONALLY,
        OPCODE_SIGNAL_NON_BOOLEAN,
    ];
    let mut sorted = opcodes;
    sorted.sort_unstable();
    let mut deduplicated = sorted.to_vec();
    deduplicated.dedup();
    assert_eq!(
        deduplicated.len(),
        opcodes.len(),
        "duplicate instruction tag"
    );

    let literals = [
        LITERAL_NIL,
        LITERAL_BOOLEAN,
        LITERAL_SMALL_INTEGER,
        LITERAL_FLOAT,
        LITERAL_STRING,
        LITERAL_SYMBOL,
        LITERAL_UNRESOLVED_IDENTIFIER,
        LITERAL_LARGE_INTEGER,
        LITERAL_BODY_TOO_LARGE,
    ];
    let mut sorted = literals;
    sorted.sort_unstable();
    let mut deduplicated = sorted.to_vec();
    deduplicated.dedup();
    assert_eq!(deduplicated.len(), literals.len(), "duplicate literal tag");
    assert_eq!(RETIRED_LITERALS, [6, 7, 8]);
    for retired in RETIRED_LITERALS {
        assert!(
            !literals.contains(&retired),
            "literal tag {retired} is retired"
        );
    }
}

/// A retired tag stays retired: no live instruction takes its number
/// (`image/opcodes.rs`).
#[test]
fn retired_instruction_tags_are_never_reused() {
    assert_eq!(RETIRED_OPCODES, [2, 7, 8, 9, 10, 11, 21, 22, 23, 24]);
    let live = [
        OPCODE_LOAD_LITERAL,
        OPCODE_LOAD_SELF,
        OPCODE_MOVE,
        OPCODE_STORE_LOCAL,
        OPCODE_LOAD_OUTER_LOCAL,
        OPCODE_STORE_OUTER_LOCAL,
        OPCODE_SEND_LEXICAL,
        OPCODE_SEND_PUBLIC,
        OPCODE_SEND_INLINABLE,
        OPCODE_SEND_SUPER,
        OPCODE_MAKE_CLOSURE,
        OPCODE_MAKE_ARRAY,
        OPCODE_MAKE_VECTOR,
        OPCODE_LOAD_VECTOR,
        OPCODE_STORE_VECTOR,
        OPCODE_INITIALIZE_SLOT,
        OPCODE_RETURN,
        OPCODE_IMPLICIT_RETURN,
        OPCODE_NON_LOCAL_RETURN,
        OPCODE_JUMP,
        OPCODE_JUMP_IF_NOT_BOOLEAN,
        OPCODE_BRANCH_IF_FALSE,
        OPCODE_BRANCH_IF_TRUE,
        OPCODE_INTEGER_BINARY,
        OPCODE_RETURN_SLOT_IF_FILLED,
        OPCODE_LOAD_ENCLOSING_OBJECT,
        OPCODE_MAKE_FUTURE,
        OPCODE_RESOLVE_FUTURE,
        OPCODE_LOAD_OWN_SLOT,
        OPCODE_INITIALIZE_SLOT_PROVISIONALLY,
        OPCODE_SIGNAL_NON_BOOLEAN,
    ];
    for retired in RETIRED_OPCODES {
        assert!(!live.contains(&retired), "tag {retired} is retired");
    }
}

/// A retired object-body tag stays retired, for the same reason a retired
/// instruction tag does: the closure record's payload changed on both
/// halves, so tag 4's bytes would otherwise decode into a different,
/// perfectly valid closure; the class record's mixin became a tagged
/// reference, so tag 5's row id would otherwise be read as that tag; and the
/// activation record's frame id became an object id once frames joined the
/// object table, so tag 9's id would otherwise name some other record.
#[test]
fn retired_object_body_tags_are_refused() {
    assert_eq!(RETIRED_OBJECT_BODIES, [4, 5, 9]);
    assert_eq!(OBJECT_CLOSURE, 6);
    assert_eq!(OBJECT_CLASS, 7);
    let live = [
        OBJECT_INSTANCE,
        OBJECT_STRING,
        OBJECT_SYMBOL,
        OBJECT_ARRAY,
        OBJECT_CLOSURE,
        OBJECT_CLASS,
        OBJECT_METACLASS,
        OBJECT_FRAME,
    ];
    for retired in RETIRED_OBJECT_BODIES {
        assert!(!live.contains(&retired), "tag {retired} is retired");
    }
}

/// The graph section's tags, pinned for the same reason as the instruction
/// tags above: three readers now share this numbering -- the encoder, the
/// loader's decoder, and `inspect`'s structural walk -- and a renumbering
/// would make the viewer quietly disagree with the loader about what a file
/// says.
#[test]
fn graph_section_tags_are_fixed() {
    assert_eq!(VALUE_NIL, 0);
    assert_eq!(VALUE_FALSE, 1);
    assert_eq!(VALUE_TRUE, 2);
    assert_eq!(VALUE_SMALL_INTEGER, 3);
    assert_eq!(VALUE_FLOAT, 4);
    assert_eq!(VALUE_OBJECT_ID, 5);
    assert_eq!(VALUE_CLASS_TAG, 6);
    assert_eq!(VALUE_LARGE_INTEGER, 9);
    assert_eq!(VALUE_FRACTION, 10);
    assert_eq!(VALUE_SHIPPED_MODULE, 11);
    assert_eq!(RETIRED_VALUE_TAGS, [7, 8]);

    assert_eq!(OBJECT_INSTANCE, 0);
    assert_eq!(OBJECT_STRING, 1);
    assert_eq!(OBJECT_SYMBOL, 2);
    assert_eq!(OBJECT_ARRAY, 3);
    assert_eq!(OBJECT_CLOSURE, 6);
    assert_eq!(OBJECT_CLASS, 7);
    assert_eq!(OBJECT_METACLASS, 8);
    assert_eq!(OBJECT_FRAME, 10);

    assert_eq!(MIXIN_ROW, 0);
    assert_eq!(MIXIN_SHIPPED, 2);
    assert_eq!(RETIRED_MIXIN_TAGS, [1]);

    assert_eq!(CLASS_RECORD, 2);
    assert_eq!(CLASS_METACLASS, 4);
    assert_eq!(CLASS_MODULE, 6);
    assert_eq!(CLASS_MODULE_LEVEL, 7);
    assert_eq!(CLASS_SHIPPED, 8);
    assert_eq!(RETIRED_CLASS_TAGS, [0, 1, 3, 5]);

    assert_eq!(FRAME_METHOD, 0);
    assert_eq!(FRAME_CLOSURE, 1);
    assert_eq!(FRAME_TOP_LEVEL, 2);
    assert_eq!(FRAME_INITIALIZER, 3);
    assert_eq!(FRAME_SUPERCLASS_CLAUSE, 4);
    assert_eq!(FRAME_SUPERCLASS_MESSAGE, 5);
    assert_eq!(FRAME_MIXIN_APPLICATION_MESSAGE, 6);

    assert_eq!(HASHED_METACLASS, 3);
    assert_eq!(HASHED_MODULE_CLASS, 5);
    assert_eq!(HASHED_MODULE_LEVEL, 6);
    assert_eq!(HASHED_SHIPPED, 7);
    assert_eq!(RETIRED_HASHED_TAGS, [0, 1, 2, 4]);
}

/// The live value tags, which no retired one may rejoin.
const LIVE_VALUE_TAGS: [u8; 10] = [
    VALUE_NIL,
    VALUE_FALSE,
    VALUE_TRUE,
    VALUE_SMALL_INTEGER,
    VALUE_FLOAT,
    VALUE_OBJECT_ID,
    VALUE_CLASS_TAG,
    VALUE_LARGE_INTEGER,
    VALUE_FRACTION,
    VALUE_SHIPPED_MODULE,
];

/// The live mixin references, which no retired one may rejoin.
const LIVE_MIXIN_TAGS: [u8; 2] = [MIXIN_ROW, MIXIN_SHIPPED];

/// The live class tags, which no retired one may rejoin. `CLASS_METACLASS`
/// names only a module class's metaclass now.
const LIVE_CLASS_TAGS: [u8; 5] = [
    CLASS_RECORD,
    CLASS_METACLASS,
    CLASS_MODULE,
    CLASS_MODULE_LEVEL,
    CLASS_SHIPPED,
];

/// The live tag-named hash tags, which no retired one may rejoin.
const LIVE_HASHED_TAGS: [u8; 4] = [
    HASHED_METACLASS,
    HASHED_MODULE_CLASS,
    HASHED_MODULE_LEVEL,
    HASHED_SHIPPED,
];

/// A retired value, mixin, class or hash tag stays retired: 1 and 3 named
/// the one root class and its levels before an image carried several
/// modules, and the builtin, platform and platform path tags each named a
/// shipped object before a shipped name did. A stale image's reference must
/// meet an unknown tag rather than name whatever took the number.
#[test]
fn retired_class_and_hashed_tags_are_never_reused() {
    for retired in RETIRED_VALUE_TAGS {
        assert!(
            !LIVE_VALUE_TAGS.contains(&retired),
            "value tag {retired} is retired"
        );
    }
    for retired in RETIRED_MIXIN_TAGS {
        assert!(
            !LIVE_MIXIN_TAGS.contains(&retired),
            "mixin tag {retired} is retired"
        );
    }
    for retired in RETIRED_CLASS_TAGS {
        assert!(
            !LIVE_CLASS_TAGS.contains(&retired),
            "class tag {retired} is retired"
        );
    }
    for retired in RETIRED_HASHED_TAGS {
        assert!(
            !LIVE_HASHED_TAGS.contains(&retired),
            "hashed tag {retired} is retired"
        );
    }
}

/// Within each family a number means one thing. Nothing but a copy-paste
/// stops two constants sharing one, and the compiler would not object.
#[test]
fn no_two_graph_tags_in_a_family_share_a_number() {
    let families: [&[u8]; 6] = [
        &LIVE_VALUE_TAGS,
        &[
            OBJECT_INSTANCE,
            OBJECT_STRING,
            OBJECT_SYMBOL,
            OBJECT_ARRAY,
            OBJECT_CLOSURE,
            OBJECT_CLASS,
            OBJECT_METACLASS,
            OBJECT_FRAME,
        ],
        &LIVE_MIXIN_TAGS,
        &LIVE_CLASS_TAGS,
        &[
            FRAME_METHOD,
            FRAME_CLOSURE,
            FRAME_TOP_LEVEL,
            FRAME_INITIALIZER,
            FRAME_SUPERCLASS_CLAUSE,
            FRAME_SUPERCLASS_MESSAGE,
            FRAME_MIXIN_APPLICATION_MESSAGE,
        ],
        &LIVE_HASHED_TAGS,
    ];
    for family in families {
        let mut seen = family.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), family.len(), "duplicate tag within a family");
    }
}

/// A builtin class's tag is how a module's root section names the builtin
/// its root builds on, so renumbering one would decode a stale image's
/// reference into a different, perfectly valid builtin class. The tags are
/// a match in
/// `encode::builtin_class_tag` rather than constants in `opcodes.rs`, and are
/// pinned here all the same.
#[test]
fn builtin_class_tags_are_fixed() {
    assert_eq!(builtin_class_tag(BuiltinClass::Object), 0);
    assert_eq!(builtin_class_tag(BuiltinClass::Boolean), 1);
    assert_eq!(builtin_class_tag(BuiltinClass::True), 2);
    assert_eq!(builtin_class_tag(BuiltinClass::False), 3);
    assert_eq!(builtin_class_tag(BuiltinClass::Integer), 4);
    assert_eq!(builtin_class_tag(BuiltinClass::Float), 5);
    assert_eq!(builtin_class_tag(BuiltinClass::String), 6);
    assert_eq!(builtin_class_tag(BuiltinClass::Symbol), 7);
    assert_eq!(builtin_class_tag(BuiltinClass::Array), 9);
    assert_eq!(builtin_class_tag(BuiltinClass::UndefinedObject), 11);
    assert_eq!(builtin_class_tag(BuiltinClass::Closure), 12);
    assert_eq!(builtin_class_tag(BuiltinClass::VmMirror), 14);
    assert_eq!(builtin_class_tag(BuiltinClass::Top), 15);
    assert_eq!(builtin_class_tag(BuiltinClass::Value), 16);
    assert_eq!(builtin_class_tag(BuiltinClass::Number), 17);
    assert_eq!(builtin_class_tag(BuiltinClass::Behavior), 20);
    assert_eq!(builtin_class_tag(BuiltinClass::Class), 21);
    assert_eq!(builtin_class_tag(BuiltinClass::Metaclass), 22);
    assert_eq!(builtin_class_tag(BuiltinClass::Activation), 23);
    assert_eq!(builtin_class_tag(BuiltinClass::Exception), 24);
    assert_eq!(builtin_class_tag(BuiltinClass::Error), 25);
    assert_eq!(builtin_class_tag(BuiltinClass::MessageNotUnderstood), 26);
    assert_eq!(builtin_class_tag(BuiltinClass::Message), 27);
    assert_eq!(builtin_class_tag(BuiltinClass::Halt), 28);
    assert_eq!(builtin_class_tag(BuiltinClass::FutureImplementation), 30);
    assert_eq!(builtin_class_tag(BuiltinClass::LargeInteger), 31);
    assert_eq!(builtin_class_tag(BuiltinClass::ArgumentError), 32);
    assert_eq!(builtin_class_tag(BuiltinClass::CannotReturn), 33);
    assert_eq!(builtin_class_tag(BuiltinClass::NonBooleanReceiver), 34);
    assert_eq!(builtin_class_tag(BuiltinClass::CannotInstantiate), 35);
    assert_eq!(builtin_class_tag(BuiltinClass::Proxy), 36);
    assert_eq!(builtin_class_tag(BuiltinClass::Future), 37);
    assert_eq!(builtin_class_tag(BuiltinClass::Collection), 38);
    assert_eq!(builtin_class_tag(BuiltinClass::Fraction), 39);
    // 8 was `Character`, retired 2026-09-25 with the class: a stale image's
    // tag-8 reference names no builtin rather than the next one assigned.
    assert_eq!(builtin_class_from_tag(8), None);
    // 13, 18, 19 and 29 were `Platform`, `Transcript`, `Collections` and
    // `KernelNamespace`, retired 2026-09-26 when the four moved into the
    // shipped `PlatformFactory.ns`: an image names them by their declarations.

    // 10 was `OrderedCollection`, retired 2026-09-28 when `platform collections
    // List`, Newspeak in the shipped `CollectionsFactory.ns`, replaced it.
    assert_eq!(RETIRED_BUILTIN_CLASS_TAGS, [8, 10, 13, 18, 19, 29]);
    for retired in RETIRED_BUILTIN_CLASS_TAGS {
        assert_eq!(
            builtin_class_from_tag(retired),
            None,
            "builtin class tag {retired} is retired"
        );
    }
}

/// A mixin-table slot record's kind byte: the accessor shape a slot
/// declares. The two eager kinds are the mutability tags the record carried
/// before lazy slots, unchanged, so an older image's slots still read as
/// what they were.
#[test]
fn slot_kind_tags_are_fixed() {
    assert_eq!(SLOT_IMMUTABLE, 0);
    assert_eq!(SLOT_MUTABLE, 1);
    assert_eq!(SLOT_LAZY_IMMUTABLE, 2);
    assert_eq!(SLOT_LAZY_MUTABLE, 3);
}

#[test]
fn source_origin_tags_are_fixed() {
    assert_eq!(ORIGIN_FILE, 0);
    assert_eq!(ORIGIN_EMBEDDED, 1);
}

#[test]
fn header_flags_are_fixed() {
    use ns_runtime::image::encode;
    assert_eq!(encode::FLAG_HAS_SOURCE, 1);
    assert_eq!(encode::FLAG_START, 4);
    // Bit 2 was `FLAG_STATELESS`; it is retired and never reused.
    assert_eq!(encode::RETIRED_HEADER_FLAGS, 2);
    assert_eq!(encode::KNOWN_FLAGS & encode::RETIRED_HEADER_FLAGS, 0);
}

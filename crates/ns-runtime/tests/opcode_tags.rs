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

use ns_runtime::image::opcodes::*;

#[test]
fn instruction_tags_are_fixed() {
    assert_eq!(OPCODE_LOAD_LITERAL, 0);
    assert_eq!(OPCODE_LOAD_SELF, 1);
    assert_eq!(OPCODE_LOAD_BUILTIN_CLASS, 2);
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
}

#[test]
fn literal_tags_are_fixed() {
    assert_eq!(LITERAL_NIL, 0);
    assert_eq!(LITERAL_BOOLEAN, 1);
    assert_eq!(LITERAL_SMALL_INTEGER, 2);
    assert_eq!(LITERAL_FLOAT, 3);
    assert_eq!(LITERAL_STRING, 4);
    assert_eq!(LITERAL_SYMBOL, 5);
    assert_eq!(LITERAL_CHARACTER, 6);
    assert_eq!(LITERAL_ARRAY, 7);
    assert_eq!(LITERAL_MALFORMED_INTEGER, 8);
    assert_eq!(LITERAL_UNRESOLVED_IDENTIFIER, 9);
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
        OPCODE_LOAD_BUILTIN_CLASS,
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
        LITERAL_CHARACTER,
        LITERAL_ARRAY,
        LITERAL_MALFORMED_INTEGER,
        LITERAL_UNRESOLVED_IDENTIFIER,
    ];
    let mut sorted = literals;
    sorted.sort_unstable();
    let mut deduplicated = sorted.to_vec();
    deduplicated.dedup();
    assert_eq!(deduplicated.len(), literals.len(), "duplicate literal tag");
}

/// A retired tag stays retired: no live instruction takes its number
/// (`image/opcodes.rs`).
#[test]
fn retired_instruction_tags_are_never_reused() {
    assert_eq!(RETIRED_OPCODES, [7, 8, 9, 10, 11, 21, 22, 23, 24]);
    let live = [
        OPCODE_LOAD_LITERAL,
        OPCODE_LOAD_SELF,
        OPCODE_LOAD_BUILTIN_CLASS,
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
    ];
    for retired in RETIRED_OPCODES {
        assert!(!live.contains(&retired), "tag {retired} is retired");
    }
}

/// A retired object-body tag stays retired, for the same reason a retired
/// instruction tag does: the closure record's payload changed on both
/// halves, so tag 4's bytes would otherwise decode into a different,
/// perfectly valid closure.
#[test]
fn retired_object_body_tags_are_refused() {
    assert_eq!(RETIRED_OBJECT_BODIES, [4]);
    assert_eq!(OBJECT_CLOSURE, 6);
    let live = [
        OBJECT_INSTANCE,
        OBJECT_STRING,
        OBJECT_SYMBOL,
        OBJECT_ARRAY,
        OBJECT_CLOSURE,
        OBJECT_CLASS,
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
    assert_eq!(VALUE_PLATFORM, 7);

    assert_eq!(OBJECT_INSTANCE, 0);
    assert_eq!(OBJECT_STRING, 1);
    assert_eq!(OBJECT_SYMBOL, 2);
    assert_eq!(OBJECT_ARRAY, 3);
    assert_eq!(OBJECT_CLASS, 5);
    assert_eq!(OBJECT_CLOSURE, 6);

    assert_eq!(CLASS_BUILTIN, 0);
    assert_eq!(CLASS_USER_ROOT, 1);
    assert_eq!(CLASS_RECORD, 2);

    assert_eq!(FRAME_METHOD, 0);
    assert_eq!(FRAME_CLOSURE, 1);
    assert_eq!(FRAME_TOP_LEVEL, 2);
    assert_eq!(FRAME_INITIALIZER, 3);
    assert_eq!(FRAME_SUPERCLASS_CLAUSE, 4);
    assert_eq!(FRAME_SUPERCLASS_MESSAGE, 5);

    assert_eq!(HASHED_BUILTIN_CLASS, 0);
    assert_eq!(HASHED_ROOT_CLASS, 1);
    assert_eq!(HASHED_PLATFORM, 2);
}

/// Within each family a number means one thing. Nothing but a copy-paste
/// stops two constants sharing one, and the compiler would not object.
#[test]
fn no_two_graph_tags_in_a_family_share_a_number() {
    let families: [&[u8]; 5] = [
        &[
            VALUE_NIL,
            VALUE_FALSE,
            VALUE_TRUE,
            VALUE_SMALL_INTEGER,
            VALUE_FLOAT,
            VALUE_OBJECT_ID,
            VALUE_CLASS_TAG,
            VALUE_PLATFORM,
        ],
        &[
            OBJECT_INSTANCE,
            OBJECT_STRING,
            OBJECT_SYMBOL,
            OBJECT_ARRAY,
            OBJECT_CLOSURE,
            OBJECT_CLASS,
        ],
        &[CLASS_BUILTIN, CLASS_USER_ROOT, CLASS_RECORD],
        &[
            FRAME_METHOD,
            FRAME_CLOSURE,
            FRAME_TOP_LEVEL,
            FRAME_INITIALIZER,
            FRAME_SUPERCLASS_CLAUSE,
            FRAME_SUPERCLASS_MESSAGE,
        ],
        &[HASHED_BUILTIN_CLASS, HASHED_ROOT_CLASS, HASHED_PLATFORM],
    ];
    for family in families {
        let mut seen = family.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), family.len(), "duplicate tag within a family");
    }
}

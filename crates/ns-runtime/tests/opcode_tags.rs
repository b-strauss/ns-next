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
    assert_eq!(OP_LOAD_LITERAL, 0);
    assert_eq!(OP_LOAD_SELF, 1);
    assert_eq!(OP_LOAD_BUILTIN_CLASS, 2);
    assert_eq!(OP_MOVE, 3);
    assert_eq!(OP_STORE_LOCAL, 4);
    assert_eq!(OP_LOAD_OUTER_LOCAL, 5);
    assert_eq!(OP_STORE_OUTER_LOCAL, 6);
    assert_eq!(OP_SEND_LEXICAL, 7);
    assert_eq!(OP_SEND_PUBLIC, 8);
    assert_eq!(OP_SEND_SUPER, 9);
    assert_eq!(OP_MAKE_CLOSURE, 10);
    assert_eq!(OP_MAKE_ARRAY, 11);
    assert_eq!(OP_INIT_SLOT, 12);
    assert_eq!(OP_RETURN, 13);
    assert_eq!(OP_IMPLICIT_RETURN, 14);
    assert_eq!(OP_NON_LOCAL_RETURN, 15);
    assert_eq!(OP_JUMP, 16);
    assert_eq!(OP_JUMP_IF_NOT_BOOLEAN, 17);
    assert_eq!(OP_BRANCH_IF_FALSE, 18);
    assert_eq!(OP_BRANCH_IF_TRUE, 19);
    assert_eq!(OP_SEND_INLINABLE, 20);
}

#[test]
fn literal_tags_are_fixed() {
    assert_eq!(LIT_NIL, 0);
    assert_eq!(LIT_BOOL, 1);
    assert_eq!(LIT_SMALL_INT, 2);
    assert_eq!(LIT_FLOAT, 3);
    assert_eq!(LIT_STR, 4);
    assert_eq!(LIT_SYM, 5);
    assert_eq!(LIT_CHAR, 6);
    assert_eq!(LIT_ARRAY, 7);
    assert_eq!(LIT_MALFORMED_INT, 8);
    assert_eq!(LIT_UNRESOLVED_IDENT, 9);
}

/// Two tags sharing a number would make the encoder ambiguous and the decoder
/// wrong, and neither the pinning tests above nor the compiler would notice:
/// the constants are independent `u8`s, so a copy-paste that repeats a number
/// is well-formed Rust.
#[test]
fn no_two_tags_share_a_number() {
    let ops = [
        OP_LOAD_LITERAL,
        OP_LOAD_SELF,
        OP_LOAD_BUILTIN_CLASS,
        OP_MOVE,
        OP_STORE_LOCAL,
        OP_LOAD_OUTER_LOCAL,
        OP_STORE_OUTER_LOCAL,
        OP_SEND_LEXICAL,
        OP_SEND_PUBLIC,
        OP_SEND_SUPER,
        OP_MAKE_CLOSURE,
        OP_MAKE_ARRAY,
        OP_INIT_SLOT,
        OP_RETURN,
        OP_IMPLICIT_RETURN,
        OP_NON_LOCAL_RETURN,
        OP_JUMP,
        OP_JUMP_IF_NOT_BOOLEAN,
        OP_BRANCH_IF_FALSE,
        OP_BRANCH_IF_TRUE,
        OP_SEND_INLINABLE,
    ];
    let mut sorted = ops;
    sorted.sort_unstable();
    let mut deduped = sorted.to_vec();
    deduped.dedup();
    assert_eq!(deduped.len(), ops.len(), "duplicate instruction tag");

    let lits = [
        LIT_NIL,
        LIT_BOOL,
        LIT_SMALL_INT,
        LIT_FLOAT,
        LIT_STR,
        LIT_SYM,
        LIT_CHAR,
        LIT_ARRAY,
        LIT_MALFORMED_INT,
        LIT_UNRESOLVED_IDENT,
    ];
    let mut sorted = lits;
    sorted.sort_unstable();
    let mut deduped = sorted.to_vec();
    deduped.dedup();
    assert_eq!(deduped.len(), lits.len(), "duplicate literal tag");
}

/// The graph section's tags, pinned for the same reason as the instruction
/// tags above: three readers now share this numbering -- the encoder, the
/// loader's decoder, and `inspect`'s structural walk -- and a renumbering
/// would make the viewer quietly disagree with the loader about what a file
/// says.
#[test]
fn graph_section_tags_are_fixed() {
    assert_eq!(VAL_NIL, 0);
    assert_eq!(VAL_FALSE, 1);
    assert_eq!(VAL_TRUE, 2);
    assert_eq!(VAL_SMALL_INT, 3);
    assert_eq!(VAL_FLOAT, 4);
    assert_eq!(VAL_OBJECT_ID, 5);
    assert_eq!(VAL_CLASS_TAG, 6);
    assert_eq!(VAL_PLATFORM, 7);

    assert_eq!(OBJ_INSTANCE, 0);
    assert_eq!(OBJ_STR, 1);
    assert_eq!(OBJ_SYM, 2);
    assert_eq!(OBJ_ARRAY, 3);
    assert_eq!(OBJ_CLOSURE, 4);
    assert_eq!(OBJ_CLASS, 5);

    assert_eq!(CLS_BUILTIN, 0);
    assert_eq!(CLS_USER_ROOT, 1);
    assert_eq!(CLS_RECORD, 2);

    assert_eq!(FRAME_METHOD, 0);
    assert_eq!(FRAME_CLOSURE, 1);
    assert_eq!(FRAME_TOP_LEVEL, 2);
    assert_eq!(FRAME_INITIALIZER, 3);
    assert_eq!(FRAME_SUPERCLASS_CLAUSE, 4);
    assert_eq!(FRAME_SUPERCLASS_MESSAGE, 5);

    assert_eq!(PC_KIND_TREE_WALK, 0);
    assert_eq!(PC_KIND_BYTECODE, 1);

    assert_eq!(HASHED_BUILTIN_CLASS, 0);
    assert_eq!(HASHED_ROOT_CLASS, 1);
    assert_eq!(HASHED_PLATFORM, 2);
}

/// Within each family a number means one thing. Nothing but a copy-paste
/// stops two constants sharing one, and the compiler would not object.
#[test]
fn no_two_graph_tags_in_a_family_share_a_number() {
    let families: [&[u8]; 6] = [
        &[
            VAL_NIL,
            VAL_FALSE,
            VAL_TRUE,
            VAL_SMALL_INT,
            VAL_FLOAT,
            VAL_OBJECT_ID,
            VAL_CLASS_TAG,
            VAL_PLATFORM,
        ],
        &[
            OBJ_INSTANCE,
            OBJ_STR,
            OBJ_SYM,
            OBJ_ARRAY,
            OBJ_CLOSURE,
            OBJ_CLASS,
        ],
        &[CLS_BUILTIN, CLS_USER_ROOT, CLS_RECORD],
        &[
            FRAME_METHOD,
            FRAME_CLOSURE,
            FRAME_TOP_LEVEL,
            FRAME_INITIALIZER,
            FRAME_SUPERCLASS_CLAUSE,
            FRAME_SUPERCLASS_MESSAGE,
        ],
        &[PC_KIND_TREE_WALK, PC_KIND_BYTECODE],
        &[HASHED_BUILTIN_CLASS, HASHED_ROOT_CLASS, HASHED_PLATFORM],
    ];
    for family in families {
        let mut seen = family.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), family.len(), "duplicate tag within a family");
    }
}

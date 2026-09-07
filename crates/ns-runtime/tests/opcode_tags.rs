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

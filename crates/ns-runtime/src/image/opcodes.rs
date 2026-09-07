//! The wire tags that identify an instruction or a literal in the image
//! encoding.
//!
//! **A tag is assigned once and never changes.** A new instruction takes the
//! next free number; a removed one retires its number rather than freeing it
//! for reuse. This is a stronger promise than the rest of the format makes --
//! images are disposable and `FORMAT_VERSION` is deliberately never bumped --
//! and it exists because renumbering fails in the one way the untrusted-input
//! guarantee does not cover. That guarantee is "no byte sequence may panic",
//! not "no byte sequence may be misread": a stale image whose opcodes were
//! renumbered decodes into a *different, perfectly valid* program and runs it,
//! reporting nothing. Retiring numbers instead means the stale bytes hit an
//! unknown tag and are refused.
//!
//! Both `encode` and `decode` read the numbers from here, so there is one
//! declaration rather than two parallel lists of magic numbers, and
//! `tests/opcode_tags.rs` pins every value so a renumbering fails the suite
//! instead of being noticed later by an image that no longer means what it
//! said.

// Instructions. Next free number: 20.
pub const OP_LOAD_LITERAL: u8 = 0;
pub const OP_LOAD_SELF: u8 = 1;
pub const OP_LOAD_BUILTIN_CLASS: u8 = 2;
pub const OP_MOVE: u8 = 3;
pub const OP_STORE_LOCAL: u8 = 4;
pub const OP_LOAD_OUTER_LOCAL: u8 = 5;
pub const OP_STORE_OUTER_LOCAL: u8 = 6;
pub const OP_SEND_LEXICAL: u8 = 7;
pub const OP_SEND_PUBLIC: u8 = 8;
pub const OP_SEND_SUPER: u8 = 9;
pub const OP_MAKE_CLOSURE: u8 = 10;
pub const OP_MAKE_ARRAY: u8 = 11;
pub const OP_INIT_SLOT: u8 = 12;
pub const OP_RETURN: u8 = 13;
pub const OP_IMPLICIT_RETURN: u8 = 14;
pub const OP_NON_LOCAL_RETURN: u8 = 15;
pub const OP_JUMP: u8 = 16;
pub const OP_JUMP_IF_NOT_BOOLEAN: u8 = 17;
pub const OP_BRANCH_IF_FALSE: u8 = 18;
pub const OP_BRANCH_IF_TRUE: u8 = 19;

// Literals, carried by `OP_LOAD_LITERAL` and nested inside `LIT_ARRAY`.
// Next free number: 10.
pub const LIT_NIL: u8 = 0;
pub const LIT_BOOL: u8 = 1;
pub const LIT_SMALL_INT: u8 = 2;
pub const LIT_FLOAT: u8 = 3;
pub const LIT_STR: u8 = 4;
pub const LIT_SYM: u8 = 5;
pub const LIT_CHAR: u8 = 6;
pub const LIT_ARRAY: u8 = 7;
/// An integer the lexer accepted and the parser rejects. Carried as its
/// original text because the error it raises is produced at execution time by
/// the same code the tree-walker uses.
pub const LIT_MALFORMED_INT: u8 = 8;
pub const LIT_UNRESOLVED_IDENT: u8 = 9;

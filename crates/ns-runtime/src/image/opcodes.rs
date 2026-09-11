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
/// original text because the error it raises is produced at execution time, by
/// `interpreter::eval::malformed_int_error`.
pub const LIT_MALFORMED_INT: u8 = 8;
pub const LIT_UNRESOLVED_IDENT: u8 = 9;

// ---------------------------------------------------------------------------
// Graph-section tags. Same rule as above: assigned once, never renumbered.
//
// These describe the object and frame records, which only an image carries.
// They were bare literals in `encode.rs` and again in `decode.rs` until
// `inspect` grew a third reader over the same bytes -- a structural walk that
// reports records instead of materializing them. Three hand-kept copies of one
// numbering is how a viewer starts quietly disagreeing with the loader about
// what a file says, so the numbering lives here and all three read it.
// ---------------------------------------------------------------------------

/// How a `Value` is encoded. Scalars carry their payload inline; the last
/// three name something outside the value itself.
pub const VAL_NIL: u8 = 0;
pub const VAL_FALSE: u8 = 1;
pub const VAL_TRUE: u8 = 2;
pub const VAL_SMALL_INT: u8 = 3;
pub const VAL_FLOAT: u8 = 4;
/// An index into the object table.
pub const VAL_OBJECT_ID: u8 = 5;
/// A class, named by a class tag rather than by object identity.
pub const VAL_CLASS_TAG: u8 = 6;
/// The `platform` singleton, resolved to the loading interpreter's own.
pub const VAL_PLATFORM: u8 = 7;

/// An object record's body, following its class tag and identity hash.
pub const OBJ_INSTANCE: u8 = 0;
pub const OBJ_STR: u8 = 1;
pub const OBJ_SYM: u8 = 2;
pub const OBJ_ARRAY: u8 = 3;
pub const OBJ_CLOSURE: u8 = 4;
pub const OBJ_CLASS: u8 = 5;

/// How a class reference is named. A builtin and the module's root class are
/// not in the graph at all -- the bootstrap and the mixin table produce them --
/// so only the third case indexes the object table.
pub const CLS_BUILTIN: u8 = 0;
pub const CLS_USER_ROOT: u8 = 1;
pub const CLS_RECORD: u8 = 2;

/// What a frame was activated for.
pub const FRAME_METHOD: u8 = 0;
pub const FRAME_CLOSURE: u8 = 1;
pub const FRAME_TOP_LEVEL: u8 = 2;
pub const FRAME_INITIALIZER: u8 = 3;
pub const FRAME_SUPERCLASS_CLAUSE: u8 = 4;
pub const FRAME_SUPERCLASS_MESSAGE: u8 = 5;

/// Retired. A frame record once said which unit its `pc` counted in -- a
/// statement or an instruction -- and only bytecode was ever representable,
/// so the byte was removed from the record. Both numbers stay assigned so
/// that a pc-unit field, if one ever returns, cannot give them a new meaning.
pub const PC_KIND_TREE_WALK: u8 = 0;
pub const PC_KIND_BYTECODE: u8 = 1;

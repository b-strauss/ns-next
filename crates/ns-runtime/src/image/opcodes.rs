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

// Instructions, numbered in the order `bytecode::Instruction` declares them.
// Next free number: 21.
pub const OPCODE_LOAD_LITERAL: u8 = 0;
pub const OPCODE_LOAD_SELF: u8 = 1;
pub const OPCODE_LOAD_BUILTIN_CLASS: u8 = 2;
pub const OPCODE_MOVE: u8 = 3;
pub const OPCODE_STORE_LOCAL: u8 = 4;
pub const OPCODE_LOAD_OUTER_LOCAL: u8 = 5;
pub const OPCODE_STORE_OUTER_LOCAL: u8 = 6;
pub const OPCODE_SEND_LEXICAL: u8 = 7;
pub const OPCODE_SEND_PUBLIC: u8 = 8;
pub const OPCODE_SEND_INLINABLE: u8 = 9;
pub const OPCODE_SEND_SUPER: u8 = 10;
pub const OPCODE_MAKE_CLOSURE: u8 = 11;
pub const OPCODE_MAKE_ARRAY: u8 = 12;
pub const OPCODE_INITIALIZE_SLOT: u8 = 13;
pub const OPCODE_RETURN: u8 = 14;
pub const OPCODE_IMPLICIT_RETURN: u8 = 15;
pub const OPCODE_NON_LOCAL_RETURN: u8 = 16;
pub const OPCODE_JUMP: u8 = 17;
pub const OPCODE_JUMP_IF_NOT_BOOLEAN: u8 = 18;
pub const OPCODE_BRANCH_IF_FALSE: u8 = 19;
pub const OPCODE_BRANCH_IF_TRUE: u8 = 20;

// Literals, carried by `OPCODE_LOAD_LITERAL` and nested inside `LITERAL_ARRAY`.
// Next free number: 10.
pub const LITERAL_NIL: u8 = 0;
pub const LITERAL_BOOLEAN: u8 = 1;
pub const LITERAL_SMALL_INTEGER: u8 = 2;
pub const LITERAL_FLOAT: u8 = 3;
pub const LITERAL_STRING: u8 = 4;
pub const LITERAL_SYMBOL: u8 = 5;
pub const LITERAL_CHARACTER: u8 = 6;
pub const LITERAL_ARRAY: u8 = 7;
/// An integer the lexer accepted and the parser rejects. Carried as its
/// original text because the error it raises is produced at execution time, by
/// `interpreter::literals::malformed_integer_error`.
pub const LITERAL_MALFORMED_INTEGER: u8 = 8;
pub const LITERAL_UNRESOLVED_IDENTIFIER: u8 = 9;

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
pub const VALUE_NIL: u8 = 0;
pub const VALUE_FALSE: u8 = 1;
pub const VALUE_TRUE: u8 = 2;
pub const VALUE_SMALL_INTEGER: u8 = 3;
pub const VALUE_FLOAT: u8 = 4;
/// An index into the object table.
pub const VALUE_OBJECT_ID: u8 = 5;
/// A class, named by a class tag rather than by object identity.
pub const VALUE_CLASS_TAG: u8 = 6;
/// The `platform` singleton, resolved to the loading interpreter's own.
pub const VALUE_PLATFORM: u8 = 7;

/// An object record's body, following its class tag and identity hash.
pub const OBJECT_INSTANCE: u8 = 0;
pub const OBJECT_STRING: u8 = 1;
pub const OBJECT_SYMBOL: u8 = 2;
pub const OBJECT_ARRAY: u8 = 3;
pub const OBJECT_CLOSURE: u8 = 4;
pub const OBJECT_CLASS: u8 = 5;

/// How a class reference is named. A builtin and the module's root class are
/// not in the graph at all -- the bootstrap and the mixin table produce them --
/// so only the third case indexes the object table.
pub const CLASS_BUILTIN: u8 = 0;
pub const CLASS_USER_ROOT: u8 = 1;
pub const CLASS_RECORD: u8 = 2;

/// What a frame was activated for, numbered in the order `types::FrameKind`
/// declares it.
pub const FRAME_METHOD: u8 = 0;
/// A closure activation: the object id of the closure object it runs.
pub const FRAME_CLOSURE: u8 = 1;
pub const FRAME_TOP_LEVEL: u8 = 2;
pub const FRAME_INITIALIZER: u8 = 3;
pub const FRAME_SUPERCLASS_CLAUSE: u8 = 4;
pub const FRAME_SUPERCLASS_MESSAGE: u8 = 5;

/// Which tag-named object an entry of the graph section's closing hash table
/// names. The three are exactly the objects `CLASS_BUILTIN`, `CLASS_USER_ROOT`
/// and `VALUE_PLATFORM` name: none of them is a record, so without this table
/// nothing in an image would carry a hash one of them was given
/// (`docs/design/image-format.md`, "Identity hashes are preserved verbatim").
/// A builtin-class entry is followed by its builtin class tag.
pub const HASHED_BUILTIN_CLASS: u8 = 0;
pub const HASHED_ROOT_CLASS: u8 = 1;
pub const HASHED_PLATFORM: u8 = 2;

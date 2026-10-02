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

// Instructions, numbered in the order `bytecode::Instruction` declares them,
// except the four sends: retired at 7-10 (see `RETIRED_OPCODES`) once their
// call site widened from a `u16` instruction index to a `u32` family number,
// reassigned to 21-24, then retired a second time and reassigned below at
// 25-28 once the number itself stopped being part of the artifact at all --
// a property the decoder now derives instead of reading. `MakeClosure` is
// retired at 11 and reassigned to 29 for the same reason: closure
// conversion gave it a list of copied registers, so its old bytes describe
// a different instruction.
// Next free number: 40.

/// Loads a literal into a register; a string, symbol or array
/// literal allocates a fresh object every time it runs.
pub const OPCODE_LOAD_LITERAL: u8 = 0;
/// Loads the receiver, `self`, into a register.
pub const OPCODE_LOAD_SELF: u8 = 1;
/// Copies one register into another.
pub const OPCODE_MOVE: u8 = 3;
/// Writes a register into a local variable of the running activation; reading
/// one needs no instruction, since its register is its value.
pub const OPCODE_STORE_LOCAL: u8 = 4;
/// Reads a local variable of an enclosing activation, `depth` steps out along
/// the chain of captured frames.
pub const OPCODE_LOAD_OUTER_LOCAL: u8 = 5;
/// Writes a local variable of an enclosing activation, reached the same way.
pub const OPCODE_STORE_OUTER_LOCAL: u8 = 6;
/// Creates an array from the values of a list of registers, for a `{ ... }`
/// expression.
pub const OPCODE_MAKE_ARRAY: u8 = 12;
/// Writes the initial value of one of the running mixin's own slots into
/// `self`; emitted in instance initializers and lazy slots' getters.
pub const OPCODE_INITIALIZE_SLOT: u8 = 13;
/// Returns a register's value from the running method or initializer, a `^`
/// outside a block.
pub const OPCODE_RETURN: u8 = 14;
/// Returns from a body that fell off its end, `self` from a method and the last
/// value from a block; it runs exactly like `Return`.
pub const OPCODE_IMPLICIT_RETURN: u8 = 15;
/// Returns from a block's home method, a `^` inside a block, unwinding every
/// activation above that method.
pub const OPCODE_NON_LOCAL_RETURN: u8 = 16;
/// Jumps unconditionally; emitted only by control-flow inlining.
pub const OPCODE_JUMP: u8 = 17;
/// Jumps when a register holds no Boolean at all: the guard in front of inlined
/// control flow, whose target handles any other receiver.
pub const OPCODE_JUMP_IF_NOT_BOOLEAN: u8 = 18;
/// Jumps when a register holds exactly `false`, and falls through for anything
/// else.
pub const OPCODE_BRANCH_IF_FALSE: u8 = 19;
/// Jumps when a register holds exactly `true`, and falls through for anything
/// else.
pub const OPCODE_BRANCH_IF_TRUE: u8 = 20;
/// Sends a message with an implicit receiver, the enclosing object at the
/// lexical depth the resolver fixed, by the rules of §5.7–§5.9.
pub const OPCODE_SEND_LEXICAL: u8 = 25;
/// Sends a message to an explicit receiver, found by `lookupPublic` (§5.5).
pub const OPCODE_SEND_PUBLIC: u8 = 26;
/// Exactly a `SendPublic`, emitted only by the reference lowering for a
/// control-flow send the optimized lowering inlines, so that both lowerings
/// count recursion depth alike.
pub const OPCODE_SEND_INLINABLE: u8 = 27;
/// Sends a message to `self`, looked up from the superclass of the running
/// method's defining class (§5.10).
pub const OPCODE_SEND_SUPER: u8 = 28;
/// Creates a block closure over a compiled block body, capturing the running
/// activation and copying the values of a list of registers into it.
pub const OPCODE_MAKE_CLOSURE: u8 = 29;
/// Builds a scope's indirection vector: an `Array` of `size` `nil` slots that
/// every block needing one of the scope's captured-and-written locals is
/// handed a reference to.
pub const OPCODE_MAKE_VECTOR: u8 = 30;
/// Reads one slot of an indirection vector.
pub const OPCODE_LOAD_VECTOR: u8 = 31;
/// Writes one slot of an indirection vector.
pub const OPCODE_STORE_VECTOR: u8 = 32;
/// An integer leaf's inlined fast path (`Instruction::IntegerBinary`).
pub const OPCODE_INTEGER_BINARY: u8 = 33;
/// A lazy slot's getter's return of its filled field
/// (`Instruction::ReturnSlotIfFilled`).
pub const OPCODE_RETURN_SLOT_IF_FILLED: u8 = 34;
/// `outer N` as a value, the kth enclosing object
/// (`Instruction::LoadEnclosingObject`).
pub const OPCODE_LOAD_ENCLOSING_OBJECT: u8 = 35;
/// §6.3.2: a new unresolved future of a closure (`Instruction::MakeFuture`).
pub const OPCODE_MAKE_FUTURE: u8 = 36;
/// A value seen through a future, forcing it first if it is unresolved
/// (`Instruction::ResolveFuture`).
pub const OPCODE_RESOLVE_FUTURE: u8 = 37;
/// `self`'s own slot read back without a getter send
/// (`Instruction::LoadOwnSlot`).
pub const OPCODE_LOAD_OWN_SLOT: u8 = 38;
/// `InitializeSlot` without §3.1.1's value-slot check, for a simultaneous
/// clause's future only (`Instruction::InitializeSlotProvisionally`).
pub const OPCODE_INITIALIZE_SLOT_PROVISIONALLY: u8 = 39;

/// Instruction tags that no longer name anything and are never reused:
/// 7–10 were the four sends while their call site was a `u16` instruction
/// index; widening it to a `u32` family number moved them to 21–24. Retiring
/// those in turn and moving to 25–28 is what makes a stale artifact from
/// before call-site numbers stopped being serialized at all meet an unknown
/// tag instead of decoding into a different, perfectly valid program. 11 was
/// `MakeClosure` before it carried a list of copied registers; a stale
/// artifact's tag-11 bytes would otherwise decode as a `MakeClosure` whose
/// block is handed nothing, which is a different, perfectly valid program.
/// 2 was `LoadBuiltinClass`, retired 2026-09-24 when a bare builtin class
/// name became a send to `Object`'s accessor instead of a loaded constant
/// (`newspeak-spec.pdf` §3.2): no lowering emits it any more, and retiring
/// the number keeps a stale artifact's tag-2 bytes from ever being read as
/// whatever instruction takes the next free number instead.
pub const RETIRED_OPCODES: [u8; 10] = [2, 7, 8, 9, 10, 11, 21, 22, 23, 24];

// Literals, carried by `OPCODE_LOAD_LITERAL`.
// Next free number: 11.

/// `nil`.
pub const LITERAL_NIL: u8 = 0;
/// `true` or `false`.
pub const LITERAL_BOOLEAN: u8 = 1;
/// An integer that fits a 64-bit signed word.
pub const LITERAL_SMALL_INTEGER: u8 = 2;
/// A 64-bit floating-point number.
pub const LITERAL_FLOAT: u8 = 3;
/// A string literal.
pub const LITERAL_STRING: u8 = 4;
/// A symbol literal, `#name`.
pub const LITERAL_SYMBOL: u8 = 5;
/// An identifier that names nothing, a module root's clause receiver; it
/// raises its error when it runs.
pub const LITERAL_UNRESOLVED_IDENTIFIER: u8 = 9;
/// An integer beyond `i64`, in `VALUE_LARGE_INTEGER`'s wire form: a sign
/// byte, a `u32` digit count, that many `u64` digits, least significant
/// first. Its digits, never its source text, so the decoder parses nothing.
/// A `LargeInteger` literal that fits `i64` travels as
/// `LITERAL_SMALL_INTEGER`, which loads as a `LargeInteger` beyond 61 bits.
pub const LITERAL_LARGE_INTEGER: u8 = 10;

/// Literal tags that no longer name anything and are never reused: 6 was the
/// character literal, retired 2026-09-25 when `"c"` became a one-character
/// string (§5.1.4 announces character literals going away, §5.1.5 makes
/// `"σ"` a string). A stale artifact's tag-6 literal meets an unknown tag
/// instead of decoding as whatever literal takes the next free number. 7 was
/// the literal array, `#( ... )`, retired 2026-09-27 with the syntax, which
/// neither the specification nor the canonical grammar has (a tuple,
/// `{ ... }`, is §5.1.7's array); its elements nested, so a stale artifact's
/// tag-7 bytes would otherwise decode as whatever literal took the number.
/// 8 was `MalformedInteger`, an integer literal beyond `i64`, until every
/// integer literal became representable (2026-09-29); it carried the
/// literal's text, which today's `LITERAL_LARGE_INTEGER` does not.
pub const RETIRED_LITERALS: [u8; 3] = [6, 7, 8];

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

/// How a `Value` is encoded. Scalars carry their payload inline; 5 to 8
/// name something outside the value itself. Next free number: 10.
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
/// An object the loading platform answers along a path of public members,
/// followed by a `u32` index into the image's platform path section
/// (`image::platform_paths`).
pub const VALUE_PLATFORM_PATH: u8 = 8;
/// A `LargeInteger`, carried inline as a value rather than as an object
/// record, since a value has no identity (§3.1.1): a sign byte, a `u32`
/// digit count, that many `u64` digits, least significant first. Only the
/// canonical encoding is accepted -- no zero top digit, no negative zero,
/// and nothing that fits `i64`, which travels as `VALUE_SMALL_INTEGER` and
/// loads as a `SmallInteger` in 61 bits, a `LargeInteger` beyond them.
pub const VALUE_LARGE_INTEGER: u8 = 9;

/// An object record's body, following its class tag and identity hash.
/// Next free number: 10.
pub const OBJECT_INSTANCE: u8 = 0;
pub const OBJECT_STRING: u8 = 1;
pub const OBJECT_SYMBOL: u8 = 2;
pub const OBJECT_ARRAY: u8 = 3;
/// A block closure: its code, the frame it captured, and its copied values.
pub const OBJECT_CLOSURE: u8 = 6;
/// A class object, in one of the two shapes its mixin reference
/// (`MIXIN_ROW`/`MIXIN_BUILTIN`) selects.
pub const OBJECT_CLASS: u8 = 7;
/// The metaclass of a class *record*: a record of its own, so a slot can
/// name it by id before its class record is decoded and its identity hash
/// travels in its header like any record's. Its header class tag is
/// `Metaclass`'s, and its body is one class reference, back to its class --
/// everything else about a metaclass is derived from that class
/// (`docs/design/object-model.md`, "The image format").
pub const OBJECT_METACLASS: u8 = 8;
/// An instance of the kernel-private `Activation` class: one frame id, the
/// reified activation it stands for. Its header class tag is `Activation`'s
/// builtin tag, the one place an image may name that class
/// (`decode::read_referenced_builtin_class`); any other header on this body,
/// or that header on any other body, is refused.
pub const OBJECT_ACTIVATION: u8 = 9;

/// Object-body tags that no longer name anything and are never reused: 4 was
/// the closure record while it carried its parameter names and its home
/// frame, and carried no copied values. Both halves changed, so a stale
/// artifact's tag-4 record would decode -- field for field, by luck of the
/// layout or not at all -- into something other than what it said. Retiring
/// the number makes those bytes an unknown tag instead. 5 was the class
/// record while it named its mixin by a bare row id and could not describe
/// a level a mixin application added; a stale tag-5 record's row id would
/// otherwise be read as a mixin-reference tag and the next four bytes.
pub const RETIRED_OBJECT_BODIES: [u8; 2] = [4, 5];

/// How a class record names the mixin its class object applies, which also
/// decides the rest of the record. Every class the graph creates applies a
/// declaration of the module's own -- a row of the mixin table, followed by
/// the superclass, enclosing object, declaring class and, for a level a `<:`
/// clause added, that clause -- except a level a `<:` clause naming a builtin
/// added, whose mixin is that builtin's: the bootstrap supplies it, no row
/// describes it, and so it is named the way a builtin class is, by builtin
/// tag, followed by the superclass and the clause only. Its enclosing object
/// and declaring class are the builtin's, which no image carries.
pub const MIXIN_ROW: u8 = 0;
pub const MIXIN_BUILTIN: u8 = 1;

/// How a class reference is named. A builtin, a module's root class and the
/// levels a module root's own `<:` clauses add are not in the graph at all --
/// the bootstrap, the mixin table and the module section produce them -- so
/// only `CLASS_RECORD` indexes the object table. `CLASS_MODULE` is followed by
/// a `u32` module index; `CLASS_MODULE_LEVEL` by a `u32` module index and the
/// level's `u32` position among that root's levels, lowest first.
/// `CLASS_METACLASS` names the metaclass of one of those prebuilt classes,
/// which is no record either, and is followed by that class's own reference
/// -- `CLASS_BUILTIN`, `CLASS_MODULE`, `CLASS_MODULE_LEVEL` or
/// `CLASS_PLATFORM_PATH`, never a second `CLASS_METACLASS` or a
/// `CLASS_RECORD` (whose metaclass is a record, `OBJECT_METACLASS`), which
/// bounds the nesting at one. `CLASS_PLATFORM_PATH` names a class the loading
/// platform answers along a path of public members, followed by a `u32`
/// index into the image's platform path section: like a builtin, it exists
/// before the graph is decoded. Next free number: 8.
pub const CLASS_BUILTIN: u8 = 0;
pub const CLASS_RECORD: u8 = 2;
pub const CLASS_METACLASS: u8 = 4;
pub const CLASS_PLATFORM_PATH: u8 = 5;
pub const CLASS_MODULE: u8 = 6;
pub const CLASS_MODULE_LEVEL: u8 = 7;

/// Class tags that no longer name anything and are never reused: 1 was the
/// one root class an image could carry, 3 a level of it by position alone.
/// Once an image carries several modules, each names its module by index
/// (`CLASS_MODULE`, `CLASS_MODULE_LEVEL`); a stale image's tag-1 or tag-3
/// reference meets an unknown tag instead of naming whichever module is first.
pub const RETIRED_CLASS_TAGS: [u8; 2] = [1, 3];

/// Builtin class tags (`encode::builtin_class_tag`, which a `CLASS_BUILTIN`
/// reference, a `MIXIN_BUILTIN` level and a `HASHED_BUILTIN_CLASS` entry are
/// followed by) that no longer name anything and are never reused: 8 was
/// `Character`, retired 2026-09-25 with the class; 10 was `OrderedCollection`,
/// retired 2026-09-28 when `platform collections List`, Newspeak in the
/// shipped `CollectionsFactory.ns`, replaced it; 13, 18, 19 and 29 were
/// `Platform`, `Transcript`, `Collections` and `KernelNamespace`, retired
/// 2026-09-26 when the four moved into a shipped module (now
/// `PlatformFactory.ns`), whose objects an image names by their platform
/// paths. A stale image's reference to one names no builtin rather than
/// whichever class took the number. Next free number: 32.
pub const RETIRED_BUILTIN_CLASS_TAGS: [u8; 6] = [8, 10, 13, 18, 19, 29];

/// What a frame was activated for, numbered in the order `types::FrameKind`
/// declares it.
pub const FRAME_METHOD: u8 = 0;
/// A closure activation: the object id of the closure object it runs.
pub const FRAME_CLOSURE: u8 = 1;
pub const FRAME_TOP_LEVEL: u8 = 2;
pub const FRAME_INITIALIZER: u8 = 3;
pub const FRAME_SUPERCLASS_CLAUSE: u8 = 4;
pub const FRAME_SUPERCLASS_MESSAGE: u8 = 5;
/// A `<:` clause's message activation, followed by the class tag of the level
/// the clause added -- whose clause names the body that ran.
pub const FRAME_MIXIN_APPLICATION_MESSAGE: u8 = 6;

/// Which tag-named object an entry of the graph section's closing hash table
/// names. They are exactly the objects `CLASS_BUILTIN`, `CLASS_MODULE`,
/// `CLASS_MODULE_LEVEL`, `VALUE_PLATFORM`, `CLASS_METACLASS` and the platform
/// path references name: none of them is a record, so without this table
/// nothing in an image would carry a hash one of them was given
/// (`docs/design/image-format.md`, "Identity hashes are preserved verbatim").
/// A builtin-class entry is followed by its builtin class tag, a module-class
/// entry by its `u32` module index, a module-level entry by its `u32` module
/// index and `u32` level position. A metaclass entry names the metaclass of a
/// prebuilt class -- a builtin, a module's root class, a module level or a
/// class named by a platform path -- and is followed by that class's
/// reference, the four tags `CLASS_METACLASS` allows. A platform path entry is
/// followed by its `u32` index into the platform path section. Next free
/// number: 7.
pub const HASHED_BUILTIN_CLASS: u8 = 0;
pub const HASHED_PLATFORM: u8 = 2;
pub const HASHED_METACLASS: u8 = 3;
pub const HASHED_PLATFORM_PATH: u8 = 4;
pub const HASHED_MODULE_CLASS: u8 = 5;
pub const HASHED_MODULE_LEVEL: u8 = 6;

/// Tag-named hash tags that no longer name anything and are never reused: 1
/// was the one root class's entry, replaced by `HASHED_MODULE_CLASS` and its
/// module index when an image came to carry several modules.
pub const RETIRED_HASHED_TAGS: [u8; 1] = [1];

/// A mixin-table slot record's kind: the accessor shape its slot declares
/// (§6.3.2, §6.3.3). The two eager kinds are the mutability tags the record
/// carried before lazy slots, unchanged. A lazy slot's getter is one of its
/// row's methods, whose fast path the decoder restores from the slot; a
/// mutable one's setter is derived like an eager slot's. Next free number: 4.
pub const SLOT_IMMUTABLE: u8 = 0;
pub const SLOT_MUTABLE: u8 = 1;
pub const SLOT_LAZY_IMMUTABLE: u8 = 2;
pub const SLOT_LAZY_MUTABLE: u8 = 3;

/// How a module's `SourceOrigin` is written. Next free number: 2.
pub const ORIGIN_FILE: u8 = 0;
pub const ORIGIN_EMBEDDED: u8 = 1;

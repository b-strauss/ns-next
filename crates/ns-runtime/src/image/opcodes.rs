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
// except the four sends and the two initializer invocations. The sends were
// retired at 7-10 (see `RETIRED_OPCODES`) once their call site widened from
// a `u16` instruction index to a `u32` family number, reassigned to 21-24,
// then retired a second time and reassigned at 25-28 once the number
// itself stopped being part of the artifact at all -- a
// property the decoder now derives instead of reading -- and retired a
// third time, with the two initializer invocations at 44 and 45, when a
// send came to name its argument slots' base beside each argument's source
// (46-51). `MakeClosure` is retired at 11 and reassigned to 29 for the same
// reason: closure conversion gave it a list of copied registers, so its old
// bytes describe a different instruction.
// Next free number: 52.

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
/// lexical depth the resolver fixed, by the rules of §5.7–§5.9. Like every
/// send and initializer invocation below, it names its arguments as the
/// base of their contiguous slots and the register each slot is copied
/// from (`bytecode::Instruction`).
pub const OPCODE_SEND_LEXICAL: u8 = 46;
/// Sends a message to an explicit receiver, found by `lookupPublic` (§5.5).
pub const OPCODE_SEND_PUBLIC: u8 = 47;
/// Exactly a `SendPublic`, emitted only by the reference lowering for a
/// control-flow send the optimized lowering inlines, so that both lowerings
/// count recursion depth alike.
pub const OPCODE_SEND_INLINABLE: u8 = 48;
/// Sends a message to `self`, looked up from the superclass of the running
/// method's defining class (§5.10).
pub const OPCODE_SEND_SUPER: u8 = 49;
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
/// An inlined loop's guard failure: signals `NonBooleanReceiver` for the
/// condition's answer and ends its path (`Instruction::SignalNonBoolean`).
pub const OPCODE_SIGNAL_NON_BOOLEAN: u8 = 40;
/// A factory body's allocation of an instance of the class object the
/// factory was sent to (`Instruction::AllocateInstance`).
pub const OPCODE_ALLOCATE_INSTANCE: u8 = 41;
/// A factory body's run of its class object's initializer on the new
/// instance (`Instruction::InitializeInstance`).
pub const OPCODE_INITIALIZE_INSTANCE: u8 = 42;
/// §6.1's check of a `<:` clause's selector against the primary factory of
/// the level it adds (`Instruction::CheckAppliedClause`).
pub const OPCODE_CHECK_APPLIED_CLAUSE: u8 = 43;
/// An initializer's run of its superclass's initializer, after §6.1's check
/// (`Instruction::InitializeSuperclass`).
pub const OPCODE_INITIALIZE_SUPERCLASS: u8 = 50;
/// An initializer's run of the own part of a level a `<:` clause adds
/// (`Instruction::InitializeAppliedLevel`).
pub const OPCODE_INITIALIZE_APPLIED_LEVEL: u8 = 51;

/// Instruction tags that no longer name anything and are never reused:
/// 7–10 were the four sends while their call site was a `u16` instruction
/// index; widening it to a `u32` family number moved them to 21–24. Retiring
/// those in turn and moving to 25–28 is what makes a stale artifact from
/// before call-site numbers stopped being serialized at all meet an unknown
/// tag instead of decoding into a different, perfectly valid program. 25–28
/// were the four sends, and 44 and 45 `InitializeSuperclass` and
/// `InitializeAppliedLevel`, while they named an arbitrary list of argument
/// registers; naming the slots' base beside the sources moved them to
/// 46–51, since a stale artifact's list would otherwise be read as the base
/// and the bytes after it. 11 was
/// `MakeClosure` before it carried a list of copied registers; a stale
/// artifact's tag-11 bytes would otherwise decode as a `MakeClosure` whose
/// block is handed nothing, which is a different, perfectly valid program.
/// 2 was `LoadBuiltinClass`, retired 2026-09-24 when a bare builtin class
/// name became a send to `Object`'s accessor instead of a loaded constant
/// (`newspeak-spec.pdf` §3.2): no lowering emits it any more, and retiring
/// the number keeps a stale artifact's tag-2 bytes from ever being read as
/// whatever instruction takes the next free number instead.
pub const RETIRED_OPCODES: [u8; 16] = [2, 7, 8, 9, 10, 11, 21, 22, 23, 24, 25, 26, 27, 28, 44, 45];

// Literals, carried by `OPCODE_LOAD_LITERAL`.
// Next free number: 16.

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
/// The body that stands in for one past the register ceiling; it raises
/// its error when it runs. No payload.
pub const LITERAL_BODY_TOO_LARGE: u8 = 11;
/// The body that stands in for one past the argument limit, one taking or
/// sending more than `bytecode::ARGUMENT_LIMIT` arguments; it raises its
/// error when it runs. No payload.
pub const LITERAL_BODY_PAST_THE_ARGUMENT_LIMIT: u8 = 12;
/// The body that stands in for one past `bytecode::INSTRUCTION_LIMIT`
/// instructions; it raises its error when it runs. No payload.
pub const LITERAL_BODY_PAST_THE_INSTRUCTION_LIMIT: u8 = 13;
/// An asynchronous send, which ns-next does not build yet; it raises its
/// error when it runs. Followed by its message's selector, as
/// `LITERAL_SYMBOL` is.
pub const LITERAL_UNBUILT_ASYNCHRONOUS_SEND: u8 = 14;
/// A pattern literal, which ns-next does not build yet; it raises its
/// error when it runs. No payload.
pub const LITERAL_UNBUILT_PATTERN_LITERAL: u8 = 15;

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
// These describe the object records, frames among them, which only an image
// carries.
// They were bare literals in `encode.rs` and again in `decode.rs` until
// `inspect` grew a third reader over the same bytes -- a structural walk that
// reports records instead of materializing them. Three hand-kept copies of one
// numbering is how a viewer starts quietly disagreeing with the loader about
// what a file says, so the numbering lives here and all three read it.
// ---------------------------------------------------------------------------

/// How a `Value` is encoded. Scalars carry their payload inline; 5, 6 and
/// 11 name something outside the value itself. Next free number: 12.
pub const VALUE_NIL: u8 = 0;
pub const VALUE_FALSE: u8 = 1;
pub const VALUE_TRUE: u8 = 2;
pub const VALUE_SMALL_INTEGER: u8 = 3;
pub const VALUE_FLOAT: u8 = 4;
/// An index into the object table.
pub const VALUE_OBJECT_ID: u8 = 5;
/// A class, named by a class tag rather than by object identity.
pub const VALUE_CLASS_TAG: u8 = 6;
/// A `LargeInteger`, carried inline as a value rather than as an object
/// record, since a value has no identity (§3.1.1): a sign byte, a `u32`
/// digit count, that many `u64` digits, least significant first. Only the
/// canonical encoding is accepted -- no zero top digit, no negative zero,
/// and nothing that fits `i64`, which travels as `VALUE_SMALL_INTEGER` and
/// loads as a `SmallInteger` in 61 bits, a `LargeInteger` beyond them.
pub const VALUE_LARGE_INTEGER: u8 = 9;
/// A `Fraction`, carried inline as a value rather than as an object
/// record, since a value has no identity (§3.1.1): the numerator and then
/// the denominator, each a `VALUE_SMALL_INTEGER` or a
/// `VALUE_LARGE_INTEGER` in its canonical form. Only a fraction in normal
/// form is accepted -- a denominator above 1, no common factor -- since
/// anything else would break `=` and `hash`.
pub const VALUE_FRACTION: u8 = 10;
/// A shipped module's instance, followed by a `u32` index into the image's
/// shipped-name section (`image::shipped_names`), whose entry must be a
/// module instance. It stands only as a frame record's receiver or
/// register: a module instance holds the core library's state, so no other
/// value of a graph may name it, and only a frame of shipped code holds one.
/// A started image names none at all, since its frames are read by the
/// closures the program holds.
pub const VALUE_SHIPPED_MODULE: u8 = 11;

/// Value tags that no longer name anything and are never reused: 7 was
/// `VALUE_PLATFORM`, the platform singleton, and 8 `VALUE_PLATFORM_PATH`, an
/// object the loading platform answered along a path of public members. A
/// shipped name replaced both on 2026-10-12: the platform is an ordinary
/// record, and what a path reached is named by its declaration
/// (`VALUE_CLASS_TAG` with `CLASS_SHIPPED`, or `VALUE_SHIPPED_MODULE`).
pub const RETIRED_VALUE_TAGS: [u8; 2] = [7, 8];

/// An object record's body, following its class tag and identity hash.
/// Next free number: 17.
pub const OBJECT_INSTANCE: u8 = 0;
pub const OBJECT_STRING: u8 = 1;
pub const OBJECT_SYMBOL: u8 = 2;
pub const OBJECT_ARRAY: u8 = 3;
/// A block closure: its code, the frame it captured, and its copied values.
pub const OBJECT_CLOSURE: u8 = 6;
/// A class object, in one of the two shapes its mixin reference
/// (`MIXIN_ROW`/`MIXIN_SHIPPED`) selects.
pub const OBJECT_CLASS: u8 = 7;
/// The metaclass of a class *record*: a record of its own, so a slot can
/// name it by id before its class record is decoded and its identity hash
/// travels in its header like any record's. Its header class tag is
/// `Metaclass`'s, and its body is one class reference, back to its class --
/// everything else about a metaclass is derived from that class
/// (`docs/design/object-model.md`, "The image format").
pub const OBJECT_METACLASS: u8 = 8;
/// A reified activation: an instance of the kernel-private `Activation`
/// class, the object a frame reference names. Its header names
/// `Kernel Activation` by its shipped name (`CLASS_SHIPPED`), the one record
/// an image may give that class; any other header on this body, or that
/// header on any other body, is refused. Its body is the frame's
/// kind tag (`FRAME_*`) and that kind's payload, the body that runs in it
/// (a `FRAME_BODY_*` tag, followed by a `u32` code id for
/// `FRAME_BODY_CODE`), a `u32` program counter, `uncontinuable`, the
/// receiver, the sender and the home (each a presence flag and an object
/// id), the current class (a presence flag and a class tag), then a `u32`
/// register count and that many values. The kind comes first and the
/// register count is on the wire, so the decoder's first pass sizes the
/// record from its own bytes (`decode::decode_graph_section`). A program's
/// body is named by code id rather than found through the frame's class,
/// since after an edit the class's current version carries another body,
/// and the code an older version runs is in the code table too.
pub const OBJECT_FRAME: u8 = 15;
/// A `ByteArray`: a `u32` length, then that many bytes, in order. The
/// first pass sizes the record from its own bytes -- the length is refused
/// when fewer bytes follow it, before anything is allocated -- and the
/// second fills the placeholder. Only the bytes travel: the alignment
/// reserve ahead of them is not written, since the decoder places the data
/// at its own placeholder's alignment.
pub const OBJECT_BYTE_ARRAY: u8 = 11;
/// A `WeakArray`: a `u32` count, then that many values, its slots in order.
/// Only a record whose header is `Kernel WeakArray` carries it. A slot names
/// its object only when the graph reaches that object strongly otherwise,
/// and is `nil` if not: the encoder writes what a collection would keep.
/// What takes no record -- an immediate, a shipped name, a `LargeInteger`,
/// boxed `Float` or `Fraction` written by value -- is written as it is
/// whether or not anything else reaches it: a collection's timing is
/// unspecified, so keeping more than one would is never wrong.
pub const OBJECT_WEAK_ARRAY: u8 = 12;
/// An `Ephemeron`: a `u32` field count, then that many values, the key
/// first. Only a record whose header is `Kernel Ephemeron` carries it, with
/// exactly that class's field count, and that header carries no other
/// body. An ephemeron whose key the graph never reaches strongly is written
/// with every field `nil`, as a collection would leave it.
pub const OBJECT_EPHEMERON: u8 = 13;
/// A reified mixin, the kernel-private `Mixin` object a mixin mirror
/// reflects: the mixin's reference, as a class record names its mixin
/// (`MIXIN_ROW` and a row id, or `MIXIN_SHIPPED` and the index of a shipped
/// class's name, the instance side in either case), then a side byte
/// (`MIXIN_SIDE_*`). Only a record whose header is `Kernel Mixin` carries
/// it, and that header carries no other body. The id the object holds is
/// not written: the decoder mints it afresh, and refuses a second record
/// for one mixin, since the object is canonical as a symbol is.
pub const OBJECT_MIXIN: u8 = 14;

/// A body-less declaration's class object (`Mixin::has_body`): the level
/// its last `<:` clause added, written as `OBJECT_CLASS` writes it -- in
/// either shape, ending with the clause and the class it named -- then the
/// declaration's own scope, which no other class record has: its enclosing
/// object (a presence flag and a value, an object) and its declaring class
/// (a presence flag and a class tag), both absent for a module's root.
/// The applied mixin's code walks out from the level's own enclosing
/// object and declaring class, the named class's; the declaration's own
/// code, its clause messages and their arguments, from these. A record
/// with this body whose clause is not a body-less declaration's last, and
/// an `OBJECT_CLASS` record whose clause is, are refused.
pub const OBJECT_BODY_LESS_CLASS: u8 = 16;

/// Object-body tags that no longer name anything and are never reused: 4 was
/// the closure record while it carried its parameter names and its home
/// frame, and carried no copied values. Both halves changed, so a stale
/// artifact's tag-4 record would decode -- field for field, by luck of the
/// layout or not at all -- into something other than what it said. Retiring
/// the number makes those bytes an unknown tag instead. 5 was the class
/// record while it named its mixin by a bare row id and could not describe
/// a level a mixin application added; a stale tag-5 record's row id would
/// otherwise be read as a mixin-reference tag and the next four bytes. 9
/// was the activation record while frames had a table of their own: its
/// body was one frame-table id, which an object id read in its place would
/// resolve to some other record, so the frame record took a new number
/// (`OBJECT_FRAME`) rather than this one. 10 was that frame record while
/// it named no body and a method frame carried no access: its kind payload
/// would otherwise be read with the bytes after it as a code id.
pub const RETIRED_OBJECT_BODIES: [u8; 4] = [4, 5, 9, 10];

/// How a class record names the mixin its class object applies, which also
/// decides the rest of the record. Every class the graph creates applies a
/// declaration of the module's own -- a row of the mixin table, followed by
/// the superclass, enclosing object, declaring class and, for a level a `<:`
/// clause added, that clause -- except a level a `<:` clause naming a
/// shipped class added, whose mixin is that class's: the loading binary
/// supplies it and no row describes it, so it is named by the `u32` index of
/// the shipped class's entry in the shipped-name section -- a class, allowed
/// as a superclass -- followed by the superclass and the clause only. Its
/// enclosing object and declaring class are the shipped class's, which no
/// image carries. An `OBJECT_MIXIN` record names a mixin the same two
/// ways, an instance side only, and only its side byte follows.
pub const MIXIN_ROW: u8 = 0;
pub const MIXIN_SHIPPED: u8 = 2;

/// Mixin references that no longer name anything and are never reused: 1
/// was `MIXIN_BUILTIN`, a level applying a builtin's mixin, named by
/// builtin class tag. A shipped name replaced it on 2026-10-12
/// (`MIXIN_SHIPPED`), which names any shipped class's mixin by declaration.
/// Next free number: 3.
pub const RETIRED_MIXIN_TAGS: [u8; 1] = [1];

/// Which side of the mixin an `OBJECT_MIXIN` record names reflects: the
/// mixin itself, or its class side -- the declared one, or the one
/// synthesized for a class that declares none. Next free number: 2.
pub const MIXIN_SIDE_INSTANCE: u8 = 0;
pub const MIXIN_SIDE_CLASS: u8 = 1;

/// How a class reference is named. A shipped class, metaclass or level is
/// not in the graph at all -- the loading binary produces it -- so it is
/// named by its shipped name; every other class object, a module's root
/// class, the levels its `<:` clauses add and their metaclasses among them,
/// is a record, and `CLASS_RECORD` names it by object id.
///
/// `CLASS_SHIPPED` is followed by a `u32` index into the image's
/// shipped-name section (`image::shipped_names`), whose entry must be a
/// class, a metaclass or a level -- never a module instance -- allowed where
/// the reference stands (`docs/design/image-format.md`, "Shipped names"): as a
/// value, a shipped class that is public or that a public class accessor of
/// its module answers, and its metaclass; as a superclass, such a
/// class, not its metaclass, but `Behavior`, `Class` and `Metaclass`; as a
/// record's class, any shipped class but a module's root and `Top`,
/// `Activation` only on a frame record and `FutureImplementation` only on a
/// future's; as a frame's class or a class record's declaring class, any
/// shipped class, metaclass or level.
/// Next free number: 9.
pub const CLASS_RECORD: u8 = 2;
pub const CLASS_SHIPPED: u8 = 8;

/// Class tags that no longer name anything and are never reused: 1 was the
/// one root class an image could carry, 3 a level of it by position alone,
/// replaced by 6, `CLASS_MODULE`, a module's root class by module index, and
/// 7, `CLASS_MODULE_LEVEL`, one of its levels by module index and position,
/// once an image carried several modules. 4 was `CLASS_METACLASS`, the
/// metaclass of such a root or level, followed by that class's own tag. A
/// module's root class, its levels and their metaclasses became records of
/// the graph on 2026-10-13 (`CLASS_RECORD`), which the module section names
/// by object id, so a stale image's tag-1, 3, 4, 6 or 7 reference meets an
/// unknown tag instead of naming whichever module is first. 0 was
/// `CLASS_BUILTIN`, a builtin by builtin class tag, and 5
/// `CLASS_PLATFORM_PATH`, a class the loading platform answered along a
/// path of public members; a shipped name replaced both on 2026-10-12
/// (`CLASS_SHIPPED`), which names a builtin and every other shipped class,
/// metaclass and level by declaration.
///
/// The builtin class tags those references and a module's root section were
/// written in -- a numbering of `BuiltinClass` of its own, 0 to 39 with 8,
/// 10, 13, 18, 19 and 29 retired -- are retired as a whole: no part of an
/// image names a builtin by position any more, only by its shipped name.
pub const RETIRED_CLASS_TAGS: [u8; 7] = [0, 1, 3, 4, 5, 6, 7];

/// What a frame was activated for (`types::FrameKind`), numbered in the
/// order the kinds were added; an initializer has one tag per body it can
/// run.
///
/// A method activation: the `u32` symbol id of its selector and its
/// method's access tag (`mixins::access_tag`), which together with its
/// body describe a method an edit superseded as well as a current one.
pub const FRAME_METHOD: u8 = 0;
/// A closure activation: the object id of the closure object it runs.
pub const FRAME_CLOSURE: u8 = 1;
pub const FRAME_TOP_LEVEL: u8 = 2;
/// An initializer activation running its level's full body
/// (`InitializerBody::Full`), followed by the class tag of the level it was
/// entered at; `FRAME_OWN_INITIALIZER` and `FRAME_STANDALONE_INITIALIZER`
/// name the other two bodies the same way.
pub const FRAME_INITIALIZER: u8 = 3;
pub const FRAME_SUPERCLASS_CLAUSE: u8 = 4;
/// Frame kinds 5 and 6 were the activations of a superclass clause's message
/// and of a `<:` clause's message, retired when those arguments were lowered
/// into the initializers. A stale image naming one meets an unknown tag.
pub const RETIRED_FRAME_KINDS: [u8; 2] = [5, 6];
/// An initializer activation running its level's own part
/// (`InitializerBody::OwnPart`), followed by the level's class tag.
pub const FRAME_OWN_INITIALIZER: u8 = 7;
/// An initializer activation running its level's standalone body
/// (`InitializerBody::Standalone`), followed by the level's class tag.
pub const FRAME_STANDALONE_INITIALIZER: u8 = 8;
/// A factory activation, followed by the class tag of the class object it
/// runs on. Its body is always `FRAME_BODY_DECLARED`: a factory body is
/// derived from the class, never carried. Next free number: 10.
pub const FRAME_FACTORY: u8 = 9;

/// How a frame record names the body that runs in it; each frame has one
/// spelling. A top-level frame runs none (`FRAME_BODY_NONE`), and no other
/// frame is written so. A body of a shipped declaration is
/// `FRAME_BODY_DECLARED`: found through the frame's kind in the declaration
/// of the class it resolves through -- a method by selector in the current
/// class's mixin, the initializer and the superclass clause in the
/// kind's class's mixin -- since the code table carries no shipped code,
/// and a shipped mixin is never edited, so the lookup finds that very
/// body; a closure frame is never written so, its closure's code being in
/// the table. Every other body is `FRAME_BODY_CODE`, followed by its `u32`
/// code id. Next free number: 3.
pub const FRAME_BODY_NONE: u8 = 0;
pub const FRAME_BODY_CODE: u8 = 1;
pub const FRAME_BODY_DECLARED: u8 = 2;

/// Code kind tag 4 was the body of a clause message's arguments, retired when
/// those arguments were lowered into the initializers. Tags 0-3 named the
/// four live kinds while a code record carried no `local_count`, retired
/// when it came to carry one, since the kind opens the record: a stale
/// record meets a retired tag rather than having its source span read as
/// its local count. The live code kind tags, 5-8, are written by
/// `encode::code_kind_tag` and read by `decode`, not named here. Next free
/// number: 9.
pub const RETIRED_CODE_KIND_TAGS: [u8; 5] = [0, 1, 2, 3, 4];

/// Which tag-named object an entry of the graph section's closing hash table
/// names: an object of the shipped-name section, which is no record, so
/// without this table nothing in an image would carry a hash one of them
/// was given (`docs/design/image-format.md`, "Identity hashes are preserved
/// verbatim"). A shipped entry is followed by the `u32` index of a name of
/// any kind in the shipped-name section. Next free number: 8.
pub const HASHED_SHIPPED: u8 = 7;

/// Tag-named hash tags that no longer name anything and are never reused: 1
/// was the one root class's entry, replaced by 5, `HASHED_MODULE_CLASS`, a
/// module's root class by module index, when an image came to carry several
/// modules; 6 was `HASHED_MODULE_LEVEL`, one of its levels, and 3
/// `HASHED_METACLASS`, the metaclass of either. A module's root class, its
/// levels and their metaclasses became records on 2026-10-13, whose hashes
/// travel in their headers like any record's. 0 was
/// `HASHED_BUILTIN_CLASS`, a builtin by builtin class tag, 2
/// `HASHED_PLATFORM`, the platform singleton, and 4 `HASHED_PLATFORM_PATH`,
/// an object a platform path named; a shipped name replaced the three on
/// 2026-10-12 (`HASHED_SHIPPED`), and the platform's hash travels in its
/// record's header like any record's.
pub const RETIRED_HASHED_TAGS: [u8; 7] = [0, 1, 2, 3, 4, 5, 6];

/// A mixin-table slot record's kind: the accessor shape its slot declares
/// (§6.3.2, §6.3.3). The two eager kinds are the mutability tags the record
/// carried before lazy slots, unchanged. A lazy slot's getter is one of its
/// row's methods, whose fast path the decoder restores from the slot; a
/// mutable one's setter is derived like an eager slot's. Next free number: 4.
pub const SLOT_IMMUTABLE: u8 = 0;
pub const SLOT_MUTABLE: u8 = 1;
pub const SLOT_LAZY_IMMUTABLE: u8 = 2;
pub const SLOT_LAZY_MUTABLE: u8 = 3;

/// Where a module record's path lies (`image::modules`): `ORIGIN_PROJECT`,
/// followed by the module's path relative to its project, or `ORIGIN_CORE`,
/// followed by the file name of a module the binary holds -- `Manifest.ns`
/// -- whose path is `core:` and that name. Next free number: 4.
pub const ORIGIN_PROJECT: u8 = 2;
pub const ORIGIN_CORE: u8 = 3;

/// Origins that no longer name anything and are never reused: 0 was
/// `ORIGIN_FILE`, the path a module was read from on the building machine,
/// and 1 `ORIGIN_EMBEDDED`, a source compiled into the binary, named by its
/// file. A module record came to carry the module's identity on
/// 2026-11-19 -- its path relative to the project, its id and its version
/// (`docs/design/modules.md`, decision 10) -- so a stale record meets an
/// unknown tag instead of being read as a path. Next free number: 4.
pub const RETIRED_ORIGIN_TAGS: [u8; 2] = [0, 1];

/// Whether a module record carries a version after its id: none, or one,
/// followed by its text. Next free number: 2.
pub const MODULE_UNVERSIONED: u8 = 0;
pub const MODULE_VERSIONED: u8 = 1;

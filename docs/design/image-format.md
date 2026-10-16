# Image format — serialization and deployment artifacts

> **Status:** built. One artifact: an application assembled from one source
> file or a project of several modules, packaged just before `main:args:`,
> round-trips through `build` and `load` and can be inspected without being
> loaded. The decoder decides from the value alone whether an integer or a
> float loads as an immediate or as an object ("Integers and floats on the
> wire"). The decoder reads the graph section in two passes, sizing every
> record's object from its own bytes before it allocates any ("Two
> passes"), and a reified frame is an object record like every other
> ("Frames are object records"), as a `ByteArray` is, its bytes without
> the alignment reserve ("A `ByteArray` is a record of its own"). An image names every object the binary
> owns -- a shipped module's instance, a class its root declares, their
> metaclasses and levels -- by its declaration, resolved by table lookup
> against the loading binary's shipped world, of which the loader first
> builds, by the binary's own code, whatever the image names that the boot
> did not, and carries the fingerprint of that world in its header
> ("Shipped names"); a module's
> root class is a record of the graph like any class. A load tenures
> everything once before it decodes the graph, which it allocates old
> ("The image under generations"). **Planned:** suspend and resume.
> **Code:** `crates/ns-runtime/src/image/` — `mod.rs` (`package`, `snapshot`,
> `load`), `encode.rs`, `decode.rs`, `mixins.rs`, `modules.rs`,
> `shipped_names.rs`, `fingerprint.rs`, `symbols.rs`, `opcodes.rs`,
> `inspect.rs`; `crates/ns-runtime/src/interpreter/shipped_world.rs` (the
> shipped world and what may be named where);
> `crates/ns-runtime/core-library/Manifest.ns`. CLI: `ns-next build`,
> `ns-next load`, `ns-next-analyzer inspect`, and `ns-next-analyzer disasm` on an artifact;
> `crates/ns-cli/src/project.rs` (the project file) and `program.rs`
> (reading, compiling and assembling a program).

## Principles

**Not a memory dump.** The image is an explicit, self-describing,
architecture-neutral serialization decoded on load — never `mmap`ed VM memory,
which could not be cross-architecture. No native code, no JIT output, no raw
pointers, no baked-in tagging or host endianness.

**Canonical encodings.** Little-endian integers, IEEE-754 `binary64` for floats,
UTF-8 for strings and symbols, an integer as an `i64` when it fits one and
otherwise as a sign and its 64-bit digits, least significant first. Symbols are
re-interned on load rather than assumed to keep their ids. The decoder picks
a `String`'s width, one byte or four per code point, from the code points it
reads ([core-library.md](core-library.md), "Strings are code points in two
widths").

**The bootstrap is the base image.** There is no pre-built base image file.
`run`, `build` and `load` all bootstrap the core library and the `Object`
hierarchy from Rust primitives, and the image describes only what the
program's modules added.

**An image is untrusted input.** No byte sequence may cause a panic or a process
abort. Every id, tag, count and length is validated before it indexes, allocates
or slices, and any recursion whose depth the input controls is bounded or made
iterative.

That guarantee is about **decoding**. `load` sends the packaged application
`main: platform args:` with no flag at all, so starting one executes its code
with the same exposure as running the equivalent source. Recursion depth is
bounded by one counted limit, 100,000 activations, an ordinary Newspeak error
rather than a native stack overflow, since nothing re-enters the run loop from
Rust; and a crafted `SendInlinable` cannot use the count's one exemption to
recurse uncounted. Nor can it stack more exempt activations above a counted
one than source can: that number is bounded by how deeply blocks nest, which
the decoder caps at the parser's own ceiling ("Verification" below); see
[execution.md](execution.md), "The depth limit". A mutated artifact that
decodes cleanly can still abort while executing — a native that indexes an
argument it was not given, for one: the identity leaves read `arguments[0]`
unchecked, and the decoder does not check a send's
argument count against its selector — and that is not a decoder defect.

## The artifact

An **application** is the instance of a root class that is sent `main:
platform args:` (§8.4). The platform enters there and nowhere else: no root
class is instantiated with it, so nothing assembled before `main:args:` can
hold it, and an artifact written at that point cannot reach it. `build`
writes exactly that: the application, assembled by running its factory and
every initializer, serialized just before the send, as Newspeak on Squeak
(`DeploymentManager.ns`, `appClass packageUsing: ide namespacing manifest`,
then `Serializer new serialize: app`) and Primordial Soup (`CompilerApp.ns`,
`createSnapshotsFromNamespace:outputs:`) do ("Packaging" below). `load`
decodes it and sends it `main: platform args:` with the loading process's
platform, the first platform the application ever sees.

There is **one format and one artifact**. An image is an object graph with
the code and declarations it runs, and the header's `FLAG_START` says the
graph's one root is an application to be started. The image with heap state
that liveness asks for ([liveness-and-reflection.md](liveness-and-reflection.md))
is the same artifact written at another point of the same run --
mid-execution rather than before `main:args:`, resumed rather than started
("Planned: suspend and resume"). Bracha's position -- "Ideally, you have
images and a binary format … what is important is that you do not have just
an image" ([An Image Problem](https://gbracha.blogspot.com/2009/10/image-problem.html))
-- is met the way upstream meets it: the deployment artifact is an image
taken at the one point where no capability is bound yet, so it carries the
program and none of the world it will run in.

`image::package` writes the started image. `image::snapshot` writes a graph
from whatever roots a host hands it, without `FLAG_START`; no command calls
it -- the tests use it to take the graph shapes a started image never holds
(a stored platform, the classes and facets it hands out, several roots), and it
is the writer the image with heap state will build on. Both go through
one writer (`write_image`).

A host holds what a load answers as roots: `image::load`'s `LoadedImage`
carries every module's root class and the image's roots as `Root`s, and
whether the image is to be started ([memory.md](memory.md), "Host roots").
`image::load` decodes inside one entry of the interpreter it bootstraps and
stashes them once decoding and every verification are done, before that
entry ends — nothing runs while an artifact decodes, so nothing moves under
the decoder's handles. `image::package` and `image::snapshot` take a
`&Mutator` and values a host fetches from its roots in the same entry, for
the same reason: encoding runs no code.

## Layout

```
[header][symbols][code][mixins][modules][shipped names][graph][tag-named hashes]

header:        magic "NSNX", FORMAT_VERSION (u32), flags (u8), fingerprint (u64)
modules:       count (u32), then per module [origin][source?][root object id (u32)]
shipped names: count (u32), then per name [module][declaration][kind (u8)][clause (u32)?]
graph:         object table, roots
```

The header carries magic, `FORMAT_VERSION`, a flag byte and the shipped
world's fingerprint ("Shipped names" below). Two flags are
assigned (`image/encode.rs`): `FLAG_HAS_SOURCE` (1) says every module in the
module section carries its source text, and `FLAG_START` (4) says the graph's
one root is an application `load` starts. Bit 2 is retired
(`RETIRED_HEADER_FLAGS`): it was `FLAG_STATELESS`, the stateless binary, and
a stale binary's header now meets an unknown flag
(`a_retired_stateless_flag_is_refused_as_unknown`,
`crates/ns-runtime/tests/image.rs`). `read_header`
**refuses any flag this build does not implement** rather than masking it off: an
unimplemented flag means a section the reader cannot see, and continuing would
read the next field out of whatever that section occupies. A started image
whose graph has any number of roots but one is refused
(`DecodeError::StartedImageRootCount`), since it leaves the host nothing, or
nothing certain, to start.

`FORMAT_VERSION` is 1 and is not bumped. The project has no external users, so
images predating a change are disposable and no migration path is owed to them.
The guarantee that still has to hold is the untrusted-input one, and that comes
from validating ids and lengths, not from a version field. What the version
cannot say -- that the image was written by a binary whose shipped world is
the loading one's -- the fingerprint says: a load whose binary computes
another refuses the image before it reads anything past the header
(`DecodeError::ShippedWorldMismatch`).

## What is *not* serialized

This is the part that decides how big an artifact is and how large its trusted
surface is.

- **The shipped world** -- every object the binary owns: each shipped
  module's instance (`Kernel`, `PlatformFactory`, `TranscriptFactory`,
  `TimeFactory`, `CollectionsFactory`, `TypedArraysFactory`),
  each class its root declares, the builtins among them, their metaclasses
  and the levels their `<:` clauses add -- is named by its **shipped
  name**, never structurally encoded: the module, the declaration and the
  kind, listed once in a section of its own and resolved by table lookup
  against the loading binary's own shipped world before the graph is
  decoded ("Shipped names" below). The binary's own code builds that world
  -- the boot `Kernel` and `PlatformFactory`, the loader every other
  module and class the image names -- and it is the fixed
  root/special-objects table. So is the core library: a shipped class's
  methods are the loading process's own `Kernel.ns`, `PlatformFactory.ns`
  and the four modules under `platform/`, built by its bootstrap or its
  loader -- decoded from the code the binary embeds, or compiled
  from source ([core-library.md](core-library.md), "The embedded shipped
  code") -- and no image carries their code or the VM mirror — which is
  also why `Instruction::SendMirror`, emitted only into shipped
  core-library source, has no wire tag ([compiler.md](compiler.md),
  "Mirror sends"). The embedded shipped code carries the kernel's
  `SendMirror`s in a codec of its own that the image decoder never
  reaches, so no image byte sequence decodes into one.
- **`nil`, `true` and `false`** are unboxed `Value` scalars with no heap identity
  to preserve, encoded as scalar tags.
- **The platform is a record, not a reference to the loading one.** It is an
  instance of the shipped class `PlatformFactory Platform`, a value whose
  lazy slots are written as they stand, so it decodes to a value equal to
  the loading platform -- lazy slots are no part of a value's state -- and
  fills an empty slot on demand through the canonical `PlatformFactory`
  instance. A started image holds no platform: nothing before `main:args:`
  hands a program one ("Packaging" below).
- **Mixin applications** are not serialized. Layout and lookup follow
  deterministically from a mixin and its superclass application, so they are
  rebuilt as class objects decode. This removes a section, a verification pass,
  and the possibility of loading an image whose stored layout disagrees with its
  mixins. It is also what proves two instances that shared an application before
  a snapshot still share one after it. A **metaclass** stores nothing twice:
  its mixin, application, enclosing object and declaring class all follow from
  its class. The metaclass of a class *record* is a record of its own
  (`OBJECT_METACLASS`, header `Metaclass`) whose body is only the class
  reference back to its class, and the decoder applies its class's metaclass
  mixin over `Class`, as `Mutator::attach_metaclass` does (§6); a class
  record's header names its metaclass record; a module's root class and the
  levels its clauses add are class records too, with metaclass records of
  their own. The metaclass of a shipped class or level is named by its own
  shipped name. A class record applying some
  row's class side is refused (`ClassRecordAppliesAClassSide`)
  ([object-model.md](object-model.md), "The image format").
- **Inline-cache state and dispatch ids.** No cache entry, polymorphic block,
  global-cache entry or code family's range is written, and no activation's or
  closure's `feedback_base`: a decoded closure carries "not assigned" and is
  given a fresh range when it first runs, and a loaded artifact starts with
  empty caches ([dispatch.md](dispatch.md), "3b — The caches"). The dispatch
  ids the caches guard on are minted per heap as the applications are
  rebuilt, so an id never comes from the bytes either; every decoded
  object's header is stamped from its class in one pass once they all are
  ([dispatch.md](dispatch.md), "The dispatch id").
- **The bindings map.** A binding is resolved by span only at compile time, which
  a loaded world never reaches: lowering has already turned every binding into an
  instruction, and nothing that runs compiled code takes a map. Reconstructing
  one would require re-resolving the source, which would require re-parsing it —
  exactly the load path that was removed.

A hostile image therefore cannot replace a builtin or any other shipped
class, because a shipped name resolves against the loading binary's own
shipped world rather than against file content: the image chooses which of
the binary's modules and classes the loader builds, never what they are.

## Sections

In order: the symbol table, the code table, the mixin table, the module
section, the shipped names, the graph (object table, roots),
and the tag-named hashes. Each is described below; the code table's
instruction encoding has a section of its own.

**Symbol table**, right after the header. Every selector and name once, referenced by index
everywhere else. Selectors are the format's most repeated strings — name-based
virtual dispatch puts the same selector at very many call sites — so this shrinks
the file and turns interning into one pass over the table instead of one
`Symbol::intern` per send instruction.

**Code table.** Every `CompiledCode` reachable from the modules — each method,
each initializer, each superclass clause, every nested block body — in a flat
id-addressed table, blocks before the code that creates them so a `MakeClosure`
operand encodes as an already-decoded id. The image's bytecode is what runs after
a load; nothing recompiles. How each instruction is encoded is below.

## The instruction encoding

On the wire an instruction is a `bytecode::Instruction`, written as **a tag
byte followed by a fixed operand layout** for that tag. Registers are `u16`,
jump targets are instruction indices, a send's call site is its `u32` number
within its code family, selectors and names are symbol-table ids, and a
literal carries its own tag byte from a second table. No instruction's
operands nest.

In memory a body is stored in a different form, eight bytes an instruction
with side tables ([execution.md](execution.md), "The compact instruction
encoding"), which the wire does not see: the encoder writes the
`Instruction`s it reads back through the body's decoded view, and the
decoder emits every `Instruction` it reads into a `CodeBuilder`. One
instruction is one eight-byte slot, so a jump target, the statement map and
a frame's `program_counter` are the same instruction index on the wire and
in memory.

**A tag is assigned once and never changes.** A new instruction takes the next
free number; a removed one retires its number rather than freeing it for reuse.

That is a stronger promise than the rest of this format makes, and it is
deliberate. Images are disposable and `FORMAT_VERSION` is never bumped, so a
stale image is not refused for being stale. If a tag changed meaning, the stale
bytes would not fail — they would decode into a **different, perfectly valid
program**, and it would run. The untrusted-input guarantee does not cover that
case: it promises no panic, not no misreading. Retiring numbers instead means
stale bytes hit an unknown tag and are refused, which is the outcome that can be
noticed.

The instructions are numbered in the order `bytecode::Instruction` declares
them, and the frame kinds in the order `types::FrameKind` does.

Instructions:

- 0 `LoadLiteral`: loads a literal into a register; a string, symbol or array
  literal allocates a fresh object every time it runs.
- 1 `LoadSelf`: loads the receiver, `self`, into a register.
- 2: retired. It was `LoadBuiltinClass`, which loaded the builtin class a
  bare name such as `Object` denoted when no enclosing declaration declared
  that name; such a name is now a send to `Object`'s accessor (§3.2,
  [dispatch.md](dispatch.md), "Binding variants"), and bytes written under 2
  meet an unknown tag and are refused (`opcodes::RETIRED_OPCODES`,
  `the_retired_load_builtin_class_tag_is_refused`).
- 3 `Move`: copies one register into another.
- 4 `StoreLocal`: writes a register into a local variable of the running
  activation; reading one needs no instruction, since its register is its value.
- 5 `LoadOuterLocal`: reads a local variable of an enclosing activation, `depth`
  steps out along the chain of captured frames.
- 6 `StoreOuterLocal`: writes a local variable of an enclosing activation,
  reached the same way.
- 7–11 and 21–24: retired. 7–10 were the sends while a call site was a `u16`
  instruction index; 21–24 were the sends while the number was serialized as
  a `u32` (see the call-site paragraph below); 11 was `MakeClosure` before
  closure conversion gave it a list of copied registers. The sends are 25–28
  and `MakeClosure` is 29; bytes written under any retired number meet an
  unknown tag and are refused (`opcodes::RETIRED_OPCODES`).
- 12 `MakeArray`: creates an array from the values of a list of registers, for a
  `{ ... }` expression.
- 13 `InitializeSlot`: writes the initial value of one of the running mixin's
  own slots into `self`; emitted in instance initializers and in lazy slots'
  getters.
- 14 `Return`: returns a register's value from the running method or
  initializer, a `^` outside a block.
- 15 `ImplicitReturn`: returns from a body that fell off its end, `self` from a
  method and the last value from a block; it runs exactly like `Return`.
- 16 `NonLocalReturn`: returns from a block's home method, a `^` inside a block,
  unwinding every activation above that method.
- 17 `Jump`: jumps unconditionally; emitted by control-flow inlining and by
  `lower_integer_binary`, past the fallback send it emits for a successful
  arithmetic leaf.
- 18 `JumpIfNotBoolean`: jumps when a register holds no Boolean at all: the
  guard in front of inlined control flow, whose target handles any other receiver.
- 19 `BranchIfFalse`: jumps when a register holds exactly `false`, and falls
  through for anything else.
- 20 `BranchIfTrue`: jumps when a register holds exactly `true`, and falls
  through for anything else.
- 25 `SendLexical`: sends a message with an implicit receiver, the enclosing
  object at the lexical depth the resolver fixed, by the rules of §5.7–§5.9.
- 26 `SendPublic`: sends a message to an explicit receiver, found by
  `lookupPublic` (§5.5).
- 27 `SendInlinable`: exactly a `SendPublic`, emitted only by the reference
  lowering for a control-flow send the optimized lowering inlines, so that both
  lowerings count recursion depth alike.
- 28 `SendSuper`: sends a message to `self`, looked up from the superclass of
  the running method's defining class (§5.10).
- 29 `MakeClosure`: creates a block closure over a compiled block body,
  capturing the running activation, and carries the registers whose values the
  closure is handed ([execution.md](execution.md), "Closure conversion").
- 30 `MakeVector`: builds a body's indirection vector, the `Array` the locals
  a block writes live in; emitted once, as the body's first instruction.
- 31 `LoadVector`: reads one slot of an indirection vector.
- 32 `StoreVector`: writes one slot of an indirection vector.
- 33 `IntegerBinary`: an integer leaf's inlined fast path (`operation`,
  `destination`, `left`, `right`, `target`), encoded as the operation byte,
  three `u16` registers (destination, left, right) and the `u32` jump target,
  in that order. The operation byte is `IntegerOperation` as
  `image::encode::integer_operation_tag` maps it, exhaustively: 0 `Add`, 1
  `Subtract`, 2 `Multiply`, 3 `FloorDivide`, 4 `Less`, 5 `Greater`, 6
  `LessOrEqual`, 7 `GreaterOrEqual`, 8 `Equal`. This byte is its own space,
  not an instruction tag -- assigned once and never renumbered or reused by
  the same rule, but nine values wide rather than one, since nothing outside
  `lower_integer_binary` and the VM arm ever reads it standalone.
- 34 `ReturnSlotIfFilled`: a lazy slot's getter's first instruction
  (§6.3.3), its `own_index` a `u32`: returns `self`'s own slot at that index,
  addressed as `InitializeSlot` addresses it, when it holds anything but
  `nil`, and otherwise falls through to the initializer.
- 35 `LoadEnclosingObject`: `outer N` with no message after it
  ([dispatch.md](dispatch.md), "Shapes the resolver must recognize"),
  `destination` and `depth`, both `u16`: loads the kth enclosing object of the
  running method's defining class, the object `outer N m` sends to.
- 36 `MakeFuture`: §6.3.2, a new unresolved future of a closure,
  `destination` and `closure`, both `u16`.
- 37 `ResolveFuture`: a value seen through a future, forcing it first if it is
  unresolved, `destination` and `source`, both `u16`.
- 38 `LoadOwnSlot`: `self`'s own slot read back without a getter send,
  `destination` a `u16` and `own_index` a `u32`, addressed as `InitializeSlot`
  addresses it.
- 39 `InitializeSlotProvisionally`: `InitializeSlot` without §3.1.1's
  value-slot check, for a simultaneous clause's future only, `source` a `u16`
  and `own_index` a `u32`.
- 40 `SignalNonBoolean`: an inlined loop's guard failure, `register` a `u16`:
  signals `NonBooleanReceiver` for the condition's answer in it. Like a
  return, it may end a body.

The next free instruction number is 41. A send's `CallSite` -- its number
within its code family, a `u32`, or `u32::MAX` (`CallSite::UNCACHED`) for
every send the reference lowering emits ([compiler.md](compiler.md),
"Safepoints and call-site identity") -- is not among its operands at all: see
the call-site paragraph below.

Literals:

- 0 `Nil`: `nil`.
- 1 `Boolean`: `true` or `false`.
- 2 `SmallInteger`: an integer that fits a 64-bit signed word.
- 3 `Float`: a 64-bit floating-point number.
- 4 `String`: a string literal.
- 5 `Symbol`: a symbol literal, `#name`.
- 6: retired. It was `Character`, the character literal, until `"c"` became
  a one-character string (2026-09-25, [frontend.md](frontend.md), "Lexer");
  a stale artifact's tag-6 literal meets an unknown tag (`RETIRED_LITERALS`).
- 7: retired. It was `Array`, the literal array `#( ... )`, whose elements
  nested, until the syntax went (2026-09-27, [frontend.md](frontend.md)):
  neither the specification nor the canonical grammar has it, and a tuple,
  `{ ... }`, is the array literal (§5.1.7).
- 8: retired. It was `MalformedInteger`, an integer literal beyond `i64`
  that raised its error when it ran, until every integer literal became
  representable (2026-09-29); it carried the literal's text, which tag 10
  does not.
- 9 `UnresolvedIdentifier`: an identifier that names nothing, a module
  root's clause receiver; it raises its error when it runs.
- 10 `LargeInteger`: an integer literal beyond `i64`, in the same wire form
  as value tag 9 below -- a sign byte, a `u32` digit count and that many
  `u64` digits, least significant first -- so the decoder parses no text.
- 11 `BodyTooLarge`: no payload; what the body standing in for one past the
  register ceiling loads first. It raises its error when it runs
  ([compiler.md](compiler.md), "Compilation cannot fail").

**A count past its field refuses the image, never aborts it, except the
count of heap objects.** A count a
program decides is written through a checked narrowing that answers an
error, `image encoding cannot write a <what> of <n>: it does not fit the
image format's 16-bit field` (or `32-bit`): in 16 bits a register list, a
closure's parameter and copied-value counts and a method's or factory's
arity (`encode::u16_field`), all of which a valid program can pass
([compiler.md](compiler.md), "Compilation cannot fail"); in 32 bits a
string's, a symbol's, a value list's and a `LargeInteger`'s digit count
(`encode::u32_length`). Every other count goes through the asserting
`encode::u32_field`, because no program is meant to pass it: object ids, a
frame's among them, and counts of heap objects, which only the heap's
memory bounds since a handle became a 64-bit offset, so a write past about
four billion objects aborts rather than refusing
([OPEN-WORK.md](../OPEN-WORK.md)); declaration, statement and instruction
counts by source, whose `Span` offsets are 32-bit --
`ns_syntax::lex`, which `parse` goes through, refuses a source of more than
`u32::MAX` bytes with a parse diagnostic before lexing it -- and the symbol
table's count by the two together.

**A call-site number is a property of the code, never a byte on the wire.** It
is feedback-table bookkeeping, not program meaning: the decoder assigns every
family's numbers itself, in the compiler's own emission order, once the whole
code table is read (`decode::number_call_site_families`), rather than trusting a
number the file claims, writing each into the decoded body in place through
`EncodedBody::for_each_emitted_mut`. A code record carries one flag, whether its
sends were compiled uncached (the reference lowering never numbers a send at
all, so it marks the whole body rather than leaving each send to say so), and
one count, `copied_count`, so that a block's register layout is known from its
code alone. `outer_reach` is not on the wire either: the decoder derives it as
the compiler does, in `CompiledCode::from_builder`, which works because a
block's code precedes its parent's in the table. A body's `family_size` is not
on the wire either way; both sides derive it from the sends' numbers and the
blocks' sizes the same way, read off the encoding as `bytecode::family_size_of`
reads the decoded view. A block precedes its parent in the table, but its
numbers depend on where its `MakeClosure` sits in its root's emission order, so
the decoder numbers a family only once its root -- the body nothing else's
`MakeClosure` creates -- has been read in full. The golden image is 1363 bytes.

Both lists are declared once, in `image/opcodes.rs`, and used by the encoder and
the decoder alike rather than repeated as magic numbers on each side.
`crates/ns-runtime/tests/opcode_tags.rs` pins every value and checks that no two
share a number, so a renumbering fails the suite at the moment it happens instead
of surfacing later as an image that no longer means what it said.

The graph section's six tag families live there under the same rule: how a
`Value` is encoded, an object record's body, how a class record names its
mixin, how a class reference is named, what a frame was activated for, and
which tag-named object an entry of the closing hash table names (`HASHED_*`,
below). The shipped names gave four of them a tag each, followed by a `u32`
index into the shipped-name section: `VALUE_SHIPPED_MODULE` = 11,
`MIXIN_SHIPPED` = 2, `CLASS_SHIPPED` = 8 and `HASHED_SHIPPED` = 7
("Shipped names" below). They were bare
literals in the encoder and again in the decoder until `inspect` became a
third reader of those bytes — and three hand-kept copies of one numbering is
how a viewer starts quietly disagreeing with the loader. A closure frame
(`FRAME_CLOSURE`) names the closure object it runs by object id: the closure's
data sits inline in its object, so that the tracer can rewrite its two frame
handles in place ([memory.md](memory.md), "What the collector traces").

**A `LargeInteger` is a value on the wire, never a record.** Value tag 9,
`VALUE_LARGE_INTEGER`, carries it inline wherever a value goes -- a field,
an element, a register, a copied value -- as a sign byte (0 or 1), a `u32`
digit count and that many `u64` digits, least significant first; literal
tag 10 carries a literal in the same form. It has no identity (§3.1.1), so
the graph walk gives it no object id and two references to one object load
as two equal integers, which is all a value promises. Only the canonical
encoding decodes: a count bounded against the bytes left before its digits
are allocated, a sign byte of 0 or 1, no zero top digit, no negative zero,
and nothing that fits `i64`, which travels as `VALUE_SMALL_INTEGER` or
`LITERAL_SMALL_INTEGER`; anything else is `DecodeError::NonCanonicalInteger`
(or `InvalidTag` for the sign), so each integer has one encoding on the wire
and one representation after a load (`a_non_canonical_large_integer_is_refused`,
`crates/ns-runtime/tests/image.rs`). The decoder makes the object through
`Mutator::new_integer`, which counts toward a failed load's peak
([memory.md](memory.md), "The amplification bound of a rejected load").
`inspect` renders one as `large-int <decimal>`, through the same
bounded read (`decode::read_large_integer_parts`), without the canonical
checks, since a structural walk reports what the bytes say. The decimal is
bounded too, in `inspect` and in `disasm`'s literals alike
(`ArbitraryInteger::to_bounded_text`): decimal conversion is quadratic in
the digit count, so an artifact holding one integer of millions of digits
would keep either tool busy for hours. Up to `DECIMAL_RENDERING_LIMIT`, 64
digits (about 1,233 decimal digits, rendered in microseconds), the full
decimal is printed; past it, the digit count and the hexadecimal of the
first and last two digits, `large-int <100000 words>
0x186a0000000000001869f...00000000000000020000000000000001`, which costs
the same at any length
(`inspect_renders_a_huge_large_integer_elided_and_a_normal_one_in_decimal`,
`crates/ns-runtime/tests/image.rs`).

**A `Fraction` is a value on the wire too.** Value tag 10,
`VALUE_FRACTION`, carries it inline as its numerator and then its
denominator, each a `VALUE_SMALL_INTEGER` or a `VALUE_LARGE_INTEGER` in its
canonical form; it has no identity (§3.1.1), so the graph walk gives it no
object id and its parts are no objects of the graph. Only a fraction in
normal form decodes -- a denominator above 1, no factor the two parts
share -- since `=` and `hash` compare the parts; anything else is
`DecodeError::NonNormalFraction`, and a part under any other tag is
`InvalidTag`, both refused by the first pass before anything is allocated
(`a_fraction_outside_its_normal_form_is_refused`,
`crates/ns-runtime/tests/image.rs`). The decoder makes it through
`Mutator::new_fraction`. `inspect` renders one as `fraction
<numerator>/<denominator>`, each part as its integer tag renders.

**Integers and floats on the wire** are independent of where a
`SmallInteger` ends or which floats are immediate, so an artifact does not
depend on the value representation. An integer that fits `i64` is written as
an `i64`, `VALUE_SMALL_INTEGER` or `LITERAL_SMALL_INTEGER`, whichever
representation it has in memory, and every other one as value tag 9 or
literal tag 10; `read_large_integer` still refuses a payload that fits
`i64`. The decoder decides by the 61-bit range: an `i64` within it loads as
a `SmallInteger` (`Literal::SmallInteger`), and one beyond it as a
`LargeInteger` (`Literal::LargeInteger`), so an integer between 2^60 and
2^63 travels as an `i64` and loads as a `LargeInteger`
(`a_large_integer_in_i64_travels_as_an_i64_and_loads_as_a_large_integer`).
A float, immediate or boxed, is `VALUE_FLOAT` and an `f64`, with no record,
since a boxed `Float` is a value with no identity; the decoder answers an
immediate `SmallFloat64` or a boxed `Float` through `Mutator::new_float`,
from the value alone (`a_boxed_float_value_round_trips_without_a_record`).
Neither needs a tag of its own. A decoded integer beyond 61 bits and a decoded
boxed float allocate, and count toward a failed load's peak
([memory.md](memory.md), "The amplification bound of a rejected load").

**Mixin table.** One row per declaration: the name, its own slots, the primary
factory selector and arity, the compiled methods as selector → code id, the ids
of its nested declarations with their accessor names and accessibility, the
class-side mixin id, the initializer code id, and the provenance span. This is
what replaced the embedded source text and the re-parse on load.

The table is a **forest**: one tree per module, each in `mixin::preorder`
order, the trees in module order (`image/mixins.rs`). A tree's first row is
its module's root declaration, so the module section's count must equal the
number of trees the rows form (`DecodeError::ModuleCountMismatch`): a surplus
tree would be declarations no module names, a missing one a module without a
root. Row ids and the nesting ceiling cover the whole forest, and a row's
nested declarations must lie in its own tree (`MixinTableNotATree`).

A slot is its name, its kind and its accessibility (the access tags a method
uses), because those three decide which accessors it defines (§6.3.2). The kind
is one of `opcodes.rs`'s `SLOT_*` tags: 0 immutable, 1 mutable, 2 lazy and
immutable, 3 lazy and mutable (§6.3.3); the two eager kinds are the mutability
tags the record carried before lazy slots. The accessors themselves are not
written: the decoder derives them from the slots, as building a mixin from
source does. A lazy slot's getter is the exception: it carries the
initializer's compiled body, so it is written as one of the row's compiled
methods -- a body that opens with `ReturnSlotIfFilled` (tag 34) and carries no
fast path, so the decoder restores nothing from the slot record for it -- and
a row whose lazy slot has no unary method of its name and accessibility among
its methods is refused (`LazySlotWithoutGetter`). A mutable
lazy slot's setter is derived like any other. A row in which two members would
share a name — a compiled method, an eager slot's getter, a mutable slot's
setter, a nested declaration — is refused before any mixin is built, so a
crafted row cannot make one member silently hide another.

A row also carries its `<:` clauses (`newspeak-spec.pdf` §6.2), in declaration
order: each one's selector, whether its message takes arguments, and the code id
of that message's body. How each clause's *receiver* is identified is not
repeated there: the superclass clause's body evaluates the superclass and every
`<:` receiver together and answers them as one array
(`interpreter::nested::superclass_clause_expression`), so the row's clause
count is also how many named classes that body answers beside the superclass.

**Class objects are ordinary objects in the graph** (object body tag 7),
encoded as `{MIXIN_ROW, row id, superclass, enclosing object, declaring
class, applied clause?}`. The declaring class has to be there because `outer`
walks the declaration chain rather than the receiver's dynamic one, so an image
that dropped it would come back resolving `outer` from the wrong chain. The
applied clause is present exactly for a level a `<:` clause added, as the
applying declaration's row id and the clause's position in that row, so the
selector and message a level is initialized with are the ones its row carries
and `verify_mixins` reconciled with that row's factory. A level a program's
clause added over a shipped class (`<: Value`, or a `List` the program holds,
`<: listClass new: 4`) is recorded as `{MIXIN_SHIPPED, shipped-name index,
superclass, applied clause}`: its mixin is the shipped class's, which no row
describes, and its enclosing object and declaring class are the shipped
class's own — for `Value`, the core library's `Kernel`, which holds the VM
mirror — so the record carries neither, and the decoder takes both from the
loading bootstrap, as building the level from source does. The name must be
one the rule allows as a superclass ("What may be named where" below). The
record changed shape when its mixin became a tagged reference, so the old
class-record tag 5 is retired (`opcodes::RETIRED_OBJECT_BODIES`); mixin
reference 1, `MIXIN_BUILTIN`, which named a builtin's mixin by builtin class
tag, is retired too (`RETIRED_MIXIN_TAGS`).

**A class record's header names its metaclass record** (object body tag 8,
`OBJECT_METACLASS`): every class object's class is its metaclass (§3.2), so a
class record's class tag is `CLASS_RECORD` with the id of a record whose own
header is the builtin `Metaclass` and whose body is one class reference, back
to its class record. Nothing else about a metaclass is written — its mixin,
application, enclosing object and declaring class follow from its class
([object-model.md](object-model.md), "The image format") — and the decoder
applies it over `Class` from its class's row (`rebuild_metaclasses`). The
metaclass of a class that is no record — a shipped class or level — is
named by its own shipped name, of kind metaclass or level metaclass, under
`CLASS_SHIPPED` like any shipped class reference. The encoder refuses a class
record whose class object has no metaclass rather than write a header the
decoder could not pair. A metaclass
record is a record like any other, sized and allocated as one placeholder, and
the smallest record there is: counted with a two-byte class tag, 9 bytes
for a class object of 80, about 8.9x, which with the decoder's own tables
sets the bound a rejected load reaches in a release build, about 10.7x of
the graph section's bytes; in a debug one the root list sets it, at 16x.
Every live class tag is five bytes, so a metaclass record is at least 15
and the release figure an upper bound ([memory.md](memory.md), "The
amplification bound of a rejected load"); a shipped metaclass is no
record at all and allocates nothing.

**Module section.** Which top-level modules the artifact carries
(`image/modules.rs`): a `u32` count, equal to the mixin table's trees, then
per module, in module order, its origin, its source text when the header
sets `FLAG_HAS_SOURCE`, and the `u32` object id of its root class's record
in the graph. **Module order** is the
application module (a project file's `main`) first, then a project file's
modules in the order written, then the embedded `Manifest.ns` ("Packaging"
below); a source file's program is its one module. The count is bounded by the bytes left before anything is
reserved for it.

The **origin** is a tag from `opcodes.rs`, `ORIGIN_FILE` (0) followed by the
path the module was read from, or `ORIGIN_EMBEDDED` (1) followed by the name
of a source compiled into the binary -- `Manifest.ns`'s. It is provenance, in
every artifact, source or not; `inspect` shows it. Source travels for every
module or for none, as the header's one flag says, so the encoder refuses a
set of modules of which only some carry theirs rather than invent an empty
text.

A module's **root class is a record of the graph** like any class: a
`MIXIN_ROW` class record applying the module's root row, with a metaclass
record of its own, and the level its `<:` clause adds a `MIXIN_SHIPPED`
record applying `Kernel Value`. The encoder walks every module's root class
after the image's roots, so a root no value of the graph reaches still has
its record, and a root the image's roots reach keeps the id that walk gives
it; the module section names each by that id. Once the graph is decoded,
the loader holds the record each id names to its module
(`modules::module_root_classes`): an id past the object table is
`BadObjectId`; a record that is no class applying the module's own root row
as its outermost level -- a metaclass, a level, a class of another row, one
with an enclosing object or a declaring class -- is
`ModuleRootNotItsRecord`; a superclass chain the root rule does not build
is `RootRule`. The root rule, as both sides check it
(`modules::root_record_defect`, which the encoder and the decoder share, so
the encoder never writes a root the loader refuses): a root row with no
`<:` clause has `Kernel Object` or `Kernel Value` as its superclass, `Value
new` included; a row with its one permitted clause has as its superclass
the level that clause, index 0, added, applying `Kernel Value`'s mixin over
`Kernel Object`; a row with more clauses is refused
([object-model.md](object-model.md), "Mixin application", "The root
rule"). A module is named by its index and never by its name, which is why
two modules may share one ("Packaging" below). The class tags 1
(`CLASS_USER_ROOT`) and 3 (`CLASS_ROOT_LEVEL`), which named *the* root class
of an artifact that carried one module, 6 (`CLASS_MODULE`) and 7
(`CLASS_MODULE_LEVEL`), which named a module's root and its levels by module
index, and 4 (`CLASS_METACLASS`), which named their metaclasses, are retired
(`opcodes::RETIRED_CLASS_TAGS`), with the per-module root section that
described the root's superclass by builtin class tag: a stale image's
reference meets an unknown tag. A second class record applying a module's
root row, beside the one the module section names, is not refused
([OPEN-WORK.md](../OPEN-WORK.md)).

**Shipped names.** Immediately before the graph: a `u32` count, then per
name the module's symbol id, the declaration's symbol id -- for a module's
root, the module's own name -- and a kind byte: 0 the module instance, 1
the class, 2 its metaclass, 3 a level, followed by the `u32` index of the
clause that adds it, 4 that level's metaclass, followed the same way
(`image::shipped_names`). The encoder lists each name the graph and its
closing hash table use once, in the order they first use each, so one
world encodes to one byte sequence; a reference in the graph is its tag and
a `u32` index into the list. The section's reader refuses more than
`MAX_SHIPPED_NAMES` (65,536) entries (`TooManyShippedNames`), a count past
the bytes left at the 9 bytes of the smallest entry, a symbol id past the
symbol table (`BadSymbolId`), a kind byte no kind has (`InvalidTag`) and a
clause index at or past `CLAUSE_INDEX_LIMIT`, which no class object can
number (`UnknownShippedName`); it asks no world, so `inspect` reads the
section the same way. How the loader resolves the names, and what a name
may stand for where, is "Shipped names" below. A started image names only
the kernel's classes and levels, never a module instance; an image
`image::snapshot` writes names whatever its graph reaches of the shipped
world, the module instances only in a frame.

**Object table.** Every heap object reachable from the roots — instances,
strings, symbols, arrays, byte arrays, closures, class and metaclass
records, reified frames — after a `u32` count, each record a header (its class tag and its
`u32` identity hash) and a body under a body tag, so cycles and shared
references round-trip with identity preserved. A closure encodes its compiled
body as a code id, its captured frame as the object id of its frame record,
and the values it was handed when it was created: a `u16` count and that many
encoded values. It encodes neither its parameter names, which nothing reads —
the one thing anything asked of them was how many there were, and that is its
body's `argument_count` — nor its home, which `Heap::closure_home` derives
from the captured frame. That is a different record shape, so the closure
body tag 4 is retired (`opcodes::RETIRED_OBJECT_BODIES`) and the record is 6.

**Frames are object records.** A reified frame is an object of the frame
format, an instance of the kernel-private `Activation`
([object-model.md](object-model.md), "The object layout"), and it is written
as one: `OBJECT_FRAME` (body tag 10) under a header that names
`Kernel Activation` by its shipped name (`CLASS_SHIPPED`) — the one record
an image may give that class; a frame record under any other header is
`FrameRecordUnderAnotherClass`, and `Kernel Activation` as the header of any
other record is `ShippedNameNotAllowed`, as it is anywhere but a record's
class and a frame's class ("What may be named where" below). The body is, in
order: the frame's kind tag (`FRAME_*`) and that kind's payload — a method
frame's selector symbol id, a closure frame's closure object id, nothing for
a top-level frame, a class tag for the four class-carrying kinds — a `u32`
`program_counter`, an instruction index into the frame's compiled body;
`uncontinuable`; the receiver; the `sender` and the `home`, each a presence
flag and an object id; the `current_class`, a presence flag and a class tag;
then a `u32` register count and that many values — every register the frame
object has, `nil` where its activation copied none. A reference to a frame —
a closure's captured frame, a frame's sender or home — is an object id that
must name a frame record (`NotAFrameReference`), checked as it is read, since
every placeholder exists by then; that a closure frame's kind names a closure
(`NotAClosureReference`) is checked once every record has been read, as a
class reference is. A `<:` clause message's frame
(`FRAME_MIXIN_APPLICATION_MESSAGE`, 6) names the *level* its clause added
rather than the applying class: the applying class's mixin carries one
message body per clause, and the level's applied clause says which one ran,
which is how `verify_graph` finds the body to reconcile the frame with. A
block written in such a message captures that frame, so a snapshot can hold
one. The kernel's `Activation` object for a frame is the frame object itself,
so an activation object in a graph is this one record. There is no frame
table: tag 9, the activation record whose body was one frame-table id, is
retired with it (`RETIRED_OBJECT_BODIES` is 4, 5 and 9).

**A `ByteArray` is a record of its own**, `OBJECT_BYTE_ARRAY` (body tag
11): a `u32` length, then that
many bytes, in order, under a header that names `Kernel ByteArray` by its
shipped name. Only the bytes travel: the three reserve words ahead of the
data are not written, since the decoder places the data at its own
placeholder's alignment ([memory.md](memory.md), "Every mover realigns a
`ByteArray`'s data"). A length past the bytes left is refused before
anything is allocated, a byte-array body under any other header and a
`ByteArray` header over any other body are `ByteArrayRecordMismatch`
("Verification" below), and `inspect` prints the record as its size and
its first 16 bytes in hex, "byte array of 3 bytes [01 02 03]". A typed array, a `DataView` or a `StringBuilder` is
an ordinary instance record of its shipped class, its slot holding the
`ByteArray`'s record ([core-library.md](core-library.md), "`ByteArray`,
`StringBuilder` and the views").

**A `WeakArray` and an `Ephemeron` are records of their own**
(`opcodes.rs`, "Next free number: 14"): `OBJECT_WEAK_ARRAY` (body tag 12),
a `u32` count and that many values, its slots in order, under a header
naming `Kernel WeakArray`; and `OBJECT_EPHEMERON` (13), a `u32` field count
and that many values, the key first, under a header naming `Kernel
Ephemeron`. **The encoder writes what a collection would keep**: a weak
slot names its object only if the graph reaches that object strongly
otherwise, and is `nil` if not; an ephemeron's value and finalizer are
followed only once its key has been reached, to a fixpoint, and an
ephemeron whose key never is is written with every field `nil`
(`GraphTable::build`, `GraphTable::weakly_held` in `encode.rs`). The
exception is what takes no record: a weak word or a key holding an
immediate, a shipped name or an object written by value -- a
`LargeInteger`, a boxed `Float`, a `Fraction` -- is written as it is
whether or not anything else reaches it, since a collection's timing is
unspecified and keeping more than one would is never wrong. A weakly held
object is still met by the walk, so a weakly held unnamed shipped class or
the VM mirror refuses the image as a strongly held one does. The decoder
refuses a weak-array body under any class but `WeakArray`
(`WeakArrayRecordUnderAnotherClass`; `WeakArray`'s header carries an
instance record too, the slotless one its factory `withoutSlots` makes),
an ephemeron body under any class but `Ephemeron` and any other body under
`Ephemeron`'s (`EphemeronRecordMismatch`), and an ephemeron record whose
field count is not the class's (`EphemeronFieldCount`). Nothing is cleared
at load, since the decoder never collects ("The image under
generations"), and `inspect` prints the records as "weak array [...]" and
"ephemeron {...}" ([core-library.md](core-library.md), "Weak
references").

**Two passes.** Records reference each other freely and forward, so every
record's object must exist before any is filled. `decode_graph_section`
therefore reads the object table twice. The first pass (`Sizing`) reads
every header and body exactly as the second will and stores nothing: values
are parsed and dropped, ids range-checked against the count, shipped-name
indices, code ids, mixin rows, clause indices and symbol ids resolved, each
shipped name held to the position it stands in, a symbol
record naming a symbol-table entry already named refused (`DuplicateSymbol`),
a `FutureImplementation` record of a field count other than two refused
(`FutureRecord`), a record of a shipped class that is no builtin
refused unless it is an instance record (`ShippedInstanceBody`), and a
byte-array body and a `ByteArray` header refused unless they come together
(`ByteArrayRecordMismatch`), a weak-array body under another class than
`WeakArray` (`WeakArrayRecordUnderAnotherClass`), and an ephemeron body
and an `Ephemeron` header refused unless they come together
(`EphemeronRecordMismatch`) and with the class's field count
(`EphemeronFieldCount`); each record yields its object's size, read from its own
bytes — the kind comes first in a frame record, the register count, a
byte array's length and a weak array's or an ephemeron's count are on the
wire, so no size depends on another record. If the objects would take
more than eight words a byte of the input left, the load is refused
(`UnexpectedEndOfFile`) before anything is allocated; a symbol's text is not
counted against that, since it is bounded once per entry by the symbol
section. Then every record's placeholder is allocated at exactly its size —
an instance of the builtin `Object` with nil fields, or for a frame record a
frame object of its register count — and the second pass fills each in place:
a header and class word rewritten, each value stored as it is read, a string
packed straight from the buffer, a byte array's bytes copied to the data
words at its placeholder's 32-byte alignment, the header's format becoming
`AlignedBytes`, a weak array's or an ephemeron's header taking its format,
`WeakPointers` or `Ephemeron`, a class record's eight fields copied in
(`Heap::shape_placeholder` and the `fill_placeholder_*` and `fill_frame_*`
accessors), so no record is ever built beside its placeholder. A frame
record's body is filled last, from its remembered position, once the
applications are rebuilt and the headers stamped, since a method frame's
code is found through its class's application. What that
bounds a rejected load's peak at is "Verification" below.

**Identity hashes are preserved verbatim** for every object the image carries
as a record. A hash is 22 bits of the header word, so a hash of 2^22 or more
is refused (`IdentityHashTooWide`). After a load the identity-hash counter is
advanced past the largest hash the image contained, so a hash assigned
afterwards does not collide with a loaded one until the counter wraps within
its 22 bits, which a hash may ([object-model.md](object-model.md), "The
identity hash is 22 bits wide"). A record's 0 means "not yet assigned" and is
preserved the same way.

**The exception is an object the image names rather than carries**: an
object of the shipped world -- a module instance, a shipped class, its
metaclass, a level and a level's metaclass. None of them is a record, so
nothing about a record could carry its hash. Instead the graph section
closes, after its roots, with a table of exactly the shipped objects whose
stored hash is not 0, in the shipped world's own order -- per module, the
instance, the root class, its metaclass and levels, then every class the
root declares in declaration order, each with its metaclass and levels --
each entry `HASHED_SHIPPED` (hash tag 7), the `u32` index of the object's
name in the shipped-name section, and its hash
(`encode::encode_tag_named_hashes`). Every object of the shipped world is
considered, not only those the graph reaches, since a program can keep
`Integer hash` in a slot without keeping `Integer`; a name the graph did
not use is added to the section for its entry. An index past the section
is `BadShippedName` (`a_tag_named_hash_past_the_shipped_names_is_refused`).
Every other hash tag is retired (`RETIRED_HASHED_TAGS`, 0 to 6): the
builtin classes', the platform's and the platform paths' entries, which a
shipped name replaced, and the module roots', their levels' and their
metaclasses', whose hashes travel in the headers of their records now.
`load` restores each hash onto the object
the bootstrap built (`decode::decode_tag_named_hashes`), and refuses a 0, a
duplicate entry or a hash of 2^22 or more (`IdentityHashTooWide`) like any
other malformed hash. The identity-hash counter is then advanced past the
larger of this table's maximum and the object table's, so a hash assigned
afterward does not collide with either before the counter wraps.
`inspect` reads the table as part of the graph section
(`Report::graph_tag_named_hashes`).

**A module's `<:` levels are records**, whose hashes travel in their headers,
although no program can obtain one as a value today — there is no
reflective `superclass` — and the one thing that names a level in a graph
besides its class's superclass is a frame's `current_class`, when a method
of the shipped class the clause named, a kernel method, runs on an instance
of the module's root class (`a_module_levels_own_hash_survives_an_image`,
`crates/ns-runtime/tests/image.rs`). A shipped declaration's own levels --
the levels the kernel mixin `IndexedCollection` adds under `Array`,
`ByteArray` and `TypedArray`, named `Kernel Array level 0` and the like --
are shipped names and have hash entries like every shipped object
(`the_hash_of_a_shipped_level_survives_a_snapshot`).

That is what Cog and Primordial Soup get without trying: their images carry
every object, classes included — a Spur class's identity hash is even its index
in the class table (`ensureBehaviorHash`, `enterIntoClassTable`) — so no hash
is ever rebuilt from anything. ns-next's bootstrap rebuilds these objects
instead, so the image has to carry what the program observed of them. That
stays true with the core library in Newspeak
([core-library.md](core-library.md)): the shipped world stays out of the
image ("How upstream compares" below), so it stays named. The alternative, a
deterministic hash for each named object from a reserved range, was
rejected: every shipped object added later would need a reserved number, and
the hash would no longer be the one the program was actually given, which is
what "verbatim" promises for every other object.
`a_hash_given_to_a_tag_named_object_survives_build_and_load` in
`crates/ns-cli/tests/build_load.rs` pins the result end to end.

## How frames actually get into an image

Only as objects something reaches. A running activation lives on the
activation stack and is not in the object graph at all; a frame object exists
for it only once something reified it -- a block capturing it, or the
exception machinery naming it as an `Activation` object, which is the frame
object itself ([execution.md](execution.md), "Activations"). So a frame record
enters the object table only as a closure's captured frame — a closure a
value names, or the one a closure frame already in the table runs — as an
`Activation` object a value names, or as the `sender` or `home` of a frame
already in it. (A closure has no `home` field of its own to bring one in:
`Heap::closure_home` derives it from the captured frame.) An image is written
at a **quiesced** point — `build` writes one once the application's factory
and initializers have returned and before `main:args:` is sent, and a host
calling `image::snapshot` between sends, with the Rust call stack already
unwound and the activation stack empty; the one writer refuses to
encode otherwise — so every frame in it is dead:
divorced when its activation returned, or decoded from an earlier image and never
run. Such a frame has no sender, and holds its registers' values only when
something could still read them by reference -- its other registers are
`nil`, written as one byte each -- so a closure brings in the frames it
names, the values it copied, and nothing of the call chain that created
them, and the table never holds a suspended computation.

The format still records a `sender` per frame, and the decoder restores one if a
record carries it; only such a decoded frame can carry one into a later
snapshot.

The mechanism exists because the liveness design requires a running world,
including an open debugger session, to round-trip. What is missing to actually
use it that way is control flow, not format; see "Planned: suspend and resume".

## Source

The front end is not on the load path. `load` decodes the mixin table instead of
re-running parse → resolve → bootstrap → `load_module`, which also keeps the
parser away from attacker-controlled text.

**Optional source text** remains behind a header flag, `FLAG_HAS_SOURCE`,
in the module section, one text per module —
`build --debug-source` writes every module's, `load --print-source` prints
them in module order, one after another. It is never
parsed, never resolved, never executed. It exists so that the liveness
requirement "an image can be exported back to `.ns`" is met today, while the
decompiler that is supposed to meet it properly does not exist.

## Verification

Two structural passes run before anything the image describes can execute:
`verify_code_table` over the decoded code table, and `verify_graph` resolving
each frame's body off the decoded mixins that carry it. Between them,
`verify_mixins` reconciles every decoded mixin with the bodies its row attached
— each body's kind and arity against the declaration's, and a body present
wherever the declaration needs one. `mixins::decode` attaches without checking,
so `inspect` can still display an artifact whose halves disagree; only `load`
refuses it. A corrupt but version-matching image is an error there, never a
panic in the VM later.

**The loader builds before the graph, and only what the binary has.** After
the first passes and before the tenure, the loader builds the shipped modules
and classes the shipped-name section names that the boot did not
(`Interpreter::build_shipped_names`, "Shipped names" below). It checks every
name against the binary's declarations first and refuses an unknown module or
declaration, a level past its clauses, a duplicate, and in a started image a
name of any module but the kernel or of any module instance, the kernel's
included ("The section" in "Shipped names" below), before it builds anything,
so a refused image builds nothing; what it then runs is the binary's own code
-- a module's factory and initializers, a class's accessor -- as a program's
first demand runs it, never anything the image describes; the image chooses
only which of the binary's few modules and classes are built.

**A closure's copied values are reconciled with the code that runs them.** Both
halves come off the wire independently, so a crafted image can make them
disagree: a `MakeClosure`'s `copied` list against its block's `copied_count`
(checked in `lexical_parents`), a closure record's own values against the same
count (checked in `decode_closure_record`, before any of them is read), and
`argument_count + copied_count` against `register_count` (checked in
`decode_code`, in `u32` arithmetic, once the arity itself fits). Every register
a `MakeClosure` copies, and every register operand of `MakeVector`,
`LoadVector` and `StoreVector`, is inside its own body's register file, like
every other register operand; a vector slot is not a register and is checked at
run time, since a vector's size is a run-time value.

**A frame record's register file matches its body.** Every reified frame is
allocated with its code's full register file ([object-model.md](object-model.md),
"The object layout"), and the encoder writes every register it has, `nil`
where its activation copied none ([execution.md](execution.md),
"Lifecycle"), so `verify_graph` refuses a frame record whose register count
is anything but its body's `register_count` (`FrameRegisterFile`; a frame
record with no registers for a body that has some is refused,
`a_frame_with_no_registers_for_a_body_that_has_some_is_refused`). A crafted
image therefore cannot pair a by-reference closure with a frame too small
for the registers its code reads, and no frame object ever grows.

**`Instruction::InitializeSlot`'s and `ReturnSlotIfFilled`'s `own_index` is not
one of the counts these passes bound.** `verify_mixins` checks a body's kind and
arity against its declaration, but nothing checks that a slot instruction's
index falls within the mixin's own slot count. This is harmless at run time:
`Heap::instance_set_field` and `Heap::instance_field` are total, so an
out-of-range index writes nothing and reads as an empty slot, and an in-range
but wrong one writes or reads a different slot of the instance -- silently
wrong output, never a panic. That is acceptable for
the reason `CLAUDE.md`'s "An image is untrusted input" gives: starting a
packaged application already runs its own code, with the same exposure as
running the equivalent source, so a wrong slot from a malformed index is no worse
than any other wrong answer a mutated program's own code can produce.

**A mixin application is checked against the row that declares it.** Every
`<:` clause's message body is a clause-message body taking exactly the row's
factory arity, since §6.2 evaluates it with the applying declaration's factory
arguments (`CodeArityMismatch`, `WrongCodeKind`), and a clause whose message
takes arguments has one (`MissingMixinApplicationMessage`) and a clause whose
message takes none has none (`UnexpectedMixinApplicationMessage`), so a row and
the frames running its clauses agree on what each clause runs; a row with `<:`
clauses but no superclass clause is refused
(`MixinApplicationWithoutSuperclass`). A level's record names an applying row
and a position that must exist (`BadMixinId`, `BadMixinApplicationClause`) and
fit the 23 bits a class object's packed applied clause gives it
(`ClauseIndexTooLarge`, at 2^23 or more, the limit the resolver holds a
source to; [object-model.md](object-model.md), "The object layout"), and
a module's root class record is held to that module's own root row and to
the root rule -- never another module's (`a_module_root_is_held_to_its_own_modules_row`;
"Module section" above).
Levels whose superclass links close on themselves have no order to be rebuilt
in and are refused like any other class records that do (`SuperclassCycle`).
What is not checked is that a level sits directly above a class of its applying
row; the instantiation walk re-checks a clause body's arity against the
arguments it is given and raises a Newspeak error rather than indexing past
them.

**A class record and its metaclass record are held to each other both ways.**
A class record's header must name a metaclass record whose reference back is
that class record, a metaclass record's reference back must name a class
record, every metaclass record must be named by exactly one class record's
header, and a metaclass record's own header must be `Metaclass` — anything
else is `MetaclassPairing` (`rebuild_metaclasses` and `pair_metaclasses` in
`decode.rs`). A live class has exactly one metaclass (§3.2, "unique"), so this
refuses nothing a snapshot writes, and it is what keeps a metaclass's identity
across a round trip: the metaclass a slot holds is the one its class answers.
Metaclass records are applied before the class records, since each needs only
its class's mixin and `Class`'s application, so a crafted class record whose
superclass is a metaclass record is applied over the metaclass's real
application. An object record that is not a class object but whose class's
application refuses instantiation as a class object — an instance of `Class`,
`Metaclass` or a metaclass, which no source can make — is
`InstanceOfAClassClass`.

**A root the root rule forbids is refused on both sides.** The encoder refuses
a module whose root mixin carries a root defect, in `image::package` and
`image::snapshot` alike, with "image encoding cannot serialize `M`: a
module's root class may inherit only Object or Value, or write Object <:
Value, and …" — the reason the instantiation refusal gives: such a root can
never be instantiated, so an artifact of it could only fail, and refusing it
needs no wire field. From source a root defect is a resolve error, so `run`
and `build` refuse such a project alike, before anything runs, whether or not
the application would instantiate the module; the encoder's refusal guards a
`ResolvedModule` a host assembled past `resolve`. Before the walk the
encoder also holds each module's root class to `modules::root_record_defect`
("root object is not a class", "… is a metaclass, a level, a class applying
another declaration's mixin, or a class with an enclosing object or a
declaring class, such as a nested class", "… does not inherit what the root
rule allows"), and refuses a root
that is shipped. The loader holds the record the module section names to the
same check and refuses what the encoder cannot write: a record that is not
the module's own root class, as `ModuleRootNotItsRecord`, and a superclass
chain the rule does not build -- a superclass other than `Kernel Object` or
`Kernel Value`, a level applying anything but `Kernel Value`, a level over
anything but `Kernel Object`, a level its root's clause did not add -- as
`RootRule`, each naming its module
(`a_root_record_over_kernel_array_breaks_the_root_rule`,
`the_root_rule_holds_a_root_classs_superclass_chain`). So a crafted artifact
cannot bring back a root the rule refuses, and no decoded mixin carries a
root defect.

**A decoded value class is re-decided; a decoded instance is not re-checked.**
§3.1.1's two class-level conditions on a value class — no `==`, every
enclosing object a value — are wire fields of neither the application nor
the class object, so neither is trusted from the artifact's bytes. No `==`
needs no recompute of its own: it is a property of the application, and
`rebuild_applications` builds every decoded class's real application through
the same `Mutator::apply_mixin` a class built from source goes through, root
included, so the condition is already folded into `instantiation_refusal`
the moment the application exists. Every enclosing object a value *is* a
verdict on the class object, so `rebuild_applications` recomputes it with
`check_value_class` for every decoded class whose application inherits
`Value`, once every decoded class has its real application
([object-model.md](object-model.md), "A value object's four conditions"): a
snapshot of a refused value class decodes refused either way. A ready-made
*instance*, by contrast, is rebuilt from its record without its slots
passing through `InitializeSlot`, so a crafted image can hand back a value
whose slot holds an `Array` — a wrong answer, never a crash
([core-library.md](core-library.md), "What is still unchecked").

The mixin tree's nesting depth is input-controlled, so building it is iterative —
the "bound any recursion whose depth the input controls" case. Symbol ids, mixin
ids, class-object references, slot counts and arities are all validated before
first use.

**How deeply blocks nest is capped while the code table is read.** The table is
flat on the wire, but a decoded body owns, through `Rc`s, the block bodies its
`MakeClosure`s create, and the depth of that chain is the artifact's to choose.
`decode_code_table` measures each body's block nesting from the bodies it
creates, which precede it, and refuses one past 128 levels
(`DecodeError::BlockNestingTooDeep`, `MAX_BLOCK_NESTING` in `decode.rs`) — the
parser's own ceiling, which every program's code table stays within, so the
cap refuses nothing a lowering produces. Two things rest on it. The depth
limit's exemption lets a chain of exempt activations stack as high as blocks
nest ([compiler.md](compiler.md), "How the limit counts"), so without the cap
the limit would bound no memory for an artifact. And dropping a chain of
bodies recurses once per level: refused only after decoding, a chain a few
megabytes long overflows the stack when the refused table is dropped, which is
why the check runs as each body is read rather than in `verify_code_table`.
`a_deeply_nested_block_chain_is_a_decode_error_not_a_stack_overflow` and
`the_deepest_block_nesting_the_parser_accepts_builds_loads_and_runs` in
`crates/ns-runtime/tests/image.rs` pin both sides.

**Every code family is numbered densely, by construction.** After the
per-body checks, `number_call_site_families` walks each family — a body no
`MakeClosure` creates (a root), with every block under it — and assigns its
sends, other than `CallSite::UNCACHED`, one shared counter's values in the
compiler's own emission order: a root's own sends in instruction order,
recursing into a nested block at its `MakeClosure`. Nothing on the wire says
what a send's number is, so there is no claimed count to refuse -- a family
cannot be sparse or share a number the way a serialized one could, and the
root's `family_size` is recomputed from the freshly assigned numbers
(`bytecode::family_size_of`) rather than read. What this guarantees is what
the density check used to: two sends of one family never share a cache entry
and run each other's targets, and the range a family's first activation
allocates in the heap's cache table is bounded by that family's own decoded
sends. What it does not: that a send's code agrees with the mixin it is
attached to, which no cache relies on ([dispatch.md](dispatch.md), "The
private rule, decided once per site"); a site whose recorded decision no
longer fits misses, and a mismatch gives wrong but safe behaviour.
`a_decoded_family_is_numbered_as_the_compiler_numbered_it` and
`a_retired_send_tag_is_refused` (looped over every entry of
`RETIRED_OPCODES`) in `crates/ns-runtime/tests/image.rs` pin the numbering
and the retired tags.

**A symbol is one record per text, of the builtin `Symbol` class.** Symbols
are canonical per heap ([object-model.md](object-model.md), "Values and
objects"), so no encoder writes two symbol records of the same text; a graph
section that holds two is corrupt or crafted, and `decode_graph_section`'s
first pass refuses it with `DecodeError::DuplicateSymbol`, before anything is
allocated, rather than merging them,
which would mean rewriting every reference to the second
(`a_symbol_recorded_twice_is_refused` in `crates/ns-runtime/tests/image.rs`).
Every symbol record it accepts enters the loading heap's canonical table
through `Heap::record_canonical_symbol`, so a literal evaluated after the
load answers the very object the graph decoded
(`a_decoded_symbol_round_trips_to_the_loaded_worlds_canonical_object`) --
which is also why the record's class tag is checked before that entry is
made: a record whose class names anything other than the builtin `Symbol`
class is refused with `DecodeError::SymbolOfWrongClass`
(`a_symbol_recorded_with_a_class_other_than_symbol_is_refused`), since
entering it under the wrong class would make every later literal of that
text answer the mis-classed object too. The converse is refused as well: a
string record (`OBJECT_STRING`) whose class is the builtin `Symbol` is
`DecodeError::StringOfSymbolClass`
(`a_string_recorded_with_the_class_symbol_is_refused`). A symbol's words are
a string's and only its class tells them apart
([object-model.md](object-model.md), "Strings have two widths"), so such a
record would be a second `Symbol` of its text that no canonical table
holds, against §5.1.6.

**The VM mirror is refused on both sides of the format.** The mirror is a
capability: whoever holds it can run any leaf, and the bootstrap hands it to
the shipped core-library modules alone, `Kernel.ns` and `PlatformFactory.ns`,
which hands it on only to the factories of the shipped modules it builds
([core-library.md](core-library.md), "The VM mirror is the whole Rust
boundary"). **Its class has no declaration in any shipped module, and so no
shipped name**: no byte sequence can name it, as a value, a superclass, a
record's class or anywhere else, so an artifact cannot instantiate a second
mirror or reach the one there is and claim every primitive, which no source
module can. The encoder refuses the other direction: an image whose graph
reaches the mirror object is not written at all, since it would hand the
capability to whatever loaded it (`a_snapshot_that_reaches_the_mirror_is_refused`,
`interpreter/mirror.rs`) -- that refusal is checked ahead of every other --
and neither is one whose graph reaches the mirror's class or its metaclass
("image encoding refuses the VM mirror's class: it has no declaration, so
no shipped name names it"). Neither can happen to a program built from
source, which cannot obtain the mirror; both are there for the day
something could.

**Every other shipped class is held to what may be named where** ("Shipped
names" below), by the encoder and the decoder alike: the decoder refuses a
name in a position the rule does not allow as `ShippedNameNotAllowed`, and
the encoder refuses to write one ("image encoding refuses *name* as
*position*: no program may name it there, and the loader refuses an image
that does"; `a_shipped_name_in_a_forbidden_position_is_refused`,
`a_graph_reaching_an_unnameable_class_is_refused`). What the rule keeps out,
and why:

- **`Top`** is no value, no superclass and no record's class. §3.2 lets only
  `Object` have `Top` as its superclass, and `Kernel.ns` enforces that by
  making `Top` a private class with no accessor, so no source module can name
  it; an artifact naming it would otherwise build a class outside §3.2's
  hierarchy that inherits nothing at all, not even `doesNotUnderstand:`, or
  an object of it (`a_record_of_top_is_refused`). It may stand as a frame's
  class, as every shipped class may.
- **`Behavior`, `Class` and `Metaclass`** are no superclass and no mixin a
  level applies: a class object is made only by evaluating a declaration, so
  a class over one of them could never be instantiated. As a value they are
  accepted: they carry no authority, a program still reaches a metaclass
  as a class side's `self class`, a host can hand a program any of them,
  and a narrower rule would refuse to build such a graph; either may stand
  in a slot. An object record that is no class object but whose
  class refuses instantiation as a class object — an instance of `Class`,
  `Metaclass` or a metaclass — is `InstanceOfAClassClass`, and a class record
  whose row is some row's class side is `ClassRecordAppliesAClassSide`: no
  source makes either.
- **A module's root and its instance.** A shipped module's root class, its
  metaclass and the levels its clause adds are a frame's class and nothing
  else, and a module instance is a frame record's receiver or register and
  nothing else (`VALUE_SHIPPED_MODULE`): a module instance holds the core
  library's state, the kernel's the mirror, and no program reaches one
  (`a_module_instance_outside_a_frame_is_refused`,
  `an_objects_class_naming_a_module_root_or_instance_is_refused`).

**A frame record is the one place an image names `Activation` as a record's
class.** The kernel's activation object is the frame object itself, an
instance of the kernel-private `Activation` class, written as an
`OBJECT_FRAME` record ("Frames are object records" above), its header
`CLASS_SHIPPED` naming `Kernel Activation`. Header and body are held to each
other both ways: a frame record under any other class is
`FrameRecordUnderAnotherClass`, and `Kernel Activation` as the header of any
other record is `ShippedNameNotAllowed` ("a record's class",
`an_activation_header_on_another_record_is_refused`); as a value, a
superclass or a level's mixin it is refused as `Top` is
(`activation_named_anywhere_else_is_refused`). An activation reads other
frames and unwinds stacks, a capability Newspeak hands out only through
mirrors, so no program may name its class. A record under the retired
activation body tag 9 is an unknown tag
(`a_record_under_the_retired_activation_tag_is_refused`).

**`NonBooleanReceiver` is private, and an image carries its instances,
never its class.** No source names it and no public class accessor answers
it, and the VM signals its instances into a program, which catches one; but
`class` is protected, so the program never holds the class
([core-library.md](core-library.md), "`class` and the class accessors are
protected"). A caught instance is an ordinary instance record whose class
the image names as its header
(`a_caught_non_boolean_receiver_round_trips_through_a_package`); the class
object as a value is refused as any private class's is, by the encoder and
by the decoder as `ShippedNameNotAllowed`
(`non_boolean_receiver_as_a_value_is_refused`).

**Futures in a graph: a resolved future is its value, an unresolved one a
record of `Kernel FutureImplementation`.** `FutureImplementation` is the
kernel-private pull future of §6.3.2's simultaneous slots and of `Future
computing:` ([object-model.md](object-model.md), "Simultaneous slots"). No
program names it, so the rule allows it as a value, a superclass or a
level's mixin nowhere, and the encoder refuses a graph that reaches the class
or its metaclass, exactly as for `Activation`
(`a_graph_naming_the_future_implementation_as_a_class_is_refused`). A future
*object* is written two ways. A resolved one is
never a record: the graph walk and the value encoder both see through it
(`future::seen_through`), so every reference to it is written as a reference
to its value, and a load gives back one object where the program saw one —
the value a collection would have put in its place anyway
([memory.md](memory.md)). An unresolved one is an ordinary instance record
(`OBJECT_INSTANCE`) whose header names `Kernel FutureImplementation`, the
one other header besides an activation record's that names a
kernel-private class. Its two fields are its computation -- a
block, or any other value, `Future computing:`'s argument as it was handed
over -- and the state 0. A future is *resolving* only while its
computation runs, and the image writer refuses a world with a running
activation, so no image meets one; the encoder handles the case
defensively all the same, writing it as unresolved, its computation still
in the first field (`future::fields_for_an_image`), which is what an
abandonment would make of it — a load starts no computation, so nothing
could finish resolving it.

The decoder holds a record under that header to what the encoder writes: an
instance body of exactly two fields whose second is the state 0; the
first may be any value. Anything else under that header — another body,
another field count, the state 1 or 2 or one that is none of the three —
is `FutureRecord`,
refused rather than decoded, so no image hands the runtime a future whose
fields are no future's, and no resolved future stands apart from the value
every other reference was written as. Crafted code can still store into a
future's slots once it holds one -- a decoded frame may carry any receiver,
and `InitializeSlot` stores into `self` unchecked -- so
`Mutator::future_state` reads the fields as untrusted: a state that is none
of the three, or a resolved future whose value is a future, is
`FutureState::Malformed`, which forcing raises as
an ordinary error, the collector leaves in place, and the encoder refuses to
write (`forcing_a_future_with_corrupted_fields_is_an_error_not_a_panic`,
`tests/vm.rs`; `a_future_with_corrupted_fields_is_refused_by_the_encoder`,
`tests/image.rs`). A graph that decodes a future sets the
loading world's `futures_made`, as making one does, so the leaves
see through it and every check that stays off a program's paths until a
future exists runs in the loaded world
(`a_resolved_future_is_written_as_its_value`,
`an_unresolved_future_round_trips_and_computes_after_load`,
`a_future_record_that_is_not_an_unresolved_future_is_refused`,
`a_graph_naming_the_future_implementation_as_a_class_is_refused`,
`an_argument_error_a_proxy_and_a_programs_future_round_trip`,
`tests/image.rs`;
`goldens/language/slot/simultaneous/spec_6_3_2_a_data_cycle_survives_an_image.ns`, whose build/load
round trip carries a data cycle through resolved futures).

**The other public kernel classes are ordinary nameable classes.**
`Exception`, `Error`, `MessageNotUnderstood`, `Message`, `Halt`,
`ArgumentError`, `CannotReturn`, `CannotInstantiate`, `Proxy`, the public
`Future`, `Collection`, `Value` and `Number` may stand wherever a class
may; a program reaches them through `platform kernel` or `Object`'s
accessors and may subclass them, and an instance of a program's subclass is
an ordinary instance record. **`LargeInteger`** is public in `Kernel.ns`
although `Object` declares no accessor for it, so it may stand wherever a
class may; with `class` protected no program reaches it on its own, but a host
can hand it to one, as
`crates/ns-runtime/tests/representation.rs`'s
`a_subclass_of_large_integer_is_no_number` does, which keeps it in a slot
and subclasses it. A `LargeInteger` itself is never a record ("A `LargeInteger`
is a value on the wire" above); an instance of the class that holds no
digits, which only `LargeInteger new` makes, is an ordinary instance record.
**`Fraction`** is reached through `Object`'s accessor; a fraction itself is
never a record ("A `Fraction` is a value on the wire too" above), and the
one record its header may carry is `Fraction new`'s, an instance of no
field: any other is `DecodeError::FractionRecord`, since a record of fields
would be a fraction whose normal form nothing checked
(`a_fraction_record_with_fields_is_refused`).

**A builtin is a fact of Rust's, not of the wire.** No part of an image
names a class by its position in `BuiltinClass::ALL`: the builtin class tags
that once did -- a numbering of their own, 0 to 39 -- are retired as a
whole, with class tag 0 (`CLASS_BUILTIN`), mixin reference 1
(`MIXIN_BUILTIN`) and hash tag 0 (`HASHED_BUILTIN_CLASS`), which carried
them (`opcodes::RETIRED_CLASS_TAGS`). Since a builtin is named by its
declaration, removing or adding one retires nothing on the wire; the
header's fingerprint, which covers the names in `BuiltinClass::ALL`, keeps
a stale image from loading against the new binary
(`an_image_of_another_shipped_world_is_refused`).

**A record of a shipped class carries exactly that class's layout.** The
code and the layout of a shipped class that is no builtin -- a `List`, a
`Stopwatch`, the platform -- come from the loading binary, not from the
image, so a record whose header names one must be an instance record
(`ShippedInstanceBody` otherwise, refused by the first pass before anything
is allocated, `a_record_of_a_shipped_class_with_another_body_is_refused`)
carrying exactly the class's slot count of fields
(`ShippedInstanceFieldCount`, `a_record_of_a_shipped_class_carries_exactly_its_layout`).
A builtin's record is held to its own format instead, as above; for a
`ByteArray` both ways, since its format is the one no other class may
have: a byte-array body under any header but `Kernel ByteArray`, and a
`Kernel ByteArray` header over any other body, are
`ByteArrayRecordMismatch`, refused by the first pass and the second alike;
an `Ephemeron`'s both ways too (`EphemeronRecordMismatch`), and a weak
array's body under any header but `Kernel WeakArray`
(`WeakArrayRecordUnderAnotherClass`), whose own header carries an
instance record as well, the slotless one of its factory `withoutSlots`.
A view's or a `StringBuilder`'s record is an ordinary instance record of
its shipped class; what its slots hold is the program's to check, and the
kernel refuses a crafted one with an `ArgumentError`
([core-library.md](core-library.md), "The bootstrap and the image take
them").

**Trailing bytes are refused, not ignored.** `load` checks that the byte stream
is exhausted once the last section has been read and returns
`DecodeError::TrailingBytes` if not. An unchecked tail would let an image
with bytes appended — or a truncated, then padded, one — load as something
its author did not write; see `bytes_after_the_last_section_are_refused_not_ignored`
in `crates/ns-runtime/tests/image.rs`.

## Reading an artifact

`disasm` accepts an artifact as well as a `.ns` file: the same decoder that
`load` uses rebuilds the mixin tree with each body attached to its declaration,
and `disasm` formats a decoded body's `Instruction`s exactly as it does a freshly
compiled one's. Like every load, it refuses an artifact `verify_mixins` finds
inconsistent before printing anything. It addresses module 0 alone, the
application module (a project's `main`): `--class` is a path rooted at that
module's class name, so the bodies of a project's other modules, the manifest's included,
are not reachable through `disasm` of the artifact (`inspect --full` shows
them).

`ns-next-analyzer inspect <artifact>` prints the file itself. Its default is a table of
contents — header flags and the shipped world's fingerprint, section
offsets, the symbol table, **the code table with every body named**, the
mixin table, the module section, each module's origin, whether it carries
source and its root's object id, and the count of shipped names — and
`--full` prints everything, the shipped names among it, each as
`ShippedName::describe` spells it (`Kernel Array`, `Kernel Integer class`,
`CollectionsFactory instance`):
the mixin rows expanded to slot names and per-body code ids, then every body
disassembled. The intent is `javap -v`'s: an artifact should be followable end
to end from the text, without opening the file again. The default stays brief
because a real corpus module runs to 281 bodies.

The code table's listing is what joins the two views. A section line saying
"4 bodies" tells a reader that four exist but not what to ask `disasm` for; the
mixin table already maps selector to code id, so attributing them costs no new
decoding. A body no declaration claims is labelled by its kind — a block body
is reachable only from the `MakeClosure` that creates it — and anything left
over prints as `(unnamed)`, which means the code table and the mixin table
disagree and is worth seeing rather than hiding.

**An image's object graph is read, not rebuilt.** `decode_graph_section`
materializes objects and resolves shipped names against a live shipped world, so it needs
an `Interpreter`; a viewer must not bootstrap one, any more than `javap`
instantiates a class to print it. `inspect` therefore walks the same bytes and
reports each record: an object's class reference, identity hash and slot values, a
frame record's kind, `program_counter`, `uncontinuable`, receiver, sender,
home, current class and registers (`class Activation, hash H, frame <kind>,
pc P, …, registers [..]`), and the roots, and a started image's one root is
the application. Nothing else shows
the graph. A module count that disagrees with the mixin table's trees is
refused, as `load` refuses it, rather than listed past: which tree a
module's root is held to depends on the count, so a report past it would
describe modules that are not there.

Being a second reader over one layout is a drift risk, taken deliberately and
held by two things. The tags it interprets are the shared constants in
`opcodes.rs`, so it cannot disagree with the loader about what a byte *means*.
And the walk must consume the section exactly — its recorded end is the file's
last byte — so a disagreement about record *shape* desynchronizes it and fails
loudly instead of printing plausible nonsense. That guard is only as good as
its fixtures: the first version of the test passed with a field deliberately
dropped, because every fixture produced zero frames and no frame record was
never walked. A source with a block in a slot, the only way a frame reaches the
graph, is what makes it bite.

`load --print-source` still only returns the debug variant's `.ns` text,
every module's in module order with nothing marking where one ends; for
anything else, `inspect` is the tool.

This exists because an artifact ships **without** source unless `build` is
asked for the debug variant: the one tool that prints bytecode would
otherwise require the thing the artifact was built to do without. It also
makes checkable against the artifact itself what could otherwise be
verified only by reading the encoder -- that a started image holds no
platform, no module instance and names nothing of the shipped world but
the kernel.

**One-way only. There is no text format that can be loaded back.** That would
be a second encoder and decoder pair to keep in step with the binary one — the
drift shape this project has already paid for in two copies of the tag table.
The argument that would justify it is hand-authoring test images, and that need
is already met differently: the untrusted-input tests take real images and
corrupt them at located offsets, which exercises real structures rather than the
ones someone thought to write out.

**This is not the decompiler**, and the two are easy to confuse:

| | Produces | Lets a reader understand |
|---|---|---|
| Dump | structure as text | the *file* |
| Decompiler | `.ns` source | the *program* |

Part of why "export back to `.ns`" looks large is that the easy half gets counted
into it. It is not in it.

## Packaging

`ns-next build <file.ns | project> <out> [--debug-source] [--lowering
…]` assembles the application exactly as `run` does and writes it where `run`
would send `main:args:`. `ns-next load <image> [-- args]` starts it. The CLI
side is `crates/ns-cli/src/program.rs` (`read_program`, `load_program`,
`instantiate_application`, `module_sources`), shared by `run`, `build` and
`bench`, and `crates/ns-cli/src/project.rs`, the project file.

**Two ways to assemble, one entry.**

- **A single source file**: `ns-next run app.ns [-- args]`. The root class's
  primary factory must be `new`, written out or left implicit (§6: "If the
  signature is omitted it defaults to `new`"); any other factory is refused
  before anything runs, naming the factory found
  (`a_source_file_whose_root_takes_the_platform_is_refused_before_anything_runs`,
  `crates/ns-cli/tests/run.rs`). The root is sent `new`, then `main:
  platform args:`.
- **A project**: `ns-next run [app] [-- args]`, where `app` is a directory
  holding a `newspeak.json`, or that file itself; with no program, the
  current directory. A project file is recognized by its name alone, as a
  Node package's `package.json` is, so a command given a directory finds it
  and no other JSON file is mistaken for one: a directory without one, and a
  `.json` file of any other name, are refused naming `newspeak.json`
  (`project::locate_program`; decided with the user on 2026-10-05). `bench`
  takes the same, `build` the same but always with its program written out,
  since its output path follows. The root class's primary factory must be
  `packageUsing:`. Every module is parsed, resolved and compiled first; the
  root is sent `packageUsing: manifest`, then `main: platform args:`.

Either way the platform arrives only as `main:args:`'s first argument, and
the application's factory and initializers run with no platform in
existence.

**The project file** is JSON (the user's choice; it brings `serde_json` into
`ns-cli`, which `ns-analyzer` already used):

```json
{
  "$schema": "../../schemas/newspeak.schema.json",
  "main": "Harness.ns",
  "modules": ["Sieve.ns", "../shared/RedBlackTree.ns"],
  "scripts": {
    "sieve": "Sieve 10",
    "timed": "--timed 'Delta Blue'"
  }
}
```

- `main` is the application module's source file, whose root class is sent
  `packageUsing:`; `modules` the other modules the manifest holds.
  `modules` may be absent or empty. `main`'s module is not in the manifest.
- `scripts` is optional: named command lines for `--script` ("Scripts"
  below).
- Every other key is ignored, so a file may carry what an editor or another
  tool wants beside what ns-next reads. `$schema` is one of them: it tells
  an editor without the plugin where the schema is, and the reader never
  follows it. The reader does check that it is a string when present, as the
  schema describes it, so that no file the schema refuses is run.
- A module's name in the manifest is the name its source file declares for
  its class, as in both upstream systems (Primordial Soup's
  `namespace at: klass name put: klass`; Squeak's `Root` namespace).
- The project's own path, given to a command, is absolute or relative
  to the working directory. A path inside it is absolute or relative to the
  project file. Either separator, `/` or `\`, is accepted in every path,
  relative or absolute, on every operating system: the foreign one is
  rewritten to the host's and the platform's own path parsing does the rest,
  which keeps drive letters, UNC and verbatim prefixes intact
  (`resolve_written_path`).
- A module may declare the name of `main`'s root class, or be named
  `Manifest`: `main`'s module is not in the manifest, and an image names a
  module by its index, never by its name, so neither collides with
  anything.

**The rules** (`project::read_project`). Each is refused before anything
runs, with `error: <project file>: ` and a message of its own:

| Refused | Message |
|---|---|
| text that is no JSON | serde's syntax error, with its line and column |
| a file that is no JSON object (an array, a string) | `` a project file is a JSON object, of `main` and `modules` `` |
| no `main` (a file that still says `root` included, which is not singled out) | serde's `` missing field `main` `` |
| `$schema`, `main`, `modules` or `scripts` of the wrong type (a `$schema` of `null` included) | serde's `invalid type: …, expected …` |
| one of those four keys written twice | serde's `` duplicate field `main` `` |
| a script name written twice | `` script `timed` is written twice `` |
| an empty script name | `a script name is empty` |
| a script name beginning or ending with whitespace | `` script name ` timed` has leading or trailing whitespace `` |
| a script that is no string | serde's `` invalid type: sequence, expected a script of `scripts`, one command-line string `` |
| a script with an unterminated `'` or `"` | `` script `timed` holds an unterminated `'` `` |
| a script ending in a backslash outside quotes | `` script `timed` holds a backslash at its end, escaping nothing `` |
| an empty path in `main` or `modules` | `` `main` holds an empty path `` |
| a path not ending in `.ns`, matched case-sensitively | `` `modules` names `B`, which is no `.ns` file `` |
| a module path equal to `main`'s | `` `modules` names `main`'s module `./App.ns` again `` |
| a module path written twice | `` `modules` names `lib\A.ns` twice `` |

Two paths are the same when their components are: either separator, a
doubled one and a `./` all name one path. The file system is not asked, so
a `..` that comes back, a symbolic link and a difference of letter case on a
case-insensitive file system are not seen through; two such modules declare
one name, and are refused by the next check. A key the reader ignores is
not checked for duplicates.

After the file, `program::read_program` refuses, also before anything runs:
a missing or unreadable module file, a module that does not parse or
resolve, two modules declaring the same name, and a root whose factory is
not `packageUsing:`. A module whose name is a public member of every
object -- `yourself`, `printString` -- is refused too, once the manifest
exists and before the root is sent `packageUsing:`: the member would answer
the send before the manifest's `doesNotUnderstand:` could
(`Interpreter::responds_publicly`,
`a_module_shadowed_by_an_object_member_is_refused`). `Object`'s class
accessors are protected, so a module named `Array` or `String` is reached
through the manifest as any other
(`a_module_named_like_a_class_accessor_is_reached_through_the_manifest`).

**The schema** is `schemas/newspeak.schema.json` (JSON Schema 2020-12), the
one copy. It describes `$schema`, `main`, `modules` and `scripts` and states
these rules: the file is an object (`type`) with a `main` (`required`);
`$schema` is a string; `main` and every entry of `modules` a string of at
least one character (`minLength`) ending in `.ns` (`pattern: "\\.ns$"`);
`modules` an array whose entries are not written twice as the same string
(`uniqueItems`); `scripts` an object whose every name is non-empty and
neither begins nor ends with whitespace (`propertyNames`, `pattern:
"^\\S([\\s\\S]*\\S)?$"`, whitespace being what a pattern's `\s` matches) and
whose every value (`additionalProperties`) is a string. Every
other key is allowed. An editor's validator may read the pattern's `\s` as
ASCII whitespace only, as Java's regular expressions do, and so let a name
with other whitespace at an end pass; the reader decides. No validator crate
reads the schema, so the project file brings no dependency beyond
`serde_json`: the reader states each rule itself, with a message of its own,
and `crates/ns-cli/tests/project_schema.rs` holds the two together. That test
interprets the schema with exactly the keywords it uses -- a keyword it does
not know fails the test, so the schema cannot gain a rule nothing checks the
reader against -- and runs `ns-next run` on a table of files, each of which
the schema and the reader must both accept or both refuse. The rules the
reader alone holds cannot be stated in JSON Schema: one of its four keys
written twice and a script name written twice (a schema sees only the last
value of a key), one module path in two spellings (`uniqueItems` compares
strings as written), a `modules` entry naming `main`'s module (no keyword
compares one property with another), and a script that does not split as a
command line, such as `"'unterminated"` (the schema states only that a
script is a string). They stay out of the table, and `project.rs`'s own
tests pin them. The JetBrains plugin bundles the same file and gives it to
every `newspeak.json` ([editor-support.md](editor-support.md), "The project
file and running projects").

**Scripts**, as Node's are. `scripts` maps a name to a string, a command
line. `ns-next run --script <name>` and `ns-next bench --script <name>` hand
its arguments to `main:args:`, exactly as if they were written after `--`;
arguments after `--` are still taken and follow the script's, so with the
example above `ns-next run --script timed -- Sieve` hands `--timed`, `Delta
Blue` and then `Sieve`, as `npm run timed -- Sieve` does
(`program::entry_arguments`;
`a_script_hands_its_arguments_to_main_and_the_command_line_follows`,
`crates/ns-cli/tests/run.rs`). A script's name is a non-empty name that
neither begins nor ends with whitespace.

A script is split into arguments as a POSIX shell splits a command line, on
every platform (`command_line::split_command_line`):

- unquoted whitespace -- a space, a tab or a newline -- separates
  arguments; whitespace at either end or repeated makes no empty argument,
  and any other character (a carriage return, a no-break space) belongs to
  an argument;
- `'…'` holds everything literally up to the next `'`;
- `"…"` holds everything literally except a backslash before `"`, `\`, `$`,
  a backtick or a newline, which escapes it; a backslash before a newline
  removes both, and any other backslash stays;
- outside quotes a backslash escapes the next character (`second\ word` is
  one argument), and a backslash before a newline removes both;
- adjacent parts make one argument, so `a"b c"'d'` is `ab cd`, and `""` or
  `''` alone is one empty argument;
- nothing is expanded: `$HOME`, `*`, `~` and backticks are literal;
- nothing is an operator or a comment: `|`, `<`, `>`, `;`, `&`, `(` and `)`
  are plain characters, and so is `#`, even at the start of a word, where a
  shell would begin a comment (`a #b (c) & d` is `a`, `#b`, `(c)`, `&` and
  `d`), because no shell runs a script.

So `"--name=\"Delta Blue\" '' $HOME"`, as written in the JSON, is the
arguments `--name=Delta Blue`, an empty one, and `$HOME`. The script is
split when the project file is read, so an unterminated `'` or `"`, or a
backslash at the very end, refuses the file before anything runs, whichever
script is asked for. Arguments stay strings everywhere, from a script as
from the command line or an image, so a program sees one form however it is
started.

Refused before anything runs: `--script` with a source file rather than a
project ("--script names a script of a project's newspeak.json, and the
program given is a source file"), and a name the project's `scripts` does
not hold ("`<project file>` has no script `zz`; its scripts are sieve,
timed", or "it has none";
`an_unknown_script_is_refused_naming_the_scripts_there_are`,
`a_script_with_a_source_file_is_refused`). Written as one token,
`--script=<name>`, a name may start with `-`; the JetBrains plugin always
passes it so, and `--script=` is refused as a name no project holds
(`a_script_named_in_one_token_may_start_with_a_dash_and_not_be_empty`).

`build` takes no `--script`. It does not send `main:args:`, so it has no
arguments to take, and `load` takes its arguments after `--` like any
start; a script recorded into an image would need a section of the image
format of its own ([OPEN-WORK.md](../OPEN-WORK.md), "Planned: images,
liveness and reflection").

**Module order** is fixed and shared by everything that loads or writes
modules: the application module first (a source file, or a project's
`main`), then the project file's `modules` in the order written, then the
embedded `Manifest.ns` (only for a project).
`load_program` compiles them into one interpreter in that order, `build`
writes them in it, and `load` rebuilds their classes in it, which is what
keeps every identity hash the bootstrap draws the same across `run`, `build`
and `load`.

**The manifest** is an instance of a small Newspeak class,
`crates/ns-runtime/core-library/Manifest.ns`, embedded in the binary as
`ns_runtime::MANIFEST_SOURCE` ([core-library.md](core-library.md), "The
manifest"). It is neither privileged nor shipped: it has no VM mirror, is
resolved with `resolve` like a user module and compiled into the program
beside the modules (`SourceOrigin::Embedded("Manifest.ns")`), so an
application that keeps its manifest in a slot serializes it like any other
object (`goldens/tool/manifest/an_application_may_keep_its_manifest`). The host
instantiates it with `names: definitions:`, an `Array` of names (`Symbol`s)
and an `Array` of module definitions (§6.4: the class objects, not
instances). Its `doesNotUnderstand:` answers the module definition whose
name is the message's selector, searching linearly, and passes anything else
to `super`, so `manifest Missing` is an ordinary `doesNotUnderstand:` at run
time (`goldens/tool/manifest/a_name_the_manifest_does_not_hold_is_not_understood`)
-- the lookup both upstream `Manifest` classes make over a map
(`Namespacing.ns`, `CompilerApp.ns`).

**What `build` writes.** `image::package` takes the modules in module order
and the application, and writes an image whose graph has the application as
its one root and whose header sets `FLAG_START`. No platform exists for the
application yet, and before `main:args:` a program reaches nothing of the
shipped world but the kernel, so a graph naming anything of
`PlatformFactory` or `CollectionsFactory` is refused ("image encoding
refuses PlatformFactory Platform in a started application: nothing before
main:args: reaches it"; `package_refuses_an_application_that_reaches_the_platform`,
`crates/ns-runtime/tests/image.rs`). So is a graph naming a module's
instance, the kernel's included, which no program reaches. The loader
refuses such a started image too ("Shipped names" below). An application
that reached the platform could only have been handed it, and nothing hands
one out before `main:args:`. `--debug-source` stores every module's source text. An error
raised while assembling -- a refused factory, a resolve error, an
initializer that raises -- fails the build with the status and report `run`
gives the same error, so a golden whose assembly fails round-trips as that
failure.

**Loading.** `load` reads the image with `image::load`, which decodes and
verifies it as "Verification" describes, and refuses one that is not started
-- an image without `FLAG_START`, which only a host's `image::snapshot`
writes (`load_refuses_an_image_that_is_not_started`,
`crates/ns-cli/tests/build_load.rs`). It sends the application `main:
platform args:` with the loading interpreter's platform. The factory and the
initializers do not run again: they ran at build time, and their effects are
in the graph (`a_loaded_application_does_not_rerun_its_initializers`). The
send stays in the CLI rather than in `image::load`: deciding that a
command-line start supplies the platform is a CLI concern, because nothing in
the runtime needs to know it was invoked from a terminal.

**`args:` is the program's arguments.** §8.4 gives `main:args:` "a platform
object (8.3) as its first parameter, and a list of any environment-specific
(e.g., command line) arguments as its second". `crates/ns-cli/src/entry.rs`'s
`send_main` -- the one entry send every started program receives, from
`run`, `load` and `bench` alike -- supplies the platform and then the program's
arguments, each a `String`, in an `Array` (`main_arguments`; `ns-next run
app.ns -- a b`, `ns-next load app.img -- a b`), empty when there are none:
for `run` and `bench` a script's arguments followed by the words after `--`
("Scripts" above), for `load` the words after `--`. They are the CLI's, not
the format's: nothing in an image records them, so one artifact runs with
whatever arguments each `load` is given.

**`bench`** assembles a fresh application for every sample, as `run` does
and untimed, and times `main:args:` alone. **`run-corpus`** starts each
corpus module through a generated application: the six corpus modules on
`corpus/runnable.toml` are upstream's and declare no `main:args:`, so for
each row it writes a wrapper application and a project file into a scratch
directory. The wrapper's `main:args:` instantiates the corpus module from its
manifest -- `new`, or `usingPlatform: platform` with the platform
`main:args:` received -- and sends it the row's `send`, a unary selector.
All four lanes, `run` of the project file and `build` then `load` of it,
under both lowerings, go through that one wrapper
(`crates/ns-cli/src/commands/run_corpus.rs`).

**Tests**, in order of importance: `build` then `load` reproduces
`run`'s stdout, stderr and exit status byte for byte, for every golden, the
project goldens included (`crates/ns-cli/tests/golden_round_trips.rs`, one
test per golden and lowering, `build_load::<lowering>::<path>`);
two modules draw the same identity hashes under `run` and `load`
(`crates/ns-cli/tests/build_load.rs`); the CLI refusals above
(`crates/ns-cli/tests/run.rs`, and `project.rs`'s own tests for the rules
and the paths); the schema and the reader in agreement
(`crates/ns-cli/tests/project_schema.rs`);
and mutation and truncation tests for the module section, the shipped-name
section, `FLAG_START`, the fingerprint and the retired tags and flag
(`crates/ns-runtime/tests/image.rs`).

## The image under generations

The decoder builds its objects in the object space ([memory.md](memory.md),
"The object space"): each placeholder is a run of words the first pass
sized, bumped off the old space's top and filled in place. The wire format
is id-based, and neither road step 3b nor road step 4 changed anything on
it: nothing on the wire becomes an address, an offset or a generation, so
an artifact stays independent of word size, layout and nursery size, as
Primordial Soup's snapshots are
(`prior-art/primordialsoup/vm/snapshot.cc:398-470`); Spur's image, which is
old space's bytes relocated on load, is the model this does not take.

- **Every load tenures once, between the shipped-name section and the graph
  pass** (decided with the user on 2026-10-04). A load runs in two phases
  (`image::load`), after it bootstraps an interpreter
  (`Interpreter::bootstrap`), which builds `Kernel` and `PlatformFactory`.
  The first decodes everything up to and including the shipped-name section;
  it reads bytes only, builds no object and keeps the names, not handles
  (`decode_described`). Then, from the host, the loader builds the binary's
  own modules and classes the section names that the boot did not build
  (`Interpreter::build_shipped_names`): it checks every name first -- an
  unknown module or declaration, a level past its clauses, a name given
  twice, and in a started image a name of any module but the kernel or of any
  module instance, is refused before anything is built ("The section" in
  "Shipped names" below) -- then builds each named lazy module as a program's
  first demand does and each named class of it through its accessor, and
  resolves every name against the world by table lookup. Then, outside any
  entry, where only roots hold values, one scavenge that tenures everything
  (`Interpreter::tenure_now`) makes the shipped world old, the builds'
  objects with it. The second phase, in an entry of its own, resolves the
  names again -- a lookup, no allocation -- and decodes the graph against
  them, the module roots' records among it. Spur, Primordial Soup, Strongtalk
  and HotSpot's CDS all load into a heap that holds nothing young; this makes
  the heap so before the graph is decoded. One tenure suffices: the builds
  allocate, but they all run before it, and nothing between the tenure and
  the graph pass allocates, so the graph pass is the only allocator after it.
- **The graph pass allocates old.** It sets the heap allocating old
  (`Heap::set_allocating_old`) for its whole length, the refusal path
  included, so every placeholder -- the boxed floats and large integers it
  creates among them -- goes to the old space, as Primordial Soup's
  `AllocateSnapshot` allocates a snapshot's objects old
  (`prior-art/primordialsoup/vm/heap.cc:228`), since a decoded graph is
  long-lived. Its stores are then old-to-old and take no write barrier, and
  its objects are not born remembered; a debug build asserts at every fill
  that the word it stores names no young object
  ([memory.md](memory.md), "Generations").
- **The graph pass never collects.** It never polls a safepoint, so nothing
  moves while its placeholders exist, and the transient peak of a rejected
  load is unchanged by the generations ([memory.md](memory.md), "The
  amplification bound of a rejected load").

## Shipped names

An image names every object the binary owns -- each shipped module's
instance, each class its root declares, their metaclasses and the levels
their `<:` clauses add -- by where it is declared, resolved against the
loading binary's shipped world, which the boot and the loader build by the
binary's own code (decided with the user on 2026-10-12, built the same week;
[its
plan](../superpowers/plans/2026-10-12-2224-shipped-names-in-the-image.md)).
It replaced four ways of naming: the builtin class tags, `MIXIN_BUILTIN` and
`CLASS_METACLASS`; the platform paths and `VALUE_PLATFORM`; the module class
tags `CLASS_MODULE` and `CLASS_MODULE_LEVEL`; and every tag-named hash but
`HASHED_SHIPPED`. Code: `image/shipped_names.rs` (the section, its reader and
the resolution), `interpreter/shipped_world.rs` (the world, its names and
what may stand where), `image/fingerprint.rs`.

**Why one mechanism.** The VM mirror is never in an image; every shipped
module instance holds it or the kernel that does; every shipped class object
reaches its module instance lexically. The whole shipped world is therefore
the loader's, and an image can only name into it. The image used to name in
four coordinate systems -- a position in `BuiltinClass::ALL`, a path of
sends from the platform replayed at load, an image-local module index and
the platform singleton -- and each broke on the next class that was neither
a builtin nor handed out by the platform: a plain kernel class, a private
one, the level a kernel mixin adds under a builtin. The coordinate system
Newspeak itself provides is the lexical declaration tree of the shipped
modules. Every shipped class is declared directly in its module's root, so
a name is a pair: the module and the declaration. Rebuilding a shipped class
as a record was rejected because Rust compares class objects by identity
(the builtin field, the exception classes, `resolve.rs`): a name has to
resolve to the loading world's own, canonical object, as Java's
serialization re-resolves an enum constant by name. Naming by a path from
the platform was rejected because the loader had to send the selectors the
image chose before it decoded the graph, and because a path reaches only
what the platform hands out. Putting the shipped world into the image, as
Primordial Soup's snapshot does, was rejected because it makes every image
as trusted as the binary: an untrusted file holding the mirror runs every
leaf, so locks (3) to (6) of the mirror would go, and a fix to the kernel
would no longer reach a deployed artifact ([core-library.md](core-library.md),
"The VM mirror is the whole Rust boundary").

**The shipped world is a function of the binary, built on demand.** Only the
binary's own code builds it (`interpreter/core_library.rs`,
`interpreter/shipped_world.rs`). The boot
(`Interpreter::install_shipped_modules`) records the kernel instance after
the kernel's phase, builds `PlatformFactory` (`usingVmMirror:kernel:`), and
`complete_shipped_world` builds every class the two roots declare, in
declaration order, metaclasses and levels included, before the platform is
made. Every other module -- `TranscriptFactory` and `TimeFactory`
(`usingVmMirror:`), `CollectionsFactory` (`usingKernel:`, since it needs no
mirror and so is given none) and `TypedArraysFactory`
(`usingVmMirror:kernel:`) -- is built on its first demand, through the
mirror's `shippedModuleClassNamed:` and `recordShippedModule:`, and each of
its classes is named as its accessor builds it (`nested::after_clause`); the
loader builds the ones an image names the same way before it decodes the
graph (`Interpreter::build_shipped_names`). The image chooses which of the
binary's modules and classes exist, never what runs. The world keeps two
tables, traced as roots (`ShippedWorld`, in the step roots beside the
kernel): the module instances, the kernel first, which the mirror's
`moduleInstanceNamed:` answers, so one collections module serving every
platform is a guarantee of the VM; and every shipped object under its name,
`ShippedName` -- (module, declaration, kind), the kind being the module
instance, the class, its metaclass, the level a clause adds by the clause's
index, or that level's metaclass (`ShippedKind`) -- with the positions it may
stand in (`Positions`), recorded in the order the objects are built, so the
encoder walks them in an order that is a function of the program and its run.
Every name resolves to the object the binary built, so no comparison by
identity in Rust changes. No build draws an identity hash, so every hash a
run draws is the same under `run` and `load`
(`goldens/library/object/identity_hash.ns`;
`building_a_module_on_demand_draws_no_identity_hash`). The world costs a
process start little: an optimized boot decodes the compiled code `cargo
xtask shipped-code` generates and the binary embeds, for the kernel and
`PlatformFactory` only, 1.22 ms for a hello program's boot on the measuring
workstation, where building every module at boot took 1.77 ms in the same
measurement, and a first `platform collections` costs 0.31 ms; compiling the
shipped source took 13.6 ms in an earlier measurement, against 1.6 ms for an
eager boot from the artifact there, and each pair compares only within its
own measurement ([execution.md](execution.md), "What the precompiled shipped
code moved"; [core-library.md](core-library.md), "The embedded shipped
code"). The format is untouched by it: the embedded code is the boot's, never
the image's.

**The section.** A `u32` count, then per entry the module's symbol, the
declaration's symbol (for the root, the module's own name), the kind as a
`u8` and, for a level or a level's metaclass, the clause's `u32` index
("Shipped names" in "Sections" above). Each name the image uses appears once,
in the order of first use. The loader checks the whole section against the
binary's declarations before it builds anything (`shipped_names.rs`'s
`check_names`): a name the binary does not have -- an unknown module or
declaration, a level past the declaration's clauses -- is
`UnknownShippedName`, a name given twice `DuplicateShippedName`, and in a
started image a name of a module other than the kernel, or of a module
instance, `ShippedNameInStartedImage`
(`building_the_shipped_names_refuses_a_name_before_it_builds_anything`). It
then builds what the section names that the boot did not
(`Interpreter::build_shipped_names`) and resolves the whole section before
the graph by table lookup alone (`shipped_names::resolve`), which sends
nothing and refuses the same names again. In the graph, a reference is a tag
and a `u32` index into the section -- five bytes, against the two a builtin
class tag took -- so an image of millions of small records pays for each name
once and resolves each reference by indexing; an index past the section is
`BadShippedName`:

| Tag | Number | Followed by | Retired in its place |
|---|---|---|---|
| `CLASS_SHIPPED` | class tag 8 | an entry of a class, a metaclass or a level | `CLASS_BUILTIN` (0), `CLASS_PLATFORM_PATH` (5), and `CLASS_METACLASS` (4), `CLASS_MODULE` (6), `CLASS_MODULE_LEVEL` (7), whose objects are records now |
| `MIXIN_SHIPPED` | mixin reference 2 | an entry of a class, whose mixin the class record applies | `MIXIN_BUILTIN` (1) |
| `VALUE_SHIPPED_MODULE` | value tag 11 | an entry of a module instance | `VALUE_PLATFORM` (7), `VALUE_PLATFORM_PATH` (8) |
| `HASHED_SHIPPED` | hash tag 7 | an entry of any kind | `HASHED_BUILTIN_CLASS` (0), `HASHED_PLATFORM` (2), `HASHED_PLATFORM_PATH` (4), and `HASHED_METACLASS` (3), `HASHED_MODULE_CLASS` (5), `HASHED_MODULE_LEVEL` (6), whose objects are records now |

A class as a value stays `VALUE_CLASS_TAG` with a class reference, and a
`<:` clause's message frame names its level by an ordinary class reference.
Retired and never reused (`opcodes.rs`): class tags 0, 1, 3, 4, 5, 6 and 7,
mixin reference 1, value tags 7 and 8, hash tags 0 to 6, and the builtin
class tags as a whole; being a builtin is a fact of Rust's and left the
wire. The live naming tags are `CLASS_RECORD` and `CLASS_SHIPPED`,
`MIXIN_ROW` and `MIXIN_SHIPPED`, `VALUE_SHIPPED_MODULE` and
`HASHED_SHIPPED` (`crates/ns-runtime/tests/opcode_tags.rs`).

**What is an ordinary record.** The platform is a record of class
`PlatformFactory Platform` like any instance, its lazy slots written as they
stand; it decodes to a value equal to the loading platform, since lazy slots
are no part of a value's state, and fills an empty one on demand through the
canonical `PlatformFactory` instance
(`the_platform_decodes_to_a_record_equal_to_the_interpreters_own`). Its
facets, a `Stopwatch`, a `List` and a `Map` are records of their shipped
classes (`a_platform_member_round_trips_as_a_record`), and a level a
program's clause adds over a shipped class that is no builtin is a
`MIXIN_SHIPPED` record like one over `Value`
(`a_level_applying_a_shipped_class_that_is_no_builtin_round_trips`). The
decoder holds every record of a shipped class that is no builtin to its
class's layout ("A record of a shipped class carries exactly that class's
layout", "Verification" above). The program's module roots and the levels
their clauses add are class records ("Module section" above).

**The header carries the shipped world's fingerprint**, a `u64` after the
flag byte (`fingerprint::shipped_world_fingerprint`): FNV-1a over the shipped
sources' fingerprint, which `crates/ns-runtime/build.rs` takes over the six
shipped modules' sources -- `Kernel.ns`, `PlatformFactory.ns` and every `.ns`
under `core-library/platform/` (`CORE_LIBRARY_FINGERPRINT`) -- the names in
`BuiltinClass::ALL` and the mirror's selectors, each part preceded by its
length. It covers neither the Rust sources, so an image stays loadable
across a Rust edit that leaves the shipped world as it was, nor
`Manifest.ns`, which is no part of the shipped world: a program compiles it
as a module of its own and an image carries it. The lowering is not in it:
`load` always builds the shipped world with the optimized lowering, and an
application built under the reference lowering carries its own code. A load
whose binary computes another refuses the image before it reads anything else
(`ShippedWorldMismatch`, `an_image_of_another_shipped_world_is_refused`). It
replaced retiring a builtin tag per removed class, and it closes a coupling
nothing else checks: a frame of a kernel method is reconciled against the
loading binary's body of that method. `FORMAT_VERSION` stays 1; images are
disposable ("Layout"). `inspect` prints it (`shipped world …`).

**What may be named where** is a static rule, computed from the declarations
when a module is recorded and kept with each name (`Positions`, recorded by
`record_shipped_module` and `record_shipped_class`) and checked per position
by the encoder and the decoder alike (`shipped_names::NamePosition`):

| Position | Allowed |
|---|---|
| a value (slot, register, element, root), a class as a value | a shipped class that is public or that a recognized public class accessor of its module answers (`MixinMethod::class_accessor`), and its metaclass -- but a kernel class public only for another shipped module (`IndexedCollection`) |
| a superclass, `MIXIN_SHIPPED` | such a class, not its metaclass, but `Behavior`, `Class` and `Metaclass` |
| a record's class | any shipped class but a module's root and `Top`; `Activation` only on a frame record, `FutureImplementation` only on an unresolved future's; never a metaclass or a level |
| a frame's `current_class` and kind, a class record's declaring class | any shipped class, metaclass or level |
| `VALUE_SHIPPED_MODULE` | a frame record's receiver or registers only, and never in a started image |

It keeps every capability out of a value -- `Top`, `Activation`,
`FutureImplementation`, the module roots and their instances are no value,
as no builtin class tag or platform path could name them either -- lets a
program keep every class it can reach by running, and keeps a private
class without a public accessor, `NonBooleanReceiver` or a future `File`
say, unnamed as a value. The mirror's class has no declaration and so no name.
Locks (3) and (4) of the mirror are this rule: the encoder names a shipped
object by its declaration and refuses one the rule does not allow where it
stands, and the decoder refuses the same, `Top` and `Activation` among
them, as `ShippedNameNotAllowed` ("Verification" above). A class accessor
counts wherever it is declared in the module, by the class name it answers
(`shipped_world.rs`, `accessed_classes`), which is right while every shipped
class is declared directly in its root ([OPEN-WORK.md](../OPEN-WORK.md)).
So `ByteArray`, `StringBuilder` and `DataView`, and `TypedArray` and the ten
typed arrays, named by their module, `TypedArraysFactory TypedArray`,
`TypedArraysFactory Int16Array` and so on, stand as values, superclasses and
records' classes; the private `LittleEndianDataView` and
`BigEndianDataView` as a record's class or a frame's only; and the levels
`IndexedCollection` adds as a frame's class only
(`a_view_and_a_builder_round_trip_through_a_package`,
`crates/ns-runtime/tests/image.rs`). `IndexedCollection` itself is public
only so that `TypedArraysFactory.ns`'s root, which holds the kernel, may
send `kernel IndexedCollection`; neither `Object`'s accessors nor
`KernelNamespace`'s hand it out, so no program reaches it by running, and
it stands where a private class without an accessor does: as a record's
class or a frame's, never as a value or a superclass
(`shipped_world.rs`, `KERNEL_CLASSES_FOR_SHIPPED_MODULES`, a named list
because neither facet's accessors are recognized as class accessors of a
declaration; `indexed_collection_as_a_value_or_a_superclass_is_refused`).

**A started image names only the kernel, and no module instance.** Before
`main:args:` a program can reach nothing of `PlatformFactory` or the
modules under `platform/`, so the decoder refuses an image with `FLAG_START`
whose shipped-name section names another module
(`ShippedNameInStartedImage`,
`a_started_image_naming_another_module_is_refused`). No program reaches a
module's instance either, the kernel's included, and a started image may
hold frames -- a block in a slot brings its frame along -- whose registers
a closure reads, so the decoder refuses a started image naming any module
instance the same way (`a_started_image_names_no_module_instance`). The
encoder refuses to package either ("Packaging" above).

**A running world's image is trusted.** A started image holds no module
instance and stays untrusted input. An image of a running world
-- a snapshot today, a suspended one later ("Planned: suspend and resume")
-- may hold frames of shipped code whose registers legitimately hold the
kernel, a module instance or an `Activation`, and a crafted frame could pass
one to the program's code. Such an image is loaded only when its host
decides to, and decoding it still never panics; a check over each body's
dataflow that would close the gap is open ([OPEN-WORK.md](../OPEN-WORK.md)).

## Planned: suspend and resume

An image cannot be taken mid-execution, and the obstacle is control flow rather
than format. A reified frame is fully encoded, every running activation is an
entry on the explicit activation stack ([execution.md](execution.md),
"Activations"), and nothing a running computation does holds a Rust frame: the
activation stack is its only copy, since sends do not recurse through the Rust
call stack ([execution.md](execution.md), "The trampoline") and nothing
re-enters the run loop from Rust ([execution.md](execution.md), "No re-entry
from Rust"). What is missing is a yield path out of `vm::run`, a re-entry point
that would continue a decoded frame at its `program_counter`, and an encoding
for what each stack entry's `Continuation` carries in Rust: a native loop's
`WhileLoop` or `DoLoop`, a nested class's `NestedClassCreation`, an
instantiation's `Instantiation`, the unwind an `ensure:` block's return carries
on (`Unwind`, holding a `PendingUnwind`: its target frame and whether it returns
a value, restarts or terminates), the end of the run a VM error's `signal`
returns into (`VmErrorSignalled`) and a future's forcing (`Future`, holding a
`FutureForcing`). An image records none of them today, and a suspended
activation without its continuation would resume without the rest of the loop,
the class creation, the walk or the unwind it belongs to. The depth limit's
reserve (`Mutator.depth_reserve`), a frame held while an error near the limit is
signalled, would have to travel with the stack as well
([execution.md](execution.md), "The depth limit"). Every reified frame has
room for its registers, since a frame object is allocated with its code's
full register count ([object-model.md](object-model.md), "The object
layout"). Suspending
then means reifying and divorcing the whole stack, which is the same operation a
debugger performs on the frames it touches — and the same operation upstream
performs, since Cog flushes its stack zone to contexts (`divorceAllFrames`)
before taking a snapshot. An activation's `program_counter` being stored only at
sends, `MakeClosure` and `NonLocalReturn` is *not* an obstacle: those are
exactly the safepoints, and a suspension could only happen at a send boundary.

Whoever builds this must first close a decoder gap that today is inert only
because nothing reactivates a decoded frame. `decode::verify_graph` checks a
frame's `program_counter` and register-file size against the compiled body it reconciles
each frame against, but it never cross-checks `home` against `kind`: a
crafted image can pair a method-kind frame (`home` meaningful only for
`FrameKind::Closure`) with a non-nil `home`. Nothing observes the
mismatch today, because the VM never runs a frame loaded from an image
-- only ever an activation `Mutator::push` built.
A decoded home is only ever consulted as a return target, and a decoded
frame is never married to a live activation, so a `^` into one is
`cannotReturn` whatever its record says.
Suspend-and-resume and live debugging are exactly what would start running
a decoded frame — at which point this dormant assumption becomes
load-bearing, and whoever wires up re-entry has to decide, deliberately,
whether `verify_graph` gains the cross-check or the re-entry path treats a
mismatched `home` as its own untrusted-input case.

## Planned: edited mixins in the image

Would change: a program a MirrorBuilder edited is written to an image and
loads back edited -- every mixin it names now, with its compiled code, is a
row of the mixin table, whether a module declared it or an install put it
there (decided with the user on 2026-10-05; road step 5,
[liveness-and-reflection.md](liveness-and-reflection.md), "Planned: road
step 5, mirrors and atomic installation").

Today the rows come from what the modules declared: `image::mixins::encode`
takes the module roots' mixins and plans one tree per module from them
(`image/mixins.rs`), the code table holds the code those rows reach, and
the encoder refuses a class object whose mixin is part of no module the
image carries (`image/encode.rs`). An edit installs a fresh mixin under the
old one's id ([liveness-and-reflection.md](liveness-and-reflection.md),
"Planned with road step 5: an edit replaces the mixin under its id"), so
the encoder walks each module's tree through the ids to the mixins they
name now: an edited mixin is the row of the declaration it replaced, in
the same place of the same tree, and its code enters the code table like
any other. Nothing about the format's trust changes: a row an edit
produced is verified as any row is ("Verification"), the shipped mixins
stay named by declaration, and an edit to a shipped mixin is refused, so no image
carries one. This is the step toward the graph style for user-defined
behaviour that "How upstream compares" below names, taken in the table
style: the row is the unit, not a graph of method objects.

Open, for road step 5's plan:

- **Code an older version of a mixin still runs.** A closure keeps the code
  it was made with, and a frame the code it was running, after an edit
  replaced its mixin; the code table holds today only code a row reaches,
  so such code has to enter it from the graph's closures and frames, with
  the decoder's reconciliation of a frame against its compiled body
  ("Verification") unchanged.
- **Source text and provenance of an edited mixin.** `FLAG_HAS_SOURCE`
  carries one text per module ("Source") and a row's provenance span points
  into it; an edited mixin's span points into text that no longer
  describes it. Writing the edited declaration's own text, dropping the
  module's text once any of its mixins was edited, or marking the row's
  span absent are the candidates.

## How upstream compares

Recorded because the difference is a deliberate choice, not an accident. Sources
are listed per file in `docs/references/PROVENANCE.md`.

Primordial Soup serializes an object graph rooted in an **object store** — an
array holding every object the VM must reach without a name lookup: the
singletons, the scheduler, the selectors the VM itself sends, and the classes
with VM-known formats. It needs one because a snapshot is a graph, so those
objects are named by position rather than by address. Its deployment artifact is
an application object packaged with `packageUsing: manifest` and serialized
having never seen a platform, which arrives later as `main:args:`'s first
argument. Its snapshots contain no running state: activations are not serialized,
and activation objects materialize only when a mirror asks for one.

We need no object store, because the bootstrap is Rust and deterministic: what
the VM needs is reconstructible rather than serialized. The deployment
artifact is packaged the same way upstream's is, just before `main:args:`
with the application as the graph's root, and needs no store for that
either: the application is the graph's one root, and the entry is always
`main:args:`.

The trade is real in both directions. Upstream can ship a modified kernel, can
round-trip behaviour created at run time because methods are ordinary graph
edges, and reuses one format for isolate messages. We get artifacts in kilobytes
rather than a ~1.27 MB floor, and hold the untrusted-input invariant cheaply
because a hostile file cannot replace a shipped class. **The decision is to
stay with the table style**, and to move the line one step toward the graph
style for *user-defined* behaviour only -- with road step 5's edits, as rows
of the mixin table ("Planned: edited mixins in the image") -- never for the
shipped world.

## Decisions

- **The application is packaged just before `main:args:`, as upstream
  packages it** (decided with the user on 2026-09-29, before the Are We Fast
  Yet benchmarks, whose harness is a program of several modules). Newspeak
  on Squeak's `DeploymentManager.ns` sends `packageUsing:` and serializes
  the result, and Primordial Soup's `CompilerApp.ns` does the same; the
  artifact then holds what the program is and none of the world it runs in,
  because the platform arrives only as `main:args:`'s first argument. It
  replaced two artifacts -- a stateless binary written before instantiation
  with a recorded entry selector, and a snapshot written after it -- with
  one, and the command line's `snapshot`, `--send` and `--entry` with the
  one entry every started program receives. The run-corpus gate kept its
  modules, which declare no `main:args:`, by starting each through a
  generated application rather than by keeping a second entry.
- **One format; the image with heap state is the same artifact.** A started
  image and an image written mid-execution differ in where the run was when
  it was written, not in their sections, so there is one decoder to hold the
  untrusted-input guarantee, and a header flag, `FLAG_START`, says which to
  expect. The entry needs no field, because it is always `main:args:`.
- **Modules reach the application through a manifest answering
  `doesNotUnderstand:`**, not through a namespace builtin, a factory
  convention or a lookup on the VM mirror: it is what both upstream
  `Manifest` classes do, it needs nothing privileged, so the manifest is
  ordinary user code an image may carry, and a missing module is an
  ordinary `doesNotUnderstand:` at run time. A module shadowed by a public
  member of every object is refused before anything runs, since the
  manifest could never answer it.
- **The project file is JSON** (the user's choice), read with `serde_json`,
  which `ns-analyzer` already depended on, so no new crate entered the lock file.
- **A project is the directory its `newspeak.json` describes** (decided with
  the user on 2026-10-05). The file is recognised by its name alone, as a
  Node package's `package.json` is, so a command given a directory finds it
  and no other JSON file is taken for one.
- **The application module's key is `main`** (decided with the user on
  2026-10-05). A file that still says `root` is refused for the missing
  `main`, as any file without one is; `root` is not singled out.
- **Other keys are allowed and ignored, by the reader and by the schema**
  (decided with the user on 2026-10-05), so a file may carry what an editor
  or another tool wants. `$schema` is one of them, described by the schema as
  a string so that an editor without the plugin finds the schema and
  completes it; the reader checks only that type. The rules are: `main`
  required, it and every entry of `modules` a non-empty path ending in
  `.ns`; `modules` optional, with no path in it twice or equal to `main`.
- **Validation is strict in the reader, held to the schema by a test**
  (decided with the user on 2026-10-05). No JSON Schema validator crate is
  added, so no dependency comes with the schema: the reader states each rule
  with a message of its own, and `project_schema.rs` keeps the two in
  agreement. The reader alone refuses what JSON Schema cannot state: a key
  it reads written twice, a script name written twice (decided with the user
  on 2026-10-05, as a duplicate key is refused), one module path in two
  spellings, and a `modules` entry equal to `main`'s path; `project.rs`'s
  own tests pin them. Chosen beside it: `.ns` is matched case-sensitively,
  as the schema's `pattern` and the plugin's file type match it; the reader
  compares duplicate paths by their components.
- **The schema lives at `schemas/newspeak.schema.json`, the one copy**
  (decided with the user on 2026-10-05). The plugin's build copies it into
  its resources, and the plugin maps it to every file named `newspeak.json`.
- **Scripts, as Node's are** (decided with the user on 2026-10-05). A
  script's arguments are handed to `main:args:` as if written after `--`,
  and arguments after `--` follow them, as `npm run timed -- Sieve` does.
  Arguments stay strings from every source, so a program sees one form
  however it is started. `--script` is for `run` and `bench` only: `build`
  sends no `main:args:`, and a script recorded into an image would need a
  section of the format of its own. Chosen beside it: `--script` with a
  source file, and a name the project lacks, are refused, the latter listing
  the names there are.
- **A script is one string, split like a POSIX command line on every
  platform** (decided with the user on 2026-10-05). One string reads as the
  command line it stands for, as a script in `package.json` does, where an
  array of strings, one per argument, does not; the array form is not
  accepted. The rules are a POSIX shell's word splitting, quotes and
  backslashes, the same on every system rather than each platform's own
  (Windows splits a command line otherwise), so that one `newspeak.json`
  gives the same arguments everywhere. Nothing is expanded, and nothing is
  an operator or a comment, `#` included, since no shell runs a script. The
  splitter is hand-written in `ns-cli`, so no dependency comes with it, and
  runs when the project file is read, so that a script which does not split
  refuses the file before anything runs.
- **A script's name is a non-empty name without surrounding whitespace**
  (decided with the user on 2026-10-05). An empty name, or one beginning or
  ending with whitespace, is refused by the reader and by the schema
  (`propertyNames`): such a name is easily mistyped on a command line and
  invisible in a list of names. Whitespace inside a name is allowed, and so
  is a leading `-`, which `--script=<name>` passes as one token.
- **Module order is fixed: the application module, the project's modules
  as written, then `Manifest.ns`.** `run`, `build` and `load` compile or
  rebuild the modules in the same order, so every identity hash the bootstrap draws is the same
  under all three, and an image names a module by its index, so two modules
  may share a name without either becoming unreachable to the format.
- **A module is named by index, and the one-root tags retire.** `CLASS_USER_ROOT`
  (class tag 1), `CLASS_ROOT_LEVEL` (class tag 3), `HASHED_ROOT_CLASS` (hash
  tag 1) and the header's `FLAG_STATELESS` (bit 2) assumed one module per
  artifact or an artifact with no graph; their numbers are retired, never
  reused, and `CLASS_MODULE` (6), `CLASS_MODULE_LEVEL` (7),
  `HASHED_MODULE_CLASS` (5), `HASHED_MODULE_LEVEL` (6), `FLAG_START` (4) and
  the origin tags `ORIGIN_FILE` (0) and `ORIGIN_EMBEDDED` (1) took the next
  free numbers, so a stale artifact meets an unknown tag instead of decoding
  into a different program. `CLASS_MODULE`, `CLASS_MODULE_LEVEL`,
  `HASHED_MODULE_CLASS` and `HASHED_MODULE_LEVEL` are retired in turn, since
  a module's root class and its levels became records of the graph, which
  the module section names by object id (2026-10-13, "An image names the
  shipped world by declaration" below).
- **Applications are rebuilt, not serialized**, so a stored layout can never
  disagree with its mixins.
- **Inline-cache state is never serialized** (2026-09-16), and neither is a
  dispatch id or a `feedback_base`: all three are per heap, so a loaded
  artifact starts with empty caches and mints its own ids.
- **Widening the send's call site retired its four tags** (2026-09-16), 7–10,
  rather than reusing them, and the call-site list stayed in the code record,
  written empty and refused otherwise, so a stale body was refused rather than
  misread. Superseded 2026-09-18, next bullet: the list and its count are gone.
- **Call-site numbers are not serialized** (2026-09-18): a number is
  feedback-table bookkeeping, not program meaning, so the decoder assigns it
  instead of trusting the file. 21–24 retired in turn for 25–28; the
  call-site list and its count field, and the density check
  (`CallSitesNotDense`) it existed to run, go with the numbers; a code
  record carries a flag for a family compiled uncached instead. Rejected:
  per-family hash tables (a hash per hit, and shared numbers stay unsafe);
  the selector in every cache entry (a compare per hit, and an oversized
  number still needs bounding) — the alternatives 3b's refusal already
  rejected, still true once the refusal moved from checking a claimed number
  to never reading one.
- **Retiring `LoadBuiltinClass` retired its tag 2** (2026-09-24), never
  reused: a bare predefined class name became a send to `Object`'s accessor,
  so no lowering emits the instruction, and a stale artifact's tag-2 bytes
  meet an unknown tag instead of decoding as whatever instruction took the
  number next. A kernel method's fast path is never carried, so the format
  needed nothing new.
- **`FORMAT_VERSION` is not bumped**, because there are no external users and the
  safety property does not depend on it.
- **Artifacts are readable but not writable as text.** A dump serves inspection;
  a loadable text format would be a second codec to keep in step, and the need
  that would justify it is already met by corrupting real images.
- **An image records the hashes of the objects it names rather than
  carries**, so a hash the program was given survives a load like every
  other; a reserved-range rule was rejected.
- **Frames are object records, and the decoder sizes every record before it
  allocates** (decided with the user on 2026-09-29 and 2026-10-03, built with
  road step 3a on 2026-10-03; "Frames are object records", "Two passes"). A
  reified frame is an object, so it joined the object table under the next
  free body tag, `OBJECT_FRAME` (10), and the frame table, its count and the
  activation record (9, retired, not reused) went; every frame is written
  with its full register file, so its record's count is its size like any
  other's. A placeholder sized from its own record's bytes makes each byte
  count once, so the bound of a rejected load is the largest of the per-record
  and per-value ratios rather than their sum; the wire stays id-based and
  records carry no size field.
- **An image names the shipped world by declaration** (decided with the
  user on 2026-10-12, built 2026-10-13; "Shipped names"): the binary's own
  code builds every shipped module instance and class -- the boot the
  kernel's and `PlatformFactory`'s, a first demand or the loader the rest
  (since 2026-10-15) -- and an image names one by module, declaration and
  kind through a section the loader checks, builds and resolves by lookup
  before the graph; the platform and the program's module roots are
  ordinary records; a fingerprint of the shipped world in the header
  refuses an image another binary wrote. Building the whole world at boot
  cost about 3.7 ms a process start when the shipped source was compiled
  ([execution.md](execution.md), "What the collections module cost"); the
  embedded shipped code and the build on first demand took it back
  ("What the precompiled shipped code moved" there).
  Rejected: more builtin tags (a builtin is a class Rust needs a handle on),
  records relying on value semantics (a twin, not the canonical object),
  instances the loader supplies through accessors the image chooses, and
  the shipped world in the image (every image as trusted as the binary).
- **A string record under the `Symbol` class is refused**
  (`StringOfSymbolClass`, 2026-10-03): with `Symbol` a subclass of `String`
  and a symbol told from a string by its class alone, such a record would be
  a second, non-canonical `Symbol`, which §5.1.6 forbids.
- **A `ByteArray` is a record of its own, and only `ByteArray` has one**
  (decided with the user on 2026-10-12, built on 2026-10-13; "A
  `ByteArray` is a record of its own"): the next free body tag,
  `OBJECT_BYTE_ARRAY` (11), its length and its bytes, the alignment
  reserve not on the wire since the decoder aligns to its own placeholder;
  the views and `StringBuilder` are ordinary instance records of their
  shipped classes, so none of them is a builtin. The body and the
  `ByteArray` header are held to each other both ways, since a record
  carries exactly its class's layout (`ByteArrayRecordMismatch`).
- **An image keeps what a collection would keep** (designed with the user
  on 2026-10-16, built the same day; "A `WeakArray` and an `Ephemeron` are
  records of their own"): `OBJECT_WEAK_ARRAY` (12) and `OBJECT_EPHEMERON`
  (13), the next free body tags, and an encoder that writes a weak slot
  `nil` and mourns an ephemeron where nothing else in the graph reaches
  the object or the key, as Primordial Soup's serializer does
  (`prior-art/primordialsoup/newspeak/Serialization.ns:548-561, 718-726,
  793-795`), so a snapshot carries no object only weak references hold.
  What takes no record -- an immediate, a shipped name, a value written by
  value -- is kept whether or not anything else reaches it: keeping more
  than a collection would is never wrong, since a collection's timing is
  unspecified.

## Open questions

- **The decompiler** that is to provide export back to `.ns`. Mitigated by the
  debug source text, unwritten.
- **Whether upstream strips source at packaging time.** A measurement of four
  `.vfuel` artifacts concluded that nothing is stripped, ~44% long text runs in
  each. `CompilerApp.ns`'s `keepSource: (appName = 'TestRunner')` shows a switch
  that is false for every ordinary application, which sits in tension with that.
  Both can be true — different toolchain, or the measured runs were selectors
  rather than source — but the claim should be re-checked rather than left
  standing.

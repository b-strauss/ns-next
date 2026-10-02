# Image format — serialization and deployment artifacts

> **Status:** built. One artifact: an application assembled from one source
> file or a project of several modules, packaged just before `main:args:`,
> round-trips through `build` and `load` and can be inspected without being
> loaded. The decoder decides from the value alone whether an integer or a
> float loads as an immediate or as an object ("Integers and floats on the
> wire"). **Planned:** the decoder builds variably sized objects from a wire
> format that stays id-based ("Planned: the image under the new
> representation"); and suspend and resume.
> **Code:** `crates/ns-runtime/src/image/` — `mod.rs` (`package`, `snapshot`,
> `load`), `encode.rs`, `decode.rs`, `mixins.rs`, `modules.rs`,
> `platform_paths.rs`, `symbols.rs`, `opcodes.rs`, `inspect.rs`;
> `crates/ns-runtime/core-library/Manifest.ns`. CLI: `ns-next build`,
> `ns-next load`, `ns-next inspect`, and `ns-next disasm` on an artifact;
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
`main: platform args:` with no flag at all, so starting one executes its
code with the same exposure as running the equivalent source. Recursion depth
is bounded by one counted limit, 100,000 activations, an ordinary Newspeak
error rather than a native stack overflow, since nothing re-enters the run loop
from Rust; and a crafted `SendInlinable` cannot use the count's one
exemption to recurse uncounted. Nor can it stack more exempt activations above
a counted one than source can: that number is bounded by how deeply blocks
nest, which the decoder caps at the parser's own ceiling ("Verification"
below); see [execution.md](execution.md), "The depth limit". A mutated
artifact that decodes cleanly can still abort while executing — a native that
indexes an argument it was not given, for one: the `whileTrue:` and
`whileFalse:` leaves read `arguments[0]` unchecked, and the decoder does not check a send's argument count
against its selector — and that is not a decoder defect.

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
(a stored platform, the objects platform paths name, several roots), and it
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
[header][symbols][code][mixins][modules][platform paths][graph][tag-named hashes]

header:  magic "NSNX", FORMAT_VERSION (u32), flags (u8)
modules: count (u32), then per module [origin][source?][root section]
graph:   object table, frame table, roots
```

The header carries magic, `FORMAT_VERSION` and a flag byte. Two flags are
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
from validating ids and lengths, not from a version field.

## What is *not* serialized

This is the part that decides how big an artifact is and how large its trusted
surface is.

- **Builtin classes, each module's root class and the levels its `<:`
  clauses add** are referenced by a **class tag**
  wherever a class object appears — an object's `.class`, a class-valued slot, a
  frame's `current_class` — never structurally encoded. On load a tag resolves to
  the live class the bootstrap produced. The bootstrap itself is the fixed
  root/special-objects table. So is the core library: a builtin class's
  methods are the loading process's own `Kernel.ns`, compiled at its
  bootstrap ([core-library.md](core-library.md)), and no image carries the
  kernel's code, its instance or the VM mirror — which is also why
  `Instruction::SendMirror`, emitted only into shipped core-library source
  (`Kernel.ns`, `PlatformFactory.ns`), has no wire tag
  ([compiler.md](compiler.md), "Mirror sends").
- **`nil`, `true` and `false`** are unboxed `Value` scalars with no heap identity
  to preserve, encoded as scalar tags.
- **The `platform` singleton** gets its own tag, resolved to the loading
  interpreter's own singleton. Essentially every module stores it in a slot, so
  without this the whole platform graph would come along.
- **Every object reached through the platform** is named by its **platform
  path**, the public members sent from the platform that answer it
  (`transcript`, `collections`, `Transcript`, `Platform`): listed in a section
  of its own, resolved by the
  loading host before the graph is decoded, and never written as a record
  ("Platform paths" below, [core-library.md](core-library.md), "Platform
  paths in the image"). The platform's own tag is the empty path.
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
  record's header names its metaclass record. The metaclass of a builtin, a
  module's root class or one of its levels is named by tag, `CLASS_METACLASS`
  followed by that class's own reference, one level deep. A class record applying some
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

A hostile image therefore cannot replace a builtin, because a class tag resolves
against the fresh bootstrap rather than against file content.

## Sections

In order: the symbol table, the code table, the mixin table, the module
section, the platform paths, the graph (object table, frame table, roots),
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

The next free instruction number is 40. A send's `CallSite` -- its number
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

**A count past its field refuses the image, never aborts it.** A count a
program decides is written through a checked narrowing that answers an
error, `image encoding cannot write a <what> of <n>: it does not fit the
image format's 16-bit field` (or `32-bit`): in 16 bits a register list, a
closure's parameter and copied-value counts and a method's or factory's
arity (`encode::u16_field`), all of which a valid program can pass
([compiler.md](compiler.md), "Compilation cannot fail"); in 32 bits a
string's, a symbol's, a value list's and a `LargeInteger`'s digit count
(`encode::u32_length`). Every other count goes through the asserting
`encode::u32_field`, because no program can pass it: object and frame ids
and counts are bounded by the heap's 32-bit handles, declaration, statement
and instruction counts by source, whose `Span` offsets are 32-bit --
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
`MakeClosure` creates -- has been read in full. The golden image is 1281 bytes.

Both lists are declared once, in `image/opcodes.rs`, and used by the encoder and
the decoder alike rather than repeated as magic numbers on each side.
`crates/ns-runtime/tests/opcode_tags.rs` pins every value and checks that no two
share a number, so a renumbering fails the suite at the moment it happens instead
of surfacing later as an image that no longer means what it said.

The graph section's six tag families live there under the same rule: how a
`Value` is encoded, an object record's body, how a class record names its mixin,
how a class reference is named, what a frame was activated for, and which tag-named object an entry of the closing
hash table names (`HASHED_*`, below). The platform paths added one tag to three
of them, each followed by a `u32` index into the platform path section:
`VALUE_PLATFORM_PATH` = 8, `CLASS_PLATFORM_PATH` = 5 and
`HASHED_PLATFORM_PATH` = 4. They were bare literals in the encoder and
again in the decoder until `inspect` became a third reader of those bytes — and
three hand-kept copies of one numbering is how a viewer starts quietly
disagreeing with the loader. A closure frame (`FRAME_CLOSURE`) names the closure
object it runs by object id: the closure's data sits inline in its object, so
that the tracer can rewrite its two frame handles in place
([memory.md](memory.md), "What the collector traces").

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
`Mutator::new_integer`, which adds to a failed load's peak
([memory.md](memory.md), "The interim cost, and the one question still
open"). `inspect` renders one as `large-int <decimal>`, through the same
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
boxed float allocate, and add to a failed load's peak
([memory.md](memory.md), "The interim cost, and the one question still
open").

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
and `verify_mixins` reconciled with that row's factory. A level whose clause
named a builtin (`<: Value`) is recorded as `{MIXIN_BUILTIN, builtin tag,
superclass, applied clause}`: its mixin is the builtin's, which no row
describes, and its enclosing object and declaring class are the builtin's own —
the core library's `Kernel`, which holds the VM mirror — so the record carries
neither, and the decoder takes both from the loading bootstrap, as building the
level from source does. The record changed shape when its mixin became a tagged
reference, so the old class-record tag 5 is retired
(`opcodes::RETIRED_OBJECT_BODIES`).

**A class record's header names its metaclass record** (object body tag 8,
`OBJECT_METACLASS`): every class object's class is its metaclass (§3.2), so a
class record's class tag is `CLASS_RECORD` with the id of a record whose own
header is the builtin `Metaclass` and whose body is one class reference, back
to its class record. Nothing else about a metaclass is written — its mixin,
application, enclosing object and declaring class follow from its class
([object-model.md](object-model.md), "The image format") — and the decoder
applies it over `Class` from its class's row (`rebuild_metaclasses`). The
metaclass of a class that is no record — a builtin, a module's root class or
one of its levels, or a class a platform path names — is named by class tag 4,
`CLASS_METACLASS`, followed by that class's own class tag, which may be
`CLASS_BUILTIN`, `CLASS_MODULE`, `CLASS_MODULE_LEVEL` or
`CLASS_PLATFORM_PATH` and nothing else (`read_prebuilt_class`), so the nesting
is one level deep. The encoder refuses a class record whose class object has no
metaclass rather than write a header the decoder could not pair. A metaclass
record is a record like any other — no shorter than the minimum record, one
placeholder in the first pass — so it adds nothing to the decode
amplification, whose placeholders come to about 16.0x of the bytes left at
the graph section in a release build and whose decoded values that allocate
-- the worst an `i64` beyond 61 bits, a `LargeInteger` for 9 bytes -- bring
it to about 23.1x ([memory.md](memory.md), "The interim cost, and the one
question still open"); a tag-named metaclass is no record at all
and allocates nothing.

**Module section.** Which top-level modules the artifact carries
(`image/modules.rs`): a `u32` count, equal to the mixin table's trees, then
per module, in module order, its origin, its source text when the header
sets `FLAG_HAS_SOURCE`, and its root section. **Module order** is the root
module first, then a project file's modules in the order written, then the
embedded `Manifest.ns` ("Packaging" below); a source file's program is its
one module. The count is bounded by the bytes left before anything is
reserved for it.

The **origin** is a tag from `opcodes.rs`, `ORIGIN_FILE` (0) followed by the
path the module was read from, or `ORIGIN_EMBEDDED` (1) followed by the name
of a source compiled into the binary -- `Manifest.ns`'s. It is provenance, in
every artifact, source or not; `inspect` shows it. Source travels for every
module or for none, as the header's one flag says, so the encoder refuses a
set of modules of which only some carry theirs rather than invent an empty
text.

A module's **root section** is the class its root inherits from, as a
builtin tag behind a presence flag that is always set, then the root's own
`<:` levels, lowest first, each as its clause's position in that module's
root row and the builtin that clause named. The root rule lets a root
inherit only `Object` or `Value` and apply only `Value` over `Object`
([object-model.md](object-model.md), "Mixin application", "The root rule"),
so builtin tags are enough, and there is at most one level; a root the rule
refuses is never written ("Verification" below). `load` rebuilds every
module's root class and levels in module order
(`modules::build_module_classes`), before the graph, so the graph names a
module's root by the module's index (`CLASS_MODULE`, class tag 6, followed by
a `u32`) and a level by module index and position (`CLASS_MODULE_LEVEL`,
class tag 7, followed by two `u32`s) rather than as records. An index past
the modules is `BadModuleReference`, a position past a module's levels
`BadModuleLevel`. A module is named by its index and never by its name,
which is why two modules may share one ("Packaging" below). The class tags 1
(`CLASS_USER_ROOT`) and 3 (`CLASS_ROOT_LEVEL`), which named *the* root class
of an artifact that carried one module, are retired
(`opcodes::RETIRED_CLASS_TAGS`). Rebuilding the modules in the order the
program loaded them is also what keeps every identity hash the bootstrap
draws the same as in the run that wrote the image.

**Platform paths.** Immediately before the graph:
a `u32` count, then per path a `u32` member count and that many symbol ids
(`image::platform_paths`). A started image's section is empty: nothing
before `main:args:` holds the platform or anything reached through it
("Packaging" below), so the section serves an image `image::snapshot`
writes, and the image with heap state to come. A path is the list of public members sent from the
platform, one unary send each. The encoder finds them with
`platform_paths::discover`, which walks from the platform over, for every level
of an object's class, each **public** slot's field — a lazy slot's included,
skipped while `nil` — and each **public** nested class's cache slot, skipped
while empty, and then over each **public class accessor** the object
answers: a method the compiler recognized, in shipped source other than the
kernel, as exactly `^outer <Root> <Nested>` naming a nested class of its
module's root (`MixinMethod::class_accessor`, read by the image code alone —
no fast path and no per-send check). For one, `discover` decides without
running anything what the send would do — the public lookup must find the
recognized method, its `outer <Root>` must reach a module root's shipped
instance, and the lexical send of `<Nested>` there must find that root's
nested class — and reads the root instance's cache slot for that class,
skipped while empty. It records every **shipped object** it reaches — a class object
whose own mixin is compiled from shipped source (`Mixin.shipped`,
`Heap::class_is_shipped`), or an instance with slots of such a class — and
descends into shipped instances only, never into a class object. The one
definition, `platform_paths::is_shipped_object`, is what the encoder's
refusals and the loader apply too, so no path is written that a loader must
refuse. Breadth-first, visiting each object
once, so an object's path is its shortest, and among equals the first in
member order — superclass level first, within a level its slots and nested
classes in declaration order, then the class accessors by selector: one
world encodes to one byte sequence. A string or an array is no shipped instance even when its class is a
builtin: it carries its own data, and the image writes it as a record. Only the
paths the graph **uses** are written, in the order the graph first reaches
them, numbered densely from 0; at most `MAX_PLATFORM_PATHS` (4096) of at most
`MAX_PLATFORM_PATH_LENGTH` (32) members each. Today the platform is an
instance of the nested class `Platform` of the shipped `PlatformFactory.ns`
([core-library.md](core-library.md), "The platform"): its public slots are
the lazy `transcript`, `collections`, `kernel` and `time`, and its class
accessors `Platform`, `Transcript`, `KernelNamespace`, `Time` and
`Stopwatch` name those classes of the module, once the module root's
accessor for each has run. `time`'s own class accessor `Stopwatch` answers
the class `Stopwatch` does, so the path `Stopwatch`, the shorter, is the
one written for it. `collections`
answers an instance of the shipped `CollectionsFactory.ns`'s facet
`Collections`, whose own class accessors `Collections` and `List` name that
module's two classes the same way, so `collections Collections` and
`collections List` are platform paths too, with the facet as their first
member. All seven classes are private to their root: a class accessor reaches
them by a lexical send (§5.9), and `discover` looks the class up the same way
(`dispatch::lexical_member`), so a private nested class is named by its
accessor's path like a public one.
Neither root instance is named by any path, and neither is ever written:
nothing a program can name answers `PlatformFactory`'s root, and
`CollectionsFactory`'s root is reached only lexically, from inside its own
module, never through a slot a program can read. A second platform a program
made with `platform class new` is an ordinary instance record of the class
the path `Platform` names, and a second facet made with `platform collections
class new` is one of the class the path `collections Collections` names.
A `Stopwatch` is an object, not a value, and is written the same way, as a
record of the class the path `Stopwatch` names with its readings; a started
one reads a wrong time once loaded into another process
([core-library.md](core-library.md), "The platform").

An object a path names is referenced as `VALUE_PLATFORM_PATH` (value tag 8), a
class a path names as `CLASS_PLATFORM_PATH` (class tag 5), each followed by the
`u32` index into the section; a path-named class counts as prebuilt, so
`CLASS_METACLASS` may be followed by one. The encoder refuses a shipped object
no path names instead of writing it: a class object compiled from shipped
source that no builtin tag names, and a **module instance** — an instance of a
shipped class whose class object has no enclosing object, such as the kernel
instance — which holds the core library's state as the platform holds the
mirror. A level a `<:` clause naming a builtin added applies that builtin's
shipped mixin but belongs to the user class, and stays a `MIXIN_BUILTIN`
record.

**Object table.** Every heap object reachable from the roots — instances,
strings, symbols, arrays, closures — decoded in two passes (placeholder, then
fill) so cycles and shared references round-trip with identity preserved. A
closure encodes its compiled body as a code id, its captured frame as a frame
id, and the values it was handed when it was created: a `u16` count and that
many encoded values. It encodes neither its parameter names, which nothing
reads — the one thing anything asked of them was how many there were, and that
is its body's `argument_count` — nor its home, which `Heap::closure_home`
derives from the captured frame. That is a different record shape, so the
closure body tag 4 is retired (`opcodes::RETIRED_OBJECT_BODIES`) and the
record is 6.

**Frame table.** Every `Frame` reachable from the roots, with receiver, args,
temps, `program_counter` — an instruction index into the frame's compiled body — `sender`,
`home`, `current_class`, `kind` and `uncontinuable`. A closure frame's `kind`
names the closure object it runs, by object id; that the id names a closure is
checked once every record has been read (`NotAClosureReference`), as a class
reference is. A `<:` clause message's frame (`FRAME_MIXIN_APPLICATION_MESSAGE`,
6) names the *level* its clause added rather than the applying class: the
applying class's mixin carries one message body per clause, and the level's
applied clause says which one ran, which is how `verify_graph` finds the body to
reconcile the frame with. A block written in such a message captures that
frame, so a snapshot can hold one.
Frames and objects reference each other freely, so both tables are
placeholder-allocated before either is filled.

**Identity hashes are preserved verbatim** for every object the image carries
as a record. After a load the identity-hash counter is advanced past the largest
hash the image contained, so a hash assigned afterwards never collides with a
loaded one. A record's 0 means "not yet assigned" and is preserved the same way
([object-model.md](object-model.md)).

**The exception is an object the image names by tag rather than by record**: a
builtin class, a module's root class or one of its levels, the `platform`
singleton, an object a used platform path names, and the metaclass of a
builtin, of a module's root class or level or of a path-named class
(`encode::is_class_tag`, and the platform and path checks beside it in
`GraphTable::visit_value`). None of them is a record, so nothing about a
record could carry its hash. Instead the graph section closes, after its
roots, with a table of exactly the tag-named objects whose stored hash is not
0 — the builtin classes in wire-tag order, then the module roots in module
order (`HASHED_MODULE_CLASS`, 5, followed by the module's index), then their
levels, module by module and lowest first (`HASHED_MODULE_LEVEL`, 6, followed
by the module's index and the level's position), then `platform`, then the
path-named objects in section order (`HASHED_PLATFORM_PATH` followed by the
path's index), then the metaclasses (`HASHED_METACLASS` followed by the
class's reference) of the builtins in wire-tag order, of the module roots,
of their levels and of the path-named classes in section order — each entry
a tag from the `HASHED_*` family in `opcodes.rs` naming the object, and its
hash (`encode::encode_tag_named_hashes`). An entry naming a module or level
past the ones the module section rebuilt is `BadModuleReference` or
`BadModuleLevel` (`a_tag_named_hash_past_the_modules_is_refused`). Hash tag
1 (`HASHED_ROOT_CLASS`), which named the one root class, is retired
(`RETIRED_HASHED_TAGS`). A builtin
class a path names keeps only its builtin entry, since the loader refuses one
object listed twice. `load` restores each hash onto the object
the bootstrap built (`decode::decode_tag_named_hashes`), and refuses a 0, a
duplicate entry or `u32::MAX` like any other malformed hash. The
identity-hash counter is then advanced past the larger of this table's maximum
and the object table's, so a hash assigned afterward never collides with either.
`inspect` reads the table as part of the graph section
(`Report::graph_tag_named_hashes`).

**A module's `<:` levels have entries of their own**, although no program
can obtain one as a value today — there is no reflective `superclass` — and
the one thing that names a level in a graph is a frame's `current_class`,
when a method of the builtin the clause named, a kernel method, runs on an
instance of the module's root class; that is why `CLASS_MODULE_LEVEL` exists
rather than a refusal. The decoder rebuilds the levels with their root
class, so without an entry a hash one was given would be drawn afresh after
a load (`a_module_levels_own_hash_survives_an_image`,
`crates/ns-runtime/tests/image.rs`).

That is what Cog and Primordial Soup get without trying: their images carry
every object, classes included — a Spur class's identity hash is even its index
in the class table (`ensureBehaviorHash`, `enterIntoClassTable`) — so no hash
is ever rebuilt from anything. ns-next's bootstrap rebuilds these objects
instead, so the image has to carry what the program observed of them. That
stays true when the core library moves into Newspeak
([core-library.md](core-library.md)): the builtins stay out of the image
("How upstream compares" below), so they stay tag-named. The alternative, a
deterministic hash for each tag-named object from a reserved range, was
rejected: every tag-named object added later would need a reserved number, and
the hash would no longer be the one the program was actually given, which is
what "verbatim" promises for every other object.
`a_hash_given_to_a_tag_named_object_survives_build_and_load` in
`crates/ns-cli/tests/build_load.rs` pins the result end to end.

## How frames actually get into an image

Only through a closure or an activation object. A running activation lives on
the activation stack and is not in the object graph at all; a frame exists for
it only once a block captured it or an activation object reified it
([execution.md](execution.md), "Activations"). So a frame enters the
table only as a closure's `captured_frame` — a closure a value names, or the
one a closure frame already in the table runs — as the frame an activation
object reifies, or as the `home` of a frame already in it. (A closure has no
`home` field of its own to bring one in: `Heap::closure_home` derives it from
the captured frame.) An image is written
at a **quiesced** point — `build` writes one once the application's factory
and initializers have returned and before `main:args:` is sent, and a host
calling `image::snapshot` between sends, with the Rust call stack already
unwound and the activation stack empty; the one writer refuses to
encode otherwise — so every frame in it is dead:
divorced when its activation returned, or decoded from an earlier image and never
run. Such a frame has no sender, and carries its own registers only when
something could still read them by reference, so a closure brings in the frames
it names, the values it copied, and nothing of the call chain that created
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

**A frame with no registers at all is accepted, which a by-reference closure
can then read as `nil`.** A returning activation copies its registers into its
reified frame only when something can reach them by reference
([execution.md](execution.md), "Lifecycle"), so an empty register file is now
the ordinary shape of a returned converted frame, and `verify_graph`'s
register-file check accepts it instead of demanding a file that matches the
body. A crafted image can use that to pair a by-reference closure with a
register-less frame; the reads then answer `nil` and the writes are dropped
(`Heap::frame_local`/`frame_set_local`) where the file used to be refused.
Tightening the check is not available: nothing on the wire distinguishes a
legitimately register-less frame from a crafted one. It is the same class as
the `own_index` gap below — wrong but safe output from a mutated program,
never a panic. Under the planned object layout the gap will close: every reified frame
will have room for its registers and the encoder will write its full
register file, so a frame record's register file will have to match its body
and `verify_graph` will be able to refuse one that does not ("Planned: the image under the new representation" below).

**`Instruction::InitializeSlot`'s and `ReturnSlotIfFilled`'s `own_index` is not
one of the counts these passes bound.** `verify_mixins` checks a body's kind and
arity against its declaration, but nothing checks that a slot instruction's
index falls within the mixin's own slot count. This is harmless at run time:
`Heap::set_instance_field` and `Heap::instance_field` are total, so an
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
and a position that must exist (`BadMixinId`, `BadMixinApplicationClause`), and
each module's root section lists its root's levels at rising positions within
the clauses of that module's own root row -- never another module's
(`a_root_section_clause_is_checked_against_its_own_modules_row`) -- which
also bounds their count before anything is reserved for it.
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
`ResolvedModule` a host assembled past `resolve`. The
root-section reader (`decode_root_section`) refuses what the encoder cannot
write: a section naming no superclass at all, as `RootWithoutSuperclass`; a
superclass tag other than `Object`'s or `Value`'s, and a level naming
anything but `Value`, as `InvalidTag` ("invalid builtin class a module's root
may inherit tag *n*", "… may apply tag *n*"); a second level, or a level over
anything but `Object`, as `BadMixinApplicationClause`. So a crafted artifact
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
section that holds two is corrupt or crafted, and `decode_graph_section`
refuses it with `DecodeError::DuplicateSymbol` rather than merging them,
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
text answer the mis-classed object too.

**The VM mirror is refused on both sides of the format.** The mirror is a
capability: whoever holds it can run any leaf, and the bootstrap hands it to
the shipped core-library modules alone, `Kernel.ns` and `PlatformFactory.ns`
([core-library.md](core-library.md), "The VM mirror is the whole Rust
boundary"). Its class has builtin class tag **14**
(`BuiltinClass::VmMirror`, appended to `BuiltinClass::ALL` after the retired
13), which `builtin_class_from_tag` knows so that the closing table of
tag-named hashes can name it as it names every builtin. **Everywhere the
graph names a class — a class reference, the class behind a
`CLASS_METACLASS`, a root class's superclass, the builtin a `<:` clause
named — the decoder reads the tag through `read_referenced_builtin_class`,
which refuses 14** as an invalid tag (`read_program_builtin_class`, which
reads a superclass or a `<:` clause's builtin, calls it first): an
artifact that could name the mirror's class could instantiate a second mirror
or reach the one there is, and so claim every primitive, which no source
module can (`a_level_applying_the_mirrors_or_tops_mixin_is_refused` and
`a_root_level_naming_a_clause_out_of_place_is_refused`, whose last case is a
root level naming tag 14, in `crates/ns-runtime/tests/image.rs`). The encoder refuses the other direction:
an image whose graph reaches the mirror object is not written at all, since
it would hand the capability to whatever loaded it
(`a_snapshot_that_reaches_the_mirror_is_refused`, `interpreter/mirror.rs`),
and neither is one whose graph reaches the mirror's class, `Top`,
`Activation`, `Future` or the metaclass of one of them (`encode::unnameable_class`),
which the decoder would refuse.
Neither can happen to a program built from source, which cannot obtain the
mirror; both are there for the day something could.

**`Top` is refused the same way, for the same kind of reason.** The three
classes the object model gained carry tags **15** (`Top`), **16** (`Value`)
and **17** (`Number`), appended after the mirror's 14 rather than slotted in
beside their siblings, because a tag is assigned once and never renumbered
("A tag is assigned once and never changes" above) — so `BuiltinClass::ALL`
is a tag order and not a superclass order. `Value` and `Number` are ordinary nameable classes:
a program may write `Value` or `Number` in a superclass clause, so an image
may carry their tags. `Top` may not. §3.2 lets only `Object` have `Top` as
its superclass, and `Kernel.ns` enforces that by making `Top` the one
private class it declares, so no source module can name it —
`read_referenced_builtin_class` refuses tag 15 exactly as it refuses 14, so no
artifact can name what no source can
(`a_load_builtin_class_naming_top_is_refused`,
`crates/ns-runtime/tests/image.rs`). An artifact naming `Top` would
otherwise build a class outside §3.2's hierarchy that inherits nothing at
all, not even `doesNotUnderstand:`.

**`Behavior`, `Class` and `Metaclass` carry tags 20, 21 and 22**, after
the retired 18 and 19. `read_program_builtin_class` refuses them as the
builtin a `<:` level applies (`MIXIN_BUILTIN`) and as a root's superclass or
level, since `Object` declares no accessor for any of them and no source
module can name them — and a class over one of them could never be
instantiated. As a class reference in the graph they are accepted, since a
program reaches `Metaclass` as `3 class class class` and may keep it in a
slot; a class record's superclass is such a reference, so a crafted class
record over `Class` decodes, and its application carries the refusal to
instantiate a class object like any class below `Behavior`. An object record that is no class object but whose class refuses
instantiation as a class object — an instance of `Class`, `Metaclass` or a
metaclass — is `InstanceOfAClassClass`, and a class record whose row is some
row's class side is `ClassRecordAppliesAClassSide`: no source makes either.

**An activation object is a record of its own; its class is named nowhere
else.** `ObjectData::Activation`, an instance of the kernel-private
`Activation` class (builtin tag **23**), is written as object body tag 9,
`OBJECT_ACTIVATION`, whose body is one frame id, resolved and range-checked
like a closure's captured frame (`BadFrameId`). Its header is `CLASS_BUILTIN`
followed by tag 23, and that header is the one place an image may name
`Activation`: `decode_object_header` accepts it there, holds header and body
to each other both ways — an activation record under any other class, or the
`Activation` header on any other record, is `ActivationHeader` — and
`read_referenced_builtin_class` refuses tag 23 everywhere else, as a value, a
superclass, a `<:` clause's builtin and behind `CLASS_METACLASS`, exactly as
it refuses `Top`'s 15. An activation reads other frames and unwinds stacks, a
capability Newspeak hands out only through mirrors, so no program may name its
class; the encoder never walks an activation object's header as a reference
(`GraphTable::visit_value`), so writing activations never trips the refusal of
a graph that reaches the class itself.

**The other classes the exceptions added are ordinary nameable builtins.**
`Exception`, `Error`, `MessageNotUnderstood`, `Message` and `Halt` carry
tags **24** to **28**, in that order, after `Activation`'s 23
([core-library.md](core-library.md), "Exceptions"). A program reaches them
through `platform kernel` and may subclass them, so an image may name any
of the five wherever it names a class: an exception a program made is an
ordinary instance record.

**Futures in a graph: a resolved future is its value, an unresolved one a
record under tag 30.** `Future` carries builtin tag **30**, the
kernel-private pull future of §6.3.2's simultaneous slots
([object-model.md](object-model.md), "Simultaneous slots"). No
program names it, so `read_referenced_builtin_class` refuses tag 30 wherever
the graph names a class — as a value, a superclass, a `<:` clause's builtin
and behind `CLASS_METACLASS` — and the encoder refuses a graph that reaches
the class or its metaclass (`encode::unnameable_class`), exactly as for
`Activation`'s 23. A future *object* is written two ways. A resolved one is
never a record: the graph walk and the value encoder both see through it
(`future::seen_through`), so every reference to it is written as a reference
to its value, and a load gives back one object where the program saw one —
the value a collection would have put in its place anyway
([memory.md](memory.md)). An unresolved one is an ordinary instance record
(`OBJECT_INSTANCE`) whose header is `CLASS_BUILTIN` followed by tag 30, the
one other header besides an activation record's that names a
kernel-private class; the walk does not visit that header as a reference, as
it does not an activation's. Its two fields are its closure and the state 0.
A future is *resolving* only while its block runs, and the image writer
refuses a world with a running activation, so no image meets one; the
encoder handles the case defensively all the same, writing it as
unresolved, its closure still in the first field
(`future::fields_for_an_image`), which is what an abandonment would make of
it — a load starts no block, so nothing could finish resolving it.

The decoder accepts tag 30 in an object's header alone
(`decode_object_header`) and holds the record to what the encoder writes: an
instance body of exactly two fields whose second is the state 0 and whose
first, once every record is read, is a closure. Anything else under that
header — another body, another field count, the state 1 or 2 or one that is
none of the three, a computation that is no closure — is `FutureRecord`,
refused rather than decoded, so no image hands the runtime a future whose
fields are no future's, and no resolved future stands apart from the value
every other reference was written as. Crafted code can still store into a
future's slots once it holds one -- a decoded frame may carry any receiver,
and `InitializeSlot` stores into `self` unchecked -- so
`Mutator::future_state` reads the fields as untrusted: a state that is none
of the three, an unresolved future without a closure, or a resolved one
whose value is a future is `FutureState::Malformed`, which forcing raises as
an ordinary error, the collector leaves in place, and the encoder refuses to
write (`forcing_a_future_with_corrupted_fields_is_an_error_not_a_panic`,
`tests/vm.rs`; `a_future_with_corrupted_fields_is_refused_by_the_encoder`,
`tests/image.rs`). A graph that decodes a future sets the
loading world's `futures_made`, as `Mutator::new_future` does, so the leaves
see through it and every check that stays off a program's paths until a
future exists runs in the loaded world
(`a_resolved_future_is_written_as_its_value`,
`an_unresolved_future_round_trips_and_computes_after_load`,
`a_future_record_that_is_not_an_unresolved_future_is_refused`,
`a_graph_naming_future_as_a_class_is_refused`, `tests/image.rs`;
`goldens/language/slot/simultaneous/spec_6_3_2_a_data_cycle_survives_an_image.ns`, whose build/load
round trip carries a data cycle through resolved futures).

**`LargeInteger` carries builtin tag 31**, the next free number. A
program never names the class -- `Object` declares no accessor for it --
but reaches it as an integer's class (`(9223372036854775807 + 1) class`)
and may keep it in a slot or subclass it, so an image may name tag 31
wherever it names a class. A `LargeInteger` itself is never a record
("A `LargeInteger` is a value on the wire" above); an instance of the class
that holds no digits, which only `LargeInteger new` makes, is an ordinary
instance record.

**Retired builtin class tags: 8, 10, 13, 18, 19 and 29.** 8 was `Character`;
10 was `OrderedCollection`, replaced by `platform collections List`, Newspeak
in the shipped `CollectionsFactory.ns`
([core-library.md](core-library.md), "The collections module (plan 2)");
13, 18, 19 and 29 were `Platform`, `Transcript`, `Collections` and
`KernelNamespace`, which moved into a shipped module, now `PlatformFactory.ns`
([core-library.md](core-library.md), "The platform"). An image names the
platform, its members and their classes by their platform paths instead
(`transcript`, `Transcript`, `kernel`, ...), and a stale image's reference
to a retired tag is refused as an unknown builtin
(`opcodes::RETIRED_BUILTIN_CLASS_TAGS`).

**A platform path is resolved before the graph, and only looked up while it
decodes.** `load` runs in three phases. The first decodes everything up to and
including the platform path section and roots every module's root class and levels.
The section's reader refuses what a loader cannot follow before anything is
sent: more than `MAX_PLATFORM_PATHS` paths (`TooManyPlatformPaths`), a path of
no members or more than `MAX_PLATFORM_PATH_LENGTH` (`PlatformPathLength`), a
member that is no symbol id (`BadSymbolId`) and one that is no unary selector
(`PlatformPathMemberNotUnary`). The second, outside any entry, sends each
path's members to the loading interpreter's platform with `Interpreter::send`.
Each step must be one `discover` could have taken — a public slot's getter, a
lazy slot's included, a public nested class's accessor, or a public class
accessor of the shipped instance reached so far, found by the same
enumeration of its members (`platform_paths::is_path_step`) — and is checked
before it is sent, then
sent as an ordinary public send. So a crafted `transcript newLine` is refused
before anything prints, rather than run 131,072 times while the image is
read. The image is refused, naming the path, when a step is none of these,
when a send fails, or when the final answer is no shipped object
(`platform_paths::is_shipped_object`). The third decodes the graph against the answers
(`DecodeContext::platform_paths`), so the decoder still runs no code; an index
past the section is `BadPlatformPath`, and a `CLASS_PLATFORM_PATH` whose
object is no class is `PlatformPathNotAClass`. A crafted image can make the
second phase read public slots, nested classes and class accessors of
shipped objects, and so run their lazy initializers, the class accessors'
one send and superclass clauses; it can run no other method. A crafted
`collections List` decodes: `List` is a class accessor step of the facet,
exactly `^outer CollectionsFactory List`. A crafted `collections
newCollections` is refused instead: `newCollections` is the collections
factory's own public member, never a step of the facet's shipped instance, so
no enumeration of the facet's members finds it.

**An instance of a path-named class carries exactly that class's layout.** The
code and the layout of a shipped class come from the loading binary, not from
the image, so an `OBJECT_INSTANCE` record whose header is a
`CLASS_PLATFORM_PATH` must carry exactly the class's `slot_count` fields, or
the image is refused (`PlatformInstanceFieldCount`).

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
application's root module: `--class` is a path rooted at that module's class
name, so the bodies of a project's other modules, the manifest's included,
are not reachable through `disasm` of the artifact (`inspect --full` shows
them).

`ns-next inspect <artifact>` prints the file itself. Its default is a table of
contents — header flags, section offsets, the symbol table, **the code table
with every body named**, the mixin table, and the module section, each
module's origin, whether it carries source and its root section — and
`--full` prints everything:
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
materializes objects and resolves class tags against live builtins, so it needs
an `Interpreter`; a viewer must not bootstrap one, any more than `javap`
instantiates a class to print it. `inspect` therefore walks the same bytes and
reports each record: an object's class tag, identity hash and slot values, a
frame's receiver, arguments, temporaries, `program_counter` and kind, and the
roots, and a started image's one root is the application. Nothing else shows
the graph. A module count that disagrees with the mixin table's trees is
refused, as `load` refuses it, rather than listed past: which tree a root
section is read against depends on the count, so a report past it would
describe modules that are not there.

Being a second reader over one layout is a drift risk, taken deliberately and
held by two things. The tags it interprets are the shared constants in
`opcodes.rs`, so it cannot disagree with the loader about what a byte *means*.
And the walk must consume the section exactly — its recorded end is the file's
last byte — so a disagreement about record *shape* desynchronizes it and fails
loudly instead of printing plausible nonsense. That guard is only as good as
its fixtures: the first version of the test passed with a field deliberately
dropped, because every fixture produced zero frames and the frame half was
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
platform and names no platform path.

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

`ns-next build <file.ns | project.json> <out> [--debug-source] [--lowering
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
- **A project file**: `ns-next run app.json [-- args]`, recognized by its
  `.json` extension. The root class's primary factory must be
  `packageUsing:`. Every module is parsed, resolved and compiled first; the
  root is sent `packageUsing: manifest`, then `main: platform args:`.

Either way the platform arrives only as `main:args:`'s first argument, and
the application's factory and initializers run with no platform in
existence.

**The project file** is JSON (the user's choice; it brings `serde_json` into
`ns-cli`, which `ns-lsp` already used):

```json
{
  "root": "Harness.ns",
  "modules": ["Sieve.ns", "../shared/RedBlackTree.ns"]
}
```

- `root` is the application's source file; `modules` the other modules the
  manifest holds. `modules` may be absent or empty. The root module is not in
  the manifest.
- A module's name in the manifest is the name its source file declares for
  its class, as in both upstream systems (Primordial Soup's
  `namespace at: klass name put: klass`; Squeak's `Root` namespace).
- The project file's own path, given to a command, is absolute or relative
  to the working directory. A path inside it is absolute or relative to the
  project file. Either separator, `/` or `\`, is accepted in every path,
  relative or absolute, on every operating system: the foreign one is
  rewritten to the host's and the platform's own path parsing does the rest,
  which keeps drive letters, UNC and verbatim prefixes intact
  (`resolve_written_path`).
- Refused before anything runs: an unknown field, a missing root, a missing
  or unreadable file, a module that does not parse or resolve, two modules
  declaring the same name, and a root whose factory is not `packageUsing:`.
  A module whose name is a public member of every object -- `Array`,
  `String` or any other predefined class `Object` has an accessor for -- is
  refused too, once the manifest exists and before the root is sent
  `packageUsing:`: the member would answer the send before the manifest's
  `doesNotUnderstand:` could (`Interpreter::responds_publicly`,
  `a_module_shadowed_by_an_object_member_is_refused`).
- A module may declare the root's name, or be named `Manifest`: the root is
  not in the manifest, and an image names a module by its index, never by
  its name, so neither collides with anything.

**Module order** is fixed and shared by everything that loads or writes
modules: the root module first, then the project file's `modules` in the
order written, then the embedded `Manifest.ns` (only for a project).
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
application yet, so none is named: the encoder is handed `nil` as the
platform, discovers no platform path, and a graph that reaches the platform
or anything shipped meets the encoder's refusal of a shipped object no path
names (`package_refuses_an_application_that_reaches_the_platform`,
`crates/ns-runtime/tests/image.rs`). An application that reached the
platform could only have been handed it, and nothing hands one out before
`main:args:`. `--debug-source` stores every module's source text. An error
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
`run`, `load` and `bench` alike -- supplies the platform and then the words
after `--` on the command line, each a `String`, in an `Array`
(`main_arguments`; `ns-next run app.ns -- a b`, `ns-next load app.img -- a
b`), empty when there are none. They are the CLI's, not the format's:
nothing in an image records them, so one artifact runs with whatever
arguments each `load` is given.

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
(`crates/ns-cli/tests/run.rs`, and `project.rs`'s own tests for the paths);
and mutation and truncation tests for the module section, the module class
and hash tags, `FLAG_START` and the retired tags and flag
(`crates/ns-runtime/tests/image.rs`).

## Planned: the image under the new representation

Would change: the decoder builds the planned representation -- one space of
variably sized objects ([object-model.md](object-model.md), "Planned: the object
layout"; [memory.md](memory.md), "Planned: the object space") -- from a wire
format that stays id-based, as the one-word value already is ("Integers and
floats on the wire" above). Nothing on the wire becomes an address or an offset,
so an artifact stays independent of word size and layout, as Primordial Soup's
snapshots are (`prior-art/primordialsoup/vm/snapshot.cc:398-470`); Spur's image,
which is old space's bytes relocated on load, is the model this does not take.

- **Integers and floats** keep their wire form. In the object space a
  decoded negative `LargeInteger` sets the header's negative bit, and a
  boxed `Float` is an object of a header, a class word and its `f64`.
- **Symbols** stay UTF-8 on the wire, as strings do; the decoder picks a
  symbol's one-byte or four-byte form from the code points it reads, as it
  already does a string's, and both become byte- or 32-bit-indexed objects
  in the space.
- **Frames become object records.** A reified frame is an object of the frame
  format ([object-model.md](object-model.md), "Planned: the object layout"), so
  its record joins the object table under a new object tag. The separate frame
  count and frame table go, and a frame id in a closure record or any other
  record becomes an object id. `OBJECT_ACTIVATION` (9), whose body is one
  frame id, retires into the new frame object tag, since a kernel-private
  `Activation` is then the frame object itself. The frame's kind stays a field
  of the frame object and keeps its `FRAME_*` kind tags. The encoder writes
  every frame's full register file, `nil` where no value was copied, since
  every reified frame has room for its registers, so a frame record's count
  is its size like any other record's and the register-less frame record of
  today goes.
- **The decoder learns each record's size before it allocates.** A placeholder
  per record, allocated before any record is read, assumes a fixed size; in a
  space of variably sized objects the first pass parses each record's body,
  whose field, element or unit count gives the object's size -- records carry
  no size field, and none is added -- and checks it against the bytes
  remaining, then allocates, and the second pass fills the fields, whose
  forward references then name objects that already exist.
- **The decode amplification is recomputed** as the largest of the ratios:
  per record kind, that kind's fixed object size against its minimum record
  -- a class or metaclass object, eight fields and its header and class word,
  80 bytes against the smallest metaclass record, 9 bytes (a class tag of 2,
  the hash's 4, `OBJECT_METACLASS` and a two-byte `CLASS_BUILTIN` reference
  for the class it describes, which `decode_class_tag` accepts and
  `pair_metaclasses` refuses only once every record is read:
  `image/encode.rs:1645-1648`, `image/decode.rs:3306-3311`), about 8.9x, and a
  frame object, about 96 bytes against a record of about 19, some 5x -- and
  per value, the words a decoded value occupies against the bytes that
  encode it -- a one-byte `VALUE_NIL` element becomes an 8-byte word, 8x,
  a UTF-8 byte widened to the 32-bit string form 4x, and a
  `VALUE_LARGE_INTEGER` of one digit, 14 bytes, becomes an object of a
  header, a class word and its digit. The class kind's
  8.9x is the largest, and the bound is whichever ratio is largest once the
  layouts are final, since the first pass allocates each object's full size
  before validation. A symbol record names a symbol-table entry rather than
  carrying its text, so the first pass refuses a second record naming the
  same entry, or one the heap's table already holds, before allocating, and
  allocates each entry's text at most once; that text is bounded by its own
  UTF-8 encoding in the symbol section -- at most 4x its encoded bytes in the
  32-bit form -- so the amplification's base is the graph section and the
  symbol section together, not the graph bytes remaining alone, which a
  valid image whose graph ends in one symbol record with a long name would
  otherwise fail ([memory.md](memory.md), "The interim cost, and the one
  question still open", where the figures are, and `CLAUDE.md`'s paragraph on
  untrusted images). `bounded_count` keeps bounding every pre-allocation by
  the bytes remaining.
- **Every load tenures before the decoder allocates.** Under generations
  ([memory.md](memory.md), "Planned: generations") the decoder allocates
  everything old, and every load runs `Interpreter::bootstrap()` first
  (`image/mod.rs:290`) and then rebuilds every module's root class
  (`modules::build_module_classes`, `image/mod.rs:305`), so every load will
  scavenge and tenure right after the bootstrap, before the decoder
  allocates anything; a load whose image names platform paths does it
  again after following them, before the graph pass. Each is a collection
  between entries, where only roots hold values, as `Interpreter::collect_now`
  is today, never one inside a step. The decoder's stores are then
  old-to-old and need no barrier.
- **Identity hashes** are preserved as they are today, per record and in the
  tag-named table; what changes is their width. The header word holds 22 bits
  ([object-model.md](object-model.md), "Planned: the object layout"), so the
  decoder refuses a hash of 2^22 or more where it refuses `u32::MAX` today.
  The counter is still advanced past the largest loaded hash, so a hash
  assigned after a load avoids every loaded one until the counter wraps;
  after a wrap, which skips 0, two objects may share a hash, as any hash
  allows, so "never collides" above holds only for today's width.

## Planned: suspend and resume

An image cannot be taken mid-execution, and the obstacle is control flow rather
than format. A reified `Frame` is fully encoded, every running activation is an
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
([execution.md](execution.md), "The depth limit"). Under the planned object
layout every reified frame will have room for its registers, since a frame
object will be allocated with its code's full register count
([object-model.md](object-model.md), "Planned: the object layout"). Suspending
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
because a hostile file cannot replace a builtin. **The decision is to stay with
the table style**, and to move the line one step toward the graph style later for
*user-defined* behaviour only, when edit-and-continue is built — never for the
builtins.

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
  which `ns-lsp` already depended on, so no new crate entered the lock file.
  Unknown fields are refused, so a misspelt key is a message rather than a
  module silently missing.
- **Module order is fixed: root, the project's modules as written, then
  `Manifest.ns`.** `run`, `build` and `load` compile or rebuild the modules
  in the same order, so every identity hash the bootstrap draws is the same
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
  into a different program.
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
- **An image records the hashes of the objects it names by tag**, so a hash the
  program was given survives a load like every other; a reserved-range
  rule was rejected.

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

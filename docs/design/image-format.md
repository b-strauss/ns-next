# Image format — serialization and deployment artifacts

> **Status:** built. Both artifacts round-trip, and both can be inspected without
> being loaded.
> **Code:** `crates/ns-runtime/src/image/` — `mod.rs`, `encode.rs`, `decode.rs`,
> `mixins.rs`, `symbols.rs`, `opcodes.rs`, `inspect.rs`. CLI: `ns-next snapshot`,
> `ns-next load`, `ns-next build`, `ns-next inspect`, and `ns-next disasm` on an
> artifact.

## Principles

**Not a memory dump.** The image is an explicit, self-describing,
architecture-neutral serialization decoded on load — never `mmap`ed VM memory,
which could not be cross-architecture. No native code, no JIT output, no raw
pointers, no baked-in tagging or host endianness.

**Canonical encodings.** Little-endian integers, IEEE-754 `binary64` for floats,
UTF-8 for strings and symbols, `SmallInteger` fixed at the `i64` range. Symbols are
re-interned on load rather than assumed to keep their ids.

**The bootstrap is the base image.** There is no pre-built base image file.
`run`, `snapshot` and `load` all bootstrap the core library and the `Object`
hierarchy from Rust primitives, and the image describes only what the user's
module added.

**An image is untrusted input.** No byte sequence may cause a panic or a process
abort. Every id, tag, count and length is validated before it indexes, allocates
or slices, and any recursion whose depth the input controls is bounded or made
iterative.

That guarantee is about **decoding**. `load` on a stateless binary instantiates
and sends the recorded entry with no flag at all, so starting one executes its
code with the same exposure as running the equivalent source. Recursion depth
is bounded by one counted limit, 100,000 activations, an ordinary Newspeak
error rather than a native stack overflow, since nothing re-enters the run loop
from Rust; and a crafted `SendInlinable` cannot use the count's one
exemption to recurse uncounted. Nor can it stack more exempt activations above
a counted one than source can: that number is bounded by how deeply blocks
nest, which the decoder caps at the parser's own ceiling ("Verification"
below); see [execution.md](execution.md), "The depth limit". A mutated
binary that decodes cleanly can still abort while executing — a native that
indexes an argument it was not given, for one: the control-flow natives read
`arguments[0]` unchecked, and the decoder does not check a send's argument count
against its selector — and that is not a decoder defect.

## Two artifacts

Following Bracha's position — "Ideally, you have images and a binary format …
what is important is that you do not have just an image"
([An Image Problem](https://gbracha.blogspot.com/2009/10/image-problem.html)) —
there are two:

| | **Image** | **Stateless binary** |
|---|---|---|
| Contains | code, structure, heap, reachable activations | code and structure only |
| Produced by | `snapshot`, after instantiation | `build`, before instantiation |
| Capabilities | already bound into slots | enter at start time |
| Is | a snapshot of a running world | a deployment artifact |

They are **one format**, distinguished by a header flag. The binary is the
image's leading sections stopping where the object graph would begin, plus a
recorded entry point.

A host holds what a load answers as roots: `image::load`'s `LoadedImage`
carries the root class and the image's roots as `Root`s
([memory.md](memory.md), "Host roots"). `image::load` decodes inside one
entry of the interpreter it bootstraps and stashes them once decoding and
every verification are done, before that entry ends — nothing runs while an
artifact decodes, so nothing moves under the decoder's handles.
`image::snapshot` takes a `&Mutator` and `image::build` a `&Heap`, with
values a host fetches from its roots in the same entry, for the same reason:
encoding runs no code.

## Layout

```
Image:   [header][source?][origin][symbols][code][mixins][root class][entry][graph]
Binary:  [header][source?][origin][symbols][code][mixins][root class][entry]
```

The header carries magic, `FORMAT_VERSION` and a flag byte. `read_header`
**refuses any flag this build does not implement** rather than masking it off: an
unimplemented flag means a section the reader cannot see, and continuing would
read the next field out of whatever that section occupies.

`FORMAT_VERSION` is 1 and is not bumped. The project has no external users, so
images predating a change are disposable and no migration path is owed to them.
The guarantee that still has to hold is the untrusted-input one, and that comes
from validating ids and lengths, not from a version field.

## What is *not* serialized

This is the part that decides how big an artifact is and how large its trusted
surface is.

- **Builtin classes, the root class and the levels the root's `<:` clauses
  add** are referenced by a **class tag**
  wherever a class object appears — an object's `.class`, a class-valued slot, a
  frame's `current_class` — never structurally encoded. On load a tag resolves to
  the live class the bootstrap produced. The bootstrap itself is the fixed
  root/special-objects table. So is the core library: a builtin class's
  methods are the loading process's own `Kernel.ns`, compiled at its
  bootstrap ([core-library.md](core-library.md)), and no image carries the
  kernel's code, its instance or the VM mirror — which is also why
  `Instruction::SendMirror`, emitted only into `Kernel.ns`, has no wire tag
  ([compiler.md](compiler.md), "Mirror sends").
- **`nil`, `true` and `false`** are unboxed `Value` scalars with no heap identity
  to preserve, encoded as scalar tags.
- **The `platform` singleton** gets its own tag, resolved to the loading
  interpreter's own singleton. Essentially every module stores it in a slot, so
  without this the whole platform graph would come along.
- **Mixin applications** are not serialized. Layout and lookup follow
  deterministically from a mixin and its superclass application, so they are
  rebuilt as class objects decode. This removes a section, a verification pass,
  and the possibility of loading an image whose stored layout disagrees with its
  mixins. It is also what proves two instances that shared an application before
  a snapshot still share one after it. A **metaclass** is the one record the
  rebuild does not read a superclass for: it is written as an ordinary class
  record whose superclass is `Object`'s class object, and rebuilding it from
  that link would chain its application and un-flatten it (§3.2). The decoder
  recognizes a record whose mixin is some declaration's class side and applies
  it with no superclass application, the shape `Mutator::metaclass_of` mints
  ([object-model.md](object-model.md), "Class sides and metaclasses").
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

**Symbol table**, at the head. Every selector and name once, referenced by index
everywhere else. Selectors are the format's most repeated strings — name-based
virtual dispatch puts the same selector at very many call sites — so this shrinks
the file and turns interning into one pass over the table instead of one
`Symbol::intern` per send instruction.

**Code table.** Every `CompiledCode` reachable from the module — each method,
each initializer, each superclass clause, every nested block body — in a flat
id-addressed table, blocks before the code that creates them so a `MakeClosure`
operand encodes as an already-decoded id. The image's bytecode is what runs after
a load; nothing recompiles. How each instruction is encoded is below.

## The instruction encoding

In memory an instruction is a Rust enum; on the wire it is **a tag byte followed
by a fixed operand layout** for that tag. Registers are `u16`, jump targets are
instruction indices, a send's call site is its `u32` number within its code
family, selectors and names are symbol-table ids, and a literal
carries its own tag byte from a second table. `LoadLiteral` is the only
instruction whose operands nest, and the nesting depth of a literal array is
input-controlled and therefore bounded.

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

- 0 `LoadLiteral`: loads a literal into a register; a string, symbol, character
  or array literal allocates a fresh object every time it runs.
- 1 `LoadSelf`: loads the receiver, `self`, into a register.
- 2 `LoadBuiltinClass`: loads the builtin class a bare name such as `Object`
  denotes when no enclosing declaration declares that name.
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
  own slots into `self`; emitted only in instance initializers.
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

The next free instruction number is 34. A send's `CallSite` -- its number
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
- 6 `Character`: a character literal.
- 7 `Array`: a literal array, `#( ... )`, whose elements are literals
  themselves.
- 8 `MalformedInteger`: an integer literal the lexer accepts and the parser
  rejects, out of range or a fractional radix; it raises its error when it runs.
- 9 `UnresolvedIdentifier`: an identifier that names nothing, such as a bare
  `super`; it raises its error when it runs.

**A call-site number is a property of the code, never a byte on the wire.**
It is feedback-table bookkeeping, not program meaning: the decoder assigns
every family's numbers itself, in the compiler's own emission order, once
the whole code table is read (`decode::number_call_site_families`), rather
than trusting a number the file claims. A code record carries one flag,
whether its sends were compiled uncached (the reference lowering never
numbers a send at all, so it marks the whole body rather than leaving each
send to say so), and one count, `copied_count`, so that a block's register
layout is known from its code alone. `outer_reach` is not on the wire either:
the decoder derives it with the compiler's own `bytecode::outer_reach_of`,
which works because a block's code precedes its parent's in the table. A
body's `family_size` is not on the wire either way; both
sides derive it from the sends' numbers and the blocks' sizes with the same
function (`bytecode::family_size_of`). A block precedes its parent in the
table, but its numbers depend on where its `MakeClosure` sits in its root's
emission order, so the decoder numbers a family only once its root -- the
body nothing else's `MakeClosure` creates -- has been read in full. The
golden image is 1243 bytes.

Both lists are declared once, in `image/opcodes.rs`, and used by the encoder and
the decoder alike rather than repeated as magic numbers on each side.
`crates/ns-runtime/tests/opcode_tags.rs` pins every value and checks that no two
share a number, so a renumbering fails the suite at the moment it happens instead
of surfacing later as an image that no longer means what it said.

The graph section's six tag families live there under the same rule: how a
`Value` is encoded, an object record's body, how a class record names its mixin,
how a class reference is named, what a frame was activated for, and which tag-named object an entry of the closing
hash table names (`HASHED_*`, below). They were bare literals in the encoder and
again in the decoder until `inspect` became a third reader of those bytes — and
three hand-kept copies of one numbering is how a viewer starts quietly
disagreeing with the loader. A closure frame (`FRAME_CLOSURE`) names the closure
object it runs by object id: the closure's data sits inline in its object, so
that the tracer can rewrite its two frame handles in place
([memory.md](memory.md), "What the collector traces").

**Mixin table.** One row per declaration: the name, its own slots, the primary
factory selector and arity, the compiled methods as selector → code id, the ids
of its nested declarations with their accessor names and accessibility, the
class-side mixin id, the initializer code id, and the provenance span. This is
what replaced the embedded source text and the re-parse on load.

A slot is its name, its mutability (0 immutable, 1 mutable) and its
accessibility (the access tags a method uses), because those three decide which
accessors it defines (§6.3.2). The accessors themselves are not written: the
decoder derives them from the slots, as building a mixin from source does. A row
in which two members would share a name — a compiled method, a slot's getter, a
mutable slot's setter, a nested declaration — is refused before any mixin is
built, so a crafted row cannot make one member silently hide another.

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

**Root class.** The class the root inherits from, as an optional builtin tag,
then the root's own `<:` levels, lowest first, each as its clause's position in
row 0 and the builtin that clause named. The root rule lets a root inherit
only `Object` or `Value` and apply only `Value` over `Object`
([object-model.md](object-model.md), "Mixin application", "The root rule"), so
builtin tags are enough, and there is at most one level; a root the rule
refuses is never written ("Verification" below). `load` rebuilds the level
with the root class, before the graph, so the graph names it by its position
(`CLASS_ROOT_LEVEL`, class tag 3) rather than as a record.

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
builtin class, the module's root class and the `platform` singleton
(`encode::is_class_tag`, and the platform check beside it in
`GraphTable::visit_value`). None of them is a record, so nothing about a record
could carry its hash. Instead the graph section closes, after its roots, with a
table of exactly the tag-named objects whose stored hash is not 0 — the builtin
classes in wire-tag order, then the root class, then `platform` — each entry a
tag from the `HASHED_*` family in `opcodes.rs` naming the object, and its hash
(`encode::encode_tag_named_hashes`). `load` restores each hash onto the object
the bootstrap built (`decode::decode_tag_named_hashes`), and refuses a 0, a
duplicate entry or `u32::MAX` like any other malformed hash. The
identity-hash counter is then advanced past the larger of this table's maximum
and the object table's, so a hash assigned afterward never collides with either.
`inspect` reads the table as part of the graph section
(`Report::graph_tag_named_hashes`).

**Gap: a root class's `<:` levels are tag-named but have no entry in that
table**, so a hash one of them was given would not survive a snapshot: the
decoder rebuilds the levels with the root class and they draw fresh hashes
when asked. It is not observable today. No program can obtain a root level as
a value — there is no reflective `superclass` — and the one thing that can name
one in an image is a frame's `current_class`, when a method of the builtin the
clause named, a kernel method, runs on an instance of the root class; that is
why `CLASS_ROOT_LEVEL` exists rather than a refusal. A `HASHED_*` tag naming a
root level by position would close the gap once something can reach one.

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
`a_hash_given_to_a_tag_named_object_survives_a_snapshot` in
`crates/ns-cli/tests/snapshot_load.rs` pins the result end to end.

## How frames actually get into an image

Only through a closure. A running activation lives on the activation stack and
is not in the object graph at all; a frame exists for it only once a block
captured it ([execution.md](execution.md), "Activations"). So a frame enters the
table only as a closure's `captured_frame` — a closure a value names, or the
one a closure frame already in the table runs — or as the `home` of a frame
already in it. (A closure has no `home` field of its own to bring one in:
`Heap::closure_home` derives it from the captured frame.) A snapshot is taken
at a **quiesced** point —
after instantiation, optionally after an entry send, with the Rust call stack
already unwound and the activation stack empty; `image::snapshot` refuses to
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

An **optional source blob** remains behind a header flag —
`snapshot --debug-source` writes it, `load --print-source` shows it. It is never
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
never a panic.

**`Instruction::InitializeSlot`'s `own_index` is not one of the counts these passes
bound.** `verify_mixins` checks a body's kind and arity against its
declaration, but nothing checks that a slot-store instruction's index falls
within the mixin's own slot count. This is harmless at run time:
`Heap::set_instance_field` is total, so an out-of-range index writes nothing,
and an in-range but wrong one writes a different slot of the instance being
initialized — silently wrong output, never a panic. That is acceptable for
the reason `CLAUDE.md`'s "An image is untrusted input" gives: loading a
stateless binary already runs its own code, with the same exposure as running
the equivalent source, so a wrong slot from a malformed index is no worse
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
and a position that must exist (`BadMixinId`, `BadMixinApplicationClause`), and the
root-class section lists the root's levels at rising positions within row 0's
clauses, which also bounds their count before anything is reserved for it.
Levels whose superclass links close on themselves have no order to be rebuilt
in and are refused like any other class records that do (`SuperclassCycle`).
What is not checked is that a level sits directly above a class of its applying
row; the instantiation walk re-checks a clause body's arity against the
arguments it is given and raises a Newspeak error rather than indexing past
them.

**A root the root rule forbids is refused on both sides.** The encoder refuses
a module whose root mixin carries a root defect, at `build` and `snapshot`
alike, with "image encoding cannot serialize `M`: a module's root class may
inherit only Object or Value, or write Object <: Value, and …" — the reason
the instantiation refusal gives: such a root can never be instantiated, so an
artifact of it could only fail, and refusing it needs no wire field. The
root-section reader (`decode_root_section`) refuses what the encoder cannot
write: a superclass tag other than `Object`'s or `Value`'s, and a level naming
anything but `Value`, as `InvalidTag` ("invalid builtin class a module's root
may inherit tag *n*", "… may apply tag *n*"); a second level, or a level over
anything but `Object`, as `BadMixinApplicationClause`. So a crafted artifact
cannot bring back a root the rule refuses, and no decoded mixin carries a
root defect.

**A decoded value class is re-decided; a decoded instance is not re-checked.**
§3.1.1's two conditions on a value class — no `==`, every enclosing object a
value — are a verdict on the class object, not a wire field, so
`rebuild_applications` recomputes it with `check_value_class` for every
decoded class whose application inherits `Value`, once every decoded class
has its real application ([object-model.md](object-model.md), "A value
object's four conditions"): a snapshot of a refused value class decodes
refused. A ready-made *instance*, by contrast, is rebuilt from its record
without its slots passing through `InitializeSlot`, so a crafted image can
hand back a value whose slot holds an `Array` — a wrong answer, never a crash
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
`Kernel.ns` alone ([core-library.md](core-library.md), "The VM mirror is the
whole Rust boundary"). Its class has builtin class tag **14**
(`BuiltinClass::VmMirror`, appended to `BuiltinClass::ALL` after `Platform`'s
13), which `builtin_class_from_tag` knows so that the closing table of
tag-named hashes can name it as it names every builtin. **Everywhere a
program names a class — a `LoadBuiltinClass`, a class reference in the graph,
a root class's superclass, the builtin a `<:` clause named — the decoder reads the tag through
`read_program_builtin_class`, which refuses 14** as an invalid tag: an
artifact that could name the mirror's class could instantiate a second mirror
or reach the one there is, and so claim every primitive, which no source
module can (`a_load_builtin_class_naming_the_mirror_is_refused` in
`crates/ns-runtime/tests/image.rs`). The encoder refuses the other direction:
an image whose graph reaches the mirror object is not written at all, since
it would hand the capability to whatever loaded it
(`a_snapshot_that_reaches_the_mirror_is_refused`, `interpreter/mirror.rs`).
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
`read_program_builtin_class` refuses tag 15 exactly as it refuses 14, so no
artifact can name what no source can
(`a_load_builtin_class_naming_top_is_refused`,
`crates/ns-runtime/tests/image.rs`). An artifact naming `Top` would
otherwise build a class outside §3.2's hierarchy that inherits nothing at
all, not even `doesNotUnderstand:`.

**Trailing bytes are refused, not ignored.** `load` checks that the byte stream
is exhausted once the last section has been read and returns
`DecodeError::TrailingBytes` if not. With two artifact shapes sharing one
format, an unchecked tail would let a binary with a graph section appended — or
a truncated, then padded, image — decode as something other than what it is;
see `a_binary_with_a_graph_appended_is_refused` in `crates/ns-runtime/tests/image.rs`.

## Reading an artifact

`disasm` accepts an artifact as well as a `.ns` file: the same decoder that
`load` uses rebuilds the mixin tree with each body attached to its declaration,
and `disasm` formats a decoded body's `Instruction`s exactly as it does a freshly
compiled one's. Like every load, it refuses an artifact `verify_mixins` finds
inconsistent before printing anything.

`ns-next inspect <artifact>` prints the file itself. Its default is a table of
contents — header flags, section offsets, the symbol table, **the code table
with every body named**, and the mixin table — and `--full` prints everything:
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
frame's receiver, arguments, temporaries, `program_counter` and kind, and the roots. That is
the half an image has and a binary does not, and nothing else showed it.

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

`load --print-source` still only returns the debug variant's `.ns` text; for
anything else, `inspect` is the tool.

This exists because the stateless binary's whole purpose is to ship **without**
source: the one tool that prints bytecode would otherwise require the thing the
artifact was built to do without. It also paid for itself in the binary's own
tests: when the prefix-equality check between `build` and `snapshot` fails, a
differing byte offset says nothing on its own, and a structural diff says
exactly what moved; and the claim that the platform is cut out of an artifact,
previously verifiable only by reading the encoder, is now verifiable against
the artifact itself.

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

## The stateless binary

`ns-next build <file.ns> <out> [--entry SEL] [--debug-source]` runs the pipeline
up to compiling the code onto the declarations — parse, resolve, bootstrap,
compile — and then **does not instantiate**. That omission is the point: the
artifact never sees a platform.

**Format delta.** A second header flag, `FLAG_STATELESS`, and an entry-selector
field encoded as a symbol id, bounds-checked like every other id. The field sits
at the end of the stateless run rather than in the header, because it references
the symbol table and because down there it does not disturb the equality of the
describing sections. It is optional in an image and mandatory in a binary — which
is what separates a startable artifact from a snapshot.

**The consistency guarantee, stated precisely:** `build` and `snapshot` share
their section writers, which makes the section *order* and the *encoding logic*
for code, mixins and the root class agree by construction rather than by
discipline. Not "the binary is a prefix of the image", which would be false the
moment one records an entry and the other does not.

What that sharing does *not* cover is the symbol table's contents: the table
stays open past the shared sections so each caller can still extend it —
`build` interns the entry selector, `snapshot`'s object graph interns whatever
its live objects name, including a `Symbol` minted at run time by `asSymbol`
with no source occurrence at all. So for the same source file and the same
`--debug-source` setting, the bytes from `origin` through `root class` are
identical in a `build` artifact and a `snapshot` artifact only when both hold:

(a) the entry selector is one the code or mixin tables already interned
(equivalently, one the root class's own mixin chain carries as a method or
as a nested declaration — see "Choosing the entry" below); and
(b) the snapshotted graph interns no string absent from those same tables.

(a) is enforced by the build-time entry validation described below under
"Choosing the entry": a selector that survives it was necessarily interned
while encoding the declaration that defines it. That enforcement is sound
only while every
ancestor mixin in a root class's superclass chain is an empty builtin mixin
(no cross-module inheritance exists yet); cross-module inheritance would need
this validation revisited. (b) is not enforced anywhere, is expected to fail
for an arbitrary program, and is not something this format tries to guarantee
— a `snapshot` of a world holding a run-time-minted symbol simply disagrees
with a `build` of the same source from wherever the symbol table diverges
onward. Neither condition affects `build`'s or `snapshot`'s own correctness;
only whether the two agree byte-for-byte on top of it.

**Choosing the entry.** If the root class declares `main:args:` that is the
default, which makes upstream's convention a default rather than a rule.
Otherwise `--entry` is required. `build` refuses an arity other than 0, 1 or 2,
and a selector the root class's own mixin chain carries as neither a method
nor a nested declaration, rather than deferring either to start-up. This is
narrower than what a real send would find at start-up: a primitive, though an
entry of a mixin's own `methods` table like any other
([dispatch.md](dispatch.md), "Primitives"), is never a selector
`mixins::encode` interns — the builtin mixins that carry primitives are never
serialized — and `doesNotUnderstand:` answers a send `build`'s check found
nothing for at all. On purpose: see `root_class_answers` in
`crates/ns-cli/src/commands/build.rs` for why only this narrower set keeps the
byte-identity guarantee above sound.

**Deviation: `args:` receives `nil`.** §8.4 gives `main:args:` "a platform
object (8.3) as its first parameter, and a list of any environment-specific
(e.g., command line) arguments as its second". `crates/ns-cli/src/entry.rs`'s
`entry_arguments` — the one place any command that sends an entry builds its
arguments — supplies the platform and then `Value::Nil`, because there is no
command-line-argument passthrough into Newspeak. A program that inspects `args`
sees `nil` where the specification promises an empty list. The deviation is the
CLI's, not the format's: the encoded entry is a selector, and nothing about it
records what the arguments are.

**Loading.** `load` reads the header and branches. A stateful image is unchanged.
A stateless binary decodes symbols → code → mixins → root class → entry id, with
no graph section, then verifies the decoded declarations against their code and
hands back empty roots with the entry as an option. The caller instantiates the
root class with the freshly bootstrapped platform and sends the entry, so the
primary factory and every initializer run **inside the loaded world** against
the artifact's own code. Capabilities enter at start time.

Instantiation stays in the CLI rather than in `image::load`: deciding *that* a
command-line entry point supplies the platform is a CLI concern, because nothing
in the runtime needs to know it was invoked from a terminal.

**Tests**, in order of importance: `build` then `load` reproduces
`run`'s stdout, stderr and exit status byte for byte
(`every_golden_runs_identically_from_source_and_from_a_binary`); `build` and
`snapshot` agree on the describing sections' bytes
(`a_build_and_a_snapshot_agree_on_the_describing_sections`); `run-corpus`
carries a third lane that builds and loads each whitelisted entry, where a
row's `send` is the build's `--entry`; and mutation and truncation tests cover
the new flag, the entry field and the trailing-byte check
(`crates/ns-runtime/tests/image.rs`).

## Planned: suspend and resume

An image cannot be taken mid-execution, and the obstacle is control flow rather
than format. A reified `Frame` is fully encoded, every running activation is an
entry on the explicit activation stack ([execution.md](execution.md),
"Activations"), and nothing a running computation does holds a Rust frame: the
activation stack is its only copy, since sends do not recurse through the Rust
call stack ([execution.md](execution.md), "The trampoline") and nothing
re-enters the run loop from Rust ([execution.md](execution.md), "No re-entry
from Rust"). What is missing is a yield path out of `vm::run`, a re-entry
point that would continue a decoded frame at its `program_counter`, and an encoding for
what each stack entry's `Continuation` carries in Rust: a native loop's
`WhileLoop` or `DoLoop`, a nested class's `NestedClassCreation`, an
instantiation's `Instantiation`. An image records none of them today, and a
suspended activation without its continuation would resume without the rest
of the loop, the class creation or the walk it belongs to.
Suspending then means reifying and divorcing the whole stack, which is the same
operation a debugger performs on the frames it touches — and the same operation upstream
performs, since Cog flushes its stack zone to contexts (`divorceAllFrames`)
before taking a snapshot. An activation's `program_counter` being stored only at sends,
`MakeClosure` and `NonLocalReturn` is *not* an obstacle: those are exactly the
safepoints, and a suspension could only happen at a send boundary.

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
the VM needs is reconstructible rather than serialized. That is also why the
entry selector goes in the file — there is no store to put it in.

The trade is real in both directions. Upstream can ship a modified kernel, can
round-trip behaviour created at run time because methods are ordinary graph
edges, and reuses one format for isolate messages. We get artifacts in kilobytes
rather than a ~1.27 MB floor, and hold the untrusted-input invariant cheaply
because a hostile file cannot replace a builtin. **The decision is to stay with
the table style**, and to move the line one step toward the graph style later for
*user-defined* behaviour only, when edit-and-continue is built — never for the
builtins.

## Decisions

- **One format, two artifacts, distinguished by a header flag** — not two magics
  and two decoders, which would each have to hold the untrusted-input guarantee
  independently and would drift.
- **The binary records its entry point.** A deployment artifact that cannot say
  how it starts is not one.
- **`main:args:` is the default, not the rule.** Made mandatory it would cost the
  entire `run-corpus` gate: none of the six modules that run declares it.
- **`build` is a new verb; `load` is not.** Producing the two artifacts are
  different acts; consuming them is the same act, differing only in whether
  instantiation precedes the entry send, which the header says.
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
- **`FORMAT_VERSION` is not bumped**, because there are no external users and the
  safety property does not depend on it.
- **Artifacts are readable but not writable as text.** A dump serves inspection;
  a loadable text format would be a second codec to keep in step, and the need
  that would justify it is already met by corrupting real images.
- **An image records the hashes of the objects it names by tag**, so a hash the
  program was given survives a snapshot like every other; a reserved-range
  rule was rejected.

## Open questions

- **The decompiler** that is to provide export back to `.ns`. Mitigated by the
  debug source blob, unwritten.
- **Whether upstream strips source at packaging time.** A measurement of four
  `.vfuel` artifacts concluded that nothing is stripped, ~44% long text runs in
  each. `CompilerApp.ns`'s `keepSource: (appName = 'TestRunner')` shows a switch
  that is false for every ordinary application, which sits in tension with that.
  Both can be true — different toolchain, or the measured runs were selectors
  rather than source — but the claim should be re-checked rather than left
  standing.
- **Whether an image should record its entry too**, making `load` of a stateful
  image self-starting, at the cost of the two artifacts' bytes differing in one
  more place.
- **What a multi-module artifact would look like.** Upstream's manifest lets one
  artifact carry a linked set. One root module per artifact is assumed
  throughout, and nothing in the format anticipates more.

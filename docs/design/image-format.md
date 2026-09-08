# Image format — serialization and deployment artifacts

> **Status:** partial. The stateful image is built and round-trips; the stateless
> binary is designed and not built.
> **Code:** `crates/ns-runtime/src/image/` — `mod.rs`, `encode.rs`, `decode.rs`,
> `mixins.rs`, `symbols.rs`. CLI: `ns-next snapshot`, `ns-next load`.

## Principles

**Not a memory dump.** The image is an explicit, self-describing,
architecture-neutral serialization decoded on load — never `mmap`ed VM memory,
which could not be cross-architecture. No native code, no JIT output, no raw
pointers, no baked-in tagging or host endianness.

**Canonical encodings.** Little-endian integers, IEEE-754 `binary64` for floats,
UTF-8 for strings and symbols, `SmallInt` fixed at the `i64` range. Symbols are
re-interned on load rather than assumed to keep their ids.

**The bootstrap is the base image.** There is no pre-built base image file.
`run`, `snapshot` and `load` all bootstrap the core library and the `Object`
hierarchy from Rust primitives, and the image describes only what the user's
module added.

**An image is untrusted input.** No byte sequence may cause a panic or a process
abort. Every id, tag, count and length is validated before it indexes, allocates
or slices, and any recursion whose depth the input controls is bounded or made
iterative.

## Two artifacts

Following Bracha's position — "Ideally, you have images and a binary format …
what is important is that you do not have just an image"
([An Image Problem](https://gbracha.blogspot.com/2009/10/image-problem.html)) —
there are two:

| | **Image** (built) | **Stateless binary** (planned) |
|---|---|---|
| Contains | code, structure, heap, reachable activations | code and structure only |
| Produced by | `snapshot`, after instantiation | `build`, before instantiation |
| Capabilities | already bound into slots | enter at start time |
| Is | a snapshot of a running world | a deployment artifact |

They are **one format**, distinguished by a header flag. The binary is the
image's leading sections stopping where the object graph would begin, plus a
recorded entry point.

## Layout

```
Image:   [header][source?][origin][symbols][code][mixins][root class][entry?][graph]
Binary:  [header][source?][origin][symbols][code][mixins][root class][entry ]
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

- **Builtin classes and the root class** are referenced by a **class tag**
  wherever a class object appears — an object's `.class`, a class-valued slot, a
  frame's `current_class` — never structurally encoded. On load a tag resolves to
  the live class the bootstrap produced. The bootstrap itself is the fixed
  root/special-objects table.
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
  a snapshot still share one after it.
- **The bindings map.** A binding is resolved by span only at compile time and in
  the tree-walker, neither of which a loaded world reaches, so `load` allocates
  one empty map for the signatures that still take one. Reconstructing a real map
  would require re-resolving the source, which would require re-parsing it —
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
instruction indices, selectors and names are symbol-table ids, and a literal
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

| Instructions | | | |
|---|---|---|---|
| 0 `LoadLiteral` | 5 `LoadOuterLocal` | 10 `MakeClosure` | 15 `NonLocalReturn` |
| 1 `LoadSelf` | 6 `StoreOuterLocal` | 11 `MakeArray` | 16 `Jump` |
| 2 `LoadBuiltinClass` | 7 `SendLexical` | 12 `InitSlot` | 17 `JumpIfNotBoolean` |
| 3 `Move` | 8 `SendPublic` | 13 `Return` | 18 `BranchIfFalse` |
| 4 `StoreLocal` | 9 `SendSuper` | 14 `ImplicitReturn` | 19 `BranchIfTrue` |

| Literals | | |
|---|---|---|
| 0 `Nil` | 4 `Str` | 8 `MalformedInt` |
| 1 `Bool` | 5 `Sym` | 9 `UnresolvedIdent` |
| 2 `SmallInt` | 6 `Char` | |
| 3 `Float` | 7 `Array` | |

Both tables are declared once, in `image/opcodes.rs`, and used by the encoder and
the decoder alike rather than repeated as magic numbers on each side.
`crates/ns-runtime/tests/opcode_tags.rs` pins every value and checks that no two
share a number, so a renumbering fails the suite at the moment it happens instead
of surfacing later as an image that no longer means what it said.

**Mixin table.** One row per declaration: the name, its own slot names, the
primary factory selector and arity, the method table as selector → code id, the
ids of its nested declarations with their accessor names and accessibility, the
class-side mixin id, the initializer code id, and the provenance span. This is
what replaced the embedded source text and the re-parse on load.

**Class objects are ordinary objects in the graph**, encoded as
`{mixin id, superclass, enclosing object, declaring class}`. The declaring class
has to be there because `outer` walks the declaration chain rather than the
receiver's dynamic one, so an image that dropped it would come back resolving
`outer` from the wrong chain.

**Object table.** Every heap object reachable from the roots — instances,
strings, symbols, arrays, closures — decoded in two passes (placeholder, then
fill) so cycles and shared references round-trip with identity preserved. A
closure encodes its parameters, its compiled body as a code id, and its captured
frame and home as frame ids. A closure carrying an AST body is a snapshot error
rather than a silent omission; `snapshot` always runs under the bytecode engine,
so none can exist in a world it snapshots.

**Frame table.** Every `Frame` reachable from the roots, with receiver, args,
temps, `pc`, `sender`, `home`, `current_class`, `kind` and `uncontinuable`.
Frames and objects reference each other freely, so both tables are
placeholder-allocated before either is filled.

**Identity hashes are preserved verbatim.** After a load the allocation counter
is advanced past the largest hash the image contained, so newly allocated objects
never collide with loaded ones.

## How frames actually get into an image

Only through reachability. A closure stored in a slot holds its defining
activation through `captured_frame`/`home`, and once a frame is in the table its
`sender` chain follows. A snapshot is taken at a **quiesced** point — after
instantiation, optionally after an entry send, with the Rust call stack already
unwound — so the table never holds a suspended computation.

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
`verify_code_table` between decoding and installing the code table, and
`verify_graph` once the module exists to resolve each frame's body against. A
corrupt but version-matching image is an error there, never a panic in the VM
later.

The mixin tree's nesting depth is input-controlled, so building it is iterative —
the "bound any recursion whose depth the input controls" case. Symbol ids, mixin
ids, class-object references, slot counts and arities are all validated before
first use.

**Known gap:** `load` does not check that the byte stream is exhausted when the
last section has been read. `Reader::remaining()` exists but serves only as an
allocation bound, so trailing bytes are silently ignored. With two artifact
shapes that becomes a confusion vector, and the stateless-binary work fixes it.

## Planned: reading an artifact

**You cannot presently look inside an image at all.** `disasm` takes a `.ns`
file, compiles it, and prints the result; it cannot disassemble the bytecode that
is actually in an artifact. `load --print-source` returns the debug variant's
`.ns` text, not the file's structure. For anything else you have bytes.

That asymmetry gets worse with the stateless binary, whose entire purpose is to be
shipped **without** source: the one tool that prints bytecode would require the
thing the artifact was built to do without.

Two additions, both wiring rather than new mechanism, and **this is the first
piece of work**, ahead of the binary itself:

- `disasm` accepts an artifact as well as a `.ns` file. The decoder already
  produces a `CompiledModule` and `disasm` already formats `Instr`.
- A structural listing: header flags, sections with their offsets, the symbol
  table, the mixin table, class objects, and object and frame counts.

It comes first because the binary's own tests need it. When the prefix-equality
check fails, a differing byte offset says nothing and a structural diff says
exactly what moved. And the claim that the platform is cut out of an artifact is
today verifiable only by reading the encoder; with a listing it is verifiable
against the artifact.

**One-way only. There will be no text format that can be loaded back.** That
would be a second encoder and decoder pair to keep in step with the binary one —
the drift shape this project has already paid for twice, in two engines that must
agree and in two copies of the tag table. The argument that would justify it is
hand-authoring test images, and that need is already met differently: the
untrusted-input tests take real images and corrupt them at located offsets, which
exercises real structures rather than the ones someone thought to write out.

**This is not the decompiler**, and the two are easy to confuse:

| | Produces | Lets a reader understand |
|---|---|---|
| Dump | structure as text | the *file* |
| Decompiler | `.ns` source | the *program* |

Part of why "export back to `.ns`" looks large is that the easy half gets counted
into it. It is not in it.

## Planned: the stateless binary

`ns-next build <file.ns> <out> [--entry SEL] [--debug-source]` runs the pipeline
up to engine installation — parse, resolve, bootstrap, compile — and then **does
not instantiate**. That omission is the point: the artifact never sees a
platform.

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
(equivalently, one the root application's lookup already finds — see
"Choosing the entry" below); and
(b) the snapshotted graph interns no string absent from those same tables.

(a) is enforced, by the build-time entry validation described below under
"Choosing the entry" — a later task adds it to `ns-next build`, and once it
does, a selector that survives it was necessarily interned while encoding the
declaration that defines it. That enforcement is sound only while every
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
and a selector the root application's lookup cannot find, rather than deferring
either to start-up.

**Loading.** `load` reads the header and branches. A stateful image is unchanged.
A stateless binary decodes symbols → code → mixins → root class → entry id, with
no graph section, then installs the bytecode engine, seals the world, and hands
back empty roots with the entry as an option. The caller instantiates the root
class with the freshly bootstrapped platform and sends the entry, so the primary
factory and every initializer run **inside the loaded world** against the
artifact's own code. Capabilities enter at start time.

Instantiation stays in the CLI rather than in `image::load`: deciding *that* a
command-line entry point supplies the platform is a CLI concern, because nothing
in the runtime needs to know it was invoked from a terminal.

**Tests**, in order of importance: `build` then `load` must be byte-identical to
`run --engine=bytecode` in stdout, stderr and exit status; prefix equality
between `build` and `snapshot`; a binary lane in `run-corpus` over the same
entries, where a row's `send` is the build's `--entry`; and mutation and
truncation tests over the new flag, the entry field and the trailing-byte check.

## Planned: suspend and resume

An image cannot be taken mid-execution, and the obstacle is control flow rather
than format. `Frame` is heap-allocated, chained and fully encoded, but sends
recurse through the Rust call stack, so the activation chain exists twice —
reified on the heap and implicitly on the Rust stack. Only the first can be
serialized; only the second can be resumed. There is no yield path out of
`run_code` and no re-entry point that would continue a decoded frame at its `pc`.

The fix is the trampoline described in [execution.md](execution.md), where the
moving collector needs it anyway. `frame.pc` being stored only at sends,
`MakeClosure` and `NonLocalReturn` is *not* an obstacle: those are exactly the
safepoints, and a suspension could only happen at a send boundary.

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
- **`FORMAT_VERSION` is not bumped**, because there are no external users and the
  safety property does not depend on it.
- **Artifacts are readable but not writable as text.** A dump serves inspection;
  a loadable text format would be a second codec to keep in step, and the need
  that would justify it is already met by corrupting real images.

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

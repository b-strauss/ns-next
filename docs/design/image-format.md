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

That guarantee is about **decoding**. `load` on a stateless binary instantiates
and sends the recorded entry with no flag at all, so starting one executes its
code with the same exposure as running the equivalent source -- recursion depth
bounded only by the native stack until the VM is trampolined; see
[execution.md](execution.md), "Planned: the trampoline".

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

The graph section's four tag families live there under the same rule: how a
`Value` is encoded, an object record's body, how a class reference is named, and
what a frame was activated for. They were bare literals in the encoder and again
in the decoder until `inspect` became a third reader of those bytes — and three
hand-kept copies of one numbering is how a viewer starts quietly disagreeing with
the loader. A fifth family is *retired*: a frame record once carried a byte
naming the unit its `pc` counted in, and only instruction indices were ever
representable, so the byte was removed. Its two numbers, 0 and 1 (`PC_KIND_*`),
stay assigned and pinned, so a unit field that ever returns cannot give them a
new meaning.

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
frame and home as frame ids.

**Frame table.** Every `Frame` reachable from the roots, with receiver, args,
temps, `pc` — an instruction index into the frame's compiled body — `sender`,
`home`, `current_class`, `kind` and `uncontinuable`.
Frames and objects reference each other freely, so both tables are
placeholder-allocated before either is filled.

**Identity hashes are preserved verbatim.** After a load the allocation counter
is advanced past the largest hash the image contained, so newly allocated objects
never collide with loaded ones.

## How frames actually get into an image

Only through a closure. A running activation lives on the activation stack and
is not in the object graph at all; a frame exists for it only once a block
captured it ([execution.md](execution.md), "Activations"). So a frame enters the
table only as a closure's `captured_frame` or `home`, or as the `captured_frame`
or `home` of a frame already in it. A snapshot is taken at a **quiesced** point —
after instantiation, optionally after an entry send, with the Rust call stack
already unwound and the activation stack empty; `image::snapshot` refuses to
encode otherwise — so every frame in it is dead:
divorced when its activation returned, or decoded from an earlier image and never
run. A frame the runtime reified carries its own registers and has no sender, so
a closure brings in the frames it names and nothing of the call chain that
created them, and the table never holds a suspended computation.

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

The mixin tree's nesting depth is input-controlled, so building it is iterative —
the "bound any recursion whose depth the input controls" case. Symbol ids, mixin
ids, class-object references, slot counts and arities are all validated before
first use.

**Trailing bytes are refused, not ignored.** `load` checks that the byte stream
is exhausted once the last section has been read and returns
`DecodeError::TrailingBytes` if not. With two artifact shapes sharing one
format, an unchecked tail would let a binary with a graph section appended — or
a truncated, then padded, image — decode as something other than what it is;
see `a_binary_with_a_graph_appended_is_refused` in `crates/ns-runtime/tests/image.rs`.

## Reading an artifact

`disasm` accepts an artifact as well as a `.ns` file: the same decoder that
`load` uses rebuilds the mixin tree with each body attached to its declaration,
and `disasm` formats a decoded body's `Instr`s exactly as it does a freshly
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
frame's receiver, arguments, temporaries, `pc` and kind, and the roots. That is
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
narrower than what a real send would find at start-up — `doesNotUnderstand:`
and every native method live outside a mixin's own `methods` table — on
purpose: see `root_class_answers` in `crates/ns-cli/src/main.rs` for why only
this narrower set keeps the byte-identity guarantee above sound.

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
(`every_fixture_runs_identically_from_source_and_from_a_binary`); `build` and
`snapshot` agree on the describing sections' bytes
(`a_build_and_a_snapshot_agree_on_the_describing_sections`); `run-corpus`
carries a third lane that builds and loads each whitelisted entry, where a
row's `send` is the build's `--entry`; and mutation and truncation tests cover
the new flag, the entry field and the trailing-byte check
(`crates/ns-runtime/tests/image.rs`).

## Planned: suspend and resume

An image cannot be taken mid-execution, and the obstacle is control flow rather
than format. A reified `Frame` is fully encoded, and every running activation is
an entry on the explicit activation stack ([execution.md](execution.md),
"Activations"), but sends still recurse through the Rust call stack, so the chain
of running activations exists twice — on the activation stack and implicitly on
the Rust stack. Only the first can be serialized, by reifying it; only the
second can be resumed. There is no yield path out of `run_code` and no re-entry
point that would continue a decoded frame at its `pc`.

The fix is the trampoline ([execution.md](execution.md), "Planned: the
trampoline"), which makes the activation stack the only copy. Suspending then
means reifying and divorcing the whole stack, which is the same operation a
debugger performs on the frames it touches — and the same operation upstream
performs, since Cog flushes its stack zone to contexts (`divorceAllFrames`)
before taking a snapshot. An activation's `pc` being stored only at sends,
`MakeClosure` and `NonLocalReturn` is *not* an obstacle: those are exactly the
safepoints, and a suspension could only happen at a send boundary.

Whoever builds this must first close a decoder gap that today is inert only
because nothing reactivates a decoded frame. `decode::verify_graph` checks a
frame's `pc` and register-file size against the compiled body it reconciles
each frame against, but it never cross-checks `home` against `kind`: a
crafted image can pair a method-kind frame (`home` meaningful only for
`FrameKind::Closure`) with a non-nil `home`. Nothing observes the
mismatch today, because the VM never runs a frame loaded from an image
-- only ever an activation one of the four push sites built.
A decoded home is only ever consulted as a return target, and a decoded
frame is never married to a live activation, so a `^` into one is
`cannotReturn` whatever its record says.
Trampolined sends,
suspend-and-resume and live debugging are exactly what would start running
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

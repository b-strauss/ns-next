# ns-next — Stateless Binary Deployment Artifact Design

> Status: proposed. Extends
> `2026-09-06-1659-ns-next-nesting-mixins-source-free-images-design.md`
> (the source-free image format) and settles one item left open by
> `2026-09-04-1153-ns-next-liveness-images-vm-design.md`'s "Deployment
> artifacts without source".

## Summary

`ns-next` gains a second serialization artifact: a **stateless binary** that
carries code and structure and no instance state at all. It sits beside the
existing image, which stays what it is — a snapshot of a running world, heap
plus reachable activations.

The two artifacts are one format. The stateless binary is the image's leading
sections, stopping where the object graph would begin, plus a recorded entry
point. A new `ns-next build` produces it; the existing `ns-next load` consumes
both, branching on a header flag.

This is Gilad Bracha's position, adopted deliberately: "None of this means you
can't have an image as well as a binary format. What is important is that you
do not have just an image. … Ideally, you have images and a binary format"
([An Image Problem](https://gbracha.blogspot.com/2009/10/image-problem.html)).
Milestone 3 already built everything the binary needs — a source-free load
path, a symbol table, a mixin table, and a cut at the platform boundary — so
the delta is a header flag, one field, one command, and a branch.

## Goals

- A deployment artifact that contains **no instance state**: no object table,
  no frame table, no roots.
- The artifact is **self-starting**: it records the selector to send, so
  running it needs nothing but the file.
- **Capabilities enter at start time, not at build time.** The primary factory
  and every instance initializer run on load, against the freshly bootstrapped
  platform.
- The two artifacts stay **provably consistent**: the sections that describe
  code and structure are byte-identical between a `build` and a `snapshot` of
  the same source.
- The binary is **verified like every other input**, and running one produces
  byte-identical output to running its source.

## Non-Goals

- **Suspending a running computation and serializing it.** Not possible today
  and not attempted here; see "Suspension is out of scope" below.
- **Runtime-created behaviour in the artifact.** A method installed by a mirror
  at run time does not round-trip; see "What the graph style can do that this
  one cannot".
- **A decompiler.** Unchanged from the liveness spec, still the intended way to
  export an artifact back to `.ns`, still unwritten. The optional debug source
  blob remains the interim answer.
- **Multi-module binaries and a manifest** in the sense of upstream's
  `packageUsing: manifest`. One root module per artifact, as today.
- **A `FORMAT_VERSION` bump.** The project has no external users; the header's
  flag byte is the extension point.

## How upstream does it, and where we differ

Checked against the Primordial Soup sources rather than inferred from the blog
posts.

**Upstream splits packaging time from start time with two selectors.**
`newspeak/HelloApp.ns`, in full:

```
class HelloApp packageUsing: manifest = () (
public main: platform args: args = (
	'Hello, polymerizing new world!' out.
)
) : (
)
```

The primary factory is `packageUsing: manifest` — it takes a manifest of module
declarations, not a platform. That instance is what gets serialized. The
platform arrives later, as `main:args:`'s first argument. The artifact is
stateless because it has never held a capability.

**A snapshot carries two objects.** From `newspeak/CompilerApp.ns`:

```
runtime:: (namespace at: runtimeName) packageRuntimeUsing: manifest.
app:: (namespace at: appName) packageUsing: manifest.
bytes:: Snapshotter new
    snapshotApp: app
    withRuntime: runtime
    keepSource: (appName = 'TestRunner').
```

An application object and a runtime object. `newspeak/Runtime.ns` is
`class Runtime packageRuntimeUsing: manifest` and contains the nested
`Platform` class, so the platform is built by the serialized runtime, not
supplied by the VM.

**The VM names no application entry point.** `vm/main.cc` maps the snapshot and
posts an isolate message carrying `argc - 2, &argv[2]`. The jump into Newspeak
is in `vm/isolate.cc`:

```cpp
String selector = interpreter_->object_store()->dispatch_message();
Method method = interpreter_->MethodAt(cls, selector);
interpreter_->Push(message_loop);
```

The selector comes from the **object store** — the snapshot's root array,
holding every object the VM must reach without a name lookup: `nil`, `true`,
`false`, the `message_loop` scheduler, the selectors the VM itself sends
(`doesNotUnderstand:`, `cannotReturn:`, `dispatchMessage`, …), and the classes
with VM-known formats (`Array`, `String`, `Closure`, `SmallInteger`,
`Activation`, `Method`, …). `docs/design.md` also records that "the object
store and the current activation record are the GC roots".

So upstream's entry point is stored **at VM level** (an object-store cell) and
conventional **at application level** (`main:args:`).

**Where we differ, and why.** The object store exists because the VM cannot
bake in addresses: a snapshot is a graph, so the objects the VM needs are named
by position in a known array. We do not need one. Our bootstrap is Rust and
deterministic, so what the VM needs is *reconstructible* rather than
serialized:

| | Primordial Soup | ns-next |
|---|---|---|
| `nil`/`true`/`false` | object-store cells | `Value` variants, not objects |
| Builtin classes | object-store cells | class tag → `interp.builtin_class(kind)` |
| Platform | inside the serialized runtime object | tag 7 → the bootstrap's platform |
| Selectors | object-store cells + `common_selectors` | the symbol table |
| VM entry | `message_loop` + `dispatch_message` | does not exist |

That is why the entry selector goes in the file rather than into "the store":
we have no store to put it in.

## What the graph style can do that this one cannot

Recorded so the trade-off is not rediscovered as a surprise. The difference
between the two styles is not table versus array — our mixin table *is* a
graph, written flat. It is **where the line falls between "carried in the
artifact" and "reconstructed by the runtime"**. Upstream draws it at nearly
nothing; we draw it far toward reconstruction.

Upstream can do three things we structurally cannot:

- **Ship a modified kernel or platform.** Theirs is Newspeak and travels in the
  artifact. Ours is Rust, so a changed platform means a different `ns-next`
  binary, and our artifacts are portable only across identical runtime
  versions.
- **Round-trip runtime-created behaviour.** A method installed by a mirror is
  an edge in their graph and serializes itself. Our compiled code is a table of
  compiler output, and `Mixin` has no interior mutability — the problem already
  recorded as an open question in the Milestone 3 spec.
- **Reuse one format for isolate messages**, which are partial graphs in the
  same encoding.

We can do two things they structurally cannot:

- **Keep the artifact small.** Their floor is ~1.27 MB because serialization
  starts at the application object and everything reachable from the platform
  comes along, compiler included. Ours is kilobytes.
- **Hold the untrusted-input invariant cheaply.** A hostile image here cannot
  replace a builtin: a class tag resolves against the fresh bootstrap, not
  against file content. Upstream's snapshot *is* the kernel, so the same
  guarantee would have to cover every object in the system.

**The decision is to stay with the table style**, on three grounds: the object
store buys the binary nothing given a deterministic bootstrap; native builtins
serve the project's performance priority; and the untrusted-input invariant is
already written into `CLAUDE.md` and is far cheaper to hold this way. The line
is movable, though, and it will move one step toward the graph style later —
for *user-defined* behaviour only, when edit-and-continue is built. Never for
the builtins.

## Format

Two flags and one field. Section order is unchanged; the binary stops earlier.

```
Image:   [header][source?][origin][symbols][code][mixins][root class][entry?][graph]
Binary:  [header][source?][origin][symbols][code][mixins][root class][entry ]
```

**`FLAG_STATELESS`** joins `FLAG_HAS_SOURCE` in the header's flag byte.
`read_header` already refuses any flag this build does not implement rather
than masking it off, so the extension point behaves correctly without change.
The two flags combine: a debug binary is legal and useful.

**The entry field is a symbol id**, consistent with every other name in the
format, bounds-checked before use. It sits at the *end* of the stateless run
rather than in the header for two reasons: it references the symbol table, so
it must follow it; and down there it does not disturb the equality of the
describing sections. It is optional in an image (a `snapshot` without `--send`
has no entry) and mandatory in a binary — which is exactly what separates a
startable artifact from a snapshot.

**The consistency guarantee, stated precisely:**

> For the same source file and the same `--debug-source` setting, the bytes
> from `origin` through `root class` are identical in a `build` artifact and a
> `snapshot` artifact.

Not "the binary is a prefix of the image" — that would be false the moment one
artifact records an entry and the other does not. As stated, it is exact and
directly testable.

**`FORMAT_VERSION` stays 1.** Images predating this change are disposable. The
untrusted-input guarantee does not come from the version field.

## The build path

`ns-next build <file.ns> <out> [--entry SEL] [--debug-source]` runs exactly
`run`'s pipeline up to engine installation — parse, resolve, bootstrap,
compile — and then **does not instantiate**. That omission is the whole point:
the artifact never sees a platform.

**Choosing the entry.** If the root class declares `main:args:`, that is the
default, which makes upstream's convention our default rather than our rule.
Otherwise `--entry` is required. `build` then refuses two things early rather
than deferring them to start-up:

- an arity other than 0, 1 or 2, because `entry_args` can supply nothing else;
  and
- a selector the root application's lookup cannot find, as either a public
  method or the accessor of a public nested class.

**`build` and `snapshot` share their section writers.** The byte equality above
must follow from construction, not from discipline — otherwise it drifts at the
first format change and the test catches it only later. `image::` gains a
second entry function beside `snapshot` that writes the shared prefix through
the same code and then emits the entry field in place of the graph section.
`platform` and `roots` leave its signature: with no graph there is nothing to
tag references against.

## The load path

`ns-next load <artifact> [--send SEL]` reads the header and branches.

- **Stateful** — unchanged. Decode the graph, `verify_graph`, advance the
  identity hash, send to `roots[0]`.
- **Stateless** — symbols → code table → `verify_code_table` → mixins →
  `build_module` → root superclass → `build_root_class` → entry id. No graph
  section. Then install the bytecode engine, seal the world, and hand back an
  empty `roots` with the entry as `Option<Symbol>`.

The caller instantiates the root class with the freshly bootstrapped platform
and sends the entry, or `--send`'s override. So the primary factory and every
instance initializer run **inside the loaded world**, against the artifact's
own compiled code — upstream's shape exactly: capabilities enter at start time.

**Instantiation stays in the CLI**, not in `image::load`. The boundary is
already drawn and documented on `factory_args_for`: deciding *that* a
command-line entry point supplies the platform is a CLI concern, because
nothing in the runtime needs to know it was invoked from a terminal. This
design keeps that line rather than crossing it for convenience.

**The tree-walker is refused**, unchanged, and now for a more basic reason than
before: a binary contains no AST at all.

**Errors stay ordinary.** An entry the world does not understand is a
`doesNotUnderstand:` through `report_entry_send`, not a special case.

## Verification

The stateless path skips `verify_graph` — there is nothing to verify — and
keeps `verify_code_table` unchanged. The entry symbol id is bounds-checked
before use, like every other id.

**One hardening this design adds.** `load` does not currently check that the
byte stream is exhausted when the last section has been read;
`Reader::remaining()` exists but serves only as an allocation bound, so
trailing bytes are silently ignored. With two artifact shapes that becomes a
confusion vector — a binary with graph-shaped garbage appended loads without
complaint today. `load` must refuse trailing bytes.

## Testing and gates

In order of importance:

1. **Equivalence.** For every module and entry, `build` followed by `load` must
   be byte-identical to `run --engine=bytecode` in stdout, stderr and exit
   status. This replaces the engine-versus-engine check a binary structurally
   cannot have — and because the differential harness already pins bytecode
   against tree-walk for the same sources, the equality holds transitively
   against the tree-walker too. The verification
   redundancy survives; it just routes through one more edge.
2. **Prefix equality.** `build` and `snapshot` of the same source with the same
   `--debug-source` setting agree byte for byte from `origin` through
   `root class`. This is what keeps the shared section writers honest.
3. **A binary lane in `run-corpus`** over the same entries. No new field in
   `corpus/runnable.toml` is needed: a row's `send` *is* the build's `--entry`.
4. **Untrusted input.** Mutation and truncation tests over the new flag, the
   entry field, and the new trailing-byte check.

## Suspension is out of scope

Recorded because it was asked directly and the answer shapes a later milestone.

An image cannot be taken mid-execution today, and the obstacle is control flow,
not the format. `Frame` is always heap-allocated, chained through `sender`, and
fully encoded — receiver, args, temps, `sender`, `home`, `current_class`,
`kind`, `pc`, `code_id`. But `vm.rs`'s send is a real Rust recursion
(`run_code` → `interp.send` → `invoke_method` → `run_code`), so the activation
chain exists twice: reified on the heap, and implicitly on the Rust stack. Only
the first can be serialized; only the second can be resumed. There is no yield
path out of `run_code`, and no re-entry point that would continue a decoded
frame at its `pc` and unwind into its `sender`.

Frames reach an image today only through reachability — a closure in a slot
holds its defining activation through `captured_frame`/`home`, and that frame's
`sender` chain follows. That is a by-product, not suspension.

The fix is a trampoline: an explicit frame stack in `run_code`, where a send
pushes instead of recursing. Then suspending is "leave the loop, the chain is
already on the heap" and resuming is "re-enter with the top frame". The
alternative — keep recursing and unwind through a signal — is not viable,
because native frames (builtins that call back into Newspeak) would be lost.

**Decision: the trampoline belongs to Milestone 4**, where the moving collector
requires it anyway (upstream's own GC roots are the object store and the
current activation, nothing else), and where the debugger and any actor
scheduling need it too. The stateless binary must not wait behind a VM rewrite
it does not need.

One detail that is *not* an obstacle: `frame.pc` is stored only at every
`Send*`, `MakeClosure` and `NonLocalReturn` (`vm.rs`, deliberately, to keep a
borrow and a write off the straight-line path). Those are exactly the
safepoints, and a suspension could only happen at a send boundary anyway.

## Decisions

1. **One format, two artifacts, distinguished by a header flag** — not two
   magics and two decoders. A second decoder would have to hold the
   untrusted-input guarantee independently, and the two would drift.
2. **The binary records its entry point.** A deployment artifact that cannot
   say how it starts is not one. Upstream stores the VM-level entry in the
   object store; lacking a store, we put the application-level entry in the
   file.
3. **`main:args:` is the default, not the rule.** Upstream's convention costs
   us the entire `run-corpus` gate if made mandatory — none of the six modules
   that run declares it; their entries are `return42`, `Klass`,
   `TextProperties`, `Duct`, `AST` and `InstructionStream`.
4. **`build` is a new verb; `load` is not.** Producing the two artifacts are
   genuinely different acts — a build against source versus a snapshot of a
   running world. Consuming them is the same act, deserialize and start,
   differing only in whether instantiation precedes the entry send, which the
   header says. One `load` keeps verification and error paths in one place.
5. **Stay with the table style.** See "What the graph style can do that this
   one cannot" for the grounds and for what is knowingly given up.
6. **The trampoline is deferred to Milestone 4.** See "Suspension is out of
   scope".

## Open questions

- **Whether upstream strips source at packaging time.** The liveness spec
  records a measurement of four `.vfuel` artifacts from
  `newspeaklanguage.org/samples/` concluding that nothing is stripped (~44%
  long text runs in each). `CompilerApp.ns`'s
  `keepSource: (appName = 'TestRunner')` shows a switch that is false for every
  ordinary application, which sits in tension with that. Both can be true — the
  samples may come from a different toolchain, or the measured text runs may be
  selectors rather than source — but the spec's claim should be re-checked
  rather than left standing.
- **Whether an image should record its entry too.** The field is optional in an
  image and this design never sets it. Letting `snapshot --send X` record `X`
  would make `load` of a stateful image self-starting as well, at the cost of
  making the two artifacts' bytes differ in one more place.
- **What a multi-module artifact would look like.** Upstream's
  `packageUsing: manifest` takes a manifest of module declarations, so one
  artifact can carry a linked set. One root module per artifact is assumed
  throughout here, and nothing in the format currently anticipates more.

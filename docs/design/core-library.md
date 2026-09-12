# Core library and the platform

> **Status:** partial by design — the surface grows on demand, not
> speculatively.
> **Code:** `crates/ns-runtime/src/builtins/`, `platform.rs`.

The core library is native Rust, not Newspeak source. That is the choice that
lets an image stay small and keeps a hostile image from replacing a builtin
class, and it is also the choice that means a modified platform is a different
`ns-next` binary rather than a different artifact. See the comparison in
[image-format.md](image-format.md).

## What exists

`Object`, `Behavior`/`Class`, `Boolean`/`True`/`False`, `Integer`, `Float`,
`String`, `Symbol`, `Character`, `Array`, `OrderedCollection`/`List`, `Nil`,
`Block`.

A minimal `platform` object exposes these plus a stdout stream object, enough for
transcript-style output.

## The on-demand rule

**`platform.rs` is widened only on demand.** An accessor exists because a module
on `corpus/runnable.toml` needs it, and the whitelist entry that demanded it is
named in the accessor's comment.

This is not minimalism for its own sake. A capability nothing on that list asks
for has no test that would catch it rotting, so it is a liability rather than an
asset. A module that needs something nobody has asked for is a reason to widen
the whitelist deliberately — with a real module, run end to end, its output
recorded — not a reason to build a speculative library.

The `collections` namespace is the worked example. It has exactly one member,
`List`, because exactly one is asked for; `List` names the growable ordered
sequence the runtime already has under its Smalltalk name `OrderedCollection`, so
it is a name for an existing class rather than a new implementation. The corpus
also asks that namespace for `Map`, `IdentityMap`, `IdentitySet`, `WeakArray` and
`Collection`. Those name classes the runtime does not have, and inventing them —
or worse, aliasing them onto `OrderedCollection`, which is neither a dictionary
nor a set — would be a collections library built without a caller. Modules
needing them stay off the whitelist.

## Tagging inside the platform

The platform singleton, its `transcript` stream and its `collections` namespace
all share one builtin-class tag and are told apart by payload shape: zero fields
for the singleton, one for the stream, two for the namespace. None ever holds
real slot data; the field count is purely a tag.

This is a wart, and it is recorded rather than hidden. It exists because there was
no second builtin-class slot to spare, and adding one would renumber the image
format's builtin-class table for no behavioural gain.

## Deviation: `i64` integers

Integers are `i64`. Overflow raises a Newspeak error rather than promoting to a
bignum. Arbitrary-precision integers are not built.

This is observable to any program that overflows, which is why it is a recorded
deviation rather than an implementation detail.

## Planned: the leaf/callback split

The shape of a real collections library, of arbitrary-precision integers, and of
the mirror-based reflection protocol the platform would hand out are all open;
the last of those is sketched in
[liveness-and-reflection.md](liveness-and-reflection.md). Beyond that, "widen
when a whitelisted module demands it" still governs what gets added.

What is decided is *where a builtin may live*, and it is a constraint on future
work rather than a wish. Everything in this runtime must stay optimizable,
builtins included: a builtin must eventually be able to become native code. A
Rust native that calls back into Newspeak cannot. Written as a Rust loop that
re-enters the interpreter, `do:` can never have its block inlined into the loop,
never be type-specialized, and never be compiled as a unit — not because the JIT
is immature, but because no compiler optimizes through an opaque Rust function
body. That is a permanent ceiling on the methods that are hottest in real code.

Both reference implementations avoid this rather than accept it. V8 writes the
builtins that involve control flow or call user code in Torque/CodeStubAssembler,
a form its own compiler understands and can call without crossing a C++ boundary,
and keeps plain C++ runtime functions only for the slow paths it is content to
leave opaque. Cog writes `do:` in Smalltalk and keeps only irreducible leaf
operations as primitives.

**The rule: a native that calls back into Newspeak must not stay a Rust native.**
A native that is a leaf computation should stay one, and should get an inlined
fast path instead. Today ten selectors call back, through
`builtins::call_block`, and they have three different destinations:

| Selector | Destination |
|---|---|
| `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `ifTrue:`, `ifFalse:`, `and:`, `or:` | Lowered by the compiler, with the native kept as the guarded fallback — already how it works for the literal-block case. The trampoline turns the fallback native into a trampoline step that returns the block to activate instead of calling it ([execution.md](execution.md), "The trampoline (1b)") |
| `whileTrue:`, `whileFalse:` | The same, both halves |
| `value`, `value:`, … | Absorbed by the trampoline: activating a closure is what the activation stack does. See [execution.md](execution.md) |
| `do:` | Written in Newspeak |

So after the trampoline exactly one of today's methods still calls back, and it
is the one that moves to Newspeak. Everything else in `builtins/` — `size`,
`at:`, `class`, `hash`, `printString`, the arithmetic, the comparisons,
`asString`, `asSymbol` — is a leaf, and its computation stays in Rust.

**Where this is going: the core library as Newspeak source, with Rust
primitives underneath** (the user's direction, 2026-09-12). Primordial Soup
holds its whole core library as Newspeak (`KernelForPrimordialSoup.ns`),
compiled like any other class; a leaf is a Newspeak method that names a
primitive by pragma and carries a Newspeak fallback body for when the primitive
fails — `Closure>>value` is primitive 156 with a `numArgsError:` fallback,
`True>>ifTrue: then` is simply `^then value`. ns-next's target is the same:
every builtin method a real method that lookup, mirrors and the debugger see,
the leaves backed by the Rust primitives that exist today. The computation of a
leaf does not move; what changes is that it becomes reachable as a method. That
is what lets a debugger step into `do:` and edit-and-continue reach library
code ([liveness-and-reflection.md](liveness-and-reflection.md)), and what gives
the JIT something to work with: every send inside library code gets a call site
of its own, whose inline cache records what flows through it, where a Rust
native records nothing. Library code is shared by every caller, so those sites
are often megamorphic — `action value:` inside `do:` sees every block in the
program — and the feedback pays off once the JIT inlines library methods into
their callers, not from the caches alone.

It comes after the inline caches, for the reason "The ordering matters" gives
for `do:`, and it does not change the image's decision to keep the builtins out
of every artifact ([image-format.md](image-format.md), "How upstream
compares"): the library would be compiled from source the runtime ships, as the
bootstrap is built from Rust today. Nothing done before then should make the
move harder; a control-flow native written as a trampoline step, for one, maps
directly onto a primitive of the kind `Closure>>value` has.

**The ordering matters and is the opposite of the obvious one.** `do:` in
Newspeak costs an activation for `do:` itself plus real sends for the loop
mechanism, on top of the block activation per element it already pays. On the
activation stack a user-method send costs ~91 ns, against ~146 ns on the arena
binary measured in the same sitting ([execution.md](execution.md), "What the
activation stack moved"), and the per-call lookup the inline caches remove is
still in it. A regression today, so the move happens *after* the caches, not
before. What Milestone 4 owes this section is only that it not deepen the
investment in Rust callbacks while it is rewriting the code around them.

A related item, same cause: native dispatch selects with `match
selector.as_str()`, a string-comparison chain on the hot path that is not a
shape a compiler can specialize either. It is part of what one native `+` costs
on the activation stack, ~53 ns: `bench_send.ns` minus the send decomposition's
row two. The arena binary's figure for the same difference, ~84 ns, also carries
a temporaries allocation, so it overstates the native send
([execution.md](execution.md), "What the activation stack moved"). The fix
lives with the inline caches, not in a faster `match`: a lookup that reaches a
native binds an enumerated primitive, a native inline-cache entry carries it,
and the JIT emits it inline ([dispatch.md](dispatch.md), "Planned: inline
caches"). A leaf such as `+` additionally gets an inlined fast path in compiled
code, guarded like the control-flow inlining, as Cog's `bytecodePrimAdd` does.

## Decisions

- **Leaf primitives are native Rust; anything that calls back into Newspeak is
  not**, because a Rust callback is a permanent barrier to optimization. This
  narrows the earlier "native Rust rather than Newspeak source" decision rather
  than reversing it: the reasons for that decision — artifact size and the trust
  boundary — apply to leaves, which are almost all of the library.
- **A native is bound as an enumerated primitive, not a function pointer**, so
  the JIT can emit it inline.
- **The core library is headed for Newspeak source with Rust primitives
  underneath**, as Primordial Soup's is, so that every builtin method is a real
  method. Planned, after the inline caches.
- **The platform grows only against a whitelisted module**, so every capability
  has a test that would notice it breaking.
- **`i64` with an error on overflow**, rather than silent wraparound or a bignum
  library nothing yet needs.

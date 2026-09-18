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
format's builtin-class table for no behavioural gain. Natives are method-table
entries ([dispatch.md](dispatch.md), "Primitives"), so all three objects'
selectors sit in the one `Platform` table, and a selector sent to the wrong one
of them is a primitive failure, which continues the lookup at `Object` exactly
as any other primitive's failure does. They keep the shared class, since
`platform transcript class` answering `Platform` is observable.

## Deviation: `i64` integers

Integers are `i64`. Overflow raises a Newspeak error rather than promoting to a
bignum. Arbitrary-precision integers are not built. §5.1.1 says the opposite in
one sentence — "*Note that integers are not limited in size*" — and puts no
bound on the *n* in the digit sum it defines a literal by, so a literal past
`i64`'s range is an ordinary `Integer` there and a deferred
`Literal::MalformedInteger` here.

This is observable to any program that overflows, which is why it is a recorded
deviation rather than an implementation detail.

## Deviation: a non-integral literal is a binary float

§5.1.1: "Non-integral numbers denote an object that obeys the protocol of class
`FixedPointNumber`" — an exact decimal type, chosen there so that "financial
applications in particular" get "accurate decimal numbers" and users are not
exposed to floating point's "counterintuitive ways". `2.5` here is an `f64`
(`compiler/lower.rs` parses the literal's text straight into one), so the
arithmetic is binary floating point: `(0.1 + 0.2) = 0.3` answers `false` where
an exact decimal type answers `true`.

The specification records the same divergence in its own implementation
("non-integral numbers are currently represented as instances of the Squeak
class `Fraction`. This is a bug"), and it names the open design question —
whether a user chooses the numeric implementation — rather than settling it.
A golden for the rule would have to be skipped behind a whole numeric tower,
so the deviation is recorded here instead.

## Planned: the core library as Newspeak source

The shape of a real collections library, of arbitrary-precision integers, and of
the mirror-based reflection protocol the platform would hand out are all open;
the last of those is sketched in
[liveness-and-reflection.md](liveness-and-reflection.md). Beyond that, "widen
when a whitelisted module demands it" still governs what gets added.

**This step also carries the rest of §3.2** (decided 2026-09-18): `Class`,
`Behavior` and `Metaclass` become declared kernel classes rather than objects
hand-built in Rust, and a class object's own class becomes its metaclass. Both
references declare exactly these three in their kernel source
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`,
`prior-art/newspeak/KernelForJS.ns`); building them earlier would mean building
them twice, and the reflection work right after this step cannot start without
them — a mirror's test for a metaclass is `Metaclass = classOf: reflectee`,
which needs the instance-of relation reified. The flat metaclass hierarchy that
§3.2 also requires did *not* wait for this step: it was corrected ahead of 3c,
because the primary factory's special cases depended on it
([object-model.md](object-model.md), "Class sides and metaclasses").

What is decided is *where a builtin may live*, and it is a constraint on future
work rather than a wish. Everything in this runtime must stay optimizable,
builtins included: a builtin must eventually be able to become native code. A
Rust native that calls back into Newspeak cannot. A `do:` written as a Rust
loop that re-enters the interpreter can never have its block inlined into the
loop, never be type-specialized, and never be compiled as a unit — not because the JIT
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
fast path instead. No native calls back into Newspeak: each native that runs a
block answers it as a trampoline step instead of calling it:

| Selector | How it runs its block |
|---|---|
| `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `ifTrue:`, `ifFalse:`, `and:`, `or:` | Lowered by the compiler for a literal block, with the native kept as the guarded fallback. The native is a trampoline step: it returns the block to activate instead of calling it, with a continuation for what `and:` and `or:` do with its value ([execution.md](execution.md), "The trampoline") |
| `whileTrue:`, `whileFalse:` | The same, both halves: the native is a trampoline step that returns the condition block, and its continuation activates the body or ends the loop |
| `value`, `value:`, … | A trampoline step that returns the block to activate: activating a closure is what the activation stack does. See [execution.md](execution.md) |
| `do:` | A trampoline step: it answers the block for the first element, and its continuation activates it for each next one ([execution.md](execution.md), "No re-entry from Rust"); written in Newspeak in step 6 of M4's order of work ([PROGRESS.md](../PROGRESS.md)) |

So no native calls back into Newspeak, which the root discipline needs as well:
no Rust frame may be live below a safepoint ([memory.md](memory.md), "No Rust
frame is live below a safepoint"). `do:` still moves to Newspeak after the
caches, so that it becomes a real method, one that lookup, mirrors and the
debugger see; being a trampoline step does not make that move harder, for the
reason the end of this section gives. Everything else in `builtins/` — `size`,
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

It is step 6 of M4's order of work ([PROGRESS.md](../PROGRESS.md)), for the
reason "The ordering matters" gives for `do:`, and it does not change the image's decision to keep the builtins out
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
activation stack moved"), and the per-call lookup the inline caches now remove
was still in it then. A regression before the caches, so the move happens
*after* them, not before. What Milestone 4 owes this section is only that it not deepen the
investment in Rust callbacks while it is rewriting the code around them; step
2a took the last callback away, as a trampoline step rather than as Newspeak,
which leaves this ordering as it is.

A related item, same cause: every native is an entry of its builtin class's
method table carrying an enumerated primitive, found by the ordinary lookups
and executed by a `match` on the enumeration ([dispatch.md](dispatch.md),
"Primitives") — a shape a compiler can specialize, part of what one native
`+` costs on the activation stack, ~53 ns (`bench_send.ns` minus the send
decomposition's row two; the arena binary's figure for the same difference,
~84 ns, also carried a temporaries allocation, so it overstated the native
send, [execution.md](execution.md), "What the activation stack moved"). A
native inline-cache entry will carry the primitive once 3b exists, and the
JIT will emit it inline. A native whose receiver has the wrong shape fails,
and a failure passes its level over, so the lookup continues at `Object` and
ends in `doesNotUnderstand:` if nothing answers. It is the first half of
"Where this is going" above — every builtin
method a real method — for the leaves that exist now; the second half,
Newspeak fallback bodies and library code in Newspeak, is step 6 of M4's order
of work. A leaf such as `+` will additionally get an inlined fast path in
compiled code, guarded like the control-flow inlining, as Cog's
`bytecodePrimAdd` does — not built yet, since it waits on the caches too.

## Decisions

- **Leaf primitives are native Rust; anything that calls back into Newspeak is
  not**, because a Rust callback is a permanent barrier to optimization. This
  narrows the earlier "native Rust rather than Newspeak source" decision rather
  than reversing it: the reasons for that decision — artifact size and the trust
  boundary — apply to leaves, which are almost all of the library.
- **A native is bound as an enumerated primitive, not a function pointer**, so
  a later JIT tier can emit it inline. Found as an entry of its builtin class's
  method table, with an access modifier (§5.5 makes `Object`'s
  `doesNotUnderstand:` protected).
- **The core library is headed for Newspeak source with Rust primitives
  underneath**, as Primordial Soup's is, so that every builtin method is a real
  method. Planned as step 6 of M4's order of work ([PROGRESS.md](../PROGRESS.md)):
  after the caches, closure conversion and the collector, and before the JIT
  tiers, which need the library as real Newspeak code to optimize. Everything
  that calls back into Newspeak moves; the leaves stay primitives.
- **The platform grows only against a whitelisted module**, so every capability
  has a test that would notice it breaking.
- **`i64` with an error on overflow**, rather than silent wraparound or a bignum
  library nothing yet needs.

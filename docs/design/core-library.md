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

## Planned

Nothing is designed beyond "widen when a whitelisted module demands it". The
shape of a real collections library, of arbitrary-precision integers, and of the
mirror-based reflection protocol the platform would hand out are all open; the
last of those is sketched in
[liveness-and-reflection.md](liveness-and-reflection.md).

## Decisions

- **Native Rust rather than Newspeak source**, for artifact size and for the
  trust boundary.
- **The platform grows only against a whitelisted module**, so every capability
  has a test that would notice it breaking.
- **`i64` with an error on overflow**, rather than silent wraparound or a bignum
  library nothing yet needs.

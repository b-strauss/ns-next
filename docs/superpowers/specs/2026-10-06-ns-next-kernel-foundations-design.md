# ns-next — the kernel's foundations (design)

The second of the six sub-projects that complete `Kernel.ns` and
`CollectionsFactory.ns` to Primordial Soup's public core API
(`docs/design/core-library.md`, "Planned: the kernel completed to Primordial
Soup's core API"). This spec is temporary: its content moves into the living
design documents at the end of the batch, and the file is deleted then.

Every decision below was taken with the user on 2026-10-06.

## Scope

The error classes, the `isKindOf…` rule, `yourself`, `identityHash`, the
kernel facet's accessors, `Proxy`, a public `Future`, and a generic
`CannotInstantiate`. Classes of later sub-projects (`Collection`,
`Fraction`, `ByteArray`, `StringBuilder`, `WeakArray`, `WeakMap`,
`Ephemeron`) and `garbageCollect` are not part of it; each brings its own
`isKindOf…` answer and facet accessor when it is built.

## The classes

| Class | Superclass | Factory | Public slot | Reached through |
|---|---|---|---|---|
| `ArgumentError` | `Error` | `value:`; class side `new` = `value: nil` | `argument` | the kernel facet |
| `NotFound` | `Error` | `element:` | `element` | `CollectionsFactory.ns`'s namespace |
| `CannotReturn` | `Error` | `result:` | `result` | the kernel facet |
| `NonBooleanReceiver` | `Error` | `receiver:` | `receiver` | nothing: private, as Primordial Soup's |
| `CannotInstantiate` | `Error` | `behavior:` | `behavior` | the kernel facet |
| `Proxy` | `Object` | `new` | -- | the kernel facet |
| `Future` | `Object` | class side `computing:` | -- | the kernel facet |
| `FutureImplementation` | `Proxy` | `computing:` | -- | nothing: private |

- **The error classes are Primordial Soup's, under `Error`**, so that
  `on: Error do:` still catches them. Each one's `messageText` is today's
  text where there is one, and otherwise one built from its slot (for
  example `not found: #missing`). A handler receives an instance of the
  concrete class and reads its slot. Primordial Soup keeps
  `ArgumentError`'s slot `argument` private; here it is public, so that a
  handler can read it. It is not named `value`, after the factory's
  keyword: an exception answering `value` would pass for a block wherever
  the kernel sends `value` to an argument, which is why `Object` has no
  `value` (`core-library.md`, "`True` and `False` are quick methods").
- **`CannotInstantiate` is an addition**, not part of Primordial Soup's
  API (which marks a class nobody may instantiate by naming its primary
  factory `_cannotInstantiate`). It is public so that a program can catch
  it and signal it for its own classes. The slot is named `behavior`, not
  `class`, which would override `Object>>class`.
- **`Future` is a public class whose only use is its class-side secondary
  factory** `computing: block`, which answers
  `FutureImplementation computing: block`. Its instance initializer
  signals `CannotInstantiate behavior: self class`, so `Future new` fails,
  and so does instantiating a subclass, since §6.2 runs `Future`'s initializer
  for every subclass. A subclass does not inherit `computing:` (§3.2:
  metaclasses are flat).
- **`FutureImplementation` is today's private `Future`**, renamed and moved
  under `Proxy`. It stays the one class the runtime knows as a future
  (`BuiltinClass::Future`, wire tag 30), so every future a program can hold
  keeps `docs/design/object-model.md`'s four rules: rule 3, that a resolved
  future is observable by no program, among them. No program can name it,
  so none can subclass it. It declares no `isKindOfFuture`: answering it
  would make a future observable, so the message is forwarded to the value
  like any other.
- **`Proxy` overrides every public member of `Object` as protected** --
  `class`, `==`, `=`, `hash`, `identityHash`, `yourself`, `printString`, the
  class accessors, the nil protocol, `halt` -- so that a message from
  outside finds no public method under §5.5's `lookupPublic` and reaches
  `doesNotUnderstand:`. A subclass that implements only
  `doesNotUnderstand:` is a total forwarding proxy. It is pure Newspeak: a
  program's proxy is distinguishable as an argument (`target == proxy` is
  false), as in Primordial Soup; only `FutureImplementation` gets the
  runtime's symmetric see-through. The test that checks today's `Future`
  overrides every public member of `Object` moves to `Proxy`.

## `Object`

- `yourself` answers the receiver, for cascades.
- `identityHash` answers the identity hash; for a value it is its `hash`,
  since `==` on a value is `=` (as `core-library.md` already records).
- `doesNotUnderstand:` answers `false` for a unary selector that begins with
  `isKindOf`, and signals `MessageNotUnderstood` for everything else, a
  keyword `isKindOf…:` selector included.

## The `isKindOf…` answers

Every core class that exists answers its own question with `true`:
`isKindOfArray`, `isKindOfBehavior`, `isKindOfBoolean`, `isKindOfClosure`,
`isKindOfFloat`, `isKindOfInteger`, `isKindOfNumber`, `isKindOfString`,
`isKindOfList`, `isKindOfMap`, `isKindOfSet`. `Number` answers `false`
explicitly for `isKindOfFloat`, `isKindOfInteger` and `isKindOfFraction`,
as Primordial Soup's does. A program introduces a question of its own by
answering it in its classes, without touching `Object`.

## The kernel facet

`PlatformFactory.ns`'s `KernelNamespace` answers, besides today's
`Exception`, `Error`, `MessageNotUnderstood`, `Halt` and `Message`:
`ArgumentError`, `CannotReturn`, `CannotInstantiate`, `Closure`, `Future`
and `Proxy`. `NotFound` is answered by the collections' namespace, as
Primordial Soup keeps it in its collections module. `NonBooleanReceiver`
is answered by nothing.

## Where the errors come from

**Rust names the concrete class at the site that finds the error.**
`NewspeakError` carries a kind: `Error` (as today), `ArgumentError(value)`,
`NotFound(element)`, `CannotReturn(result)`, `NonBooleanReceiver(receiver)`
or `CannotInstantiate(behavior)`. `Mutator::signal_vm_error`, the one place
that turns a Rust error into a Newspeak object, instantiates the matching
kernel class with the text as `messageText`, its slot, and `originSelector`
and `originReceiver` as today, and sends it `signal`. The site cannot build
and signal the exception itself: signalling runs Newspeak, which a leaf
never calls back into, and a leaf that declines or raises has changed
nothing (`object-model.md`, "Where Rust sees a future"), which an
allocation would break. The optimized lowering, the reference lowering and
a later JIT all reach `signal_vm_error`.

`a_signal_fits_in_the_niche_of_its_one_unboxed_error` bounds the size of
`Signal`. If the kind and its value push `NewspeakError` past it, the kind
is boxed; only the error path pays for it.

**Which site gets which class**: the class Primordial Soup signals in the
same situation, its slot holding what Primordial Soup's holds.

- `ArgumentError`: an index out of range, a bad size, an argument of the
  wrong kind (`,` without a `String`, `asStringFixed:` outside 0 to 20,
  `asInteger` of a non-finite `Float`), and a wrong number of block
  arguments (Primordial Soup's `numArgsError:`, the block as `argument`).
  Whether division by zero is one is settled in the plan against Primordial
  Soup's method.
- `CannotReturn`: both cannot-return errors (`interpreter/mod.rs`,
  `raise_return`; `interpreter/unwind.rs`, `dead_activation_error`), so
  also `restart`, `returnValue:`, and `retry`, `resume:` and `return:` on a
  dead handler, as Primordial Soup's tests ask. `result` is the value being
  returned, `nil` for `restart`.
- `NonBooleanReceiver`: a loop's condition that answers no Boolean -- the
  inlined loop, the native loop and the reference lowering alike.
  `mustBeBoolean` disappears from the compiler, the VM and `Kernel.ns`. An
  inlined conditional is unchanged: a receiver that is no Boolean is sent
  the real message, and one that does not understand it signals
  `MessageNotUnderstood`. The specification has no inlining, and Primordial
  Soup's tests call its `NonBooleanReceiver` for a conditional a "cheat";
  a loop's behaviour, by contrast, is the library's to define.
- `CannotInstantiate`: `Symbol new`, the VM mirror's class (with `behavior`
  `nil`, so that no path to the mirror's class depends on the reasoning
  that only its holder can trigger it), a class object made otherwise than
  by its declaration, a value class that defines `==`, and `Future new`
  from Newspeak.
- `NotFound`: raised only in Newspeak, by `Map>>at:`, `removeKey:`,
  `Set>>remove:` and `List>>remove:`.
- Everything else stays an `Error`: the depth limit, a divergent future
  (an `Error` in Primordial Soup too), a superclass clause that is no class,
  a wrong number of factory arguments, and the internal errors.
  `MessageNotUnderstood` keeps its own path.

**The Newspeak side**: the kernel and collection methods that signal
`Error signal: '…'` for one of the cases above signal the class instead,
the text unchanged (`Kernel.ns`, `CollectionsFactory.ns`). The comment in
`CollectionsFactory.ns` saying it uses texts where Primordial Soup signals
`NotFound` or `ArgumentError` goes. This closes `OPEN-WORK.md`'s "Failure
blocks and Primordial Soup's error classes".

## Image and wire tags

`FutureImplementation` stays `BuiltinClass::Future` and tag 30. If an error
class becomes a builtin so that `signal_vm_error` can build it by name and
an image needs a tag for it, it takes the next free number, pinned in
`tests/opcode_tags.rs`. `Proxy` and `Future` are ordinary kernel classes,
named by platform path like other shipped classes. A future a program made
with `computing:` round-trips as today's record.

## Testing

Under both lowerings:

- Goldens that change: the two loop-condition goldens
  (`non_boolean_while_condition_fails_with_must_be_boolean`,
  `variable_while_condition_that_goes_non_boolean_fails_with_must_be_boolean`)
  report `NonBooleanReceiver` instead of a missing `#mustBeBoolean`. The
  other affected goldens keep their texts; one that prints the class of a
  caught error changes, with the reason recorded.
- New goldens: each error class caught by name, its slot read, and caught by
  `on: Error do:`; `CannotReturn` for `restart`, `retry`, `resume:` and
  `return:` on a dead handler; `NonBooleanReceiver` from an inlined, a
  native and a variable loop; `isKindOf…` -- a class's own `true`, `false`
  for another's question, a program's own question, a keyword selector
  that stays a `MessageNotUnderstood`; `yourself` in a cascade;
  `identityHash` of an object and of a value; a `Proxy` subclass with only
  `doesNotUnderstand:` forwarding `printString`, `=`, `hash`, `yourself` and
  `class`; `Future computing:` -- computed on first contact, symmetric `==`
  once resolved, divergent evaluation, an exception in the computation and
  the retry; `Future new`, a subclass of `Future` and `Symbol new`
  signalling `CannotInstantiate`; the kernel facet answering each new
  class.
- Rust: the override test on `Proxy`; one test per kind that
  `signal_vm_error` builds the right class with its slot; the size tests in
  a release build.
- The collection stress mode covers a program's future being replaced under
  a scavenge and a compaction.

## Documentation

`core-library.md` (the classes, the facet, sub-project 2 built, its
decisions), `object-model.md` (`FutureImplementation`, `Future computing:`,
`CannotInstantiate`, `Proxy`), `execution.md` (`NonBooleanReceiver` where
`mustBeBoolean` was), `OPEN-WORK.md` ("Next" moves to sub-project 3; the
failure-blocks item goes), `PROGRESS.md` (an entry).

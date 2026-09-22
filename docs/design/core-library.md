# Core library and the platform

> **Status:** partial — the core library is built as Newspeak source,
> `Kernel.ns`, over one VM mirror; missing are `Class`, `Behavior` and
> `Metaclass` (step 6's third plan) and `Integer>>/`, and the surface grows
> on demand, not speculatively.
> **Code:** `crates/ns-runtime/core-library/Kernel.ns` (the library),
> `interpreter/kernel.rs` (the bootstrap's kernel phase),
> `interpreter/mirror.rs` (how a send to the mirror runs), `builtins/` (the
> leaves; `builtins/primitive.rs`'s `MIRROR` is the mirror's table),
> `compiler/primitive_method.rs`, `platform.rs`.

The core library is Newspeak source the runtime ships:
`crates/ns-runtime/core-library/Kernel.ns`, compiled into the binary
(`include_str!`) and compiled again at every bootstrap, never carried in an
artifact. Its methods reach Rust through exactly one object, the VM mirror,
as §8.3.1 requires, and the leaves — the arithmetic, the comparisons,
`size`, `at:put:`, `printString`, `show:` — stay Rust. Keeping the library
out of the artifact is what lets an image stay small and keeps a hostile
image from replacing a builtin class; it also means a modified library is a
different `ns-next` binary rather than a different artifact. See the
comparison in [image-format.md](image-format.md).

## What exists

`Kernel.ns` declares one nested class per builtin class but the mirror's:

```
Top                     (private; no superclass, declares nothing)
└── Object              class, ==, =, hash, printString, doesNotUnderstand:
    ├── Value           =, ==, hash, and the two slot-reading leaves
    │   ├── Boolean     ├── True
    │   │               └── False
    │   ├── Number      ├── Integer
    │   │               └── Float
    │   ├── String
    │   ├── Symbol
    │   ├── Character
    │   └── Nil
    ├── Array
    ├── OrderedCollection
    ├── Block
    └── Platform
```

That is §3.2's shape and §3.1.1's, taken literally. §3.2 says "a class is
either the empty class `Top` or the application of a mixin to another class",
and "only the class `Object` may have `Top` as its superclass", so `Top`
exists, declares nothing, and is the one class `Kernel.ns` keeps private —
every other class says `public`, because §6.3.1 makes an undeclared member
protected and a protected class is unreachable from any other module, which
is where a program's `Integer` or `String` has to come from. §3.1.1 says a
value object's "class inherits from class `Value`", so every class whose
instances are values hangs below `Value`, and `Number` sits between it and
`Integer` and `Float` because §3.1.1 names `Number` among them.

**This is not SOMns's shape, and the difference is the one rule SOMns does
not keep.** `prior-art/somns/core-lib/Kernel.ns` has `Top` and `Value`, and
its value classes hang below `Value` exactly as they do here — its `=` is
even the same leaf, `vmMirror value: self sameAs: other`. What it does
differently is insert a class between: `Top` → `Thing`, which declares
`doesNotUnderstand:`, `asString`, `isNil` and the rest, and then `Object`
and `Value` as **siblings** under `Thing`, `Object` declaring only `==` and
`~=` and `Value` only `=`, `<>` and `copy`. So in SOMns a value is not an
`Object`. §3.2 is explicit that it must be — "except for `Top` all classes
are subclasses of class `Object`" — so `Value` is a subclass of `Object`
here, and the behaviour SOMns puts on `Thing` is on `Object`. The cost of
the rule is that `Value` must *override* what `Object` declares, `=`, `==`
and `hash`, rather than declaring them where nothing else has; the benefit
is that `3 doesNotUnderstand: …` and `'a' printString` need no second
declaration, and that a program may assume of any receiver what §3.2 says it
may.

Almost every method is a one-line forwarder to the mirror ("The
shape of a kernel method" below): `Object`'s `class`, `==`, `=`, `hash`,
`printString` and protected `doesNotUnderstand:`; `Boolean`'s conditionals,
`not`, `&`, `and:`, `|`, `or:` and `printString`; `Integer`'s `+ - * // < >
<= >= =`, `negated`, `abs`, `printString` and `asString`; `Float`'s
`+ - * / < > =`, `hash`, `printString` and `asString`; `String`'s `size`,
`,`, `=`, `hash`, `asSymbol` and `printString`; `Symbol`'s `asString`, `=`,
`hash` and `printString`; `Character`'s `asInteger`, `asString` and `=`;
`Array`'s `do:`, `at:put:`, `size` and `at:`; `OrderedCollection`'s `do:`,
`add:`, `size` and `at:`; `Block`'s `value` to `value:value:value:`,
`numArgs`, `whileTrue:` and `whileFalse:`; and the platform's accessors.
`Nil`, `True` and `False` declare nothing of their own.

The three exceptions are `hash` on `Integer` (`^self`), on `Character`
(`^self asInteger`) and on `Boolean` (`^self ifTrue: [1] ifFalse: [0]`):
ordinary methods, because the value each answers is already at hand and a
row for it would only repeat what `Kernel.ns` can say. Every value class
declares the `hash` its own `=` demands — §3.1.1 lets a value be copied
across an actor boundary, so a copy has to hash like its original, and none
of them may inherit `Object`'s identity hash. `String`'s, `Character`'s and
`Symbol`'s `=` each require an argument of the receiver's own class:
`String` and `Character` share an `ObjectData::String` payload, so a rule
that read the payload alone would make `"a" = 'a'` true while the two hash
apart.

Four further rows serve `Value` itself rather than one builtin:
`value:sameAs:ifFail:` (reported `=`), `valueHash:ifFail:` (`hash`),
`valueSlotCount:ifFail:` (`slotCount`) and `value:slotAt:ifFail:`
(`slotAt:`). The first two are the equality and hash `builtins/value.rs`
derives from an instance's state, for a class that inherits `Value`'s `=`
rather than declaring one; they decline anything else, and `Value`'s
`ifFail:` block answers instead. Neither walk recurses or calls back into
Newspeak.

The last two are the first *reflective* leaves the mirror carries: given an
object, how many slots its class's application declares, and what the slot
at an index holds. They exist because the Newspeak fallback has to read
state from a receiver it cannot send a private selector to, but they are
the same two questions an object mirror asks
([liveness-and-reflection.md](liveness-and-reflection.md)), and the mirror
work is meant to reuse them rather than declare a second pair. What they do
not yet answer is a slot's *name*, which `Value` does not need and a mirror
does; that is the shape to extend when it is built.

`Value` is where §3.1.1's "a value object is globally unique, in the sense
that no other object is equal to it" is written down. It declares `=` as a
forwarder to `value:sameAs:ifFail:`, `hash` to `valueHash:ifFail:`, and the
identity method `==` as `^self = other` — an ordinary method, since §3.1.1
requires an identity method a value's class declares to "return the same
results as `Value`'s equality method", and a send of `=` makes that true by
construction for every subclass. Where a leaf declines — a slot whose class
brings its own `=`, a shape it does not handle, its node budget — the
failure block runs `valueEquals:` or `valueHashOfSlots`, which walk the
slots from Newspeak and send `=` or `hash` to each. Those two read a slot
through `valueSlotCountOf:` and `valueSlotOf:at:`, which name their subject
rather than forwarding `self`: a private member is reachable only through a
lexical send (§5.9), so the *other* side's state cannot be read by sending
it a private selector, and both sides have to go through the same leaf.
Naming the subject is also what costs those two the primitive-method fast
path, which only the declining path pays.

**A value's state is the slots its classes *declare*, and nothing else.**
An instance's field vector holds more than that: each level lays its own
slots down at its `own_slot_base` and then one hidden cache slot per nested
class declaration ([object-model.md](object-model.md), "The nested-class
accessor"), so the declared slots of a chain are interleaved with hidden ones
rather than forming a prefix. A nested class's cache slot is not state — §6
gives every instance a class object of its own for each nested declaration,
so two equal values hold *different* objects there as soon as each has been
sent its accessor, and counting the slot made them answer `false` at exit
status 0. Which indices are state is decided once, when the application is
built: `MixinApplication::declared_slots`, computed in `apply_mixin` beside
`nested_slot` and `inherits_value` because nothing about an application
changes afterwards. Everything that asks borrows that one list through one
wrapper — both Rust walks, and the `slotCount` and `slotAt:` leaves the
Newspeak fallback reads a slot through. That is what keeps the leaf and the
fallback comparing the same slots in the same order — if they disagreed, `=`
would answer one thing when the leaf decided and another when it declined —
and it is why the declining path, which asks once per slot read, allocates
nothing to ask.
`goldens/kernel/a_values_nested_class_is_not_part_of_its_state.ns` pins it,
including the case where only one of the two instances has been touched and
a class that *inherits* the declaration from a value superclass while
declaring a slot of its own after it.

**What `==` on a value now costs.** It used to be one primitive method:
`Object`'s `==`, whose leaf compares two handles and answers. It is now an
*activation* of `Value>>==`, which sends `=` — a second dispatch, to a
primitive method whose leaf walks the two values' state — and, where that
leaf declines, a third activation for `valueEquals:` and a send of `=` per
slot below it. So the cheapest case is two sends and a state walk where it
was one send and a pointer comparison, and the declining case is Newspeak
code in a loop.

That is not a cost worth removing by hand: §3.1.1's agreement rule leaves
no cheaper answer that is also the right one, since a value has no identity
for `==` to report. It is a cost worth *recording*, because it is on the
path of every `==` a program writes about a number, a string or a symbol —
the common shapes decide inside the leaf, without unfolding anything, and
the later tiers are where it is meant to come back: `Value>>==`'s body is
one send, exactly the shape an inliner folds into its callee, and the leaf
is a primitive a JIT can emit inline for a receiver whose type feedback
says `SmallInteger`. Until then, a program that compares immediates in a
hot loop pays two dispatches where Milestone 1 paid one. **The measurement
does not bound this**: none of the eight benchmark sources sends `=` or
`==` in its loop, which was recorded before measuring
([execution.md](execution.md), "What `Top`, `Value` and `Number` cost"). A
source that does is what would show it, and there is none yet.

**§3.1.1's conditions are enforced, and where.** "An object is a value
object if it is deeply immutable ... and all objects reachable from it are
also deeply immutable" is two separate checks, both refusing rather than
silently allowing a value that is not one:

- **A mutable slot below `Value` means no instance.** `Mutator::apply_mixin`
  computes `inherits_value` and the first `mutable_value_slot` of the whole
  chain once, when it builds the application;
  `instantiate::instantiate_class` refuses before it allocates anything, and
  names both the class asked and the class that declares the slot, since a
  subclass inherits the fault. §6.3.2's immutable slot is what makes this
  checkable at all: a value class may declare `| held = … |`, never
  `| held ::= … |`.
- **A slot of a value may hold only a value.** `Instruction::InitializeSlot`
  asks `instantiate::check_value_slot` on every store into an instance whose
  class inherits `Value`; anything that is not itself a value — an `Array`,
  an ordinary instance, a class object — is refused with the slot named.
  Deep immutability is then transitive by induction: whatever a value's slot
  holds was itself checked when it was made.

**What is still unchecked.**

- **A value class nothing instantiates is accepted.** Both refusals sit
  where an instance comes into being, not where the class does. A class that
  inherits `Value` and declares a mutable slot is a defect the moment it is
  written, and it is reported the moment something asks for an instance —
  never before. The alternative was making class creation fallible, which
  would turn the public `load_module` into a fallible API at every call site
  that builds a class, to report a fault that cannot yet have done anything.
- **A ready-made instance decoded from an image is not re-checked.** The
  decoder rebuilds an instance's slots from the artifact's bytes without
  going through `InitializeSlot`, so a crafted image can hand back something
  whose class inherits `Value` while a slot holds an `Array`. That is a
  wrong answer for a crafted artifact, not a crash: the decoder stays
  panic-free, and the derivation declines any shape it cannot rule on rather
  than guessing ([image-format.md](image-format.md), "Verification", on what
  the two passes do and do not guarantee).
- **`=` and `hash` are expected to agree, not checked to.** A subclass that
  overrides one and not the other breaks the contract silently, as it does
  in every Smalltalk.
- **Identity is not globally unique.** §3.1.1's "globally unique, in the
  sense that no other object is equal to it" is met for `=` and `==`, which
  the derivation makes state-based, but two equal `String`s remain two
  distinct objects to the collector. Canonicalization is not required for
  the derived equality and was left out deliberately.
- **`1 = 1.0` and `1.0 = 1` still disagree**, as they did before: each is
  its own class's `=`, and the missing numeric tower is what would
  reconcile them ("Deviation: `i64` integers" and "Gap: an integer has no
  `/`" above).
The builtin value classes' own immutability rests on there being no mutating
leaf for a `String`, a `Symbol` or a `Character` — their state is a Rust
payload, not slots, so neither check above is about them. And the actor
boundary that gives all of this its purpose does not exist
([liveness-and-reflection.md](liveness-and-reflection.md), "Value objects
exist; the boundary they cross does not").

The leaves are the 78 rows of `builtins/primitive.rs`'s `MIRROR` table. A row
names the **mirror selector** (`collection:at:put:ifFail:`), the **reported
selector** — the kernel selector every error text of that leaf names,
`at:put:` — and the `Primitive` it runs. Every mirror selector ends in
`ifFail:`; its arguments are the subject (the kernel method's `self`), the
leaf's own arguments, and the failure block. `Array` and `OrderedCollection`
share rows 56–60, as they share their payload.

A minimal `platform` object exposes the builtin classes, a `collections`
namespace and a stdout stream object, enough for transcript-style output.

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
format's builtin-class table for no behavioural gain. All three objects'
selectors are methods of the one kernel class `Platform`, and a selector sent to
the wrong one of them is a leaf that declines: the method's failure block sends
the message to `super`, which is `Object`'s, and ends in `doesNotUnderstand:`
([dispatch.md](dispatch.md), "A primitive can fail"). They keep the shared
class, since `platform transcript class` answering `Platform` is observable.

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

## Gap: an integer has no `/`

`7 // 2` answers `3`; `7 / 2` ends in `doesNotUnderstand:`. `Kernel.ns`
declares `/` only on `Float`, forwarding to the mirror row
`float:divide:ifFail:`, while `Integer` declares `//` alone, over
`integer:floorDivide:ifFail:`. So `7.0 / 2` works and `7 / 2` does not, which
is asymmetric enough to surprise.

**This is not a specification violation.** The specification mentions `Number`
exactly twice — §3.1.1, listing it among the value objects, and §5.1.1, on
non-integral *literals* — and fixes no arithmetic protocol anywhere. Which
selectors `Integer` answers is a library question, and §8.3 leaves the
platform's modules to provide classes "some of which are standard and mentioned
in this specification". `/` is not one of them. Formally this is the on-demand
rule doing its job: nothing on `corpus/runnable.toml` asks for it.

**Step 6's first plan left it open deliberately** (decided with the user on
2026-09-21): moving the library into `Kernel.ns` kept every recorded outcome,
and adding `Integer>>/` would have been a new one. The second plan's `Number`
class did not settle it either: `Number` is declared, empty, as the place
between `Value` and `Integer`/`Float` that §3.1.1 names, and **it is where
the numeric tower goes when there is one** — what `Integer` and `Float`
share moves up to it, and a `Fraction` would hang beside them. Declaring it
empty first is what makes that a later addition rather than a later
restructuring. **The question still has to be settled**, and the choice is as
it was. Primordial Soup declares both on
`Integer` (`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`):
`/` as a primitive falling back to `^other divideFromNumber: self`, and `//`
beside it, commented "Integer division, rounded toward negative infinity".
That works there because Soup has a `Fraction` class, which its comparison
methods test for. ns-next has no `Fraction` and no exact decimal type, so
writing `Integer>>/` into `Kernel.ns` means choosing: answer a `Float`, which
diverges from Smalltalk and from Soup and quietly makes an exact quotient
inexact; build the numeric tower the deviation above already defers; or keep
the selector absent and say so. What was decided is only that it has to be
settled eventually, deliberately rather than in passing; when, and by which
step, is not.

## The core library as Newspeak source over a VM mirror

The shape of a real collections library, of arbitrary-precision integers, and of
the mirror-based reflection protocol the platform would hand out are all open;
the last of those is sketched in
[liveness-and-reflection.md](liveness-and-reflection.md). Beyond that, "widen
when a whitelisted module demands it" still governs what gets added.

**Step 6 also carries the rest of §3.2** (decided 2026-09-18): `Class`,
`Behavior` and `Metaclass` become declared kernel classes rather than objects
hand-built in Rust, and a class object's own class becomes its metaclass. That
is its third plan, not built yet. Both references declare exactly these three
in their kernel source
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`,
`prior-art/newspeak/KernelForJS.ns`); the reflection work right after this
step cannot start without them — a mirror's test for a metaclass is
`Metaclass = classOf: reflectee`, which needs the instance-of relation
reified. The flat metaclass hierarchy that §3.2 also requires did *not* wait
for this step: it was corrected ahead of 3c, because the primary factory's
special cases depended on it ([object-model.md](object-model.md), "Class
sides and metaclasses").

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
fast path instead. No native calls back into Newspeak: each leaf that runs a
block answers it as a trampoline step instead of calling it:

| Selector | How it runs its block |
|---|---|
| `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `ifTrue:`, `ifFalse:`, `and:`, `or:` | Lowered by the compiler for a literal block, with the kernel method kept as the guarded fallback. The leaf is a trampoline step: it returns the block to activate instead of calling it, with a continuation for what `and:` and `or:` do with its value ([execution.md](execution.md), "The trampoline") |
| `whileTrue:`, `whileFalse:` | The same, both halves: the leaf is a trampoline step that returns the condition block, and its continuation activates the body or ends the loop |
| `value`, `value:`, … | A trampoline step that returns the block to activate: activating a closure is what the activation stack does. See [execution.md](execution.md) |
| `do:` | A trampoline step: it answers the block for the first element, and its continuation activates it for each next one ([execution.md](execution.md), "No re-entry from Rust"). `Array>>do:` is a kernel method forwarding to it; a Newspeak loop is "Planned: real bodies for control flow and `do:`, before the JIT tiers" below |

So no native calls back into Newspeak, which the root discipline needs as well:
no Rust frame may be live below a safepoint ([memory.md](memory.md), "No Rust
frame is live below a safepoint"). Everything else the mirror holds — `size`,
`at:`, `class`, `hash`, `printString`, the arithmetic, the comparisons,
`asString`, `asSymbol` — is a leaf, and its computation stays in Rust.

**Every builtin method is a real method** (the user's direction, 2026-09-12;
the mirror decided 2026-09-21), one that lookup, mirrors and the debugger see.
The computation of a leaf did not move; what changed is that it is reachable
as a method, and that Rust is reached through exactly one object. That is what
lets a debugger step into `do:` and edit-and-continue reach library code
([liveness-and-reflection.md](liveness-and-reflection.md)), and what gives the
JIT something to work with: every send inside library code gets a call site
of its own, whose inline cache records what flows through it, where a Rust
native records nothing. Library code is shared by every caller, so those sites
are often megamorphic — `action value:` inside a Newspeak `do:` would see
every block in the program — and the feedback pays off once the JIT inlines
library methods into their callers, not from the caches alone.

### The VM mirror is the whole Rust boundary

**§8.3.1 settles the shape, and it is not a matter of taste.** "The VM mirror
supports the set of operations traditionally implemented as primitives in
Smalltalk. *There is no syntax for a primitive call in Newspeak. A primitive
call is a message send to the VM mirror.*" The caveat below it calls the
canonical implementation's practice of reaching primitives through Squeak
"a bug".

So Primordial Soup's shape is the one ns-next does *not* take, although an
earlier version of this document named it as the target. Soup marks a kernel
method with a primitive pragma in a comment and carries a Newspeak fallback
body for when the primitive fails — which is precisely the primitive-call
syntax §8.3.1 rules out, written into a comment because the grammar has no
place for it. SOMns takes the other shape, and it is the one required and the
one built: `prior-art/somns/core-lib/Kernel.ns` declares
`class Kernel vmMirror: vmMirror = Object` and writes each kernel method as a
send to the mirror — `public + argument` answers
`vmMirror int: self add: argument`, `public == other` answers
`vmMirror object: self identicalTo: other`. The receiver is passed as an
argument, because the mirror is always the receiver.

**The mirror is the one instance of a builtin class, `BuiltinClass::VmMirror`,
whose method table carries every leaf as an enumerated primitive.** The
machinery of step 3a is kept whole: a row is a `MethodEntry::Primitive`
carrying `Primitive::Mirror(row)`, found by the ordinary lookup
([dispatch.md](dispatch.md), "Primitives"). The builtin classes keep their
identity — a value still needs a class for lookup and for its dispatch id —
while their behaviour is `Kernel.ns`. SOMns builds its mirror the same way
(`src/som/vm/ObjectSystem.java`, `constructVmMirror`): an object without
fields over a synthetic class holding the primitives. The mirror's class and
its table are built once, at bootstrap, from the same static table the
compiler reads, which is what lets the compiler bind a mirror send to its row
([compiler.md](compiler.md), "Mirror sends").

**A mirror is a capability**, which is the second reason to prefer it and the
one §8.2 gives for mirror-based reflection generally. Whoever is not handed
the mirror cannot call a primitive. Under a pragma, any method anywhere can
claim one and nothing marks where the VM surface lies. The bootstrap creates
the mirror and hands it to exactly one factory, `Kernel vmMirror:`; every
other module would receive it only if `Kernel` or a configuration module
passed it on, and none does. Six things keep it that way, and loosening any
one of them lets a program claim every primitive:

- Only `resolver::resolve_kernel` records `Binding::MirrorSend`, the mark the
  compiler's recognizers read, and only `Kernel.ns` is resolved with it; a
  user module goes through `resolve`, so copying the kernel's text gets an
  ordinary send to whatever that module's own `vmMirror` holds.
- The bootstrap checks what the resolver cannot: that `Kernel`'s root class
  declares exactly one slot, the immutable `vmMirror`, and that after
  instantiation it holds the mirror (`interpreter::kernel`).
- No program can name the mirror's class: a bare name resolves only to the
  builtin classes `instantiate::builtin_kind_from_name` lists, which omit it,
  and the image decoder refuses builtin class tag 14 wherever a program names
  a class ([image-format.md](image-format.md), "Verification").
- The image encoder refuses an image whose graph reaches the mirror.
- `decide_factory` refuses to instantiate the mirror's class, so there is
  never a second mirror.
- `SendMirror`, the instruction that runs a leaf with no lookup, is emitted
  only for a `MirrorSend` and has no wire tag, so no artifact can carry one.

`platform.rs` keeps its separate role: §8.3 distinguishes the platform object,
which is the environment, from the VM mirror, which is the virtual machine.
Loading a module belongs on the mirror, as it does in SOMns, not on the
platform.

**The cost the design accepted, and what recovers it.** Through the mirror a
leaf send is a send to the kernel method, an activation, a lexical send of
`vmMirror`, the mirror send and the return, where it was one cached send.
SOMns carries this because Graal optimizes it away; ns-next has no JIT yet.
Three things take it back out. The integer leaves are inlined under a type
guard (`Instruction::IntegerBinary`, [compiler.md](compiler.md), "Arithmetic
inlining"), built first for exactly this reason. A kernel method whose whole
body is a mirror send is a primitive method, whose leaf `act` runs without
activating it ([dispatch.md](dispatch.md), "Primitive methods"). And inside a
kernel method the mirror send's failure block is lowered inline
(`Instruction::SendMirror`, [compiler.md](compiler.md), "Mirror sends"). For a
later JIT the mirror's two inlining steps are the easiest case an inliner
meets: `Integer>>+` is a one-expression body, and the send to the mirror is
monomorphic by construction, since the mirror is a single object — its guard
folds away once the first step is inlined. That is Strongtalk's ordinary
procedure, not an edge case.

### The shape of a kernel method

```newspeak
public + other = ( ^vmMirror integer: self add: other ifFail: [^super + other] )
```

Every kernel method is this: the mirror selector, with `self` and then the
method's own parameters in order, and a literal failure block that resends the
same message to `super`. The failure path is an explicit `ifFail:` block
rather than something the runtime does implicitly (decided with the user on
2026-09-21): a real send to the mirror, as §8.3.1 requires, whose failure
path is visible Newspeak code, as Cog's method body is. SOMns uses the same
shape for its I/O primitives (`prior-art/somns/core-lib/Files.ns`,
`vmMirror file: self readAt: pos ifFail: errf`).

The one rule `[^super …]` reproduces what a declining native used to do by
itself: a receiver of the wrong shape — an instance of a user subclass of
`Array`, which carries no array, or a platform object sent another platform
object's selector — reaches the superclass's method, `Object`'s, and a
selector `Object` does not have ends in `Object`'s `doesNotUnderstand:`
through §5.10's super failure path, with the same message text.
`goldens/errors/native_that_fails_on_its_receiver_falls_to_objects` and
`goldens/cache/cached_primitive_that_fails_on_a_receiver_of_the_same_class`
pin it, and `goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods`
pins it family by family. `Object`'s own leaves never decline, so its
blocks are unreachable; they keep the shape so that one rule, and one
recognizer, covers every method. Every method is `public` but `Object`'s
`doesNotUnderstand:`, which §5.5 makes `protected`.

Two things about the shape are enforced rather than conventional:

- **A literal block may appear in a mirror send only as its final `ifFail:`
  argument.** `interpreter::kernel`'s `check_kernel_shape` refuses anything
  else at bootstrap (`check_mirror_send_arguments`). The reference lowering
  marks a mirror send whose failure block the optimized lowering inlines as a
  `SendInlinable`, and that mark covers the whole send, so it would exempt
  from the depth count a literal block the leaf itself activates —
  `boolean:ifTrue:ifFail:`'s — which the optimized lowering counts; the two
  lowerings would then overflow at different depths ([compiler.md](compiler.md),
  "How the limit counts"). `Boolean>>ifTrue:` therefore passes its parameter,
  which neither lowering marks.
- **A cascade to `vmMirror` is not a mirror send.** The resolver marks only a
  plain explicit-receiver send, so a cascade compiles as ordinary sends to the
  mirror object — which still run their rows, sent, with a real failure block
  ([compiler.md](compiler.md), "Mirror sends"). `Kernel.ns` uses none.

### The bootstrap: placeholders first, kernel second

SOMns's procedure transferred directly
(`src/som/vm/ObjectSystem.java` pulls each nested mixin definition out of the
compiled kernel module by name and initializes a pre-existing class object
with it). `Interpreter::bootstrap_with` runs it in six steps
(`interpreter/kernel.rs`, `install_kernel`):

1. **Placeholders exist.** `World::new` builds a class object for every
   `BuiltinClass`, each over a placeholder mixin with an empty method table,
   because the runtime must be able to answer what class an integer literal
   has at any moment. `BuiltinClass::ALL` and `Mutator::builtin_class` are
   that table, and `trace.rs` reaches it. A new builtin is appended to
   `BuiltinClass::ALL`, never inserted, because its order is also the wire tag
   order (`image::encode::builtin_class_tag`). `VmMirror`'s class, whose mixin
   already holds the `MIRROR` rows, and its one instance are built here too.
2. **`Kernel.ns` is compiled but not instantiated.** Its root mixin and its
   nested mixins are built with `Mutator::build_mixin`, compiled under the
   interpreter's lowering like any module, and `check_kernel_shape` checks
   what the phase relies on: one nested class per builtin, named as
   `kernel_class_name` says, each with no slots and the default factory; the
   one immutable `vmMirror` slot on the root; a superclass clause on every
   class but `Top` that is a bare sibling name, read and never run, naming a
   kernel class, from which the chain reaches `Top` without repeating; that
   only `Object` names `Top`, as §3.2 allows; and the literal-block rule
   above. A kernel that breaks it is a defect in the binary, and panics.
3. **Each placeholder gets its kernel application**, in `fill_order`, which
   sorts the kinds by how many superclass links separate them from `Top`, so
   a superclass is always filled before its subclasses. `BuiltinClass::ALL`
   is no longer that order and cannot become it again: a class added later is
   appended, because its position is a wire tag, so `Value` and `Number` sit
   at the end of `ALL` and in the middle of the hierarchy. Each class gets
   the nested mixin named like it, applied to the application of the class
   its clause names — `Top`'s to none, `Object`'s to `Top`'s, and so on down
   — installed with `Heap::set_class_application`, which restamps the
   dispatch id of the few objects that exist this early, and the class
   object's superclass link set to match. `Value`'s application is recorded
   as it is filled rather than after the loop, because every class below it
   asks `apply_mixin` whether its superclass application is that one.
   `VmMirror` keeps its own Rust mixin, re-applied over the filled `Object`.
4. **The immediate dispatch-id table is refreshed**, because an immediate's
   ids are read off its class's application, which just changed.
5. **The kernel class is built and instantiated** from the host, with the
   mirror as its factory argument: `Kernel vmMirror: theMirror`. The
   instantiation is an ordinary host run, so the stress modes see it.
6. **The instance is wired in.** Every builtin class's `enclosing` becomes the
   kernel instance and its `declaring_class` the kernel class, so a kernel
   method's lexical send of `vmMirror` reaches the slot; each hidden
   nested-class slot of the kernel instance is filled with its builtin class,
   so the nested-class accessor `Array` inside kernel code answers the builtin
   `Array` rather than building a second class object of the same mixin; and
   the instance is kept as `Mutator::kernel`, a root beside the mirror
   ([memory.md](memory.md), "What the collector traces"). The phase checks
   that the slot holds the mirror, and that no metaclass was built before this
   point, since a metaclass copies its class's lexical context.

The resolved kernel is parsed and resolved once per thread and kept
(`thread_local!`): the test suite bootstraps an interpreter well over a
hundred times, and the AST and bindings are immutable. Each interpreter still
compiles it, because compiled code is numbered against one heap's inline
caches.

**A builtin mixin carries the factory `new`, with no arguments.** A kernel
class declares no factory, so its mixin has §6's default, and
`decide_instantiation`'s arity check applies to it like any class's: a host's
`Interpreter::instantiate` of a builtin class with factory arguments is
refused ("wrong number of factory arguments"), where a placeholder, which
declared no factory at all, let any count through. `Array new` and
`OrderedCollection new` still build their own payload
(`instantiate::decide_factory`).

`Kernel` is declared like any other module, `= Object new (…)`, where SOMns
writes `= Object <: Value`. The `Value` class exists here now, so what is
missing is only the syntax: `<:`, a mixin application in a superclass
clause, is not parsed ([README.md](README.md), deviations). Nothing depends
on it. §6.4 makes a module *definition* a value object with nothing
declared, and §3.1.1's conditions on its *instances* exist for passing
objects across actor boundaries, which ns-next has none of
([liveness-and-reflection.md](liveness-and-reflection.md), "Value objects
exist; the boundary they cross does not").

### Where the library source lives, and how modules are found

The library is source the runtime ships, compiled at bootstrap as the Rust
bootstrap used to be built, so running a program from source is unchanged:
the bootstrap gained a phase in front of it and nothing above that changed.
Loading several *user* modules is a separate, later step.

**Planned: the module namespace on the mirror.** For the modules there are,
the namespace belongs on the mirror (decided with the user on 2026-09-21): a
primitive that looks a module up in a table the CLI filled in advance, rather
than a namespace builtin of its own or an invented factory convention.
Nothing would touch the file system at run time, which is what linking
statically means here. Later the same primitive falls back to the file system
on a miss, so dynamic loading extends one primitive and changes neither
`Kernel.ns` nor anything else on the Newspeak side. Nothing loads a second
module yet, so the primitive belongs with the step that does.

**Start-up was measured, not assumed: the kernel costs about 0.5 ms** —
`ns-next run` of a module that answers at once, 4.07 ms against 3.56 ms
before the move ([execution.md](execution.md), "What the core library's move
cost"). Adding `Top`, `Value` and `Number` and `Value`'s seven methods to it
did not move that measurably, which is the figure to watch as the library
grows ([execution.md](execution.md), "What `Top`, `Value` and `Number`
cost"). That does not call for embedding the compiled kernel as an image,
which stays the next step if the kernel grows until it does, and the
comparison must then include what `image::load`'s two verification passes
cost, since loading is not obviously cheaper than compiling. If it is
embedded, it is generated at build time into the build script's output
directory and included from there, never checked in: a checked-in image that
nobody regenerated decodes into a different, perfectly valid kernel and runs
it silently, which is the failure mode the wire-tag rule exists to prevent.
Embedding does not conflict with [image-format.md](image-format.md)'s "never
for the builtins", because that decision is about what an artifact arriving
from outside may carry; bytes compiled into the binary are as trustworthy as
the Rust beside them.

### What this step did not break

The goldens are what made the move verifiable: `Integer>>+`,
`Boolean>>ifTrue:` and the rest moved from Rust to Newspeak, and every golden
stayed byte-identical in `status`, `stdout` and `stderr` under both
lowerings — the reference lowering running uncached as the oracle — the gates
with them: `parse-corpus` at 210/0/1/0, `run-corpus` at ran 6 / failed 0 /
skipped 205, and all three stress modes. The one difference the move makes is
observable only through a program no golden had recorded, below.

`crates/ns-cli/src/commands/build.rs`'s `root_class_answers`, which refuses a
build whose entry selector the root class does not answer, used to walk past
the root level on the assumption that a builtin level held nothing but
primitives. With the builtin classes' methods in `Kernel.ns` that would have
accepted a selector only the core library answers — absent from the image's
symbol table, which interns only the module's own declarations — instead of
refusing the build. The walk now stops at the first builtin level, and
`build_refuses_an_entry_only_a_builtin_answers`
(`crates/ns-cli/tests/build_load.rs`) pins it.

### The one observable difference

**A builtin subclass's own `doesNotUnderstand:` is passed by, as §5.10 says**
(decided with the user on 2026-09-22). When a leaf declines on an instance of
a user subclass of a builtin, the kernel method's failure block sends
`super …`, which reaches `Object`; a selector `Object` does not have then ends
in "the `#doesNotUnderstand:` method defined for S" (§5.10) — `Object`'s,
since S is the superclass of the class declaring the `super` send — and not
in a handler the subclass declares. Before the move a declining native passed
its level over and the *receiver's* own lookup continued, which reached that
handler.

It follows from the explicit failure block, and nothing short of the
rejected implicit pass-over could keep the old answer: Newspeak has no send
that continues a public lookup above a given level. No golden had pinned the
old behaviour. `goldens/kernel/a_builtin_subclass_handler_is_passed_by_as_section_5_10_says`
pins the new one: `Handled = Array new`, declaring
`doesNotUnderstand:`, answers `Handled new size` with `Object`'s error, not
with `'handled here'`.

### The ordering, and what it cost

It is step 6 of M4's order of work ([PROGRESS.md](../PROGRESS.md)), after the
caches, closure conversion and the collector, for the reason below, and it
did not change the image's decision to keep the builtins out of every artifact
([image-format.md](image-format.md), "How upstream compares"): the library is
compiled from source the runtime ships.

**The ordering was the opposite of the obvious one.** A library in Newspeak
costs an activation and a mirror send per leaf where a native cost one send,
and before the inline caches every one of those sends paid a lookup; so the
move came *after* the caches, and after the inlined integer fast path that
was sequenced to cushion it. What Milestone 4 owed this section was only that
it not deepen the investment in Rust callbacks while it rewrote the code
around them; step 2a took the last callback away, as a trampoline step, which
maps directly onto the leaf the kernel's `Block>>value` forwards to.

**Measured, the move cost nothing measurable**, against `e2bb31e`, the
commit before it, over the eight sources in two sittings: with primitive
methods −4.5% to +2.1%, no group entirely above in either sitting. Without
them (`NS_NEXT_NO_PRIMITIVE_METHODS=1`, the same binary) the three sources
that send a leaf the fast path does not absorb in their loop were clearly
slower — `bench_megamorphic.ns` +23.6%, `bench_block.ns` +21.3%,
`bench_do.ns` +5.5% — which is what the primitive methods recover. See
[execution.md](execution.md), "What the core library's move cost", for the
tables and what is left unexplained.

**Giving it the specification's object model cost about as little**,
measured the same way against the commit before it: seven of the eight
sources level, `bench_new2.ns` 3.6% slower and recorded as an open finding
with the change that would recover it
([execution.md](execution.md), "What `Top`, `Value` and `Number` cost").
That measurement also found that `bench_block.ns` spreads 16% between builds
of the same tree, which is a fact about the protocol rather than about this
work.

### Planned: real bodies for control flow and `do:`, before the JIT tiers

Would change: `Boolean`'s conditionals, `and:`, `or:`, `Block`'s
`whileTrue:` and `whileFalse:`, and the collections' `do:` become real
Newspeak loops and branches instead of forwarders to today's leaves.

**Kernel method bodies forward; they do not reimplement** (decided with the
user on 2026-09-22). The control-flow methods and `do:` are forwarders over
the trampoline-step leaves, as mirror primitives, so `do:`'s loop is still
opaque: nothing can inline its block or compile the loop as a unit. Rewriting
them must happen **before the JIT tiers** — they are what the tiers need to
see through — and pays off **only with one**: before a JIT a Newspeak loop is
more sends and activations than the leaf it replaces, and costs.

The step owns four semantic decisions, each of which the leaves settle today
and a Newspeak body would have to settle again, visibly:

- **A non-block argument is taken as its own value.** The conditionals,
  `and:` and `or:` activate an argument that is a block and take any other
  argument as the value it stands for, and `and:` and `or:` then coerce that
  value by `builtins::boolean::truthy`'s rule — only exactly `true` is true —
  so `true and: three`, with `three` holding `[3]`, answers `false`
  (`goldens/language/variable_and_block_coerces_its_value_to_a_boolean`). A
  Newspeak body that sends `value` to its argument needs `Object>>value`
  answering `self` for the non-block case — which would be a new, observable
  selector every object answers, not a refactoring — and must keep the
  coercion.
- **`do:` iterates over a copy.** `Heap::array_elements` clones the payload
  when the loop starts, so an `add:` inside the block does not extend the
  iteration; a Newspeak loop over `size` and `at:` would see it.
- **`do:` with a non-block argument is a no-op** that answers the receiver
  (`builtins::collections`, `do_step`), rather than sending `value:` to it.
- **`whileTrue:` with a non-block body re-runs the condition**: the leaf
  treats a body that is not a block as its own value and loops on the
  condition alone (`builtins::block`).

## Decisions

- **Leaf primitives are native Rust; anything that calls back into Newspeak is
  not**, because a Rust callback is a permanent barrier to optimization. This
  narrows the earlier "native Rust rather than Newspeak source" decision rather
  than reversing it: the reasons for that decision — artifact size and the trust
  boundary — apply to leaves, which are almost all of the library.
- **A leaf is bound as an enumerated primitive, not a function pointer**, so
  a later JIT tier can emit it inline. It is a method-table entry of the VM
  mirror's class, `public` like every row.
- **The core library is Newspeak source over a VM mirror**, as SOMns's is, so
  that every builtin method is a real method: step 6 of M4's order of work
  ([PROGRESS.md](../PROGRESS.md)), after the caches, closure conversion and
  the collector, and before the JIT tiers, which need the library as real
  Newspeak code to optimize.
- **The VM mirror is the whole Rust boundary** (decided with the user on
  2026-09-21): one object holding every leaf, handed to `Kernel.ns`, and
  nothing else crosses into Rust. §8.3.1 requires it — "There is no syntax for
  a primitive call in Newspeak. A primitive call is a message send to the VM
  mirror" — so Primordial Soup's primitive pragma, which this document
  previously named as the target, is the shape the specification rules out,
  and the specification calls the practice behind it a bug. A mirror is also a
  capability, per §8.2: whoever is not handed it cannot call a primitive.
- **The inlined fast path for leaves came before the library moved**, not
  after. It is independent of what the slow path looks like — the compiler
  emits the operation under a type guard and falls back to the ordinary send —
  so it was measured on its own and then cushioned the move.
- **`Kernel` is declared without a `Value` mixin application.** §6.4 already
  makes a module definition a value object; §3.1.1's instance-side condition
  serves actor boundaries, which do not exist here, so the value semantics go
  with the actor work rather than with this step.
- **Modules are to be linked statically, through a namespace on the mirror**
  (decided with the user on 2026-09-21, not built): the CLI fills a table in
  advance and a mirror primitive looks names up in it, so nothing touches the
  file system at run time. Dynamic loading later extends that one primitive
  rather than changing the Newspeak side.
- **The platform grows only against a whitelisted module**, so every capability
  has a test that would notice it breaking.
- **`i64` with an error on overflow**, rather than silent wraparound or a bignum
  library nothing yet needs.

The move itself implements nine decisions, each taken with the user on the
date given
([plan](../superpowers/plans/2026-09-22-0107-ns-next-kernel-over-vm-mirror.md)):

1. **A mirror primitive's failure path is an explicit `ifFail:` block,
   inlined by the compiler** (2026-09-21) — "The shape of a kernel method"
   above; [compiler.md](compiler.md), "Mirror sends".
2. **`Integer>>/` stays absent**, and the gap stays recorded as something to
   solve (2026-09-21) — "Gap: an integer has no `/`" above.
3. **Every builtin family moves** (2026-09-21), `do:`, `whileTrue:` and
   `value` included.
4. **The arithmetic fast path's `Jump` saving stays a separate step**
   (2026-09-21) — [compiler.md](compiler.md), "Arithmetic inlining".
5. **Primitive methods** (2026-09-22): a kernel method whose whole body
   forwards `self` and its own parameters to one mirror row runs the leaf
   without an activation, recognized only while compiling `Kernel.ns`, under
   both lowerings — [dispatch.md](dispatch.md), "Primitive methods".
6. **The fast path was built early, with a measurement-only switch**
   (2026-09-22), `NS_NEXT_NO_PRIMITIVE_METHODS=1`.
7. **Kernel method bodies forward; they do not reimplement** (2026-09-22) —
   "Planned: real bodies for control flow and `do:`, before the JIT tiers"
   above.
8. **`True` and `False` are subclasses of `Boolean`** (2026-09-22), as both
   references declare them.
9. **A builtin subclass's own `doesNotUnderstand:` is passed by, per §5.10**
   (2026-09-22) — "The one observable difference" above.

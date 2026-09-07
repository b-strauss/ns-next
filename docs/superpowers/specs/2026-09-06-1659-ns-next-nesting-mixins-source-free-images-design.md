# ns-next — Nested Classes, Mixins, and Source-Free Images, Milestone 3 Design

Status: approved for planning
Date: 2026-09-06
Author: Benjamin

## Summary

Milestone 3 makes `ns-next` run Newspeak's defining construct: classes
nested inside other classes, accessed as members of the enclosing object.
It replaces the Milestone 1 class model with the three-layer model the
language specification requires — a shared **mixin** per declaration, a
cached **mixin application** per (mixin, superclass) pair, and a distinct
**class object** per enclosing instance — and it makes the image format
follow, which turns the image source-free.

It also closes the two Milestone 1 instantiation gaps that nesting
exposes (the superclass clause's message is never evaluated; every
inheritance level runs in the subclass's activation), stops discarding
class-side declarations, and fixes the CLI entry point, which today sends
a one-argument factory message to every module regardless of what its
primary factory declares.

This milestone deliberately precedes inline caches and the garbage
collector (now Milestone 4). The reason is measurement, not taste: the
runtime currently executes six fixtures and no corpus module at all, so
an inline cache built now would be tuned against monomorphic toy code
whose call sites do not resemble real Newspeak, where every module is a
nest of classes reached through platform indirection.

## Goals

1. **Nested classes with the specification's semantics**, including the
   lazily created, per-receiver-cached class object, the per-instance
   enclosing object, and the fact that a nested class's superclass is not
   known at compile time.
2. **Class-side declarations are kept** and dispatch as the class's
   metaclass behaviour, instead of being parsed and dropped.
3. **The superclass clause is evaluated**, and each inheritance level's
   initializer runs in its own activation with its own arguments.
4. **A source-free image**: a mixin table and a symbol table replace the
   embedded source text and the re-parse on load.
5. **A running measurement**: a curated `run-corpus` gate that executes
   real corpus modules on both engines, and a `--bench` mode that gives
   Milestone 4 a before-number to beat.
6. **Both engines still agree byte for byte**, with no fixture exempt.

## Non-Goals

- The garbage collector, the arena/handle object-model rewrite, inline
  caches, and every JIT tier. Those are Milestone 4 and later.
- The full `Class` / `Metaclass` hierarchy of the language spec's §3.2.
  A class's metaclass is created lazily and carries the class side; it is
  not a subclass of a reified `Class`, and metaclasses are not reachable
  as ordinary objects yet.
- A platform library beyond what the curated corpus whitelist forces.
  Growth here is demand-driven, one module at a time.
- The explicit mixin application operator `<:`. `parser/decl.rs` records
  that `class C = Super <: Mixin (...)` is unimplemented and that no
  corpus file uses it; adding it is a frontend change, and runtime work
  does not touch `ns-syntax`. The mixins this milestone introduces come
  implicitly from every class declaration, which is the semantics the
  language spec's §6 requires; `<:` is only its explicit spelling.
- The decompiler. It stays open work — see the image section for how this
  milestone stays honest without it.
- A numeric tower, and the Milestone 1 corpus-parse deferral (`expr -1`).
  Both are unrelated to nesting.

## The language specification's model, and why Milestone 1's is wrong

Milestone 1 stored `enclosing: Option<ObjRef>` on `ClassData` and never
loaded a nested class, so no class object ever had one. That storage
decision was right and its consequence was never drawn: an enclosing
object is per-instance state, so a class that has one cannot be shared
between instances of its enclosing class.

`newspeak-spec.pdf` §6 states this directly:

> Every instance of a class C has a distinct nested class for each nested
> class declaration given in the declaration of C and its superclasses.
> [...] each nested class has one piece of state — its enclosing object,
> and so there must be a distinct class per instance of the enclosing
> class.

and gives the accessor's semantics:

> A declaration of a nested class N nested directly within another class
> OC implicitly defines a method with the same name and accessibility in
> OC. The first time this method is invoked on a given receiver r, it
> evaluates the class declaration, using r as the enclosing object, and
> returns the resulting class as its result. Subsequent calls on r always
> return the same class object.

§2.3 shows why the superclass cannot be resolved statically. A nested
class declaration can be overridden by a slot in a subclass of its
enclosing class, and that slot can be assigned at run time:

```newspeak
class ChildOfOuter = Outer () (
    public class Nested = Inner () ()
)
class GrandChildOfOuter = ChildOfOuter( | public Inner ::= String. |) ()

g Nested.                (* a subclass of String, not of Inner *)
(g Inner: Array) Nested. (* still the same subclass of String *)
```

The spec draws the conclusion for us: "all nested classes must be
compiled as mixins, as the superclass cannot be reliably known at
compilation time."

The sharing axis is named in §6.2: "Any change made to the structure or
code of a mixin affects all of its applications. [...] they all share
their code and structural definition with the mixin." That sentence is
both the performance strategy and the edit-and-continue requirement.

## Object model

Three layers, replacing `ClassData`'s single one:

```rust
struct Mixin {                      // one per declaration, shared for its lifetime
    name: Symbol,
    methods: HashMap<Symbol, Rc<ast::MethodDecl>>,
    instance_slots: Vec<Rc<ast::SlotDecl>>,
    init_exprs: Vec<ast::Stmt>,
    factory: Option<Rc<ast::MessagePattern>>,
    nested: Vec<Rc<Mixin>>,         // the nested declarations, for their accessors
    class_side: Option<Rc<Mixin>>,
}

struct MixinApplication {           // (mixin, super application) -> behaviour
    mixin: Rc<Mixin>,
    superclass: Option<Rc<MixinApplication>>,
    slot_index: HashMap<Symbol, usize>,   // absolute layout, inherited slots included
    num_slots: usize,
}

struct ClassData {                  // one per enclosing instance, per level
    application: Rc<MixinApplication>,
    enclosing: Option<ObjRef>,
    declaring_class: Option<ClassRef>,  // the declaration chain `outer` walks
    superclass: Option<ClassRef>,   // the class-object chain, parallel to the applications
    builtin: Option<BuiltinClass>,
    metaclass: Option<ClassRef>,    // built on first class-side send, cached here
}
```

**The application cache is keyed by (mixin identity, superclass
*application* identity), never by the superclass class object.** If the
superclass is itself a nested class, its class object is per-instance, so
a key built from it would never hit and every instance would get its own
behaviour. Keyed by the superclass's application, all instances whose
mixin chains agree share one behaviour object.

Two parallel chains result, and both are needed:

- The **class-object chain** carries identity and, at every level, that
  level's enclosing object plus the class object of the level that
  lexically declares it (`ClassData::declaring_class`).
  `dispatch::kth_enclosing_object` walks the *declaration* chain, seeded
  from the activation's `Frame.current_class`: a lexical depth counts
  levels out from the class that defines the running method, which is not
  the receiver's class once a method or a nested declaration is inherited
  across a nesting boundary.
- The **application chain** carries code and layout, and is shared.

`slot_index`, `num_slots` and method lookup move off `ClassData` onto
`MixinApplication`, because a superclass contributes slots and can differ
between instances of the same enclosing class.

**Inline caches (Milestone 4) must guard on the application, not on the
class object.** Two `Inner` instances from different `Outer` instances
have different class objects, as the spec requires, but the same
application, so the call site stays monomorphic. A cache entry records
(application → method, defining level *k*); reaching that level's class
object for `outer` and `super` starts at the defining level's own class
object and steps *k* levels out along the declaration chain, and *k* is 0
for an ordinary send. The Milestone 2 IC slots stay
inert this milestone, but their intended key is recorded here so
Milestone 4 does not re-derive it and pick the obvious wrong one.

## Loading, the accessor, and instantiation

**Module loading becomes two-stage.** `load_module` builds the *mixin
tree* — root declaration, every nested declaration, every class side —
and nothing else. That is pure structure with no evaluation, so it stays
total. Only the root class is then created, as the application of the
root mixin to its superclass, with `enclosing = nil` (§3.3: a module
definition's enclosing object is nil, which is why a module has no state
of its own).

**The implicit accessor** is defined for every nested declaration, with
the declaration's own name and accessibility. Its body:

1. Read the hidden slot reserved for this declaration in the receiver's
   layout. If it is not nil, return it — this is §6's "subsequent calls
   on r always return the same class object".
2. Otherwise evaluate the superclass clause **in the receiver's context**,
   yielding a class object; take its application.
3. Look up (mixin, that application) in the application cache, creating
   the application if absent.
4. Allocate the class object `{application, enclosing: r, superclass}`.
5. Store it in the hidden slot and return it.

Caching in a hidden slot rather than a side table is what §6 describes
("the class is effectively cached within r") and is a slot read on the
fast path instead of a hash lookup.

**Inherited nested declarations get their own slots.** §6 says a distinct
nested class exists for each declaration "given in the declaration of C
*and its superclasses*", so the hidden slots are part of the absolute
layout an application computes, contributed at the level that declares
them — exactly like ordinary inherited slots. They are derived from a
mixin's `nested` list rather than declared, so nothing stores them
separately.

**The override behaviour of §2.3 needs no special case.** Because the
accessor is an ordinary method, a slot declaration in a subclass
overrides it under the normal lookup rules, and reassigning that slot
later cannot disturb an already-cached nested class. That this falls out
of the existing dispatch is evidence the layering is right.

**The accessor lives in `interpreter/`**, as shared orchestration. It
evaluates Newspeak code (the superclass clause), so it runs that code
through the same body-execution hook `instantiate` already uses. It is
not synthesized bytecode: a compiler-side accessor would have to be
mirrored in the tree-walker, and the two would drift in exactly the way
the project's shared-orchestration invariant exists to prevent.

**The superclass clause is evaluated at last.** Milestone 1 walked
`ClassData.superclass` upwards and ran each level's slots and initExprs
in the *subclass's* activation with the subclass's factory arguments.
Per §6.2, the instance initializer evaluates the superclass clause's
message, invokes the superclass's initializer on the same object with
*those* arguments, and only then runs its own mixin's initializer.
Milestone 3 implements that: one activation per level, deepest first,
each with its own arguments.

**Class sides become metaclass behaviour.** A class side is the mixin for
the class's metaclass (§6.3). The metaclass is created lazily, on the
first class-side send or reflective access, so a class with no class side
costs nothing.

**The CLI entry point stops guessing.** `cmd_run` today instantiates
every root class with `usingPlatform: platform` — one argument — which is
why `ADummyPush`, whose primary factory is `new`, fails with "wrong
number of factory arguments: factory takes 0, got 1". The entry point
reads the root class's declared primary factory pattern and sends what it
declares.

**Platform surface grows only on demand**, driven by the curated corpus
whitelist below. A module that needs a capability nobody has asked for is
a reason to widen the whitelist deliberately, not to build a speculative
library.

## Engines

**Access to a nested class needs no new instruction.** The resolver
already treats a class name as a member name, so a nested-class reference
is an ordinary send or a `LexicalSend` with a depth, and the compiler
lowers it as one. The work of this milestone is in the object model and
the loader, not in `compiler/`.

**Compiled code is keyed by mixin, not by class.** `bytecode.rs`'s
`class_key` derives identity from a `ClassRef` today; with a class object
per enclosing instance that would recompile the same method body per
instance, contradicting §6.2's "they all share their code and structural
definition with the mixin". `class_key` becomes a mixin id, and `disasm`
addresses code through mixins.

**`Frame.current_class` stays a class object**, because `outer` and
`super` need the per-instance chain. Dispatch therefore returns the level
at which it found the method, alongside the method itself.

**The three engine hooks are unchanged**, and the differential invariant
is unchanged: every fixture runs under both engines with identical exit
status, stdout and stderr, no exemptions, and the tree-walker is the
reference when they disagree.

## Image format

**A symbol table at the head.** Every selector and name once, referenced
by index everywhere else. Selectors are the format's most repeated
strings — name-based virtual dispatch puts the same selector at very many
call sites — so this shrinks the image and turns interning at load time
into one pass over the table instead of one `Symbol::intern` per send
instruction.

**A mixin table** replaces the class table an earlier draft proposed: one
row per declaration, carrying the name, its own slot names, the primary
factory selector and arity, the method table (selector → code id), the
ids of its nested declarations with their accessor names and
accessibility, the class-side mixin id, the initializer code id, and the
provenance span.

**Applications are not serialized.** Layout and lookup follow
deterministically from a mixin and its superclass application, so they
are rebuilt as class objects decode. This removes a section, a
verification pass, and the possibility of loading an image whose stored
layout disagrees with its mixins.

**Class objects are ordinary objects in the graph**, encoded as
`{mixin id, superclass, enclosing object, declaring class}` — the
declaring class because `outer` walks the declaration chain, not the
receiver's dynamic one, so an image that dropped it would come back
resolving `outer` from the wrong chain. This retires the binary
`builtin`/`UserRoot` class tag, whose own comment in `image/encode.rs`
records that a second user-defined class would encode as `UserRoot` and
decode as the root — a wrong class rather than a decode error. Nesting is
exactly what would have triggered it. The tag keeps its first two cases,
which name classes the graph does not create (a builtin, and the module's
root class), and gains a third that names an object-table record.

**The source leaves the load path.** `load` decodes the mixin table
instead of re-running parse → resolve → bootstrap → `load_module`, and
`LoadedImage.bindings` disappears with it. The front end is no longer on
the load path, and with it the re-parsing of attacker-controlled text.

**An optional source blob stays**, behind a header flag, for the liveness
spec's debug variant. This is what keeps the milestone honest while the
decompiler does not exist: the requirement "an image can be exported back
to `.ns`" is met today by the debug variant, and by the decompiler when
it is written.

**`FORMAT_VERSION` is not bumped** (decision, 2026-09-06). The project
has no external users, so images that predate this change are disposable
and no migration path is owed to them. The guarantee that still has to
hold is the untrusted-input invariant, and it does not come from the
version field: an old or corrupt image must fail through the structural
verification passes as an `Err`, never a panic or an abort. Validating
every id, tag, count and length before it indexes, allocates or slices is
what provides that, unchanged.

**Verification gains one genuinely new hazard.** The mixin tree's nesting
depth is input-controlled, so building it is iterative or depth-limited —
precisely the "bound any recursion whose depth the input controls" case.
Symbol ids, mixin ids, class-object references, slot counts and arities
are all validated before first use.

**`Frame.pc` gets its discriminator.** `pc` means a statement index under
the tree-walker and an instruction index under the VM, with nothing on
the frame saying which (`types.rs`). Frames are serialized, so this is a
format-visible ambiguity and is resolved here rather than inherited.

## Testing and gates

**Fixtures follow the specification's own claims**, and run under both
engines automatically because `differential.rs` covers every
`tests-fixtures/*.ns`:

- `x Inner == x Inner` — the accessor caches in the receiver.
- `(a Inner == b Inner) not` for two instances of the enclosing class —
  the spec requires distinct class objects. Spelled with `==` and `not`
  rather than `~~`, which `Object` has no native for.
- The §2.3 override case, including that `Nested` keeps its already-cached
  superclass after `Inner` is reassigned.
- A superclass clause with arguments and side effects; one initializer
  activation per inheritance level, each with its own arguments.
- `outer` at depth > 1, a class-side method, and deep nesting.

**A `run-corpus` gate**, built like `parse-corpus` and reported the same
way in `CLAUDE.md`. A curated whitelist — `corpus/runnable.toml`, beside
the existing `corpus/blacklist.toml` — names the modules that must run,
each with its expected exit status and stdout, and the gate runs them
under both engines. The whitelist grows deliberately; a silent regression
in what runs is what it exists to prevent.

**Image round-trips test the property, not just the shape.** Beyond
encode-decode equality: two instances that shared an application before a
snapshot must share one after it, or the rebuild is wrong and Milestone
4's call sites would go megamorphic after every load. Identity hashes and
`enclosing` chains are checked, and a golden-bytes fixture pins the wire
format — the Milestone 2 deferral, landed here rather than at a version
bump that will not happen.

**Untrusted-input tests are part of the milestone, not an afterthought**:
a truncated mixin table, out-of-range ids, and deeply or cyclically
nested mixin trees, each of which must produce an `Err` and never a
panic.

Also corrected here: `the_code_table_encoding_is_deterministic` proves
nothing today, because both encodes share one `HashMap` instance. It
compiles the module twice from now on.

The standing gates are unchanged: `cargo test --workspace`, a
warning-free `cargo clippy --workspace --all-targets`, `cargo fmt --all`,
and `parse-corpus` at 210/0/1/0.

**A `--bench` mode closes the milestone**, with warm-up-aware counters
and the tree-walker-versus-bytecode microbenchmark that Milestone 2
deferred twice. Milestone 4 is the inline-cache milestone, and without a
before-number no claimed improvement is verifiable.

## Milestone renumbering and documentation sweep

This milestone becomes **M3**; inline caches and the GC become **M4**;
the JIT tiers become **M5–M6**. Every `Milestone 3` reference in the
specs and in code comments moves to its new meaning in the same change.
The completed plans under `docs/superpowers/plans/` are execution records
of finished work and stay as they are; `docs/PROGRESS.md` records the
renumbering once.

Two sentences committed to the liveness spec earlier today argue from a
`FORMAT_VERSION` bump — "this costs one format break rather than two" and
the golden-bytes fixture "coupled to the first `FORMAT_VERSION` bump".
Both are corrected in the same sweep: the symbol table is included on its
own merits, and the golden-bytes fixture lands with this milestone.

## Decisions

1. **Language completeness before the GC and inline caches** — the
   runtime executes six fixtures and no corpus module, so optimization
   work now would be tuned against code that does not resemble Newspeak.
2. **Success is measured by a curated `run-corpus` gate**, not by the
   whole corpus and not by fixtures alone. The alternative, making the
   189 `usingPlatform:` modules run, requires a platform library and is
   its own milestone.
3. **The full break to a source-free image happens here.** Keeping the
   re-parse path would mean making it nested-class-aware and
   class-side-aware, which is more work than removing it.
4. **`FORMAT_VERSION` is not bumped**, because the project has no
   external users; the untrusted-input guarantee is independent of it.
5. **The three-layer model, not shared class objects.** An earlier draft
   of this design proposed moving the enclosing pointer onto the instance
   so class objects could be shared. The language spec forbids it: the
   nested class *is* the state, and its superclass can differ per
   instance. The performance goal is met instead by keying behaviour on
   the mixin application.
6. **The mixin application cache is required, not deferred.** Without it
   the semantics are wrong or every call site is megamorphic; there is no
   version of this milestone in which it is optional.

## Open questions

- The **decompiler** that is to provide "export back to `.ns`" for a
  source-free image. Unchanged from the liveness spec, and mitigated here
  by the optional debug source blob.
- **How far the metaclass model goes.** This milestone gives a class a
  lazily created metaclass carrying its class side. The spec's §3.2 model
  — every class an instance of a unique metaclass, metaclasses as direct
  subclasses of `Class` — is not built, and what reflection will need
  from it is not yet known.
- **Whether an application cache entry must be weak.** Applications are
  shared and cached globally; nothing collects them today because nothing
  collects at all. Milestone 4's collector has to decide whether an
  application whose classes are all unreachable is itself collectable.
- **`frame.pc` on every instruction.** The discriminator is settled here;
  whether the VM must store `pc` at every instruction rather than only
  where control can leave the loop is a debugger and profiler question,
  and belongs to the milestone that builds them.
- **`Mixin` has no interior mutability.** Editing a live class's method
  table re-applies a fresh mixin; existing instances keep the old
  application, and their instances go on running the old code. §6.2
  requires an edit to a mixin to affect all of its applications, so the
  milestone that builds `MirrorBuilder` and edit-and-continue must choose
  between interior mutability on `Mixin` and a mixin-versioning scheme —
  and, either way, decide what happens to the `applications` cache, which
  today grows without bound because every re-application mints a fresh
  entry that nothing ever evicts.
- **`Interpreter::send` never consults `dispatch::lookup_slot`.** A public
  slot's implicit accessors are reachable only through an ordinary,
  unqualified send, not through `send` with an explicit receiver — so the
  language spec's own example spelling, `(g Inner: Array) Nested`, does
  not work today. Fixing it means adding slot entries to `lookup_public`'s
  walk, deciding setter-selector naming and its precedence against a
  same-named method, and enforcing a slot's access modifier, none of
  which exists yet.
- **An entry send's return value is never printed.** `report_entry_send`
  in `crates/ns-cli/src/main.rs` reports only failure (an uncaught
  Newspeak error, or a non-local return escaping the entry send); on
  success it discards the returned `Value`. A module whose result *is*
  its output — rather than something it prints via `transcript show:` —
  is silently unobservable through `ns-next run`.
- **`Object::class` is not an `Option<ClassRef>`.** The iterative `Drop`
  impl (`crates/ns-runtime/src/types.rs`) relies on that to unwind an
  object graph without recursing, and the image decoder refuses an image
  in which a class object's own class is another record rather than the
  bootstrap `Object` placeholder. That matches the runtime's own
  invariant today (every class object's `.class` is the bootstrap
  `Object`), but it is a decision that must be revisited if §3.2's
  reified `Class`/`Metaclass` hierarchy is ever built, since that model
  makes a class object's class a real, distinct object.
- **`nil printString` answers `an Object`.** `nil`'s class has no
  `printString` override of its own, so the send falls through to the
  generic `Object` builtin (`crates/ns-runtime/src/builtins/object_class.rs`)
  instead of answering something that identifies `nil`.

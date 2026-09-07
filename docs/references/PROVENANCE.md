# Reference Papers — Provenance

Primary sources behind `ns-next`'s design decisions, vendored so the specs can cite
something that will still be there. All were downloaded on 2026-09-06 from the authors'
own pages.

**These are third-party works and are not covered by this repository's MIT license.**
Several were published by the ACM, Springer or the IEEE and remain under their
respective copyrights; the copies here are the author-hosted versions. They are included
for reference while working on this project. Redistribute them only under whatever terms
their publishers grant.

| File | Work |
|---|---|
| `newspeak-spec.pdf` | Gilad Bracha. *Newspeak Programming Language Draft Specification*. <https://newspeaklanguage.org/spec/newspeak-spec.pdf> |
| `newspeak-modules.pdf` | Gilad Bracha, Peter Ahe, Vassili Bykov, Yaron Kashai, William Maddox, Eliot Miranda. *Modules as Objects in Newspeak*. ECOOP 2010, Maribor; Springer LNCS. Best paper award. <https://bracha.org/newspeak-modules.pdf> |
| `dyla-final.pdf` | Gilad Bracha. *On the Interaction of Method Lookup and Scope with Inheritance and Nesting*. 3rd ECOOP Workshop on Dynamic Languages and Applications, 2007. <https://bracha.org/dyla-final.pdf> |
| `mirrors.pdf` | Gilad Bracha, David Ungar. *Mirrors: Design Principles for Meta-level Facilities of Object-Oriented Programming Languages*. OOPSLA 2004; Most Influential Paper award, 2014. <https://bracha.org/mirrors.pdf> |
| `newspeak.pdf` | Gilad Bracha, Peter Ahe, Vassili Bykov, Yaron Kashai, Eliot Miranda. *The Newspeak Programming Platform*. Overview as of May 2008. <https://bracha.org/newspeak.pdf> |
| `objectsAsSoftwareServices.pdf` | Gilad Bracha. *Objects as Software Services*. Whitepaper accompanying the invited talk at the Dynamic Languages Symposium, OOPSLA 2005. <https://bracha.org/objectsAsSoftwareServices.pdf> |
| `newspeak-exemplars.pdf` | Gilad Bracha. *Enhancing Liveness with Exemplars in the Newspeak IDE*. <https://newspeaklanguage.org/pubs/newspeak-exemplars.pdf> |
| `modularity-meets-inheritance.pdf` | Gilad Bracha, Gary Lindstrom. *Modularity Meets Inheritance*. IEEE International Conference on Computer Languages, April 1992. <https://bracha.org/modularity-meets-inheritance.pdf> |

## Why each one is here

- **`newspeak-spec.pdf`** — v0.106, the authoritative source for language *semantics*
  (name resolution, dispatch, access control, instantiation), where the vendored
  `corpus/reference/NewspeakGrammar.ns` covers only *syntax*. It is explicitly a draft
  and says so in its own front matter, and it can disagree with the reference
  implementation: **where the spec and the vendored corpus's actual behaviour
  disagree, prefer the corpus** and note the discrepancy. It is not corpus material: it
  has its own source and release cadence, independent of the `newspeaklanguage/newspeak`
  repository the corpus is copied from.

  Refreshing it: re-download from the URL above, update the version here, and re-check
  that `docs/superpowers/specs/2026-09-01-2252-ns-next-runtime-design.md`'s section
  citations still point at the same content — section numbers can shift between spec
  versions.
- **`dyla-final.pdf`** — the paper behind Milestone 1's three access-control-aware dispatch
  algorithms. It is about exactly the hard part: what a name means in a dynamic language
  with nested classes and inheritance.
- **`newspeak-modules.pdf`** — why a module declaration is a parameterized, stateless
  *value* (`class X usingPlatform: platform = …`) and why there are no globals. This is
  what makes an application object serializable as a unit upstream.
- **`mirrors.pdf`** — reflection as a capability handed out by the platform rather than a
  property of every object. The liveness spec's decision that a deployed image stays
  reflective, and that inspection is prevented by withholding the mirror rather than by
  stripping metadata, rests on this.
- **`objectsAsSoftwareServices.pdf`** and **`newspeak.pdf`** — deployment and platform
  context for the image-format work.
- **`newspeak-exemplars.pdf`** — liveness in the IDE, background for the liveness spec's
  debugger and mirror requirements.
- **`modularity-meets-inheritance.pdf`** — the mixin-composition theory Newspeak's class
  model is built on. Background today; load-bearing once nested classes and mixins land.

## Not vendored, but cited by the specs

- **Robert Nystrom, *Crafting Interpreters*** — <https://craftinginterpreters.com/>. The
  implementation-technique reference, and the only one here that is not about Newspeak.
  Part III, *A Bytecode Virtual Machine*, is the closest published walkthrough of the
  ground `compiler/` and `vm.rs` cover: compiling straight to a flat instruction stream,
  constant tables, call frames, closures, and the dispatch-loop and inline-caching
  techniques that decide whether a VM is fast. Read it for *how* and the Newspeak papers
  above for *what*: clox is a stack machine with globals and single inheritance, ns-next
  is a register VM with no globals and access-control-aware lookup, so the data
  structures carry over but the semantics do not. Linked rather than vendored: the book
  is commercially published, and the freely readable web edition above is its canonical
  form.
- Bracha's blog *Room 101*, in particular
  [An Image Problem](https://gbracha.blogspot.com/2009/10/image-problem.html) and
  [Serialization Killer](https://gbracha.blogspot.com/2010/02/serialization-killer.html).
- The Newspeak implementation at <https://github.com/newspeaklanguage/newspeak>.
- **Primordial Soup** — <https://github.com/rmacnak/primordialsoup>. Ryan Macnak's
  Newspeak VM, running on Android, Fuchsia, Linux, macOS, Windows and the web. This is
  the reference *implementation* of everything the image-format work has to be measured
  against, in the way the papers above are the reference for semantics, and it is cited
  directly rather than paraphrased because the blog posts alone leave the mechanism
  ambiguous. The parts that carry design weight here:

  - `docs/design.md` — the snapshot format. Clustered serialization (objects grouped by
    class, all nodes encoded before all edges) rather than a memory dump; the **object
    store**, an array at the root of the graph holding every object the VM must reach
    without a name lookup; and the statement that running state is *not* persisted —
    activations are not in snapshots, and activation objects materialize on demand when
    a mirror asks for one. It also records that the object store and the current
    activation are the only GC roots.
  - `vm/object.h` — the object store's actual field list, which is what makes "what a
    VM must be able to name by identity" concrete: the singletons, the selectors the VM
    itself sends (`doesNotUnderstand:`, `cannotReturn:`, `dispatchMessage`, …), and the
    classes with VM-known formats.
  - `vm/main.cc` and `vm/isolate.cc` — startup. The VM maps the snapshot and posts an
    isolate message; the jump into Newspeak takes its selector from the object store, so
    the VM names no application entry point of its own.
  - `newspeak/HelloApp.ns`, `newspeak/CompilerApp.ns`, `newspeak/Runtime.ns` — the
    deployment convention: a module is packaged with `packageUsing: manifest` and
    serialized having never seen a platform, which arrives later as `main:args:`'s first
    argument. `CompilerApp.ns` also shows packaging itself, including the `keepSource:`
    switch.

  Linked rather than vendored: it is an actively developed source tree under its own
  license, and pinning a copy here would rot without telling anyone.

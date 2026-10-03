# Reference Papers — Provenance

Primary sources behind `ns-next`'s design decisions, vendored so the design
documents can cite something that will still be there. All were downloaded on 2026-09-06 from the authors'
own pages.

**These are third-party works and are not covered by this repository's MIT license.**
Several were published by the ACM, Springer or the IEEE and remain under their
respective copyrights; the copies here are the author-hosted versions. They are included
for reference while working on this project. Redistribute them only under whatever terms
their publishers grant.

| File | Work |
|---|---|
| `newspeak-spec.pdf` | Gilad Bracha. *Newspeak Programming Language Draft Specification*. <https://newspeaklanguage.org/spec/newspeak-spec.pdf> |
| `newspeak-spec.md` | A transcription of the above, for searching. Same work, same copyright. |
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

  Refreshing it: re-download from the URL above, update the version here, re-make the
  transcription below, and re-check that `docs/design/`'s section citations still point
  at the same content — section numbers can shift between spec versions.
- **`newspeak-spec.md`** — the same document as Markdown, so that a question about the
  language can be answered by searching the *whole* specification in one step. That is
  what it is for: the answer to a question is often in a section whose heading does not
  look related, and reading only the section that seems topical is how a rule gets
  missed. Before stating that the specification says nothing about something, search
  this file.

  It is a transcription, not a summary: the wording is the PDF's, and no paragraph is
  left out. It was produced from the PDF's text spans, which is what makes it faithful
  where a plain text extraction is not — the PDF's fonts say which words are Newspeak
  code, which are emphasized and which are headings, so code is fenced, emphasis is kept
  and the section structure is real rather than guessed. Ligatures are normalized so
  that a search for "defined" finds it, quotation marks and apostrophes are the plain
  ASCII ones throughout rather than the typographic pairs the faces render them as, and
  every word the typesetter broke across two lines was rejoined, each decision checked
  by hand.

  Four classes of defect had to be found and fixed, and they are worth knowing about,
  because each is invisible to an ordinary text extraction and the first was reported by
  a reader rather than caught by a check:

  - **Characters the fonts render from the wrong slot.** A type annotation's angle
    brackets arrive as inverted marks, Newspeak's caret as a modifier circumflex, the
    message metavariable's Greek mu as a micro sign, and every quote as a curly one.
    They are mapped back. One glyph is ambiguous: the left single quote is both an
    opening quotation mark and Newspeak's namespace separator, and only its neighbours
    tell them apart — between two word characters it is the separator, as in
    ``Past`Future``, and anywhere else a quote, as in the grammar's `'"'`.
  - **Characters that are not text at all.** The sans face has no underscore, so the
    typesetter draws a short rule on the baseline and leaves a space in the text. Every
    extraction of this document therefore loses all three underscores, including the one
    in `underscore → "_";`. They are recovered from the page's drawings.
  - **Content dropped by a filter that was too broad.** The table of contents is
    recognized by its dot leaders, and the body sets an ellipsis the same way, so seven
    rules of the specification proper — the string literal's equivalence, both tuple
    literal forms, the cascade and the statement sequence among them — were being
    dropped. The filter is now confined to the pages the contents are on.
  - **Structure the fonts alone do not give.** The page geometry decides the rest, and
    each rule had to be found against the rendered pages: a vertical bar is set in the
    symbol face, so `[:p | body]` fell out of its code span in three pieces; the second
    level of a list is marked with a dash and the sub-items of a numbered step with a
    bullet one step further in, so both were flattened; a paragraph set back at the
    margin after a list was pulled into the last item; a run-in heading is a wholly
    bold line sharing a baseline with the text it opens, which a paragraph merely
    beginning with a bold word is not.

  It was then compared word by word against an independent `pdftotext` extraction: about
  99% of the words match exactly, and every difference was reviewed. They fall into four
  groups — the labels drawn inside the figures, which this file leaves out along with
  the figures themselves; places where the two extractions read the page in a different
  order, such as the bibliography's labels; places where `pdftotext` is wrong and this
  file is right (`def iningClass`, `lookupP ublic`, dropped prime marks and every
  underscore); and three places where the PDF's own text layer omits a space between two
  words, which are repaired here. That comparison is necessary but not sufficient: both
  extractions read the same text layer, so it is blind to anything that is not in it,
  which is how the underscores survived it. What caught the rest was rendering pages as
  images and reading them.

  **The source's own mistakes are reproduced, not corrected.** The specification is a
  draft and says so, and it carries the slips of one: doubled words ("in order to to
  define", "the the execution of the send", "a class body is is defined", "via a a
  mirror"), misspellings ("faciliate", "flexiblility", "instantation", "we coud give
  warnings"), a missing word ("A method defines an action be executed"), a number that
  does not agree ("A top level expressions is executed"), an "it's" for an "its", and one
  grammar production written with `::` where every other uses an arrow. They are all in
  the PDF, and finding them here is evidence the transcription is faithful rather than
  that it is careless. Correcting them would make this file say something the
  specification does not.

  **The PDF remains the authority.** Cite sections as before (`newspeak-spec.pdf` §6.3);
  this file is how to find them, not a second source. If the two ever disagree, the PDF
  wins and the transcription is wrong and should be fixed.
- **`dyla-final.pdf`** — the paper behind Milestone 1's three access-control-aware dispatch
  algorithms. It is about exactly the hard part: what a name means in a dynamic language
  with nested classes and inheritance.
- **`newspeak-modules.pdf`** — why a module declaration is a parameterized, stateless
  *value* (`class X usingPlatform: platform = …`) and why there are no globals. This is
  what makes an application object serializable as a unit upstream.
- **`mirrors.pdf`** — reflection as a capability handed out by the platform rather than a
  property of every object. The decision that a deployed image stays
  reflective, and that inspection is prevented by withholding the mirror rather than by
  stripping metadata, rests on this.
- **`objectsAsSoftwareServices.pdf`** and **`newspeak.pdf`** — deployment and platform
  context for the image-format work.
- **`newspeak-exemplars.pdf`** — liveness in the IDE, background for
  `docs/design/liveness-and-reflection.md`'s debugger and mirror requirements.
- **`modularity-meets-inheritance.pdf`** — the mixin-composition theory Newspeak's class
  model is built on. Background today; load-bearing once nested classes and mixins land.

## Sources cited by the design documents but not vendored here

Nothing below is vendored — no code or text from any of it is checked into this
repository. They divide by whether they are nevertheless reachable from the
working tree.

### Reachable only by following the link

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
### In the tree, as git submodules under `prior-art/`

Each is pinned to one commit and none is built, tested or linked — a plain clone
does not even fetch them (`git submodule update --init <name>`, by name: one of
them is ~255 MB). What each is for, the commit it is pinned at and its license
are in `prior-art/PROVENANCE.md`, which also explains why the canonical Newspeak
implementation appears in this repository twice and why neither copy may be
deleted. The file paths cited below are therefore real paths in the working
tree once the submodule is initialized, not places to go looking upstream.

Four of the ten are described here part by part. The other six are cited by
documents under `docs/design/` too and are covered by
`prior-art/PROVENANCE.md`, which describes all ten: `prior-art/num-bigint/`,
`prior-art/num-integer/` and `prior-art/num-rational/` for the large-integer
arithmetic adapted from the first two and the `Fraction` the third models
(`core-library.md`, "Large integers"); `prior-art/somns/` as a
point of comparison throughout and as the source of the Are We Fast Yet
benchmarks in `benchmarks/are-we-fast-yet/` (`docs/design/execution.md`, "The
Are We Fast Yet benchmarks"); `prior-art/newspeak-benchmarks/` as a suite to
measure the JIT tiers against (`execution.md`, "Planned: JIT tiers"); and
`prior-art/strongtalk/` for its inline and lookup caches, its memory
management and its `become:` (`dispatch.md`, `memory.md`, `object-model.md`).

- **The canonical Newspeak implementation** — `prior-art/newspeak/`, from
  <https://github.com/newspeaklanguage/newspeak>. The reference for what the
  language does where the draft specification above is silent or ambiguous, and
  the tree the `corpus/` gate material was flattened out of at an earlier commit.
- **Primordial Soup** — `prior-art/primordialsoup/`, from
  <https://github.com/rmacnak/primordialsoup>. Ryan Macnak's
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

- **Nova** — `prior-art/nova/`, from <https://github.com/trynova/nova>. A JavaScript
  engine in Rust, and the one entry here that is not about Newspeak at all. It is the
  model `docs/design/memory.md` names for its typed handles, which ns-next first
  had as an arena of indices and now has as offsets into its object space, and
  the comparison it weighs its root discipline against. Rust is the whole reason
  it is the comparison rather than V8: the borrow checker is available as an
  enforcement mechanism, and a C++ engine's conventions do not carry over to a
  solution built on one. MPL-2.0, the most restrictive license of the ten. The parts
  the design documents cite:

  - `GARBAGE_COLLECTOR.md` — the `GcScope`/`NoGcScope` lifetimes and `Scoped` roots,
    which `memory.md` rejects in favour of a gc-arena-style lifetime brand
    ("Considered and rejected: Nova's scopes").
  - `nova_vm/src/engine/context.rs` — `Bindable` and its `unbind`, which rebrand a
    handle from one lifetime to another: where `memory.md` ("The brand") locates the
    hole Nova's discipline leaves in the static guarantee, and which ns-next has no
    public counterpart to.
  - `nova_vm/src/engine/bytecode/vm.rs` — `Vm::inner_execute` checking for a
    collection at the top of the instruction loop, one row of `memory.md`'s
    comparison in "Where a collection can happen".
  - `nova_vm/src/ecmascript/execution/agent.rs` — `Agent::check_call_depth`, the fixed
    count of execution contexts `docs/design/execution.md` ("How other systems
    bound recursion depth") compares its own depth limit against.
- **The OpenSmalltalk VM** — `prior-art/opensmalltalk-vm/`, from
  <https://github.com/OpenSmalltalk/opensmalltalk-vm>. Cog and Spur, the Smalltalk VM
  lineage `docs/design/execution.md` names for activations living on a stack and
  reified into first-class contexts only when something asks for one. The design lives
  in the Smalltalk-side sources and their comments; most of the tree's bulk is
  generated C and platform build directories.

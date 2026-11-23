# The specification of ns-next

ns-next gets a specification of its own: the language it runs, the
run-time semantics, the bytecode, the image format and the invariants the
implementation keeps, written as one document with an executable model in
Lean 4 beside each rule it models. Nothing of it is built yet; this
document records what was decided and in which order it is built.

## Why

The rules ns-next keeps are spread over the design documents, the
invariants of `CLAUDE.md` and the code, each told in prose beside its
reasoning, its prior art and its mechanism. A question such as what an
install does to a removed class's objects had to be answered from five
documents at once, and every invariant of `CLAUDE.md` has already produced
a real defect. Bracha's specification (`docs/references/newspeak-spec.pdf`)
leaves parts open or marked *Unimplemented*, and ns-next has already
decided syntax it departs from ([README.md](README.md), "Deviations from
the language specification"). The specification states the rules once,
normatively; the design documents keep the reasoning and the mechanism and
point at the rules (decided with the user on 2026-11-23, before the
complete install, [liveness-and-reflection.md](liveness-and-reflection.md),
"Planned: the complete install", goes on).

## The decisions

Each decided with the user on 2026-11-23.

1. **A self-contained specification of ns-next, Bracha's document its
   source.** It defines the language as ns-next implements it, derived
   from Bracha's document where that holds and completed where it is
   silent; Bracha's document is no longer the measure, so the principle
   that a deviation from the specification is a defect
   ([README.md](README.md), "Principles") refers to this one once it
   stands. An appendix lists where it differs from Bracha's Newspeak and
   why.
2. **Five parts.** (1) The language: its lexical structure, its grammar
   and its semantics, modules, their identity and metadata included. (2)
   The run-time semantics as abstract operations: lookup and access,
   instantiation, slot access, `outer`, unwinding and exceptions,
   `become:`, the install and the mirrors, the VM mirror as a capability,
   the depth limit and what a program observes of memory. (3) The
   bytecode: the instructions, their encoding, each one's register
   effects, and the rules the decoder checks. (4) The image format: its
   structure, sections and tags, its validation and its bounds. (5)
   Separately, the VM's invariants: the internal rules no program sees
   whose breach is silent wrong output -- the write barrier, handles and
   the brand, what the collector traces, the rules of machine code --
   each naming the rule of parts 1 to 4 it protects.
3. **For the language and the run-time semantics, every current deviation
   is decided on its own.** It is adopted as a normative rule, or marked
   in the text as a gap, as Bracha marks what is *Unimplemented*; what was
   never discussed stays a marked gap and nothing is invented for it. For
   the formats the current state is the contract.
4. **Prose in fixed forms, and a mechanized, executable semantics.** The
   grammar is a PEG over tokens, with a lexical grammar beneath it; the
   semantics are named abstract operations in numbered steps, as
   ECMA-262 writes them; each instruction has a page in the frame of the
   JVM specification's chapter 6 -- operands, encoding, the registers read
   and written, outer reach, jump targets, effect, errors; the image is
   structure tables with numbered validation rules.
5. **The model is Lean 4.** It is executable, compiled to native code,
   and fit for proofs. Cedar, whose Rust implementation is tested
   differentially against its Lean model, is the precedent.
6. **The model runs every program at the source and at the bytecode
   level.** A Lean parser of its own, after the specified grammar, whose
   trees are compared with `ns-syntax`'s; a definitional interpreter of
   the source that runs the shipped core library unchanged, only the VM
   mirror's leaves modelled in Lean, the whole Rust boundary
   ([core-library.md](core-library.md), "The VM mirror is the whole Rust
   boundary"); every golden and every runnable corpus module run in it
   and held to its outcome; the image decoder's validation rules
   modelled and fuzzed against `image::load`; and a semantics of the
   bytecode, which runs the bodies the compiler emits under both
   lowerings and compares them with the source semantics -- translation
   validation per program -- checking the register invariants and each
   instruction's declared register effects on the way. Proofs, when they
   come, are made over the same executable model.
7. **One source: Verso.** A Lake project, `spec/`, at the repository's
   root holds the specification as a Verso document -- Lean's own
   documentation system, in which the Lean Language Reference is written
   -- and the model's modules. Each normative rule is a section with a
   stable tag, and its Lean definition stands beside it or is included,
   type-checked as the document builds. The grammar, the instruction
   table and the tag table are Lean data, which the document renders and
   an export writes as JSON for the Rust tests: `opcodes.rs` checked
   against the tag table, whose numbers' uniqueness is a property in
   Lean. A `cargo xtask` task builds the HTML. The design documents, code
   comments and `CLAUDE.md` point at a rule by its tag rather than state
   it again, and the transcription of Bracha's document stays under
   `docs/references/` as the source. Lean and Verso are pinned together.
   The alternatives laid out were Markdown beside a separate model, and a
   DSL of the repository's own in the manner of WebAssembly's SpecTec,
   which generates prose, formal rules and prover definitions from one
   definition; Ott and Lem, K, Sail, ecmarkup and Kaitai Struct were
   weighed as tools for one part or another.
8. **The gates are staggered, and a change keeps the specification with
   it.** Every commit that changes more than Markdown builds the Lake
   project, runs the tests of the JSON exports, and compares the Lean
   parser's trees with `ns-syntax`'s over every `.ns`; the end of a batch
   that changed the runtime or `spec/` runs a new group, `spec`, part of
   `end-of-batch`: every golden run differentially at the source and the
   bytecode level under both lowerings, and the decoder fuzzed against
   the model for a fixed time. A commit that changes observable
   behaviour, a format or an invariant changes the specification in the
   same commit. Lean is pinned by `lean-toolchain`, installed with `elan`
   on a working machine and in the gates' Docker image.

## The order of building it

Semantics first (decided with the user on 2026-11-23), each its own plan,
branch and end-of-batch gates:

1. **The foundation**: the Lake and Verso project, the toolchain, the
   `spec` gate group and the commit gate's steps, the rule in `CLAUDE.md`,
   and the document's skeleton of five parts and the appendix.
2. **The syntax**: the lexical structure and the grammar, the Lean parser
   and the comparison of its trees with `ns-syntax`'s, and the syntax
   deviations decided one by one.
3. **The language and the run-time semantics**: the source interpreter,
   the mirror leaves, every golden run differentially, and the other
   deviations decided; the install as it is.
4. **The image format**: the structure, the validation rules as a model,
   and the decoder fuzzed against it.
5. **The bytecode**: the instruction table and the register effects as
   Lean data, the bytecode semantics and the translation validation.
6. **The VM invariants**, and the design documents and `CLAUDE.md`
   pointing at the rules by tag.

The complete install ([liveness-and-reflection.md](liveness-and-reflection.md),
"Planned: the complete install") resumes after the sixth, and changes the
specification and the model as it changes the runtime.

## Open

- Which properties are proved first over the model, once it runs.
- How the specification is versioned against ns-next's own versions.

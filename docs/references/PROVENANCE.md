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

- **`newspeak-spec.pdf`** — the language `crates/ns-syntax` implements. The first place to
  check when the parser and the corpus disagree.
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

Not vendored, but cited by the specs: Bracha's blog *Room 101*, in particular
[An Image Problem](https://gbracha.blogspot.com/2009/10/image-problem.html) and
[Serialization Killer](https://gbracha.blogspot.com/2010/02/serialization-killer.html),
and the Newspeak implementation at <https://github.com/newspeaklanguage/newspeak>.

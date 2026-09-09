# Prior Art

Reference implementations, checked out as git submodules. **Nothing here is
built, tested, linked or shipped.** They are here to be read: when a design
decision in `docs/design/` needs grounding in how a working system solved the
same problem, this is where the evidence lives.

Because they are submodules, a plain `git clone` of this repository does not
fetch them. To get them:

```bash
git submodule update --init          # all four
git submodule update --init prior-art/newspeak    # just one
```

None is needed to build `ns-next`, run its tests, or pass either corpus gate.

## What each one is for

| Submodule | Upstream | Why it is here |
|---|---|---|
| `newspeak/` | [newspeaklanguage/newspeak](https://github.com/newspeaklanguage/newspeak) | The canonical Newspeak implementation, on Squeak. The reference for what the language actually does when the specification is silent or ambiguous. |
| `newspeak-benchmarks/` | [newspeaklanguage/benchmarks](https://github.com/newspeaklanguage/benchmarks) | The same benchmarks in Newspeak, Smalltalk, Dart, Java and JavaScript. Useful both as workloads and as a cross-language baseline for what a given program *should* cost. |
| `primordialsoup/` | [rmacnak/primordialsoup](https://github.com/rmacnak/primordialsoup) | A Newspeak VM in C++ by Ryan Macnak, with its own bytecode set, image format and garbage collector. The closest prior art to what this repository is building, and the most direct source of comparison for the image format and the collector. |
| `somns/` | [smarr/SOMns](https://github.com/smarr/SOMns) | A Newspeak on Truffle/Graal by Stefan Marr. The reference for the optimizing end of the design: self-optimizing interpreters, specialization, and the measurement methodology behind them. |
| `nova/` | [trynova/nova](https://github.com/trynova/nova) | A JavaScript engine in Rust. Not a Newspeak, and here for one specific reason: it is the model [memory.md](../docs/design/memory.md) names for the arena of typed handles and for the `GcScope`/`NoGcScope` root discipline. Rust is the point — a C++ engine's conventions do not carry over, because the borrow checker is available as an enforcement mechanism and their solutions do not use one. |
| `opensmalltalk-vm/` | [OpenSmalltalk/opensmalltalk-vm](https://github.com/OpenSmalltalk/opensmalltalk-vm) | Cog and Spur, the Smalltalk VM lineage. The model [execution.md](../docs/design/execution.md) names for activations living on a stack and reified into first-class contexts only on demand, and the closest thing to a reference answer for what that costs and what it complicates. |

## Pinned versions

A submodule records one commit. These are the ones this repository points at.

| Submodule | Commit | Upstream date | Added |
|---|---|---|---|
| `newspeak` | `4c8677e2c8c39bad62a85d9479bc914a13a2ee95` | 2026-09-06 | 2026-09-10 |
| `newspeak-benchmarks` | `874ba34d48a84249e1e663061a70eeb608bcf601` | 2018-01-27 | 2026-09-10 |
| `primordialsoup` | `338f0bb342a618f5608ab72ab1b1fae9b226e472` | 2026-05-03 | 2026-09-10 |
| `somns` | `e56f4c2815cef4cae2045a9b05dc1fd190315ba6` | 2019-07-07 | 2026-09-10 |
| `nova` | `4eea7c6fae180a8a2eed45c3f6acd0d9256522b1` | 2026-06-13 | 2026-09-10 |
| `opensmalltalk-vm` | `3ea6bd17fabb5aca6fcbd175185d2fccdd50d919` | 2026-09-07 | 2026-09-10 |

To move one forward: `git -C prior-art/<name> fetch && git -C prior-art/<name>
checkout <commit>`, then commit the changed submodule pointer here and update
the row above. There is no reason to keep them current on a schedule — a
reference implementation is useful at whatever commit you read it at, and a
moving pointer makes a citation in `docs/design/` unresolvable.

## `newspeak/` is here twice, deliberately

`corpus/` also contains code from `newspeaklanguage/newspeak`, and the two
copies are not redundant. They serve different purposes and are pinned
independently:

- `corpus/modules/` is a **flattened, vendored copy** of every `*.ns` file at
  commit `a76295549defe53047a65d464b63a9796d823a69`, checked into this
  repository. It is test input: `parse-corpus` and `run-corpus` run against it
  and their pinned pass counts depend on its exact contents, so it moves only
  through the refresh procedure in `corpus/PROVENANCE.md`.
- `prior-art/newspeak/` is the **whole upstream tree** at a later commit, not
  checked in, not flattened, and not touched by any gate. It carries the
  Squeak-side implementation, the class comments and the history that the
  flattened corpus drops.

Do not resolve the duplication by deleting either one. Removing the corpus copy
breaks both gates; removing the submodule leaves the implementation questions
with nowhere to look.

## Licensing

Each submodule keeps its own license, in its own tree, and none of it is
vendored into this repository — the submodule pointers are commit hashes, not
code. `ns-next`'s own license is at `LICENSE`.

- `newspeak` — Apache 2.0 for the Cadence-developed code, alongside the
  underlying Squeak system's own terms and other contributions; see
  `prior-art/newspeak/LICENSE`.
- `primordialsoup` — Apache 2.0, with the Newspeak sources in it in part
  derived from the Newspeak-on-Squeak implementation; see
  `prior-art/primordialsoup/LICENSE`.
- `somns` — MIT, Stefan Marr and Michael Haupt / Hasso Plattner Institute; see
  `prior-art/somns/LICENSE`.
- `newspeak-benchmarks` — no license file in the tree. Treat it as
  read-only reference material and do not copy code out of it without
  resolving that first.
- `nova` — Mozilla Public License 2.0; see `prior-art/nova/LICENSE.md`. Note
  that the MPL is file-level copyleft, which makes it the most restrictive
  license here for any purpose beyond reading.
- `opensmalltalk-vm` — MIT; see `prior-art/opensmalltalk-vm/LICENSE`.

Reading these to understand a design is fine. Copying code out of any of them
into `ns-next` is a licensing decision, not a technical one, and none of it has
been made here.

## Checkout size

Worth knowing before running `git submodule update --init` without an argument:
`opensmalltalk-vm` is about **255 MB** checked out, an order of magnitude larger
than everything else here combined (`newspeak` 17 MB, `nova` 8 MB, `somns`
5.7 MB, `primordialsoup` 2.8 MB, `newspeak-benchmarks` 389 KB). It is mostly
generated C sources and platform build trees, none of which is the interesting
part — the Cog and Spur design lives in the Smalltalk-side sources and the
comments. Initialize it by name when you want it rather than pulling everything.

## Nested submodules

Two of these declare submodules of their own, and none of them is initialized —
nothing here is built, so nothing needs them. `git submodule update --init
--recursive` would fetch them if you ever want them.

- `somns` — `libs/truffle`, `libs/mx`, `libs/black-diamonds`, needed only to
  build SOMns.
- `nova` — `tests/test262`, the ECMAScript conformance suite, needed only to
  run Nova's own tests.

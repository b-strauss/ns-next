# Prior Art

Reference implementations, checked out as git submodules. **Nothing here is
built, tested, linked or shipped.** They are here to be read: when a design
decision in `docs/design/` needs grounding in how a working system solved the
same problem, this is where the evidence lives.

Because they are submodules, a plain `git clone` of this repository does not
fetch them. To get them:

```bash
git submodule update --init          # all of them
git submodule update --init prior-art/newspeak    # just one
```

None is needed to build `ns-next`, run its tests, or pass either corpus gate.

`docs/references/PROVENANCE.md` is the companion to this file: it covers the
papers the design argues from, and lists the four submodules here that a
document under `docs/design/` actually cites, with the specific files in each
that carry design weight.

## What each one is for

| Submodule | Upstream | Why it is here |
|---|---|---|
| `newspeak/` | [newspeaklanguage/newspeak](https://github.com/newspeaklanguage/newspeak) | The canonical Newspeak implementation, on Squeak. The reference for what the language actually does when the specification is silent or ambiguous. |
| `eurasia/` | [newspeaklanguage/eurasia](https://github.com/newspeaklanguage/eurasia) | A Newspeak VM in Rust by Gilad Bracha, for native platforms and the web (wasm32) from one code base, which is to replace Primordial Soup as the reference implementation: V5 bytecode, a snapshot format of its own (`docs/SNAPSHOT_FORMAT.md`), and the VM-kernel contract the `newspeak` tree documents (`VM_KERNEL_CONTRACT_DESIGN_2026-11-09.md`, `BYTECODES_V5_SPEC_2026-07-13.md`). Young: its first commit is of 2026-11-17, and it moves fast. |
| `newspeak-benchmarks/` | [newspeaklanguage/benchmarks](https://github.com/newspeaklanguage/benchmarks) | The same benchmarks in Newspeak, Smalltalk, Dart, Java and JavaScript. Useful both as workloads and as a cross-language baseline for what a given program *should* cost. |
| `primordialsoup/` | [rmacnak/primordialsoup](https://github.com/rmacnak/primordialsoup) | A Newspeak VM in C++ by Ryan Macnak, with its own bytecode set, image format and garbage collector. The closest prior art to what this repository is building, and the most direct source of comparison for the image format and the collector. |
| `somns/` | [smarr/SOMns](https://github.com/smarr/SOMns) | A Newspeak on Truffle/Graal by Stefan Marr. The reference for the optimizing end of the design: self-optimizing interpreters, specialization, and the measurement methodology behind them. |
| `are-we-fast-yet/` | [smarr/are-we-fast-yet](https://github.com/smarr/are-we-fast-yet) | Are We Fast Yet, by Stefan Marr and others: fourteen benchmarks and the protocol for running them, kept in the same shape across languages, so that one program measures several language implementations. The Newspeak versions came to `benchmarks/are-we-fast-yet/` through `somns`; the Java versions here are the source of the Java counterparts beside them, which put ns-next's scores beside a JVM's. |
| `nova/` | [trynova/nova](https://github.com/trynova/nova) | A JavaScript engine in Rust. Not a Newspeak, and here for one specific reason: it is the model [memory.md](../docs/design/memory.md) names for the arena of typed handles, which ns-next now has. Its `GcScope`/`NoGcScope` root discipline is the comparison memory.md weighs and rejects in favour of a gc-arena-style lifetime brand ("Considered and rejected: Nova's scopes"). Rust is the point — a C++ engine's conventions do not carry over, because the borrow checker is available as an enforcement mechanism and their solutions do not use one. |
| `opensmalltalk-vm/` | [OpenSmalltalk/opensmalltalk-vm](https://github.com/OpenSmalltalk/opensmalltalk-vm) | Cog and Spur, the Smalltalk VM lineage. The model [execution.md](../docs/design/execution.md) names for activations living on a stack and reified into first-class contexts only on demand, and the closest thing to a reference answer for what that costs and what it complicates. |
| `strongtalk/` | [b-strauss/strongtalk](https://github.com/b-strauss/strongtalk) | The Animorphic Smalltalk system that Sun open-sourced in 2006, in a GitHub copy of the project's Google Code repository; its VM became the basis of HotSpot. The reference for the optimizing end still ahead: an interpreter whose sends carry inline caches in the bytecode (`vm/interpreter/interpretedIC.*`), polymorphic inline caches (`vm/code/compiledPIC.*`), a recompiler that inlines along the recorded receiver types (`vm/recompiler/`, `vm/compiler/inliner.*`), and deoptimization of optimized frames back into interpreted activations (`vm/runtime/vframe.*`, `DeltaProcess::deoptimize_*` in `vm/runtime/process.cpp`), which is what keeps optimized code debuggable. It is also Newspeak's ancestor: Gilad Bracha's optional type system and first-class mixins are documented under `documentation/`. |
| `v8/` | [v8/v8](https://github.com/v8/v8) | Google's JavaScript engine. The reference for a tiered JIT over a register-machine bytecode: Ignition, whose register file lives in the frame as ns-next's does (`src/interpreter/`); Sparkplug, a baseline compiler that translates bytecode one instruction at a time into machine code keeping the interpreter's frame layout and sharing its feedback vector (`src/baseline/`); Maglev and TurboFan above it (`src/maglev/`, `src/compiler/`); its macro assemblers per architecture (`src/codegen/`); and deoptimization back into interpreter frames (`src/deoptimizer/`). |
| `jdk/` | [openjdk/jdk](https://github.com/openjdk/jdk) | OpenJDK, whose VM is HotSpot, Strongtalk's descendant. The reference for the JIT tiers at production scale: the template interpreter generated at start-up by the macro assembler (`src/hotspot/share/interpreter/`, `src/hotspot/cpu/<arch>/templateTable_<arch>.cpp`), the C1 and C2 compilers (`src/hotspot/share/c1/`, `src/hotspot/share/opto/`), the code cache (`src/hotspot/share/code/`), deoptimization and its debug information at safepoints (`src/hotspot/share/runtime/deoptimization.cpp`, `src/hotspot/share/code/debugInfo*`), and the assemblers for x64 and AArch64 (`src/hotspot/cpu/x86/`, `src/hotspot/cpu/aarch64/`). |
| `num-bigint/` | [rust-num/num-bigint](https://github.com/rust-num/num-bigint) | The arithmetic `crates/ns-runtime/src/arbitrary_integer/` adapts: addition and subtraction with carries, long, half-Karatsuba, Karatsuba and Toom-3 multiplication, multiplication by the number-theoretic transform, Knuth's algorithm D, Burnikel-Ziegler division, and the radix and `f64` conversions (not its divide-and-conquer radix conversion) (MIT OR Apache-2.0; taken under MIT, notices in `THIRD-PARTY-NOTICES.md`). |
| `num-integer/` | [rust-num/num-integer](https://github.com/rust-num/num-integer) | The floor division `arbitrary_integer/` adapts (MIT OR Apache-2.0; taken under MIT, notices in `THIRD-PARTY-NOTICES.md`). |
| `num-rational/` | [rust-num/num-rational](https://github.com/rust-num/num-rational) | The model for a future `Fraction` and for exact float-to-rational conversion (`docs/design/core-library.md`, "Gap: an integer has no `/`"). Nothing is taken from it yet. |

## Pinned versions

A submodule records one commit. These are the ones this repository points at.

| Submodule | Commit | Upstream date | Added |
|---|---|---|---|
| `newspeak` | `c4aa0e46868b35435cfc3e9dc592c5c8e5e1a5a8` | 2026-11-23 | 2026-09-10 |
| `eurasia` | `ddd32bd1a9a23f3184e7473e298bb2e6baa3b2c8` | 2026-11-23 | 2026-11-24 |
| `newspeak-benchmarks` | `874ba34d48a84249e1e663061a70eeb608bcf601` | 2018-01-27 | 2026-09-10 |
| `primordialsoup` | `e7a7ece673f57fdeb43b586f297552e7e84c24ff` | 2026-09-26 | 2026-09-10 |
| `somns` | `e56f4c2815cef4cae2045a9b05dc1fd190315ba6` | 2019-07-07 | 2026-09-10 |
| `are-we-fast-yet` | `f35aed0a61171594804a44efe9d75f1f35360700` | 2026-11-15 | 2026-10-22 |
| `nova` | `591bf7ba0d35b095656dd31cdf397ef813a17fc2` | 2026-11-07 | 2026-09-10 |
| `opensmalltalk-vm` | `886d66aa28357b985b356361981e5ca3af2d1efa` | 2026-11-20 | 2026-09-10 |
| `strongtalk` | `e4ce7b69ad20e1013f07a53dfc11fdf57b6748cf` | 2010-01-06 | 2026-09-15 |
| `num-bigint` | `41f0b035d12971c5c7f7501917e31f0f6439c26c` | 2026-08-21 | 2026-09-29 |
| `num-integer` | `765fd9b3eb9397471d41b9f613e530f6d9d6383f` | 2026-08-11 | 2026-09-29 |
| `num-rational` | `cf95d6719c585d4721645c7ace5dfe1dd5f063e2` | 2026-07-07 | 2026-09-29 |
| `v8` | `45e84925bff25c1fac5c750c3e08bc622a95a799` | 2026-11-24 | 2026-10-27 |
| `jdk` | `c3b9d08dfdca3c8c3c6c93b80b7a675401b09673` | 2026-11-24 | 2026-10-27 |

`strongtalk` is pinned on its `gcc-linux` branch, not its default `master`, and
`.gitmodules` names that branch so `git submodule update --remote` follows it.
`gcc-linux` is the port to GCC and Linux, with build trees for Linux, macOS and
Windows, and is the newest line of the VM: it carries every change on `master`
but one, a name added to `contributorLicenses.html`, and 61 commits of its own.

`v8` and `jdk` are shallow: `.gitmodules` marks them `shallow = true`, so
`git submodule update --init` fetches only the pinned commit, not the
history of either (several gigabytes each). To move one forward, fetch the
new commit by hash with `--depth 1` before checking it out.

To move one forward: `git -C prior-art/<name> fetch && git -C prior-art/<name>
checkout <commit>`, then commit the changed submodule pointer here and update
the row above. Moving one forward moves the lines that `docs/design/` and
the code's comments cite in it, so the same commit checks every citation
of a line in that submodule against the new tree -- the construct it names
found again, its numbers rewritten, and a claim the new code no longer
bears out rewritten too; the seven that moved on 2026-11-24 were checked so.

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
  `prior-art/primordialsoup/LICENSE`. Code is adapted from it:
  `benchmarks/are-we-fast-yet/AreWeFastYet.ns` from
  `newspeak/BenchmarkRunner.ns`, `RichardsTon80.ns` from
  `newspeak/Richards.ns` and `DeltaBlueTon80.ns` from
  `newspeak/DeltaBlue.ns`, with its notice and the licence text in
  `THIRD-PARTY-NOTICES.md`.
- `somns` — MIT, Stefan Marr and Michael Haupt / Hasso Plattner Institute; see
  `prior-art/somns/LICENSE`. Code is adapted from it: the benchmarks in
  `benchmarks/are-we-fast-yet/`, from `core-lib/Benchmarks/`, with its
  notice in `THIRD-PARTY-NOTICES.md`.
- `are-we-fast-yet` — mixed: its files fall under MIT (Are We Fast Yet's and
  SOM's authors, and EclipseSource for minimal-json), the Apache License 2.0
  (Google, for Havlak), the Revised BSD licence of the Computer Language
  Benchmarks Game (Mandelbrot, NBody), and Mario Wolczko's terms (Richards,
  DeltaBlue), and `som/IdentityDictionary.java`,
  `havlak/UnionFindNode.java`, `NBody.java` and `nbody/*` carry no licence
  notice at all (NBody's comment names the Benchmarks Game); the Java files
  of `cd/` carry none either, but CD in every language is a port of
  Purdue University's CDx, under the BSD-3 notice (Purdue 2001-2010, Apple
  2015) that the JavaScript and SOM versions carry, so the adapted
  `CD.java` and `cd/*` carry it too; see `prior-art/are-we-fast-yet/LICENSE.md`, and the section
  "Are We Fast Yet" of `THIRD-PARTY-NOTICES.md` for which file carries
  which. Code is adapted from it: the Java sources in
  `benchmarks/are-we-fast-yet/java/`, from `benchmarks/Java/src/`, with every
  notice and licence text in `THIRD-PARTY-NOTICES.md`.
- `newspeak-benchmarks` — no license file in the tree. Treat it as
  read-only reference material and do not copy code out of it without
  resolving that first.
- `eurasia` — the three-clause BSD licence, Gilad Bracha 2026-2027; see
  `prior-art/eurasia/LICENSE`, which also says it holds no code of
  Primordial Soup's.
- `nova` — Mozilla Public License 2.0; see `prior-art/nova/LICENSE.md`. Note
  that the MPL is file-level copyleft, which makes it the most restrictive
  license here for any purpose beyond reading.
- `opensmalltalk-vm` — MIT; see `prior-art/opensmalltalk-vm/LICENSE`.
- `strongtalk` — a BSD-style license, Sun Microsystems 1994–2006; see
  `prior-art/strongtalk/sourceLicense.html`. Contributions made after the
  release are under the New BSD license, per `contributorLicenses.html`
  beside it.
- `v8` — the BSD-3-Clause licence of the V8 project authors; see
  `prior-art/v8/LICENSE`, which also lists the third-party code in the tree
  under its own terms.
- `jdk` — GPL version 2 with the Classpath Exception, Oracle and the other
  contributors; see `prior-art/jdk/LICENSE` and
  `prior-art/jdk/ADDITIONAL_LICENSE_INFO`. GPL code is never copied into
  `ns-next`; it is here to be read only.
- `num-bigint`, `num-integer` and `num-rational` — each MIT OR Apache-2.0, at
  the licensee's choice, The Rust Project Developers; see `LICENSE-MIT` and
  `LICENSE-APACHE` in each tree.

Reading these to understand a design is fine. Copying code out of any of them
into `ns-next` is a licensing decision, not a technical one. It has been made
three times: `crates/ns-runtime/src/arbitrary_integer/` adapts code from
`num-bigint` and `num-integer`, taken under MIT; `benchmarks/are-we-fast-yet/`
adapts the Are We Fast Yet benchmarks from `somns` (MIT) and its harness,
`RichardsTon80.ns` and `DeltaBlueTon80.ns` from `primordialsoup` (Apache
2.0); and `benchmarks/are-we-fast-yet/java/` adapts the Java versions of
those benchmarks and their harness from `are-we-fast-yet`, each file under
its upstream licence. `THIRD-PARTY-NOTICES.md` at the repository
root carries every one of those notices and licence texts. Each adapted file
names its source in a header. Nothing has been copied from any other
submodule.

## Checkout size

Worth knowing before running `git submodule update --init` without an argument:
`opensmalltalk-vm` is about **255 MB** checked out, several times larger than
everything else here combined (`strongtalk` 38 MB, `newspeak` 32 MB, `nova`
8 MB, `somns` 5.7 MB, `are-we-fast-yet` 3.3 MB, `primordialsoup` 2.8 MB,
`num-bigint` 777 KB, `newspeak-benchmarks` 389 KB, `num-rational` 192 KB,
`num-integer` 163 KB, `eurasia` 1.3 MB). It is mostly generated C sources and platform build
trees, none of which is the interesting part — the Cog and Spur design lives in
the Smalltalk-side sources and the comments. Initialize it by name when you
want it rather than pulling everything.

`jdk` (about 840 MB) and `v8` (about 200 MB) are larger still, even as the
shallow checkouts they are; initialize them by name too.

## Nested submodules

Two of these declare submodules of their own, and none of them is initialized —
nothing here is built, so nothing needs them. `git submodule update --init
--recursive` would fetch them if you ever want them.

- `somns` — `libs/truffle`, `libs/mx`, `libs/black-diamonds`, needed only to
  build SOMns.
- `nova` — `tests/test262`, the ECMAScript conformance suite, needed only to
  run Nova's own tests.

# ns-next

A native runtime for [Newspeak](https://newspeaklanguage.org/), written in Rust.

`ns-next` parses and runs Newspeak source directly: a hand-written lexer and parser for
the Newspeak3 textual syntax, a name resolver implementing Newspeak's lexical, `outer`
and `super` scoping, and two execution engines — a tree-walking interpreter and a
register-based bytecode compiler with its own VM. A running world can be serialized to a
versioned image and loaded back.

## Status

Working today: the full frontend, Newspeak's real name resolution, nested classes and
mixins with a class object per enclosing instance, both execution engines, and a
source-free image format. Every `.ns` file in the vendored corpus parses but one.

Not built yet, and the limits worth knowing before trying it on real code:

- **No garbage collector.** Objects are reference-counted and cycles leak.
- **Integers are `i64`.** Overflow raises a Newspeak error; there are no bignums.
- **The platform is minimal**, and widens only when a whitelisted corpus module needs
  something. Six of the 211 corpus modules run end to end; the rest ask for
  collections, streams and mirrors that do not exist yet.
- **No actors, no reflection API, no debugger, no JIT.** Eventual sends and
  Typed-Newspeak annotations parse and are discarded.

`docs/design/` describes how the system works and how much of each aspect exists;
`docs/PROGRESS.md` is the milestone chronology beside it.

## Requirements

Rust 1.98.0, pinned by `rust-toolchain.toml` — `rustup` picks it up automatically. No
other tooling is needed; the project has no external runtime dependencies.

## Build and test

```bash
# build every crate; the ns-next binary lands in target/debug
cargo build --workspace
# the full suite, including the differential harness over both engines
cargo test --workspace
# formatting
cargo fmt --all
# lints, must be warning-free
cargo clippy --workspace --all-targets
# parser corpus gate: must report "parsed 210, failed 0, expected-fail 1, stale 0"
cargo run -p ns-cli -- parse-corpus corpus/modules
# runnable corpus gate: must report "ran 6, failed 0, skipped 205"
cargo run -p ns-cli -- run-corpus corpus/modules
```

An optimized build, which is the one to measure — the debug build is slow enough to
mislead:

```bash
# the binary lands in target/release
cargo build --workspace --release
```

On Windows the C runtime is linked statically (see `.cargo/config.toml`), so
`ns-next.exe` is one self-contained file: it needs no Rust installation and no VC++
redistributable on the machine that runs it.

The differential harness runs every fixture under both engines and compares exit status,
stdout and stderr byte for byte. The corpus is a vendored set of real Newspeak modules,
kept as a non-regression gate for the parser.

## Usage

```bash
# run a file end to end: parse, resolve, bootstrap, load, instantiate, send the entry selector
cargo run -p ns-cli -- run tests-fixtures/hello.ns
```

The entry selector defaults to `main:args:`; `--send <selector>` picks another one. Two
engines are available and must produce identical output — the tree-walker is kept as a
differential oracle for the bytecode engine, which is the default:

```bash
# the same program under the Milestone 1 tree-walking interpreter
cargo run -p ns-cli -- run tests-fixtures/while_loop.ns --engine tree-walk
```

```bash
# print one method's bytecode; compile only, nothing is instantiated or run
cargo run -p ns-cli -- disasm tests-fixtures/blocks_and_return.ns find:in:
```

```bash
# serialize a running world to an image, then load it back and send it a message
cargo run -p ns-cli -- snapshot tests-fixtures/instance_init.ns world.img
cargo run -p ns-cli -- load world.img --send main:args:
```

```bash
# check that a file parses; --ast prints the syntax tree
cargo run -p ns-cli -- parse tests-fixtures/hello.ns
```

```bash
# time one entry send under both engines; measure with --release, never a debug build
cargo run --release -p ns-cli -- bench tests-fixtures/bench_loop.ns --iterations 30
```

`ns-next <command> --help` documents the remaining flags.

## Repository layout

```
crates/ns-syntax     lexer, parser, AST, spans
crates/ns-runtime    resolver, object model, dispatch, both engines, core library, image format
crates/ns-cli        the `ns-next` binary
corpus/              vendored Newspeak modules used as a parser corpus
tests-fixtures/      end-to-end programs with their expected output
docs/design/         living design documents — start at its README.md
docs/PROGRESS.md     milestone chronology
docs/superpowers/    implementation plans, kept as execution records
docs/references/     the Newspeak papers the design argues from (third-party, see PROVENANCE.md)
```

## License

MIT — see [LICENSE](LICENSE). The vendored corpus under `corpus/` is third-party
material with its own terms — see `corpus/PROVENANCE.md` and `corpus/UPSTREAM-LICENSE`.

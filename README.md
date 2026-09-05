# ns-next

A native runtime for [Newspeak](https://newspeaklanguage.org/), written in Rust.

`ns-next` parses and runs Newspeak source directly: a hand-written lexer and parser for
the Newspeak3 textual syntax, a name resolver implementing Newspeak's lexical, `outer`
and `super` scoping, and two execution engines — a tree-walking interpreter and a
register-based bytecode compiler with its own VM. A running world can be serialized to a
versioned image and loaded back.

## Status

| Milestone | Status |
|---|---|
| M1 — Frontend (lexer, parser, AST) | Complete |
| M1 — Runtime (resolver, object model, dispatch, interpreter) | Complete |
| M2 — Bytecode compiler, register VM, image format | Complete |
| M3 — Inline caches and GC | Not started |
| M4–M5 — JIT | Not started |

`docs/PROGRESS.md` carries the details and links the design spec and implementation plan
behind each milestone.

## Requirements

Rust 1.98.0, pinned by `rust-toolchain.toml` — `rustup` picks it up automatically. No
other tooling is needed; the project has no external runtime dependencies.

## Build and test

```bash
cargo test --workspace
cargo fmt --all
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
```

The test suite includes a harness that runs every fixture under both engines and compares
exit status, stdout and stderr byte for byte. The last command checks the parser against a
vendored corpus of real Newspeak modules; it is a non-regression gate and must report
`parsed 210, failed 0, expected-fail 1, stale 0`.

## Usage

Run a Newspeak file end to end — parse, resolve, bootstrap, load, instantiate, and send
the entry selector:

```bash
cargo run -p ns-cli -- run tests-fixtures/hello.ns
```

The entry selector defaults to `main:args:`; `--send <selector>` picks another one. Two
engines are available and must produce identical output — the tree-walker is kept as a
differential oracle for the bytecode engine, which is the default:

```bash
cargo run -p ns-cli -- run tests-fixtures/while_loop.ns --engine tree-walk
```

Print the bytecode of a single method (compile only — nothing is instantiated or run):

```bash
cargo run -p ns-cli -- disasm tests-fixtures/blocks_and_return.ns find:in:
```

Serialize a world to an image and load it back:

```bash
cargo run -p ns-cli -- snapshot tests-fixtures/instance_init.ns world.img
```

```bash
cargo run -p ns-cli -- load world.img --send main:args:
```

`parse` checks a single file and reports whether it parsed, with `--ast` to print the
syntax tree. `ns-next <command> --help` documents the remaining flags.

## Repository layout

```
crates/ns-syntax     lexer, parser, AST, spans
crates/ns-runtime    resolver, object model, dispatch, both engines, core library, image format
crates/ns-cli        the `ns-next` binary
corpus/              vendored Newspeak modules used as a parser corpus
tests-fixtures/      end-to-end programs with their expected output
docs/                progress tracker, design specs, implementation plans
```

## License

MIT, per the workspace manifest. The vendored corpus under `corpus/` is third-party
material with its own terms — see `corpus/PROVENANCE.md` and `corpus/UPSTREAM-LICENSE`.

# One Artifact, Packaged Before `main:args:` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `run` and `build` assemble an application from one source file (its root instantiated with `new`) or a JSON project file (its root instantiated with `packageUsing: manifest`), `build` serializes the assembled application just before `main:args:`, `load` starts it by sending `main: platform args:`, and the stateless binary, `snapshot` and every `--send`/`--entry` go away.

**Architecture:** The image format learns to carry several top-level modules: the mixin table becomes a forest (one preorder tree per module, concatenated), a new module section after the mixin table records each module's origin, optional source and root-class data, and new class and hash tags name a module's root class and its levels by module index. A new `image::package` writes such an image with the application instance as the graph's only root and a header flag, `FLAG_START`, that tells `load` to send `main:args:` to it. On the CLI side a project file (JSON, read with `serde_json`) names the root source and the other modules; a small unprivileged Newspeak class, `Manifest`, compiled into the program like a user module, answers each module definition by name through `doesNotUnderstand:`. The goldens move their platform access into `main:args:` first, while the old CLI still runs them, so every intermediate commit stays green.

**Tech Stack:** Rust 1.98.0, edition 2021; `ns-runtime` keeps `#![forbid(unsafe_code)]`. One new dependency for `ns-cli`: `serde_json = "1"` (already in `Cargo.lock` through `ns-lsp`). Tests under `cargo-nextest`; goldens under `goldens/`; Python 3 (`/c/Python314/python`) for the one-off golden migration script.

**Spec:** `docs/design/image-format.md`, "Planned: one artifact, packaged before `main:args:`" (decided with the user on 2026-09-29). Read it first; this plan argues from it. Background in the same document: "Two artifacts", "The stateless binary", "Verification", and `docs/design/core-library.md`, "The VM mirror is the whole Rust boundary". §6.4 and §8.4 of `docs/references/newspeak-spec.md`.

## Global Constraints

- Every change obeys `CLAUDE.md`, "Invariants that are easy to break". In particular:
  - **Program outcomes are pinned.** No golden's `.toml` changes in this plan. A golden whose recorded `status`, `stdout` or `stderr` stops matching is a defect to find, never a `.toml` to update.
  - **An image is untrusted input.** Every new count, index and tag is validated before it indexes or allocates; no byte sequence may panic. `CodeIds::id_of` must stop panicking (Task 3).
  - **A wire tag is assigned once and never renumbered or reused.** New numbers: class tags `CLASS_MODULE = 6`, `CLASS_MODULE_LEVEL = 7`; hash tags `HASHED_MODULE_CLASS = 5`, `HASHED_MODULE_LEVEL = 6`; origin tags `ORIGIN_FILE = 0` (today's literal 0, now named), `ORIGIN_EMBEDDED = 1`; header flag `FLAG_START = 4`. Retired, never reused: class tags 1 (`CLASS_USER_ROOT`) and 3 (`CLASS_ROOT_LEVEL`), hash tag 1 (`HASHED_ROOT_CLASS`), header flag 2 (`FLAG_STATELESS`, retired in Task 12). Every one is pinned in `crates/ns-runtime/tests/opcode_tags.rs`.
  - **The VM mirror is a capability.** `Manifest.ns` is resolved with `ns_runtime::resolve`, never `resolve_kernel`, and holds no mirror. It is not a shipped module (`ModuleSource::User`), so an image may carry it as ordinary code.
  - **Never `{:?}` a `Value`, never `.unwrap()`/`.expect()` an `EvaluationResult` or `HostResult`.** Tests use a local `expect_ok` helper copied from a neighbouring test file.
- The platform enters a program only as `main:args:`'s first argument. No command instantiates a root with the platform; `build` runs the application's factory and initializers with no platform reachable, and `image::package` refuses a graph that reaches a shipped module's instance or class.
- Module order is fixed and shared by everything that loads or writes modules: **the root module first, then the project file's `modules` in the order written, then the `Manifest` module** (the last only for a project). `load` rebuilds module classes in that order, which is what keeps identity hashes identical between `run`, `build` and `load`.
- Paths: the project file's own path is absolute or relative to the working directory; a path inside it is absolute or relative to the project file; `/` and `\` are both accepted in every path on every operating system.
- Error texts that goldens or tests pin stay byte-identical: `Uncaught Newspeak error: …`, `Uncaught Newspeak error during instantiation: …`, `internal: non-local return escaped the entry send`, `internal: non-local return escaped instantiation`.
- Names are written out (`CLAUDE.md`, "Conventions"; clippy's `min_ident_chars`). No `m`, `pc`, `args` of our own; `arguments`, `module_index`, `project_directory`.
- Comments explain the decision, not the mechanics.
- A Newspeak module written inline in a Rust test is an `indoc! {"…"}` indented with the Rust around it, closing `"}` on its own line.
- Conventional commits. Every commit ends with
  `Co-Authored-By: <the model that actually wrote the code> <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01XkuJBQierVYxAMXpiaz4ty`.
  A subagent's commit names the subagent's own model.
- Per-commit gate (`CLAUDE.md`, "Working in this repository"):
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets` (warning-free)
  - `cargo nextest run --workspace --exclude ns-lsp`
  - `cargo test --workspace --exclude ns-lsp --doc`
  - `cargo run -p ns-cli -- parse-corpus corpus/modules`, which must report "parsed 211, failed 0"

  A commit that changes only Markdown runs no gates. Task 1 changes goldens, which `crates/ns-lsp/tests/corpus.rs` and the JetBrains lexer tests read, so Task 1 also runs `cargo nextest run -p ns-lsp` and `./editors/jetbrains/gradlew -p editors/jetbrains check`. The stress modes, `run-corpus` and the rest of the end-of-batch gates run once, in Task 14, through the `finishing-a-batch` skill.

## Review Focus

- **A project module named like a public member of `Object`.** `manifest Array`, `manifest String` or `manifest printString` would answer `Object`'s accessor or method, never the module, so a project whose module is declared `class Array …` would silently get the kernel's `Array`. Expected: `run`/`build` refuse the project before anything runs, naming the module and the member. Pinned in Task 7 (`a_module_shadowed_by_an_object_member_is_refused`).
- **A graph that reaches a class of a module the image does not carry, or a closure over such a module's code.** Today the first is an `Err` from `encode_class_record`, but the second **panics** in `CodeIds::id_of`. Expected: both are an encoding `Err` naming the module. Pinned in Task 4 (`a_graph_reaching_a_module_the_image_does_not_carry_is_refused`, `a_closure_over_code_no_module_carries_is_refused_not_a_panic`).
- **The same source run, built and loaded must draw the same identity hashes.** A program printing `Object new hash` from a slot initializer prints the same number under `run` and under `build` then `load`. It breaks the moment `load` rebuilds modules in a different order than the CLI loaded them, or the manifest is built at a different point. Pinned in Task 9 (`an_initializer_is_not_rerun_by_load_and_draws_the_same_hash`).
- **A crafted module section.** Expected: each of these is a `DecodeError`, never a panic:
  - a count that disagrees with the mixin table's trees;
  - a count far beyond the bytes left;
  - a `CLASS_MODULE`/`CLASS_MODULE_LEVEL`/`HASHED_MODULE_*` index past the modules or a module's levels;
  - a root section whose clause index is out of range for *its own* module's row.

  Pinned in Task 4.
- **Windows-style and Unix-style paths in one project file.** Expected, on every operating system: `modules/Sieve.ns` and `modules\Sieve.ns` name the same file relative to the project file; `C:\x\A.ns`, `C:/x/A.ns` and `/x/A.ns` are taken as absolute where the host treats them so. Pinned in Task 6.

---

## File Structure

**ns-runtime**

- `crates/ns-runtime/core-library/Manifest.ns` (new) — the unprivileged `Manifest` class.
- `crates/ns-runtime/src/interpreter/core_library.rs` — `pub const MANIFEST_SOURCE: &str`, next to the shipped modules' `include_str!`s but deliberately outside their table.
- `crates/ns-runtime/src/lib.rs` — re-export `MANIFEST_SOURCE`.
- `crates/ns-runtime/src/types.rs` — `SourceOrigin::Embedded(String)`.
- `crates/ns-runtime/src/symbol.rs` — well-known `PACKAGE_USING`, `MAIN_ARGUMENTS`.
- `crates/ns-runtime/src/interpreter/mod.rs` — `Interpreter::responds_publicly`.
- `crates/ns-runtime/src/image/opcodes.rs` — the new and retired tags and flags.
- `crates/ns-runtime/src/image/modules.rs` (new) — `ModuleClasses` (the root classes and their levels, by module index), the module section's encoder and decoder, `build_module_classes`.
- `crates/ns-runtime/src/image/encode.rs`, `decode.rs`, `mixins.rs`, `inspect.rs`, `mod.rs` — `root_class`/`root_levels` become `&ModuleClasses`; `package`; `LoadedImage.modules`.
- `crates/ns-runtime/tests/image.rs`, `opcode_tags.rs`, `trampoline.rs`, new `tests/manifest.rs`.

**ns-cli**

- `crates/ns-cli/Cargo.toml` — `serde_json = "1"`.
- `crates/ns-cli/src/project.rs` (new) — reading and resolving a project file.
- `crates/ns-cli/src/program.rs` (new) — a program's sources loaded, compiled into an interpreter, and assembled into an application.
- `crates/ns-cli/src/entry.rs` — shrinks to the reporting helpers and `main_arguments`.
- `crates/ns-cli/src/source.rs` — `load_embedded`.
- `crates/ns-cli/src/commands/run.rs`, `build.rs`, `load.rs`, `bench.rs`, `run_corpus.rs`, `disasm.rs`; `commands/snapshot.rs` deleted; `main.rs`.
- `crates/ns-cli/tests/common/mod.rs`, `run_goldens.rs`, `golden_round_trips.rs`, `run.rs`, `build_load.rs`, `inspect.rs`, `lowering.rs`, `disasm_snapshots.rs`, `cli.rs`; `snapshot_load.rs` deleted, its surviving tests moved into `build_load.rs`.

**Goldens and docs**

- `goldens/**/*.ns` — roots without `usingPlatform:` (Task 1); `goldens/manifest/` (new category, Task 8).
- `corpus/runnable.toml` header; `README.md`; `CLAUDE.md`; `docs/design/README.md`, `image-format.md`, `core-library.md`, `compiler.md`, `execution.md`, `dispatch.md`; `docs/OPEN-WORK.md`; `docs/PROGRESS.md`.

---

### Task 1: The goldens receive the platform only in `main:args:`

Every golden's root is `class X usingPlatform: p = …` today. The old CLI instantiates a root whose factory takes no argument (`entry::factory_arguments_for`), so moving the goldens first keeps every lane green, and every later task runs against goldens already in their final form.

A dry run of the script below against a copy of `goldens/` rewrote 262 goldens. The release binary of `53b1c3d` then ran every rewritten golden under both lowerings, and each produced its recorded `status`, `stdout` and `stderr` byte for byte. The five it leaves are migrated by hand (Step 3).

**Files:**
- Modify: `goldens/**/*.ns` (267 files)
- Create (not committed): `.temp/migrate_goldens.py`

- [ ] **Step 1: Write the migration script** to `.temp/migrate_goldens.py` (git-ignored). It rewrites in place and names every file it cannot rewrite mechanically.

```python
"""Moves every golden's platform from its root's `usingPlatform:` factory into
`main:args:`. Usage: python migrate_goldens.py <goldens-directory>
Rewrites files in place and prints every file it could not rewrite
mechanically, which is then rewritten by hand."""
import pathlib
import re
import sys


def mask(text):
    """`text` with comments and string literals blanked, same length, so a
    position found in the mask indexes the original."""
    out = list(text)
    index = 0
    depth = 0
    while index < len(text):
        if text.startswith("(*", index):
            depth += 1
            out[index] = out[index + 1] = " "
            index += 2
            continue
        if depth and text.startswith("*)", index):
            depth -= 1
            out[index] = out[index + 1] = " "
            index += 2
            continue
        if depth:
            if text[index] != "\n":
                out[index] = " "
            index += 1
            continue
        if text[index] == "'":
            end = index + 1
            while True:
                if text[end] == "'" and end + 1 < len(text) and text[end + 1] == "'":
                    end += 2
                    continue
                if text[end] == "'":
                    break
                end += 1
            for position in range(index + 1, end):
                if text[position] != "\n":
                    out[position] = " "
            index = end + 1
            continue
        if text[index] == "$" and index + 1 < len(text):
            out[index + 1] = " "
            index += 2
            continue
        index += 1
    return "".join(out)


def matching(masked, open_index):
    depth = 0
    for index in range(open_index, len(masked)):
        if masked[index] == "(":
            depth += 1
        elif masked[index] == ")":
            depth -= 1
            if depth == 0:
                return index
    raise ValueError("unbalanced parentheses")


def word(name):
    return re.compile(r"(?<![\w:])" + re.escape(name) + r"(?![\w])")


HEADER = re.compile(r"^class\s+(\w+)\s+usingPlatform:\s*(\w+)\s*", re.M)


def strip_factory(text):
    header = HEADER.search(mask(text))
    return text[: header.start()] + f"class {header.group(1)} " + text[header.end():]


def bars_at(masked, index):
    """The `|` or `||` token starting at `index`, as its length, or 0."""
    if masked[index] != "|":
        return 0
    return 2 if masked[index + 1] == "|" else 1


def closing_bars(masked, start, length, limit):
    """Where the `|`/`||` token of `length` that closes a clause opened just
    before `start` begins, skipping bracketed code (a block's `[:x | ...]`)."""
    depth = 0
    index = start
    while index < limit:
        character = masked[index]
        if character in "([{":
            depth += 1
        elif character in ")]}":
            depth -= 1
        elif depth == 0 and character == "|":
            found = bars_at(masked, index)
            if found == length:
                return index
            index += found
            continue
        index += 1
    return None


def slot_list(masked, initializer_open, initializer_close):
    """(opening, opening length, closing) of the initializer's slot clause,
    or None."""
    index = initializer_open + 1
    while masked[index].isspace():
        index += 1
    length = bars_at(masked, index)
    if not length:
        return None
    closing = closing_bars(masked, index + length, length, initializer_close)
    return (index, length, closing) if closing is not None else None


def declarations_of(masked, opening, closing):
    found = []
    start = opening + 1
    depth = 0
    for index in range(opening + 1, closing):
        character = masked[index]
        if character in "([{":
            depth += 1
        elif character in ")]}":
            depth -= 1
        elif character == "." and depth == 0 and not masked[index + 1].isdigit():
            found.append((start, index + 1))
            start = index + 1
    if masked[start:closing].strip():
        found.append((start, closing))
    return found


DECLARATION = re.compile(
    r"(\s*)((?:private|public|protected)\s+)?(\w+)\s*(::=|=)\s*(.*?)\.?\s*$", re.S
)


def migrate(text):
    masked = mask(text)
    header = HEADER.search(masked)
    if not header:
        return None, "no usingPlatform: root"
    parameter = header.group(2)
    initializer_open = masked.index("(", masked.index("=", header.end()))
    initializer_close = matching(masked, initializer_open)
    main = re.search(r"public\s+main:\s*(\w+)\s+args:\s*(\w+)\s*=\s*\(", masked)
    if not main:
        return None, "no main:args:"
    main_parameter = main.group(1)
    main_open = main.end() - 1
    main_close = matching(masked, main_open)
    uses = [
        match.start()
        for match in word(parameter).finditer(masked)
        if not header.start() <= match.start() < header.end()
        and not main.start() <= match.start() < main_close
    ]
    if not uses:
        return strip_factory(text), None
    clause = slot_list(masked, initializer_open, initializer_close)
    if clause is None or any(not clause[0] < use < clause[2] for use in uses):
        return None, "the factory parameter is used outside the root's slot declarations"
    opening, opening_length, closing = clause
    declarations = declarations_of(masked, opening + opening_length - 1, closing)
    derived = []
    for begin, end in declarations:
        if not word(parameter).search(masked, begin, end):
            continue
        declaration = DECLARATION.match(text[begin:end])
        if not declaration:
            return None, f"cannot read the slot declaration {text[begin:end].strip()!r}"
        derived.append(
            (
                begin,
                end,
                declaration.group(1),
                declaration.group(2) or "",
                declaration.group(3),
                declaration.group(4),
                word(parameter).sub(main_parameter, declaration.group(5).strip()),
            )
        )
    locals_found = []
    assigned = []
    for begin, end, leading, access, name, operator, expression in derived:
        outside_main = [
            match.start()
            for match in word(name).finditer(masked)
            if not begin <= match.start() < end and not main.start() <= match.start() < main_close
        ]
        if any(initializer_open < position < initializer_close for position in outside_main):
            return None, f"the slot {name} is read inside the root's initializer"
        if outside_main:
            assigned.append((begin, end, leading, access, name, expression))
        else:
            locals_found.append((begin, end, name, operator, expression))
    body = text[main_open + 1 : main_close]
    indentation = re.search(r"\n([ \t]*)\S", body)
    indentation = indentation.group(1) if indentation else "        "
    new_locals = "".join(
        f" {name} {operator} {expression}." for _, _, name, operator, expression in locals_found
    )
    statements = "".join(
        f"\n{indentation}{name}:: {expression}." for _, _, _, _, name, expression in assigned
    )
    edits = []
    body_start = main_open + 1
    while masked[body_start].isspace():
        body_start += 1
    temporaries_length = bars_at(masked, body_start)
    if temporaries_length:
        close = closing_bars(masked, body_start + temporaries_length, temporaries_length, main_close)
        if statements and "=" in masked[body_start:close]:
            return None, "main's temporaries have initializers that run before a slot main fills"
        if new_locals:
            insert_at = body_start + temporaries_length
            edits.append((insert_at, insert_at, new_locals))
        if statements:
            after = close + temporaries_length
            edits.append((after, after, statements))
    else:
        piece = ""
        if new_locals:
            piece += f"\n{indentation}|{new_locals} |"
        piece += statements
        if piece:
            edits.append((main_open + 1, main_open + 1, piece))
    for begin, end, leading, access, name, _ in assigned:
        edits.append((begin, end, f"{leading}{access}{name} ::= nil."))
    if len(locals_found) == len(declarations):
        # Every slot moved into main: the clause itself goes.
        edits.append((opening, closing + opening_length, ""))
    else:
        for begin, end, *_ in locals_found:
            edits.append((begin, end, ""))
    for low, high, replacement in sorted(edits, key=lambda edit: (edit[0], edit[1]), reverse=True):
        text = text[:low] + replacement + text[high:]
    return strip_factory(text), None


if __name__ == "__main__":
    manual = 0
    for path in sorted(pathlib.Path(sys.argv[1]).glob("*/*.ns")):
        migrated, problem = migrate(path.read_text(encoding="utf-8", newline=""))
        if problem:
            print(f"MANUAL {path}: {problem}")
            manual += 1
            continue
        path.write_text(migrated, encoding="utf-8", newline="")
    print(f"manual: {manual}")
```

What it does, so a reviewer can check a diff against it:

- A factory parameter used nowhere outside `main:args:` loses only the header's `usingPlatform: p`. `class X = Object new (` has the implicit `new` factory (§6: "If the signature is omitted it defaults to `new`").
- A root slot whose initializer names the factory parameter and that nothing outside `main:args:` reads becomes a local of `main:args:`: `| t = platform transcript. |`, the parameter renamed to `main:`'s own. A simultaneous clause (`|| … ||`) takes it as one more simultaneous temporary.
- Such a slot that something else reads (another method, a nested class, a nested superclass clause) stays a slot, declared `::= nil`, and `main:args:` fills it first thing: `List:: platform collections List.`.
- A slot clause emptied entirely goes, `(| t = p transcript. |)` becoming `()`.

- [ ] **Step 2: Run the script on the tree**

Run: `/c/Python314/python .temp/migrate_goldens.py goldens`
Expected: exactly these four lines, then `manual: 4`:
```
MANUAL goldens\language\instance_init.ns: the slot out is read inside the root's initializer
MANUAL goldens\mixin\mixin_same_mixin_applied_twice_in_one_chain.ns: main's temporaries have initializers that run before a slot main fills
MANUAL goldens\spec\spec_6_3_2_a_future_is_a_block_argument_like_its_value.ns: main's temporaries have initializers that run before a slot main fills
MANUAL goldens\spec\spec_6_3_2_a_future_is_an_argument_like_its_value.ns: main's temporaries have initializers that run before a slot main fills
```
`git diff --stat` must show only `.ns` files under `goldens/`, and no line-ending change: `git diff --ignore-cr-at-eol --stat` must list the same files, and `git diff | grep -c '^[-+].*\r$'` must be 0 in a checkout without CR line endings.

- [ ] **Step 3: Migrate the five by hand**

Each keeps its recorded outcome, and each keeps its point: read the `(* … *)` comment at the top first, and keep what it says true, editing its wording where it names the factory.

1. `goldens/language/instance_init.ns`. Its initializer prints `init ran` through a slot it fills from the platform, which no initializer can reach any more. Keep the point, that the instance initializer's statements run once, at instantiation, before `main:args:`: the initializer records the fact, and `main:args:` prints it first. The stdout `init ran\nhi 2\n` is unchanged:
   ```newspeak
   class InstanceInit = Object new (
   	| greeting ::= 'hi'. count ::= 0. initialized ::= false. |
   	count:: greeting size.
   	initialized:: true.
   ) (
   	public main: p args: a = (
   		| out = p transcript. |
   		initialized ifTrue: [out show: 'init ran'; newLine].
   		out show: greeting; show: ' '; show: count printString; newLine
   	)
   ) : ()
   ```
   Its initializer's disasm snapshot changes with it (Step 5).
2. `goldens/mixin/mixin_same_mixin_applied_twice_in_one_chain.ns` and the two `spec_6_3_2_a_future_is_*_like_its_value.ns`. `main:args:`'s own temporaries (`| twice = Twice new. |`, `| probe = Probe new. |`) are initialized before any statement, and the classes they instantiate read the slot `t`. Turn that temporary into a declaration without an initializer and an assignment after `t` is filled, keeping `t` a slot declared `::= nil`:
   ```newspeak
   public main: platform args: arguments = (
       | twice |
       t:: platform transcript.
       twice:: Twice new.
       …
   ```
   None of the three golden's comments is about the temporary being immutable. Check that for each one before changing it.
3. `goldens/mixin/mixin_module_declares_itself_a_value.ns`. The script rewrites it, but line 36, `twin:: self class usingPlatform: platform.`, instantiates the module again with the old factory. Make it `twin:: self class new.`, and rewrite the comment that argues the platform "arrives as the factory's argument and again as main:args:'s": now it arrives only as `main:args:`'s, so the module's value-ness no longer rests on the platform being a value when passed to the factory. Keep what the golden pins about the root being a value.

- [ ] **Step 4: Check that nothing mentions the old factory**

Run: `grep -rln "usingPlatform" goldens`
Expected: no output.

- [ ] **Step 5: Regenerate the disasm snapshots the migration changed**

`crates/ns-cli/tests/disasm_snapshots.rs` disassembles six goldens. The migration changes some of their bodies:
- an initializer takes no factory argument and no longer initializes a platform slot;
- a `main:args:` gains the locals that moved into it.

Run: `INSTA_UPDATE=always cargo nextest run -p ns-cli --test disasm_snapshots`, then `git diff crates/ns-cli/tests/snapshots`. Every change must be one of those two, for instance an initializer's `args 1` becoming `args 0` with its `#transcript` send gone. Any other change is a migration mistake.

- [ ] **Step 6: Run every golden and the other gates**

Run: `cargo nextest run --workspace --exclude ns-lsp` (every golden under both lowerings, both round trips, the disasm snapshots)
Expected: PASS. A golden that fails is a migration mistake. Fix the `.ns`, never the `.toml`.
Run: `cargo nextest run -p ns-lsp` and `./editors/jetbrains/gradlew -p editors/jetbrains check` (both walk every golden)
Expected: PASS.

- [ ] **Step 7: Update the one document that describes a golden's root**

`docs/design/README.md`, "The goldens": add one sentence after the `.toml` paragraph: "A golden's root class is instantiated with `new` and reaches the platform only as `main:args:`'s first argument (§8.4), as every program does ([image-format.md](image-format.md), \"Planned: one artifact, packaged before `main:args:`\")." Task 13 rewrites the reference to the planned section once it is built.

- [ ] **Step 8: Commit**

```bash
git add goldens crates/ns-cli/tests/snapshots docs/design/README.md
git commit -m "test: the goldens reach the platform only through main:args:"
```

---

### Task 2: An embedded module's origin

The `Manifest` module comes from a source compiled into the binary, not from a file. An image records each module's origin, and a file path would claim a file that does not exist.

**Files:**
- Modify: `crates/ns-runtime/src/types.rs:432-434`
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (new constants)
- Modify: `crates/ns-runtime/src/image/encode.rs:122-129` (`encode_source_origin`)
- Modify: `crates/ns-runtime/src/image/decode.rs:824-832` (`decode_source_origin`)
- Modify: every `match` on `SourceOrigin` (`grep -rn "SourceOrigin::File" crates`)
- Test: `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/tests/opcode_tags.rs`

**Interfaces:**
- Produces: `SourceOrigin::Embedded(String)`, the name of an embedded source (`"Manifest.ns"`); `opcodes::ORIGIN_FILE = 0`, `opcodes::ORIGIN_EMBEDDED = 1`.

- [ ] **Step 1: Write the failing tests.** In `crates/ns-runtime/tests/opcode_tags.rs`, add:

```rust
#[test]
fn source_origin_tags_are_fixed() {
    assert_eq!(opcodes::ORIGIN_FILE, 0);
    assert_eq!(opcodes::ORIGIN_EMBEDDED, 1);
}
```

In `crates/ns-runtime/tests/image.rs`, next to the other origin tests (search `decode_source_origin`), add:

```rust
#[test]
fn an_embedded_origin_round_trips() {
    let mut writer = encode::Writer::new();
    encode::encode_source_origin(&mut writer, &SourceOrigin::Embedded("Manifest.ns".to_string()));
    let bytes = writer.into_bytes();
    let mut reader = decode::Reader::new(&bytes);
    match decode::decode_source_origin(&mut reader) {
        Ok(SourceOrigin::Embedded(name)) => assert_eq!(name, "Manifest.ns"),
        Ok(SourceOrigin::File(path)) => panic!("decoded a file origin: {}", path.display()),
        Err(error) => panic!("decoding failed: {error}"),
    }
}

#[test]
fn an_unknown_origin_tag_is_refused() {
    let bytes = [2u8, 0, 0, 0, 0];
    let mut reader = decode::Reader::new(&bytes);
    assert!(matches!(
        decode::decode_source_origin(&mut reader),
        Err(decode::DecodeError::InvalidTag { what: "source origin", tag: 2 })
    ));
}
```

- [ ] **Step 2: Run them to see them fail.** Run: `cargo nextest run -p ns-runtime --test opcode_tags --test image origin`. Expected: FAIL to compile (`ORIGIN_FILE`, `SourceOrigin::Embedded` unknown).

- [ ] **Step 3: Implement.**

`types.rs`:
```rust
pub enum SourceOrigin {
    File(PathBuf),
    /// A source compiled into the binary, named by the file it was read from
    /// at build time: `Manifest.ns`. An image names it so rather than by a
    /// path, which would claim a file that exists nowhere at run time.
    Embedded(String),
}
```

`opcodes.rs`, beside the other families (with the usual "Next free number" line):
```rust
/// How a module's `SourceOrigin` is written. Next free number: 2.
pub const ORIGIN_FILE: u8 = 0;
pub const ORIGIN_EMBEDDED: u8 = 1;
```

`encode_source_origin`:
```rust
pub fn encode_source_origin(writer: &mut Writer, origin: &SourceOrigin) {
    match origin {
        SourceOrigin::File(path) => {
            writer.write_u8(ORIGIN_FILE);
            writer.write_str(&path.to_string_lossy());
        }
        SourceOrigin::Embedded(name) => {
            writer.write_u8(ORIGIN_EMBEDDED);
            writer.write_str(name);
        }
    }
}
```

`decode_source_origin`:
```rust
pub fn decode_source_origin(reader: &mut Reader) -> Result<SourceOrigin, DecodeError> {
    match reader.read_u8()? {
        ORIGIN_FILE => Ok(SourceOrigin::File(PathBuf::from(reader.read_str()?))),
        ORIGIN_EMBEDDED => Ok(SourceOrigin::Embedded(reader.read_str()?)),
        tag => Err(DecodeError::InvalidTag {
            what: "source origin",
            tag,
        }),
    }
}
```

Every other exhaustive `match` on `SourceOrigin` the compiler names gets an `Embedded` arm. Where the origin is displayed (`inspect.rs`), show `embedded Manifest.ns`.

- [ ] **Step 4: Run the tests.** Run: `cargo nextest run -p ns-runtime --test opcode_tags --test image origin`. Expected: PASS.

- [ ] **Step 5: Run the per-commit gate, then commit.**

```bash
git add crates/ns-runtime
git commit -m "feat: an image names an embedded module's origin"
```

---

### Task 3: The code table and the mixin table cover several modules

The wire stays byte-identical for one module. The tables become a forest, one `mixin::preorder` tree per module, concatenated in module order. Everything that walks them takes the module roots as a slice.

**Files:**
- Modify: `crates/ns-runtime/src/image/mixins.rs` — `plan` (111), `encode` (240), `check_preorder` (711), `Decoded` (`root()` at 402)
- Modify: `crates/ns-runtime/src/image/encode.rs` — `encode_code_table` (388), `CodeIds::id_of` (182), `encode_closure_record`, `mixins::encode`'s callers
- Modify: `crates/ns-runtime/src/image/mod.rs` — `encode_describing` (65)
- Modify: `crates/ns-runtime/src/image/decode.rs` — `build_root_class` (2637), `decode_root_section` (2695): `mixins.root()` becomes `mixins.module_root(0)`
- Modify: `crates/ns-runtime/src/image/inspect.rs` — its use of `Decoded::root`
- Test: `crates/ns-runtime/tests/image.rs` — helpers `round_trip_code_table` (690), `mixin_ids_for` (1952) and every direct call of `encode_code_table` / `mixins::encode` / `mixins::plan`

**Interfaces:**
- Produces:
  - `mixins::plan(heap: &Heap<'gc>, module_roots: &[ClassReference<'gc>]) -> Result<Vec<Row>, String>`
  - `mixins::encode(writer, heap, module_roots: &[ClassReference<'gc>], symbols, codes) -> Result<MixinIds, String>`
  - `encode::encode_code_table(writer, heap, module_roots: &[ClassReference<'gc>], symbols) -> Result<CodeIds, String>`
  - `CodeIds::id_of(&self, code: &Rc<CompiledCode>) -> Result<u32, String>`
  - `mixins::Decoded { mixins, code, module_rows: Vec<u32> }` and `Decoded::module_root(&self, module_index: usize) -> Option<&Rc<Mixin>>`; `Decoded::root` is removed.

- [ ] **Step 1: Write the failing tests** in `crates/ns-runtime/tests/image.rs`, next to the mixin-table tests (search `fn mixin_table_of`):

```rust
#[test]
fn two_modules_encode_as_two_trees_in_module_order() {
    let (mut interpreter, first) = load(indoc! {"
        Newspeak3
        'Test'
        class First = Object new () (
            class Inner = Object new () ( public answer = ( ^1 ) ) : ()
        ) : ()
    "});
    let second_source = indoc! {"
        Newspeak3
        'Test'
        class Second = Object new () ( public answer = ( ^2 ) ) : ()
    "};
    let second_module = ns_syntax::parse(second_source).map_err(|_| "parse").unwrap_or_else(|error| panic!("{error}"));
    let resolved = ns_runtime::resolve(
        second_module,
        second_source.into(),
        SourceOrigin::File("Second.ns".into()),
    )
    .unwrap_or_else(|_| panic!("resolve"));
    let second = interpreter.load_module(&resolved);
    interpreter.enter(|mutator| {
        let roots = [class_reference(mutator, &first), class_reference(mutator, &second)];
        let rows = mixins::plan(mutator.heap(), &roots).unwrap_or_else(|error| panic!("{error}"));
        let names: Vec<&str> = rows.iter().map(|row| row.mixin.name.as_str()).collect();
        // First's tree -- itself, its nested Inner, Inner's and its own
        // class sides in preorder -- then Second's.
        assert_eq!(names.first(), Some(&"First"));
        let second_at = names.iter().position(|name| *name == "Second").unwrap_or_else(|| panic!("no Second row"));
        assert!(names[..second_at].contains(&"Inner"));
        assert!(!names[second_at..].contains(&"Inner"));
        let mut symbols = symbols::Builder::new();
        let mut code = encode::Writer::new();
        let ids = encode::encode_code_table(&mut code, mutator.heap(), &roots, &mut symbols)
            .unwrap_or_else(|error| panic!("{error}"));
        let mut writer = encode::Writer::new();
        mixins::encode(&mut writer, mutator.heap(), &roots, &mut symbols, &ids)
            .unwrap_or_else(|error| panic!("{error}"));
        let table = symbols.finish();
        let code_bytes = code.into_bytes();
        let decoded_code = decode::decode_code_table(&mut decode::Reader::new(&code_bytes), &table)
            .unwrap_or_else(|error| panic!("{error}"));
        let bytes = writer.into_bytes();
        let decoded = mixins::decode(&mut decode::Reader::new(&bytes), &table, &decoded_code.codes)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded.module_rows, vec![0, second_at as u32]);
        assert_eq!(decoded.module_root(1).map(|mixin| mixin.name.as_str()), Some("Second"));
        assert!(decoded.module_root(2).is_none());
    });
}

#[test]
fn a_body_the_code_table_does_not_hold_has_no_id() {
    let ids = encode::CodeIds::default();
    let orphan = std::rc::Rc::new(every_instruction_once());
    assert!(ids.id_of(&orphan).is_err(), "an unknown body must be an error, not a panic");
}
```

The whole-graph version of this, a closure over a module the image does not carry, needs Task 4's module slices and is written there.

Adjust the first test to the helper names the file already has: `load` (260) answers `(Interpreter, Root)`; parse and resolve the second source the way `load` does; if a `symbols::Builder` has no `finish()`, use whatever the file's existing round-trip helper (`round_trip_code_table`, 690) uses to turn a builder into a `symbols::Table`, and copy its decoding lines. `CodeIds::default()` needs `#[derive(Default)]` on `CodeIds`. If a public constructor is unwanted, write the second test as a unit test inside `encode.rs` instead.

- [ ] **Step 2: Run them to see them fail.** Run: `cargo nextest run -p ns-runtime --test image two_modules a_body_the_code_table`. Expected: FAIL to compile (`plan` takes one class, `module_rows` unknown, `id_of` returns `u32`).

- [ ] **Step 3: Implement the forest.**

`mixins::plan`: walk every module root in order, concatenating preorders. The `ids` map and the depth check cover the whole forest. A slice entry that is no class object contributes nothing, as today.
```rust
pub fn plan<'gc>(
    heap: &Heap<'gc>,
    module_roots: &[ClassReference<'gc>],
) -> Result<Vec<Row>, String> {
    let mut order = Vec::new();
    for root_class in module_roots {
        if let Some(application) = heap.class_application(*root_class) {
            crate::mixin::preorder(&application.mixin, &mut order);
        }
    }
    // … the rest of today's body unchanged, from `let ids: HashMap<…>` on …
}
```
`mixins::encode` and `encode::encode_code_table` take `module_roots: &[ClassReference<'gc>]` and pass it to `plan`. `encode_describing` passes `std::slice::from_ref(&root_class)` for now, and refuses a `root_clause_defect` on each module root.

`check_preorder` accepts a forest and answers each tree's root row:
```rust
/// … (keep the existing explanation, and add:) A table describing several
/// modules is several such trees, one after another in module order, so the
/// walk starts a new tree wherever the previous one ended; each tree's
/// first row is a module's root, which is how the module section names it.
fn check_preorder(raw: &[RawRow]) -> Result<Vec<u32>, DecodeError> {
    let mut module_rows = Vec::new();
    let mut expected: u32 = 0;
    while (expected as usize) < raw.len() {
        module_rows.push(expected);
        let mut stack: Vec<(u32, usize)> = vec![(expected, 1)];
        while let Some((id, depth)) = stack.pop() {
            if id != expected {
                return Err(DecodeError::MixinTableNotATree { row: id });
            }
            if depth > MAX_MIXIN_DEPTH {
                return Err(DecodeError::NestingTooDeep {
                    what: "mixin nesting",
                    limit: MAX_MIXIN_DEPTH,
                });
            }
            expected += 1;
            let row = &raw[id as usize];
            if let Some(class_side) = row.class_side {
                stack.push((class_side, depth + 1));
            }
            for id in row.nested.iter().rev() {
                stack.push((*id, depth + 1));
            }
        }
    }
    Ok(module_rows)
}
```
A row id that points *backwards* into an earlier tree still fails (`id != expected`). A row nothing reaches now starts a tree of its own, and the module section refuses it, because its count must equal the number of trees (Task 4). Until Task 4, `image::load` must refuse `module_rows.len() != 1`, with a new `DecodeError::ModuleCountMismatch { declared: u32, trees: u32 }` whose text says "the image declares {declared} module(s) but its mixin table holds {trees} tree(s)". The existing test that expects `MixinTableNotATree` for an unreachable row (search `MixinTableNotATree` in `image.rs`) now expects `ModuleCountMismatch { declared: 1, trees: 2 }` from `image::load`. Update its assertion and its comment.

`Decoded` gains `pub module_rows: Vec<u32>`, filled from `check_preorder`. `root()` becomes:
```rust
/// Module `module_index`'s root declaration, the first row of its tree.
pub fn module_root(&self, module_index: usize) -> Option<&Rc<Mixin>> {
    let row = *self.module_rows.get(module_index)?;
    self.mixins.get(row as usize)
}
```

`CodeIds::id_of` answers an error instead of panicking. Its callers in `mixins::encode` and `encode_closure_record` propagate it with `?`:
```rust
impl CodeIds {
    /// A body no module the image carries compiled has no id: a graph that
    /// reaches a closure over another module's code. An `Err` here, not a
    /// panic, because what the graph reaches is the program's to decide.
    pub fn id_of(&self, code: &Rc<CompiledCode>) -> Result<u32, String> {
        self.ids.get(&(Rc::as_ptr(code) as usize)).copied().ok_or_else(|| {
            "image encoding cannot serialize a closure over code that no module the \
             image carries compiled"
                .to_string()
        })
    }
}
```
`encode_code` also calls `id_of` for a `MakeClosure` operand. There the body *is* in the table by construction (`collect_code` put it there), so it keeps a panic, with the reason in the message: `.expect("internal: collect_code assigned every nested block an id before its parent")` after an `.ok()`. That is a compiler invariant, not input. `encode_closure_record` returns `Result`; change its signature and its call in `encode_object` accordingly.

- [ ] **Step 4: Update the test helpers and call sites.** Every call in `crates/ns-runtime/tests/image.rs`, `trampoline.rs` and `crates/ns-cli/tests/disasm_snapshots.rs` that passes one class to `plan`, `encode` or `encode_code_table` passes `&[class]`; every `decoded.root()` becomes `decoded.module_root(0)`; every `ids.id_of(code)` in a test that expects a hit becomes `ids.id_of(code).unwrap_or_else(|error| panic!("{error}"))`.

- [ ] **Step 5: Run the tests.** Run: `cargo nextest run -p ns-runtime --test image --test trampoline`. Expected: PASS, the fixture test `the_wire_format_matches_the_golden_fixture` included (one module encodes exactly as before).

- [ ] **Step 6: Run the per-commit gate, then commit.**

```bash
git add crates
git commit -m "refactor: the code and mixin tables describe a forest of modules"
```

---

### Task 4: The module section

The origin, the optional source and the root-class section move out of the header and into one module section after the mixin table, repeated per module. New class and hash tags name a module's root and its levels by module index. The wire changes, so the golden fixture is regenerated.

New layout, for every artifact:

```
header        MAGIC, FORMAT_VERSION, flags (FLAG_HAS_SOURCE: every module carries its source)
symbols
code table
mixin table   a forest, one tree per module, in module order
modules       u32 count (= the mixin table's tree count)
              per module: origin, [source: str, if FLAG_HAS_SOURCE],
                          root section (today's: presence flag, superclass tag,
                          level count, per level clause index + builtin tag,
                          checked against this module's own row)
entry         (unchanged until Task 12)
platform paths, graph, tag-named hashes   (image only, unchanged in shape)
```

**Files:**
- Create: `crates/ns-runtime/src/image/modules.rs`
- Modify: `crates/ns-runtime/src/image/opcodes.rs`, `encode.rs`, `decode.rs`, `inspect.rs`, `mod.rs`
- Modify: `crates/ns-cli/src/commands/load.rs`, `build.rs`, `snapshot.rs`, `disasm.rs` (mechanical: the new `snapshot`/`build` signatures and `LoadedImage.modules`)
- Test: `crates/ns-runtime/tests/image.rs`, `opcode_tags.rs`, `trampoline.rs`; `crates/ns-cli/tests/disasm_snapshots.rs`, `inspect.rs`, `build_load.rs`
- Regenerate: `crates/ns-runtime/tests/fixtures/golden-image.bin` (the ignored test `regenerate_the_golden_fixture`)

**Interfaces:**
- Consumes: Task 3's forest (`plan`, `module_rows`, `module_root`); Task 2's `SourceOrigin::Embedded`.
- Produces:
  ```rust
  // image/modules.rs
  /// One module an artifact carries, as a writer is handed it.
  pub struct ModuleSource<'a, 'gc> {
      pub class: ClassReference<'gc>,
      pub origin: &'a SourceOrigin,
      pub source: Option<&'a str>,
  }
  /// The module classes an artifact names by tag rather than carries as
  /// records: each module's root class and the levels its own `<:` clauses
  /// added, lowest first, by module index.
  pub struct ModuleClasses<'gc> {
      pub roots: Vec<ClassReference<'gc>>,
      pub levels: Vec<Vec<ClassReference<'gc>>>,
  }
  pub enum ModuleClass { Root(u32), Level { module: u32, position: u32 } }
  impl<'gc> ModuleClasses<'gc> {
      pub fn of(heap: &Heap<'gc>, roots: &[ClassReference<'gc>]) -> ModuleClasses<'gc>;
      pub fn find(&self, class: ClassReference<'gc>) -> Option<ModuleClass>;
      pub fn root(&self, module: u32) -> Option<ClassReference<'gc>>;
      pub fn level(&self, module: u32, position: u32) -> Option<ClassReference<'gc>>;
  }
  pub struct DecodedModule { pub origin: SourceOrigin, pub source: Option<String>, pub section: decode::RootSection }
  pub fn encode_modules<'gc>(writer: &mut Writer, heap: &Heap<'gc>, modules: &[ModuleSource<'_, 'gc>], with_source: bool) -> Result<(), String>;
  pub fn decode_modules(reader: &mut Reader, mixins: &mixins::Decoded, with_source: bool) -> Result<Vec<DecodedModule>, DecodeError>;
  pub fn build_module_classes<'gc>(mutator: &mut Mutator<'gc>, mixins: &mixins::Decoded, modules: &[DecodedModule]) -> Result<ModuleClasses<'gc>, DecodeError>;

  // image/mod.rs
  pub fn snapshot<'gc>(mutator: &Mutator<'gc>, modules: &[ModuleSource<'_, 'gc>], platform: &Value<'gc>, roots: &[Value<'gc>]) -> Result<Vec<u8>, String>;
  pub fn build<'gc>(heap: &Heap<'gc>, modules: &[ModuleSource<'_, 'gc>], entry: Symbol) -> Result<Vec<u8>, String>;
  pub struct LoadedModule { pub class: Root, pub origin: SourceOrigin, pub source: Option<String> }
  pub struct LoadedImage { pub interpreter: Interpreter, pub modules: Vec<LoadedModule>, pub roots: Vec<Root>, pub entry: Option<Symbol>, pub stateless: bool }
  ```
  `LoadedImage.root_class`, `.origin` and `.source` are gone; module 0 is the application's module.
- Opcodes: `CLASS_MODULE = 6` (followed by a `u32` module index), `CLASS_MODULE_LEVEL = 7` (followed by a `u32` module index and a `u32` level position); `HASHED_MODULE_CLASS = 5` (followed by a `u32` module index), `HASHED_MODULE_LEVEL = 6` (module index, position); `RETIRED_CLASS_TAGS: [u8; 2] = [1, 3]`, `RETIRED_HASHED_TAGS: [u8; 1] = [1]`. The `CLASS_METACLASS` and `HASHED_METACLASS` references accept `CLASS_BUILTIN`, `CLASS_MODULE`, `CLASS_MODULE_LEVEL` and `CLASS_PLATFORM_PATH`.
- New `DecodeError`s: `ModuleCountMismatch { declared: u32, trees: u32 }` (Task 3), `BadModuleReference { module: u32 }`, `BadModuleLevel { module: u32, position: u32 }`, `MixedModuleSources` is an *encoding* error string, not a `DecodeError`.

`HASHED_MODULE_LEVEL` closes a gap. Today a root level's own identity hash is not recorded (only its metaclass's is), so a program that keeps `self class superclass hash` for a module applying `<: Value` would see it change across an image.

- [ ] **Step 1: Write the failing tests.**

In `opcode_tags.rs`, extend `graph_section_tags_are_fixed` and `no_two_graph_tags_in_a_family_share_a_number`. Remove `CLASS_USER_ROOT`, `CLASS_ROOT_LEVEL` and `HASHED_ROOT_CLASS` from both, and add:
```rust
assert_eq!(opcodes::CLASS_MODULE, 6);
assert_eq!(opcodes::CLASS_MODULE_LEVEL, 7);
assert_eq!(opcodes::HASHED_MODULE_CLASS, 5);
assert_eq!(opcodes::HASHED_MODULE_LEVEL, 6);
assert_eq!(opcodes::RETIRED_CLASS_TAGS, [1, 3]);
assert_eq!(opcodes::RETIRED_HASHED_TAGS, [1]);
```
Also add a check that no live class tag is a retired one and no live hashed tag is a retired one, in the style of the file's existing retired-literal check (search `RETIRED_LITERALS` in `opcode_tags.rs` and copy its shape).

In `image.rs`, add, next to the whole-image snapshot tests (search `fn snapshot_of(`):
```rust
#[test]
fn an_image_of_two_modules_restores_both_and_the_graph_reaching_them() {
    // First's instance holds Second's class and an instance of it; after a
    // load, both module classes are rebuilt, and the instance answers its
    // own class's method.
    let second_source = indoc! {"
        Newspeak3
        'Test'
        class Second = Object new () ( public answer = ( ^2 ) ) : ()
    "};
    let first_source = indoc! {"
        Newspeak3
        'Test'
        class First = Object new (| public held ::= nil. public made ::= nil. |) () : ()
    "};
    let (mut interpreter, first) = load(first_source);
    let second = load_another(&mut interpreter, second_source, "Second.ns");
    let instance = expect_ok(interpreter.instantiate(&first, &[]), "First new");
    let second_instance = expect_ok(interpreter.instantiate(&second, &[]), "Second new");
    expect_ok(interpreter.send(&instance, Symbol::intern("held:"), &[second.clone()]), "held:");
    expect_ok(interpreter.send(&instance, Symbol::intern("made:"), &[second_instance]), "made:");
    let first_origin = SourceOrigin::File("First.ns".into());
    let second_origin = SourceOrigin::File("Second.ns".into());
    let platform = interpreter.platform();
    let bytes = interpreter
        .enter(|mutator| {
            let modules = [
                image::ModuleSource { class: class_reference(mutator, &first), origin: &first_origin, source: None },
                image::ModuleSource { class: class_reference(mutator, &second), origin: &second_origin, source: None },
            ];
            let platform = mutator.fetch(&platform);
            image::snapshot(mutator, &modules, &platform, &[mutator.fetch(&instance)])
        })
        .unwrap_or_else(|error| panic!("{error}"));
    let mut loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(loaded.modules.len(), 2);
    assert!(matches!(&loaded.modules[1].origin, SourceOrigin::File(path) if path.ends_with("Second.ns")));
    let root = loaded.roots[0].clone();
    let made = send_to(&mut loaded, &root, "made");
    let answer = send_to(&mut loaded, &made, "answer");
    let held = send_to(&mut loaded, &root, "held");
    let module_class = loaded.modules[1].class.clone();
    loaded.interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&answer), Value::SmallInteger(2)));
        assert!(same_value(&mutator.fetch(&held), &mutator.fetch(&module_class)));
    });
}
```
`load_another` is a new helper beside `load` (260). It parses, resolves with `SourceOrigin::File(name.into())`, and calls `interpreter.load_module`. `same_value` compares two `Value::Object`s with `.same`. Write it if the file has no equivalent; `same_object` (10979) works on a `LoadedImage` and may serve directly.

Also add these crafted-input tests (each is a `load_error(&bytes)` expectation, built by taking a real two-module image and corrupting one field; find the field's offset with `image::inspect::inspect(&bytes)`'s section offsets, as the existing corruption tests do):
- `a_module_count_that_disagrees_with_the_trees_is_refused`: set the count to 1 in a two-module image. Expect the text of `ModuleCountMismatch { declared: 1, trees: 2 }`.
- `a_module_count_past_the_bytes_left_is_refused`: set the count to `u32::MAX`. Expect a `DecodeError` (`UnexpectedEndOfFile` or `ModuleCountMismatch`), never an allocation or panic.
- `a_module_class_tag_past_the_modules_is_refused`: a crafted graph whose object's class is `CLASS_MODULE` + 7 in a one-module image. Expect `BadModuleReference { module: 7 }`. Build it with the `GraphFixture` helper (2353) the way `a_root_level_class_tag_past_the_roots_levels_is_a_decode_error_not_a_panic` does, and adapt that test itself to `CLASS_MODULE_LEVEL` + module 0 + position 5 (`BadModuleLevel { module: 0, position: 5 }`).
- `a_root_section_clause_is_checked_against_its_own_modules_row`: module 1's section names clause index 0 while module 1's row declares no `<:` clause (module 0's does). Expect `BadMixinApplicationClause { row: <module 1's row>, index: 0 }`.
- `a_graph_reaching_a_module_the_image_does_not_carry_is_refused`: `First`'s instance holds `Second`'s class, and a snapshot is given `First` alone. Expect an `Err` naming `Second`: today's "not part of the module's declaration tree" text, reworded to "not part of any module the image carries".
- `a_closure_over_code_no_module_carries_is_refused_not_a_panic`: `First`'s instance holds a block made by a method of `Second` (`Second new maker`, where `maker = ( ^[42] )`), and a snapshot is given `First` alone. Expect an `Err`, not a panic: Task 3's `id_of` error, propagated through `encode_closure_record`.
- `a_module_levels_own_hash_survives_an_image`: a module `class M = Object <: Value (…)` whose instance keeps `self class superclass hash` in a slot, snapshotted and loaded. The slot's value equals the loaded level's `hash`.

- [ ] **Step 2: Run them to see them fail.** Run: `cargo nextest run -p ns-runtime --test image --test opcode_tags`. Expected: FAIL to compile.

- [ ] **Step 3: Implement `image/modules.rs`.**

```rust
//! The module section: which top-level modules an artifact carries, where
//! each came from, and what its root class inherits -- and the module
//! classes a graph names by tag (`CLASS_MODULE`, `CLASS_MODULE_LEVEL`)
//! rather than carries as records, because `load` rebuilds them from the
//! mixin table before the graph exists.

use std::rc::Rc;

use super::decode::{self, DecodeError, Reader, RootSection};
use super::encode::{self, Writer};
use super::mixins;
use crate::heap::Heap;
use crate::types::{ClassReference, SourceOrigin};
use crate::Mutator;

pub struct ModuleSource<'a, 'gc> {
    pub class: ClassReference<'gc>,
    pub origin: &'a SourceOrigin,
    pub source: Option<&'a str>,
}

pub struct ModuleClasses<'gc> {
    pub roots: Vec<ClassReference<'gc>>,
    pub levels: Vec<Vec<ClassReference<'gc>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleClass {
    Root(u32),
    Level { module: u32, position: u32 },
}

impl<'gc> ModuleClasses<'gc> {
    pub fn of(heap: &Heap<'gc>, roots: &[ClassReference<'gc>]) -> ModuleClasses<'gc> {
        ModuleClasses {
            roots: roots.to_vec(),
            levels: roots
                .iter()
                .map(|root| encode::root_levels(heap, *root))
                .collect(),
        }
    }

    pub fn find(&self, class: ClassReference<'gc>) -> Option<ModuleClass> {
        for (module, root) in self.roots.iter().enumerate() {
            if root.same(class) {
                return Some(ModuleClass::Root(module as u32));
            }
            if let Some(position) = self.levels[module].iter().position(|level| level.same(class)) {
                return Some(ModuleClass::Level {
                    module: module as u32,
                    position: position as u32,
                });
            }
        }
        None
    }

    pub fn root(&self, module: u32) -> Option<ClassReference<'gc>> {
        self.roots.get(module as usize).copied()
    }

    pub fn level(&self, module: u32, position: u32) -> Option<ClassReference<'gc>> {
        self.levels.get(module as usize)?.get(position as usize).copied()
    }
}

/// Every module's origin, source when `with_source`, and root section, in
/// module order -- the order of the mixin table's trees.
pub fn encode_modules<'gc>(
    writer: &mut Writer,
    heap: &Heap<'gc>,
    modules: &[ModuleSource<'_, 'gc>],
    with_source: bool,
) -> Result<(), String> {
    writer.write_u32(encode::u32_field("module count", modules.len()));
    for module in modules {
        encode::encode_source_origin(writer, module.origin);
        if with_source {
            let Some(source) = module.source else {
                return Err("image encoding carries source for every module or for none: \
                            one module was handed none"
                    .to_string());
            };
            writer.write_str(source);
        }
        encode::encode_root_class(writer, heap, module.class)?;
    }
    Ok(())
}

/// The smallest a module record can be: an origin tag, an empty name's
/// length, and a root section with no level.
const MIN_MODULE_BYTES: usize = 1 + 4 + 1 + 1 + 4;

pub fn decode_modules(
    reader: &mut Reader,
    mixins: &mixins::Decoded,
    with_source: bool,
) -> Result<Vec<DecodedModule>, DecodeError> {
    let declared = reader.read_u32()?;
    let trees = mixins.module_rows.len() as u32;
    if declared != trees {
        return Err(DecodeError::ModuleCountMismatch { declared, trees });
    }
    let count = decode::bounded_count(declared as usize, MIN_MODULE_BYTES, reader)?;
    let mut modules = Vec::with_capacity(count);
    for module_index in 0..count {
        let origin = decode::decode_source_origin(reader)?;
        let source = if with_source { Some(reader.read_str()?) } else { None };
        let section = decode::decode_root_section(reader, mixins, module_index)?;
        modules.push(DecodedModule { origin, source, section });
    }
    Ok(modules)
}

pub struct DecodedModule {
    pub origin: SourceOrigin,
    pub source: Option<String>,
    pub section: RootSection,
}

/// Each module's root class and levels, rebuilt in module order -- the order
/// the program loaded them in, which is what keeps every identity hash the
/// bootstrap draws the same as the run that wrote the image.
pub fn build_module_classes<'gc>(
    mutator: &mut Mutator<'gc>,
    mixins: &mixins::Decoded,
    modules: &[DecodedModule],
) -> Result<ModuleClasses<'gc>, DecodeError> {
    let mut roots = Vec::with_capacity(modules.len());
    let mut levels = Vec::with_capacity(modules.len());
    for (module_index, module) in modules.iter().enumerate() {
        let (root, root_levels) =
            decode::build_root_class(mutator, mixins, module_index, &module.section)?;
        roots.push(root);
        levels.push(root_levels);
    }
    Ok(ModuleClasses { roots, levels })
}
```
Make `encode::u32_field`, `decode::bounded_count` and `decode::Reader` accessible to the sibling module (`pub(crate)` where they are private). `decode_root_section` and `build_root_class` gain a `module_index: usize` parameter. They read `mixins.module_root(module_index)` and report `BadMixinApplicationClause { row: mixins.module_rows[module_index], index }`.

- [ ] **Step 4: Thread `ModuleClasses` through the graph code.** Mechanically, in `encode.rs` and `decode.rs`: every `(root_class, root_levels)` pair, every `root_class: ClassReference` and `root_levels: &[ClassReference]` parameter or field that exists to recognise module classes, becomes one `modules: &ModuleClasses<'gc>`:

  - `is_prebuilt_class` (919): `… || modules.find(object).is_some() || …` replaces the two `same` checks.
  - `is_class_tag`, `GraphTable::build`, `GraphTable::visit_value`, `EncodeContext` (`root_class` and `root_levels` fields become `modules`), `encode_graph_section` (its `root_class` parameter becomes `modules: &ModuleClasses<'gc>`).
  - `encode_class_tag`, the `None` arms:
    ```rust
    None => match context.modules.find(class) {
        Some(ModuleClass::Root(module)) => {
            writer.write_u8(CLASS_MODULE);
            writer.write_u32(module);
        }
        Some(ModuleClass::Level { module, position }) => {
            writer.write_u8(CLASS_MODULE_LEVEL);
            writer.write_u32(module);
            writer.write_u32(position);
        }
        None => {
            writer.write_u8(CLASS_RECORD);
            writer.write_u32(context.table.object_id(context.heap, class));
        }
    },
    ```
  - `encode_tag_named_hashes`: the `HASHED_ROOT_CLASS` entry becomes, per module in order, `HASHED_MODULE_CLASS` + module index; then per module and level `HASHED_MODULE_LEVEL` + module + position. The `prebuilt` list for metaclass entries pushes each module root as `[CLASS_MODULE, index…]` and each level as `[CLASS_MODULE_LEVEL, module…, position…]`. Keep the doc comment's fixed-order description true: builtins, then module roots in module order, then module levels, then `platform`, then path-named objects, then metaclasses.
  - `decode.rs`: `DecodeContext`'s `root_class` and `root_levels` fields become `modules: &'a ModuleClasses<'gc>`. `decode_class_tag_after` and `read_prebuilt_class` get arms:
    ```rust
    CLASS_MODULE => {
        let module = reader.read_u32()?;
        modules.root(module).ok_or(DecodeError::BadModuleReference { module })
    }
    CLASS_MODULE_LEVEL => {
        let module = reader.read_u32()?;
        let position = reader.read_u32()?;
        modules
            .level(module, position)
            .ok_or(DecodeError::BadModuleLevel { module, position })
    }
    ```
    and lose `CLASS_USER_ROOT` and `CLASS_ROOT_LEVEL`, which now fall through to `InvalidTag`. `read_root_level` goes.
  - `method_declaration` (2494): the `None` fallback reads `context.modules.root(0)`. Keep the comment and say "the application's module, module 0, stands in".
  - `decode_tag_named_hashes` takes `modules: &ModuleClasses<'gc>` instead of `root_class`, with the two new tags.
  - `inspect.rs`: sections "origin", "source" and "root class" become one "modules" section, detail `"{count} modules"`. `Report` loses `origin` and `has_source` in favour of `modules: Vec<ModuleReport>`:
    ```rust
    pub struct ModuleReport { pub origin: String, pub has_source: bool, pub root: String }
    ```
    `root` is today's "root class" detail string. `render` prints `modules: N` and one line per module, `  <index>: <origin>, source <present|absent>, <root>`. The graph walk prints `<module N>` and `<module N level M>` where it printed `<root class>` and `<root class level N>`, and `<module N> hash H` for `HASHED_MODULE_CLASS`.

- [ ] **Step 5: Rewrite `image/mod.rs`'s writers and `load` on the new layout.**
  - `encode_describing(modules: &[ModuleSource], heap)` checks each module's `root_clause_defect`, and passes the module classes to `encode_code_table` and `mixins::encode`.
  - `write_describing` writes the header (`FLAG_HAS_SOURCE` when every module carries source; a mix is an `Err` from `encode_modules`), symbols, code, mixins, then `modules::encode_modules`.
  - `snapshot(mutator, modules, platform, roots)` builds `ModuleClasses::of(heap, &classes)` and passes it to `encode_graph_section` and `encode_tag_named_hashes`.
  - `build(heap, modules, entry)` is the same with `FLAG_STATELESS`.
  - `load` decodes the header, symbols, code, mixins, then `decode_modules(&mut reader, &mixins, header.has_source)` and `build_module_classes`. It roots every module's root and level class (today it roots the one root and its levels in `Described`), and answers `LoadedImage { modules: Vec<LoadedModule>, … }` with each module's class rooted, its origin and its source.

  The header's source blob and origin go (they are per module now). Update the module doc comment at the top of `mod.rs` to the new section order.

- [ ] **Step 6: Adapt every caller.** The compiler lists them:
  - `crates/ns-cli/src/commands/snapshot.rs` and `build.rs` pass one `ModuleSource { class, origin: &loaded.origin, source: debug_source.then_some(&loaded.source) }`.
  - `load.rs` reads `loaded.modules[0].class`, and `--print-source` prints every module's source in module order.
  - `disasm.rs` uses `modules[0].class`.
  - In the tests:
    - `image.rs`'s helpers `snapshot_of`, `snapshot_world`, `snapshot_module`, `snapshot_of_without_source` and the direct `image::snapshot`/`image::build` calls pass one-module slices;
    - `loaded.root_class` becomes `loaded.modules[0].class`;
    - `GraphFixture` and `round_trip_graph` build a `ModuleClasses::of(heap, &[class])`;
    - assertions on the strings `<root class>`, `"origin"`, `"root class"` and `"source"` in `image.rs`, `trampoline.rs`, `crates/ns-cli/tests/inspect.rs` and `build_load.rs` name the new section and strings;
    - `a_built_binary_and_a_snapshot_of_the_same_source_agree_up_to_the_root_class` (image.rs) and `a_build_and_a_snapshot_agree_on_the_describing_sections` (build_load.rs) compare from "symbols" through "modules";
    - `image_with_graph_section` (7102) and `a_relabelled_snapshot_…` recompute their offsets from `inspect`'s sections rather than from a constant;
    - the tests that used `HASHED_ROOT_CLASS` / `CLASS_ROOT_LEVEL` use the module tags.

- [ ] **Step 7: Regenerate the wire fixture.** Run: `cargo nextest run -p ns-runtime --test image regenerate_the_golden_fixture --run-ignored only`, then `cargo nextest run -p ns-runtime --test image the_wire_format_matches_the_golden_fixture`. Expected: PASS. Then `cargo run -p ns-cli -- inspect crates/ns-runtime/tests/fixtures/golden-image.bin` must show one `modules` section and the graph naming `<module 0>` where it named `<root class>`.

- [ ] **Step 8: Run the tests.** Run: `cargo nextest run --workspace --exclude ns-lsp`. Expected: PASS.

- [ ] **Step 9: Run the per-commit gate, then commit.**

```bash
git add crates
git commit -m "feat: an image carries several modules, each named by its index"
```

---

### Task 5: `image::package` and `FLAG_START`

**Files:**
- Modify: `crates/ns-runtime/src/image/opcodes.rs` or `encode.rs` (wherever `FLAG_HAS_SOURCE` lives: `encode.rs:103`)
- Modify: `crates/ns-runtime/src/image/mod.rs`, `decode.rs` (`Header`, `read_header`), `inspect.rs`
- Test: `crates/ns-runtime/tests/image.rs`, `opcode_tags.rs`

**Interfaces:**
- Consumes: Task 4's `ModuleSource`, `snapshot`'s internals.
- Produces:
  - `encode::FLAG_START: u8 = 4`, in `KNOWN_FLAGS`;
  - `decode::Header { has_source, stateless, start }`;
  - `LoadedImage.start: bool`;
  - `pub fn package<'gc>(mutator: &Mutator<'gc>, modules: &[ModuleSource<'_, 'gc>], application: Value<'gc>) -> Result<Vec<u8>, String>`;
  - `inspect::Report.start: bool`, rendered as `start: yes|no`.

- [ ] **Step 1: Write the failing tests.** In `opcode_tags.rs`:
```rust
#[test]
fn header_flags_are_fixed() {
    assert_eq!(encode::FLAG_HAS_SOURCE, 1);
    assert_eq!(encode::FLAG_STATELESS, 2);
    assert_eq!(encode::FLAG_START, 4);
}
```
In `image.rs`: the existing `an_unknown_header_flag_is_still_rejected_now_that_two_are_known` (7946) writes `0b100`. Rename it `an_unknown_header_flag_is_still_rejected_now_that_three_are_known`, and make it write `0b1000`.
```rust
#[test]
fn a_packaged_application_loads_as_a_started_image_with_one_root() {
    let (mut interpreter, class) = load(indoc! {"
        Newspeak3
        'Test'
        class App = Object new (| public count ::= 41. |) (
            public main: platform args: arguments = ( count:: count + 1. ^count )
        ) : ()
    "});
    let application = expect_ok(interpreter.instantiate(&class, &[]), "App new");
    let origin = SourceOrigin::File("App.ns".into());
    let bytes = interpreter
        .enter(|mutator| {
            let modules = [image::ModuleSource { class: class_reference(mutator, &class), origin: &origin, source: None }];
            image::package(mutator, &modules, mutator.fetch(&application))
        })
        .unwrap_or_else(|error| panic!("{error}"));
    let mut loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    assert!(loaded.start);
    assert_eq!(loaded.roots.len(), 1);
    let root = loaded.roots[0].clone();
    let count = send_to(&mut loaded, &root, "count");
    loaded.interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&count), Value::SmallInteger(41)));
    });
}

#[test]
fn package_refuses_an_application_that_reaches_the_platform() {
    let (mut interpreter, class) = load(indoc! {"
        Newspeak3
        'Test'
        class App = Object new (| public held ::= nil. |) () : ()
    "});
    let application = expect_ok(interpreter.instantiate(&class, &[]), "App new");
    let platform = interpreter.platform();
    expect_ok(interpreter.send(&application, Symbol::intern("held:"), &[platform]), "held:");
    let origin = SourceOrigin::File("App.ns".into());
    let refused = interpreter.enter(|mutator| {
        let modules = [image::ModuleSource { class: class_reference(mutator, &class), origin: &origin, source: None }];
        image::package(mutator, &modules, mutator.fetch(&application))
    });
    let error = match refused {
        Ok(_) => panic!("an application holding the platform was packaged"),
        Err(error) => error,
    };
    assert!(error.contains("platform"), "{error}");
}

#[test]
fn a_snapshot_is_not_started() {
    let bytes = snapshot_of(indoc! {"
        Newspeak3
        'Test'
        class App = Object new () () : ()
    "});
    let loaded = image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    assert!(!loaded.start);
}
```

- [ ] **Step 2: Run them to see them fail.** Run: `cargo nextest run -p ns-runtime --test image --test opcode_tags package started header_flag snapshot_is_not`. Expected: FAIL to compile.

- [ ] **Step 3: Implement.**

`encode.rs`:
```rust
/// The image is an application packaged just before `main:args:`: `load`
/// starts it by sending `main: platform args:` to the graph's one root
/// (`docs/design/image-format.md`).
pub const FLAG_START: u8 = 4;
pub const KNOWN_FLAGS: u8 = FLAG_HAS_SOURCE | FLAG_STATELESS | FLAG_START;
```
`read_header` sets `start: flags & FLAG_START != 0`. It refuses `FLAG_STATELESS | FLAG_START` together, with a new `DecodeError::ContradictoryHeaderFlags { flags }` ("image header sets flags {:#04x}, which contradict each other: a stateless artifact has no graph to start").

`mod.rs`: factor `snapshot`'s body into a private `fn write_image(mutator, modules, platform: &Value, roots, flags: u8)`. Then:
```rust
/// Serializes an application assembled but not yet started: its instance is
/// the graph's one root, and `FLAG_START` tells `load` to send it
/// `main: platform args:`.
///
/// No platform exists for it yet, so none is named: the platform value is
/// `nil` here, which means no platform path is discovered and a graph that
/// reaches the platform or anything shipped meets the encoder's refusal of a
/// shipped object no path names. An application that reached the platform
/// could only have been handed it, and nothing hands one out before
/// `main:args:`.
pub fn package<'gc>(
    mutator: &Mutator<'gc>,
    modules: &[ModuleSource<'_, 'gc>],
    application: Value<'gc>,
) -> Result<Vec<u8>, String> {
    write_image(mutator, modules, &Value::Nil, &[application], encode::FLAG_START)
}
```
Check what the encoder does today with the platform object when the `platform` argument is `nil`. `GraphTable::visit_value` skips only an object equal to `platform`, so the real platform is visited like any object. `shipped_without_path` should refuse it: it is an instance of `PlatformFactory.ns`'s nested `Platform`, a shipped class. If the refusal text does not contain "platform", make `package`'s test assertion match the actual refusal ("shipped") rather than weakening the refusal.

`inspect.rs`: `Report.start`, `render` prints `start: yes|no` after `stateless:`.

- [ ] **Step 4: Run the tests.** Run: `cargo nextest run -p ns-runtime`. Expected: PASS.

- [ ] **Step 5: Add `package` to the truncation and mutation sweeps.** `every_truncation_of_a_binary_is_reported_rather_than_panicking` (8201) and the snapshot truncation test (search `every_truncation` in `image.rs`): add a third input, the packaged `App` of Step 1. Its every prefix must be an `Err` from `image::load`.

- [ ] **Step 6: Run the per-commit gate, then commit.**

```bash
git add crates
git commit -m "feat: image::package writes an application to be started"
```

---

### Task 6: The project file

**Files:**
- Modify: `crates/ns-cli/Cargo.toml` (`serde_json = "1"` under `[dependencies]`)
- Create: `crates/ns-cli/src/project.rs`
- Modify: `crates/ns-cli/src/main.rs` (`mod project;`)

**Interfaces:**
- Produces:
  ```rust
  pub(crate) struct Project { pub(crate) root: PathBuf, pub(crate) modules: Vec<PathBuf> }
  pub(crate) fn read_project(path: &Path) -> Result<Project, String>
  pub(crate) fn resolve_written_path(project_directory: &Path, written: &str) -> PathBuf
  pub(crate) fn is_project_file(path: &Path) -> bool   // extension `json`, case-insensitive
  ```

- [ ] **Step 1: Write the failing tests** as a `#[cfg(test)] mod tests` at the bottom of `project.rs` (unit tests, since the functions are crate-private):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "ns-next-project-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap_or_else(|error| panic!("{error}"));
        directory
    }

    #[test]
    fn a_relative_path_is_relative_to_the_project_file_with_either_separator() {
        let directory = Path::new("projects").join("awfy");
        assert_eq!(
            resolve_written_path(&directory, "modules/Sieve.ns"),
            directory.join("modules").join("Sieve.ns")
        );
        assert_eq!(
            resolve_written_path(&directory, "modules\\Sieve.ns"),
            directory.join("modules").join("Sieve.ns")
        );
        assert_eq!(
            resolve_written_path(&directory, "../shared/Tree.ns"),
            directory.join("..").join("shared").join("Tree.ns")
        );
    }

    #[test]
    fn an_absolute_path_is_taken_as_written() {
        let directory = Path::new("projects");
        #[cfg(windows)]
        {
            assert_eq!(resolve_written_path(directory, "C:\\x\\A.ns"), PathBuf::from("C:\\x\\A.ns"));
            assert_eq!(resolve_written_path(directory, "C:/x/A.ns"), PathBuf::from("C:\\x\\A.ns"));
        }
        #[cfg(not(windows))]
        {
            assert_eq!(resolve_written_path(directory, "/x/A.ns"), PathBuf::from("/x/A.ns"));
            assert_eq!(resolve_written_path(directory, "\\x\\A.ns"), PathBuf::from("/x/A.ns"));
        }
    }

    #[test]
    fn modules_may_be_absent_or_empty() {
        let directory = scratch("optional-modules");
        let absent = directory.join("absent.json");
        std::fs::write(&absent, r#"{ "root": "App.ns" }"#).unwrap_or_else(|error| panic!("{error}"));
        let project = read_project(&absent).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(project.root, directory.join("App.ns"));
        assert!(project.modules.is_empty());
        let empty = directory.join("empty.json");
        std::fs::write(&empty, r#"{ "root": "App.ns", "modules": [] }"#).unwrap_or_else(|error| panic!("{error}"));
        assert!(read_project(&empty).unwrap_or_else(|error| panic!("{error}")).modules.is_empty());
    }

    #[test]
    fn an_unknown_field_and_a_missing_root_are_refused() {
        let directory = scratch("refusals");
        let unknown = directory.join("unknown.json");
        std::fs::write(&unknown, r#"{ "root": "App.ns", "entry": "main:args:" }"#).unwrap_or_else(|error| panic!("{error}"));
        let error = read_project(&unknown).err().unwrap_or_default();
        assert!(error.contains("entry"), "{error}");
        let missing = directory.join("missing.json");
        std::fs::write(&missing, r#"{ "modules": [] }"#).unwrap_or_else(|error| panic!("{error}"));
        let error = read_project(&missing).err().unwrap_or_default();
        assert!(error.contains("root"), "{error}");
    }
}
```

- [ ] **Step 2: Run them to see them fail.** Run: `cargo nextest run -p ns-cli --bin ns-next project`. Expected: FAIL to compile.

- [ ] **Step 3: Implement `project.rs`.**

```rust
//! The project file: a program of several modules, named in JSON. `root` is
//! the application's source; `modules` are the other modules its manifest
//! holds (`docs/design/image-format.md`, "Planned: one artifact, packaged
//! before `main:args:`").

use std::path::{Path, PathBuf};

/// The file as written. Unknown fields are refused so that a misspelt key
/// is a message rather than a module silently missing.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    root: String,
    #[serde(default)]
    modules: Vec<String>,
}

pub(crate) struct Project {
    pub(crate) root: PathBuf,
    pub(crate) modules: Vec<PathBuf>,
}

pub(crate) fn is_project_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

pub(crate) fn read_project(path: &Path) -> Result<Project, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("reading {}: {error}", path.display()))?;
    let file: ProjectFile = serde_json::from_str(&text)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let directory = path.parent().unwrap_or(Path::new(""));
    Ok(Project {
        root: resolve_written_path(directory, &file.root),
        modules: file
            .modules
            .iter()
            .map(|written| resolve_written_path(directory, written))
            .collect(),
    })
}

/// A path as a project file writes it: either separator, relative to the
/// project file or absolute. Both separators are accepted everywhere so one
/// project file serves every operating system; on a host whose separator is
/// `/`, a `\` is taken as one rather than as a character of a file name.
pub(crate) fn resolve_written_path(project_directory: &Path, written: &str) -> PathBuf {
    let normalized: PathBuf = written
        .split(['/', '\\'])
        .enumerate()
        .fold(PathBuf::new(), |mut path, (index, component)| {
            if index == 0 && component.is_empty() {
                path.push(std::path::MAIN_SEPARATOR_STR);
            } else if index == 0 && component.ends_with(':') {
                // A Windows drive, `C:`; pushed with its separator so `C:x`
                // does not become a drive-relative path.
                path.push(format!("{component}{}", std::path::MAIN_SEPARATOR));
            } else if !component.is_empty() {
                path.push(component);
            }
            path
        });
    if normalized.is_absolute() || normalized.has_root() {
        normalized
    } else {
        project_directory.join(normalized)
    }
}
```
A path `/x/A.ns` on Windows `has_root()` but is not absolute. It names the current drive's root, and joining it onto the project directory would replace the directory anyway. The explicit `has_root()` makes that intent visible.

- [ ] **Step 4: Run the tests.** Run: `cargo nextest run -p ns-cli --bin ns-next project`. Expected: PASS. Also check with `cargo clippy -p ns-cli --all-targets --target x86_64-unknown-linux-gnu` that the `cfg(not(windows))` branch compiles. The target is installed for `ns-memory`; `rustup target add x86_64-unknown-linux-gnu` if not.

- [ ] **Step 5: Run the per-commit gate, then commit.** `project.rs` is dead code until Task 7. If clippy reports `dead_code` for items only tests use, put `#[cfg_attr(not(test), allow(dead_code))]` on the module declaration in `main.rs` with a comment "used from Task 7's `program.rs`", and remove it in Task 7.

```bash
git add crates/ns-cli Cargo.lock
git commit -m "feat: ns-cli reads a JSON project file"
```

---

### Task 7: The manifest, and `run` assembles an application

**Files:**
- Create: `crates/ns-runtime/core-library/Manifest.ns`
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs` (the constant), `crates/ns-runtime/src/lib.rs` (re-export)
- Modify: `crates/ns-runtime/src/symbol.rs` (`PACKAGE_USING`, `MAIN_ARGUMENTS`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Interpreter::responds_publicly`)
- Create: `crates/ns-runtime/tests/manifest.rs`
- Create: `crates/ns-cli/src/program.rs`
- Modify: `crates/ns-cli/src/entry.rs`, `source.rs`, `commands/run.rs`, `main.rs` (`Run` loses `--send`)
- Test: `crates/ns-cli/tests/run.rs`

**Interfaces:**
- Consumes: Task 6's `project::{read_project, is_project_file, Project}`.
- Produces:
  - `ns_runtime::MANIFEST_SOURCE: &str`
  - `Symbol::PACKAGE_USING` (`"packageUsing:"`), `Symbol::MAIN_ARGUMENTS` (`"main:args:"`)
  - `Interpreter::responds_publicly(&mut self, receiver: &Root, selector: Symbol) -> bool`
  - `program.rs`:
    ```rust
    pub(crate) enum Assembly { New, Package }
    pub(crate) struct Program { pub(crate) assembly: Assembly, pub(crate) sources: Vec<source::LoadedSource> }  // module order: root, project modules, Manifest
    pub(crate) struct Loaded { pub(crate) interpreter: ns_runtime::Interpreter, pub(crate) classes: Vec<ns_runtime::Root> }  // same order
    pub(crate) fn read_program(path: &Path) -> Result<Program, ExitCode>
    pub(crate) fn load_program(program: &Program, lowering: ns_runtime::Lowering) -> Loaded
    pub(crate) fn instantiate_application(program: &Program, loaded: &mut Loaded) -> Result<ns_runtime::Root, ExitCode>
    pub(crate) fn module_sources<'a, 'gc>(program: &'a Program, classes: &[ns_runtime::Root], mutator: &ns_runtime::Mutator<'gc>, with_source: bool) -> Vec<ns_runtime::image::ModuleSource<'a, 'gc>>
    ```
  - `entry.rs`: `pub(crate) fn main_arguments(interpreter: &mut Interpreter, program_arguments: &[String]) -> Vec<Root>` and `pub(crate) fn send_main(interpreter: &mut Interpreter, application: &Root, program_arguments: &[String]) -> HostResult`; `report_entry_send` and the statistics printers stay. `factory_arguments_for`, `selector_arity`, `entry_arguments` and `instantiate_root` go once their last caller is gone (Tasks 9 to 11; keep them until then, with `#[allow(dead_code)]` only if clippy asks).

- [ ] **Step 1: Write the manifest and its runtime test.**

`crates/ns-runtime/core-library/Manifest.ns`:
```newspeak
Newspeak3
'Core-Library'
(* What a project's root receives as `packageUsing:`'s argument: every
   module the project file names, answered by the name its source declares
   (docs/design/image-format.md, "Planned: one artifact, packaged before
   `main:args:`"). Newspeak on Squeak's `Namespacing.ns` and Primordial
   Soup's `CompilerApp.ns` answer the same way, from `doesNotUnderstand:`
   over a map. Not privileged: it holds no mirror and is compiled into the
   program like any module, so an application may keep it. *)
class Manifest names: moduleNames definitions: moduleDefinitions = (
    | private names = moduleNames.
      private definitions = moduleDefinitions. |
) (
    protected doesNotUnderstand: message = (
        | index |
        index:: 1.
        [index <= names size] whileTrue: [
            ((names at: index) = message selector) ifTrue: [^definitions at: index].
            index:: index + 1].
        ^super doesNotUnderstand: message
    )
) : ()
```
`core_library.rs`, after the shipped modules' `include_str!`s:
```rust
/// Not one of the shipped modules above, and never in their table: the
/// manifest a project's root receives is compiled into the program like a
/// user module, holds no mirror, and may be carried by an image.
pub const MANIFEST_SOURCE: &str = include_str!("../../core-library/Manifest.ns");
```
Re-export it from `lib.rs` next to the other public items (`pub use interpreter::core_library::MANIFEST_SOURCE;`), widening the module path to `pub` if needed.

`symbol.rs`, appended to `well_known_symbols!` (appending keeps every earlier index):
```rust
    PACKAGE_USING => "packageUsing:",
    MAIN_ARGUMENTS => "main:args:",
```

`interpreter/mod.rs`:
```rust
/// Whether an explicit send of `selector` to `receiver` finds a public
/// member (§5.5's `lookupPublic`), without sending it: the CLI asks this of
/// a manifest, whose own public members would answer a module's name before
/// its `doesNotUnderstand:` could.
pub fn responds_publicly(&mut self, receiver: &Root, selector: Symbol) -> bool {
    self.enter(|mutator| {
        let receiver = mutator.fetch(receiver);
        let class = mutator.class_of(&receiver);
        !matches!(
            crate::dispatch::lookup_public(&mutator.heap, class, selector),
            crate::dispatch::Lookup::NotFound
        )
    })
}
```
Widen `class_of`, `Lookup` and `lookup_public` to `pub(crate)` if they are private.

`crates/ns-runtime/tests/manifest.rs`:
```rust
//! The manifest a project's root receives answers each module by name, and
//! nothing else.

use indoc::indoc;
use ns_runtime::{HostResult, Interpreter, Root, SourceOrigin, Symbol, Value};

/// `HostSignal` carries no `Debug` impl, so `Result::expect` is unusable on
/// a `HostResult` -- unwrap by hand.
fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(ns_runtime::HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(ns_runtime::HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

fn load(interpreter: &mut Interpreter, source: &str, origin: SourceOrigin) -> Root {
    let module = ns_syntax::parse(source).unwrap_or_else(|_| panic!("parse"));
    let resolved = ns_runtime::resolve(module, source.into(), origin).unwrap_or_else(|_| panic!("resolve"));
    interpreter.load_module(&resolved)
}

fn manifest_over(interpreter: &mut Interpreter, name: &str, module: &Root) -> Root {
    let manifest_class = load(interpreter, ns_runtime::MANIFEST_SOURCE, SourceOrigin::Embedded("Manifest.ns".into()));
    let (names, definitions) = interpreter.enter(|mutator| {
        let symbol = mutator.new_symbol(Symbol::intern(name));
        let names = mutator.new_array(vec![symbol]);
        let definitions = mutator.new_array(vec![mutator.fetch(module)]);
        (mutator.stash(Value::Object(names)), mutator.stash(Value::Object(definitions)))
    });
    expect_ok(interpreter.instantiate(&manifest_class, &[names, definitions]), "Manifest names:definitions:")
}

#[test]
fn a_manifest_answers_a_module_definition_by_its_name() {
    let mut interpreter = Interpreter::bootstrap();
    let module = load(&mut interpreter, indoc! {"
        Newspeak3
        'Test'
        class Sieve = Object new () () : ()
    "}, SourceOrigin::File("Sieve.ns".into()));
    let manifest = manifest_over(&mut interpreter, "Sieve", &module);
    let answer = expect_ok(interpreter.send(&manifest, Symbol::intern("Sieve"), &[]), "manifest Sieve");
    interpreter.enter(|mutator| {
        match (mutator.fetch(&answer), mutator.fetch(&module)) {
            (Value::Object(left), Value::Object(right)) => assert!(left.same(right)),
            _ => panic!("the manifest answered no class"),
        }
    });
}

#[test]
fn a_manifest_does_not_understand_a_name_it_does_not_hold() {
    let mut interpreter = Interpreter::bootstrap();
    let module = load(&mut interpreter, indoc! {"
        Newspeak3
        'Test'
        class Sieve = Object new () () : ()
    "}, SourceOrigin::File("Sieve.ns".into()));
    let manifest = manifest_over(&mut interpreter, "Sieve", &module);
    match interpreter.send(&manifest, Symbol::intern("Missing"), &[]) {
        Ok(_) => panic!("the manifest answered a module it does not hold"),
        Err(ns_runtime::HostSignal::Error(error)) => {
            assert!(error.message.contains("does not understand #Missing"), "{}", error.message)
        }
        Err(ns_runtime::HostSignal::Return) => panic!("a non-local return escaped"),
    }
}

#[test]
fn an_object_member_answers_before_the_manifest_can() {
    let mut interpreter = Interpreter::bootstrap();
    let module = load(&mut interpreter, indoc! {"
        Newspeak3
        'Test'
        class Sieve = Object new () () : ()
    "}, SourceOrigin::File("Sieve.ns".into()));
    let manifest = manifest_over(&mut interpreter, "Sieve", &module);
    assert!(!interpreter.responds_publicly(&manifest, Symbol::intern("Sieve")));
    assert!(interpreter.responds_publicly(&manifest, Symbol::intern("Array")));
    assert!(interpreter.responds_publicly(&manifest, Symbol::intern("printString")));
}
```
`ns-runtime` needs `ns-syntax` and `indoc` as dev-dependencies. Check `crates/ns-runtime/Cargo.toml`; its other test files already parse sources, so they are there. Adjust the error-message assertion to the exact `doesNotUnderstand:` text the runtime produces: run the test once and read it. Other tests assert "does not understand #foo", so the fragment above is expected to match.

- [ ] **Step 2: Run them to see them pass or fail.** Run: `cargo nextest run -p ns-runtime --test manifest`. Expected: FAIL until the constant, the symbols and `responds_publicly` exist; then PASS.

- [ ] **Step 3: Write the failing CLI tests** in `crates/ns-cli/tests/run.rs`. Delete these, whose subject (`--send`, `usingPlatform:` roots) is gone:
- `a_zero_argument_factory_no_longer_rejects_instantiation`
- `a_zero_argument_factory_reaches_its_public_entry_method`
- `a_multi_parameter_factory_is_refused_with_a_clear_error`
- `run_supports_a_custom_entry_selector_via_send`
- `a_unary_entry_selector_gets_no_arguments_and_reads_its_temporary_as_nil`
- `an_entry_selector_the_cli_cannot_fill_is_refused_before_the_send`

Rewrite `run_sends_main_arguments_and_prints_via_platform_transcript` with a `class Hello = Object new () ( public main: platform args: arguments = ( … ) )` root. Add these, each writing its files into a `ScratchDirectory` (copy `common` usage from `build_load.rs`):
```rust
#[test]
fn a_source_file_whose_root_takes_the_platform_is_refused_before_anything_runs() {
    // root `class Old usingPlatform: platform = Object new () ( public main: platform args: arguments = ( platform transcript show: 'ran' ) ) : ()`
    // expect: failure, empty stdout, stderr contains
    // "cannot start `Old`: its primary factory is `usingPlatform:`" and "new"
}

#[test]
fn a_project_assembles_its_root_with_the_manifest() {
    // App.ns: class App packageUsing: manifest = Object new (| Greeter = manifest Greeter. |) (
    //     public main: platform args: arguments = ( platform transcript show: Greeter new greeting; newLine ) ) : ()
    // Greeter.ns: class Greeter = Object new () ( public greeting = ( ^'hello from a module' ) ) : ()
    // app.json: { "root": "App.ns", "modules": ["Greeter.ns"] }
    // expect: success, stdout "hello from a module\n"
}

#[test]
fn a_project_module_is_reached_through_a_backslash_path() {
    // same, with Greeter.ns under a subdirectory `lib` and "modules": ["lib\\Greeter.ns"]
}

#[test]
fn a_projects_root_must_take_the_manifest() {
    // App.ns with `class App = Object new () (...)`; expect stderr contains
    // "cannot start `App`: its primary factory is `new`" and "packageUsing:"
}

#[test]
fn two_modules_of_one_name_are_refused() {
    // modules A.ns and B.ns both declare `class Twin = …`; expect stderr names `Twin` and both files
}

#[test]
fn a_missing_module_file_is_refused() {
    // "modules": ["Nowhere.ns"]; expect stderr contains "Nowhere.ns"
}

#[test]
fn a_module_shadowed_by_an_object_member_is_refused() {
    // a module declaring `class Array = Object new () () : ()`; expect stderr
    // contains "`Array`" and "public member", and nothing runs (empty stdout)
}

#[test]
fn a_program_without_main_is_an_uncaught_does_not_understand() {
    // `class Quiet = Object new () () : ()`; expect status 1 and stderr
    // "Uncaught Newspeak error: " … "does not understand #main:args:"
}
```
Write each out fully in the file's style: `ns_next()` with `.arg("run").arg(path)`, `output()`, assert on `status.success()`, `String::from_utf8_lossy(&output.stdout)` and stderr. The Newspeak sources are `indoc!` blocks written to the scratch directory with `std::fs::write`.

- [ ] **Step 4: Run them to see them fail.** Run: `cargo nextest run -p ns-cli --test run`. Expected: the new tests FAIL (a `.json` is parsed as Newspeak; a `usingPlatform:` root runs).

- [ ] **Step 5: Implement `program.rs` and switch `run`.**

`source.rs` gains:
```rust
/// Parses and resolves a source compiled into the binary -- the manifest --
/// exactly as a file's source, under an embedded origin.
pub(crate) fn load_embedded(name: &str, source: &str) -> LoadedSource {
    let module = ns_syntax::parse(source)
        .unwrap_or_else(|_| panic!("internal: the embedded {name} does not parse"));
    let origin = ns_runtime::SourceOrigin::Embedded(name.to_string());
    let resolved = ns_runtime::resolve(module, source.into(), origin.clone())
        .unwrap_or_else(|_| panic!("internal: the embedded {name} does not resolve"));
    LoadedSource { source: source.to_string(), origin, resolved }
}
```
(A shipped source that does not parse is a build defect of this binary, not input.)

`program.rs`:
```rust
//! A program as the CLI starts it: one source file whose root is
//! instantiated with `new`, or a project file whose root is instantiated
//! with `packageUsing:` a manifest of the other modules. Either way the
//! platform arrives only as `main:args:`'s first argument
//! (`docs/design/image-format.md`, "Planned: one artifact, packaged before
//! `main:args:`").

use std::path::Path;
use std::process::ExitCode;

use crate::{project, source};

pub(crate) enum Assembly {
    New,
    Package,
}

/// Every source in module order: the root, then the project file's
/// modules as written, then the manifest. `build` writes them in this
/// order and `load` rebuilds them in it, which keeps every identity hash
/// the same across `run`, `build` and `load`.
pub(crate) struct Program {
    pub(crate) assembly: Assembly,
    pub(crate) sources: Vec<source::LoadedSource>,
}

pub(crate) struct Loaded {
    pub(crate) interpreter: ns_runtime::Interpreter,
    pub(crate) classes: Vec<ns_runtime::Root>,
}

pub(crate) fn read_program(path: &Path) -> Result<Program, ExitCode> {
    if !project::is_project_file(path) {
        return Ok(Program {
            assembly: Assembly::New,
            sources: vec![source::load_source(path)?],
        });
    }
    let project = project::read_project(path).map_err(|message| {
        eprintln!("error: {message}");
        ExitCode::FAILURE
    })?;
    let mut sources = vec![source::load_source(&project.root)?];
    for module in &project.modules {
        sources.push(source::load_source(module)?);
    }
    // Two modules of one name: the manifest could answer only one of them.
    for (index, later) in sources.iter().enumerate().skip(1) {
        let name = declared_name(later);
        if let Some(earlier) = sources[1..index].iter().find(|earlier| declared_name(earlier) == name) {
            eprintln!(
                "error: two modules declare `{name}`: {} and {}",
                describe_origin(&earlier.origin),
                describe_origin(&later.origin)
            );
            return Err(ExitCode::FAILURE);
        }
    }
    sources.push(source::load_embedded("Manifest.ns", ns_runtime::MANIFEST_SOURCE));
    Ok(Program { assembly: Assembly::Package, sources })
}

fn declared_name(loaded: &source::LoadedSource) -> &str {
    &loaded.resolved.module.root_class.name.name
}

fn describe_origin(origin: &ns_runtime::SourceOrigin) -> String {
    match origin {
        ns_runtime::SourceOrigin::File(path) => path.display().to_string(),
        ns_runtime::SourceOrigin::Embedded(name) => format!("the embedded {name}"),
    }
}

/// Compiles every module into one interpreter, in module order.
pub(crate) fn load_program(program: &Program, lowering: ns_runtime::Lowering) -> Loaded {
    let mut interpreter = ns_runtime::Interpreter::bootstrap_with(lowering);
    let classes = program
        .sources
        .iter()
        .map(|loaded| interpreter.load_module(&loaded.resolved))
        .collect();
    Loaded { interpreter, classes }
}

/// The application: the root sent `new`, or `packageUsing:` a manifest of
/// every other module but the manifest's own. A root whose factory is not
/// the one its assembly sends is refused before anything runs, naming the
/// factory it has.
pub(crate) fn instantiate_application(
    program: &Program,
    loaded: &mut Loaded,
) -> Result<ns_runtime::Root, ExitCode> {
    let root = &program.sources[0];
    let name = declared_name(root);
    let factory = root.resolved.module.root_class.factory.as_ref().map(selector_of);
    let arguments = match (&program.assembly, factory.as_deref()) {
        (Assembly::New, None | Some("new")) => Vec::new(),
        (Assembly::New, Some(other)) => {
            eprintln!(
                "error: cannot start `{name}`: its primary factory is `{other}`, and a program \
                 started from a source file is instantiated with `new` -- the platform arrives \
                 as `main:args:`'s first argument"
            );
            return Err(ExitCode::FAILURE);
        }
        (Assembly::Package, Some("packageUsing:")) => vec![build_manifest(program, loaded)?],
        (Assembly::Package, other) => {
            eprintln!(
                "error: cannot start `{name}`: its primary factory is `{}`, and a project's root \
                 is instantiated with `packageUsing:` its manifest",
                other.unwrap_or("new")
            );
            return Err(ExitCode::FAILURE);
        }
    };
    loaded
        .interpreter
        .instantiate(&loaded.classes[0], &arguments)
        .map_err(crate::entry::report_instantiation)
}

fn selector_of(pattern: &ns_syntax::ast::MessagePattern) -> String {
    match pattern {
        ns_syntax::ast::MessagePattern::Unary { selector } => selector.name.clone(),
        ns_syntax::ast::MessagePattern::Binary { selector, .. } => selector.name.clone(),
        ns_syntax::ast::MessagePattern::Keyword { parts } => {
            parts.iter().map(|(keyword, _)| format!("{}:", keyword.name)).collect()
        }
    }
}

/// The manifest over the project's modules -- every source but the root and
/// the manifest itself -- refused when a module's name is one of `Object`'s
/// public members, which would answer the send before the manifest's
/// `doesNotUnderstand:` could.
fn build_manifest(program: &Program, loaded: &mut Loaded) -> Result<ns_runtime::Root, ExitCode> {
    let last = program.sources.len() - 1;
    let modules: Vec<(ns_runtime::Symbol, ns_runtime::Root)> = (1..last)
        .map(|index| {
            (
                ns_runtime::Symbol::intern(declared_name(&program.sources[index])),
                loaded.classes[index].clone(),
            )
        })
        .collect();
    let (names, definitions) = loaded.interpreter.enter(|mutator| {
        let names = modules.iter().map(|(name, _)| mutator.new_symbol(*name)).collect();
        let definitions = modules.iter().map(|(_, class)| mutator.fetch(class)).collect();
        let names = mutator.new_array(names);
        let definitions = mutator.new_array(definitions);
        (
            mutator.stash(ns_runtime::Value::Object(names)),
            mutator.stash(ns_runtime::Value::Object(definitions)),
        )
    });
    let manifest = loaded
        .interpreter
        .instantiate(&loaded.classes[last], &[names, definitions])
        .map_err(crate::entry::report_instantiation)?;
    for (name, _) in &modules {
        if loaded.interpreter.responds_publicly(&manifest, *name) {
            eprintln!(
                "error: the module `{name}` cannot be reached through the manifest: `{name}` is a \
                 public member of every object, which answers before the manifest can"
            );
            return Err(ExitCode::FAILURE);
        }
    }
    Ok(manifest)
}

/// The modules as `image::package` takes them, in module order. Takes the
/// classes rather than `Loaded`, because the caller holds `Loaded`'s
/// interpreter mutably while it encodes.
pub(crate) fn module_sources<'program, 'gc>(
    program: &'program Program,
    classes: &[ns_runtime::Root],
    mutator: &ns_runtime::Mutator<'gc>,
    with_source: bool,
) -> Vec<ns_runtime::image::ModuleSource<'program, 'gc>> {
    program
        .sources
        .iter()
        .zip(classes)
        .map(|(source, class)| ns_runtime::image::ModuleSource {
            class: crate::entry::class_reference(mutator, class),
            origin: &source.origin,
            source: with_source.then_some(source.source.as_str()),
        })
        .collect()
}
```
The shadowing check runs after the manifest is instantiated, and instantiating it runs no program code (the manifest's own initializer only). Nothing the program wrote has run by then, so "before anything runs" holds. `load_program` never fails, because `load_module` does not.

`entry.rs` gains, and renames the private `report_signal` to `pub(crate) fn report_instantiation(signal: HostSignal) -> ExitCode` (same texts):
```rust
/// `main:args:`'s arguments (§8.4): the platform, and the program's
/// arguments -- the words after `--` -- as an `Array` of `String`s, empty
/// when there are none.
pub(crate) fn main_arguments(
    interpreter: &mut ns_runtime::Interpreter,
    program_arguments: &[String],
) -> Vec<ns_runtime::Root> {
    let platform = interpreter.platform();
    let arguments = interpreter.enter(|mutator| {
        let strings = program_arguments
            .iter()
            .map(|argument| ns_runtime::Value::Object(mutator.new_string(argument.clone())))
            .collect();
        let array = mutator.new_array(strings);
        mutator.stash(ns_runtime::Value::Object(array))
    });
    vec![platform, arguments]
}

pub(crate) fn send_main(
    interpreter: &mut ns_runtime::Interpreter,
    application: &ns_runtime::Root,
    program_arguments: &[String],
) -> ns_runtime::HostResult {
    let arguments = main_arguments(interpreter, program_arguments);
    interpreter.send(application, ns_runtime::Symbol::MAIN_ARGUMENTS, &arguments)
}
```

`commands/run.rs`:
```rust
//! `ns-next run`.

use std::path::Path;
use std::process::ExitCode;

use crate::{entry, program};

/// `ns-next run`: read the program, compile every module, assemble the
/// application, send it `main: platform args:`, report.
pub(crate) fn execute(
    file: &Path,
    lowering: ns_runtime::Lowering,
    cache_statistics: bool,
    program_arguments: &[String],
) -> ExitCode {
    let program = match program::read_program(file) {
        Ok(program) => program,
        Err(code) => return code,
    };
    let mut loaded = program::load_program(&program, lowering);
    let application = match program::instantiate_application(&program, &mut loaded) {
        Ok(application) => application,
        Err(code) => return code,
    };
    let result = entry::send_main(&mut loaded.interpreter, &application, program_arguments);
    if cache_statistics {
        entry::print_cache_statistics(&loaded.interpreter.cache_statistics());
    }
    entry::report_entry_send(result)
}
```
`main.rs`: `Run` loses `send`, and its doc says "a `.ns` source file or a `.json` project file". `execute` passes the new arguments.

- [ ] **Step 6: Run the tests.** Run: `cargo nextest run -p ns-cli --test run --test run_goldens`. Expected: PASS. Every golden passes, because Task 1 moved them. `cli.rs`'s run-corpus tests still use `run --send` through `run_corpus.rs`, so they FAIL until Task 11. Make `run_corpus::run_one` keep working in the meantime by leaving it as is: it passes `--send`, which `run` no longer accepts. So mark the six run-corpus tests in `cli.rs` `#[ignore = "run-corpus moves to wrapper applications in Task 11"]` in this commit, and un-ignore them in Task 11. Check that nothing else in the workspace passes `--send` to `run` (`grep -rn '"--send"' crates`). Every other hit belongs to `load`, `snapshot` or `bench`, which later tasks change.

- [ ] **Step 7: Run the per-commit gate, then commit.**

```bash
git add crates
git commit -m "feat: run assembles an application with new or a project's manifest"
```

---

### Task 8: Goldens for the manifest

A golden can now be a project: `goldens/<category>/<name>.json` with its `.toml`, and its modules in the directory `goldens/<category>/<name>/`. That directory is the one place a golden may nest.

**Files:**
- Modify: `crates/ns-cli/tests/common/mod.rs` (`golden_files`, `golden_sources`, `every_golden_is_a_complete_pair`'s helper if it is there)
- Modify: `crates/ns-cli/tests/run_goldens.rs` (`every_golden_is_a_complete_pair`)
- Create: `goldens/manifest/` with three goldens
- Modify: `docs/design/README.md`, "The goldens" (the table and the discovery paragraph)

**Interfaces:**
- Produces: `common::golden_programs() -> Vec<PathBuf>`, every `.ns` golden and every `.json` golden, sorted, which `run_goldens.rs` and `golden_round_trips.rs` iterate instead of `golden_sources()`.

- [ ] **Step 1: Write the goldens.**

`goldens/manifest/a_project_reaches_its_modules_through_the_manifest.json`:
```json
{
  "root": "a_project_reaches_its_modules_through_the_manifest/App.ns",
  "modules": [
    "a_project_reaches_its_modules_through_the_manifest/Greeting.ns",
    "a_project_reaches_its_modules_through_the_manifest/Counter.ns"
  ]
}
```
`…/App.ns`:
```newspeak
Newspeak3
'Test'
(* A project's root is instantiated with `packageUsing:` and a manifest of
   the project's other modules, each answered by the name its source
   declares (docs/design/image-format.md). Each module definition is a
   class (§6.4), instantiated here with whatever its own factory takes; the
   platform reaches the program only through main:args: (§8.4). *)
class App packageUsing: manifest = Object new (
    | private Greeting = manifest Greeting.
      private Counter = manifest Counter. |
) (
    public main: platform args: arguments = (
        | counter = Counter new. |
        counter bump; bump.
        platform transcript
            show: (Greeting named: 'modules') text; newLine;
            show: counter count printString; newLine
    )
) : ()
```
`…/Greeting.ns`:
```newspeak
Newspeak3
'Test'
class Greeting named: name = Object new (| public text = 'hello, ', name. |) () : ()
```
`…/Counter.ns`:
```newspeak
Newspeak3
'Test'
class Counter = Object new (| public count ::= 0. |) (
    public bump = ( count:: count + 1 )
) : ()
```
`….toml`:
```toml
stdout = '''
hello, modules
2
'''
```

`goldens/manifest/a_name_the_manifest_does_not_hold_is_not_understood.json` with root `…/App.ns` and `"modules": []`. The root has `| Missing = manifest Missing. |`, and its `main:args:` never runs. The `.toml` records the instantiation error: `status = 1` and `stderr = "Uncaught Newspeak error during instantiation: <the runtime's does-not-understand text for #Missing>\n"`. Run it once and read the exact text from `run`'s stderr. Before recording it, establish that the text is the ordinary `doesNotUnderstand:` report other goldens pin (search `does not understand #` in `goldens/**/*.toml`).

`goldens/manifest/an_application_may_keep_its_manifest.json`: root `App.ns` whose slot `| private modules = manifest. |` keeps the manifest, and whose `main:args:` sends `modules Counter new count printString` to the transcript (expected `0`). Its point is that the manifest is an ordinary object the program may keep. Task 9's round trip then carries it through an image.

- [ ] **Step 2: Run them to see them fail.** Run: `cargo nextest run -p ns-cli --test run_goldens`. Expected: FAIL. `golden_files` panics: "`…` is a directory inside the category …".

- [ ] **Step 3: Extend discovery.** In `common::golden_files`, a directory inside a category is allowed exactly when a sibling `<its name>.json` exists, and its contents are not goldens:
```rust
if path.is_dir() {
    assert!(
        path.with_extension("json").is_file(),
        "{} is a directory inside the category {} with no {}.json beside it -- only a \
         project golden's modules may live one level deeper",
        path.display(),
        category.display(),
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    continue;
}
```
Add `pub fn golden_programs() -> Vec<PathBuf>`: `golden_files("ns")` and `golden_files("json")`, merged and sorted. `run_goldens.rs` and `golden_round_trips.rs` iterate `golden_programs()`. `every_golden_is_a_complete_pair` pairs `.toml` with `.ns` *or* `.json`. `golden_name` keeps `category/stem` for both kinds.

- [ ] **Step 4: Run the tests.** Run: `cargo nextest run -p ns-cli --test run_goldens`. Expected: PASS. `golden_round_trips.rs` still uses the old `build`/`snapshot` and cannot take a `.json` yet, so in this commit it iterates `golden_sources()` (the `.ns` goldens only), and Task 9 switches it to `golden_programs()`. Also run `cargo nextest run -p ns-lsp` and `./editors/jetbrains/gradlew -p editors/jetbrains check`. Both walk `goldens/` recursively and must parse the new modules.

- [ ] **Step 5: Document.** `docs/design/README.md`, "The goldens": a golden is a `.ns` program or a `.json` project file; a project golden's modules live in the directory named like it, the one place a golden nests. Add a table row: `manifest/` — "projects: a root assembled with `packageUsing:` and the manifest of the other modules".

- [ ] **Step 6: Run the per-commit gate, then commit.**

```bash
git add goldens crates/ns-cli/tests docs/design/README.md
git commit -m "test: project goldens pin the manifest"
```

---

### Task 9: `build` packages, `load` starts, `snapshot` goes

**Files:**
- Modify: `crates/ns-cli/src/commands/build.rs`, `load.rs`, `disasm.rs` (its artifact branch), `main.rs`
- Delete: `crates/ns-cli/src/commands/snapshot.rs` (and its line in `commands/mod.rs`)
- Modify: `crates/ns-cli/tests/common/mod.rs` (delete `assert_snapshot_load_matches_run`), `golden_round_trips.rs`, `build_load.rs`, `inspect.rs`, `lowering.rs`, `disasm_snapshots.rs`
- Delete: `crates/ns-cli/tests/snapshot_load.rs` (its surviving tests move to `build_load.rs`, Step 1)

**Interfaces:**
- Consumes: Task 5's `image::package`, `LoadedImage.start`; Task 7's `program::{read_program, load_program, instantiate_application, module_sources}`, `entry::send_main`.
- Produces: `build <file.ns|project.json> <out> [--debug-source] [--lowering …]`; `load <artifact> [--print-source] [-- arguments]`.

- [ ] **Step 1: Write the failing tests.**

`golden_round_trips.rs`: delete the `snapshot_load::` trials and `a_snapshot_reproduces_what_run_prints`. `a_binary_runs_as_its_source_does` becomes `an_artifact_runs_as_its_source_does`, over `common::golden_programs()`, named `build_load::<lowering>::<category>/<name>`. Its rule: run `build <program> <out> --lowering <l>` and `run <program> --lowering <l> [-- arguments]`.
- If the build fails, its status and stderr must equal `run`'s, and both stdouts must be empty. Assembly fails the same way in both, before anything is printed, which covers the root-rule goldens, the resolve errors and instantiation errors alike.
- Otherwise `load <out> [-- arguments]` must equal `run` in success, stdout and stderr.

Delete the three special cases ("a module's root class may inherit only", "resolve error:", "--entry").

`build_load.rs`: delete these, whose subject is gone:
- `build_writes_a_stateless_binary_that_inspect_recognizes`
- `build_refuses_an_entry_the_root_class_does_not_have`
- `build_refuses_an_entry_only_a_builtin_answers`
- `build_refuses_an_entry_arity_the_cli_cannot_supply`
- `build_requires_an_entry_when_the_root_class_declares_no_main`
- `a_build_and_a_snapshot_agree_on_the_describing_sections`
- `load_send_overrides_the_recorded_entry`

Rewrite these onto `new` roots:
- `a_built_binary_runs_exactly_as_its_source_does`
- `a_built_binary_applying_mixins_runs_exactly_as_its_source_does` (its root `class Applying usingPlatform: p = Object new (| t = p transcript. |)` becomes a `new` root with `t` a local of `main:args:`)
- `loading_a_binary_runs_its_initializers_against_the_loading_platform`, renamed `a_loaded_application_does_not_rerun_its_initializers`

Add these, moved from `snapshot_load.rs` and rewritten on `build`/`load`:
```rust
#[test]
fn build_writes_a_started_image_that_inspect_recognizes() {
    // build goldens/arithmetic/<the arithmetic golden> ; inspect → "start: yes", "stateless: no", a "graph" section
}

#[test]
fn an_initializer_is_not_rerun_by_load_and_draws_the_same_hash() {
    // class Stamp = Object new (| stamp = Object new hash. |) (
    //     public main: platform args: arguments = ( platform transcript show: stamp printString; newLine ) ) : ()
    // run → S; build then load → S again (a rerun initializer would draw a hash past the image's)
}

#[test]
fn a_load_reports_an_uncaught_error_exactly_as_run_does() { /* from snapshot_load.rs:117, on build/load */ }

#[test]
fn no_corruption_of_a_version_matching_image_reaches_a_panic() { /* snapshot_load.rs:148, the image from build */ }

#[test]
fn a_built_image_is_source_free_unless_debug_source_is_asked_for() { /* snapshot_load.rs:198: build, build --debug-source, load --print-source */ }

#[test]
fn a_hash_given_to_a_tag_named_object_survives_build_and_load() {
    // class Hashes = Object new (| integer = Integer hash. module = Hashes hash. meta = Hashes class hash. |) (
    //     public main: platform args: arguments = ( platform transcript
    //         show: (integer = Integer hash) printString; show: ' ';
    //         show: (module = Hashes hash) printString; show: ' ';
    //         show: (meta = Hashes class hash) printString; newLine ) ) : ()
    // run → "true true true"; load of build → "true true true"
}

#[test]
fn a_project_builds_and_loads_as_it_runs() {
    // the project of goldens/manifest/a_project_reaches_its_modules_through_the_manifest.json: build, then load, stdout equals run's
}

#[test]
fn load_refuses_an_image_that_is_not_started() {
    // write a snapshot with ns_runtime::image::snapshot in-process (one module, no roots), then `load` it:
    // failure, stderr contains "not a started image" (the text Step 3 gives load)
}
```
The `Hashes hash` inside a slot initializer is a lexical send to the module class, which works because the root class answers its own name. If it does not, use `self class hash` and `self class class hash`. Also drop `a_load_reports…`'s dependence on `common::assert_snapshot_load_matches_run`.

`snapshot_load.rs`'s other tests go with the command:
- `a_snapshot_taken_after_the_entry_send_…`, `a_world_holding_nested_class_objects_…` and `load_without_send_prints_a_summary_…`: no post-`main:args:` snapshot any more. A world holding nested class objects is covered by every nesting golden's build/load round trip.
- `a_platform_path_through_a_method_is_refused_and_prints_nothing`: before deleting it, check that `crates/ns-runtime/tests/image.rs` covers a path through a method (search `no public slot, nested class or class accessor`). If it does not, move the test there, crafting the image with `image::snapshot` as the platform-path tests beside it do.

`inspect.rs`:
- the two `snapshot` tests become `build` tests;
- `inspect_reports_the_sections_symbols_and_mixins_of_an_image` asserts "start: yes" and the sections header, symbols, code, mixins, modules, entry, platform paths and graph;
- `inspect_full_renders_every_mixin_and_every_body` no longer finds `usingPlatform:/1`: the arithmetic golden's root has no factory now. Assert a selector the golden declares instead (`main:args:/2`).

`lowering.rs`: `snapshot_compiles_with_the_lowering_it_is_given` is deleted, since `build_compiles_with_the_lowering_it_is_given` covers the same property now that `build` writes images.

`disasm_snapshots.rs`: `disasm_of_an_image_matches_disasm_of_its_source` is deleted, since `disasm_of_a_binary_matches_disasm_of_its_source` is the same test now. `disasm_refuses_an_initializer_an_artifact_does_not_carry` calls `image::package` on an instance instead of `image::build`.

- [ ] **Step 2: Run them to see them fail.** Run: `cargo nextest run -p ns-cli --test golden_round_trips --test build_load --test inspect`. Expected: FAIL (`build` still writes a stateless binary; `load` instantiates it).

- [ ] **Step 3: Implement.**

`commands/build.rs`:
```rust
//! `ns-next build`.

use std::path::Path;
use std::process::ExitCode;

use crate::program;

/// `ns-next build`: assemble the application exactly as `run` does -- its
/// factory and every initializer run here, with no platform in existence --
/// and write it instead of sending `main:args:`. An error while assembling
/// fails the build with the report `run` gives the same error.
pub(crate) fn execute(
    file: &Path,
    out: &Path,
    debug_source: bool,
    lowering: ns_runtime::Lowering,
) -> ExitCode {
    let program = match program::read_program(file) {
        Ok(program) => program,
        Err(code) => return code,
    };
    let mut loaded = program::load_program(&program, lowering);
    let application = match program::instantiate_application(&program, &mut loaded) {
        Ok(application) => application,
        Err(code) => return code,
    };
    let classes = loaded.classes.clone();
    let bytes = loaded.interpreter.enter(|mutator| {
        let modules = program::module_sources(&program, &classes, mutator, debug_source);
        ns_runtime::image::package(mutator, &modules, mutator.fetch(&application))
    });
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: build failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    match std::fs::write(out, bytes) {
        Ok(()) => {
            println!("wrote {}", out.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: writing {}: {error}", out.display());
            ExitCode::FAILURE
        }
    }
}
```
`classes` is cloned out of `loaded` before `loaded.interpreter.enter` borrows it mutably; the `Root`s are cheap `Rc`s.

`commands/load.rs`:
```rust
/// `ns-next load`: decode an image `build` wrote and start it -- send its
/// one root `main: platform args:` with this interpreter's platform, which
/// is the first the application ever sees. An image that was not packaged
/// for a start (an `image::snapshot` a host wrote) is refused: there is
/// nothing to start. A malformed image is a message, never a panic.
pub(crate) fn execute(image: &Path, print_source: bool, program_arguments: &[String]) -> ExitCode {
    let bytes = match std::fs::read(image) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: reading {}: {error}", image.display());
            return ExitCode::FAILURE;
        }
    };
    let mut loaded = match ns_runtime::image::load(&bytes) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("error: loading {}: {error}", image.display());
            return ExitCode::FAILURE;
        }
    };
    if print_source {
        if loaded.modules.iter().any(|module| module.source.is_none()) {
            eprintln!(
                "error: {} carries no source (it was not written with --debug-source)",
                image.display()
            );
            return ExitCode::FAILURE;
        }
        for module in &loaded.modules {
            print!("{}", module.source.as_deref().unwrap_or_default());
        }
        return ExitCode::SUCCESS;
    }
    if !loaded.start || loaded.roots.len() != 1 {
        eprintln!(
            "error: {} is not a started image: `build` writes one, with the application as its \
             only root",
            image.display()
        );
        return ExitCode::FAILURE;
    }
    let application = loaded.roots[0].clone();
    crate::entry::report_entry_send(crate::entry::send_main(
        &mut loaded.interpreter,
        &application,
        program_arguments,
    ))
}
```
A stateless binary (still decodable until Task 12) has `start == false` and is refused the same way; say so in the doc comment until Task 12 retires it.

`main.rs`:
- `Build` loses `entry`;
- `Load` loses `send`;
- `Snapshot` is deleted with its dispatch arm;
- the `Build` and `Load` doc comments describe the started image ("assemble the application and write it just before `main:args:`"; "decode an image `build` wrote and send it `main:args:`").

`disasm.rs`: nothing more than Task 4 did; an artifact's module 0 is the application's.

- [ ] **Step 4: Run the tests.** Run: `cargo nextest run -p ns-cli`. Expected: PASS, the run-corpus tests still ignored.

- [ ] **Step 5: Run the per-commit gate, then commit.**

```bash
git add -A crates/ns-cli
git commit -m "feat: build packages the application just before main:args:, load starts it"
```

---

### Task 10: `bench` assembles like `run`

**Files:**
- Modify: `crates/ns-cli/src/commands/bench.rs`, `main.rs` (`Bench` loses `send`)
- Modify: `crates/ns-cli/tests/cli.rs` (the three bench tests)
- Modify: `docs/design/execution.md` (the measurement sources written out there)

**Interfaces:**
- Consumes: Task 7's `program` functions and `entry::main_arguments`.

- [ ] **Step 1: Adapt the tests.** `benchmark_reports_the_entry_sends_median` asserts the timing line now reads "timing: `main:args:` only…". Take the exact wording from Step 3. The three tests otherwise keep their inputs (`language/while_loop`, `bench/bench_block`), which are `new` roots since Task 1. Add:
```rust
#[test]
fn benchmark_takes_a_project_file() {
    // bench goldens/manifest/a_project_reaches_its_modules_through_the_manifest.json --iterations 2
    // success; stdout contains "median"
}
```

- [ ] **Step 2: Run them to see them fail.** Run: `cargo nextest run -p ns-cli --test cli benchmark`. Expected: the project test FAILS.

- [ ] **Step 3: Implement.** `benchmark_entry` takes the `Program`:
  - it loads it once with `program::load_program(program, Lowering::Optimized)`;
  - per sample, it calls `program::instantiate_application` (untimed, a fresh application every sample as today, because a program mutates its slots);
  - then it times `interpreter.send(&application, Symbol::MAIN_ARGUMENTS, &arguments)` alone, with `arguments = entry::main_arguments(...)` built once.

  `instantiate_application` prints and answers an `ExitCode` on failure, so turn that into `Err(String::from("instantiation failed"))`; its report is already on stderr. The stdout line becomes `timing: \`main:args:\` only, after {iterations} …`, keeping the rest of today's sentence. `execute` reads the program with `program::read_program`.

- [ ] **Step 4: Update the measurement sources.** In `docs/design/execution.md`, every Newspeak source written out as a measurement source (`bench_new.ns`, `bench_new2.ns`, `bench_newarg.ns`, `bench_do.ns` in "What spec-conformant lookup moved"; search the file for `usingPlatform:`) gets the same migration Task 1 applied to the goldens, by hand. Say once, in "What removing the re-entries moved", that measurements before this change timed the same entry send, so figures stay comparable.

- [ ] **Step 5: Run the tests, then the per-commit gate, then commit.**

```bash
git add crates/ns-cli docs/design/execution.md
git commit -m "feat: bench times main:args: of an application assembled like run's"
```

---

### Task 11: `run-corpus` starts each corpus module through a wrapper application

**Files:**
- Modify: `crates/ns-cli/src/commands/run_corpus.rs`
- Modify: `corpus/runnable.toml` (header comment only)
- Modify: `crates/ns-cli/tests/cli.rs` (un-ignore the six run-corpus tests; adapt the `TALKATIVE` fixture)

**Interfaces:**
- Produces: `fn write_wrapper(scratch: &Path, module: &Path, send: &str) -> Result<PathBuf, String>`, which answers the project file's path.

- [ ] **Step 1: Adapt the tests.** Un-ignore the six. `TALKATIVE` (cli.rs:138) stays a `usingPlatform:` module on purpose: corpus modules are upstream's and take the platform in their factory, and the wrapper must instantiate one of those. Its row's `send = "main:args:"` becomes `send = "talk"`, a public method the fixture gains, which writes to the transcript. The fixture's module keeps the platform it was given at instantiation. Add:
```rust
#[test]
fn run_corpus_instantiates_a_new_module_with_new() {
    // a whitelisted module `class Quiet = Object new () ( public answer = ( ^42 ) ) : ()`, send = "answer",
    // expected "ran 1, failed 0, skipped 0"
}

#[test]
fn run_corpus_refuses_a_module_whose_factory_it_cannot_fill() {
    // `class Picky usingPlatform: p ide: i = …`, expect "FAIL Picky.ns:" and "usingPlatform:ide:"
}
```

- [ ] **Step 2: Run them to see them fail.** Run: `cargo nextest run -p ns-cli --test cli run_corpus`. Expected: FAIL (`run --send` is refused).

- [ ] **Step 3: Implement.** For each row, `check_row` creates one `ScratchDirectory` and calls `write_wrapper(scratch, &module_path, &entry.send)`. It writes two files:

`RunCorpusEntry.ns`, where `<Module>` is the corpus module's declared class name and `<instance>` is `Module new` for a factory that is absent or `new`, and `Module usingPlatform: platform` for `usingPlatform:`. Read both from `ns_syntax::parse` of the module's source (`module.root_class.name.name`, `module.root_class.factory`, with the `selector_of` helper of `program.rs` made `pub(crate)`). Any other factory is an `Err` naming it ("the corpus module's primary factory `usingPlatform:ide:` is neither `new` nor `usingPlatform:`"):
```newspeak
Newspeak3
'RunCorpus'
class RunCorpusEntry packageUsing: manifest = Object new (
    | private Module = manifest <Module>. |
) (
    public main: platform args: arguments = (
        (<instance>) <send>
    )
) : ()
```
and `project.json`, `{ "root": "RunCorpusEntry.ns", "modules": ["<absolute module path>"] }`, written with `serde_json::json!` so the path is escaped correctly.

`<send>` is the row's selector. Every row's is unary today. A keyword selector is refused with an `Err` ("run-corpus sends a unary selector"), since the wrapper would have to invent arguments.

The four lanes then run `run <project.json> --lowering <l>`, and `build <project.json> <scratch>/app.nsi --lowering <l>` followed by `load <scratch>/app.nsi`. The expected stdout and status are unchanged, because `main:args:` answers what the send answered and the wrapper prints nothing. One wrapper serves all four lanes.

Rewrite `corpus/runnable.toml`'s header comment:
- `send` is "the unary selector the generated wrapper application sends to the module's instance";
- the four lanes are "`run` of the wrapper's project file and `build` then `load` of it, under each lowering";
- the paragraph about `[platform, nil]` goes.

- [ ] **Step 4: Run the tests.** Run: `cargo nextest run -p ns-cli --test cli` and `cargo run -p ns-cli -- run-corpus corpus/modules`. Expected: PASS, and "ran 6, failed 0, skipped 205".

- [ ] **Step 5: Run the per-commit gate, then commit.**

```bash
git add crates/ns-cli corpus/runnable.toml
git commit -m "feat: run-corpus starts each corpus module through a generated application"
```

---

### Task 12: The stateless binary retires

Nothing writes one any more (`build` packages since Task 9). Its flag bit and its entry field go, and bit 2 is refused as an unknown flag from now on.

**Files:**
- Modify: `crates/ns-runtime/src/image/mod.rs` (delete `build`; `load` loses the stateless branches and the entry), `encode.rs` (`FLAG_STATELESS`, `encode_entry`), `decode.rs` (`Header.stateless`, `decode_entry`, the contradictory-flags error), `inspect.rs` (`stateless`, the "entry" section, `Report.entry`), `opcodes.rs` or `encode.rs` (a `RETIRED_HEADER_FLAGS` constant)
- Modify: `crates/ns-cli/src/entry.rs` (delete `factory_arguments_for`, `selector_arity`, `entry_arguments`, `instantiate_root`, `class_name` if now unused), `commands/load.rs` (doc comment)
- Test: `crates/ns-runtime/tests/image.rs`, `opcode_tags.rs`, `trampoline.rs`; `crates/ns-cli/tests/inspect.rs`

**Interfaces:**
- Produces: `encode::RETIRED_HEADER_FLAGS: u8 = 2`; `KNOWN_FLAGS = FLAG_HAS_SOURCE | FLAG_START`; `LoadedImage { interpreter, modules, roots, start }`.

- [ ] **Step 1: Adapt the tests.**
  - `opcode_tags.rs` `header_flags_are_fixed` asserts `FLAG_HAS_SOURCE == 1`, `FLAG_START == 4`, `RETIRED_HEADER_FLAGS == 2`, and `KNOWN_FLAGS & RETIRED_HEADER_FLAGS == 0`.
  - In `image.rs`, delete the tests of the stateless artifact and its entry:
    - `build_accepts_an_entry_selector_the_module_never_declares`
    - `a_binary_with_a_graph_appended_is_refused`
    - `a_stateless_artifact_without_an_entry_is_refused`
    - `a_built_binary_and_a_snapshot_of_the_same_source_agree_up_to_the_root_class`
    - `a_binary_loads_with_no_roots_and_its_entry`
    - `a_relabelled_snapshot_is_refused_for_missing_entry`
    - `every_truncation_of_a_binary_is_reported_rather_than_panicking`, whose packaged input from Task 5 moves into the snapshot truncation test
    - every other test the compiler names for `image::build`, `stateless`, `entry` or `FLAG_STATELESS`
  - Rewrite these onto `image::package` or `image::snapshot`:
    - `the_encoder_refuses_a_root_the_rule_forbids`
    - `a_root_superclass_naming_metaclass_is_refused`
    - `a_lazy_slot_without_its_getter_is_refused`
    - `a_binary_of_a_module_applying_mixins_builds_its_levels_after_loading`
    - `a_root_class_applying_a_builtin_mixin_round_trips`

    Each keeps its property.
  - Add `a_retired_stateless_flag_is_refused_as_unknown`: a header with flags `2` is refused with `UnknownHeaderFlags { flags: 2 }`'s text.
  - `trampoline.rs`: its `image::build` call (430) becomes `image::package` of an instance (or `snapshot`), and `loaded.root_class` reads `modules[0].class`.
  - `crates/ns-cli/tests/inspect.rs`: no "stateless" and no "entry" line.

- [ ] **Step 2: Run them to see them fail.** Run: `cargo nextest run -p ns-runtime --test image --test opcode_tags`. Expected: FAIL (flag 2 still known).

- [ ] **Step 3: Implement.** Delete what the File list names. `read_header` refuses any flag outside `KNOWN_FLAGS`, so bit 2 now meets `UnknownHeaderFlags` like any unknown bit. `ContradictoryHeaderFlags` goes with it. Beside `KNOWN_FLAGS`:
```rust
/// Header flag bits that no longer mean anything and are never reused: 2
/// was `FLAG_STATELESS`, the stateless binary, retired 2026-09-29 when
/// `build` began packaging the application before `main:args:`. A stale
/// binary's header meets an unknown flag rather than whatever took the bit.
pub const RETIRED_HEADER_FLAGS: u8 = 2;
```
The layout comment at the top of `image/mod.rs` loses "entry field".

- [ ] **Step 4: Run the tests.** Run: `cargo nextest run --workspace --exclude ns-lsp`. Expected: PASS. Regenerate the wire fixture as in Task 4 Step 7 (the entry field is gone).

- [ ] **Step 5: Run the per-commit gate, then commit.**

```bash
git add -A crates
git commit -m "refactor: retire the stateless binary, its header flag and its entry field"
```

---

### Task 13: The documentation describes the built state

Markdown only: no gates (`CLAUDE.md`). Every statement must describe the tree as it now is (`docs-describe-current-state`); nothing is labelled historical.

**Files:**
- `docs/design/image-format.md`:
  - the status block: CLI `ns-next build`, `load`, `inspect`, `disasm`; no `snapshot`.
  - "Two artifacts" becomes one section on the artifact: one format, written by `build` just before `main:args:` with `FLAG_START`, and the later image with heap state as the same artifact written mid-execution.
  - "Layout" and "Sections" give the new section order (header, symbols, code, mixins, modules, platform paths, graph, tag-named hashes).
  - "The stateless binary" becomes "Packaging", covering `build` and `load` as Tasks 9 and 12 built them: the consistency guarantee between `build` and `snapshot` goes, "Choosing the entry" goes, "`args:` is the program's arguments" stays, rewritten for `load`.
  - The "Planned: one artifact, packaged before `main:args:`" section's content moves into the present tense in those sections, and the heading goes.
  - "Decisions": "One format, two artifacts" and "The binary records its entry point" and "`main:args:` is the default, not the rule" and "`build` is a new verb; `load` is not" are replaced by the decisions this batch made, with their reasons: packaging before `main:args:` as upstream does; the manifest over `doesNotUnderstand:`; JSON (the user's choice); module order; retired tags and flag.
  - "Open questions": the answered question goes.
  - The class-tag and hash-tag descriptions name `CLASS_MODULE`, `CLASS_MODULE_LEVEL`, `HASHED_MODULE_CLASS`, `HASHED_MODULE_LEVEL`.
- `docs/design/README.md`:
  - the map row "Serialization": status "Built: one artifact, an application packaged just before `main:args:`, carrying every module of a project"; next "the image under the new representation; suspend and resume".
  - "From source to a running program" ends in "assemble (`new`, or `packageUsing:` a manifest) → `main: platform args:`", and says `build` stops just before the send.
  - "The `ns-next` binary" lists `parse`, `parse-corpus`, `run-corpus`, `run`, `load`, `bench`, `disasm`, `inspect`, `build`, and names `project.rs` and `program.rs`.
  - "The goldens": the round trip is build/load only, and the sentence from Task 1 points at image-format.md's built section.
- `docs/design/core-library.md`: a short section, "The manifest", saying what `Manifest.ns` is and why it is not a shipped module (no mirror, carried by images, compiled as user code). Name it in the status block's code list.
- `docs/design/compiler.md` (842-867): `snapshot` gone from the `--lowering` list; `lowering.rs` reads artifacts from `build`.
- `docs/design/execution.md` (825, 882): the byte-identity remarks name `build` artifacts only.
- `docs/design/dispatch.md:1323`: unchanged unless it names `--send`.
- `README.md` (repo root, 90-135): `run` of a file or a project file with an example `app.json`; `--send` and `snapshot` gone; `build` and `load` as they now are.
- `CLAUDE.md`:
  - the command list;
  - the untrusted-input paragraph: "`ns-next load` runs a stateless binary's code" becomes "`ns-next load` runs the packaged application's code";
  - the depth-limit clause stays.
- `docs/OPEN-WORK.md`:
  - remove the "Next" item (built);
  - "Next" names the Are We Fast Yet stage 1 plan, still to be written;
  - in the AWFY item, the harness line now reads that the harness *can* be a `packageUsing:` application.
- `docs/PROGRESS.md`:
  - a milestone entry for this batch (what was built, the commit range, the review outcome);
  - "Where the next session starts" names AWFY stage 1.
- `.claude/skills/measuring-performance/SKILL.md`: unaffected unless it names `--send`; check.

- [ ] **Step 1: Make the edits above.**
- [ ] **Step 2: Check for leftovers.** `grep -rn -- "--send\|--entry\|stateless\|snapshot\b\|FLAG_STATELESS\|CLASS_USER_ROOT\|root-class section" docs/design README.md CLAUDE.md corpus/runnable.toml .claude/skills`. Every hit describes the current tree or is gone. Plans under `docs/superpowers/plans/` are execution records and stay as written.
- [ ] **Step 3: Commit.**

```bash
git add -A docs README.md CLAUDE.md .claude
git commit -m "docs: one artifact packaged before main:args:, and projects with a manifest"
```

---

### Task 14: The end-of-batch gates

- [ ] **Step 1:** Invoke the `finishing-a-batch` skill and run everything it lists:
  - the three stress modes;
  - `cargo nextest run -p ns-lsp`;
  - `run-corpus` plain and under each stress mode ("ran 6, failed 0, skipped 205");
  - the release-only size tests (`cargo nextest run --workspace --release`).

  The JetBrains plugin's `verifyPlugin buildPlugin` runs only if the batch touched `editors/jetbrains`. It did not, but its lexer tests read the goldens, and Task 1 and Task 8 ran `gradlew check` for that.
- [ ] **Step 2:** Check with `docs-must-support-a-fresh-session`: a fresh session that reads `docs/design/README.md`, `docs/PROGRESS.md` and `docs/OPEN-WORK.md` must learn that projects exist, how to write one, and that AWFY stage 1 is next.
- [ ] **Step 3:** Fix whatever the gates find, each fix its own commit with the per-commit gate.

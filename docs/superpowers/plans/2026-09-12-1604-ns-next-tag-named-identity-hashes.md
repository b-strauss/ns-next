# Tag-Named Identity Hashes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A hash the program was given for a builtin class, the module's root
class or the `platform` singleton survives a snapshot, the way every other
object's hash already does.

**Architecture:** The graph section gains a closing table, written after its
roots: one entry per tag-named object whose stored hash is not 0, each a new
wire tag naming the object and its `u32` hash. `image::snapshot` writes it,
`image::load` restores it and advances the identity-hash counter past the
largest hash of either table, and `inspect`'s structural walk reads it as part
of the graph section.

**Tech Stack:** Rust 1.98.0, edition 2021, no external dependencies. Only
`ns-runtime` and `ns-cli` change.

**Spec:** [docs/design/image-format.md](../../design/image-format.md),
"Identity hashes are preserved verbatim" and "Planned: the image records
tag-named hashes". Read both before Task 1.

## Global Constraints

- A wire tag is assigned once and never renumbered or reused
  (`crates/ns-runtime/src/image/opcodes.rs`); `crates/ns-runtime/tests/opcode_tags.rs`
  pins every value. The new family starts at 0.
- An image is untrusted input: no byte sequence may panic or abort. Every count
  goes through `decode::bounded_count` before it drives a loop or an allocation.
- `FORMAT_VERSION` is not bumped; an image written before this plan is refused
  by the decoder (it ends before the new table), which is acceptable — images
  are disposable.
- One world encodes to one byte sequence: entries are written in a fixed order
  (builtin classes in wire-tag order, then the root class, then `platform`),
  and only hashes that are not 0.
- Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an `EvalResult` —
  tests use the local `expect_ok` helper.
- Gates before every commit: `cargo fmt --all`, `cargo clippy --workspace
  --all-targets` warning-free, `cargo test --workspace`,
  `cargo run -p ns-cli -- parse-corpus corpus/modules` reports
  `parsed 210, failed 0, expected-fail 1, stale 0`,
  `cargo run -p ns-cli -- run-corpus corpus/modules` reports
  `ran 6, failed 0, skipped 205`.
- Conventional commits, each ending with the two trailer lines:
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01UjEeSba1SZqspUW2tnKvw3
  ```

---

### Task 1: The table — wire tags, encoder, decoder, `snapshot`, `load`, `inspect`

**Files:**
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (new tag family after `PC_KIND_*`)
- Modify: `crates/ns-runtime/src/types.rs` (`BuiltinClass::ALL`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs:804-819` (`build_builtin_classes` uses `BuiltinClass::ALL`)
- Modify: `crates/ns-runtime/src/image/encode.rs` (new `encode_tag_named_hashes`)
- Modify: `crates/ns-runtime/src/image/decode.rs` (two `DecodeError` variants, new `decode_tag_named_hashes`)
- Modify: `crates/ns-runtime/src/image/mod.rs` (`snapshot` takes `&Interpreter`, writes the table; `load` reads it)
- Modify: `crates/ns-runtime/src/image/inspect.rs` (`walk_graph` reads the table; `Report` carries it)
- Modify: `crates/ns-cli/src/commands/snapshot.rs:43-50` (pass `&interp`)
- Modify: `crates/ns-runtime/tests/image.rs` (every `image::snapshot(interp.heap_ref(), …` call; new tests)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated)

**Interfaces:**
- Produces:
  - `pub const HASHED_BUILTIN_CLASS: u8 = 0; pub const HASHED_ROOT_CLASS: u8 = 1; pub const HASHED_PLATFORM: u8 = 2;` in `image::opcodes`
  - `impl BuiltinClass { pub const ALL: [BuiltinClass; 14] }` — declaration order, which is also wire-tag order
  - `pub fn encode::encode_tag_named_hashes(w: &mut Writer, interp: &Interpreter, root_class: ClassRef, platform: &Value)`
  - `pub fn decode::decode_tag_named_hashes(r: &mut Reader, interp: &mut Interpreter, root_class: ClassRef) -> Result<u32, DecodeError>` — answers the largest hash it restored, 0 if none
  - `DecodeError::UnassignedTagNamedHash`, `DecodeError::DuplicateTagNamedHash`
  - `pub fn image::snapshot(interp: &Interpreter, source: Option<&str>, origin: &SourceOrigin, root_class: ClassRef, platform: &Value, roots: &[Value]) -> Result<Vec<u8>, String>` — only the first parameter's type changes
  - `inspect::Report::graph_tag_named_hashes: Vec<String>`

- [ ] **Step 1: Write the failing round-trip test**

Append to `crates/ns-runtime/tests/image.rs`, in the "Whole-image
`snapshot`/`load`" part, after
`an_unhashed_object_round_trips_unassigned_and_draws_past_the_image_maximum`.
It is written against the *new* `snapshot` signature, so it fails to compile
until Step 3 — that is the failure this step expects.

```rust
/// A builtin class, the root class and `platform` are named by tag, not
/// carried as records, so before the closing hash table nothing recorded a
/// hash one of them had been given: after the load the bootstrap's fresh
/// objects drew new ones (`docs/design/image-format.md`, "Identity hashes are
/// preserved verbatim"). One that nothing asked stays unassigned.
#[test]
fn the_hashes_of_objects_named_by_tag_survive_a_snapshot() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()";
    let (mut interp, class) = load(src);
    let platform = interp.platform();
    let integer = interp.builtin_class(BuiltinClass::Integer);
    let integer_hash = interp.heap_mut().ensure_identity_hash(integer);
    let class_hash = interp.heap_mut().ensure_identity_hash(class);
    let platform_hash = interp.heap_mut().ensure_identity_hash(*obj(&platform));
    let bytes = image::snapshot(
        &interp,
        None,
        &Origin::File(PathBuf::from("t.ns")),
        class,
        &platform,
        &[],
    )
    .expect("snapshot");

    let mut loaded = image::load(&bytes).expect("load");
    let loaded_platform = loaded.interp.platform();
    let loaded_integer = loaded.interp.builtin_class(BuiltinClass::Integer);
    let loaded_string = loaded.interp.builtin_class(BuiltinClass::Str);
    let heap = loaded.interp.heap_mut();
    assert_eq!(heap.identity_hash(loaded_integer), integer_hash);
    assert_eq!(heap.identity_hash(loaded.root_class), class_hash);
    assert_eq!(heap.identity_hash(*obj(&loaded_platform)), platform_hash);
    assert_eq!(
        heap.identity_hash(loaded_string),
        0,
        "a builtin nothing asked for its hash stays unassigned"
    );
    let largest = integer_hash.max(class_hash).max(platform_hash);
    assert!(
        heap.ensure_identity_hash(loaded_string) > largest,
        "the counter is advanced past the tag-named hashes too"
    );
}
```

- [ ] **Step 2: Write the failing decoder tests**

Append to the same file, directly after the test above. Each builds only the
table's bytes and hands them to `decode_tag_named_hashes`, which does not exist
yet.

```rust
/// The closing table alone, decoded against a freshly loaded world.
fn decode_tag_named(bytes: &[u8]) -> Result<u32, decode::DecodeError> {
    let (mut interp, class) = load(EMPTY);
    let mut r = decode::Reader::new(bytes);
    decode::decode_tag_named_hashes(&mut r, &mut interp, class)
}

#[test]
fn a_tag_named_hash_of_zero_is_refused() {
    // 0 means unassigned, which the encoder expresses by leaving the entry
    // out; accepting it would give one world two encodings.
    let mut w = encode::Writer::new();
    w.write_u32(1);
    w.write_u8(image::opcodes::HASHED_ROOT_CLASS);
    w.write_u32(0);
    assert_eq!(
        decode_tag_named(&w.into_bytes()),
        Err(decode::DecodeError::UnassignedTagNamedHash)
    );
}

#[test]
fn a_tag_named_object_recorded_twice_is_refused() {
    let mut w = encode::Writer::new();
    w.write_u32(2);
    w.write_u8(image::opcodes::HASHED_PLATFORM);
    w.write_u32(3);
    w.write_u8(image::opcodes::HASHED_PLATFORM);
    w.write_u32(4);
    assert_eq!(
        decode_tag_named(&w.into_bytes()),
        Err(decode::DecodeError::DuplicateTagNamedHash)
    );
}

#[test]
fn a_tag_named_hash_at_the_ceiling_is_refused() {
    let mut w = encode::Writer::new();
    w.write_u32(1);
    w.write_u8(image::opcodes::HASHED_BUILTIN_CLASS);
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Integer));
    w.write_u32(u32::MAX);
    assert_eq!(
        decode_tag_named(&w.into_bytes()),
        Err(decode::DecodeError::IdentityHashExhausted)
    );
}

#[test]
fn an_unknown_tag_named_hash_tag_is_refused() {
    let mut w = encode::Writer::new();
    w.write_u32(1);
    w.write_u8(3);
    w.write_u32(5);
    assert_eq!(
        decode_tag_named(&w.into_bytes()),
        Err(decode::DecodeError::InvalidTag {
            what: "tag-named hash",
            tag: 3
        })
    );
}

#[test]
fn a_tag_named_hash_count_past_the_remaining_bytes_is_refused() {
    let mut w = encode::Writer::new();
    w.write_u32(u32::MAX);
    assert_eq!(
        decode_tag_named(&w.into_bytes()),
        Err(decode::DecodeError::UnexpectedEof)
    );
}

#[test]
fn the_tag_named_hash_table_answers_its_largest_hash() {
    let mut w = encode::Writer::new();
    w.write_u32(2);
    w.write_u8(image::opcodes::HASHED_BUILTIN_CLASS);
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Str));
    w.write_u32(9);
    w.write_u8(image::opcodes::HASHED_ROOT_CLASS);
    w.write_u32(4);
    assert_eq!(decode_tag_named(&w.into_bytes()), Ok(9));
}
```

Also add to `crates/ns-runtime/tests/opcode_tags.rs`, inside
`graph_section_tags_are_fixed` after the `PC_KIND_*` lines:

```rust
    assert_eq!(HASHED_BUILTIN_CLASS, 0);
    assert_eq!(HASHED_ROOT_CLASS, 1);
    assert_eq!(HASHED_PLATFORM, 2);
```

and in `no_two_graph_tags_in_a_family_share_a_number` change the array type to
`[&[u8]; 6]` and add the family as its last element:

```rust
        &[HASHED_BUILTIN_CLASS, HASHED_ROOT_CLASS, HASHED_PLATFORM],
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p ns-runtime --test image --test opcode_tags`
Expected: compile errors — `HASHED_BUILTIN_CLASS` not found in
`image::opcodes`, `decode_tag_named_hashes` not found in `decode`,
`UnassignedTagNamedHash` not a variant, and `image::snapshot` expecting
`&Heap` where `&Interpreter` is passed.

- [ ] **Step 4: Add the tag family and `BuiltinClass::ALL`**

In `crates/ns-runtime/src/image/opcodes.rs`, append after the `PC_KIND_*`
constants:

```rust
/// Which tag-named object an entry of the graph section's closing hash table
/// names. The three are exactly the objects `CLS_BUILTIN`, `CLS_USER_ROOT`
/// and `VAL_PLATFORM` name: none of them is a record, so without this table
/// nothing in an image would carry a hash one of them was given
/// (`docs/design/image-format.md`, "Identity hashes are preserved verbatim").
/// A builtin-class entry is followed by its builtin class tag.
pub const HASHED_BUILTIN_CLASS: u8 = 0;
pub const HASHED_ROOT_CLASS: u8 = 1;
pub const HASHED_PLATFORM: u8 = 2;
```

In `crates/ns-runtime/src/types.rs`, after the `BuiltinClass` enum:

```rust
impl BuiltinClass {
    /// Every builtin class, in declaration order. `Interpreter::builtin_class`
    /// indexes by discriminant, so the bootstrap builds them in this order,
    /// and the image's closing hash table writes them in it too -- it is also
    /// their wire-tag order (`image::encode::builtin_class_tag`).
    pub const ALL: [BuiltinClass; 14] = [
        BuiltinClass::Object,
        BuiltinClass::Boolean,
        BuiltinClass::True,
        BuiltinClass::False,
        BuiltinClass::Integer,
        BuiltinClass::Float,
        BuiltinClass::Str,
        BuiltinClass::Sym,
        BuiltinClass::Character,
        BuiltinClass::Array,
        BuiltinClass::OrderedCollection,
        BuiltinClass::Nil,
        BuiltinClass::Block,
        BuiltinClass::Platform,
    ];
}
```

In `crates/ns-runtime/src/interpreter/mod.rs`, `build_builtin_classes`: delete
the local `let kinds = [ … ];` array (lines 804-819) and change
`classes = Vec::with_capacity(kinds.len())` and `for kind in kinds` to use
`BuiltinClass::ALL`. Keep the `NOTE` comment above it — it explains why every
variant is written qualified, which `ALL` still does. The
`debug_assert_eq!(classes.len(), kind as usize)` stays; it is what ties `ALL`'s
order to the discriminants.

- [ ] **Step 5: The encoder**

In `crates/ns-runtime/src/image/encode.rs`, append after
`encode_graph_section`:

```rust
/// The graph section's closing table: the identity hash of every object the
/// image names by tag rather than carries as a record -- each builtin class,
/// the module's root class, the `platform` singleton -- so that a hash the
/// program was given survives the load. Only an assigned hash (not 0) is
/// written, in a fixed order -- the builtin classes in wire-tag order, then
/// the root class, then `platform` -- so one world encodes to one byte
/// sequence. See `docs/design/image-format.md`, "Identity hashes are
/// preserved verbatim".
///
/// Every builtin is considered, not only those the graph reaches: a program
/// can keep `Integer hash` in a slot without keeping `Integer`.
pub fn encode_tag_named_hashes(
    w: &mut Writer,
    interp: &Interpreter,
    root_class: ClassRef,
    platform: &Value,
) {
    let heap = interp.heap_ref();
    let mut entries: Vec<(u8, Option<u8>, u32)> = Vec::new();
    for kind in BuiltinClass::ALL {
        let hash = heap.identity_hash(interp.builtin_class(kind));
        if hash != 0 {
            entries.push((HASHED_BUILTIN_CLASS, Some(builtin_class_tag(kind)), hash));
        }
    }
    let hash = heap.identity_hash(root_class);
    if hash != 0 {
        entries.push((HASHED_ROOT_CLASS, None, hash));
    }
    if let Value::Obj(platform) = platform {
        let hash = heap.identity_hash(*platform);
        if hash != 0 {
            entries.push((HASHED_PLATFORM, None, hash));
        }
    }
    w.write_u32(u32_field("tag-named hash count", entries.len()));
    for (tag, builtin, hash) in entries {
        w.write_u8(tag);
        if let Some(builtin) = builtin {
            w.write_u8(builtin);
        }
        w.write_u32(hash);
    }
}
```

Add `Interpreter` to the file's imports if it is not already there
(`use crate::Interpreter;`), and make sure the `HASHED_*` constants are in scope
(the file already imports the other graph tags from `super::opcodes`; extend
that `use`).

- [ ] **Step 6: The decoder**

In `crates/ns-runtime/src/image/decode.rs`:

Add two variants to `DecodeError`, after `IdentityHashExhausted`:

```rust
    /// A tag-named hash of 0. 0 means "not yet assigned", which the encoder
    /// expresses by leaving the object out; accepting a 0 would give one world
    /// two encodings.
    UnassignedTagNamedHash,
    /// One tag-named object listed twice in the closing hash table.
    DuplicateTagNamedHash,
```

and their `Display` arms, after `IdentityHashExhausted`'s:

```rust
            DecodeError::UnassignedTagNamedHash => write!(
                f,
                "an image records identity hash 0 for an object it names by tag; an unassigned hash is recorded by leaving the object out"
            ),
            DecodeError::DuplicateTagNamedHash => write!(
                f,
                "an image records the identity hash of one tag-named object twice"
            ),
```

Add, after `MIN_FRAME_RECORD_BYTES`:

```rust
/// The smallest entry of the closing hash table: a tag (1) and a hash (4). A
/// builtin-class entry carries one more byte.
const MIN_TAG_NAMED_HASH_BYTES: usize = 5;
```

and, after `decode_graph_section`:

```rust
/// The inverse of `encode::encode_tag_named_hashes`: puts each recorded hash
/// back onto the object the bootstrap -- or `build_root_class` -- built, and
/// answers the largest one, which `load` advances the counter past together
/// with the graph's own maximum. A 0, a repeated object and `u32::MAX` are
/// refused, each for the reason its `DecodeError` gives.
pub fn decode_tag_named_hashes(
    r: &mut Reader,
    interp: &mut Interpreter,
    root_class: ClassRef,
) -> Result<u32, DecodeError> {
    let count = bounded_count(r.read_u32()? as usize, MIN_TAG_NAMED_HASH_BYTES, r)?;
    let mut restored: Vec<ObjRef> = Vec::new();
    let mut max = 0;
    for _ in 0..count {
        let target = match r.read_u8()? {
            HASHED_BUILTIN_CLASS => interp.builtin_class(read_builtin_class(r)?),
            HASHED_ROOT_CLASS => root_class,
            HASHED_PLATFORM => match interp.platform() {
                Value::Obj(platform) => platform,
                _ => unreachable!("the platform singleton is always an object"),
            },
            tag => {
                return Err(DecodeError::InvalidTag {
                    what: "tag-named hash",
                    tag,
                })
            }
        };
        let hash = r.read_u32()?;
        if hash == 0 {
            return Err(DecodeError::UnassignedTagNamedHash);
        }
        if hash == u32::MAX {
            return Err(DecodeError::IdentityHashExhausted);
        }
        if restored.iter().any(|seen| seen.same(target)) {
            return Err(DecodeError::DuplicateTagNamedHash);
        }
        restored.push(target);
        interp.heap.set_identity_hash(target, hash);
        max = max.max(hash);
    }
    Ok(max)
}
```

`restored` grows only as entries are read, and a 17th distinct entry cannot
exist (14 builtins, the root class, `platform`), so it never exceeds 16 before a
duplicate is refused. Extend the `use super::opcodes::…` import with the three
`HASHED_*` constants.

- [ ] **Step 7: `snapshot` and `load`**

In `crates/ns-runtime/src/image/mod.rs`:

Change `snapshot`'s first parameter to `interp: &Interpreter`, and bind
`let heap = interp.heap_ref();` as its first statement so the body below it is
unchanged. After the `encode::encode_graph_section(…)?;` call, append:

```rust
    encode::encode_tag_named_hashes(&mut graph_w, interp, root_class, platform);
```

and extend the doc comment's first paragraph with: "The graph section closes
with the hashes of the objects it names by tag
(`encode::encode_tag_named_hashes`)."

In `load`, replace

```rust
    if !header.stateless {
        decode::verify_graph(&graph, &table, &interp.heap).map_err(|e| e.to_string())?;
        interp.advance_identity_hash_past(graph.max_identity_hash);
    }
```

with

```rust
    if !header.stateless {
        let tag_named_max = decode::decode_tag_named_hashes(&mut r, &mut interp, root_class)
            .map_err(|e| e.to_string())?;
        decode::verify_graph(&graph, &table, &interp.heap).map_err(|e| e.to_string())?;
        interp.advance_identity_hash_past(graph.max_identity_hash.max(tag_named_max));
    }
```

and update the module doc comment's layout line to
"… object table, frame table, roots, tag-named hashes."

In `crates/ns-cli/src/commands/snapshot.rs`, change the first argument of the
`ns_runtime::image::snapshot(` call from `interp.heap_ref()` to `&interp`.

In `crates/ns-runtime/tests/image.rs`, change the first argument of every
`image::snapshot(` call from `interp.heap_ref()` (or `x.interp.heap_ref()`) to
`&interp` (or `&x.interp`). `grep -n "image::snapshot(" crates/ns-runtime/tests/image.rs`
lists them; the argument is on the same line or the next.

- [ ] **Step 8: `inspect`**

In `crates/ns-runtime/src/image/inspect.rs`:

Add `tag_named: Vec<String>` to `GraphView`, and at the end of `walk_graph`,
after `let roots = read_values(r)?;`:

```rust
    let count = decode::bounded_count(r.read_u32()? as usize, 1, r)?;
    let mut tag_named = Vec::new();
    for _ in 0..count {
        let who = match r.read_u8()? {
            opcodes::HASHED_BUILTIN_CLASS => format!("{:?}", decode::read_builtin_class(r)?),
            opcodes::HASHED_ROOT_CLASS => "<root class>".to_string(),
            opcodes::HASHED_PLATFORM => "<platform>".to_string(),
            tag => {
                return Err(decode::DecodeError::InvalidTag {
                    what: "tag-named hash",
                    tag,
                })
            }
        };
        tag_named.push(format!("{who} hash {}", r.read_u32()?));
    }
```

and return it in the `GraphView`. Initialise it as `Vec::new()` where
`inspect` builds the empty `GraphView` for a binary. Add
`pub graph_tag_named_hashes: Vec<String>` to `Report` (documented as "The
identity hashes the graph section records for the objects it names by tag.
Rendered only by `render_full`."), fill it from `graph.tag_named`, and extend
the graph section's `detail` to
`"{} objects, {} frames, {} roots, {} tag-named hashes"`. In `render_full`,
widen the condition that prints the graph block to also fire when
`graph_tag_named_hashes` is non-empty, and print after the roots line:

```rust
        let _ = writeln!(
            out,
            "  tag-named hashes  {}",
            report.graph_tag_named_hashes.join(", ")
        );
```

Add one assertion to the round-trip test from Step 1, after the snapshot is
taken and before the load:

```rust
    let report = image::inspect::inspect(&bytes).expect("inspect");
    assert_eq!(
        report.graph_tag_named_hashes,
        vec![
            format!("Integer hash {integer_hash}"),
            format!("<root class> hash {class_hash}"),
            format!("<platform> hash {platform_hash}"),
        ],
        "inspect reads the table, in the encoder's fixed order"
    );
```

- [ ] **Step 9: Run the tests**

Run: `cargo test -p ns-runtime --test image --test opcode_tags`
Expected: every new test passes. Two kinds of existing test may now fail, and
only these two:

1. `the_wire_format_matches_the_golden_fixture` — the golden image gained its
   empty table. Regenerate it with
   `cargo test -p ns-runtime --test image regenerate_the_golden_fixture -- --ignored`
   (run from `crates/ns-runtime`, where the test's relative path points), then
   check that the new file is the old one with exactly four zero bytes
   appended and nothing else:
   `git show HEAD:crates/ns-runtime/tests/fixtures/golden-image.bin > $env:TEMP\old.bin`,
   then compare lengths (new = old + 4) and that the first `old` bytes are
   identical (`fc.exe /b` or a short PowerShell byte comparison) and the last
   four are `00 00 00 00`. Any other difference is a defect in Steps 5–7, not
   something to regenerate past.
2. A test that assembles a *whole image* by hand and passes it to
   `image::load` expecting success or an error past the graph — it now ends
   before the table and gets `unexpected end of image`. Append
   `w.write_u32(0); // no tag-named hashes` after its roots. A test that
   decodes the graph section alone (`GraphFixture`, `round_trip_graph`) must
   not change: the table is not part of `decode_graph_section`.

Then run the whole gate list from Global Constraints.
Expected: all green; `parse-corpus` and `run-corpus` unchanged.

- [ ] **Step 10: Commit**

```bash
git add -A crates/ns-runtime crates/ns-cli
git commit
```

Message:

```
fix: record the identity hashes of tag-named objects in an image

A builtin class, the root class and the platform singleton are named by
tag rather than carried as records, so since identity hashes became lazy
a hash one of them was given did not survive a snapshot. The graph
section now closes with a table of those hashes, in a fixed order and
only where one was assigned; load restores them and advances the counter
past the largest. The golden image gains the empty table's four bytes.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UjEeSba1SZqspUW2tnKvw3
```

---

### Task 2: The observable answer, end to end, and the documents

**Files:**
- Modify: `crates/ns-cli/tests/snapshot_load.rs` (new test, its source inline)
- Modify: `docs/design/image-format.md`, `docs/design/object-model.md` (if it repeats the exception), `docs/design/README.md`, `docs/PROGRESS.md`

**Interfaces:**
- Consumes: Task 1's `snapshot`/`load` behaviour, through the `ns-next` binary.

- [ ] **Step 1: Write the failing end-to-end test**

The program only means something across a snapshot, so it cannot be a
`tests-fixtures/*.ns` file: `run_fixtures.rs` runs every one of those and
demands an `.expected` beside it. It lives inline in the test.

Append to `crates/ns-cli/tests/snapshot_load.rs`:

```rust
const TAG_NAMED_HASHES: &str = "Newspeak3
'Test'
class TagNamedHashes usingPlatform: p = Object new (
\t| integerHash = Integer hash. classHash = self class hash. platformHash = p hash. |
) (
\tpublic check: q = (
\t\tq transcript show: (Integer hash = integerHash) printString; newLine.
\t\tq transcript show: (self class hash = classHash) printString; newLine.
\t\tq transcript show: (q hash = platformHash) printString; newLine
\t)
) : ()
";

/// The slot initializers ask a builtin class, the root class and the
/// platform for their hashes while the world is built; the load answers the
/// same three questions against the objects it rebuilt from tags. Before the
/// image recorded tag-named hashes, all three printed `false`.
#[test]
fn a_hash_given_to_a_tag_named_object_survives_a_snapshot() {
    let dir = ScratchDir::new("tag-named-hashes");
    let img = dir.join("world.img");
    let src = dir.join("tag_named_hashes.ns");
    std::fs::write(&src, TAG_NAMED_HASHES).expect("write source");
    let (ok, _, err) = ns_next(&["snapshot".as_ref(), src.as_os_str(), img.as_os_str()]);
    assert!(ok, "{err}");
    let (ok, out, err) = ns_next(&[
        "load".as_ref(),
        img.as_os_str(),
        "--send".as_ref(),
        "check:".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert_eq!(out, "true\ntrue\ntrue\n");
}
```

- [ ] **Step 2: Check the test against the parent commit**

Run it once against Task 1's parent to confirm it is a real test of the fix:
`git stash` is not needed — build the parent in a throwaway worktree
(`git worktree add ..\ns-next-parent HEAD~1`), run
`cargo run -p ns-cli -- snapshot` / `load --send check:` there on the same
source by hand, and confirm it prints `false` three times (`Integer hash` and
`p hash` are fresh draws after the load; `self class hash` is too). Remove the
worktree afterwards (`git worktree remove ..\ns-next-parent`).

Then run: `cargo test -p ns-cli --test snapshot_load a_hash_given_to_a_tag_named_object_survives_a_snapshot`
Expected: PASS.

- [ ] **Step 3: The documents**

- `docs/design/image-format.md`: in "Identity hashes are preserved verbatim",
  replace the paragraph that begins "**The exception is an object the image
  names by tag**" and the "Planned: the image records tag-named hashes"
  subsection with present-tense text: the graph section closes with the table
  (fixed order, only assigned hashes, the `HASHED_*` family), `load` restores
  it and refuses 0, a duplicate and `u32::MAX`, and the counter is advanced
  past both tables' maximum. Keep the prior-art paragraph (Cog/Spur,
  Primordial Soup) and the rejected alternative as the reason. Move the
  Decisions bullet from "Planned" to plain present tense. Add the new table
  to the "Sections" layout list if that list names the graph section's parts.
- `docs/design/object-model.md`: if its identity-hash section still says a
  tag-named object loses its hash across a snapshot, say instead that the
  image records it, and point at image-format.md.
- `docs/design/README.md`: the image-format row's status becomes "Built", and
  "recording tag-named hashes" leaves its "Planned next" column.
- `docs/PROGRESS.md`: add a short milestone entry after "M4 — The reference
  lowering" — "M4 — Tag-named identity hashes (2026-09-12)", naming this plan,
  the regression it closes, the new tag family, the golden image's four
  appended bytes, and the test count; update "Where the next session starts"
  so its first line names 1b as next and no longer mentions the hash decision.

- [ ] **Step 4: Gates and commit**

Run the whole gate list from Global Constraints. Expected: all green.

```bash
git add crates/ns-cli/tests/snapshot_load.rs docs
git commit
```

Message:

```
test: pin that a tag-named object's hash survives snapshot and load

Also brings image-format.md, object-model.md, the design map and the
progress tracker to the finished state.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UjEeSba1SZqspUW2tnKvw3
```

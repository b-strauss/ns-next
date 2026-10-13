# The typed arrays as a shipped module of their own — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `TypedArray` and its ten classes move out of `Kernel.ns` into a shipped module `platform/TypedArraysFactory.ns`, reached as `platform typedArrays`; `ByteArray`, `DataView` and `StringBuilder` stay in the kernel.

**Architecture:** The new module follows the factory pattern of `TimeFactory.ns` and `PlatformFactory.ns`: a root `TypedArraysFactory usingVmMirror: mirror kernel: kernel = Value`, booted once with the VM mirror (the 18 element-access leaves are mirror sends) and the kernel instance (for `Collection`, `IndexedCollection`, `ArgumentError`), one public member `build` answering a private facet `TypedArrays` that hands out the eleven classes. `PlatformFactory` reaches the module through `moduleInstanceNamed:` and answers its facet from a lazy `typedArrays` slot. `IndexedCollection` becomes public in the kernel so the module can apply it; the kernel facet (`KernelNamespace`) does not hand it out.

**Tech Stack:** Rust 1.98.0, Newspeak core library, goldens, `cargo-nextest`.

**Spec:** decided with the user on 2026-10-13 (this session): the name `platform typedArrays` and the module `platform/TypedArraysFactory.ns`; `platform typedArrays` is exempt from the on-demand rule for `PlatformFactory.ns`'s accessors, as the kernel facet is; `IndexedCollection` public in the kernel but not in `KernelNamespace`; done on `feature/byte-array-and-views` before its merge. The typed arrays' own design is `docs/design/core-library.md`, "`ByteArray`, `StringBuilder` and the views" -- unchanged but for where the classes live.

## Global Constraints

- Work on the checked-out branch `feature/byte-array-and-views` in `D:\Workspace\ns-next`, no worktree. Every commit is green under the per-commit gates of `CLAUDE.md`; the end-of-batch gates run once, after Task 3, by the controller.
- Commits are conventional, each ending with `Co-Authored-By:` naming the implementing model and `Claude-Session: https://claude.ai/code/session_01BcChjJmpsEYtz6GZ8wwPBo`.
- **Behaviour does not change.** Every typed-array golden keeps its stdout and stderr byte for byte, except where a line names where a class comes from; the goldens reach the classes through `platform typedArrays` instead of `platform kernel`. No golden that predates the branch changes.
- Every error text stays as it is.
- Every member of the new module and of the changed kernel members has a `///` comment to `CLAUDE.md`'s standard (`crates/ns-runtime/tests/core_library_documentation.rs` must list the new file).
- Any change to a shipped module's source or to `SHIPPED_MODULES` changes the fingerprint: regenerate `crates/ns-runtime/tests/fixtures/golden-image.bin` and check with `cmp -l` that only the 8 fingerprint bytes differ.
- After editing a `.ns` file: `cargo run -p ns-analyzer -- fmt <files>`.
- The VM mirror stays a capability (`CLAUDE.md`, "The VM mirror is a capability"): the new root's `vmMirror` slot is checked like every shipped module's that is handed the mirror, nothing hands out the root's instance, and its nested classes are reached only through the facet.

---

### Task 1: The kernel no longer depends on `TypedArray`, and `IndexedCollection` is public

**Files:** `crates/ns-runtime/core-library/Kernel.ns`, `crates/ns-runtime/src/interpreter/kernel.rs` (the private-class list), `crates/ns-runtime/tests/fixtures/golden-image.bin`

- [ ] **Step 1:** `DataView class>>new:byteOrder:` (~Kernel.ns:5920) builds its `ByteArray` with `outer Kernel TypedArray bytesFor: size width: 1`. Make it independent of `TypedArray`: the size of a `DataView` is in bytes, so `outer Kernel ByteArray new: size` gives `Array new:`'s texts with "a ByteArray of {n} bytes" -- check that `goldens/library/data-view/` keeps its output (if a refusal text of `new:byteOrder:` changes, keep the old text by checking the size first, as `bytesFor:width:` does). Search `Kernel.ns` for every other reference to a typed-array class outside the typed-array classes themselves and remove it the same way.
- [ ] **Step 2:** `IndexedCollection` becomes `public class IndexedCollection`; its `///` comment says why: a shipped module applies it (`TypedArraysFactory`), and no facet hands it to a program. Remove it from the private-class constant in `kernel.rs` (`only_the_kernels_private_classes_are_private`). `PlatformFactory.ns`'s `KernelNamespace` gains nothing.
- [ ] **Step 3:** Format, regenerate the fixture, run the per-commit gates. Commit `refactor: make IndexedCollection public and DataView independent of TypedArray`.

### Task 2: The module `TypedArraysFactory`

**Files:**
- Create: `crates/ns-runtime/core-library/platform/TypedArraysFactory.ns`
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (the eleven classes leave), `crates/ns-runtime/core-library/PlatformFactory.ns` (the module's lazy slot, `typedArrays`, and the eleven `KernelNamespace` accessors and private slots removed)
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs` (a `TYPED_ARRAYS_FACTORY_SOURCE` and a `SHIPPED_MODULES` entry with `ShippedFactory::MirrorAndKernel`; the doc comments listing the modules and what each factory takes)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` and `shipped_world.rs` tests, `crates/ns-runtime/tests/inline_caches.rs` (boot counts), `crates/ns-runtime/tests/image.rs` (`FIXTURE_NAMES` and the shipped names of the typed arrays and of `TypedArray`'s level), `crates/ns-runtime/tests/core_library_documentation.rs` (the new file), and every other test that names a typed array as a kernel class (`grep -rn "Int16Array\|TypedArray\|Float32Array" crates/ --include=*.rs`)
- Modify: every golden under `goldens/` that reaches a typed array through `platform kernel` (`grep -rln "kernel Int\|kernel Uint\|kernel Float\|kernel TypedArray" goldens`)
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin`

- [ ] **Step 1: Switch the goldens first.** Every golden that names a typed-array class reaches it through `platform typedArrays` (e.g. `Int16Array:: platform typedArrays Int16Array.`); outputs unchanged. They fail until Step 4.
- [ ] **Step 2: Write the module.** Header and root modelled on `platform/TimeFactory.ns` and `PlatformFactory.ns` (read both, their header comments state the factory pattern; adapt them): `class TypedArraysFactory usingVmMirror: mirror kernel: kernel = Value (` with the slots `private vmMirror = mirror.` and one private slot per kernel class the classes use (`Collection`, `IndexedCollection`, `ArgumentError`, and any other the moved code names through `outer Kernel …` -- `ByteArray` and `Array` are reached through `Object`'s accessors, bare); a private facet `class TypedArrays = Value () (` with one public accessor per class (`TypedArray` and the ten), each documented as `KernelNamespace`'s were; and `public build = ( ^TypedArrays new. )`. Move `TypedArray` and the ten classes verbatim from `Kernel.ns`, changing only `outer Kernel X` to what reaches `X` here (`outer TypedArraysFactory X` for the slots and the module's own classes), and `TypedArray`'s superclass clause to `Collection <: IndexedCollection` over the slots. Their comments move with them; adjust any sentence that says "the kernel".
- [ ] **Step 3: Boot it.** `core_library.rs`: the source constant and the table entry (after `CollectionsFactory`, or wherever the table's order rule puts it -- read its comment). Check that the mirror-slot check (~core_library.rs:246-277) covers the new module because its factory takes the mirror. `PlatformFactory.ns`: `lazy private typedArraysModule = vmMirror moduleInstanceNamed: #TypedArraysFactory ifFail: [nil].` and `Platform`'s `lazy public typedArrays = typedArraysModule build.`, documented as `time` is, its comment saying it is exempt from the on-demand rule as the kernel facet is (decided with the user on 2026-10-13); remove the eleven typed-array slots and `KernelNamespace` accessors.
- [ ] **Step 4: The Rust tests.** `kernel.rs`'s tables that list the typed arrays as kernel classes (`TYPED_ARRAYS`, `TYPED_ARRAY_MEMBERS`, `NOT_FORWARDERS`, `ORDINARY_CLASS_QUICK_METHODS`, the `KernelNamespace` selector list) move to the shipped modules' fast-path test (`the_platform_modules_members_have_the_fast_path_they_should`) or are removed where the module's own members are checked; `shipped_world.rs`'s positions test names `TypedArraysFactory TypedArray`, `TypedArraysFactory Int16Array` (value, superclass, header), `TypedArraysFactory TypedArrays` and the module root (record's class only), `TypedArraysFactory TypedArray level 0` (frame class only); the image tests' crafted records name the new shipped names; `inline_caches.rs`'s boot counts change and their comment says why.
- [ ] **Step 5:** Format both `.ns` files and the goldens, regenerate the fixture, run the per-commit gates and `NS_NEXT_STRESS_MOVES=1 cargo nextest run -p ns-cli -E 'test(/typed-array/) | test(/builtin-subclass/)'`. Commit `refactor: move the typed arrays into the shipped module TypedArraysFactory`.

### Task 3: Documentation

**Files:** `CLAUDE.md` (the `ns-runtime` paragraph lists the shipped modules; lock (2) names the modules the bootstrap hands the mirror; the on-demand rule names the exempt facets), `docs/design/core-library.md` (the class table and where each class is reached; "The module table and the boot"; the factories), `docs/design/README.md` (the map's status), `docs/design/image-format.md` (shipped names of the typed arrays, if named), `docs/OPEN-WORK.md` (the boot-cost item: the typed arrays are now a module of their own, so building modules on first demand would spare a program that does not use them), `docs/PROGRESS.md`.

- [ ] **Step 1:** Make every statement current; search `docs/` and `CLAUDE.md` for `platform kernel Int`, `Kernel TypedArray`, `typed arrays … kernel`, `private class IndexedCollection`, `TimeFactory` lists of modules. Commit (Markdown only, no gates) `docs: describe the typed arrays as a module of their own`.

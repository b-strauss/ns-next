---
name: measuring-performance
description: Use when timing ns-next — comparing two trees or commits, benchmarking a change to the run loop, a send's path or the collector, running `ns-next bench`, or judging whether a change made the VM faster or slower.
---

# Measuring ns-next's performance

## Overview

A timing compares builds, not commits, unless every build is made the same
way and there are enough of them. The protocol below removes the two known
sources of build-to-build noise; `docs/design/execution.md`, "What removing
the re-entries moved", has the full measurement protocol and its history.

## Protocol

1. **Release builds only.** Never judge performance from a debug build.
   **Run `cargo xtask shipped-code` in the tree before every build that is
   measured**, or the binary embeds no current shipped code, boots from
   source -- about 13.6 ms against 1.6 ms for a hello program -- and the
   measurement measures that (`docs/design/core-library.md`, "The embedded
   shipped code"). `cargo xtask shipped-code --check` says whether the
   artifact is current. A tree from before the embedded shipped code has
   no such task and always boots from source.
2. **Pin function alignment on every binary compared, the current tree
   included.** In PowerShell:

   ```powershell
   $env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
   cargo build --workspace --release
   Remove-Item Env:RUSTFLAGS
   ```

   `RUSTFLAGS` replaces the configuration's flags, so `+crt-static` is
   repeated; a Linux or macOS build carries the alignment flag alone. Without
   the pin, identical machine code measured 1–2% apart through a 64-byte phase
   shift of every hot function. The shipped build does not carry the pin: it
   costs about 110 KB (5.6%) and measured no faster.
3. **Several builds per tree, pooled.** With the pin in place,
   `bench_block.ns` still measured 16% apart between two builds of the same
   tree, each stable to about 1%. One build per side measures the build.
   **The three builds vary the block layout inside `vm::run`**, the same
   three on both sides: build a with the pinned `RUSTFLAGS` alone, build b
   adding `-C metadata=buildb -C llvm-args=-align-all-nofallthru-blocks=4`,
   build c adding `-C metadata=buildc -C llvm-args=-align-all-nofallthru-blocks=5`.
   **They also lay the JIT's code zone out differently**, through two
   environment variables read when the binary is built
   (`option_env!`; a value is a multiple of 16 up to 4,096, or the build
   fails): build a sets neither, build b sets `NS_NEXT_ZONE_STUB_PADDING=208`
   (bytes before the first stub) and `NS_NEXT_ZONE_BODY_PADDING=32` (bytes
   between the last stub and the first body), build c sets `96` and `176`.
   The same machine code at another place in the zone ran 17% apart on
   `bench_while.ns` over seventeen paddings, and a stub's growth moves
   every later stub and body; the stubs' offsets (0, 208, 96) and the bodies'
   (the sum of the two paddings: 0, 240, 272) are pairwise distinct modulo
   64, so each build puts the stubs and the bodies on other cache-line
   phases. Both sides take the same three; a tree from before the
   paddings existed takes none, and the report says so. A regression the
   three builds still separate is investigated as before, a sweep of
   paddings for the one program allowed there.
   `-C metadata` alone reorders functions but leaves `vm::run` identical at
   every offset, so three such builds sample one layout of the run loop:
   road step 7a measured `bench_loop.ns` +1.7%, builds separated, on code
   instruction for instruction what `master` ran, and level once the
   blocks moved (`execution.md`, "What frames as words moved"). Check in a
   `dumpbin /disasm` of each build with debug information (point 6) that
   the three `vm::run`s differ in size and padding; if two coincide, take
   another alignment and say so. **Build them at once**, each in its own
   `--target-dir`: the release profile's fat LTO ends in a pass on one
   core, so builds one after another leave most cores idle. Nothing is
   timed while they build.
4. **Interim and final measurements differ.** Each step of a batch's work,
   a bisection probe or an experiment, is an *interim* measurement: one
   pinned build a side, only the sources the change can move (the micro
   sources and `bench_new*` for a send or instantiation change), `--iterations
   15`, three rounds, no seconds-long Are We Fast Yet program. It answers
   "did this move it, roughly"; a difference within the 16% build spread of
   point 3 is no verdict. The numbers a design document records, and the
   decision whether a batch regressed, are the *final* measurement: point 3's
   three builds a side, three rounds rotating, one sitting, and the Are We
   Fast Yet programs and the micro sources (point 5). Bisect a regression
   only once the final measurement shows it.
5. **Three runs of the slow programs.** Of the Are We Fast Yet programs, CD,
   Havlak, Mandelbrot, NBody and DeltaBlue take seconds per run: time them
   with `bench --iterations 3`, every other program and every micro source
   with `--iterations 8` (`execution.md`, "The Are We Fast Yet
   benchmarks"). Ten runs of them made a three-builds-a-side comparison
   take an hour and a half. Two
   ns-next trees are compared with `bench` alone; the harness's `--timed`
   is for comparing with Primordial Soup or another VM.
6. **Before timing a change to a send's path, check the inlining.**
   `run_stretch` (the loop over the frame's locals between two safepoints)
   sits near LLVM's inlining threshold and is forced inline into `vm::run`. On
   a release build with debug information, `dumpbin /disasm` (Visual Studio
   Build Tools; not on `PATH`) must show no `vm::run::closure` and no
   `run_stretch` as a function of its own (`execution.md`, "What the inline
   caches moved", "What the loop over locals moved").
7. **Pin every timed process to the performance cores.** On this machine's
   Core Ultra 9 285K, Windows moves a process that runs for a few seconds
   onto an efficiency core: a whole sitting of road step 9's second stage
   was invalid for it, Mandelbrot +4.1% on unchanged code. Start each
   timed `ns-next` with the affinity of the eight performance cores, mask
   `0xC03C03` (in PowerShell `Start-Process` and then
   `$process.ProcessorAffinity = 0xC03C03`, or `cmd /c start /affinity
   C03C03 /wait …`), on both sides, and say so in the report.
8. **Never time with `--jit-statistics`.** The flag switches the JIT's
   counting on, which compiles other machine code -- counted stubs and
   counted dispatching tails -- so a timed run with it measures that code.
   Take the statistics in runs of their own, beside the timed ones.
9. **Report** the per-build numbers, the pooled result and the spread, and
   name what the measurement did not cover before drawing a design
   conclusion from it.

## Known costs

| Change | Measured |
|---|---|
| One more instruction arm that calls a function | about 3% on `bench_loop.ns` and `bench_send.ns` — a store-forwarding stall on the send path (`execution.md`, "Where the 3% goes") |

## Common mistakes

- Comparing one pinned build against one unpinned build, or the current tree
  built with the configuration's flags against an old one built with the pin.
- A single build per side for a source known to be build-sensitive.
- Three builds a side told apart by `-C metadata` alone, which pool one
  layout of `vm::run` three times.
- Timing without the affinity to the performance cores, so the scheduler's
  move to an efficiency core shows up as a regression.
- Timing a run with `--jit-statistics`, whose counting changes the code.
- Timing a send-path change without the `dumpbin` check, so an outlined
  `run_stretch` shows up as the change's cost.

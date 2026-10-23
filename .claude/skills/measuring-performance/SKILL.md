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
   **Build them at once**, each in its own `--target-dir` (the second and
   third adding `-C metadata=buildb`, `-C metadata=buildc` to the pinned
   `RUSTFLAGS`): the release profile's fat LTO ends in a pass on one core,
   so builds one after another leave most cores idle. Nothing is timed while
   they build.
4. **Interim and final measurements differ.** Each step of a batch's work,
   a bisection probe or an experiment, is an *interim* measurement: one
   pinned build a side, only the sources the change can move (the micro
   sources and `bench_new*` for a send or instantiation change), `--iterations
   30`, three rounds, no seconds-long Are We Fast Yet program. It answers
   "did this move it, roughly"; a difference within the 16% build spread of
   point 3 is no verdict. The numbers a design document records, and the
   decision whether a batch regressed, are the *final* measurement: point 3's
   three builds a side, five rounds rotating, two sittings, and the Are We
   Fast Yet programs (point 5). Bisect a regression only once the final
   measurement shows it.
5. **Three runs of the slow programs.** Of the Are We Fast Yet programs, CD,
   Havlak, Mandelbrot, NBody and DeltaBlue take seconds per run: time them
   with `bench --iterations 3`, every other program with `--iterations 10`
   (`execution.md`, "The Are We Fast Yet benchmarks"). Ten runs of them
   made a three-builds-a-side comparison take an hour and a half. Two
   ns-next trees are compared with `bench` alone; the harness's `--timed`
   is for comparing with Primordial Soup or another VM.
6. **Before timing a change to a send's path, check the inlining.**
   `run_stretch` (the loop over the frame's locals between two safepoints)
   sits near LLVM's inlining threshold and is forced inline into `vm::run`. On
   a release build with debug information, `dumpbin /disasm` (Visual Studio
   Build Tools; not on `PATH`) must show no `vm::run::closure` and no
   `run_stretch` as a function of its own (`execution.md`, "What the inline
   caches moved", "What the loop over locals moved").
7. **Report** the per-build numbers, the pooled result and the spread, and
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
- Timing a send-path change without the `dumpbin` check, so an outlined
  `run_stretch` shows up as the change's cost.

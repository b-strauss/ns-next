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
4. **Three runs of the slow programs.** Of the Are We Fast Yet programs, CD,
   Havlak, Mandelbrot, NBody and DeltaBlue take seconds per run: time them
   with `bench --iterations 3`, every other program with `--iterations 10`
   (`execution.md`, "The Are We Fast Yet benchmarks"). Ten runs of them
   made a three-builds-a-side comparison take an hour and a half.
5. **Before timing a change to a send's path, check the inlining.**
   `run_stretch` (the loop over the frame's locals between two safepoints)
   sits near LLVM's inlining threshold and is forced inline into `vm::run`. On
   a release build with debug information, `dumpbin /disasm` (Visual Studio
   Build Tools; not on `PATH`) must show no `vm::run::closure` and no
   `run_stretch` as a function of its own (`execution.md`, "What the inline
   caches moved", "What the loop over locals moved").
6. **Report** the per-build numbers, the pooled result and the spread, and
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

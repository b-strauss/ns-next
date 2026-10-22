# Java counterparts of the Are We Fast Yet benchmarks

Decided with the user on 2026-10-22, before `new` as compiled code.

## Goal

Every program of `benchmarks/are-we-fast-yet/` gets a Java counterpart that
does the same work, so ns-next's scores can be put beside a JVM's under the
same protocol. That covers the fourteen Are We Fast Yet benchmarks, the two
Ton80 programs and the harness. The road measures its later steps against
Java (`OPEN-WORK.md`, "The road to a high-performance VM"); this is the
Java side of that comparison.

Decided:

- **Same work, not idiomatic Java.** The Java programs keep Are We Fast
  Yet's rules: the algorithm of the Newspeak version, and the `som`
  collections (`Vector`, `Dictionary`, `Set`, ...) in place of `java.util`.
  A comparison measures the two VMs on one program, not two programs.
- **The source is upstream Are We Fast Yet's Java**, adapted, not a fresh
  translation. Only the two Ton80 programs, which have no Java version
  there, are translated from our `.ns` ports.
- **Only the sixteen programs.** The microbenchmarks under
  `benchmarks/micro/` get no Java counterpart. They time the interpreter's
  internals in loops a JIT removes, so a Java score for them says nothing.
- **Plain `javac`, verified in the `benchmarks` gate.** No Maven, no
  Gradle, no JMH.
- **One protocol for both harnesses, with adjustable windows.** The Java
  harness measures exactly as `AreWeFastYet.ns` does. Both harnesses gain
  optional flags for the warm-up and measurement windows, so a peak
  comparison can give the JVM's JIT time to finish, the same on both sides.

## Provenance and licences

`smarr/are-we-fast-yet` becomes a submodule at `prior-art/are-we-fast-yet`,
pinned at `74306fec151070fd07157cefeacf19e7e0bcdc89` (2026-03-22).
`prior-art/PROVENANCE.md` gets its row in both tables and its licence entry,
"Code is adapted from it", naming the Java sources and where they went.

Upstream is not under one licence; its `LICENSE.md` says so. The Java
sources fall into these groups, and each adapted file keeps the notice it
carries upstream, as the `.ns` ports do:

| Files | Licence |
|---|---|
| `Benchmark`, `Harness`, `Run`, `Bounce`, `List`, `Permute`, `Queens`, `Sieve`, `Storage`, `Towers`, `som/*`, `CD`, `cd/*`, `Havlak`, `Json`, `json/JsonPureStringParser` | MIT, Are We Fast Yet's / SOM's authors (Stefan Marr et al.) |
| `json/*` but `JsonPureStringParser` | MIT, EclipseSource (minimal-json) |
| `havlak/*` | Apache 2.0, Google |
| `Mandelbrot` | Revised BSD, the Computer Language Benchmarks Game |
| `NBody`, `nbody/*` | the Computer Language Benchmarks Game; the file names it without a licence text, so upstream `LICENSE.md`'s Revised BSD text applies |
| `Richards`, `richards/*`, `DeltaBlue`, `deltablue/*` | derived from Mario Wolczko's versions; the files point to his licence page, as SOMns's do for the `.ns` sources we already adapted |

`THIRD-PARTY-NOTICES.md` gets an entry "Are We Fast Yet" that carries
upstream's `LICENSE.md` and the licence texts above that it does not
already carry for SOMns. The Ton80 translations are derived from our
ports of Primordial Soup's Apache 2.0 files and carry the headers those
ports carry.

## Layout

Everything lives under `benchmarks/are-we-fast-yet/java/`, beside the `.ns`
programs, with upstream's package structure:

```
benchmarks/are-we-fast-yet/java/
  AreWeFastYet.java        the harness (default package, as upstream's Harness)
  Benchmark.java
  Bounce.java CD.java DeltaBlue.java DeltaBlueTon80.java Havlak.java
  Json.java List.java Mandelbrot.java NBody.java Permute.java Queens.java
  Richards.java RichardsTon80.java Sieve.java Storage.java Towers.java
  som/  cd/  deltablue/  havlak/  json/  nbody/  richards/
  deltablueton80/  richardston80/
```

Upstream's `Harness.java` and `Run.java` are folded into `AreWeFastYet.java`.
Its `build.xml`, `build.sh` and `.checkstyle_checks.xml` are not taken. The
sources compile with `javac --release 21`.

## What changes from upstream

As much as our `.ns` ports changed, and no more:

- **Protocol.** `Benchmark` has one method, `void bench()`. It runs the
  workload once at our size and throws an `IllegalStateException` whose
  message is the `.ns` port's `Error` text (`Sieve: expected 669, got
  …`) when the result is wrong. Upstream's `benchmark()`,
  `verifyResult(Object)` and `innerBenchmarkLoop(int)` become that one
  method.
- **Sizes and results are the `.ns` ports'.** Where a port departs from
  upstream's default, the Java version departs the same way:
  - Mandelbrot 750, answering 50
  - NBody 250000 steps, answering the energy `-0.1690859889909308`
  - DeltaBlue 12000
  - Havlak 1500, answering 6102 loops in 5213 nodes, with the port's
    nesting
  - CD 250, answering 10830 collisions
  - Json parsing its input once and checking what the port checks
  - Sieve 669, Permute 8660, Towers 8191, Bounce 1331, Storage 5461,
    List 10, and Queens and Richards their ports' checks

  Every departure a port's header names, its Java version names too.
- **Names are written out**, as in the `.ns` ports (`index`, not `i`;
  `random`, not `rnd`), in every identifier the adaptation touches. The
  Java keywords and `java.lang` names stay as they are.
- **Header.** Each file starts with a comment naming its upstream file and
  commit and what was changed, then upstream's notice. This is the
  `.ns` ports' header, as a Java comment.

The algorithms and the `som` collections are not changed. Any departure
from upstream's algorithm beyond the ones above is a defect.

The two Ton80 programs are translated from `RichardsTon80.ns` and
`DeltaBlueTon80.ns` class by class, method by method, into the same
`Benchmark` protocol, keeping their sizes (100 and 100) and their checks.

## The harnesses

`AreWeFastYet.java` takes the same command line and prints the same lines
as `AreWeFastYet.ns`:

```
java -cp <classes> AreWeFastYet [--timed [--warm-up <milliseconds>] [--measure <milliseconds>]] [Benchmark]
```

- Without `--timed`, it runs each benchmark, or the one named, once and
  prints `<Name>: ok`, in the `.ns` harness's order.
- With `--timed`, it measures each benchmark for the warm-up window, then
  for the measurement window, and prints `<Name>: <runs per second>` with
  one fraction digit. `measure` is Primordial Soup's loop: run, count,
  read `System.nanoTime()`, repeat while the elapsed time is under the
  window, answer runs × 10⁹ / elapsed.
- An unknown name throws, with the message `Unknown benchmark: <name>`.
  So does a malformed command line, with the `.ns` harness's usage text,
  adjusted to `java AreWeFastYet`. Either way the process exits non-zero.

`AreWeFastYet.ns` gains the same two flags:

- `--warm-up <milliseconds>` and `--measure <milliseconds>` are accepted
  only after `--timed`, in either order, each at most once.
- They default to 3 and 20, today's windows, so the scores and the
  pinned outcome stay as they are.
- A value is a non-negative decimal integer, read with `Integer parse:`.
  Anything else, a flag without `--timed`, or a repeated flag is the
  usage error.
- The usage text names the flags. The header comment describes them, and
  so does `execution.md`.

## Verification

The `benchmarks` gate (profile `benchmarks-verify`) runs the Java side. It
lives in `crates/ns-cli/tests/run_benchmarks.rs`, beside the ns-next
trials:

- **One compilation.** The first Java trial compiles every `.java` under
  `benchmarks/are-we-fast-yet/java/` with
  `javac --release 21 -d <CARGO_TARGET_TMPDIR>/are-we-fast-yet-java`,
  behind a `OnceLock`. A compilation error fails every Java trial with
  `javac`'s output.
- **Finding the JDK.** `javac` and `java` come from `$JAVA_HOME/bin` when
  `JAVA_HOME` is set, and from `PATH` otherwise. When neither yields a
  runnable `javac`, the trial fails and says the gate needs a JDK. It is
  never skipped, because a gate that passes without running is no gate.
- **`java::<Name>`.** For each benchmark, the Java harness run by name, its
  stdout held to that benchmark's line of `benchmarks/are-we-fast-yet.toml`
  (`<Name>: ok`). The `.ns` and Java programs share one record of what
  each must answer.
- **`java-timed::<Name>`.** `--timed <Name>`, its score's shape checked as
  `timed::<Name>` checks ns-next's.
- **The flags.** On both harnesses, `--timed --warm-up 0 --measure 1
  Sieve` prints a score of the right shape, and `--warm-up 5` without
  `--timed` and `--timed --measure x` each end in the usage error.

The default profile is untouched: no JDK is needed for an ordinary commit.

## Documentation

- `docs/design/execution.md`, "The Are We Fast Yet benchmarks": the Java
  counterparts, where they come from, what was changed, the shared
  protocol and the two flags, and how the gate runs them.
- `CLAUDE.md`: the `benchmarks/are-we-fast-yet/` line names the Java
  counterparts, and the `benchmarks` gate row says it needs a JDK
  (`JAVA_HOME` or `PATH`).
- `prior-art/PROVENANCE.md` and `THIRD-PARTY-NOTICES.md` as above.
- `docs/PROGRESS.md` gets its entry. `docs/OPEN-WORK.md` loses any item
  this closes.
- This spec is temporary. Its content moves into `execution.md`, and the
  spec is deleted at the end of the batch.

## Not in scope

- Java counterparts of `benchmarks/micro/`.
- JMH, Maven or Gradle.
- A script that runs both harnesses and tabulates ns-next against Java.
- Any change to a benchmark's workload or size on the Newspeak side.

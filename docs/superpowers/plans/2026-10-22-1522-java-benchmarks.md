# Java counterparts of the Are We Fast Yet benchmarks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every program of `benchmarks/are-we-fast-yet/` gets a Java
counterpart that does the same work, run by a Java harness with
`AreWeFastYet.ns`'s command line and output, and verified in the
`benchmarks` gate.

**Architecture:** Upstream Are We Fast Yet's Java sources (a new submodule
`prior-art/are-we-fast-yet`) are adapted into
`benchmarks/are-we-fast-yet/java/`, under a one-method protocol
(`bench()`), at our `.ns` ports' sizes. The two Ton80 programs are
translated from our `.ns` ports. `crates/ns-cli/tests/run_benchmarks.rs`
compiles them once with `javac` and holds each to the line of
`benchmarks/are-we-fast-yet.toml` the `.ns` program is held to. Both
harnesses gain `--warm-up` and `--measure`.

**Tech Stack:** Java (`javac --release 21`, JDK 27 installed locally), Rust
integration tests (`libtest-mimic`), Newspeak.

**Spec:** `docs/superpowers/specs/2026-10-22-java-benchmarks-design.md`

## Global Constraints

- Branch `java-benchmarks` (exists; the spec is committed on it).
- Upstream: `https://github.com/smarr/are-we-fast-yet.git`, commit
  `74306fec151070fd07157cefeacf19e7e0bcdc89` (2026-03-22); the Java sources
  are under its `benchmarks/Java/src/`.
- Same work, not idiomatic Java: upstream's algorithm and its `som`
  collections, never `java.util` in a benchmark (the harness may use it).
- Every Java file starts with a comment naming its upstream file and commit
  and what was changed, then upstream's notice verbatim. A file with no
  notice upstream keeps upstream's comment as it is.
- Names are written out in every identifier the adaptation writes or
  touches (`CLAUDE.md`, "Names are written out"): no identifier of one or
  two characters, no truncated word (`index`, `random`, `count`, not `i`,
  `rnd`, `cnt`).
- `javac --release 21`; no build tool, no dependency.
- Sizes and expected results are the `.ns` ports' (spec, "What changes
  from upstream"). A wrong result throws `IllegalStateException` with the
  port's `Error` text verbatim.
- Every commit that changes more than Markdown runs
  `cargo xtask gates commit`. At the end of the batch run `cargo xtask
  gates end-of-batch` (`ns-cli`'s tests are runtime per `CLAUDE.md`). This
  staggering holds for every task.
- Commits are conventional (`feat:`, `test:`, `docs:`, `chore:`) and end
  with the session's `Co-Authored-By` and `Claude-Session` lines.

## Review Focus

- **The score is printed in the JVM's default locale.** On a German Windows
  `String.format("%.1f", …)` writes `123,4`. Use `Locale.ROOT`; the
  `java-timed::` shape check (Task 1) catches it on this machine.
- **`JAVA_HOME` names a directory that does not exist** (it did on this
  machine earlier today). The failure must name the path it tried, not
  fall back silently or report a bare spawn error. Test in Task 1.
- **A stale class from an earlier compile.** A renamed or deleted `.java`
  must not leave a class that still runs. Task 1 empties the output
  directory before every compile.
- **A benchmark whose check fails.** The harness must print no `ok` line
  for it and exit non-zero; the line is written only after `bench()`
  returns. Test in Task 1.
- **Java's `int` where Newspeak's integers are unbounded.** An adapted or
  translated computation that overflows 32 bits silently gives a different
  result. Upstream's types are kept. The Ton80 translations (Task 4) use
  `long` wherever the `.ns` value can exceed 2^31.

---

### Task 1: Provenance, protocol, harness, the gate, and the seven small benchmarks

**Files:**
- Create: submodule `prior-art/are-we-fast-yet` (`.gitmodules`)
- Modify: `prior-art/PROVENANCE.md` (description table, pin table,
  licence list, and the paragraph naming adapted code, beside `somns`)
- Modify: `THIRD-PARTY-NOTICES.md` (new section `## Are We Fast Yet`)
- Create: `benchmarks/are-we-fast-yet/java/Benchmark.java`,
  `AreWeFastYet.java`, `som/*.java` (all twelve upstream `som` files),
  `Bounce.java`, `List.java`, `Permute.java`, `Queens.java`,
  `Sieve.java`, `Storage.java`, `Towers.java`
- Modify: `crates/ns-cli/tests/run_benchmarks.rs`

**Interfaces:**
- Produces: `public abstract class Benchmark { public abstract void bench(); }`
  in the default package. A benchmark is a `public final class <Name>
  extends Benchmark` with a public no-argument constructor; the
  constructor does no work, so every `bench()` runs the whole workload,
  as the `.ns` `bench` does.
- Produces: `AreWeFastYet.java` with one ordered registry,
  `LinkedHashMap<String, Supplier<Benchmark>>`, in the `.ns` harness's
  order (Bounce, CD, DeltaBlue, DeltaBlueTon80, Havlak, Json, List,
  Mandelbrot, NBody, Permute, Queens, Richards, RichardsTon80, Sieve,
  Storage, Towers). Later tasks add their entries there and nothing else.
- Produces in `run_benchmarks.rs`: `fn java_classes() -> Result<&'static Path, String>`
  (compiles once behind a `OnceLock`), `fn java_tool(name: &str) -> Result<PathBuf, String>`,
  and `fn java_harness(arguments: &[&str]) -> Result<std::process::Output, String>`.

- [ ] **Step 1: Add the submodule and the licence records**

```bash
git submodule add https://github.com/smarr/are-we-fast-yet.git prior-art/are-we-fast-yet
git -C prior-art/are-we-fast-yet checkout 74306fec151070fd07157cefeacf19e7e0bcdc89
```

`PROVENANCE.md`: a row for `are-we-fast-yet/` in each table (the pin row
reads `74306fec…`, `2026-03-22`, `2026-10-22`). Its licence entry says the
licence is mixed and points to the spec's table. "Code is adapted from it:
the Java sources in `benchmarks/are-we-fast-yet/java/`, from
`benchmarks/Java/src/`." `THIRD-PARTY-NOTICES.md`, `## Are We Fast Yet`:
upstream URL and commit, upstream's `LICENSE.md` verbatim, then each
licence text from the spec's table not already in the file: the MIT
notices of Are We Fast Yet's authors and of EclipseSource, Google's
Apache 2.0 notice if the Primordial Soup section's Apache text does not
already cover it by reference, and the Benchmarks Game's Revised BSD.

- [ ] **Step 2: Write the failing Java trials in `run_benchmarks.rs`**

For each name of `common::benchmark_names(&expectation)`, add:
- `java::<Name>`: `java_harness(&[name])`, compared with `common::compare`
  against an `Expectation` whose stdout is the line `<Name>: ok\n`
  (status 0, stderr empty);
- `java-timed::<Name>`: `java_harness(&["--timed", name])`, its stdout
  checked by the shape check `prints_a_score` already does. Factor that
  check into `fn score_is_shaped(stdout: &str, name: &str) -> bool` and
  use it on both sides.

Plus three trials:
- `java::unknown-benchmark`: `java_harness(&["NoSuchBenchmark"])` exits
  non-zero, stdout empty, stderr contains `Unknown benchmark: NoSuchBenchmark`.
- `java::a-failing-check-prints-no-ok`: run
  `java -cp <classes> AreWeFastYet Sieve` with the system property
  `-DareWeFastYet.failFirstCheck=true`, which makes the harness throw
  `IllegalStateException("forced failure")` in place of the first
  `bench()` call. It exits non-zero and has empty stdout. That property is
  the harness's only test hook.
- `java::missing-jdk-is-named`: `java_tool_in(Some(Path::new("D:/no/such/jdk")), "javac")`
  answers an `Err` whose text contains `D:/no/such/jdk` and `JAVA_HOME`.
  `java_tool` is `java_tool_in(std::env::var_os("JAVA_HOME").map(PathBuf::from).as_deref(), name)`.

`java_tool_in(java_home: Option<&Path>, name: &str) -> Result<PathBuf, String>`
behaves like this:
- With a home, it answers `<home>/bin/<name>` (`.exe` on Windows) if that
  file exists. Otherwise it answers an error: `JAVA_HOME is <home>, which
  has no bin/<name>; the benchmarks gate needs a JDK`.
- Without a home, it answers `<name>`, resolved on `PATH`.

`java_classes()` does three things:
- empties `<CARGO_TARGET_TMPDIR>/are-we-fast-yet-java` (`remove_dir_all`,
  then `create_dir_all`);
- runs `javac --release 21 -encoding UTF-8 -d <that directory>` on every
  `.java` under `benchmarks/are-we-fast-yet/java/`, collected recursively
  and sorted;
- on failure, answers an error carrying `javac`'s stderr. A spawn failure
  says the gate needs a JDK, on `JAVA_HOME` or `PATH`.

- [ ] **Step 3: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --profile benchmarks-verify -E "test(/^java/)"`
Expected: FAIL. Nothing is compiled yet, so every trial but
`java::missing-jdk-is-named` fails.

- [ ] **Step 4: Write `Benchmark.java` and `AreWeFastYet.java`**

Header: `Harness.java` and `Run.java` are folded into it, and the measuring
loop and command line are `AreWeFastYet.ns`'s. Both upstream files are MIT
(SOM's authors).

`AreWeFastYet.main` accepts `[--timed] [Benchmark]` only in this task;
Task 5 adds the flags. Without `--timed`, for each selected benchmark: a
fresh instance, `bench()` once, then print `<Name>: ok`. With `--timed`:
`measure(instance, 3_000_000L)`, then `measure(instance, 20_000_000L)`,
then print `<Name>: ` + `String.format(Locale.ROOT, "%.1f", score)`.
`static double measure(Benchmark benchmark, long nanoseconds)` is
Primordial Soup's loop:

```java
long runs = 0;
long start = System.nanoTime();
long elapsed;
do {
  benchmark.bench();
  runs++;
  elapsed = System.nanoTime() - start;
} while (elapsed < nanoseconds);
return (runs * 1_000_000_000.0) / elapsed;
```

An unknown name throws
`IllegalArgumentException("Unknown benchmark: " + name)`. More than one
argument after the optional `--timed` throws
`IllegalArgumentException("usage: java AreWeFastYet [--timed] [Benchmark]")`.
Exceptions are left uncaught, so the JVM exits with status 1 and the
message on stderr. Output goes through `System.out.println`; the tests
normalize newlines.

- [ ] **Step 5: Adapt `som/*` and the seven small benchmarks**

Copy each from `prior-art/are-we-fast-yet/benchmarks/Java/src/`. Turn
`benchmark()` + `verifyResult()` into `bench()`, which runs once and
checks with the message the `.ns` port signals:

| Benchmark | Check, verbatim from the port |
|---|---|
| Sieve | `Sieve: expected 669, got <n>` |
| Permute | `Permute: expected 8660, got <n>` |
| Towers | `Towers: expected 8191, got <n>` |
| Bounce | `Bounce: expected 1331, got <n>` |
| Storage | `Storage: expected 5461, got <n>` |
| List | `List: expected 10, got <n>` |
| Queens | `Queens: expected true, got false` |

Read each port's `bench` for anything else it does differently (Bounce's
header names one difference) and carry it over. Write out every short
name.

- [ ] **Step 6: Run the Java trials of these seven**

Run: `cargo nextest run -p ns-cli --profile benchmarks-verify -E "test(/^java(-timed)?::(Bounce|List|Permute|Queens|Sieve|Storage|Towers|unknown-benchmark|a-failing-check-prints-no-ok|missing-jdk-is-named)$/)"`
Expected: PASS, 17 tests. The other names fail until Tasks 2-4; that is
expected.

- [ ] **Step 7: Gates and commit**

Run `cargo xtask gates commit`; expected green. Then:

```bash
git add .gitmodules prior-art/are-we-fast-yet prior-art/PROVENANCE.md THIRD-PARTY-NOTICES.md benchmarks/are-we-fast-yet/java crates/ns-cli/tests/run_benchmarks.rs
git commit -m "feat: Java counterparts of seven Are We Fast Yet benchmarks, verified in the benchmarks gate"
```

### Task 2: Mandelbrot, NBody, Richards and Json

**Files:**
- Create: `benchmarks/are-we-fast-yet/java/Mandelbrot.java`, `NBody.java`,
  `nbody/*.java`, `Richards.java`, `richards/*.java`, `Json.java`,
  `json/*.java`
- Modify: `benchmarks/are-we-fast-yet/java/AreWeFastYet.java` (registry)

**Interfaces:**
- Consumes: `Benchmark`, the registry, `som/*` (Task 1).

- [ ] **Step 1: Run the four trials to see them fail**

Run: `cargo nextest run -p ns-cli --profile benchmarks-verify -E "test(/^java(-timed)?::(Mandelbrot|NBody|Richards|Json)$/)"`
Expected: FAIL, `Unknown benchmark`.

- [ ] **Step 2: Adapt the four from upstream, at the ports' sizes**

- Mandelbrot: size 750, expects 50.
- NBody: 250000 steps, expects the energy `-0.1690859889909308`.
- Richards: the port's check.
- Json: parses its input once and checks what the `.ns` port checks.

Each check throws with the port's message verbatim. Each header names
every departure the port's header names. Mandelbrot keeps Brent Fulgham's
BSD text, `json/*` EclipseSource's MIT.

- [ ] **Step 3: Run them to see them pass**

Same command as Step 1. Expected: PASS, 8 tests.

- [ ] **Step 4: Gates and commit**

`cargo xtask gates commit`, then
`git commit -m "feat: Java counterparts of Mandelbrot, NBody, Richards and Json"`.

### Task 3: DeltaBlue, Havlak and CD

**Files:**
- Create: `benchmarks/are-we-fast-yet/java/DeltaBlue.java`, `deltablue/*.java`,
  `Havlak.java`, `havlak/*.java`, `CD.java`, `cd/*.java`
- Modify: `benchmarks/are-we-fast-yet/java/AreWeFastYet.java` (registry)

**Interfaces:**
- Consumes: `Benchmark`, the registry, `som/*` (Task 1).

- [ ] **Step 1: Run the three trials to see them fail**

Run: `cargo nextest run -p ns-cli --profile benchmarks-verify -E "test(/^java(-timed)?::(DeltaBlue|Havlak|CD)$/)"`
Expected: FAIL, `Unknown benchmark`.

- [ ] **Step 2: Adapt the three from upstream, at the ports' sizes**

- DeltaBlue: 12000, the port's checks.
- Havlak: 1500, expects 6102 loops in 5213 nodes, with the port's nesting
  (its header says how it derives the nested loops).
- CD: 250, expects 10830 collisions.

`havlak/*` keeps Google's Apache 2.0 notice. CD's files keep their notices
as upstream has them.

- [ ] **Step 3: Run them to see them pass**

Same command as Step 1. Expected: PASS, 6 tests.

- [ ] **Step 4: Gates and commit**

`cargo xtask gates commit`, then
`git commit -m "feat: Java counterparts of DeltaBlue, Havlak and CD"`.

### Task 4: RichardsTon80 and DeltaBlueTon80, translated from the ports

**Files:**
- Create: `benchmarks/are-we-fast-yet/java/RichardsTon80.java`,
  `richardston80/*.java`, `DeltaBlueTon80.java`, `deltablueton80/*.java`
- Modify: `benchmarks/are-we-fast-yet/java/AreWeFastYet.java` (registry)

**Interfaces:**
- Consumes: `Benchmark`, the registry (Task 1). No `som/*` unless the port
  uses the platform's `List` or `Map`. Then use `som.Vector` /
  `som.Dictionary`, the counterparts the Are We Fast Yet rules prescribe.

- [ ] **Step 1: Run the two trials to see them fail**

Run: `cargo nextest run -p ns-cli --profile benchmarks-verify -E "test(/^java(-timed)?::(RichardsTon80|DeltaBlueTon80)$/)"`
Expected: FAIL, `Unknown benchmark`.

- [ ] **Step 2: Translate each port class by class, method by method**

- One Java class per Newspeak class; a nested class becomes a top-level
  class of the package.
- Each method keeps its port's name in camelCase: `addConstraint:` becomes
  `addConstraint`, `insert:after:` becomes `insertAfter`.
- Sizes 100 and 100, and checks with the port's messages.
- A value the `.ns` code lets grow past 2^31 is a `long`.
- Each header says: "Translated for ns-next from `RichardsTon80.ns`
  (`DeltaBlueTon80.ns`), itself adapted from Primordial Soup's
  `Richards.ns` (`DeltaBlue.ns`), commit
  `338f0bb342a618f5608ab72ab1b1fae9b226e472`." Then it carries the
  Apache 2.0 notice the port carries.

- [ ] **Step 3: Run them to see them pass, then the whole Java side**

Run the Step 1 command; expected PASS, 4 tests. Then
`cargo nextest run -p ns-cli --profile benchmarks-verify -E "test(/^java/)"`:
expected PASS, 35 tests (16 × 2 + 3).

- [ ] **Step 4: Gates and commit**

`cargo xtask gates commit`, then
`git commit -m "feat: Java translations of RichardsTon80 and DeltaBlueTon80"`.

### Task 5: `--warm-up` and `--measure` in both harnesses

**Files:**
- Modify: `benchmarks/are-we-fast-yet/AreWeFastYet.ns`
- Modify: `benchmarks/are-we-fast-yet/java/AreWeFastYet.java`
- Modify: `crates/ns-cli/tests/run_benchmarks.rs`

**Interfaces:**
- Consumes: `java_harness`, `score_is_shaped` (Task 1).
- Command line on both sides:
  `[--timed [--warm-up <milliseconds>] [--measure <milliseconds>]] [Benchmark]`.
  The flags are optional and come in either order, each at most once,
  only after `--timed`. Defaults are 3 and 20. A value is a non-negative
  decimal integer.
- Usage text, both sides, verbatim except the command:
  `usage: ns-next run benchmarks/are-we-fast-yet -- [--timed [--warm-up <milliseconds>] [--measure <milliseconds>]] [Benchmark]`
  and `usage: java AreWeFastYet [--timed [--warm-up <milliseconds>] [--measure <milliseconds>]] [Benchmark]`.

- [ ] **Step 1: Write the failing trials**

For `side` in `ns-next` and `java`, each running its harness:
- `flags::<side>::windows-print-a-score`: `--timed --warm-up 0 --measure 1 Sieve`
  succeeds and `score_is_shaped(stdout, "Sieve")`.
- `flags::<side>::either-order`: `--timed --measure 1 --warm-up 0 Sieve`,
  same.
- `flags::<side>::rejected`: each of `["--warm-up", "5"]`,
  `["--timed", "--measure", "x"]`, `["--timed", "--measure", "-1"]`,
  `["--timed", "--warm-up", "1", "--warm-up", "1"]`, `["--timed", "--measure"]`
  exits non-zero, stdout empty, stderr contains `usage:`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --profile benchmarks-verify -E "test(/^flags::/)"`
Expected: FAIL, the flags are unknown (usage error on the score cases).

- [ ] **Step 3: Implement the flags**

- **`AreWeFastYet.ns`:**
  - `main:args:` walks the arguments after `--timed`. A value is read
    with `Integer parse:`, inside `on: ArgumentError do:`, so an
    ArgumentError goes to `usage`. Reach `Integer` and `ArgumentError`
    the way the module already reaches `Error`.
  - A negative value, a flag without its value, or a repeated flag also
    goes to `usage`.
  - `report:timed:` becomes `report: benchmark timed: timed warmUp: warmUpNanoseconds measure: measureNanoseconds`,
    which passes them to `measure:forAtLeast:`.
  - The header comment and the `usage` text name the flags.
  - Run `cargo run -p ns-analyzer -- fmt benchmarks/are-we-fast-yet/AreWeFastYet.ns`
    afterwards.
- **`AreWeFastYet.java`:** the same rules, with
  `Long.parseLong` + a check for a non-negative value;
  `NumberFormatException` goes to the usage `IllegalArgumentException`.

- [ ] **Step 4: Run them to see them pass, and the pinned run unchanged**

Run: `cargo nextest run -p ns-cli --profile benchmarks-verify`
Expected: PASS, all of it: the ns-next runs, `timed::`, `java::`,
`java-timed::` and `flags::`. The `.toml` is unchanged.

- [ ] **Step 5: Gates and commit**

`cargo xtask gates commit`, then
`git commit -m "feat: --warm-up and --measure in both benchmark harnesses"`.

### Task 6: Documentation and the end of the batch

**Files:**
- Modify: `docs/design/execution.md` ("The Are We Fast Yet benchmarks")
- Modify: `CLAUDE.md` (the `benchmarks/are-we-fast-yet/` paragraph; the
  `benchmarks` gate row; the closing sentence on code copied from
  `prior-art/`)
- Modify: `docs/PROGRESS.md` (new last entry), `docs/OPEN-WORK.md`
- Delete: `docs/superpowers/specs/2026-10-22-java-benchmarks-design.md`

- [ ] **Step 1: Move the spec into the living documents**

`execution.md` gets a paragraph on the Java counterparts in its "The Are
We Fast Yet benchmarks" section, covering:
- the decisions from the spec: same work, upstream's Java adapted, the
  Ton80 translations, no microbenchmarks;
- the layout, what was changed, and the licences by group;
- the shared protocol, the two flags and their defaults;
- the gate's trials and how the JDK is found;
- the peak-comparison use, for example `--timed --warm-up 10000`, the
  same on both sides.

`CLAUDE.md`:
- name the Java counterparts beside the `.ns` programs;
- the `benchmarks` row adds "and their Java counterparts; needs a JDK
  (`JAVA_HOME` or `PATH`)";
- the licensing sentence adds Are We Fast Yet
  (`benchmarks/are-we-fast-yet/java/`).

`PROGRESS.md`: an entry "Java counterparts of the Are We Fast Yet
benchmarks (2026-10-22)" in the shape of its neighbours. `OPEN-WORK.md`:
remove an item this closes, if any (search for "Java"). Delete the spec.

- [ ] **Step 2: Commit**

Documentation only. Then:

```bash
git rm docs/superpowers/specs/2026-10-22-java-benchmarks-design.md
git add docs CLAUDE.md
git commit -m "docs: the Java counterparts of the Are We Fast Yet benchmarks"
```

- [ ] **Step 3: End of the batch**

Run `cargo xtask gates end-of-batch`; expected green, the Java trials
included in `benchmarks`. Then follow the `finishing-a-batch` skill.

//! The repository's own tasks, run as `cargo xtask <task>` (the alias is in
//! `.cargo/config.toml`). Its main task is `gates`: each group of
//! `CLAUDE.md`'s table "The gates" is a group here, so that the table says
//! when a group runs and this says what it runs. The other tasks are
//! `shipped-code`, which writes the shipped modules' compiled code that the
//! gates build and test against, and `codegen`, which holds the hot
//! functions' machine code to its budgets (`codegen.rs`) in this checkout
//! or in another, given as `<tree>`, so that an older commit exported
//! there can be checked against the same budgets. Written in Rust rather than
//! a shell script so that it runs alike on every platform and can grow the
//! logic a script would make fragile -- choosing the groups from what a
//! change touched, for one.

#![forbid(unsafe_code)]

mod codegen;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

const USAGE: &str = "\
usage: cargo xtask gates <group>...
       cargo xtask shipped-code [--check]
       cargo xtask codegen [<tree>]

groups (CLAUDE.md, \"The gates\", says when each runs):
  commit               fmt, clippy, the suite, the doctests, parse-corpus, the plugin's check
  analyzer             the hints' behaviour oracle (profile hints)
  images               every golden through build then load (profile images)
  caches               every golden with a two-entry global cache (profile caches)
  memory               ns-memory's clippy for Linux and macOS
  benchmarks           the benchmark programs verified at full size (profile benchmarks-verify)
  stress               the suite under the nine stress modes and without the JIT, then run-corpus plain and under each mode
  release-checks       the suite without debug assertions (cargo profile release-checks), ns-runtime's clippy for aarch64-pc-windows-msvc, a target without a JIT, and for aarch64-apple-darwin, Apple's JIT
  codegen              the hot path's machine code against its budgets (cargo asm, release)
  plugin-verification  the plugin against several IDEs (verifyPlugin buildPlugin)
  linux                in WSL: clippy, the suite and the doctests for x86_64-unknown-linux-gnu
  aarch64              in WSL: clippy for aarch64-unknown-linux-gnu, and the suite under qemu-aarch64
  end-of-batch         images, caches, benchmarks, release-checks, codegen, stress, linux and aarch64, for a batch that changed the runtime";

/// The stress switches the runtime reads in a debug build
/// (`docs/design/README.md`, "Gates").
const STRESS_MODES: [&str; 9] = [
    "MOVES", "CACHES", "COLLECT", "SCAVENGE", "WIDE", "SUSPEND", "JIT", "EXIT", "ZONE",
];

/// The suite run beside the stress modes with the JIT off: every other
/// run compiles bodies where the JIT is built, so without this one no
/// gate would run the interpreter alone. Its log's name and its variable.
const WITHOUT_JIT: (&str, &str) = ("NO_JIT", "NS_NEXT_NO_JIT");

/// The stress group's runs of the suite: each log's name, `stress-<name>.log`,
/// and the variable it sets to `1`.
fn stress_suite_runs() -> Vec<(String, String)> {
    let mut runs: Vec<(String, String)> = STRESS_MODES
        .iter()
        .map(|mode| (mode.to_string(), format!("NS_NEXT_STRESS_{mode}")))
        .collect();
    runs.push((WITHOUT_JIT.0.to_string(), WITHOUT_JIT.1.to_string()));
    runs
}

/// Where `ns-runtime`'s `build.rs` looks for the shipped modules' compiled
/// mixins, relative to the repository root.
const SHIPPED_CODE: &str = "crates/ns-runtime/core-library/shipped-code.bin";

/// The first step of each WSL group: the working tree copied into the WSL
/// directory, so that uncommitted work is checked too. `.git` is copied,
/// since a test lists the tracked sources with it; the directory keeps its
/// own build directory, and `prior-art/` and `.temp/` hold nothing the
/// gates build. The source is the checkout the gate runs from, so a
/// gate run from another checkout tests that one.
fn copy_into_wsl() -> String {
    // Forward slashes survive the quoting of `wsl -e bash -lc` where
    // backslashes would depend on how the arguments are re-split;
    // `wslpath -u` takes either spelling.
    let root = repository_root().to_string_lossy().replace('\\', "/");
    format!(
        "mkdir -p {WSL_CLONE} && rsync -a --delete --exclude target --exclude prior-art \
         --exclude .temp \"$(wslpath -u '{root}')/\" {WSL_CLONE}/"
    )
}

/// The directory the groups `linux` and `aarch64` copy the checkout into and
/// run in: the gates' own, so that `rsync --delete` never touches a clone
/// of the user's.
const WSL_CLONE: &str = "~/.cache/ns-next-gates";

/// What the `aarch64` group's cargo runs with: the cross linker, the
/// target's libraries for `qemu-aarch64`, which binfmt hands every AArch64
/// executable to -- the test binaries and the `ns-next` the `ns-cli` tests
/// spawn -- and a test thread's stack as large as a main thread's, since
/// an emulated test's deep recursion needs more than the default.
const AARCH64_ENVIRONMENT: &str = "export \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    QEMU_LD_PREFIX=/usr/aarch64-linux-gnu RUST_MIN_STACK=8388608";

/// What runs a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Program {
    Cargo,
    /// The plugin's Gradle wrapper, `editors/jetbrains/gradlew`.
    Gradle,
    /// A bash script in the default WSL distribution, `wsl -e bash -lc`.
    Wsl,
}

/// One command of a gate.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    label: String,
    program: Program,
    arguments: Vec<String>,
    /// Variables set for this command only.
    environment: Vec<(String, String)>,
}

impl Step {
    fn cargo(label: &str, arguments: &[&str]) -> Step {
        Step {
            label: label.to_string(),
            program: Program::Cargo,
            arguments: arguments
                .iter()
                .map(|argument| argument.to_string())
                .collect(),
            environment: Vec::new(),
        }
    }

    fn gradle(label: &str, arguments: &[&str]) -> Step {
        Step {
            program: Program::Gradle,
            ..Step::cargo(label, arguments)
        }
    }

    fn wsl(label: &str, script: &str) -> Step {
        Step {
            program: Program::Wsl,
            ..Step::cargo(label, &["-e", "bash", "-lc", script])
        }
    }

    fn with_environment(mut self, name: &str, value: &str) -> Step {
        self.environment.push((name.to_string(), value.to_string()));
        self
    }
}

/// One part of a group: steps run one after another, or the stress runs of
/// the suite, run at once.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Steps(Vec<Step>),
    /// The suite under every stress mode, and without the JIT, at once
    /// (`stress_suite_runs`): each ends in a long tail of a few depth-limit
    /// tests that leaves most cores idle, and the tails overlap.
    StressSuites,
    /// `cargo xtask shipped-code`. The shipped code's fingerprint covers
    /// Rust sources, so it runs after the one step that rewrites them,
    /// `cargo fmt --all`, or else formatting would leave it stale.
    ShippedCode,
    /// The codegen budgets over this checkout (`codegen.rs`).
    Codegen,
}

/// `cargo xtask shipped-code`, run as a fresh process built in a target
/// directory of its own: the fingerprint is compiled into this binary, so
/// after `cargo fmt --all` rewrote sources only a rebuilt xtask knows the new
/// one, and the running executable cannot be rebuilt in place on Windows. It
/// builds `ns-runtime`, `ns-syntax` and their dependencies a second time in
/// `target/shipped-code` (under the gitignored `/target`), cold on the first
/// run and incrementally again whenever the sources changed.
fn shipped_code_step() -> Step {
    Step::cargo(
        "shipped code",
        &[
            "run",
            "--quiet",
            "--package",
            "xtask",
            "--target-dir",
            "target/shipped-code",
            "--",
            "shipped-code",
        ],
    )
}

/// The parts of `group`, or `None` for a name that is no group.
fn group(name: &str) -> Option<Vec<Part>> {
    let steps = |steps: Vec<Step>| vec![Part::Steps(steps)];
    Some(match name {
        "commit" => vec![
            Part::Steps(vec![Step::cargo("format", &["fmt", "--all"])]),
            Part::ShippedCode,
            Part::Steps(vec![
                Step::cargo(
                    "lint",
                    &[
                        "clippy",
                        "--workspace",
                        "--all-targets",
                        "--",
                        "-D",
                        "warnings",
                    ],
                ),
                Step::cargo("suite", &["nextest", "run", "--workspace"]),
                Step::cargo("doctests", &["test", "--workspace", "--doc"]),
                Step::cargo(
                    "corpus parse",
                    &[
                        "run",
                        "-p",
                        "ns-analyzer",
                        "--",
                        "parse-corpus",
                        "corpus/modules",
                    ],
                ),
                Step::gradle("plugin", &["-p", "editors/jetbrains", "check"]),
            ]),
        ],
        "analyzer" => steps(vec![Step::cargo(
            "hints oracle",
            &["nextest", "run", "-p", "ns-analyzer", "--profile", "hints"],
        )]),
        "images" => steps(vec![Step::cargo(
            "image round trips",
            &["nextest", "run", "-p", "ns-cli", "--profile", "images"],
        )]),
        "caches" => steps(vec![Step::cargo(
            "cache contention",
            &["nextest", "run", "-p", "ns-cli", "--profile", "caches"],
        )]),
        "memory" => steps(
            ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"]
                .iter()
                .map(|target| {
                    Step::cargo(
                        &format!("lint for {target}"),
                        &[
                            "clippy",
                            "-p",
                            "ns-memory",
                            "--all-targets",
                            "--target",
                            target,
                            "--",
                            "-D",
                            "warnings",
                        ],
                    )
                })
                .collect(),
        ),
        "benchmarks" => steps(vec![Step::cargo(
            "benchmark programs",
            &[
                "nextest",
                "run",
                "--workspace",
                "--profile",
                "benchmarks-verify",
            ],
        )]),
        "stress" => {
            let corpus = |label: &str| {
                Step::cargo(
                    label,
                    &["run", "-p", "ns-cli", "--", "run-corpus", "corpus/modules"],
                )
            };
            let mut runs = vec![corpus("run-corpus")];
            runs.extend(STRESS_MODES.iter().map(|mode| {
                corpus(&format!("run-corpus under {mode}"))
                    .with_environment(&format!("NS_NEXT_STRESS_{mode}"), "1")
            }));
            vec![Part::StressSuites, Part::Steps(runs)]
        }
        // The size tests are `cfg(not(debug_assertions))`, so only a build
        // without debug assertions runs them; the profile skips the release
        // profile's fat LTO, which a test run does not need. Beside them,
        // `ns-runtime`'s lint for a target without a JIT, where no
        // `cfg(ns_jit)` code is compiled, and for Apple's JIT, whose code
        // zone no other gate builds.
        "release-checks" => steps(vec![
            Step::cargo(
                "release checks",
                &[
                    "nextest",
                    "run",
                    "--workspace",
                    "--cargo-profile",
                    "release-checks",
                ],
            ),
            runtime_lint("lint without a JIT", "aarch64-pc-windows-msvc"),
            runtime_lint("lint for Apple's JIT", "aarch64-apple-darwin"),
        ]),
        "codegen" => vec![Part::Codegen],
        "plugin-verification" => steps(vec![Step::gradle(
            "plugin verification",
            &["-p", "editors/jetbrains", "verifyPlugin", "buildPlugin"],
        )]),
        "linux" => steps(in_wsl(
            "linux",
            "",
            &[
                ("shipped code", "cargo xtask shipped-code"),
                (
                    "lint",
                    "cargo clippy --workspace --all-targets -- -D warnings",
                ),
                ("suite", "cargo nextest run --workspace --test-threads 8"),
                ("doctests", "cargo test --workspace --doc"),
            ],
        )),
        // No doctests: `cargo test --doc` would build them for the host.
        "aarch64" => steps(in_wsl(
            "aarch64",
            AARCH64_ENVIRONMENT,
            &[
                ("shipped code", "cargo xtask shipped-code"),
                (
                    "lint",
                    "cargo clippy --workspace --all-targets \
                     --target aarch64-unknown-linux-gnu -- -D warnings",
                ),
                (
                    "suite",
                    "cargo nextest run --workspace \
                     --target aarch64-unknown-linux-gnu --test-threads 8",
                ),
            ],
        )),
        "end-of-batch" => [
            "images",
            "caches",
            "benchmarks",
            "release-checks",
            "codegen",
            "stress",
            "linux",
            "aarch64",
        ]
        .iter()
        .flat_map(|name| group(name).expect("a group"))
        .collect(),
        _ => return None,
    })
}

/// `ns-runtime`'s clippy for `target`, warning-free.
fn runtime_lint(label: &str, target: &str) -> Step {
    Step::cargo(
        label,
        &[
            "clippy",
            "-p",
            "ns-runtime",
            "--all-targets",
            "--target",
            target,
            "--",
            "-D",
            "warnings",
        ],
    )
}

/// A WSL group: the tree copied into the clone, then each of `commands`
/// run there after `environment`, each step labelled `<name> <label>`.
fn in_wsl(name: &str, environment: &str, commands: &[(&str, &str)]) -> Vec<Step> {
    let prefix = if environment.is_empty() {
        String::new()
    } else {
        format!("{environment} && ")
    };
    let mut steps = vec![Step::wsl(&format!("{name} copy"), &copy_into_wsl())];
    steps.extend(commands.iter().map(|(label, command)| {
        Step::wsl(
            &format!("{name} {label}"),
            &format!("{prefix}cd {WSL_CLONE} && {command}"),
        )
    }));
    steps
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some((task, groups)) = arguments.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if task == "shipped-code" {
        return match groups {
            [] => shipped_code(&repository_root(), false),
            [flag] if flag == "--check" => shipped_code(&repository_root(), true),
            _ => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        };
    }
    if task == "codegen" {
        let tree = match groups {
            [] => repository_root(),
            [tree] => PathBuf::from(tree),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        };
        return match codegen::check(&tree) {
            Ok(()) => ExitCode::SUCCESS,
            Err(failure) => {
                eprintln!("\nFAILED: {failure}");
                ExitCode::FAILURE
            }
        };
    }
    if task != "gates" || groups.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let mut parts = Vec::new();
    for name in groups {
        match group(name) {
            Some(group_parts) => parts.extend(group_parts),
            None => {
                eprintln!("no group `{name}`\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    place_shipped_code(&mut parts);
    let root = repository_root();
    let mut times = Vec::new();
    for part in &parts {
        let outcome = match part {
            Part::ShippedCode => run(&shipped_code_step(), &root, &mut times),
            Part::Steps(steps) => steps
                .iter()
                .try_for_each(|step| run(step, &root, &mut times)),
            Part::StressSuites => run_stress_suites(&root, &mut times),
            Part::Codegen => run_codegen(&root, &mut times),
        };
        if let Err(failure) = outcome {
            report(&times);
            eprintln!("\nFAILED: {failure}");
            return ExitCode::FAILURE;
        }
    }
    report(&times);
    println!("\nevery gate passed");
    ExitCode::SUCCESS
}

/// The gates build and test against the embedded shipped code, so it runs
/// once per call: right after `cargo fmt --all` when the run includes
/// `commit` (a group listed before `commit` then sees the artifact as it
/// was before formatting), else before the first group.
fn place_shipped_code(parts: &mut Vec<Part>) {
    let mut placed = false;
    parts.retain(|part| *part != Part::ShippedCode || !std::mem::replace(&mut placed, true));
    if !placed {
        parts.insert(0, Part::ShippedCode);
    }
}

/// Writes the shipped modules' compiled mixins where `ns-runtime`'s
/// `build.rs` embeds them; with `check`, only says whether the file there
/// is current.
fn shipped_code(root: &Path, check: bool) -> ExitCode {
    let generated = match ns_runtime::shipped_code::generate() {
        Ok(generated) => generated,
        Err(refusal) => {
            eprintln!("{refusal}");
            return ExitCode::FAILURE;
        }
    };
    let path = root.join(SHIPPED_CODE);
    let current = std::fs::read(&path).is_ok_and(|existing| existing == generated);
    if check {
        if current {
            println!("{SHIPPED_CODE} is current");
            return ExitCode::SUCCESS;
        }
        println!("{SHIPPED_CODE} is missing or stale: run `cargo xtask shipped-code`");
        return ExitCode::FAILURE;
    }
    // Writing only a changed file keeps `build.rs` from rebuilding
    // `ns-runtime` after an unchanged run. Written beside it and renamed, so
    // that a build running meanwhile embeds the old file or the new one,
    // never a half-written one whose valid header would make every boot of
    // that binary panic.
    if !current {
        let temporary = path.with_extension("bin.tmp");
        std::fs::write(&temporary, &generated).expect("the core library directory is writable");
        std::fs::rename(&temporary, &path).expect("the artifact can be replaced");
    }
    println!("{SHIPPED_CODE}: {} bytes", generated.len());
    ExitCode::SUCCESS
}

/// The repository's root, the directory above this crate's.
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lies inside the repository")
        .to_path_buf()
}

/// The cargo that runs this task, so that the gates use the same toolchain.
fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn command_of(step: &Step, root: &Path) -> Command {
    let mut command = match step.program {
        Program::Cargo => Command::new(cargo()),
        Program::Gradle => {
            let wrapper = if cfg!(windows) {
                "gradlew.bat"
            } else {
                "gradlew"
            };
            let mut command = Command::new(root.join("editors/jetbrains").join(wrapper));
            // No daemon outlives a gate run.
            command.arg("--no-daemon");
            command
        }
        Program::Wsl => Command::new("wsl"),
    };
    command.args(&step.arguments).current_dir(root);
    for (name, value) in &step.environment {
        command.env(name, value);
    }
    command
}

/// Runs `step` with its output shown, and records how long it took.
fn run(step: &Step, root: &Path, times: &mut Vec<(String, Duration)>) -> Result<(), String> {
    println!("\n==> {}", step.label);
    let started = Instant::now();
    let status = command_of(step, root)
        .status()
        .map_err(|error| format!("{}: could not start: {error}", step.label))?;
    times.push((step.label.clone(), started.elapsed()));
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} ({status})", step.label))
    }
}

/// The codegen budgets over the checkout at `root`, timed as a step is.
fn run_codegen(root: &Path, times: &mut Vec<(String, Duration)>) -> Result<(), String> {
    println!("\n==> codegen");
    let started = Instant::now();
    let outcome = codegen::check(root);
    times.push(("codegen".to_string(), started.elapsed()));
    outcome
}

/// The suite under each stress mode and without the JIT at once, each
/// into its own log under `.temp/`; then each log's summary line.
fn run_stress_suites(root: &Path, times: &mut Vec<(String, Duration)>) -> Result<(), String> {
    let runs = stress_suite_runs();
    let names: Vec<&str> = runs.iter().map(|(name, _)| name.as_str()).collect();
    println!("\n==> the suite under {}, at once", names.join(", "));
    let logs = root.join(".temp");
    std::fs::create_dir_all(&logs).map_err(|error| format!("create .temp/: {error}"))?;
    let started = Instant::now();
    let mut children = Vec::new();
    for (mode, variable) in &runs {
        let log_path = logs.join(format!("stress-{mode}.log"));
        let log = std::fs::File::create(&log_path)
            .map_err(|error| format!("create {}: {error}", log_path.display()))?;
        let error_log = log
            .try_clone()
            .map_err(|error| format!("{}: {error}", log_path.display()))?;
        let step =
            Step::cargo("", &["nextest", "run", "--workspace"]).with_environment(variable, "1");
        let child = command_of(&step, root)
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(error_log))
            .spawn()
            .map_err(|error| format!("stress {mode}: could not start: {error}"))?;
        children.push((mode, log_path, child));
    }
    let mut failed = Vec::new();
    for (mode, log_path, mut child) in children {
        let status = child
            .wait()
            .map_err(|error| format!("stress {mode}: {error}"))?;
        let summary = std::fs::read_to_string(&log_path)
            .unwrap_or_default()
            .lines()
            .find(|line| line.contains("Summary"))
            .map(|line| line.trim().to_string())
            .unwrap_or_else(|| "no summary".to_string());
        println!("  {mode}: {summary}");
        if !status.success() {
            failed.push(format!("stress {mode}: {}", log_path.display()));
        }
    }
    times.push((
        "the suite under every stress mode".to_string(),
        started.elapsed(),
    ));
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join(", "))
    }
}

fn report(times: &[(String, Duration)]) {
    println!("\n{:>8}  gate", "seconds");
    for (label, duration) in times {
        println!("{:>8.1}  {label}", duration.as_secs_f64());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every group the usage text names exists, and nothing else does.
    #[test]
    fn the_usage_names_every_group() {
        let named: Vec<&str> = USAGE
            .lines()
            .skip_while(|line| !line.starts_with("groups"))
            .skip(1)
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        assert!(!named.is_empty());
        for name in &named {
            assert!(
                group(name).is_some(),
                "the usage names `{name}`, which is no group"
            );
        }
        assert!(group("everything").is_none());
    }

    #[test]
    fn the_end_of_batch_runs_the_runtime_gates_and_the_stress_runs() {
        let parts = group("end-of-batch").expect("a group");
        let labels: Vec<&str> = parts
            .iter()
            .filter_map(|part| match part {
                Part::Steps(steps) => Some(steps.iter().map(|step| step.label.as_str())),
                Part::StressSuites | Part::ShippedCode | Part::Codegen => None,
            })
            .flatten()
            .collect();
        assert_eq!(
            &labels[..4],
            [
                "image round trips",
                "cache contention",
                "benchmark programs",
                "release checks"
            ]
        );
        assert!(parts.contains(&Part::StressSuites));
        assert!(parts.contains(&Part::Codegen));
        assert_eq!(
            labels
                .iter()
                .filter(|label| label.starts_with("run-corpus"))
                .count(),
            1 + STRESS_MODES.len()
        );
    }

    /// The labels of `parts`' steps, `shipped code` for the task.
    fn labels_of(parts: &[Part]) -> Vec<&str> {
        parts
            .iter()
            .flat_map(|part| match part {
                Part::Steps(steps) => steps.iter().map(|step| step.label.as_str()).collect(),
                Part::StressSuites => vec!["stress suites"],
                Part::ShippedCode => vec!["shipped code"],
                Part::Codegen => vec!["codegen"],
            })
            .collect()
    }

    #[test]
    fn the_shipped_code_runs_once_right_after_formatting() {
        let mut parts = group("images").expect("a group");
        parts.extend(group("commit").expect("a group"));
        parts.extend(group("commit").expect("a group"));
        place_shipped_code(&mut parts);
        let labels = labels_of(&parts);
        assert_eq!(
            labels
                .iter()
                .filter(|label| **label == "shipped code")
                .count(),
            1
        );
        assert_eq!(labels[..3], ["image round trips", "format", "shipped code"]);
    }

    #[test]
    fn without_formatting_the_shipped_code_runs_first() {
        let mut parts = group("images").expect("a group");
        parts.extend(group("caches").expect("a group"));
        place_shipped_code(&mut parts);
        assert_eq!(
            labels_of(&parts),
            ["shipped code", "image round trips", "cache contention"]
        );
    }

    /// The codegen budgets are a group of their own, and the end of a batch
    /// runs them after the release checks and before the stress runs.
    #[test]
    fn the_codegen_group_runs_the_budgets() {
        assert_eq!(group("codegen"), Some(vec![Part::Codegen]));
        let labels = labels_of(&group("end-of-batch").expect("a group")).join(",");
        assert!(labels.contains(
            "release checks,lint without a JIT,lint for Apple's JIT,codegen,stress suites"
        ));
    }

    /// The WSL groups run after everything that runs on the host, `aarch64`
    /// last, since the suite under emulation is the longest step of all.
    #[test]
    fn the_end_of_batch_runs_linux_and_aarch64_last() {
        let mut expected = group("stress").expect("a group");
        expected.extend(group("linux").expect("a group"));
        expected.extend(group("aarch64").expect("a group"));
        let parts = group("end-of-batch").expect("a group");
        assert!(parts.ends_with(&expected));
    }

    /// Each WSL group first copies the working tree into the clone, so that
    /// what it checks is the tree on the host, uncommitted work included.
    #[test]
    fn the_wsl_groups_copy_the_tree_first() {
        for name in ["linux", "aarch64"] {
            let parts = group(name).expect("a group");
            let [Part::Steps(steps)] = parts.as_slice() else {
                panic!("{name} is one run of steps");
            };
            assert_eq!(steps[0].program, Program::Wsl);
            assert_eq!(
                steps[0].arguments,
                ["-e", "bash", "-lc", copy_into_wsl().as_str()]
            );
            assert!(steps[1..].iter().all(|step| step.program == Program::Wsl
                && step.arguments[3].contains("cd ~/.cache/ns-next-gates && cargo")));
        }
    }

    /// The `aarch64` suite runs for its target, through the cross linker
    /// and the emulator's library prefix.
    #[test]
    fn the_aarch64_suite_runs_for_its_target() {
        let parts = group("aarch64").expect("a group");
        let [Part::Steps(steps)] = parts.as_slice() else {
            panic!("aarch64 is one run of steps");
        };
        let suite = &steps.last().expect("a step").arguments[3];
        assert!(suite.contains("--target aarch64-unknown-linux-gnu"));
        for variable in [
            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc",
            "QEMU_LD_PREFIX=/usr/aarch64-linux-gnu",
            "RUST_MIN_STACK=8388608",
        ] {
            assert!(suite.contains(variable), "{variable}");
        }
    }

    #[test]
    fn the_stress_group_runs_nine_modes() {
        assert_eq!(STRESS_MODES.len(), 9);
        for mode in ["JIT", "EXIT", "ZONE"] {
            assert!(STRESS_MODES.contains(&mode), "no stress mode {mode}");
        }
    }

    #[test]
    fn the_stress_group_also_runs_the_suite_without_the_jit() {
        let runs = stress_suite_runs();
        assert_eq!(runs.len(), STRESS_MODES.len() + 1);
        assert!(runs.contains(&("NO_JIT".to_string(), "NS_NEXT_NO_JIT".to_string())));
        assert!(runs.contains(&("JIT".to_string(), "NS_NEXT_STRESS_JIT".to_string())));
        let names: std::collections::HashSet<&str> =
            runs.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names.len(), runs.len(), "every run has a log of its own");
    }

    #[test]
    fn a_stress_run_of_the_corpus_sets_its_switch() {
        let parts = group("stress").expect("a group");
        let Part::Steps(runs) = &parts[1] else {
            panic!("the corpus runs follow the suites");
        };
        assert!(runs[0].environment.is_empty(), "the first run is plain");
        assert_eq!(
            runs[1].environment,
            [("NS_NEXT_STRESS_MOVES".to_string(), "1".to_string())]
        );
        assert_eq!(runs.len(), 1 + STRESS_MODES.len());
        for mode in ["SUSPEND", "JIT", "EXIT"] {
            assert!(
                runs.iter()
                    .any(|run| run.environment
                        == [(format!("NS_NEXT_STRESS_{mode}"), "1".to_string())]),
                "the corpus also runs under the {mode} stress mode"
            );
        }
    }
}

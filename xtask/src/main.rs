//! The repository's own tasks, run as `cargo xtask <task>` (the alias is in
//! `.cargo/config.toml`). Its main task is `gates`: each group of
//! `CLAUDE.md`'s table "The gates" is a group here, so that the table says
//! when a group runs and this says what it runs. The other task is
//! `shipped-code`, which writes the shipped modules' compiled code that the
//! gates build and test against. Written in Rust rather than
//! a shell script so that it runs alike on every platform and can grow the
//! logic a script would make fragile -- choosing the groups from what a
//! change touched, for one.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

const USAGE: &str = "\
usage: cargo xtask gates <group>...
       cargo xtask shipped-code [--check]

groups (CLAUDE.md, \"The gates\", says when each runs):
  commit               fmt, clippy, the suite, the doctests, parse-corpus, the plugin's check
  analyzer             the hints' behaviour oracle (profile hints)
  images               every golden through build then load (profile images)
  caches               every golden with a two-entry global cache (profile caches)
  memory               ns-memory's clippy for Linux and macOS
  benchmarks           the benchmark programs verified at full size (profile benchmarks-verify)
  stress               the five stress modes over the suite, then run-corpus plain and under each
  plugin-verification  the plugin against several IDEs (verifyPlugin buildPlugin)
  end-of-batch         images, caches, benchmarks and stress, for a batch that changed the runtime";

/// The stress switches the runtime reads in a debug build
/// (`docs/design/README.md`, "Gates").
const STRESS_MODES: [&str; 5] = ["MOVES", "CACHES", "COLLECT", "SCAVENGE", "WIDE"];

/// Where `ns-runtime`'s `build.rs` looks for the shipped modules' compiled
/// mixins, relative to the repository root.
const SHIPPED_CODE: &str = "crates/ns-runtime/core-library/shipped-code.bin";

/// What runs a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Program {
    Cargo,
    /// The plugin's Gradle wrapper, `editors/jetbrains/gradlew`.
    Gradle,
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
    /// The suite under every stress mode at once: each ends in a long tail
    /// of a few depth-limit tests that leaves most cores idle, and the tails
    /// overlap.
    StressSuites,
    /// `cargo xtask shipped-code`. The shipped code's fingerprint covers
    /// Rust sources, so it runs after the one step that rewrites them,
    /// `cargo fmt --all`, or else formatting would leave it stale.
    ShippedCode,
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
        "plugin-verification" => steps(vec![Step::gradle(
            "plugin verification",
            &["-p", "editors/jetbrains", "verifyPlugin", "buildPlugin"],
        )]),
        "end-of-batch" => ["images", "caches", "benchmarks", "stress"]
            .iter()
            .flat_map(|name| group(name).expect("a group"))
            .collect(),
        _ => return None,
    })
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
            Command::new(root.join("editors/jetbrains").join(wrapper))
        }
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

/// The suite under each stress mode at once, each into its own log under
/// `.temp/`; then each log's summary line.
fn run_stress_suites(root: &Path, times: &mut Vec<(String, Duration)>) -> Result<(), String> {
    println!("\n==> the suite under {}, at once", STRESS_MODES.join(", "));
    let logs = root.join(".temp");
    std::fs::create_dir_all(&logs).map_err(|error| format!("create .temp/: {error}"))?;
    let started = Instant::now();
    let mut children = Vec::new();
    for mode in STRESS_MODES {
        let log_path = logs.join(format!("stress-{mode}.log"));
        let log = std::fs::File::create(&log_path)
            .map_err(|error| format!("create {}: {error}", log_path.display()))?;
        let error_log = log
            .try_clone()
            .map_err(|error| format!("{}: {error}", log_path.display()))?;
        let step = Step::cargo("", &["nextest", "run", "--workspace"])
            .with_environment(&format!("NS_NEXT_STRESS_{mode}"), "1");
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
                Part::StressSuites | Part::ShippedCode => None,
            })
            .flatten()
            .collect();
        assert_eq!(
            &labels[..3],
            [
                "image round trips",
                "cache contention",
                "benchmark programs"
            ]
        );
        assert!(parts.contains(&Part::StressSuites));
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
    }
}

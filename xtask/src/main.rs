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
  stress               the suite under the twelve stress modes and without the JIT, then run-corpus plain and under each mode
  release-checks       the suite without debug assertions (cargo profile release-checks), ns-runtime's clippy for aarch64-pc-windows-msvc, a target without a JIT, and for aarch64-apple-darwin, Apple's JIT
  codegen              the hot path's machine code against its budgets (cargo asm, release)
  plugin-verification  the plugin against several IDEs (verifyPlugin buildPlugin)
  linux                in Docker: clippy, the suite, the doctests and the benchmark programs for x86_64-unknown-linux-gnu
  aarch64              in Docker: clippy for aarch64-unknown-linux-gnu, and the suite under the image's qemu-aarch64
  end-of-batch         images, caches, benchmarks, release-checks, codegen, stress, linux and aarch64, for a batch that changed the runtime";

/// The stress switches the runtime reads in a debug build
/// (`docs/design/README.md`, "Gates").
const STRESS_MODES: [&str; 12] = [
    "MOVES",
    "CACHES",
    "COLLECT",
    "SCAVENGE",
    "WIDE",
    "SUSPEND",
    "JIT",
    "EXIT",
    "ZONE",
    "OPTIMIZE",
    "DEOPT",
    "INVALIDATE",
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

/// The image the groups `linux` and `aarch64` run in, built from
/// `DOCKER_CONTEXT`'s `Dockerfile`, so that the environment they check
/// against is the repository's code.
const DOCKER_IMAGE: &str = "ns-next-gates";

/// The Dockerfile's directory, relative to the repository root: the build
/// context holds nothing else, so a build sends Docker no sources.
const DOCKER_CONTEXT: &str = "xtask/docker";

/// Where a container sees the checkout the gate runs from, read-only.
const CHECKOUT_MOUNT: &str = "/checkout";

/// The platform the image is built for and its containers run on: x86_64,
/// so that rustc runs natively and only the AArch64 binaries are emulated,
/// and so that an arm64 Docker host builds and runs the image the
/// Dockerfile describes -- emulated -- or fails clearly, rather than
/// building an arm64 image the Dockerfile does not describe.
const DOCKER_PLATFORM: &str = "linux/amd64";

/// Where a container sees the working copy it builds in.
const WORKING_COPY: &str = "/work";

/// The named volumes a container mounts, by the suffix of their name and
/// where it sees them: the working copy, so that it and the build directory
/// outlive the container and a second run builds incrementally, and the
/// build never writes into the checkout; its build directory, a volume of
/// its own, which the copy leaves alone; and cargo's registry, so that a
/// run downloads no crate an earlier one did. Each checkout has volumes of
/// its own (`volume_name`).
const VOLUMES: [(&str, &str); 3] = [
    ("work", WORKING_COPY),
    ("target", "/work/target"),
    ("registry", "/usr/local/cargo/registry"),
];

/// The name of a checkout's volume `suffix`, `ns-next-gates-<id>-<suffix>`,
/// the id a hash of the checkout's path, so that two checkouts' gates, even
/// run at once, never share a working copy, a build directory or the
/// registry, which cargo locks only within one container. FNV-1a, written
/// out, so that a checkout keeps its volumes across toolchains.
fn volume_name(root: &Path, suffix: &str) -> String {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let hash = root
        .to_string_lossy()
        .bytes()
        .fold(OFFSET_BASIS, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(PRIME)
        });
    format!("ns-next-gates-{:012x}-{suffix}", hash >> 16)
}

/// Where a container sees a linked worktree's git metadata, read-only.
const GIT_MOUNT: &str = "/checkout-git";

/// A linked worktree's git metadata: the repository's common directory,
/// and the worktree's own directory relative to it. A linked worktree's
/// `.git` is a file naming that directory by a path on the host, which the
/// copy takes along but a container cannot follow, so the common directory
/// is mounted and git pointed at it (`git_arguments`). `None` for a
/// checkout whose `.git` is the directory itself, which the copy takes
/// along.
fn linked_worktree_git(root: &Path) -> Option<(PathBuf, PathBuf)> {
    if !root.join(".git").is_file() {
        return None;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ])
        .output()
        .expect("a linked worktree's gates run git to find its metadata");
    assert!(
        output.status.success(),
        "git rev-parse failed in the linked worktree {}: {}",
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut lines = text.lines().map(PathBuf::from);
    let (Some(git_directory), Some(common_directory)) = (lines.next(), lines.next()) else {
        panic!(
            "git rev-parse named no git directory for {}",
            root.display()
        );
    };
    let relative = git_directory
        .strip_prefix(&common_directory)
        .unwrap_or_else(|_| {
            panic!(
                "the worktree's git directory {} lies outside the repository's {}",
                git_directory.display(),
                common_directory.display()
            )
        })
        .to_path_buf();
    Some((common_directory, relative))
}

/// The `docker run` arguments that give a linked worktree's container its
/// git metadata: the repository's common directory mounted read-only, and
/// `GIT_DIR` and `GIT_WORK_TREE` naming the worktree's directory in it and
/// the working copy, which is all `git ls-files` reads. None for a
/// checkout whose `.git` is a directory.
fn git_arguments(worktree: Option<(PathBuf, PathBuf)>) -> Vec<String> {
    let Some((common_directory, relative)) = worktree else {
        return Vec::new();
    };
    let relative: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    vec![
        "--mount".to_string(),
        format!(
            "type=bind,source={},target={GIT_MOUNT},readonly",
            common_directory.display()
        ),
        "--env".to_string(),
        format!("GIT_DIR={GIT_MOUNT}/{}", relative.join("/")),
        "--env".to_string(),
        format!("GIT_WORK_TREE={WORKING_COPY}"),
    ]
}

/// The first step of each Docker group after the image: the checkout
/// copied into the working copy, so that uncommitted work is checked too.
/// `.git` is copied, since a test lists the tracked sources with it (a
/// linked worktree's is mounted instead, `linked_worktree_git`). What
/// a `.gitignore` names is not -- the build directories, among them the
/// plugin's Gradle caches of nearly two gigabytes, and `.temp/` -- nor is
/// `prior-art/`, which holds nothing the gates build; an excluded file is
/// also kept from `--delete`, so the working copy's own build directory and
/// shipped code survive. `/target` is excluded by name as well: on a fresh
/// volume the working copy has no `.gitignore` yet when `--delete` runs,
/// and `target` is the mount point of the build directory's volume, which
/// rsync would try to remove.
fn copy_into_working_copy() -> String {
    format!(
        "rsync -a --delete --filter=':- .gitignore' --exclude /target --exclude /prior-art \
         {CHECKOUT_MOUNT}/ {WORKING_COPY}/"
    )
}

/// What the `aarch64` group's cargo runs with: the cross linker; the
/// image's own `qemu-aarch64` as the target's runner, which cargo and
/// nextest start every test binary through, the tests every `ns-next`
/// and `ns-next-analyzer` they start, and `run-corpus` its children of
/// itself (`ns-cli`'s `src/target_runner.rs`), so that what runs the
/// machine code is the QEMU the image pins, never a binfmt handler of the
/// engine's; the target's libraries for it; and a test
/// thread's stack as large as a main thread's, since an emulated test's
/// deep recursion needs more than the default.
const AARCH64_ENVIRONMENT: &str = "export \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER=qemu-aarch64 \
    QEMU_LD_PREFIX=/usr/aarch64-linux-gnu RUST_MIN_STACK=8388608";

/// What runs a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Program {
    Cargo,
    /// The plugin's Gradle wrapper, `editors/jetbrains/gradlew`.
    Gradle,
    /// `docker`: the gate image's build, or a bash script in a container
    /// of it.
    Docker,
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

    /// The gate image built, or taken from Docker's cache when the
    /// Dockerfile is unchanged.
    fn docker_image(label: &str) -> Step {
        Step {
            program: Program::Docker,
            ..Step::cargo(
                label,
                &[
                    "build",
                    "--platform",
                    DOCKER_PLATFORM,
                    "--tag",
                    DOCKER_IMAGE,
                    DOCKER_CONTEXT,
                ],
            )
        }
    }

    /// `script` run by bash in a fresh x86_64 container of the gate image,
    /// the checkout the gate runs from mounted read-only beside its
    /// volumes. The container's exit status is the script's, so a failing
    /// command fails the step.
    fn docker(label: &str, script: &str) -> Step {
        let root = repository_root();
        let checkout = format!(
            "type=bind,source={},target={CHECKOUT_MOUNT},readonly",
            root.display()
        );
        let mut arguments: Vec<String> = ["run", "--rm", "--platform", DOCKER_PLATFORM]
            .iter()
            .map(|argument| argument.to_string())
            .collect();
        arguments.extend(["--mount".to_string(), checkout]);
        for (suffix, target) in VOLUMES {
            arguments.extend([
                "--mount".to_string(),
                format!(
                    "type=volume,source={},target={target}",
                    volume_name(&root, suffix)
                ),
            ]);
        }
        arguments.extend(git_arguments(linked_worktree_git(&root)));
        arguments.extend(
            [DOCKER_IMAGE, "bash", "-c", script]
                .iter()
                .map(|argument| argument.to_string()),
        );
        Step {
            label: label.to_string(),
            program: Program::Docker,
            arguments,
            environment: Vec::new(),
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
        "linux" => steps(in_docker(
            "linux",
            "",
            &[
                ("shipped code", "cargo xtask shipped-code"),
                (
                    "lint",
                    "cargo clippy --workspace --all-targets -- -D warnings",
                ),
                (
                    "suite",
                    "cargo nextest run --workspace --test-threads $(nproc)",
                ),
                ("doctests", "cargo test --workspace --doc"),
                (
                    "benchmark programs",
                    "cargo nextest run --workspace --profile benchmarks-verify",
                ),
            ],
        )),
        // No doctests: `cargo test --doc` would build them for the host.
        "aarch64" => steps(in_docker(
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
                     --target aarch64-unknown-linux-gnu --test-threads $(nproc)",
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

/// A Docker group: the image, the checkout copied into the working copy,
/// then each of `commands` run there after `environment`, each step
/// labelled `<name> <label>`. The suites take as many test threads as the
/// container has CPUs, where the profiles' fixed count is the host's.
fn in_docker(name: &str, environment: &str, commands: &[(&str, &str)]) -> Vec<Step> {
    let prefix = if environment.is_empty() {
        String::new()
    } else {
        format!("{environment} && ")
    };
    let mut steps = vec![
        Step::docker_image(&format!("{name} image")),
        Step::docker(&format!("{name} copy"), &copy_into_working_copy()),
    ];
    steps.extend(commands.iter().map(|(label, command)| {
        Step::docker(&format!("{name} {label}"), &format!("{prefix}{command}"))
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
        Program::Docker => Command::new("docker"),
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

    /// The Docker groups run after everything that runs on the host,
    /// `aarch64` last, since the suite under emulation is the longest step
    /// of all.
    #[test]
    fn the_end_of_batch_runs_linux_and_aarch64_last() {
        let mut expected = group("stress").expect("a group");
        expected.extend(group("linux").expect("a group"));
        expected.extend(group("aarch64").expect("a group"));
        let parts = group("end-of-batch").expect("a group");
        assert!(parts.ends_with(&expected));
    }

    /// The script a Docker step runs, its last argument.
    fn script_of(step: &Step) -> &str {
        step.arguments.last().expect("a script")
    }

    /// Each Docker group first builds the image, then copies the working
    /// tree into the working copy, so that what it checks is the tree on
    /// the host, uncommitted work included; every later step runs in a
    /// container of the image, the checkout read-only beside the volumes.
    #[test]
    fn the_docker_groups_build_the_image_and_copy_the_tree_first() {
        for name in ["linux", "aarch64"] {
            let parts = group(name).expect("a group");
            let [Part::Steps(steps)] = parts.as_slice() else {
                panic!("{name} is one run of steps");
            };
            assert_eq!(steps[0].program, Program::Docker);
            assert_eq!(
                steps[0].arguments,
                [
                    "build",
                    "--platform",
                    "linux/amd64",
                    "--tag",
                    "ns-next-gates",
                    "xtask/docker"
                ]
            );
            assert_eq!(script_of(&steps[1]), copy_into_working_copy());
            for step in &steps[1..] {
                assert_eq!(step.program, Program::Docker);
                assert_eq!(
                    step.arguments[..4],
                    ["run", "--rm", "--platform", "linux/amd64"]
                );
                let root = repository_root();
                let mounts = [
                    format!(
                        "type=bind,source={},target=/checkout,readonly",
                        root.display()
                    ),
                    format!(
                        "type=volume,source={},target=/work",
                        volume_name(&root, "work")
                    ),
                    format!(
                        "type=volume,source={},target=/work/target",
                        volume_name(&root, "target")
                    ),
                    format!(
                        "type=volume,source={},target=/usr/local/cargo/registry",
                        volume_name(&root, "registry")
                    ),
                ];
                for mount in &mounts {
                    assert!(
                        step.arguments
                            .windows(2)
                            .any(|pair| pair == ["--mount", mount]),
                        "{}: {mount}",
                        step.label
                    );
                }
                let length = step.arguments.len();
                assert_eq!(
                    step.arguments[length - 4..length - 1],
                    ["ns-next-gates", "bash", "-c"]
                );
            }
            assert!(steps[2..]
                .iter()
                .all(|step| script_of(step).contains("cargo ")));
        }
    }

    /// Each checkout has volumes of its own, named the same on every run.
    #[test]
    fn a_volume_is_named_after_its_checkout() {
        let first = Path::new("/checkouts/first");
        let name = volume_name(first, "work");
        assert_eq!(name, volume_name(first, "work"));
        assert!(name.starts_with("ns-next-gates-") && name.ends_with("-work"));
        assert_eq!(name.len(), "ns-next-gates-".len() + 12 + "-work".len());
        assert_ne!(name, volume_name(Path::new("/checkouts/second"), "work"));
        assert_ne!(name, volume_name(first, "target"));
    }

    /// A linked worktree's container reads the repository's git metadata
    /// from a mount, through `GIT_DIR`; a checkout whose `.git` is a
    /// directory needs nothing, since the copy takes it along.
    #[test]
    fn a_linked_worktree_mounts_the_repositorys_git_metadata() {
        assert!(git_arguments(None).is_empty());
        let arguments = git_arguments(Some((
            PathBuf::from("/repository/.git"),
            Path::new("worktrees").join("feature"),
        )));
        assert_eq!(
            arguments,
            [
                "--mount",
                "type=bind,source=/repository/.git,target=/checkout-git,readonly",
                "--env",
                "GIT_DIR=/checkout-git/worktrees/feature",
                "--env",
                "GIT_WORK_TREE=/work",
            ]
        );
        if !repository_root().join(".git").is_file() {
            assert_eq!(linked_worktree_git(&repository_root()), None);
        }
    }

    /// The copy keeps the working copy's build directory, and copies `.git`.
    #[test]
    fn the_copy_leaves_the_build_directory_alone() {
        let copy = copy_into_working_copy();
        assert!(copy.starts_with("rsync -a --delete "));
        assert!(copy.contains("--filter=':- .gitignore' "));
        assert!(
            copy.contains("--exclude /target "),
            "the build directory's mount point"
        );
        assert!(!copy.contains("--exclude /.git "));
        assert!(copy.ends_with("/checkout/ /work/"));
    }

    /// `linux` also verifies the benchmark programs, as the host's
    /// `benchmarks` group does, its image having a JDK; `aarch64` does not.
    #[test]
    fn only_linux_runs_the_benchmark_programs() {
        let runs_benchmarks = |name: &str| {
            let parts = group(name).expect("a group");
            let [Part::Steps(steps)] = parts.as_slice() else {
                panic!("{name} is one run of steps");
            };
            steps.iter().any(|step| {
                script_of(step) == "cargo nextest run --workspace --profile benchmarks-verify"
            })
        };
        assert!(runs_benchmarks("linux"));
        assert!(!runs_benchmarks("aarch64"));
    }

    /// The `aarch64` suite runs for its target, through the cross linker
    /// and the emulator's library prefix.
    #[test]
    fn the_aarch64_suite_runs_for_its_target() {
        let parts = group("aarch64").expect("a group");
        let [Part::Steps(steps)] = parts.as_slice() else {
            panic!("aarch64 is one run of steps");
        };
        let suite = script_of(steps.last().expect("a step"));
        assert!(suite.contains("--target aarch64-unknown-linux-gnu"));
        assert!(suite.contains("--test-threads $(nproc)"));
        for variable in [
            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc",
            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER=qemu-aarch64",
            "QEMU_LD_PREFIX=/usr/aarch64-linux-gnu",
            "RUST_MIN_STACK=8388608",
        ] {
            assert!(suite.contains(variable), "{variable}");
        }
    }

    #[test]
    fn the_stress_group_runs_twelve_modes() {
        assert_eq!(STRESS_MODES.len(), 12);
        for mode in ["JIT", "EXIT", "ZONE", "OPTIMIZE", "DEOPT", "INVALIDATE"] {
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

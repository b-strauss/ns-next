//! Starting one of the workspace's binaries as cargo starts a test binary:
//! through the runner of the target it was built for, when one is set.
//! Built for another architecture, a binary runs only through its
//! emulator, and cargo names the emulator in
//! `CARGO_TARGET_<TRIPLE>_RUNNER`. A process that started the binary
//! directly would leave it to whatever the kernel happens to have
//! registered -- on a Docker engine, a binfmt handler of the host's, not
//! the emulator the run chose -- so `ns-next` reads the variable too, for
//! the children `run-corpus` starts of itself; where it is unset, every
//! ordinary run, a child is started directly. Both crates' tests include
//! this file (`tests/common/mod.rs`), so that they start `ns-next` and
//! `ns-next-analyzer` the same way.

use std::ffi::OsStr;
use std::process::Command;

/// The target triple the binary this file is compiled into -- `ns-next`
/// or a test -- was built for, from the package's build script.
const TARGET: &str = env!("NS_NEXT_TARGET");

/// The variable that names `target`'s runner, as cargo spells it: the
/// triple in upper case, `-` and `.` as `_`.
pub fn runner_variable(target: &str) -> String {
    format!(
        "CARGO_TARGET_{}_RUNNER",
        target.to_uppercase().replace(['-', '.'], "_")
    )
}

/// A command that starts `binary`, one of the workspace's executables
/// built for the tests' own target, through that target's runner when its
/// variable is set.
pub fn target_command(binary: impl AsRef<OsStr>) -> Command {
    command_through(
        std::env::var(runner_variable(TARGET)).ok().as_deref(),
        binary,
    )
}

/// `binary` started through `runner` -- its words split on whitespace, the
/// first the program and the rest its leading arguments, as cargo splits
/// the variable -- or directly when there is none.
pub fn command_through(runner: Option<&str>, binary: impl AsRef<OsStr>) -> Command {
    let mut words = runner.unwrap_or_default().split_whitespace();
    let Some(program) = words.next() else {
        return Command::new(binary);
    };
    let mut command = Command::new(program);
    command.args(words).arg(binary);
    command
}

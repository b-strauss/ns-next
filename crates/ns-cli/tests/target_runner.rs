//! The tests start the workspace's binaries as cargo starts a test binary:
//! through the target's runner when one is set, directly otherwise
//! (`src/target_runner.rs`).

mod common;

use common::target_runner::{command_through, runner_variable};
use std::ffi::OsStr;

#[test]
fn a_runner_goes_before_the_binary_with_its_own_arguments() {
    let command = command_through(Some("qemu-aarch64 -cpu max"), "ns-next");
    assert_eq!(command.get_program(), "qemu-aarch64");
    let arguments: Vec<&OsStr> = command.get_args().collect();
    assert_eq!(arguments, ["-cpu", "max", "ns-next"]);
}

#[test]
fn without_a_runner_the_binary_runs_itself() {
    for runner in [None, Some(""), Some("  ")] {
        let command = command_through(runner, "ns-next");
        assert_eq!(command.get_program(), "ns-next");
        assert_eq!(command.get_args().count(), 0);
    }
}

#[test]
fn the_runner_variable_is_spelled_as_cargo_spells_it() {
    assert_eq!(
        runner_variable("aarch64-unknown-linux-gnu"),
        "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER"
    );
    assert_eq!(
        runner_variable("thumbv8m.main-none-eabi"),
        "CARGO_TARGET_THUMBV8M_MAIN_NONE_EABI_RUNNER"
    );
}

/// The binary the tests start is the one cargo built for this target, and
/// it starts: through the runner if this run has one, else directly.
#[test]
fn the_binary_starts_through_the_target_command() {
    let (success, _, stderr) = common::ns_next(&[OsStr::new("--version")]);
    assert!(success, "{stderr}");
}

//! Hands the package's tests the target triple they are compiled for, as
//! `NS_NEXT_TARGET`: a test that starts one of the workspace's binaries
//! runs it through that target's runner, `CARGO_TARGET_<TRIPLE>_RUNNER`,
//! as cargo runs the test binary itself (`ns-cli`'s
//! `tests/common/target_runner.rs`), and only a build script learns the
//! triple from cargo.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").expect("cargo sets TARGET for a build script");
    println!("cargo:rustc-env=NS_NEXT_TARGET={target}");
}

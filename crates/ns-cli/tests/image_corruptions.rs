//! A version-matching image whose body has been corrupted is a reported
//! error, never a panic and never a stack trace. Most flips are answered by
//! the decoder or by one of the two verification passes before anything
//! runs; a flip that leaves the image self-consistent (a literal's value, an
//! identity hash, a selector) starts the application, which then either
//! runs or raises the way any program can.
//!
//! Past the 17-byte header (magic, version, flags, the shipped world's
//! fingerprint), which the loader refuses outright, every 7th byte is
//! flipped two ways: enough coverage to sweep every section without
//! starting thousands of processes. One trial per part of the image's
//! bytes (`libtest-mimic`, so `harness = false`): as one test, the sweep's
//! processes ran one after another at the end of the suite while the other
//! cores idled.

use libtest_mimic::{Arguments, Trial};

mod common;
use common::{ScratchDirectory, golden, ns_next};

/// How many trials share the sweep.
const PARTS: usize = 8;

/// The bytes the loader refuses outright, whatever they hold.
const HEADER: usize = 17;

/// Every how many bytes one is flipped.
const STRIDE: usize = 7;

fn main() {
    let arguments = Arguments::from_args();
    let trials = (0..PARTS)
        .map(|part| {
            Trial::test(
                format!("no_corruption_of_a_version_matching_image_reaches_a_panic::part_{part}_of_{PARTS}"),
                move || {
                    sweep(part);
                    Ok(())
                },
            )
        })
        .collect();
    libtest_mimic::run(&arguments, trials).exit();
}

/// Flips every `STRIDE`th byte of the `part`th of the image's body, each
/// two ways, and loads each result.
fn sweep(part: usize) {
    let directory = ScratchDirectory::new(&format!("corruption-{part}"));
    let image = directory.join("app.nsi");
    let source = golden("language/block/blocks_and_return");
    let (ok, _out, error) = ns_next(&["build".as_ref(), source.as_os_str(), image.as_os_str()]);
    assert!(ok, "{error}");
    let original = std::fs::read(&image).expect("read image");
    let offsets: Vec<usize> = (HEADER..original.len()).step_by(STRIDE).collect();
    let length = offsets.len().div_ceil(PARTS);
    let mine =
        &offsets[(part * length).min(offsets.len())..((part + 1) * length).min(offsets.len())];
    let (mut swept, mut rejected) = (0, 0);
    for &offset in mine {
        for delta in [1u8, 0x80] {
            let mut bytes = original.clone();
            bytes[offset] ^= delta;
            std::fs::write(&image, &bytes).expect("write image");
            let (ok, _out, error) = ns_next(&["load".as_ref(), image.as_os_str()]);
            assert!(
                !error.contains("panicked") && !error.contains("RUST_BACKTRACE"),
                "byte {offset} ^ {delta:#x} panicked the loader: {error}"
            );
            let refused = error.starts_with("error: ");
            let raised = error.starts_with("Uncaught Newspeak error: ")
                || error.starts_with("internal: non-local return escaped the entry send");
            assert!(
                ok || refused || raised,
                "byte {offset} ^ {delta:#x} failed without an error message: {error}"
            );
            swept += 1;
            rejected += usize::from(refused);
        }
    }
    // Many flips land in a field that is still self-consistent, so much of
    // a part is expected to load; this only pins that the part really
    // reaches the checks rather than passing vacuously.
    assert!(
        swept > 10 && rejected * 4 >= swept,
        "part {part}: {rejected} of {swept} corrupted images were rejected"
    );
}

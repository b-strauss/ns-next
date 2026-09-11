//! `ns-next inspect`.

use std::path::Path;
use std::process::ExitCode;

/// `ns-next inspect`: what is in this artifact. Read-only -- nothing is
/// bootstrapped, instantiated or run, so inspecting a hostile file exercises
/// only the decoders.
pub(crate) fn execute(artifact: &Path, full: bool) -> ExitCode {
    let bytes = match std::fs::read(artifact) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: reading {}: {e}", artifact.display());
            return ExitCode::FAILURE;
        }
    };
    match ns_runtime::image::inspect::inspect(&bytes) {
        Ok(report) => {
            let rendered = if full {
                ns_runtime::image::inspect::render_full(&report)
            } else {
                ns_runtime::image::inspect::render(&report)
            };
            print!("{rendered}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {}: {e}", artifact.display());
            ExitCode::FAILURE
        }
    }
}

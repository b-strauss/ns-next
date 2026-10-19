//! `ns-next build`.

use std::path::Path;
use std::process::ExitCode;

use crate::program;

/// `ns-next build`: assemble the application exactly as `run` does -- its
/// factory and every initializer run here, with no platform in existence --
/// and write it instead of sending `main:args:`. An error while assembling
/// fails the build with the report `run` gives the same error.
pub(crate) fn execute(
    file: &Path,
    out: &Path,
    with_source: bool,
    lowering: ns_runtime::Lowering,
) -> ExitCode {
    let program = match program::read_program(file) {
        Ok(program) => program,
        Err(code) => return code,
    };
    let mut loaded = program::load_program(&program, lowering);
    let application = match program::instantiate_application(&program, &mut loaded) {
        Ok(application) => application,
        Err(code) => return code,
    };
    let classes = loaded.classes.clone();
    let bytes = loaded.interpreter.enter(|mutator| {
        let modules = program::module_sources(&program, &classes, mutator, with_source);
        ns_runtime::image::package(mutator, &modules, mutator.fetch(&application))
    });
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: build failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    match std::fs::write(out, bytes) {
        Ok(()) => {
            println!("wrote {}", out.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: writing {}: {error}", out.display());
            ExitCode::FAILURE
        }
    }
}

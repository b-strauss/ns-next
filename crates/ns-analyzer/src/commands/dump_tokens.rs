//! `ns-next-analyzer dump-tokens`.

use std::process::ExitCode;

/// Prints each file's tokens as one JSON document per line, naming the file
/// as it was given, and fails if any file cannot be read or lexed.
pub(crate) fn execute(files: &[String]) -> ExitCode {
    let mut status = ExitCode::SUCCESS;
    for file in files {
        match std::fs::read_to_string(file) {
            Ok(text) => match ns_analyzer::token_dump::dump(&text) {
                Ok(mut dumped) => {
                    dumped["file"] = serde_json::Value::String(file.clone());
                    println!("{dumped}");
                }
                Err(message) => {
                    eprintln!("error: {file}: {message}");
                    status = ExitCode::FAILURE;
                }
            },
            Err(error) => {
                eprintln!("error: reading {file}: {error}");
                status = ExitCode::FAILURE;
            }
        }
    }
    status
}

//! `ns-next-analyzer lsp`.

use std::process::ExitCode;

/// Serves the Language Server Protocol on stdin and stdout until the client
/// shuts the server down.
pub(crate) fn execute() -> ExitCode {
    let (connection, io_threads) = lsp_server::Connection::stdio();
    let outcome = ns_analyzer::server::run(connection);
    // Joining the I/O threads flushes the last response before exiting.
    let joined = io_threads.join();
    match (outcome, joined) {
        (Ok(()), Ok(())) => ExitCode::SUCCESS,
        (Err(error), _) => {
            eprintln!("ns-next-analyzer: {error}");
            ExitCode::FAILURE
        }
        (_, Err(error)) => {
            eprintln!("ns-next-analyzer: {error}");
            ExitCode::FAILURE
        }
    }
}

use std::process::ExitCode;

/// The stack the server runs on. It parses and resolves, which recurse; the
/// parser's nesting ceiling bounds the depth, and `ns-cli` runs the same
/// front end on 8 MiB for the same reason (`crates/ns-cli/src/main.rs`,
/// `INTERPRETER_STACK_BYTES`). The thread is needed regardless: the symbol
/// interner is thread-local, so everything that interns must run on one.
const SERVER_STACK_BYTES: usize = 8 * 1024 * 1024;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let worker = std::thread::Builder::new()
        .name("ns-next-lsp".to_string())
        .stack_size(SERVER_STACK_BYTES)
        .spawn(move || execute(&arguments))
        .expect("spawn the server thread");
    worker.join().unwrap_or_else(|_| ExitCode::from(101))
}

fn execute(arguments: &[String]) -> ExitCode {
    match arguments.split_first() {
        None => serve(),
        Some((command, files)) if command == "dump-tokens" => dump_tokens(files),
        _ => {
            eprintln!("usage: ns-next-lsp                        (speak LSP on stdin and stdout)");
            eprintln!(
                "       ns-next-lsp dump-tokens <file>...  (print each file's tokens as JSON)"
            );
            ExitCode::from(2)
        }
    }
}

fn serve() -> ExitCode {
    let (connection, io_threads) = lsp_server::Connection::stdio();
    let outcome = ns_lsp::server::run(connection);
    // Joining the I/O threads flushes the last response before exiting.
    let joined = io_threads.join();
    match (outcome, joined) {
        (Ok(()), Ok(())) => ExitCode::SUCCESS,
        (Err(error), _) => {
            eprintln!("ns-next-lsp: {error}");
            ExitCode::FAILURE
        }
        (_, Err(error)) => {
            eprintln!("ns-next-lsp: {error}");
            ExitCode::FAILURE
        }
    }
}

fn dump_tokens(files: &[String]) -> ExitCode {
    let mut status = ExitCode::SUCCESS;
    for file in files {
        match std::fs::read_to_string(file) {
            Ok(text) => match ns_lsp::token_dump::dump(&text) {
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

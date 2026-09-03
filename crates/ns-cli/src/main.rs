#![warn(clippy::all)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ns_syntax::{Diagnostic, Severity};

#[derive(Parser)]
#[command(name = "ns-next", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Parse a single .ns file.
    Parse {
        file: PathBuf,
        #[arg(long)]
        ast: bool,
    },
    /// Parse every .ns file under a directory.
    ParseCorpus {
        dir: PathBuf,
        #[arg(long)]
        blacklist: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Cmd::Parse { file, ast } => cmd_parse(&file, ast),
        Cmd::ParseCorpus { dir, blacklist } => cmd_parse_corpus(&dir, blacklist.as_deref()),
    }
}

fn cmd_parse(file: &Path, ast: bool) -> ExitCode {
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: error: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };
    match ns_syntax::parse(&src) {
        Ok(module) => {
            if ast {
                println!("{module:#?}");
            }
            println!("OK {}", file.display());
            ExitCode::SUCCESS
        }
        Err(diags) => {
            print_diagnostics(file, &src, &diags);
            ExitCode::FAILURE
        }
    }
}

fn cmd_parse_corpus(dir: &Path, blacklist: Option<&Path>) -> ExitCode {
    let bl_path = blacklist
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.parent().unwrap_or(dir).join("blacklist.toml"));
    let blacklist = load_blacklist(&bl_path);

    let mut files: Vec<PathBuf> = Vec::new();
    collect_ns_files(dir, &mut files);
    files.sort();

    let (mut parsed, mut unexpected_fail, mut expected_fail, mut stale) = (0, 0, 0, 0);
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(f).unwrap_or_default();
        let ok = ns_syntax::parse(&src).is_ok();
        let listed = blacklist.contains(&name);
        match (ok, listed) {
            (true, false) => parsed += 1,
            (true, true) => {
                stale += 1;
                eprintln!("STALE blacklist entry (now parses): {name}");
            }
            (false, true) => expected_fail += 1,
            (false, false) => {
                unexpected_fail += 1;
                eprintln!("FAIL {name}");
            }
        }
    }
    println!(
        "parsed {parsed}, failed {unexpected_fail}, expected-fail {expected_fail}, stale {stale}"
    );
    if unexpected_fail == 0 && stale == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn collect_ns_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ns_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "ns") {
            out.push(path);
        }
    }
}

fn load_blacklist(path: &Path) -> std::collections::HashSet<String> {
    #[derive(serde::Deserialize, Default)]
    struct Bl {
        #[serde(default)]
        entry: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        file: String,
        #[allow(dead_code)]
        reason: String,
    }
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let bl: Bl = toml::from_str(&text).unwrap_or_default();
    bl.entry.into_iter().map(|e| e.file).collect()
}

fn print_diagnostics(file: &Path, src: &str, diags: &[Diagnostic]) {
    for d in diags {
        let (line, col) = line_col(src, d.span.start);
        let level = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        eprintln!("{}:{line}:{col}: {level}: {}", file.display(), d.message);
    }
}

fn line_col(src: &str, offset: u32) -> (usize, usize) {
    let offset = offset as usize;
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in src.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

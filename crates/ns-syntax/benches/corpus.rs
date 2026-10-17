//! Times the frontend over the corpus: `cargo bench -p ns-syntax --bench corpus`.
//!
//! Every module of `corpus/modules` (or of the directory given) is lexed
//! (`lex`), parsed (`parse`, which lexes, parses and attaches comments) and
//! parsed for an editor (`parse_for_editor`), each phase timed once per
//! module per round. The rounds go over the modules in turn rather than
//! timing one module many times in a row, so a module is not measured with
//! the caches its own previous run left warm, and every round's sum over the
//! modules is one sample of the corpus's total. The report gives each
//! module's median per phase and the total's minimum, median and maximum.
//!
//! A `harness = false` binary on `std::time::Instant` alone, so that it adds
//! no dependency; it runs under `cargo bench`'s profile, which is the
//! release profile. Comparing two trees takes what the `measuring-performance`
//! skill says of every timing: the alignment pin and several builds a side.
//!
//! Options, after `--`:
//!
//! - `--rounds <count>`: the rounds timed (default 20), after one round
//!   that warms up and is not counted;
//! - `--sort parse|lex|editor|size|name`: the order of the per-module table
//!   (default `parse`, slowest first);
//! - `--top <count>`: only the first `count` modules of the table;
//! - `--filter <text>`: only the modules whose file name contains `text`;
//! - `--csv <path>`: also writes every module's medians, in nanoseconds,
//!   for a comparison of two runs; a relative path is relative to
//!   `crates/ns-syntax`, where `cargo bench` runs the binary;
//! - a directory: the `.ns` files in it, instead of `corpus/modules`.

use std::hint::black_box;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// The phases timed, in the order the report lists them.
const PHASES: [Phase; 3] = [Phase::Lex, Phase::Parse, Phase::Editor];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Lex,
    Parse,
    Editor,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Lex => "lex",
            Phase::Parse => "parse",
            Phase::Editor => "editor",
        }
    }

    fn index(self) -> usize {
        match self {
            Phase::Lex => 0,
            Phase::Parse => 1,
            Phase::Editor => 2,
        }
    }

    /// Runs the phase once over `source` and answers how long it took. What
    /// it answers is dropped inside the timing: an optimization that makes
    /// the tree cheaper to free is one the measurement should see.
    fn time(self, source: &str) -> Duration {
        let start = Instant::now();
        match self {
            Phase::Lex => drop(black_box(ns_syntax::lex(black_box(source)))),
            Phase::Parse => drop(black_box(ns_syntax::parse(black_box(source)))),
            Phase::Editor => drop(black_box(ns_syntax::parse_for_editor(black_box(source)))),
        }
        start.elapsed()
    }
}

enum SortKey {
    Phase(Phase),
    Size,
    Name,
}

struct Options {
    rounds: usize,
    sort: SortKey,
    top: Option<usize>,
    filter: Option<String>,
    csv: Option<PathBuf>,
    directory: PathBuf,
}

struct Module {
    name: String,
    source: String,
    /// Per phase, one duration per timed round.
    samples: [Vec<Duration>; 3],
}

impl Module {
    fn median(&self, phase: Phase) -> Duration {
        median(&self.samples[phase.index()])
    }
}

fn main() -> ExitCode {
    let options = match parse_options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };
    let mut modules = match read_modules(&options) {
        Ok(modules) => modules,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };

    // A module that does not parse would time the error path, not the parser.
    let refused: Vec<&str> = modules
        .iter()
        .filter(|module| ns_syntax::parse(&module.source).is_err())
        .map(|module| module.name.as_str())
        .collect();
    if !refused.is_empty() {
        eprintln!("error: these modules do not parse: {}", refused.join(", "));
        return ExitCode::FAILURE;
    }

    for module in &modules {
        for phase in PHASES {
            black_box(phase.time(&module.source));
        }
    }
    for _ in 0..options.rounds {
        for module in &mut modules {
            for phase in PHASES {
                let elapsed = phase.time(&module.source);
                module.samples[phase.index()].push(elapsed);
            }
        }
    }

    print_modules(&modules, &options);
    print_totals(&modules, options.rounds);
    if let Some(path) = &options.csv {
        if let Err(error) = write_csv(path, &modules) {
            eprintln!("error: cannot write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn parse_options(mut arguments: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        rounds: 20,
        sort: SortKey::Phase(Phase::Parse),
        top: None,
        filter: None,
        csv: None,
        directory: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/modules"),
    };
    while let Some(argument) = arguments.next() {
        let mut value = |option: &str| {
            arguments
                .next()
                .ok_or_else(|| format!("{option} needs a value"))
        };
        match argument.as_str() {
            // `cargo bench` passes `--bench` to every bench target.
            "--bench" => {}
            "--rounds" => options.rounds = parse_count("--rounds", &value("--rounds")?)?,
            "--top" => options.top = Some(parse_count("--top", &value("--top")?)?),
            "--filter" => options.filter = Some(value("--filter")?),
            "--csv" => options.csv = Some(PathBuf::from(value("--csv")?)),
            "--sort" => {
                options.sort = match value("--sort")?.as_str() {
                    "lex" => SortKey::Phase(Phase::Lex),
                    "parse" => SortKey::Phase(Phase::Parse),
                    "editor" => SortKey::Phase(Phase::Editor),
                    "size" => SortKey::Size,
                    "name" => SortKey::Name,
                    other => return Err(format!("unknown sort key `{other}`")),
                }
            }
            other if other.starts_with("--") => return Err(format!("unknown option `{other}`")),
            directory => options.directory = PathBuf::from(directory),
        }
    }
    Ok(options)
}

fn parse_count(option: &str, text: &str) -> Result<usize, String> {
    match text.parse::<usize>() {
        Ok(count) if count > 0 => Ok(count),
        _ => Err(format!("{option} needs a positive count, not `{text}`")),
    }
}

fn read_modules(options: &Options) -> Result<Vec<Module>, String> {
    let entries = std::fs::read_dir(&options.directory)
        .map_err(|error| format!("cannot read {}: {error}", options.directory.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "ns"))
        .collect();
    paths.sort();
    let mut modules = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if options
            .filter
            .as_ref()
            .is_some_and(|filter| !name.contains(filter.as_str()))
        {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        modules.push(Module {
            name,
            source,
            samples: [const { Vec::new() }; 3],
        });
    }
    if modules.is_empty() {
        return Err(format!(
            "no `.ns` module in {}",
            options.directory.display()
        ));
    }
    Ok(modules)
}

fn print_modules(modules: &[Module], options: &Options) {
    let mut order: Vec<&Module> = modules.iter().collect();
    match options.sort {
        SortKey::Phase(phase) => {
            order.sort_by_key(|module| std::cmp::Reverse(module.median(phase)))
        }
        SortKey::Size => order.sort_by_key(|module| std::cmp::Reverse(module.source.len())),
        SortKey::Name => {}
    }
    let shown = options.top.unwrap_or(order.len()).min(order.len());
    let name_width = order[..shown]
        .iter()
        .map(|module| module.name.len())
        .max()
        .unwrap_or(0)
        .max("module".len());

    println!(
        "{:<name_width$} {:>9} {:>11} {:>11} {:>11} {:>11}",
        "module", "bytes", "lex µs", "parse µs", "editor µs", "parse MB/s"
    );
    for module in &order[..shown] {
        let parse = module.median(Phase::Parse);
        println!(
            "{:<name_width$} {:>9} {:>11.1} {:>11.1} {:>11.1} {:>11.1}",
            module.name,
            module.source.len(),
            microseconds(module.median(Phase::Lex)),
            microseconds(parse),
            microseconds(module.median(Phase::Editor)),
            megabytes_per_second(module.source.len(), parse),
        );
    }
    if shown < order.len() {
        println!("... {} more", order.len() - shown);
    }
    println!("(medians of each module's {} rounds)", options.rounds);
}

fn print_totals(modules: &[Module], rounds: usize) {
    let bytes: usize = modules.iter().map(|module| module.source.len()).sum();
    println!();
    println!(
        "total over {} modules, {bytes} bytes, {rounds} rounds:",
        modules.len()
    );
    println!(
        "{:<8} {:>10} {:>10} {:>10} {:>12}",
        "phase", "min ms", "median ms", "max ms", "median MB/s"
    );
    for phase in PHASES {
        let mut totals: Vec<Duration> = (0..rounds)
            .map(|round| {
                modules
                    .iter()
                    .map(|module| module.samples[phase.index()][round])
                    .sum()
            })
            .collect();
        totals.sort();
        let median = median(&totals);
        println!(
            "{:<8} {:>10.2} {:>10.2} {:>10.2} {:>12.1}",
            phase.name(),
            milliseconds(totals[0]),
            milliseconds(median),
            milliseconds(totals[totals.len() - 1]),
            megabytes_per_second(bytes, median),
        );
    }
}

fn write_csv(path: &Path, modules: &[Module]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(file, "module,bytes,lex_ns,parse_ns,editor_ns")?;
    for module in modules {
        writeln!(
            file,
            "{},{},{},{},{}",
            module.name,
            module.source.len(),
            module.median(Phase::Lex).as_nanos(),
            module.median(Phase::Parse).as_nanos(),
            module.median(Phase::Editor).as_nanos(),
        )?;
    }
    file.flush()
}

/// The median of `samples`, the lower of the middle two for an even count.
fn median(samples: &[Duration]) -> Duration {
    let mut sorted = samples.to_vec();
    sorted.sort();
    sorted[(sorted.len() - 1) / 2]
}

fn microseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

fn megabytes_per_second(bytes: usize, duration: Duration) -> f64 {
    bytes as f64 / 1e6 / duration.as_secs_f64()
}

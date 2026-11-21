//! `ns-next run`.

use std::path::Path;
use std::process::ExitCode;

use ns_runtime::{Printing, TraceFilter, TraceSettings, TraceSink};

use crate::debug::{self, DebugOptions, HostLink, HostStart};
use crate::{entry, program, trace, trace_file};

/// `--trace`, `--trace-filter`, the print options, `--print-filter` and
/// `--trace-file` as given: the categories, one pattern for each
/// `--trace-filter`, the listings asked for, one pattern for each
/// `--print-filter`, and the file the trace goes to instead of standard
/// error.
pub(crate) struct TraceOptions<'text> {
    pub(crate) categories: Option<&'text str>,
    pub(crate) filter: Vec<&'text str>,
    pub(crate) printing: Printing,
    pub(crate) print_filter: Vec<&'text str>,
    pub(crate) file: Option<&'text Path>,
}

/// The status a refused option exits with, as the command line's own
/// refusals do.
const REFUSED_OPTION: u8 = 2;

/// Reports `refusal` of a trace option and answers the status it exits
/// with.
fn refuse_trace(refusal: &str) -> ExitCode {
    eprintln!("error: {refusal}");
    ExitCode::from(REFUSED_OPTION)
}

/// `ns-next run`: read the program, compile every module, assemble the
/// application, send it `main: platform args:`, report. Under a debugger
/// (`debug`), the agent is started once every module is compiled, the
/// application is assembled only once the client configured the session,
/// and the client hears the exit status.
#[expect(
    clippy::too_many_arguments,
    reason = "one per option of the command, each passed straight through"
)]
pub(crate) fn execute(
    file: &Path,
    lowering: ns_runtime::Lowering,
    cache_statistics: bool,
    collector_statistics: bool,
    jit_statistics: bool,
    trace_options: TraceOptions<'_>,
    script: Option<&str>,
    debug: Option<DebugOptions>,
    program_arguments: &[String],
) -> ExitCode {
    // All are read before the program, so that a misspelt category or
    // pattern is refused before anything is read or runs.
    let trace_categories = match trace_options.categories.map(trace::categories) {
        Some(Ok(categories)) => Some(categories),
        Some(Err(refusal)) => return refuse_trace(&refusal),
        None => None,
    };
    let trace_filter = match TraceFilter::parse(&trace_options.filter) {
        Ok(filter) => filter,
        Err(refusal) => return refuse_trace(&format!("--trace-filter: {refusal}")),
    };
    let print_filter = match TraceFilter::parse(&trace_options.print_filter) {
        Ok(filter) => filter,
        Err(refusal) => return refuse_trace(&format!("--print-filter: {refusal}")),
    };
    let program = match program::read_program(file) {
        Ok(program) => program,
        Err(code) => return code,
    };
    let arguments = match program::entry_arguments(&program, script, program_arguments) {
        Ok(arguments) => arguments,
        Err(code) => return code,
    };
    let mut loaded = program::load_program(&program, lowering);
    entry::report_jit_unavailable(&loaded.interpreter);
    entry::count_jit_if(&mut loaded.interpreter, jit_statistics);
    let printing = trace_options.printing;
    if trace_categories.is_some() || !printing.is_empty() {
        // Whether a pattern names a module of the program is known only
        // once it is read, and is decided before anything of it runs.
        for (option, filter) in [
            ("--trace-filter", &trace_filter),
            ("--print-filter", &print_filter),
        ] {
            if let Err(refusal) =
                trace::check_filter(option, filter, program::module_names(&program))
            {
                return refuse_trace(&refusal);
            }
        }
        // Created once every option is known good and before anything of
        // the program runs, so that a path no file can be created at is
        // refused as a malformed filter is. The file replaces standard
        // error (decision 20), and only its reader needs the bodies'
        // positions.
        let categories = trace_categories.unwrap_or_default();
        let sink: Box<dyn TraceSink> = match trace_options.file {
            Some(path) => {
                let modules = trace_file::ModuleSources::new(program::debug_modules(&program));
                match trace_file::create(path, modules, categories, printing) {
                    Ok(sink) => Box::new(sink),
                    Err(refusal) => return refuse_trace(&refusal),
                }
            }
            None => Box::new(trace::StandardErrorSink),
        };
        loaded.interpreter.set_trace(
            TraceSettings {
                categories,
                filter: trace_filter,
                printing,
                print_filter,
                describe_bodies: trace_options.file.is_some(),
            },
            sink,
        );
    }
    let link = match &debug {
        Some(options) => {
            let modules = program::debug_modules(&program);
            let name = file.display().to_string();
            match debug::start(options, &mut loaded.interpreter, name, modules) {
                Ok(link) => Some(link),
                Err(code) => return code,
            }
        }
        None => None,
    };
    let code = run_loaded(
        &program,
        &mut loaded,
        link.as_ref(),
        &arguments,
        Statistics {
            cache: cache_statistics,
            collector: collector_statistics,
            jit: jit_statistics,
        },
    );
    if let Some(link) = &link {
        link.exited(debug::status_of(code));
    }
    code
}

/// Which statistics to print after the run.
struct Statistics {
    cache: bool,
    collector: bool,
    jit: bool,
}

/// The host runs of a loaded program: the application's assembly, then
/// `main:args:`, each started only while no `terminate` arrived.
fn run_loaded(
    program: &program::Program,
    loaded: &mut program::Loaded,
    link: Option<&HostLink>,
    arguments: &[String],
    statistics: Statistics,
) -> ExitCode {
    if let Some(link) = link
        && link.wait_for_configuration(&mut loaded.interpreter) == HostStart::Terminate
    {
        return debug::report_terminated();
    }
    let application = match program::instantiate_application(program, loaded, link) {
        Ok(application) => application,
        Err(code) => return code,
    };
    if debug::terminated_before_run(link) {
        return debug::report_terminated();
    }
    let result = entry::send_main(&mut loaded.interpreter, &application, arguments, link);
    // The manifest's requests `main:args:` made; those of the assembly were
    // warned of when it ended (`program::instantiate_application`).
    program::warn_of_ambiguities(loaded);
    // To stderr, after the answer is decided: no fixture or golden reads
    // stdout any differently for it.
    if statistics.cache {
        entry::print_cache_statistics(&loaded.interpreter.cache_statistics());
    }
    if statistics.collector {
        entry::print_collector_statistics(&loaded.interpreter.collector_statistics());
    }
    if statistics.jit {
        entry::print_jit_statistics(&loaded.interpreter.jit_statistics());
    }
    entry::report_entry_send(result)
}

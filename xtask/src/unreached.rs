//! The tests no stress mode and no platform reaches, and the nextest
//! profiles that leave them out (`docs/design/README.md`, "Gates").
//!
//! A test whose process makes no interpreter and reads no runtime switch
//! answers the same under every stress mode but the wide encoding's, and
//! the same under that one too unless it compiles a body; a test that
//! reaches no platform-dependent code answers the same in the Docker
//! containers as on the host. Which tests these are is measured: the
//! default profile's suite run with the reach record on
//! (`ns_runtime::reach`), every test that recorded no `runtime` listed in
//! `LIST` with what it did record, by `cargo xtask unreached-tests`. The
//! `commit` group runs its suite with the record on too and fails when a
//! listed test now reaches more than the list says, so the list never
//! leaves out a test that a change made reachable; a test the list does
//! not name runs everywhere.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ns_runtime::reach::Reach;

/// The list, relative to the repository root.
pub(crate) const LIST: &str = "xtask/unreached-tests.txt";

/// Where a recorded run writes its markers, relative to the repository
/// root: under the gitignored `/target`, emptied before each run.
pub(crate) const RECORD_DIRECTORY: &str = "target/reach";

/// The nextest config the stress and Docker groups run with, relative to
/// the repository root: the repository's own, `.config/nextest.toml`, with
/// the profiles `GENERATED_PROFILES` appended, written afresh before each
/// use. A file rather than a filterset on the command line, which would
/// pass Windows' limit of 32 KiB, and which a Docker step's command, a
/// command line on the host too, could not carry either.
pub(crate) const GENERATED_CONFIG: &str = "target/xtask/nextest.toml";

/// The repository's nextest config, relative to its root.
const CONFIG: &str = ".config/nextest.toml";

/// The profile every stress mode but WIDE runs: `stress` without any
/// listed test.
pub(crate) const STRESS_PROFILE: &str = "stress-reached";

/// The profile WIDE runs: `stress` without the listed tests that compile
/// no body, since the wide encoding changes what the compiler builds.
pub(crate) const WIDE_PROFILE: &str = "stress-wide-reached";

/// The profile the Docker groups run: `default` without the listed tests
/// that reach no platform-dependent code.
pub(crate) const PLATFORM_PROFILE: &str = "platform";

/// The binaries never listed: the memory layer's, which is the platform
/// layer itself.
const NEVER_LISTED: &str = "ns-memory";

/// A test: nextest's binary id and the test's name in it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TestId {
    pub(crate) binary: String,
    pub(crate) name: String,
}

impl TestId {
    pub(crate) fn new(binary: &str, name: &str) -> TestId {
        TestId {
            binary: binary.to_string(),
            name: name.to_string(),
        }
    }
}

/// What each test recorded, or each listed test is listed as reaching.
pub(crate) type Reaches = BTreeMap<TestId, BTreeSet<Reach>>;

/// The markers a recorded run left in `directory`, by test; a temporary
/// file a killed process left before renaming it into a marker is
/// skipped.
pub(crate) fn read_record(directory: &Path) -> Result<Reaches, String> {
    let mut reaches = Reaches::new();
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("read {}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("read {}: {error}", directory.display()))?
            .path();
        if path.extension().is_some_and(|extension| extension == "tmp") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let fields: Vec<&str> = text.trim_end().split('\t').collect();
        let [binary, name, kind] = fields[..] else {
            return Err(format!("{}: not a marker: {text:?}", path.display()));
        };
        let reach =
            Reach::named(kind).ok_or_else(|| format!("{}: no kind {kind}", path.display()))?;
        reaches
            .entry(TestId::new(binary, name))
            .or_default()
            .insert(reach);
    }
    Ok(reaches)
}

/// Whether `text` can stand in a filterset's exact matcher as it is: a
/// name of anything else is not listed, so that it runs everywhere rather
/// than be matched wrongly.
fn writable(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_:./-".contains(character))
}

/// The list made from a recorded run of `tests`: each test that recorded
/// no `runtime`, with what it did record, but the memory layer's and those
/// whose names a filterset could not carry, which `unwritable` collects.
pub(crate) fn unreached_of(
    tests: &[TestId],
    record: &Reaches,
    unwritable: &mut Vec<TestId>,
) -> Reaches {
    let mut unreached = Reaches::new();
    for test in tests {
        if test.binary == NEVER_LISTED || test.binary.starts_with(&format!("{NEVER_LISTED}::")) {
            continue;
        }
        let reached = record.get(test).cloned().unwrap_or_default();
        if reached.contains(&Reach::Runtime) {
            continue;
        }
        if !writable(&test.binary) || !writable(&test.name) {
            unwritable.push(test.clone());
            continue;
        }
        unreached.insert(test.clone(), reached);
    }
    unreached
}

/// The heading of the tests that reach `reaches` -- `nothing` for none.
fn heading(reaches: &BTreeSet<Reach>) -> String {
    if reaches.is_empty() {
        return "nothing".to_string();
    }
    reaches
        .iter()
        .map(|reach| reach.name())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The list's text: a heading per set of kinds reached, each followed by
/// its tests, sorted, one per line, binary id and name.
pub(crate) fn format_list(unreached: &Reaches) -> String {
    let mut sections: BTreeMap<Vec<Reach>, Vec<&TestId>> = BTreeMap::new();
    for (test, reaches) in unreached {
        sections
            .entry(reaches.iter().copied().collect())
            .or_default()
            .push(test);
    }
    let mut text = String::from(
        "# The tests of the default profile that make no interpreter and read no\n\
         # runtime switch, under the heading of what else they reach: the compiler\n\
         # (the wide encoding's switch read) and platform-dependent code. Written by\n\
         # `cargo xtask unreached-tests`, which measures it; `cargo xtask gates\n\
         # commit` fails when a test listed here reaches more (xtask/src/unreached.rs).\n",
    );
    for (reaches, tests) in sections {
        text.push_str(&format!(
            "\n[{}]\n",
            heading(&reaches.into_iter().collect())
        ));
        for test in tests {
            text.push_str(&format!("{} {}\n", test.binary, test.name));
        }
    }
    text
}

/// The list `text` names, the inverse of `format_list`.
pub(crate) fn parse_list(text: &str) -> Result<Reaches, String> {
    let mut unreached = Reaches::new();
    let mut current: Option<BTreeSet<Reach>> = None;
    for (index, line) in text.lines().enumerate() {
        let line = line.trim_end();
        let number = index + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(heading) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let reaches = if heading == "nothing" {
                BTreeSet::new()
            } else {
                heading
                    .split(' ')
                    .map(|name| {
                        Reach::named(name)
                            .filter(|reach| *reach != Reach::Runtime)
                            .ok_or_else(|| format!("line {number}: no heading {heading}"))
                    })
                    .collect::<Result<_, _>>()?
            };
            current = Some(reaches);
            continue;
        }
        let Some(reaches) = &current else {
            return Err(format!("line {number}: a test before the first heading"));
        };
        let Some((binary, name)) = line.split_once(' ') else {
            return Err(format!("line {number}: not `<binary id> <test name>`"));
        };
        if !writable(binary) || !writable(name) {
            return Err(format!(
                "line {number}: `{line}` cannot stand in a filterset"
            ));
        }
        unreached.insert(TestId::new(binary, name), reaches.clone());
    }
    Ok(unreached)
}

/// The listed tests that a recorded run found reaching more than the list
/// says, each with what it reached.
pub(crate) fn stale(unreached: &Reaches, record: &Reaches) -> Vec<String> {
    unreached
        .iter()
        .filter_map(|(test, listed)| {
            let reached = record.get(test)?;
            (!reached.is_subset(listed)).then(|| {
                format!(
                    "{} {} (reaches {})",
                    test.binary,
                    test.name,
                    heading(reached)
                )
            })
        })
        .collect()
}

/// The listed tests that did not run among `tests`: a renamed or removed
/// test's entry, which leaves its name in every generated filter.
pub(crate) fn dead(unreached: &Reaches, tests: &[TestId]) -> Vec<String> {
    let ran: BTreeSet<&TestId> = tests.iter().collect();
    unreached
        .keys()
        .filter(|test| !ran.contains(test))
        .map(|test| format!("{} {}", test.binary, test.name))
        .collect()
}

/// The tests among `tests` the list does not name that the record found
/// reaching no runtime: a measurement would list them, so the gates could
/// leave them out too.
pub(crate) fn could_be_listed(
    unreached: &Reaches,
    tests: &[TestId],
    record: &Reaches,
) -> Vec<TestId> {
    let mut unwritable = Vec::new();
    unreached_of(tests, record, &mut unwritable)
        .into_keys()
        .filter(|test| !unreached.contains_key(test))
        .collect()
}

/// The golden a trial of `ns-cli`'s `run_goldens` runs, `golden::<lowering>::<path>`
/// naming `goldens/<path>.ns`: its path, or `None` for another test.
fn golden_of(test: &TestId) -> Option<&str> {
    if test.binary != "ns-cli::run_goldens" {
        return None;
    }
    let rest = test.name.strip_prefix("golden::")?;
    Some(rest.split_once("::")?.1)
}

/// Whether the golden at `path` below `goldens/` ends its run with status
/// 0, as its `.toml` records: a program that ran to its end, so its run
/// made an interpreter. A golden refused before it runs -- a resolve or
/// parse error -- makes none, and is no sentinel.
pub(crate) fn golden_exits_with_zero(root: &Path, path: &str) -> bool {
    std::fs::read_to_string(root.join("goldens").join(format!("{path}.toml"))).is_ok_and(|text| {
        text.lines()
            .filter_map(|line| line.trim().strip_prefix("status = "))
            .all(|status| status.trim() == "0")
    })
}

/// The sentinel over a recorded run of `tests`: every golden trial whose
/// program runs to status 0 -- `exits_with_zero` says which, by the
/// golden's path -- recorded `runtime`, and there are such trials. One
/// that did not means the record has come unwired from the interpreter --
/// a marker lost across the child process, say -- and a list measured
/// from it would leave out tests that run programs.
pub(crate) fn sentinel(
    tests: &[TestId],
    record: &Reaches,
    exits_with_zero: impl Fn(&str) -> bool,
) -> Result<(), String> {
    let programs: Vec<&TestId> = tests
        .iter()
        .filter(|test| golden_of(test).is_some_and(&exits_with_zero))
        .collect();
    if programs.is_empty() {
        return Err("the recorded run ran no golden trial of a program".to_string());
    }
    let unrecorded: Vec<String> = programs
        .iter()
        .filter(|test| {
            !record
                .get(test)
                .is_some_and(|reaches| reaches.contains(&Reach::Runtime))
        })
        .map(|test| format!("{} {}", test.binary, test.name))
        .collect();
    if unrecorded.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the reach record missed the runtime of {} of {} tests that run a program, so it \
         cannot be trusted to list tests:\n  {}",
        unrecorded.len(),
        programs.len(),
        unrecorded.join("\n  ")
    ))
}

/// A filterset matching the listed tests `leave_out` picks, a term per
/// binary, or `None` when it picks none.
pub(crate) fn exclusion(
    unreached: &Reaches,
    leave_out: impl Fn(&BTreeSet<Reach>) -> bool,
) -> Option<String> {
    let mut by_binary: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (test, reaches) in unreached {
        if leave_out(reaches) {
            by_binary
                .entry(test.binary.as_str())
                .or_default()
                .push(test.name.as_str());
        }
    }
    if by_binary.is_empty() {
        return None;
    }
    let terms: Vec<String> = by_binary
        .into_iter()
        .map(|(binary, names)| {
            let names: Vec<String> = names.iter().map(|name| format!("test(={name})")).collect();
            format!("(binary_id(={binary}) & ({}))", names.join(" | "))
        })
        .collect();
    Some(terms.join(" | "))
}

/// The `default-filter` of `profile` in the nextest config `config`, as
/// written between its quotes: every profile the generated ones inherit
/// from writes it on one line, as a basic string.
fn default_filter(config: &str, profile: &str) -> Result<String, String> {
    let header = format!("[profile.{profile}]");
    let mut lines = config.lines().skip_while(|line| line.trim() != header);
    if lines.next().is_none() {
        return Err(format!("{CONFIG} has no {header}"));
    }
    for line in lines {
        let line = line.trim();
        if line.starts_with('[') {
            break;
        }
        if let Some(value) = line.strip_prefix("default-filter = ") {
            return value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .map(str::to_string)
                .ok_or_else(|| {
                    format!("{header}'s default-filter is not a one-line basic string")
                });
        }
    }
    Err(format!("{header} in {CONFIG} sets no default-filter"))
}

/// Whether a profile leaves out a listed test that reaches what it is
/// given.
type LeaveOut = fn(&BTreeSet<Reach>) -> bool;

/// The generated profiles: each one's name, the profile it inherits --
/// its test groups, its overrides and its filter, which it narrows -- and
/// which listed tests it leaves out.
const GENERATED_PROFILES: [(&str, &str, LeaveOut); 3] = [
    (STRESS_PROFILE, "stress", |_| true),
    (WIDE_PROFILE, "stress", |reaches| {
        !reaches.contains(&Reach::Compiler)
    }),
    (PLATFORM_PROFILE, "default", |reaches| {
        !reaches.contains(&Reach::Platform)
    }),
];

/// The repository's nextest config `config` with the generated profiles
/// appended, each leaving out of its parent's filter the listed tests it
/// need not run.
pub(crate) fn generated_config(config: &str, unreached: &Reaches) -> Result<String, String> {
    let mut text = config.to_string();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!(
        "\n# Generated by `cargo xtask nextest-config` from {LIST}: do not edit.\n"
    ));
    for (name, parent, leave_out) in GENERATED_PROFILES {
        let parent_filter = default_filter(config, parent)?;
        let filter = match exclusion(unreached, leave_out) {
            Some(listed) => format!("({parent_filter}) and not ({listed})"),
            None => parent_filter,
        };
        text.push_str(&format!(
            "\n[profile.{name}]\ninherits = \"{parent}\"\ndefault-filter = \"{filter}\"\n"
        ));
    }
    Ok(text)
}

/// Reads the list and the repository's config under `root` and writes the
/// generated config, answering its path.
pub(crate) fn write_generated_config(root: &Path) -> Result<std::path::PathBuf, String> {
    let read = |relative: &str| {
        std::fs::read_to_string(root.join(relative))
            .map_err(|error| format!("read {relative}: {error}"))
    };
    let unreached = parse_list(&read(LIST)?).map_err(|error| format!("{LIST}: {error}"))?;
    let text = generated_config(&read(CONFIG)?, &unreached)?;
    let path = root.join(GENERATED_CONFIG);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    std::fs::write(&path, text).map_err(|error| format!("write {}: {error}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reaches(kinds: &[Reach]) -> BTreeSet<Reach> {
        kinds.iter().copied().collect()
    }

    fn example() -> Reaches {
        [
            (TestId::new("ns-syntax", "lexer::a_name"), reaches(&[])),
            (
                TestId::new("ns-cli::debug_metadata", "metadata::Kernel.ns"),
                reaches(&[Reach::Compiler]),
            ),
            (
                TestId::new("ns-cli::bin/ns-next", "project::a_path"),
                reaches(&[Reach::Platform]),
            ),
            (
                TestId::new("ns-runtime", "compact::a_wide_body"),
                reaches(&[Reach::Compiler, Reach::Platform]),
            ),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn the_list_reads_back_what_it_wrote() {
        let text = format_list(&example());
        assert!(text.contains("\n[nothing]\nns-syntax lexer::a_name\n"));
        assert!(text.contains("\n[compiler platform]\nns-runtime compact::a_wide_body\n"));
        assert_eq!(parse_list(&text), Ok(example()));
        assert!(
            parse_list("ns-syntax a_test\n").is_err(),
            "before a heading"
        );
        assert!(parse_list("[runtime]\nns-syntax a_test\n").is_err());
        assert!(parse_list("[nothing]\nns-syntax a(test)\n").is_err());
    }

    /// The real list parses, and names no test of the memory layer.
    #[test]
    fn the_checked_in_list_parses() {
        let text = std::fs::read_to_string(crate::repository_root().join(LIST)).expect("the list");
        let unreached = parse_list(&text).expect("a list");
        assert!(!unreached.is_empty());
        assert!(
            unreached
                .keys()
                .all(|test| !test.binary.starts_with(NEVER_LISTED))
        );
    }

    #[test]
    fn a_test_that_reaches_the_runtime_or_the_memory_layer_is_not_listed() {
        let tests = [
            TestId::new("ns-syntax", "lexer::a_name"),
            TestId::new("ns-runtime::vm", "a_send"),
            TestId::new("ns-memory", "a_reservation"),
            TestId::new("ns-memory::reservation", "a_commit"),
            TestId::new("ns-cli::run_goldens", "golden::a name with spaces"),
            TestId::new("ns-cli::debug_metadata", "metadata::Kernel.ns"),
        ];
        let record: Reaches = [
            (
                tests[1].clone(),
                reaches(&[Reach::Runtime, Reach::Compiler]),
            ),
            (tests[5].clone(), reaches(&[Reach::Compiler])),
        ]
        .into_iter()
        .collect();
        let mut unwritable = Vec::new();
        let unreached = unreached_of(&tests, &record, &mut unwritable);
        let listed: Vec<&TestId> = unreached.keys().collect();
        assert_eq!(listed, [&tests[5], &tests[0]]);
        assert_eq!(unreached[&tests[5]], reaches(&[Reach::Compiler]));
        assert_eq!(unwritable, [tests[4].clone()], "runs everywhere instead");
    }

    #[test]
    fn a_listed_test_that_reaches_more_is_stale() {
        let unreached = example();
        let record: Reaches = [
            // Reaches what it is listed as reaching: current.
            (
                TestId::new("ns-cli::debug_metadata", "metadata::Kernel.ns"),
                reaches(&[Reach::Compiler]),
            ),
            // Listed as reaching nothing, now compiles.
            (
                TestId::new("ns-syntax", "lexer::a_name"),
                reaches(&[Reach::Compiler]),
            ),
            // Listed as reaching the platform, now makes an interpreter.
            (
                TestId::new("ns-cli::bin/ns-next", "project::a_path"),
                reaches(&[Reach::Runtime, Reach::Platform]),
            ),
            // Not listed: runs everywhere anyway.
            (
                TestId::new("ns-runtime::vm", "a_send"),
                reaches(&[Reach::Runtime]),
            ),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            stale(&unreached, &record),
            [
                "ns-cli::bin/ns-next project::a_path (reaches runtime platform)",
                "ns-syntax lexer::a_name (reaches compiler)",
            ]
        );
    }

    #[test]
    fn each_profile_leaves_out_what_it_need_not_run() {
        let unreached = example();
        let left_out = |profile: &str| {
            let (_, _, leave_out) = GENERATED_PROFILES
                .iter()
                .find(|(name, _, _)| *name == profile)
                .expect("a profile");
            exclusion(&unreached, leave_out).expect("some")
        };
        assert_eq!(
            left_out(STRESS_PROFILE),
            "(binary_id(=ns-cli::bin/ns-next) & (test(=project::a_path))) \
             | (binary_id(=ns-cli::debug_metadata) & (test(=metadata::Kernel.ns))) \
             | (binary_id(=ns-runtime) & (test(=compact::a_wide_body))) \
             | (binary_id(=ns-syntax) & (test(=lexer::a_name)))"
        );
        assert_eq!(
            left_out(WIDE_PROFILE),
            "(binary_id(=ns-cli::bin/ns-next) & (test(=project::a_path))) \
             | (binary_id(=ns-syntax) & (test(=lexer::a_name)))"
        );
        assert_eq!(
            left_out(PLATFORM_PROFILE),
            "(binary_id(=ns-cli::debug_metadata) & (test(=metadata::Kernel.ns))) \
             | (binary_id(=ns-syntax) & (test(=lexer::a_name)))"
        );
        assert_eq!(exclusion(&Reaches::new(), |_| true), None);
    }

    #[test]
    fn a_generated_profile_narrows_its_parents_filter() {
        let config = "[profile.default]\ndefault-filter = \"not binary(slow)\"\n\n\
                      [profile.stress]\ndefault-filter = \"not package(ns-memory)\"\n\n\
                      [[profile.stress.overrides]]\nfilter = 'test(first)'\npriority = 100\n";
        let generated = generated_config(config, &example()).expect("a config");
        assert!(generated.starts_with(config));
        assert!(generated.contains(
            "\n[profile.stress-reached]\ninherits = \"stress\"\n\
             default-filter = \"(not package(ns-memory)) and not ((binary_id(=ns-cli::bin/ns-next)"
        ));
        assert!(generated.contains(
            "\n[profile.platform]\ninherits = \"default\"\n\
             default-filter = \"(not binary(slow)) and not ((binary_id(=ns-cli::debug_metadata)"
        ));
        assert!(generated.contains("\n[profile.stress-wide-reached]\ninherits = \"stress\"\n"));
        assert!(generated_config("[profile.stress]\n", &example()).is_err());
    }

    /// The repository's own config has the filters the generated profiles
    /// narrow.
    #[test]
    fn the_repositorys_config_has_the_parents_filters() {
        let config = std::fs::read_to_string(crate::repository_root().join(CONFIG))
            .expect("the nextest config");
        for (_, parent, _) in GENERATED_PROFILES {
            assert!(default_filter(&config, parent).is_ok(), "{parent}");
        }
    }

    #[test]
    fn a_marker_is_read_into_the_record() {
        let directory = crate::repository_root()
            .join("target")
            .join("xtask-test-markers");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a directory");
        std::fs::write(
            directory.join("a-runtime"),
            "ns-runtime::vm\ta_send\truntime\n",
        )
        .expect("a marker");
        std::fs::write(
            directory.join("a-platform"),
            "ns-runtime::vm\ta_send\tplatform\n",
        )
        .expect("a marker");
        let record = read_record(&directory).expect("a record");
        assert_eq!(
            record[&TestId::new("ns-runtime::vm", "a_send")],
            reaches(&[Reach::Runtime, Reach::Platform])
        );
        std::fs::write(directory.join("a-compiler.1234.tmp"), "").expect("a leftover");
        assert_eq!(read_record(&directory).expect("a record").len(), 1);
        std::fs::write(directory.join("broken"), "nothing to read").expect("a marker");
        assert!(read_record(&directory).is_err());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_listed_test_that_did_not_run_is_dead() {
        let unreached = example();
        let ran: Vec<TestId> = unreached.keys().skip(1).cloned().collect();
        let first = unreached.keys().next().expect("a test");
        assert_eq!(
            dead(&unreached, &ran),
            [format!("{} {}", first.binary, first.name)]
        );
        let all: Vec<TestId> = unreached.keys().cloned().collect();
        assert!(dead(&unreached, &all).is_empty());
    }

    #[test]
    fn an_unlisted_test_that_reaches_nothing_could_be_listed() {
        let unreached = example();
        let mut tests: Vec<TestId> = unreached.keys().cloned().collect();
        let new_one = TestId::new("ns-syntax", "lexer::a_new_test");
        let running = TestId::new("ns-runtime::vm", "a_send");
        tests.extend([new_one.clone(), running.clone()]);
        let record: Reaches = [(running, reaches(&[Reach::Runtime]))]
            .into_iter()
            .collect();
        assert_eq!(could_be_listed(&unreached, &tests, &record), [new_one]);
    }

    #[test]
    fn the_sentinel_wants_every_program_run_recorded() {
        let golden = TestId::new("ns-cli::run_goldens", "golden::optimized::language/a");
        let other = TestId::new("ns-cli::run_goldens", "every_golden_is_a_complete_pair");
        let refused = TestId::new("ns-cli::run_goldens", "golden::reference::language/refused");
        let tests = [golden.clone(), other, refused];
        let exits_with_zero = |path: &str| path == "language/a";
        let recorded: Reaches = [(golden.clone(), reaches(&[Reach::Runtime]))]
            .into_iter()
            .collect();
        assert_eq!(sentinel(&tests, &recorded, exits_with_zero), Ok(()));
        let error = sentinel(&tests, &Reaches::new(), exits_with_zero).expect_err("unrecorded");
        assert!(error.contains("golden::optimized::language/a"), "{error}");
        assert!(
            sentinel(&tests[1..], &recorded, exits_with_zero).is_err(),
            "no golden of a program ran"
        );
        let root = crate::repository_root();
        assert!(golden_exits_with_zero(
            &root,
            "language/block/and_or_fallback_closures_share_variables"
        ));
        assert!(!golden_exits_with_zero(
            &root,
            "language/module/root_superclass_naming_top_is_refused"
        ));
    }
}

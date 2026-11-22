//! `ns-next run --trace` (`docs/design/developer-tool.md`, decisions 8 and
//! 9): the categories and the filter it takes, and the sink that writes
//! each event the runtime delivers as one line on standard error.

use std::fmt::Write as _;
use std::io::Write as _;

use ns_runtime::tool_names::KnownModule;
use ns_runtime::trace_events::Listing;
use ns_runtime::{ModuleName, TraceCategories, TraceCategory, TraceEvent, TraceEventKind};
use ns_runtime::{TraceFilter, TraceSink};

use crate::{listing, program};

/// The word `--trace` takes for every category.
const ALL: &str = "all";

/// The categories `text` names, separated by commas: each a category's
/// name, or `all`. A name no category has is refused, naming it, so that
/// a misspelt category is never a trace that stays silent.
pub(crate) fn categories(text: &str) -> Result<TraceCategories, String> {
    categories_named(text).map_err(|reason| format!("--trace: {reason}"))
}

/// `categories`, its refusal naming no option: the MCP server's
/// `debug_trace_events` takes the categories as `--trace` does.
pub(crate) fn categories_named(text: &str) -> Result<TraceCategories, String> {
    let mut categories = TraceCategories::default();
    for name in text.split(',').map(str::trim) {
        if name == ALL {
            categories = TraceCategory::ALL
                .into_iter()
                .fold(categories, TraceCategories::with);
            continue;
        }
        let Some(category) = TraceCategory::from_name(name) else {
            let known: Vec<&str> = TraceCategory::ALL
                .into_iter()
                .map(TraceCategory::name)
                .collect();
            return Err(format!(
                "no category `{name}`; the categories are {}, or {ALL}",
                known.join(", ")
            ));
        };
        categories = categories.with(category);
    }
    Ok(categories)
}

/// Refuses a pattern of `filter`, given by `option`, that names a module
/// neither the program, whose modules are `modules`, nor the binary has,
/// naming the option and the pattern; checked once the program is loaded
/// and before it runs, so that a filter that would select nothing for
/// good is refused rather than leave the trace or the listings silent.
pub(crate) fn check_filter(
    option: &str,
    filter: &TraceFilter,
    modules: impl IntoIterator<Item = ModuleName>,
) -> Result<(), String> {
    let modules = program::known_modules(modules);
    filter
        .check(&|text: &str| KnownModule::resolve(text, &modules))
        .map_err(|reason| format!("{option}: {reason}"))
}

/// The line `event` is written as (`docs/design/developer-tool.md`,
/// decision 8): `[<category>] {<body>} <event>`, then its fields in a
/// fixed order, every name in braces, so that a line splits at the spaces
/// outside them.
pub(crate) fn line(event: &TraceEvent) -> String {
    let mut line = format!(
        "[{}] {{{}}} ",
        event.category.name(),
        event.body.as_ref().map_or("", |body| body.name.as_str())
    );
    let mut fields = Fields(&mut line);
    match &event.kind {
        TraceEventKind::Compiled { trigger, bytes } => {
            fields.event(event);
            fields.value("trigger", trigger.name());
            fields.value("bytes", bytes);
        }
        TraceEventKind::CompiledTwin { bytes } => {
            fields.event(event);
            fields.value("bytes", bytes);
        }
        TraceEventKind::CompileRefused { trigger, reason } => {
            fields.event(event);
            fields.value("trigger", trigger.name());
            fields.value("reason", reason.name());
        }
        TraceEventKind::CompileHeld { trigger, reason } => {
            fields.event(event);
            fields.value("trigger", trigger.name());
            fields.value("reason", reason.name());
        }
        TraceEventKind::TwinHeld { reason } => {
            fields.event(event);
            fields.value("reason", reason.name());
        }
        TraceEventKind::Abandoned { exits } => {
            fields.event(event);
            fields.value("exits", exits);
        }
        TraceEventKind::Revived { revival } => {
            fields.event(event);
            fields.value("by", revival.name());
        }
        TraceEventKind::RefusedForItsTwin => fields.event(event),
        TraceEventKind::AbandonedRefused { reason } => {
            fields.event(event);
            fields.value("reason", reason.name());
        }
        TraceEventKind::OsrCompiled { loop_head } | TraceEventKind::OsrEntered { loop_head } => {
            fields.event(event);
            fields.value("at", loop_head);
        }
        TraceEventKind::OsrRefused { loop_head, reason } => {
            fields.event(event);
            fields.value("at", loop_head);
            fields.value("reason", reason.name());
        }
        TraceEventKind::OptimizationRequested {
            spent_by,
            walk_end,
            key,
            entered,
        }
        | TraceEventKind::OptimizationNotDue {
            spent_by,
            walk_end,
            key,
            entered,
        } => {
            fields.event(event);
            fields.key(key, entered);
            fields.name("spent-by", &spent_by.name);
            fields.value("walk-end", walk_end.name());
        }
        TraceEventKind::OptimizationDropped {
            key,
            entered,
            cause,
        } => {
            fields.event(event);
            fields.key(key, entered);
            fields.value("cause", cause.name());
        }
        TraceEventKind::OptimizationDeferred {
            key,
            entered,
            reason,
        } => {
            fields.event(event);
            fields.key(key, entered);
            fields.value("reason", reason.name());
        }
        TraceEventKind::Optimized {
            key,
            entered,
            bytes,
            inlined_scopes,
            inlined_sites,
            type_case_arms,
            eliminated_blocks,
        } => {
            fields.event(event);
            fields.key(key, entered);
            fields.value("bytes", bytes);
            fields.value("inlined-scopes", inlined_scopes);
            fields.value("inlined-sites", inlined_sites);
            fields.value("type-case-arms", type_case_arms);
            fields.value("eliminated-blocks", eliminated_blocks);
        }
        TraceEventKind::OptimizationRefused {
            key,
            entered,
            reason,
        } => {
            fields.event(event);
            fields.key(key, entered);
            fields.value("reason", reason.name());
        }
        TraceEventKind::Inlined {
            key,
            entered,
            in_body,
            program_counter,
            callee,
        } => {
            fields.event(event);
            fields.key(key, entered);
            fields.name("in-body", &in_body.name);
            fields.name("callee", &callee.name);
            fields.value("at", program_counter);
        }
        TraceEventKind::InlineDeclined {
            key,
            entered,
            in_body,
            program_counter,
            callee,
            reason,
        } => {
            fields.event(event);
            fields.key(key, entered);
            fields.name("in-body", &in_body.name);
            fields.name("callee", &callee.name);
            fields.value("reason", reason.name());
            fields.value("at", program_counter);
        }
        TraceEventKind::InlineDeclinedNoBody {
            key,
            entered,
            reasons,
        } => {
            fields.event(event);
            fields.key(key, entered);
            fields.value(
                "count",
                reasons.iter().map(|&(_, count)| count).sum::<u32>(),
            );
            // Each reason with its count, `<reason>:<count>`, joined by
            // commas: one value, which no space splits.
            let spelled = reasons
                .iter()
                .map(|(reason, count)| format!("{}:{count}", reason.name()))
                .collect::<Vec<_>>()
                .join(",");
            fields.value("reasons", spelled);
        }
        TraceEventKind::Retired {
            key,
            entered,
            reason,
            version,
        } => {
            fields.event(event);
            fields.key(key, entered);
            fields.value("reason", reason.name());
            match version {
                Some(version) => fields.value("version", version),
                None => fields.value("version", "none"),
            }
        }
        TraceEventKind::Invalidated {
            key,
            entered,
            cause,
        } => {
            fields.event(event);
            fields.key(key, entered);
            fields.value("cause", cause.name());
        }
        TraceEventKind::Compacted {
            freeing,
            bodies_freed,
        } => {
            fields.event(event);
            fields.value("freeing", freeing.name());
            fields.value("bodies-freed", bodies_freed);
        }
        TraceEventKind::Freed {
            key,
            entered,
            tier,
            cause,
        } => {
            fields.event(event);
            fields.key(key, entered);
            fields.value("tier", tier.name());
            fields.value("cause", cause.name());
        }
        TraceEventKind::ZoneReplaced { bodies_freed } => {
            fields.event(event);
            fields.value("bodies-freed", bodies_freed);
        }
        TraceEventKind::CacheTransition {
            before,
            after,
            selector,
            site,
            receiver,
        } => {
            // The event is the transition itself, both states spelt.
            fields.word(&format!("{}->{}", before.name(), after.name()));
            fields.value("selector", format!("#{selector}"));
            fields.value("site", site);
            fields.name("receiver", receiver);
        }
        TraceEventKind::CachesVacated { cause } => {
            fields.event(event);
            fields.value("cause", cause.name());
        }
    }
    line
}

/// A line's event word and fields, written in the order they are given.
struct Fields<'line>(&'line mut String);

impl Fields<'_> {
    /// The event's word, as its kind names it.
    fn event(&mut self, event: &TraceEvent) {
        self.word(event.kind.name());
    }

    fn word(&mut self, word: &str) {
        self.0.push_str(word);
    }

    /// ` <field>=<value>`, for a value that is no name.
    fn value(&mut self, field: &str, value: impl std::fmt::Display) {
        let _ = write!(self.0, " {field}={value}");
    }

    /// ` <field>={<name>}`, for a value that names a body or a class.
    fn name(&mut self, field: &str, name: &str) {
        let _ = write!(self.0, " {field}={{{name}}}");
    }

    /// A customization key's fields, each only where the event has it: the
    /// receiver's class, and the class the body was entered at where it is
    /// another -- two fields, never joined, since `@` names a version.
    fn key(&mut self, key: &Option<String>, entered: &Option<String>) {
        if let Some(key) = key {
            self.name("key", key);
        }
        if let Some(entered) = entered {
            self.name("entered", entered);
        }
    }
}

/// Writes each event as its line on standard error.
pub(crate) struct StandardErrorSink;

impl TraceSink for StandardErrorSink {
    fn event(&mut self, event: &TraceEvent) {
        let mut text = line(event);
        text.push('\n');
        // The line and its newline in one write on the locked stream, so
        // that no other line the process writes on stderr lands inside it.
        // A stderr that cannot be written to loses the trace, never the
        // program's run.
        let _ = std::io::stderr().lock().write_all(text.as_bytes());
    }

    fn listing(&mut self, listing: &Listing) {
        // The whole listing in one write, as a line is: a script cuts it
        // out from its head to its end, which no other write interrupts.
        let text = listing::text(listing);
        let _ = std::io::stderr().lock().write_all(text.as_bytes());
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ns_runtime::trace_events::{
        BodyName, CodeTier, CompactionFreeing, CompileRefusal, CompileTrigger, Deferral, FreeCause,
        HoldReason, InlineDecline, InvalidationCause, OsrRefusal, RecompileWalkEnd, RetireReason,
        Revival, SiteState, VacateCause,
    };

    /// `name`, numbered as a trace's first body is.
    fn body_named(name: &str) -> BodyName {
        BodyName {
            number: 1,
            name: name.to_string(),
        }
    }

    /// An event of `category` about the body named `body`.
    fn about(category: TraceCategory, body: &str, kind: TraceEventKind) -> TraceEvent {
        TraceEvent {
            category,
            body: Some(body_named(body)),
            kind,
        }
    }

    /// An event of `category` about no body.
    fn about_no_body(category: TraceCategory, kind: TraceEventKind) -> TraceEvent {
        TraceEvent {
            category,
            body: None,
            kind,
        }
    }

    fn named(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    /// One event of every kind with the line it is written as. The match
    /// in `every_kind_is_listed` makes a new kind fail to compile until it
    /// has a line here.
    pub(crate) fn every_event() -> Vec<(TraceEvent, &'static str)> {
        use TraceCategory::{Caches, CodeZone, Compile, Deoptimize, Inlining, Optimize, Osr};
        vec![
            (
                about(
                    Compile,
                    "Demo>>factorial:",
                    TraceEventKind::Compiled {
                        trigger: CompileTrigger::Entry,
                        bytes: 412,
                    },
                ),
                "[compile] {Demo>>factorial:} compiled trigger=entry bytes=412",
            ),
            (
                about(
                    Compile,
                    "[] in Demo>>main:args:",
                    TraceEventKind::Compiled {
                        trigger: CompileTrigger::Entry,
                        bytes: 96,
                    },
                ),
                "[compile] {[] in Demo>>main:args:} compiled trigger=entry bytes=96",
            ),
            (
                about(
                    Compile,
                    "[] in Demo>>run",
                    TraceEventKind::CompiledTwin { bytes: 180 },
                ),
                "[compile] {[] in Demo>>run} compiled-twin bytes=180",
            ),
            (
                about(
                    Compile,
                    "Demo/Account class>>named:",
                    TraceEventKind::CompileRefused {
                        trigger: CompileTrigger::Call,
                        reason: CompileRefusal::ExitsAtOnce,
                    },
                ),
                "[compile] {Demo/Account class>>named:} refused trigger=call reason=exits-at-once",
            ),
            (
                about(
                    Compile,
                    "Demo>>run",
                    TraceEventKind::CompileHeld {
                        trigger: CompileTrigger::BackJump,
                        reason: HoldReason::ZoneFull,
                    },
                ),
                "[compile] {Demo>>run} held trigger=back-jump reason=zone-full",
            ),
            (
                about(
                    Compile,
                    "[] in Demo>>run",
                    TraceEventKind::TwinHeld {
                        reason: HoldReason::ZoneFull,
                    },
                ),
                "[compile] {[] in Demo>>run} held-twin reason=zone-full",
            ),
            (
                about(
                    Compile,
                    "Demo>>+",
                    TraceEventKind::Abandoned { exits: 1000 },
                ),
                "[compile] {Demo>>+} abandoned exits=1000",
            ),
            (
                about(
                    Compile,
                    "Demo>>run",
                    TraceEventKind::Revived {
                        revival: Revival::Decay,
                    },
                ),
                "[compile] {Demo>>run} revived by=decay",
            ),
            (
                about(Compile, "Demo>>run", TraceEventKind::RefusedForItsTwin),
                "[compile] {Demo>>run} refused-for-its-twin",
            ),
            (
                about(
                    Compile,
                    "Demo>>run",
                    TraceEventKind::AbandonedRefused {
                        reason: CompileRefusal::JitOff,
                    },
                ),
                "[compile] {Demo>>run} refused-abandoned reason=jit-off",
            ),
            (
                about(
                    Osr,
                    "Demo>>loop",
                    TraceEventKind::OsrCompiled { loop_head: 12 },
                ),
                "[osr] {Demo>>loop} osr-compiled at=12",
            ),
            (
                about(
                    Osr,
                    "Demo>>loop",
                    TraceEventKind::OsrEntered { loop_head: 12 },
                ),
                "[osr] {Demo>>loop} osr-entered at=12",
            ),
            (
                about(
                    Osr,
                    "Demo>>loop",
                    TraceEventKind::OsrRefused {
                        loop_head: 12,
                        reason: OsrRefusal::TwinHeld,
                    },
                ),
                "[osr] {Demo>>loop} osr-refused at=12 reason=twin-held",
            ),
            (
                about(
                    Optimize,
                    "Demo/Account>>deposit:",
                    TraceEventKind::OptimizationRequested {
                        spent_by: body_named("[] in Demo>>main:args:"),
                        walk_end: RecompileWalkEnd::NotInlined,
                        key: named("Demo/Account"),
                        entered: None,
                    },
                ),
                "[optimize] {Demo/Account>>deposit:} requested key={Demo/Account} \
                 spent-by={[] in Demo>>main:args:} walk-end=not-inlined",
            ),
            (
                about(
                    Optimize,
                    "Demo>>run",
                    TraceEventKind::OptimizationNotDue {
                        spent_by: body_named("Demo>>run"),
                        walk_end: RecompileWalkEnd::BottomOfStack,
                        key: None,
                        entered: None,
                    },
                ),
                "[optimize] {Demo>>run} not-due spent-by={Demo>>run} walk-end=bottom-of-stack",
            ),
            (
                about(
                    Optimize,
                    "Demo/Account>>deposit:",
                    TraceEventKind::OptimizationDropped {
                        key: named("Demo/Account"),
                        entered: named("Demo/Base"),
                        cause: InvalidationCause::Edit,
                    },
                ),
                "[optimize] {Demo/Account>>deposit:} dropped key={Demo/Account} \
                 entered={Demo/Base} cause=edit",
            ),
            (
                about(
                    Optimize,
                    "Demo>>run",
                    TraceEventKind::OptimizationDeferred {
                        key: None,
                        entered: None,
                        reason: Deferral::ZoneFull,
                    },
                ),
                "[optimize] {Demo>>run} deferred reason=zone-full",
            ),
            (
                about(
                    Optimize,
                    "Demo/Account>>deposit:",
                    TraceEventKind::Optimized {
                        key: named("Demo/Account"),
                        entered: named("Demo/Base"),
                        bytes: 640,
                        inlined_scopes: 3,
                        inlined_sites: 4,
                        type_case_arms: 2,
                        eliminated_blocks: 1,
                    },
                ),
                "[optimize] {Demo/Account>>deposit:} optimized key={Demo/Account} \
                 entered={Demo/Base} bytes=640 inlined-scopes=3 inlined-sites=4 \
                 type-case-arms=2 eliminated-blocks=1",
            ),
            (
                about(
                    Optimize,
                    "Demo>>run",
                    TraceEventKind::OptimizationRefused {
                        key: None,
                        entered: None,
                        reason: CompileRefusal::TooLarge,
                    },
                ),
                "[optimize] {Demo>>run} refused reason=too-large",
            ),
            (
                about(
                    Inlining,
                    "Demo>>main:args:",
                    TraceEventKind::Inlined {
                        key: named("Demo"),
                        entered: None,
                        in_body: body_named("Demo>>main:args:"),
                        program_counter: 7,
                        callee: body_named("Demo/Account>>deposit:"),
                    },
                ),
                "[inlining] {Demo>>main:args:} inlined key={Demo} in-body={Demo>>main:args:} \
                 callee={Demo/Account>>deposit:} at=7",
            ),
            (
                about(
                    Inlining,
                    "Demo>>main:args:",
                    TraceEventKind::InlineDeclined {
                        key: None,
                        entered: None,
                        in_body: body_named("Demo>>main:args:"),
                        program_counter: 31,
                        callee: body_named("[] in Demo>>run"),
                        reason: InlineDecline::OverWeight,
                    },
                ),
                "[inlining] {Demo>>main:args:} declined in-body={Demo>>main:args:} \
                 callee={[] in Demo>>run} reason=over-weight at=31",
            ),
            (
                about(
                    Inlining,
                    "Demo/Account>>deposit:",
                    TraceEventKind::InlineDeclinedNoBody {
                        key: named("Demo/Account"),
                        entered: named("Demo/Base"),
                        reasons: vec![
                            (InlineDecline::NotSnapshotted, 2),
                            (InlineDecline::NotExpressed, 7),
                        ],
                    },
                ),
                "[inlining] {Demo/Account>>deposit:} declined-no-body key={Demo/Account} \
                 entered={Demo/Base} count=9 reasons=not-snapshotted:2,not-expressed:7",
            ),
            (
                about(
                    Inlining,
                    "Demo>>main:args:",
                    TraceEventKind::InlineDeclinedNoBody {
                        key: None,
                        entered: None,
                        reasons: vec![(InlineDecline::NotExpressed, 1)],
                    },
                ),
                "[inlining] {Demo>>main:args:} declined-no-body count=1 \
                 reasons=not-expressed:1",
            ),
            (
                about(
                    Deoptimize,
                    "Demo/Account>>deposit:",
                    TraceEventKind::Retired {
                        key: named("Demo/Account"),
                        entered: None,
                        reason: RetireReason::Traps,
                        version: Some(2),
                    },
                ),
                "[deoptimize] {Demo/Account>>deposit:} retired key={Demo/Account} reason=traps \
                 version=2",
            ),
            (
                about(
                    Deoptimize,
                    "Demo>>run",
                    TraceEventKind::Retired {
                        key: None,
                        entered: None,
                        reason: RetireReason::Exits,
                        version: None,
                    },
                ),
                "[deoptimize] {Demo>>run} retired reason=exits version=none",
            ),
            (
                about(
                    Deoptimize,
                    "Demo>>run",
                    TraceEventKind::Invalidated {
                        key: None,
                        entered: None,
                        cause: InvalidationCause::BulkForward,
                    },
                ),
                "[deoptimize] {Demo>>run} invalidated cause=bulk-forward",
            ),
            (
                about_no_body(
                    CodeZone,
                    TraceEventKind::Compacted {
                        freeing: CompactionFreeing::ToAQuarter,
                        bodies_freed: 7,
                    },
                ),
                "[code-zone] {} compacted freeing=to-a-quarter bodies-freed=7",
            ),
            (
                about(
                    CodeZone,
                    "Demo>>run",
                    TraceEventKind::Freed {
                        key: named("Demo"),
                        entered: None,
                        tier: CodeTier::Optimized,
                        cause: FreeCause::MovedFeedbackBase,
                    },
                ),
                "[code-zone] {Demo>>run} freed key={Demo} tier=optimized \
                 cause=moved-feedback-base",
            ),
            (
                about_no_body(
                    CodeZone,
                    TraceEventKind::Freed {
                        key: None,
                        entered: None,
                        tier: CodeTier::Baseline,
                        cause: FreeCause::DeadBody,
                    },
                ),
                "[code-zone] {} freed tier=baseline cause=dead-body",
            ),
            (
                about_no_body(CodeZone, TraceEventKind::ZoneReplaced { bodies_freed: 12 }),
                "[code-zone] {} replaced bodies-freed=12",
            ),
            (
                about(
                    Caches,
                    "Demo>>main:args:",
                    TraceEventKind::CacheTransition {
                        before: SiteState::Monomorphic,
                        after: SiteState::Polymorphic,
                        selector: "deposit:".to_string(),
                        site: 3,
                        receiver: "Demo/Account".to_string(),
                    },
                ),
                "[caches] {Demo>>main:args:} monomorphic->polymorphic selector=#deposit: site=3 \
                 receiver={Demo/Account}",
            ),
            (
                about(
                    Caches,
                    "com.acme.util.Util@1.0.0/Inner>>run",
                    TraceEventKind::CacheTransition {
                        before: SiteState::Vacant,
                        after: SiteState::Monomorphic,
                        selector: ",".to_string(),
                        site: 0,
                        receiver: "newspeak.core.Kernel/String".to_string(),
                    },
                ),
                "[caches] {com.acme.util.Util@1.0.0/Inner>>run} vacant->monomorphic selector=#, \
                 site=0 receiver={newspeak.core.Kernel/String}",
            ),
            (
                about_no_body(
                    Caches,
                    TraceEventKind::CachesVacated {
                        cause: VacateCause::JitSwitched,
                    },
                ),
                "[caches] {} vacated cause=jit-switched",
            ),
        ]
    }

    #[test]
    fn every_event_has_its_line() {
        for (event, expected) in every_event() {
            assert_eq!(line(&event), expected);
        }
    }

    /// How many kinds of event there are: one past the last index the
    /// match in `every_kind_is_listed` gives, raised with every arm added.
    const EVENT_KINDS: usize = 28;

    /// Every kind of event appears in `every_event`: the match fails to
    /// compile for a kind added without its arm, and the test fails for
    /// one added without its line or without `EVENT_KINDS` raised.
    #[test]
    fn every_kind_is_listed() {
        let mut seen = std::collections::BTreeSet::new();
        for (event, _) in every_event() {
            let index = match event.kind {
                TraceEventKind::Compiled { .. } => 0,
                TraceEventKind::CompiledTwin { .. } => 1,
                TraceEventKind::CompileRefused { .. } => 2,
                TraceEventKind::Abandoned { .. } => 3,
                TraceEventKind::Revived { .. } => 4,
                TraceEventKind::RefusedForItsTwin => 5,
                TraceEventKind::OsrCompiled { .. } => 6,
                TraceEventKind::OsrEntered { .. } => 7,
                TraceEventKind::OptimizationRequested { .. } => 8,
                TraceEventKind::OptimizationNotDue { .. } => 9,
                TraceEventKind::OptimizationDeferred { .. } => 10,
                TraceEventKind::Optimized { .. } => 11,
                TraceEventKind::OptimizationRefused { .. } => 12,
                TraceEventKind::Inlined { .. } => 13,
                TraceEventKind::InlineDeclined { .. } => 14,
                TraceEventKind::Retired { .. } => 15,
                TraceEventKind::Invalidated { .. } => 16,
                TraceEventKind::Compacted { .. } => 17,
                TraceEventKind::Freed { .. } => 18,
                TraceEventKind::ZoneReplaced { .. } => 19,
                TraceEventKind::CacheTransition { .. } => 20,
                TraceEventKind::CompileHeld { .. } => 21,
                TraceEventKind::TwinHeld { .. } => 22,
                TraceEventKind::AbandonedRefused { .. } => 23,
                TraceEventKind::OsrRefused { .. } => 24,
                TraceEventKind::OptimizationDropped { .. } => 25,
                TraceEventKind::CachesVacated { .. } => 26,
                TraceEventKind::InlineDeclinedNoBody { .. } => 27,
            };
            seen.insert(index);
        }
        assert_eq!(
            seen,
            (0..EVENT_KINDS).collect(),
            "every kind listed once, `EVENT_KINDS` counting them"
        );
    }

    /// `text` split at every space outside braces.
    pub(crate) fn split_outside_braces(text: &str) -> Vec<&str> {
        let mut parts = Vec::new();
        let mut depth = 0usize;
        let mut start = 0;
        for (index, character) in text.char_indices() {
            match character {
                '{' => depth += 1,
                '}' => depth -= 1,
                ' ' if depth == 0 => {
                    parts.push(&text[start..index]);
                    start = index + 1;
                }
                _ => {}
            }
        }
        parts.push(&text[start..]);
        parts
    }

    /// A line splits at its spaces outside braces into its category, its
    /// body, its event and its fields, whatever spaces a name holds.
    #[test]
    fn a_line_splits_at_spaces_outside_braces() {
        for (event, _) in every_event() {
            let written = line(&event);
            let parts = split_outside_braces(&written);
            assert!(parts.len() >= 3, "{written}");
            assert_eq!(
                parts[0],
                format!("[{}]", event.category.name()),
                "{written}"
            );
            let body = event.body.as_ref().map_or("", |body| body.name.as_str());
            assert_eq!(parts[1], format!("{{{body}}}"), "{written}");
            assert!(!parts[2].is_empty() && !parts[2].contains('='), "{written}");
            for field in &parts[3..] {
                let (name, value) = field
                    .split_once('=')
                    .unwrap_or_else(|| panic!("`{field}` in {written}"));
                assert!(
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|character| character.is_ascii_lowercase() || character == '-'),
                    "`{field}` in {written}"
                );
                assert!(!value.is_empty(), "`{field}` in {written}");
                assert_eq!(
                    value.starts_with('{'),
                    value.ends_with('}'),
                    "`{field}` in {written}"
                );
            }
        }
    }
}

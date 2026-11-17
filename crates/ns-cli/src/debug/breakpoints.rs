//! The client's breakpoints as the VM knows them: `setBreakpoints`,
//! `setFunctionBreakpoints` and `setExceptionBreakpoints`, the parsing of
//! their hit conditions, log messages, method names and class names, and
//! the ids.
//!
//! A breakpoint's DAP id is the agent's own, never the VM's: the VM sets
//! a module's whole set afresh at every `SetBreakpoints`, under new ids,
//! while an IDE sends the set again at every change of it and expects a
//! breakpoint it keeps to keep its id. So a breakpoint keeps its DAP id
//! for as long as the client keeps sending it -- a line breakpoint by its
//! module and offset, a function breakpoint by its name -- and the VM's
//! ids of the latest answer map to it. A stop names a breakpoint only by
//! a DAP id the client's latest request for its set holds, never one it
//! replaced since the VM decided the stop.

use std::collections::BTreeMap;

use ns_runtime::{
    BreakpointState, ClassName, Command, HitCondition, LineBreakpoint, LogPart, MethodBreakpoint,
    MethodBreakpointState, ModuleName,
};
use ns_tooling::lines::{LineColumn, LineIndex};
use serde_json::json;

use super::agent::{Agent, Conversion};
use super::protocol;

/// Why a breakpoint the VM has not resolved is not verified.
const PENDING: &str = "pending: the module is not built yet, or no statement follows";

/// Why a method breakpoint the VM has not verified is not.
const UNDECLARED: &str =
    "pending: the module is not built yet, or its class declares no such method";

/// Makes a hit condition of its count.
type Condition = fn(u32) -> HitCondition;

/// A `hitCondition` as the VM counts it: `n` or `=n` or `==n` the hit
/// numbered `n`, `>n` every hit after it, `>=n` it and every one after,
/// `%n` every `n`th, `n` a decimal `u32`, surrounding blanks ignored.
pub(crate) fn parse_hit_condition(text: &str) -> Result<HitCondition, String> {
    let trimmed = text.trim();
    let forms: [(&str, Condition); 5] = [
        (">=", HitCondition::GreaterOrEqual),
        ("==", HitCondition::Equal),
        (">", HitCondition::Greater),
        ("=", HitCondition::Equal),
        ("%", HitCondition::Multiple),
    ];
    let (condition, digits) = forms
        .iter()
        .find_map(|(prefix, condition)| Some((*condition, trimmed.strip_prefix(prefix)?)))
        .unwrap_or((HitCondition::Equal, trimmed));
    let digits = digits.trim();
    let count = if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
        digits.parse::<u32>().ok()
    } else {
        None
    };
    // A count of 0 for the hit numbered so, or for every nth, would be
    // verified and never stop: hits count from 1, and no hit is a multiple
    // of 0.
    count
        .map(condition)
        .filter(|condition| {
            !matches!(
                condition,
                HitCondition::Equal(0) | HitCondition::Multiple(0)
            )
        })
        .ok_or_else(|| {
            format!("`{text}` is no hit condition: write `n`, `=n`, `>n`, `>=n` or `%n`")
        })
}

/// The class declarations named `Module.Path.To.Class`, as a function
/// breakpoint and the exception filters' conditions name one: one for each
/// module `named` takes `Module` for (`Sources::modules_of_class_name`) --
/// the program's and the shipped module of a name both declare.
pub(crate) fn parse_class_name(
    text: &str,
    named: &dyn Fn(&str) -> Vec<ModuleName>,
) -> Result<Vec<ClassName>, String> {
    let refused = || format!("`{text}` names no class: write `Module.Path.To.Class`");
    let mut segments = text.trim().split('.').map(str::to_string);
    let module = segments.next().unwrap_or_default();
    let path: Vec<String> = segments.collect();
    let malformed = |segment: &String| segment.is_empty() || segment.contains(char::is_whitespace);
    if malformed(&module) || path.iter().any(malformed) {
        return Err(refused());
    }
    Ok(named(&module)
        .into_iter()
        .map(|module| ClassName {
            module,
            path: path.clone(),
        })
        .collect())
}

/// Class names separated by commas, as an exception filter's condition
/// lists them; an empty entry is passed over.
pub(crate) fn parse_class_names(
    text: &str,
    named: &dyn Fn(&str) -> Vec<ModuleName>,
) -> Result<Vec<ClassName>, String> {
    let mut classes = Vec::new();
    for name in text.split(',').filter(|name| !name.trim().is_empty()) {
        classes.extend(parse_class_name(name, named)?);
    }
    Ok(classes)
}

/// The methods named `Module.Path.To.Class>>selector`, or
/// `Module.Path.To.Class class>>selector` for the class side's, their hit
/// condition left to the caller: one for each module `named` takes
/// `Module` for.
pub(crate) fn parse_method_name(
    text: &str,
    named: &dyn Fn(&str) -> Vec<ModuleName>,
) -> Result<Vec<MethodBreakpoint>, String> {
    let refused = || {
        format!(
            "`{text}` names no method: write `Module.Path.To.Class>>selector` or \
             `Module.Path.To.Class class>>selector`"
        )
    };
    let (class, selector) = text.split_once(">>").ok_or_else(refused)?;
    let selector = selector.trim();
    if selector.is_empty() || selector.contains(char::is_whitespace) {
        return Err(refused());
    }
    let class = class.trim();
    let (class, class_side) = match class.strip_suffix("class") {
        Some(rest) if rest.ends_with(char::is_whitespace) => (rest.trim_end(), true),
        _ => (class, false),
    };
    let classes = parse_class_name(class, named).map_err(|_| refused())?;
    Ok(classes
        .into_iter()
        .map(|class| MethodBreakpoint {
            class,
            class_side,
            selector: selector.to_string(),
            hit_condition: None,
            condition: None,
            log: None,
        })
        .collect())
}

/// The byte offset of the 1-based `line` and, for a block variant, the
/// 1-based `column` of `text`; `None` for a line the text does not have.
pub(crate) fn offset_of(text: &str, line: u32, column: Option<u32>) -> Option<u32> {
    let lines = LineIndex::new(text);
    let line = line.checked_sub(1)?;
    match column.and_then(|column| column.checked_sub(1)) {
        Some(column) => {
            lines.line_start(line)?;
            lines.offset(text, LineColumn { line, column })
        }
        None => lines.line_start(line),
    }
}

/// One set the client replaces as a whole: a module's line breakpoints, by
/// offset, or the function breakpoints, by name.
struct Set<K> {
    /// The key and DAP id of each breakpoint of the client's latest
    /// request that reached the VM, in its order.
    requested: Vec<(K, u32)>,
    /// The VM's ids of the latest answer.
    answered: Vec<u32>,
}

impl<K> Default for Set<K> {
    fn default() -> Set<K> {
        Set {
            requested: Vec::new(),
            answered: Vec::new(),
        }
    }
}

impl<K: PartialEq> Set<K> {
    /// Each of `keys`, the client's new request, with its DAP id: a key the
    /// previous request held keeps its id, each at most once. The set is
    /// left as it was until the request reached the VM.
    fn plan(&self, keys: Vec<K>, last_id: &mut u32) -> Vec<(K, u32)> {
        let mut previous: Vec<(&K, u32)> =
            self.requested.iter().map(|(key, id)| (key, *id)).collect();
        let mut planned = Vec::with_capacity(keys.len());
        for key in keys {
            let id = match previous.iter().position(|(held, _)| **held == key) {
                Some(index) => previous.remove(index).1,
                None => {
                    *last_id += 1;
                    *last_id
                }
            };
            planned.push((key, id));
        }
        planned
    }

    fn holds(&self, id: u32) -> bool {
        self.requested.iter().any(|(_, held)| *held == id)
    }
}

/// The DAP ids the agent gave out and the VM's ids they stand for.
#[derive(Default)]
pub(crate) struct Breakpoints {
    /// The last DAP id given out; the first is 1.
    last_id: u32,
    /// The highest VM id an `Event::Breakpoints` or
    /// `Event::MethodBreakpoints` named.
    highest_seen: Option<u32>,
    lines: BTreeMap<ModuleName, Set<u32>>,
    functions: Set<String>,
    /// Each VM id of each set's latest answer, with its DAP id.
    dap_ids: BTreeMap<u32, u32>,
}

impl Breakpoints {
    /// The line breakpoints at `offsets` of `module`, each with its DAP id.
    pub(crate) fn plan_lines(&mut self, module: &ModuleName, offsets: Vec<u32>) -> Vec<(u32, u32)> {
        match self.lines.get(module) {
            Some(set) => set.plan(offsets, &mut self.last_id),
            None => Set::default().plan(offsets, &mut self.last_id),
        }
    }

    /// The planned line breakpoints of `module` reached the VM: they are
    /// the client's latest request for the module.
    pub(crate) fn requested_lines(&mut self, module: &ModuleName, planned: Vec<(u32, u32)>) {
        self.lines.entry(module.clone()).or_default().requested = planned;
    }

    /// The function breakpoints named `names`, each with its DAP id.
    pub(crate) fn plan_functions(&mut self, names: Vec<String>) -> Vec<(String, u32)> {
        self.functions.plan(names, &mut self.last_id)
    }

    /// The planned function breakpoints reached the VM.
    pub(crate) fn requested_functions(&mut self, planned: Vec<(String, u32)>) {
        self.functions.requested = planned;
    }

    /// The DAP ids of the line breakpoints at `offsets` of `module`,
    /// planned and requested at once.
    #[cfg(test)]
    pub(crate) fn request_lines(&mut self, module: &ModuleName, offsets: Vec<u32>) -> Vec<u32> {
        let planned = self.plan_lines(module, offsets);
        let ids = planned.iter().map(|(_, id)| *id).collect();
        self.requested_lines(module, planned);
        ids
    }

    /// The DAP ids of the function breakpoints named `names`, planned and
    /// requested at once.
    #[cfg(test)]
    pub(crate) fn request_functions(&mut self, names: Vec<String>) -> Vec<u32> {
        let planned = self.plan_functions(names);
        let ids = planned.iter().map(|(_, id)| *id).collect();
        self.requested_functions(planned);
        ids
    }

    /// Every module the client set line breakpoints in.
    pub(crate) fn modules(&self) -> Vec<ModuleName> {
        self.lines.keys().cloned().collect()
    }

    /// Whether a breakpoints event naming the VM ids `ids` answers a
    /// command. The VM numbers every breakpoint it sets from one counter,
    /// and an answer sets its whole set afresh, so every id it names is
    /// new; an event sent unasked -- a pending breakpoint resolved, an
    /// install moved one -- names only breakpoints an earlier answer
    /// named. Telling them apart by their ids rather than by the offsets
    /// keeps an unasked event apart from the answer to a request of the
    /// same offsets, which is what an IDE sends at every change.
    ///
    /// An event naming no id passes vacuously, which is right only
    /// because the VM sends no unasked event of an empty set -- it reports
    /// a set unasked only when one of its breakpoints changed -- so an
    /// empty one answers a replacement that is on its way
    /// (`replacement_pending`), which a debug build checks.
    pub(crate) fn is_answer(&self, ids: &[u32], replacement_pending: bool) -> bool {
        debug_assert!(
            !ids.is_empty() || replacement_pending,
            "the VM sent a breakpoints event of an empty set nothing asked for"
        );
        ids.iter()
            .all(|id| self.highest_seen.is_none_or(|highest| *id > highest))
    }

    /// Records the VM ids a breakpoints event named.
    pub(crate) fn saw(&mut self, ids: &[u32]) {
        if let Some(highest) = ids.iter().max().copied() {
            self.highest_seen = Some(self.highest_seen.map_or(highest, |seen| seen.max(highest)));
        }
    }

    /// The VM's answer for `module`'s line breakpoints: the VM ids it
    /// gave, each with the DAP id it stands for.
    pub(crate) fn answered_lines(&mut self, module: &ModuleName, pairs: &[(u32, u32)]) {
        let set = self.lines.entry(module.clone()).or_default();
        Self::answered(set, &mut self.dap_ids, pairs);
    }

    /// The VM's answer for the function breakpoints.
    pub(crate) fn answered_functions(&mut self, pairs: &[(u32, u32)]) {
        Self::answered(&mut self.functions, &mut self.dap_ids, pairs);
    }

    fn answered<K>(set: &mut Set<K>, dap_ids: &mut BTreeMap<u32, u32>, pairs: &[(u32, u32)]) {
        for replaced in std::mem::take(&mut set.answered) {
            dap_ids.remove(&replaced);
        }
        for (vm_id, dap_id) in pairs {
            dap_ids.insert(*vm_id, *dap_id);
            set.answered.push(*vm_id);
        }
    }

    /// The DAP id the VM id `vm_id` stands for, if the client's latest
    /// request for its set still holds it.
    pub(crate) fn dap_id(&self, vm_id: u32) -> Option<u32> {
        let dap_id = *self.dap_ids.get(&vm_id)?;
        let current =
            self.functions.holds(dap_id) || self.lines.values().any(|set| set.holds(dap_id));
        current.then_some(dap_id)
    }

    /// Forgets every set, for the next client; the ids go on counting.
    pub(crate) fn clear(&mut self) {
        self.lines.clear();
        self.functions = Set::default();
        self.dap_ids.clear();
    }
}

/// A line breakpoint of a `setBreakpoints` request: refused by the agent
/// with its message, or sent at `offset` under the DAP id `id`.
pub(crate) enum LineEntry {
    Refused(String),
    Sent { offset: u32, id: u32 },
}

/// A function breakpoint of a `setFunctionBreakpoints` request.
pub(crate) enum FunctionEntry {
    Refused(String),
    /// Sent under the DAP id, as `count` method breakpoints of the VM's,
    /// one for each module its class name names.
    Sent {
        id: u32,
        count: usize,
    },
}

/// A line breakpoint as the client shows it, its line and column those of
/// the statement it stops at in `text`.
fn line_breakpoint(id: u32, state: &BreakpointState, text: &str) -> protocol::Breakpoint {
    let lines = LineIndex::new(text);
    let position = state
        .statement
        .map(|statement| lines.position(text, statement.start));
    protocol::Breakpoint {
        id: Some(id),
        verified: position.is_some(),
        message: position.is_none().then(|| PENDING.to_string()),
        line: position.map(|position| position.line + 1),
        column: position.map(|position| position.column + 1),
    }
}

/// A `setBreakpoints` response's breakpoints, in the request's order, each
/// the VM's state of the one at its offset; and the pairs of the VM's id
/// and the DAP id of each.
pub(crate) fn line_response(
    entries: &[LineEntry],
    states: &[BreakpointState],
    text: &str,
) -> (Vec<protocol::Breakpoint>, Vec<(u32, u32)>) {
    let mut taken = vec![false; states.len()];
    let mut pairs = Vec::new();
    let breakpoints = entries
        .iter()
        .map(|entry| match entry {
            LineEntry::Refused(message) => protocol::Breakpoint::unverified(message),
            LineEntry::Sent { offset, id } => {
                let index = (0..states.len())
                    .find(|index| !taken[*index] && states[*index].offset == *offset);
                let Some(index) = index else {
                    return protocol::Breakpoint::unverified("the VM did not set it");
                };
                taken[index] = true;
                pairs.push((states[index].id.0, *id));
                line_breakpoint(*id, &states[index], text)
            }
        })
        .collect();
    (breakpoints, pairs)
}

/// A `setFunctionBreakpoints` response's breakpoints, in the request's
/// order: the VM answers the ones sent in the order they were sent.
pub(crate) fn function_response(
    entries: &[FunctionEntry],
    states: &[MethodBreakpointState],
) -> (Vec<protocol::Breakpoint>, Vec<(u32, u32)>) {
    let mut states = states.iter();
    let mut pairs = Vec::new();
    let breakpoints = entries
        .iter()
        .map(|entry| match entry {
            FunctionEntry::Refused(message) => protocol::Breakpoint::unverified(message),
            FunctionEntry::Sent { id, count } => {
                let set: Vec<&MethodBreakpointState> = states.by_ref().take(*count).collect();
                if set.len() < *count || set.is_empty() {
                    return protocol::Breakpoint::unverified("the VM did not set it");
                }
                pairs.extend(set.iter().map(|state| (state.id.0, *id)));
                function_breakpoint(*id, set.iter().any(|state| state.verified))
            }
        })
        .collect();
    (breakpoints, pairs)
}

/// A function breakpoint as the client shows it: verified when any of the
/// VM's method breakpoints behind its id is.
fn function_breakpoint(id: u32, verified: bool) -> protocol::Breakpoint {
    protocol::Breakpoint {
        id: Some(id),
        verified,
        message: (!verified).then(|| UNDECLARED.to_string()),
        line: None,
        column: None,
    }
}

impl Agent {
    /// `setBreakpoints`: each breakpoint at the start of its line, or, with
    /// a column, at that place of its line, counted in the module's
    /// current text as far as the agent knows it (`Sources::text_for_positions`).
    pub(super) fn set_breakpoints(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) =
            self.arguments::<protocol::SetBreakpointsArguments>(connection, request)
        else {
            return;
        };
        let unverified = |message: &str| {
            let breakpoints: Vec<protocol::Breakpoint> = arguments
                .breakpoints
                .iter()
                .map(|_| protocol::Breakpoint::unverified(message))
                .collect();
            json!({ "breakpoints": breakpoints })
        };
        let Some(module) = self.sources.module_named_by(&arguments.source) else {
            let body = unverified("not a module of this program");
            self.respond(connection, request, Some(body));
            return;
        };
        let Some(text) = self.sources.text_for_positions(&module).map(str::to_string) else {
            let body = unverified("the module's text is not known yet");
            self.respond(connection, request, Some(body));
            return;
        };
        let wanted: Vec<Result<LineBreakpoint, String>> = arguments
            .breakpoints
            .iter()
            .map(|breakpoint| {
                let offset = offset_of(&text, breakpoint.line, breakpoint.column)
                    .ok_or_else(|| "the module has no such line".to_string())?;
                let hit_condition = hit_condition(breakpoint.hit_condition.as_deref())?;
                let log = breakpoint
                    .log_message
                    .as_deref()
                    .filter(|message| !message.is_empty())
                    .map(parse_log_message)
                    .transpose()?;
                Ok(LineBreakpoint {
                    offset,
                    hit_condition,
                    condition: condition(breakpoint.condition.as_deref()),
                    log,
                })
            })
            .collect();
        let sent: Vec<LineBreakpoint> = wanted.iter().flatten().cloned().collect();
        let planned = self.breakpoints.plan_lines(
            &module,
            sent.iter().map(|breakpoint| breakpoint.offset).collect(),
        );
        let mut ids = planned.iter().map(|(_, id)| *id);
        let entries = wanted
            .into_iter()
            .map(|wanted| match wanted {
                Ok(breakpoint) => LineEntry::Sent {
                    offset: breakpoint.offset,
                    id: ids.next().unwrap_or_default(),
                },
                Err(message) => LineEntry::Refused(message),
            })
            .collect();
        let command = Command::SetBreakpoints {
            module: module.clone(),
            breakpoints: sent,
        };
        let conversion = Conversion::Breakpoints {
            module: module.clone(),
            entries,
            text,
        };
        if self.forward_converted(connection, request, command, conversion) {
            self.breakpoints.requested_lines(&module, planned);
        }
    }

    /// `setFunctionBreakpoints`: a method breakpoint for each name.
    pub(super) fn set_function_breakpoints(
        &mut self,
        connection: u64,
        request: &protocol::Request,
    ) {
        let Some(arguments) =
            self.arguments::<protocol::SetFunctionBreakpointsArguments>(connection, request)
        else {
            return;
        };
        let wanted: Vec<Result<Vec<MethodBreakpoint>, String>> = arguments
            .breakpoints
            .iter()
            .map(|breakpoint| {
                let mut methods = parse_method_name(&breakpoint.name, &|name| {
                    self.sources.modules_of_class_name(name)
                })?;
                let counted = hit_condition(breakpoint.hit_condition.as_deref())?;
                let holds = condition(breakpoint.condition.as_deref());
                for method in &mut methods {
                    method.hit_condition = counted;
                    method.condition.clone_from(&holds);
                }
                Ok(methods)
            })
            .collect();
        let names = arguments
            .breakpoints
            .iter()
            .zip(&wanted)
            .filter(|(_, wanted)| wanted.is_ok())
            .map(|(breakpoint, _)| breakpoint.name.trim().to_string())
            .collect();
        let planned = self.breakpoints.plan_functions(names);
        let mut ids = planned.iter().map(|(_, id)| *id);
        let mut sent = Vec::new();
        let entries = wanted
            .into_iter()
            .map(|wanted| match wanted {
                Ok(methods) => {
                    let count = methods.len();
                    sent.extend(methods);
                    FunctionEntry::Sent {
                        id: ids.next().unwrap_or_default(),
                        count,
                    }
                }
                Err(message) => FunctionEntry::Refused(message),
            })
            .collect();
        let command = Command::SetMethodBreakpoints(sent);
        let conversion = Conversion::FunctionBreakpoints { entries };
        if self.forward_converted(connection, request, command, conversion) {
            self.breakpoints.requested_functions(planned);
        }
    }

    /// `setExceptionBreakpoints`: the filters `uncaught`, `caught` and
    /// `halt`, turned on by `filters` or `filterOptions`; the classes the
    /// options' conditions name, together, restrict both exception filters,
    /// since the VM keeps one list for them.
    pub(super) fn set_exception_breakpoints(
        &mut self,
        connection: u64,
        request: &protocol::Request,
    ) {
        let Some(arguments) =
            self.arguments::<protocol::SetExceptionBreakpointsArguments>(connection, request)
        else {
            return;
        };
        let (mut uncaught, mut caught, mut halt) = (false, false, false);
        let filters = arguments.filters.iter().chain(
            arguments
                .filter_options
                .iter()
                .map(|options| &options.filter_id),
        );
        for filter in filters {
            match filter.as_str() {
                "uncaught" => uncaught = true,
                "caught" => caught = true,
                "halt" => halt = true,
                other => {
                    let message = format!("no exception filter is named `{other}`");
                    self.respond_error(connection, request, &message);
                    return;
                }
            }
        }
        let mut classes: Vec<ClassName> = Vec::new();
        for condition in arguments
            .filter_options
            .iter()
            .filter_map(|options| options.condition.as_deref())
        {
            match parse_class_names(condition, &|name| self.sources.modules_of_class_name(name)) {
                Ok(names) => {
                    for name in names {
                        if !classes.contains(&name) {
                            classes.push(name);
                        }
                    }
                }
                Err(message) => {
                    self.respond_error(connection, request, &message);
                    return;
                }
            }
        }
        let count = arguments.filters.len() + arguments.filter_options.len();
        let command = Command::SetExceptionBreakpoints {
            uncaught,
            caught,
            halt,
            classes,
        };
        let conversion = Conversion::ExceptionBreakpoints { count };
        self.forward_converted(connection, request, command, conversion);
    }

    /// An `Event::Breakpoints` the VM sent unasked -- a pending breakpoint
    /// resolved, an install moved one -- reported as `breakpoint` events.
    /// One that arrives while a replacement of the module's set is on its
    /// way names breakpoints the replacement removes, and is dropped.
    pub(super) fn line_breakpoints_changed(
        &mut self,
        module: &ModuleName,
        states: &[BreakpointState],
    ) {
        if self.replacing("SetBreakpoints", Some(module)) {
            return;
        }
        let Some(text) = self.sources.text_for_positions(module).map(str::to_string) else {
            return;
        };
        for state in states {
            if let Some(id) = self.breakpoints.dap_id(state.id.0) {
                let breakpoint = line_breakpoint(id, state, &text);
                self.breakpoint_changed(breakpoint);
            }
        }
    }

    /// An `Event::MethodBreakpoints` the VM sent unasked: a module's build
    /// or an install verified one, or no longer declares its method.
    pub(super) fn function_breakpoints_changed(&mut self, states: &[MethodBreakpointState]) {
        if self.replacing("SetMethodBreakpoints", None) {
            return;
        }
        // A DAP id stands for every VM breakpoint its name set, one per
        // module, and is verified while any of them is: the whole set is in
        // the event.
        let mut changed: Vec<(u32, bool)> = Vec::new();
        for state in states {
            if let Some(id) = self.breakpoints.dap_id(state.id.0) {
                match changed.iter_mut().find(|(known, _)| *known == id) {
                    Some((_, verified)) => *verified |= state.verified,
                    None => changed.push((id, state.verified)),
                }
            }
        }
        for (id, verified) in changed {
            self.breakpoint_changed(function_breakpoint(id, verified));
        }
    }

    fn breakpoint_changed(&mut self, breakpoint: protocol::Breakpoint) {
        let body = protocol::BreakpointEventBody {
            reason: "changed",
            breakpoint,
        };
        self.event("breakpoint", Some(super::agent::to_value(body)));
    }
}

/// A breakpoint's hit condition; a blank one is none.
fn hit_condition(text: Option<&str>) -> Result<Option<HitCondition>, String> {
    text.filter(|text| !text.trim().is_empty())
        .map(parse_hit_condition)
        .transpose()
}

/// A breakpoint's condition, passed to the VM as written, which compiles
/// it and reports one that does not compile at the hit; a blank one is
/// none.
fn condition(text: Option<&str>) -> Option<String> {
    text.filter(|text| !text.trim().is_empty())
        .map(str::to_string)
}

/// A logpoint's message as the VM takes it: the text outside braces as it
/// is, `{{` and `}}` a brace of it, and each expression in braces
/// evaluated at the hit. An expression's own braces -- a tuple, `{1. 2}`
/// -- nest, and a brace in one of its strings or a character literal is
/// the expression's; so an expression that begins with a brace is
/// written after a blank, `{ {1. 2} size}`, since `{{` is a brace of the
/// text. A brace that never closes is refused, as is an empty
/// expression, rather than logged as text the user did not mean.
pub(crate) fn parse_log_message(text: &str) -> Result<Vec<LogPart>, String> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '{' | '}' if characters.next_if_eq(&character).is_some() => literal.push(character),
            '{' => {
                let expression = expression_in_braces(&mut characters).ok_or_else(|| {
                    format!(
                        "the log message `{text}` has an unclosed `{{`: write `{{{{` for a \
                         brace of the text"
                    )
                })?;
                if expression.trim().is_empty() {
                    return Err(format!(
                        "the log message `{text}` has an empty `{{}}`: write an expression \
                         in it, or `{{{{}}}}` for braces of the text"
                    ));
                }
                if !literal.is_empty() {
                    parts.push(LogPart::Text(std::mem::take(&mut literal)));
                }
                parts.push(LogPart::Expression(expression));
            }
            other => literal.push(other),
        }
    }
    if !literal.is_empty() {
        parts.push(LogPart::Text(literal));
    }
    Ok(parts)
}

/// The expression of a log message up to the brace that closes it, which
/// is taken; `None` when none does.
fn expression_in_braces(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Option<String> {
    let mut expression = String::new();
    let mut depth = 0_usize;
    let mut in_string = false;
    while let Some(character) = characters.next() {
        match character {
            // A doubled quote within a string ends it and begins it
            // again, which reads the same.
            '\'' => in_string = !in_string,
            // A character literal's character is never a brace or a quote
            // of the expression's.
            '$' if !in_string => {
                expression.push(character);
                if let Some(literal) = characters.next() {
                    expression.push(literal);
                }
                continue;
            }
            '{' if !in_string => depth += 1,
            '}' if !in_string => match depth.checked_sub(1) {
                Some(outer) => depth = outer,
                None => return Some(expression),
            },
            _ => {}
        }
        expression.push(character);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ns_runtime::BreakpointId;

    /// A class name's module taken for the program's alone.
    fn program(name: &str) -> Vec<ModuleName> {
        vec![ModuleName::program(name)]
    }

    /// The one method a name names when its module is the program's alone.
    fn method(text: &str) -> MethodBreakpoint {
        let mut methods = parse_method_name(text, &program).expect("a method");
        assert_eq!(methods.len(), 1, "{text}");
        methods.remove(0)
    }

    #[test]
    fn hit_conditions_parse_in_every_form() {
        let forms = [
            ("3", HitCondition::Equal(3)),
            (" =3 ", HitCondition::Equal(3)),
            ("==3", HitCondition::Equal(3)),
            (">3", HitCondition::Greater(3)),
            (">=3", HitCondition::GreaterOrEqual(3)),
            ("%2", HitCondition::Multiple(2)),
            ("> 4", HitCondition::Greater(4)),
            ("4294967295", HitCondition::Equal(u32::MAX)),
            (">0", HitCondition::Greater(0)),
            (">=0", HitCondition::GreaterOrEqual(0)),
        ];
        for (text, condition) in forms {
            assert_eq!(parse_hit_condition(text), Ok(condition), "{text}");
        }
    }

    #[test]
    fn log_messages_parse_into_text_and_expressions() {
        let text = |text: &str| LogPart::Text(text.to_string());
        let expression = |text: &str| LogPart::Expression(text.to_string());
        let forms = [
            ("plain", vec![text("plain")]),
            (
                "at {index}!",
                vec![text("at "), expression("index"), text("!")],
            ),
            ("{a}{b}", vec![expression("a"), expression("b")]),
            ("{{a}} }}{{", vec![text("{a} }{")]),
            ("a } b", vec![text("a } b")]),
            (
                "{ {1. 2} size} of {'}{' size} {$} value}",
                vec![
                    expression(" {1. 2} size"),
                    text(" of "),
                    expression("'}{' size"),
                    text(" "),
                    expression("$} value"),
                ],
            ),
            ("{'it''s' size}", vec![expression("'it''s' size")]),
        ];
        for (message, parts) in forms {
            assert_eq!(parse_log_message(message), Ok(parts), "{message}");
        }
    }

    #[test]
    fn a_malformed_log_message_is_refused() {
        for (message, said) in [
            ("at {index", "unclosed `{`"),
            ("at { {1. 2} size", "unclosed `{`"),
            ("{'}'", "unclosed `{`"),
            ("at {} and", "empty `{}`"),
            ("at { }", "empty `{}`"),
        ] {
            let refused = parse_log_message(message).expect_err(message);
            assert!(refused.contains(said), "{refused}");
            assert!(refused.contains(&format!("`{message}`")), "{refused}");
        }
    }

    #[test]
    fn a_malformed_hit_condition_is_refused() {
        for text in [
            "0",
            "=0",
            "==0",
            "%0",
            "",
            "x",
            "3 times",
            "<3",
            "=>3",
            "+3",
            "-1",
            "4294967296",
            "%",
            ">=3.5",
        ] {
            let refused = parse_hit_condition(text).expect_err(text);
            assert!(refused.contains(&format!("`{text}`")), "{refused}");
        }
    }

    #[test]
    fn method_names_parse_with_and_without_class_side() {
        let instance = method("Module.Outer.Inner>>at:put:");
        assert_eq!(
            instance,
            MethodBreakpoint {
                class: ClassName {
                    module: ModuleName::program("Module"),
                    path: vec!["Outer".to_string(), "Inner".to_string()],
                },
                class_side: false,
                selector: "at:put:".to_string(),
                hit_condition: None,
                condition: None,
                log: None,
            }
        );
        let class_side = method(" Module.Inner class >> new ");
        assert!(class_side.class_side);
        assert_eq!(class_side.class.path, ["Inner"]);
        assert_eq!(class_side.selector, "new");
        let root = method("Calls>>>>");
        assert!(root.class.path.is_empty());
        assert_eq!(root.selector, ">>");
        let named_class = method("Module.Subclass>>run");
        assert!(!named_class.class_side, "`Subclass` is a name, not a side");
    }

    /// A name whose module both the program and the binary declare names
    /// a method, and a class, in each.
    #[test]
    fn a_name_of_two_modules_names_one_class_in_each() {
        let both = |name: &str| vec![ModuleName::program(name), ModuleName::shipped(name)];
        let methods = parse_method_name("Kernel.Array>>do:", &both).expect("methods");
        assert_eq!(
            methods
                .iter()
                .map(|method| method.class.module.clone())
                .collect::<Vec<_>>(),
            [ModuleName::program("Kernel"), ModuleName::shipped("Kernel")]
        );
        let classes = parse_class_names("Kernel.Error, Program.Inner", &both).expect("classes");
        assert_eq!(classes.len(), 4);
        assert!(classes.iter().all(|class| !class.path.is_empty()));
    }

    #[test]
    fn a_malformed_method_name_is_refused() {
        for text in [
            "Calls double:",
            "Calls>>",
            ">>double:",
            "Calls..Inner>>run",
            "Calls>>at: put:",
        ] {
            assert!(parse_method_name(text, &program).is_err(), "{text}");
        }
    }

    #[test]
    fn class_names_parse_from_a_list() {
        let names = parse_class_names("Kernel.Error, Program.Outer.Inner,", &|name| {
            vec![ModuleName::shipped(name)]
        })
        .expect("names");
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].module, ModuleName::shipped("Kernel"));
        assert_eq!(names[1].path, ["Outer", "Inner"]);
        assert!(parse_class_names("Kernel.Error, Kernel Exception", &program).is_err());
        assert_eq!(parse_class_names(" ", &program), Ok(Vec::new()));
    }

    #[test]
    fn a_position_is_an_offset_of_its_line() {
        let text = "first\nsecond 😀 third\n";
        assert_eq!(offset_of(text, 2, None), Some(6));
        assert_eq!(offset_of(text, 2, Some(10)), Some(17), "after the pair");
        assert_eq!(offset_of(text, 0, None), None);
        assert_eq!(offset_of(text, 9, Some(1)), None);
    }

    #[test]
    fn a_breakpoint_keeps_its_id_while_the_client_sends_it() {
        let mut table = Breakpoints::default();
        let first = table.request_lines(&ModuleName::program("Loop"), vec![10, 20]);
        assert_eq!(first, [1, 2]);
        table.answered_lines(&ModuleName::program("Loop"), &[(0, 1), (1, 2)]);
        assert_eq!(table.dap_id(1), Some(2));
        let second = table.request_lines(&ModuleName::program("Loop"), vec![20, 30]);
        assert_eq!(second, [2, 3]);
        // The VM's ids of the earlier answer map until the next answer,
        // but a breakpoint the client removed is named no more.
        assert_eq!(table.dap_id(0), None);
        assert_eq!(table.dap_id(1), Some(2));
        table.answered_lines(&ModuleName::program("Loop"), &[(2, 2), (3, 3)]);
        assert_eq!(table.dap_id(1), None);
        assert_eq!(table.dap_id(3), Some(3));
        assert_eq!(table.request_functions(vec!["Loop>>run".to_string()]), [4]);
    }

    #[test]
    fn an_unasked_event_names_only_ids_an_answer_named() {
        let mut table = Breakpoints::default();
        assert!(table.is_answer(&[], true));
        assert!(table.is_answer(&[0, 1], true));
        table.saw(&[0, 1]);
        assert!(!table.is_answer(&[0, 1], false), "a pending one resolved");
        assert!(table.is_answer(&[2, 3], true), "the same offsets set again");
        assert!(table.is_answer(&[], true), "an empty set");
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "an empty set nothing asked for")]
    fn an_empty_event_nothing_asked_for_is_a_broken_assumption() {
        Breakpoints::default().is_answer(&[], false);
    }

    #[test]
    fn a_response_matches_the_vm_s_states_by_offset() {
        let text = "a.\nb.\n";
        let entries = [
            LineEntry::Sent { offset: 3, id: 7 },
            LineEntry::Refused("no".to_string()),
            LineEntry::Sent { offset: 0, id: 8 },
        ];
        let states = [
            BreakpointState {
                id: BreakpointId(4),
                offset: 0,
                statement: Some(ns_syntax::Span::new(0, 2)),
            },
            BreakpointState {
                id: BreakpointId(5),
                offset: 3,
                statement: None,
            },
        ];
        let (breakpoints, pairs) = line_response(&entries, &states, text);
        assert_eq!(pairs, [(5, 7), (4, 8)]);
        assert!(!breakpoints[0].verified);
        assert_eq!(breakpoints[0].id, Some(7));
        assert!(breakpoints[1].id.is_none());
        assert_eq!(breakpoints[2].line, Some(1));
    }
}

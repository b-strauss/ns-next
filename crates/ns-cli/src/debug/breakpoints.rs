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

use ns_runtime::modules;
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

/// How a class is written: its module by its id and, for one version
/// only, `@` its version, then `/` and the path to a class inside it
/// (`docs/design/modules.md`, decision 8).
const CLASS_FORM: &str = "`<id>[@<version>][/<Class>(.<Class>)*]`";

/// A module of the program's or the binary's that a class name a client
/// typed names (`Sources::module_of`): its id, and the versions the
/// program has of it -- none for an id whose module has no version, every
/// shipped module's among them.
pub(crate) struct KnownModule {
    pub(crate) id: String,
    pub(crate) versions: Vec<String>,
}

/// Why a class name is refused, as `parse_class` says it.
enum ClassRefusal {
    /// The text is not of the form at all.
    Malformed,
    /// The text is of the form, and the reason says what is wrong.
    Explained(String),
}

/// The class declaration `text` names, `<id>[@<version>][/<Class>(.<Class>)*]`:
/// a module's class when no `/` follows the module, a class inside it
/// otherwise. A name without a version names the class in every version
/// of the id. `module_of` answers the module the program or the binary
/// has that a text names (`Sources::module_of`): an id no module has is
/// refused, since it would wait, unverified, for a module that never
/// comes, and so is a version no module of the id has; a dotted one a prefix of which names a module with
/// the form that names the class inside that one -- it is the form a
/// nested class was written in before, `Demo.Account` for
/// `Demo/Account` -- and one that is the class's name of one module with
/// that module's id, `newspeak.core.Kernel/Array` for `Kernel/Array`.
fn parse_class(
    text: &str,
    module_of: &dyn Fn(&str) -> Option<KnownModule>,
) -> Result<ClassName, ClassRefusal> {
    let text = text.trim();
    let (module, path) = match text.split_once('/') {
        Some((module, path)) => (module, Some(path)),
        None => (text, None),
    };
    let (id, version) = match module.split_once('@') {
        Some((id, version)) => (id, Some(version)),
        None => (module, None),
    };
    let path: Vec<String> = match path {
        Some(path) => path.split('.').map(str::to_string).collect(),
        None => Vec::new(),
    };
    if !id.split('.').all(modules::is_identifier)
        || !path.iter().all(|segment| modules::is_identifier(segment))
    {
        return Err(ClassRefusal::Malformed);
    }
    let version = match version {
        Some(version) => Some(
            ns_runtime::modules::Version::parse(version)
                .map_err(ClassRefusal::Explained)?
                .to_string(),
        ),
        None => None,
    };
    let named = module_of(id);
    if let Some(known) = named.as_ref().filter(|known| known.id == id) {
        // A version the program has no module of names a module that
        // never comes, as an unknown id does.
        if let Some(version) = &version
            && !known.versions.contains(version)
        {
            let versions = if known.versions.is_empty() {
                format!("`{id}` has no version")
            } else {
                format!("its versions are {}", known.versions.join(", "))
            };
            return Err(ClassRefusal::Explained(format!(
                "no module `{id}@{version}`; {versions}"
            )));
        }
        return Ok(ClassName {
            module: ModuleName {
                id: id.to_string(),
                version,
            },
            path,
        });
    }
    let written = |module: &str| match &version {
        Some(version) => format!("{module}@{version}"),
        None => module.to_string(),
    };
    if id.contains('.') {
        let segments: Vec<&str> = id.split('.').collect();
        let prefix = (1..segments.len())
            .rev()
            .find_map(|length| Some((length, module_of(&segments[..length].join("."))?.id)));
        if let Some((length, prefix)) = prefix {
            let inside = segments[length..]
                .iter()
                .copied()
                .chain(path.iter().map(String::as_str))
                .collect::<Vec<&str>>()
                .join(".");
            let module = written(&prefix);
            return Err(ClassRefusal::Explained(format!(
                "no module `{id}`; a class inside a module is written `{module}/{inside}`"
            )));
        }
    }
    // Every module of the program and of the binary is known before the
    // program starts, so an id none of them has names a module that never
    // comes: the breakpoint would wait unverified for good. Where the id is
    // the class's name of one module -- `Kernel` for the binary's
    // `newspeak.core.Kernel` -- the refusal says how that one is written.
    Err(ClassRefusal::Explained(match named.map(|known| known.id) {
        Some(module) if path.is_empty() => {
            format!(
                "no module `{id}`; the module is written `{}`",
                written(&module)
            )
        }
        Some(module) => format!(
            "no module `{id}`; a class inside a module is written `{}/{}`",
            written(&module),
            path.join(".")
        ),
        None => format!("no module `{id}`"),
    }))
}

/// The class declaration named `<id>[@<version>][/<Class>(.<Class>)*]`, as
/// a function breakpoint, an exception filter's condition and the memory
/// view name one (`parse_class`).
pub(crate) fn parse_class_name(
    text: &str,
    module_of: &dyn Fn(&str) -> Option<KnownModule>,
) -> Result<ClassName, String> {
    parse_class(text, module_of).map_err(|refusal| match refusal {
        ClassRefusal::Malformed => format!("`{text}` names no class: write {CLASS_FORM}"),
        ClassRefusal::Explained(reason) => reason,
    })
}

/// Class names separated by commas, as an exception filter's condition
/// lists them; an empty entry is passed over.
pub(crate) fn parse_class_names(
    text: &str,
    module_of: &dyn Fn(&str) -> Option<KnownModule>,
) -> Result<Vec<ClassName>, String> {
    text.split(',')
        .filter(|name| !name.trim().is_empty())
        .map(|name| parse_class_name(name, module_of))
        .collect()
}

/// The method named `<class>>><selector>`, or `<class> class>><selector>`
/// for the class side's, the class written as `parse_class` reads it --
/// `com.acme.json.Json@2.1.0/Parser>>parse:` -- its hit condition left to
/// the caller.
pub(crate) fn parse_method_name(
    text: &str,
    module_of: &dyn Fn(&str) -> Option<KnownModule>,
) -> Result<MethodBreakpoint, String> {
    let refused = || {
        format!(
            "`{text}` names no method: write {CLASS_FORM} followed by `>>selector`, or by \
             ` class>>selector` for the class side"
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
    let class = parse_class(class, module_of).map_err(|refusal| match refusal {
        ClassRefusal::Malformed => refused(),
        ClassRefusal::Explained(reason) => reason,
    })?;
    Ok(MethodBreakpoint {
        class,
        class_side,
        selector: selector.to_string(),
        hit_condition: None,
        condition: None,
        log: None,
    })
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
    /// Sent under the DAP id, as one method breakpoint of the VM's.
    Sent {
        id: u32,
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
            FunctionEntry::Sent { id } => {
                let Some(state) = states.next() else {
                    return protocol::Breakpoint::unverified("the VM did not set it");
                };
                pairs.push((state.id.0, *id));
                function_breakpoint(*id, state.verified)
            }
        })
        .collect();
    (breakpoints, pairs)
}

/// A function breakpoint as the client shows it: verified when the VM's
/// method breakpoint behind its id is.
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
        let wanted: Vec<Result<MethodBreakpoint, String>> = arguments
            .breakpoints
            .iter()
            .map(|breakpoint| {
                let mut method =
                    parse_method_name(&breakpoint.name, &|id| self.sources.module_of(id))?;
                method.hit_condition = hit_condition(breakpoint.hit_condition.as_deref())?;
                method.condition = condition(breakpoint.condition.as_deref());
                Ok(method)
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
                Ok(method) => {
                    sent.push(method);
                    FunctionEntry::Sent {
                        id: ids.next().unwrap_or_default(),
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
            match parse_class_names(condition, &|id| self.sources.module_of(id)) {
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
        // A DAP id stands for the VM breakpoint its name set; the whole set
        // is in the event.
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
            // A comment's quotes and braces are the comment's own.
            '(' if !in_string && characters.peek() == Some(&'*') => {
                expression.push(character);
                // The `*` that opens the comment closes nothing.
                expression.extend(characters.next());
                let mut previous = None;
                for inner in characters.by_ref() {
                    expression.push(inner);
                    if previous == Some('*') && inner == ')' {
                        break;
                    }
                    previous = Some(inner);
                }
                continue;
            }
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

    /// The ids of the modules the tests' program and binary have.
    const MODULES: [&str; 8] = [
        "Module",
        "Calls",
        "Demo",
        "Program",
        "Kernel",
        "com.acme.json.Json",
        "newspeak.core.Kernel",
        "newspeak.core.PlatformFactory",
    ];

    /// The versions the tests' program has of `com.acme.json.Json`; its
    /// other modules have none.
    const JSON_VERSIONS: [&str; 2] = ["2.1.0", "2.1.0-beta.1"];

    /// The module of `MODULES` `text` names, as `Sources::module_of`
    /// answers it.
    fn module_of(text: &str) -> Option<KnownModule> {
        let known = |id: &str| KnownModule {
            id: id.to_string(),
            versions: if id == "com.acme.json.Json" {
                JSON_VERSIONS.map(str::to_string).to_vec()
            } else {
                Vec::new()
            },
        };
        if MODULES.contains(&text) {
            return Some(known(text));
        }
        let mut by_class = MODULES
            .iter()
            .filter(|id| !text.contains('.') && id.rsplit('.').next() == Some(text));
        match (by_class.next(), by_class.next()) {
            (Some(only), None) => Some(known(only)),
            _ => None,
        }
    }

    /// The method a name names.
    fn method(text: &str) -> MethodBreakpoint {
        parse_method_name(text, &module_of).expect("a method")
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
            (
                "{index (* it's } { *) + 1}!",
                vec![expression("index (* it's } { *) + 1"), text("!")],
            ),
            ("{(*) ' *) 2}", vec![expression("(*) ' *) 2")]),
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
            ("at {index (* } *}", "unclosed `{`"),
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
        let instance = method("Module/Outer.Inner>>at:put:");
        assert_eq!(
            instance,
            MethodBreakpoint {
                class: ClassName {
                    module: ModuleName::named("Module"),
                    path: vec!["Outer".to_string(), "Inner".to_string()],
                },
                class_side: false,
                selector: "at:put:".to_string(),
                hit_condition: None,
                condition: None,
                log: None,
            }
        );
        let class_side = method(" Module/Inner class >> new ");
        assert!(class_side.class_side);
        assert_eq!(class_side.class.path, ["Inner"]);
        assert_eq!(class_side.selector, "new");
        let named_class = method("Module/Subclass>>run");
        assert!(!named_class.class_side, "`Subclass` is a name, not a side");
    }

    #[test]
    fn method_names_parse_with_id_version_and_inner_path() {
        let parsed = method("com.acme.json.Json@2.1.0/Parser.Token class>>at:put:");
        assert_eq!(
            parsed.class.module,
            ModuleName {
                id: "com.acme.json.Json".to_string(),
                version: Some("2.1.0".to_string()),
            }
        );
        assert_eq!(parsed.class.path, ["Parser", "Token"]);
        assert!(parsed.class_side);
        assert_eq!(parsed.selector, "at:put:");
        let every_version = method("com.acme.json.Json/Parser>>parse:");
        assert_eq!(
            every_version.class.module,
            ModuleName::named("com.acme.json.Json")
        );
        let pre_release = method("com.acme.json.Json@2.1.0-beta.1>>parse:");
        assert_eq!(
            pre_release.class.module.version.as_deref(),
            Some("2.1.0-beta.1")
        );
    }

    #[test]
    fn a_root_method_has_no_slash() {
        let root = method("Calls>>>>");
        assert_eq!(root.class.module, ModuleName::named("Calls"));
        assert!(root.class.path.is_empty());
        assert_eq!(root.selector, ">>");
        let dotted = method("com.acme.json.Json>>parse:");
        assert_eq!(dotted.class.module, ModuleName::named("com.acme.json.Json"));
        assert!(dotted.class.path.is_empty());
        let class_side = method("Calls class>>new");
        assert!(class_side.class_side);
        assert!(class_side.class.path.is_empty());
    }

    /// A name in the old form, its nested class after a dot, names no
    /// module of the program but its prefix does: it is refused with the
    /// form that names the class, never read as a module of its own.
    #[test]
    fn a_dotted_nested_class_is_refused_with_the_new_form() {
        assert_eq!(
            parse_method_name("Demo.Account>>deposit:", &module_of),
            Err(
                "no module `Demo.Account`; a class inside a module is written `Demo/Account`"
                    .to_string()
            )
        );
        assert_eq!(
            parse_class_name("Demo.Account.Entry", &module_of),
            Err(
                "no module `Demo.Account.Entry`; a class inside a module is written \
                 `Demo/Account.Entry`"
                    .to_string()
            )
        );
        assert_eq!(
            parse_method_name("com.acme.json.Json.Parser>>parse:", &module_of),
            Err(
                "no module `com.acme.json.Json.Parser`; a class inside a module is written \
                 `com.acme.json.Json/Parser`"
                    .to_string()
            )
        );
        // A prefix that is the class's name of one module names that
        // module: the shipped modules were written by their class's name.
        assert_eq!(
            parse_class_name("PlatformFactory.Inner", &module_of),
            Err(
                "no module `PlatformFactory.Inner`; a class inside a module is written \
                 `newspeak.core.PlatformFactory/Inner`"
                    .to_string()
            )
        );
    }

    /// Every module is known before the program starts, so an id no
    /// module has is refused rather than left waiting for good; one that
    /// is the class's name of one module says how that module is written.
    #[test]
    fn an_id_no_module_has_is_refused() {
        assert_eq!(
            parse_method_name("org.example.Tool>>run", &module_of),
            Err("no module `org.example.Tool`".to_string())
        );
        assert_eq!(
            parse_class_name("Elsewhere/Inner", &module_of),
            Err("no module `Elsewhere`".to_string())
        );
        // Without a program module `Kernel`, `Kernel` is the class's name
        // of the binary's `newspeak.core.Kernel` alone.
        let without_kernel = |text: &str| match text {
            "Kernel" | "newspeak.core.Kernel" => Some(KnownModule {
                id: "newspeak.core.Kernel".to_string(),
                versions: Vec::new(),
            }),
            _ => None,
        };
        assert_eq!(
            parse_method_name("Kernel/Array>>do:", &without_kernel),
            Err("no module `Kernel`; a class inside a module is written \
                 `newspeak.core.Kernel/Array`"
                .to_string())
        );
        assert_eq!(
            parse_method_name("Kernel class>>new", &without_kernel),
            Err("no module `Kernel`; the module is written `newspeak.core.Kernel`".to_string())
        );
        assert_eq!(
            parse_method_name("newspeak.core.Kernel/Array>>do:", &without_kernel)
                .map(|method| method.class.module),
            Ok(ModuleName::named("newspeak.core.Kernel"))
        );
    }

    /// The program's own `Kernel` and the binary's are written apart, by
    /// their ids.
    #[test]
    fn a_program_kernel_and_the_shipped_kernel_are_named_apart() {
        let own = method("Kernel/Array>>do:");
        let shipped = method("newspeak.core.Kernel/Array>>do:");
        assert_eq!(own.class.module, ModuleName::named("Kernel"));
        assert_eq!(
            shipped.class.module,
            ModuleName::named("newspeak.core.Kernel")
        );
        assert_eq!(own.class.path, shipped.class.path);
    }

    #[test]
    fn a_malformed_method_name_is_refused() {
        for text in [
            "Calls double:",
            "Calls>>",
            ">>double:",
            "Calls/Inner..Deeper>>run",
            "Calls/>>run",
            "Calls//Inner>>run",
            "Calls..Inner>>run",
            "Calls@>>run",
            "@1.0.0>>run",
            "Calls>>at: put:",
        ] {
            assert!(parse_method_name(text, &module_of).is_err(), "{text}");
        }
        assert_eq!(
            parse_method_name("Calls@1.0>>run", &module_of),
            Err("`1.0` is no version: it needs MAJOR.MINOR.PATCH".to_string())
        );
    }

    /// A version no module of the id has names a module that never comes,
    /// as an unknown id does: refused, naming the versions there are,
    /// rather than left to wait unverified for good.
    #[test]
    fn a_version_no_module_of_the_id_has_is_refused() {
        assert_eq!(
            parse_method_name("com.acme.json.Json@9.9.9>>parse:", &module_of),
            Err(
                "no module `com.acme.json.Json@9.9.9`; its versions are 2.1.0, 2.1.0-beta.1"
                    .to_string()
            )
        );
        assert_eq!(
            parse_class_name("Calls@1.0.0/Inner", &module_of).map(|_| ()),
            Err("no module `Calls@1.0.0`; `Calls` has no version".to_string())
        );
        assert_eq!(
            parse_class_name("newspeak.core.Kernel@1.0.0/Array", &module_of).map(|_| ()),
            Err(
                "no module `newspeak.core.Kernel@1.0.0`; `newspeak.core.Kernel` has no version"
                    .to_string()
            )
        );
        assert!(parse_method_name("com.acme.json.Json@2.1.0>>parse:", &module_of).is_ok());
    }

    #[test]
    fn class_names_parse_from_a_list() {
        let names = parse_class_names(
            "newspeak.core.Kernel/Error, Program/Outer.Inner,",
            &module_of,
        )
        .expect("names");
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].module, ModuleName::named("newspeak.core.Kernel"));
        assert_eq!(names[0].path, ["Error"]);
        assert_eq!(names[1].module, ModuleName::named("Program"));
        assert_eq!(names[1].path, ["Outer", "Inner"]);
        assert!(parse_class_names("Kernel/Error, Kernel Exception", &module_of).is_err());
        assert_eq!(parse_class_names(" ", &module_of), Ok(Vec::new()));
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
        let first = table.request_lines(&ModuleName::named("Loop"), vec![10, 20]);
        assert_eq!(first, [1, 2]);
        table.answered_lines(&ModuleName::named("Loop"), &[(0, 1), (1, 2)]);
        assert_eq!(table.dap_id(1), Some(2));
        let second = table.request_lines(&ModuleName::named("Loop"), vec![20, 30]);
        assert_eq!(second, [2, 3]);
        // The VM's ids of the earlier answer map until the next answer,
        // but a breakpoint the client removed is named no more.
        assert_eq!(table.dap_id(0), None);
        assert_eq!(table.dap_id(1), Some(2));
        table.answered_lines(&ModuleName::named("Loop"), &[(2, 2), (3, 3)]);
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

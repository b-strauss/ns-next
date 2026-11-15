//! What the agent keeps of one stop: its frames, once the VM answered
//! them, the value its step's last call returned, and the
//! `variablesReference` ids it handed out, each naming a frame's
//! variables or a value's children. Everything here is valid until the
//! program goes on; the ids continue across the stops of a session, so
//! that a reference of an earlier stop names nothing rather than a value
//! of this one.

use std::collections::BTreeMap;

use ns_runtime::{FrameDescription, ReturnedValue, ValueId, Variable, VariableValue};
use ns_syntax::Span;

use super::protocol;
use super::sources::Sources;

/// What a `variablesReference` names.
#[derive(Clone)]
pub(crate) enum Target {
    /// The variables of the frame at this index of the stack trace.
    Frame(usize),
    /// A value's children.
    Value {
        id: ValueId,
        child_count: usize,
        indexed: bool,
    },
    /// The step's stop's return value, the one variable of its scope,
    /// named by the call that answered it.
    Returned(ReturnedValue),
}

/// The stop's frames, as far as the agent has them.
pub(crate) enum Frames {
    /// Not asked of the VM yet.
    Missing,
    /// Asked; the answer is on its way.
    Asked,
    Known(Vec<FrameDescription>),
    /// The VM refused the ask, for this reason.
    Refused(String),
}

pub(crate) struct Stop {
    /// Raised at every stop and every time the program goes on: an answer
    /// asked at another generation describes another stop.
    generation: u64,
    frames: Frames,
    returned: Option<ReturnedValue>,
    references: BTreeMap<i64, Target>,
    next_reference: i64,
}

impl Stop {
    pub(crate) fn new() -> Stop {
        Stop {
            generation: 0,
            frames: Frames::Missing,
            returned: None,
            references: BTreeMap::new(),
            next_reference: 1,
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// The program stopped; `returned` is the value its step's last call
    /// answered, with the call.
    pub(crate) fn begin(&mut self, returned: Option<ReturnedValue>) {
        self.clear();
        self.returned = returned;
    }

    /// The program went on: nothing of the stop is valid any more.
    pub(crate) fn end(&mut self) {
        self.clear();
    }

    fn clear(&mut self) {
        self.generation += 1;
        self.frames = Frames::Missing;
        self.returned = None;
        self.references.clear();
    }

    pub(crate) fn frames(&self) -> &Frames {
        &self.frames
    }

    pub(crate) fn frames_asked(&mut self) {
        self.frames = Frames::Asked;
    }

    pub(crate) fn frames_answered(&mut self, frames: Vec<FrameDescription>) {
        self.frames = Frames::Known(frames);
    }

    pub(crate) fn frames_refused(&mut self, reason: String) {
        self.frames = Frames::Refused(reason);
    }

    /// The number of frames and, of them, the `levels` from `start` (every
    /// one for `levels` 0) with their indices; empty unless the frames are
    /// known.
    pub(crate) fn page(
        &self,
        start: usize,
        levels: usize,
    ) -> (usize, Vec<(usize, FrameDescription)>) {
        let Frames::Known(frames) = &self.frames else {
            return (0, Vec::new());
        };
        let levels = if levels == 0 { frames.len() } else { levels };
        let page = frames
            .iter()
            .enumerate()
            .skip(start)
            .take(levels)
            .map(|(index, frame)| (index, frame.clone()))
            .collect();
        (frames.len(), page)
    }

    /// Whether the frame at `index` exists; `true` while the frames are not
    /// known, the VM then deciding.
    pub(crate) fn has_frame(&self, index: usize) -> bool {
        match &self.frames {
            Frames::Known(frames) => index < frames.len(),
            _ => true,
        }
    }

    pub(crate) fn returned(&self) -> Option<&ReturnedValue> {
        self.returned.as_ref()
    }

    /// A new reference to `target`, valid until the program goes on.
    pub(crate) fn reference(&mut self, target: Target) -> i64 {
        let reference = self.next_reference;
        self.next_reference += 1;
        self.references.insert(reference, target);
        reference
    }

    pub(crate) fn target(&self, reference: i64) -> Option<Target> {
        self.references.get(&reference).cloned()
    }

    /// A variable as a client shows it, a reference handed out for a value
    /// with children. A field not computed yet says so in its value, with
    /// no presentation hint: DAP's `lazy` hint promises a value fetched
    /// through the variable's reference, and a stop computes none.
    pub(crate) fn variable(&mut self, variable: &Variable) -> protocol::Variable {
        let value = match &variable.value {
            VariableValue::Shown(value) => value,
            VariableValue::Unavailable => return Self::without_value(variable, "not available"),
            VariableValue::NotComputed => {
                return Self::without_value(variable, "<not computed yet>")
            }
        };
        let indexed = value.indexed;
        let variables_reference = if value.child_count > 0 {
            self.reference(Target::Value {
                id: value.id,
                child_count: value.child_count,
                indexed,
            })
        } else {
            0
        };
        protocol::Variable {
            name: variable.name.clone(),
            value: value.label.clone(),
            class_name: Some(value.class_name.clone()),
            variables_reference,
            indexed_variables: indexed.then_some(value.child_count),
        }
    }

    /// A variable that holds no value, its text saying why.
    fn without_value(variable: &Variable, text: &str) -> protocol::Variable {
        protocol::Variable {
            name: variable.name.clone(),
            value: text.to_string(),
            class_name: None,
            variables_reference: 0,
            indexed_variables: None,
        }
    }
}

/// The frame at `index` of the stack trace as a `stackTrace` response
/// shows it. Its range is the selector of the send it waits on, for a
/// frame below the top, and its statement otherwise, in its module's
/// current text, which `sources` holds.
pub(crate) fn stack_frame(
    index: usize,
    frame: &FrameDescription,
    sources: &mut Sources,
) -> protocol::StackFrame {
    let place = frame.location.as_ref().and_then(|location| {
        let selector = location.send.as_ref().and_then(|send| {
            let first = send.selector.first()?;
            let last = send.selector.last()?;
            Some(Span::new(first.start, last.end))
        });
        let (start, end) =
            sources.range(&location.module, selector.unwrap_or(location.statement))?;
        Some((sources.source(&location.module), start, end))
    });
    let id = i64::try_from(index + 1).unwrap_or(i64::MAX);
    let presentation_hint = frame.hidden.then_some("subtle");
    match place {
        Some((source, start, end)) => protocol::StackFrame {
            id,
            name: frame.name.clone(),
            source: Some(source),
            line: start.line + 1,
            column: start.column + 1,
            end_line: Some(end.line + 1),
            end_column: Some(end.column + 1),
            presentation_hint,
        },
        None => protocol::StackFrame {
            id,
            name: frame.name.clone(),
            source: None,
            line: 0,
            column: 0,
            end_line: None,
            end_column: None,
            presentation_hint,
        },
    }
}

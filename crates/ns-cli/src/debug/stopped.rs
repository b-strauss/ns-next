//! What the agent keeps of one stop: its frames, once the VM answered
//! them, the value its step's last call returned, and the
//! `variablesReference` ids it handed out, each naming a frame's
//! variables or a value's children, and the value references, each naming
//! a value a mark or a forced return takes. Everything here is valid
//! until the program goes on; the ids continue across the stops of a
//! session, so that a reference of an earlier stop names nothing rather
//! than a value of this one.

use std::collections::BTreeMap;

use ns_runtime::{
    FrameDescription, ReturnedValue, ValueDescription, ValueId, Variable, VariableValue,
};
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
    /// The value references handed out, numbered from the same count as
    /// the `variablesReference`s, so that no number names both.
    values: BTreeMap<i64, ValueId>,
    /// The value reference of each value the stop handed out, by its id,
    /// so that a value shown twice -- a variable read again, an
    /// evaluation that answers it -- keeps one reference.
    value_references: BTreeMap<u32, i64>,
    next_reference: i64,
}

impl Stop {
    pub(crate) fn new() -> Stop {
        Stop {
            generation: 0,
            frames: Frames::Missing,
            returned: None,
            references: BTreeMap::new(),
            values: BTreeMap::new(),
            value_references: BTreeMap::new(),
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
        self.values.clear();
        self.value_references.clear();
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

    /// The value the value reference `reference` names (`shown`).
    pub(crate) fn value(&self, reference: i64) -> Option<ValueId> {
        self.values.get(&reference).copied()
    }

    /// A variable as a client shows it (`shown`). A field not computed
    /// yet says so in its value, with no presentation hint: DAP's `lazy`
    /// hint promises a value fetched through the variable's reference,
    /// and a stop computes none.
    pub(crate) fn variable(&mut self, variable: &Variable) -> protocol::Variable {
        let (value, shown) = match &variable.value {
            VariableValue::Shown(value) => self.shown(value),
            VariableValue::Unavailable => Self::without_value("not available"),
            VariableValue::NotComputed => Self::without_value("<not computed yet>"),
        };
        protocol::Variable {
            name: variable.name.clone(),
            value,
            shown,
        }
    }

    /// A value as a client shows it: its text, and a `variablesReference`
    /// handed out for one with children and a value reference, which
    /// marks and a forced return name it by, for one that is not withheld
    /// -- a withheld one is read-only instead. A marked value's text ends
    /// in its label, in brackets.
    pub(crate) fn shown(&mut self, value: &ValueDescription) -> (String, protocol::Shown) {
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
        let value_reference = (!value.withheld).then(|| {
            if let Some(reference) = self.value_references.get(&value.id.0) {
                return *reference;
            }
            let reference = self.next_reference;
            self.next_reference += 1;
            self.values.insert(reference, value.id);
            self.value_references.insert(value.id.0, reference);
            reference
        });
        let text = match &value.mark {
            Some(label) => format!("{} [{label}]", value.label),
            None => value.label.clone(),
        };
        let shown = protocol::Shown {
            class_name: Some(value.class_name.clone()),
            variables_reference,
            indexed_variables: indexed.then_some(value.child_count),
            presentation_hint: value.withheld.then(|| protocol::VariablePresentationHint {
                kind: None,
                attributes: vec!["readOnly"],
            }),
            value_reference,
        };
        (text, shown)
    }

    /// A referrer on the stack (`Referrer::Stack`): the variable `name` of
    /// the frame at `frame`, named `frame <id> <name>` by the frame's DAP
    /// id. What it holds is the value its referrers were asked of, so it
    /// shows the frame's name, when the stop's frames are known, and
    /// neither a value reference nor children: a `virtual` variable,
    /// naming a place.
    pub(crate) fn stack_referrer(&self, frame: usize, name: &str) -> protocol::Variable {
        let frame_name = match &self.frames {
            Frames::Known(frames) => frames
                .get(frame)
                .map(|known| known.name.clone())
                .unwrap_or_default(),
            _ => String::new(),
        };
        let (_, mut shown) = Self::without_value("");
        shown.presentation_hint = Some(protocol::VariablePresentationHint {
            kind: Some("virtual"),
            attributes: Vec::new(),
        });
        protocol::Variable {
            name: format!("frame {} {name}", frame + 1),
            value: frame_name,
            shown,
        }
    }

    /// What a variable that holds no value shows, its text saying why.
    fn without_value(text: &str) -> (String, protocol::Shown) {
        let shown = protocol::Shown {
            class_name: None,
            variables_reference: 0,
            indexed_variables: None,
            presentation_hint: None,
            value_reference: None,
        };
        (text.to_string(), shown)
    }

    /// The index in the stack trace of the frame whose DAP id is
    /// `frame_id`, its index plus 1, as `stack_frame` gives it; `None` for
    /// an id no frame of this stop has.
    pub(crate) fn frame_of_id(&self, frame_id: i64) -> Option<usize> {
        frame_id
            .checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| self.has_frame(*index))
    }
}

/// The frame at `index` of the stack trace as a `stackTrace` response
/// shows it. Its range is the selector of the send it waits on, for a
/// frame below the top, and its statement otherwise, in its module's
/// current text, which `sources` holds. A shipped frame is `subtle`, its
/// source `deemphasize`d, so that a client folds it away
/// (`docs/design/debugger.md`, "Folding the shipped frames"), as it
/// shows a hidden frame only on demand.
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
        let mut source = sources.source(&location.module);
        source.presentation_hint = frame.shipped.then_some("deemphasize");
        Some((source, start, end))
    });
    let mut shown = protocol::StackFrame {
        id: i64::try_from(index + 1).unwrap_or(i64::MAX),
        name: frame.name.clone(),
        source: None,
        line: 0,
        column: 0,
        end_line: None,
        end_column: None,
        presentation_hint: (frame.hidden || frame.shipped).then_some("subtle"),
        // The VM refuses to restart a frame of the core library.
        can_restart: frame.shipped.then_some(false),
        shipped: frame.shipped,
    };
    if let Some((source, start, end)) = place {
        shown.source = Some(source);
        shown.line = start.line + 1;
        shown.column = start.column + 1;
        shown.end_line = Some(end.line + 1);
        shown.end_column = Some(end.column + 1);
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn described(id: u32, withheld: bool) -> ValueDescription {
        ValueDescription {
            id: ValueId(id),
            label: "an Object".to_string(),
            class_name: "Object".to_string(),
            child_count: 1,
            indexed: false,
            withheld,
            mark: None,
        }
    }

    /// A value shown twice at one stop keeps its value reference, which
    /// names it and nothing else; the next stop hands out new ones.
    #[test]
    fn a_value_keeps_one_value_reference_per_stop() {
        let mut stop = Stop::new();
        stop.begin(None);
        let first = stop.shown(&described(4, false)).1.value_reference;
        let again = stop.shown(&described(4, false)).1.value_reference;
        let other = stop.shown(&described(5, false)).1.value_reference;
        assert_eq!(first, again);
        assert_ne!(first, other);
        let reference = first.expect("a value reference");
        assert_eq!(stop.value(reference), Some(ValueId(4)));
        assert_eq!(stop.shown(&described(6, true)).1.value_reference, None);
        stop.end();
        assert_eq!(stop.value(reference), None);
        stop.begin(None);
        let later = stop.shown(&described(4, false)).1.value_reference;
        assert_ne!(later, first, "a reference of an earlier stop names nothing");
    }
}

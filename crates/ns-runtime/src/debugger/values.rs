//! The values a stop hands out (`docs/design/debugger.md`, "Values the IDE
//! holds"): each held as a `Root` in the stop's table and named to the
//! agent by a `ValueId`, described by the heap's accessors alone -- no
//! Newspeak code runs while the program is stopped.

use super::remote::{ValueDescription, ValueId, Variable};
use crate::heap::ObjectShape;
use crate::interpreter::Mutator;
use crate::{Root, Value, ValueKind};

/// The values one stop handed out, by id. Its ids continue those of the
/// stop before (`first`), so that an id of an earlier stop, whose table
/// is gone, names nothing here rather than another value; the table goes
/// with its stop, releasing every `Root`.
pub(crate) struct StopValues {
    /// The id of the table's first value.
    first: u32,
    roots: Vec<Root>,
}

impl StopValues {
    /// An empty table whose first id is `first`.
    pub(crate) fn starting_at(first: u32) -> StopValues {
        StopValues {
            first,
            roots: Vec::new(),
        }
    }

    /// The id the next stop's table starts at: past every id this one
    /// handed out.
    /// Ids wrap only after 2^32 values handed out across the stops of
    /// one attachment, far past what an agent asks for.
    pub(crate) fn next_first(&self) -> u32 {
        let length = u32::try_from(self.roots.len()).unwrap_or(u32::MAX);
        self.first.wrapping_add(length)
    }

    /// `value`, held for the stop and described for the agent.
    pub(crate) fn describe<'gc>(
        &mut self,
        mutator: &Mutator<'gc>,
        value: Value<'gc>,
    ) -> ValueDescription {
        let id = ValueId(self.next_first());
        self.roots.push(mutator.heap.stash(value));
        ValueDescription {
            id,
            label: mutator.heap.describe(value),
            class_name: mutator.heap.class_display_name(mutator.class_of(value)),
            child_count: child_count(mutator, value),
        }
    }

    /// The value `id` names, if this stop handed it out.
    fn fetch<'gc>(&self, mutator: &Mutator<'gc>, id: ValueId) -> Option<Value<'gc>> {
        let index = usize::try_from(id.0.wrapping_sub(self.first)).ok()?;
        self.roots.get(index).map(|root| mutator.heap.fetch(root))
    }

    /// `count` children of the value `id` names, from the `start`th: an
    /// instance's fields, an `Array`'s elements; `None` for an id this
    /// stop did not hand out.
    pub(crate) fn children<'gc>(
        &mut self,
        mutator: &Mutator<'gc>,
        id: ValueId,
        start: usize,
        count: usize,
    ) -> Option<Vec<Variable>> {
        let value = self.fetch(mutator, id)?;
        let total = child_count(mutator, value);
        let end = start.saturating_add(count).min(total);
        let ValueKind::Object(object) = value.kind() else {
            return Some(Vec::new());
        };
        let is_array = mutator.heap.object_shape(object) == ObjectShape::Array;
        let mut children = Vec::new();
        for index in start..end {
            let (name, child) = if is_array {
                (
                    format!("[{}]", index + 1),
                    mutator.heap.array_element(object, index),
                )
            } else {
                (
                    format!("field {index}"),
                    mutator.heap.instance_field(object, index),
                )
            };
            let Some(child) = child else {
                unreachable!("internal: a child below the count is readable")
            };
            children.push(Variable {
                name,
                value: Some(self.describe(mutator, child)),
            });
        }
        Some(children)
    }
}

/// How many children `value` shows: an instance's fields, an `Array`'s
/// elements, none for anything else -- a reified mixin's one field is an
/// id, which tells a reader nothing (`Heap::describe`).
fn child_count<'gc>(mutator: &Mutator<'gc>, value: Value<'gc>) -> usize {
    let ValueKind::Object(object) = value.kind() else {
        return 0;
    };
    let count = match mutator.heap.object_shape(object) {
        ObjectShape::Instance => mutator.heap.instance_field_count(object),
        ObjectShape::Array => mutator.heap.array_element_count(object),
        _ => None,
    };
    count.unwrap_or(0)
}

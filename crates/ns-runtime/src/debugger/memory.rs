//! The memory view (`docs/design/debugger.md`, "The memory view"): at a
//! stop, how many instances each class declaration has, a declaration's
//! instances, and the objects that refer to one, each answered by a walk
//! of the object space (`Heap::each_object`). No Newspeak code runs at a
//! stop and nothing collects, so the walk finds every object the space
//! holds -- those nothing reaches any more too -- and every object it
//! hands out is a value of the stop's, described as the variables view
//! describes one (`StopValues::describe`).

use std::collections::HashMap;
use std::rc::Rc;

use super::metadata::again::declaration_path;
use super::remote::{ClassName, InstanceCount, ModuleName, Referrer, Variable, VariableValue};
use super::stack::{self, StackFrame, is_named_register};
use super::values::StopValues;
use crate::heap::{ActivationReference, ClassReference, ObjectReference};
use crate::interpreter::Mutator;
use crate::mixin::Mixin;
use crate::{Root, Value};

/// The declaration a class object is the class of, as the memory view
/// groups by it: by mixin, never by class object, of which there is one
/// per enclosing instance (`docs/design/object-model.md`, "Three layers,
/// not one").
#[derive(Clone)]
struct Declaration {
    /// Its name, as a step filter names it; `None` for a class side, whose
    /// declaration is its instance side's and whose instances are class
    /// objects, which the view does not count.
    name: Option<ClassName>,
    /// Whether a shipped module declares it: its instances are withheld.
    shipped: bool,
}

/// The declarations of the class objects one walk met, by the address of
/// the declaring mixin: every instance of one class asks the same
/// question, and every class object of one declaration answers alike.
#[derive(Default)]
struct Declarations {
    known: HashMap<*const Mixin, Option<Declaration>>,
}

impl Declarations {
    /// The declaration `class` is the class of (`Heap::class_declaration`),
    /// named as a step filter names one -- a builtin class, `String` or
    /// `Array`, under its kernel declaration -- and unnamed for a class
    /// side and a mixin no heap registered; `None` for a class object
    /// without an application.
    fn of_class<'gc>(
        &mut self,
        mutator: &Mutator<'gc>,
        class: ClassReference<'gc>,
    ) -> Option<Declaration> {
        let mixin = mutator.heap.class_declaration(class)?;
        self.known
            .entry(Rc::as_ptr(&mixin))
            .or_insert_with(|| {
                let name = if mixin.is_class_side() {
                    None
                } else {
                    declaration_path(mutator, &mixin).map(|(root, path)| ClassName {
                        module: ModuleName::of_root(&root),
                        path: path.iter().map(ToString::to_string).collect(),
                    })
                };
                // A class side a metaclass synthesizes for a declaration
                // without one is no shipped mixin of its own, but its
                // class objects are a shipped declaration's.
                let shipped = mixin.shipped
                    || (mixin.is_class_side()
                        && mixin
                            .enclosing_mixin()
                            .is_some_and(|instance_side| instance_side.shipped));
                Some(Declaration { name, shipped })
            })
            .clone()
    }
}

/// The name of the declaration `object` is an instance of, as
/// `Declarations::of_class` names it, and whether a shipped module
/// declares it; `None` for an object the view does not count -- one of a
/// class side or of no named declaration, and the VM mirror, whose class
/// has no declaration and so no name (`docs/design/core-library.md`, "The
/// VM mirror is the whole Rust boundary"), though the mixin its class
/// applies has one.
fn declared<'gc>(
    mutator: &Mutator<'gc>,
    declarations: &mut Declarations,
    object: ObjectReference<'gc>,
) -> Option<(ClassName, bool)> {
    if mutator.is_vm_mirror(Value::object(object)) {
        return None;
    }
    let declaration = declarations.of_class(mutator, mutator.heap.object_class(object))?;
    Some((declaration.name?, declaration.shipped))
}

/// How many instances each class declaration has, one count per
/// declaration with instances, ordered by count, the largest first, then
/// by name (`Command::InstanceCounts`).
pub(crate) fn instance_counts(mutator: &Mutator<'_>) -> Vec<InstanceCount> {
    let mut declarations = Declarations::default();
    let mut counts: Vec<InstanceCount> = Vec::new();
    // Each count's index in `counts`, by its name's parts.
    let mut indices: HashMap<(ModuleName, Vec<String>), usize> = HashMap::new();
    mutator.heap.each_object(|object| {
        let Some((name, _)) = declared(mutator, &mut declarations, object) else {
            return;
        };
        let key = (name.module.clone(), name.path.clone());
        let index = *indices.entry(key).or_insert_with(|| {
            counts.push(InstanceCount {
                class: name,
                count: 0,
            });
            counts.len() - 1
        });
        counts[index].count += 1;
    });
    counts.sort_by(|first, second| {
        second.count.cmp(&first.count).then_with(|| {
            (&first.class.module, &first.class.path)
                .cmp(&(&second.class.module, &second.class.path))
        })
    });
    counts
}

/// `count` of the instances of the declaration `class` names, from the
/// `start`th, each named `#<n>`, counted from 1 over all of them
/// (`Command::Instances`): grouped by application, in the order the walk
/// first meets each, and within one in the space's order. An instance of
/// a declaration a shipped module declares is withheld. Empty for a
/// name no declaration with instances has.
pub(crate) fn instances<'gc>(
    mutator: &Mutator<'gc>,
    values: &mut StopValues,
    class: &ClassName,
    start: usize,
    count: usize,
) -> Vec<Variable> {
    let mut declarations = Declarations::default();
    let mut shipped = false;
    // The instances found, by application, in the order first met.
    let mut groups: Vec<(Option<usize>, Vec<ObjectReference<'gc>>)> = Vec::new();
    mutator.heap.each_object(|object| {
        let Some((name, declared_shipped)) = declared(mutator, &mut declarations, object) else {
            return;
        };
        if name != *class {
            return;
        }
        shipped = declared_shipped;
        let application = mutator
            .heap
            .class_application_index(mutator.heap.object_class(object))
            .map(|index| index.index());
        match groups.iter_mut().find(|(group, _)| *group == application) {
            Some((_, objects)) => objects.push(object),
            None => groups.push((application, vec![object])),
        }
    });
    let found: Vec<ObjectReference<'gc>> = groups
        .into_iter()
        .flat_map(|(_, objects)| objects)
        .collect();
    numbered(mutator, values, &found, start, count, |_| shipped)
}

/// `count` of what refers to the value `target` holds, from the
/// `start`th (`Command::Referrers`): first every variable of a running
/// frame of `frames` that holds it, innermost frame first and each
/// frame's in the order the variables view shows them, `self` first --
/// read by the variables view's own reader (`stack::frame_values`), so
/// that a register the leftover analysis finds tainted and a scratch
/// register, which no variable names and which may hold a word a callee
/// or a popped frame left, count for none, and no frame is reified --
/// then, in the space's order, every object a word of which that keeps
/// what it names alive names it (`Heap::refers_to`): a closure by its
/// copied values or its captured frame, a suspended or returned frame
/// object by its fields and registers. A frame object married to a
/// running frame of `frames` is that frame's variables, not an object
/// of its own; one married to an activation below the run, which the
/// stack does not show, is an object whose live registers count, read
/// as the variables view reads them (`registers_refer_to`). An object is
/// named `#<n>` by its place in the whole answer and withheld as
/// `object_is_withheld` says. Empty for an immediate value, which no
/// word names.
pub(crate) fn referrers(
    mutator: &mut Mutator<'_>,
    frames: &[StackFrame],
    values: &mut StopValues,
    target: &Root,
    start: usize,
    count: usize,
) -> Vec<Referrer> {
    let mut on_stack: Vec<Referrer> = Vec::new();
    let mut running: Vec<ActivationReference> = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        let StackFrame::Running(activation) = frame else {
            continue;
        };
        running.push(*activation);
        // Read afresh for each frame: reading one may compile its body's
        // metadata, across which no handle is held.
        let Some((_, read)) = stack::frame_values(mutator, frames, index) else {
            continue;
        };
        let Some(object) = mutator.heap.fetch(target).as_object() else {
            return Vec::new();
        };
        on_stack.extend(read.into_iter().filter_map(|(name, value)| {
            value?
                .as_object()
                .is_some_and(|held| held.same(object))
                .then_some(Referrer::Stack { frame: index, name })
        }));
    }
    let mutator = &*mutator;
    let heap = &mutator.heap;
    let Some(object) = heap.fetch(target).as_object() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    heap.each_object(|holder| {
        let married_to_the_stack = heap
            .object_frame(holder)
            .and_then(|frame| heap.frame_activation(frame))
            .is_some_and(|activation| running.iter().any(|other| other.same(activation)));
        if !married_to_the_stack
            && (heap.refers_to(holder, object) || registers_refer_to(mutator, holder, object))
        {
            found.push(holder);
        }
    });
    let stack_count = on_stack.len();
    let mut declarations = Declarations::default();
    let objects = found
        .into_iter()
        .enumerate()
        .map(|(index, holder)| (stack_count + index, holder));
    let mut answer: Vec<Referrer> = on_stack.into_iter().skip(start).take(count).collect();
    let remaining = count - answer.len();
    let first_object = start.saturating_sub(stack_count);
    for (place, holder) in objects.skip(first_object).take(remaining) {
        let withheld = object_is_withheld(mutator, &mut declarations, holder);
        answer.push(Referrer::Object(Variable {
            name: format!("#{}", place + 1),
            value: VariableValue::Shown(values.describe(mutator, Value::object(holder), withheld)),
        }));
    }
    answer
}

/// Whether the memory view withholds `object`, a referrer: a closure or
/// a frame object when the body it captured or runs is a shipped
/// module's, as the variables view withholds a shipped frame's values
/// (`stack::frame_object_is_shipped`); any other object when a shipped
/// module declares its class, or when its class has no declaration --
/// the VM mirror's class among such, which no program may name -- so
/// that what the view cannot place is never handed out markable.
/// `Mutator::is_withheld` applies besides (`StopValues::describe`).
fn object_is_withheld<'gc>(
    mutator: &Mutator<'gc>,
    declarations: &mut Declarations,
    object: ObjectReference<'gc>,
) -> bool {
    let heap = &mutator.heap;
    if let Some(frame) = heap.object_frame(object) {
        return stack::frame_object_is_shipped(mutator, frame);
    }
    if let Some(frame) = heap.closure_captured_frame(object) {
        return stack::frame_object_is_shipped(mutator, frame);
    }
    declarations
        .of_class(mutator, heap.object_class(object))
        .is_none_or(|declaration| declaration.shipped)
}

/// Whether `holder` is a frame object married to a running activation
/// one of whose named registers -- an argument, or one past the header's
/// numbers below the first scratch register -- holds `target`, read as
/// the variables view reads a frame's: only a register the leftover
/// analysis does not find tainted at the activation's program counter,
/// since a tainted one may hold a word a callee or a popped frame left.
fn registers_refer_to<'gc>(
    mutator: &Mutator<'gc>,
    holder: ObjectReference<'gc>,
    target: ObjectReference<'gc>,
) -> bool {
    let heap = &mutator.heap;
    let Some(frame) = heap.object_frame(holder) else {
        return false;
    };
    let Some(activation) = heap.frame_activation(frame) else {
        return false;
    };
    let code = heap.activation_code(activation);
    let Ok(program_counter) = u32::try_from(heap.activation_program_counter(activation)) else {
        return false;
    };
    let tainted = code.tainted_before(program_counter);
    (0..code.local_count)
        .filter(|register| is_named_register(&code, *register))
        .filter(|register| !tainted.iter().any(|range| range.contains(register)))
        .any(|register| {
            heap.frame_local(frame, register)
                .as_object()
                .is_some_and(|object| object.same(target))
        })
}

/// `count` of `objects` from the `start`th, each named by its place among
/// them, `#1` the first, and described for the stop, withheld where
/// `withheld` says so.
fn numbered<'gc>(
    mutator: &Mutator<'gc>,
    values: &mut StopValues,
    objects: &[ObjectReference<'gc>],
    start: usize,
    count: usize,
    mut withheld: impl FnMut(ObjectReference<'gc>) -> bool,
) -> Vec<Variable> {
    objects
        .iter()
        .enumerate()
        .skip(start)
        .take(count)
        .map(|(index, object)| Variable {
            name: format!("#{}", index + 1),
            value: VariableValue::Shown(values.describe(
                mutator,
                Value::object(*object),
                withheld(*object),
            )),
        })
        .collect()
}

//! Platform paths: how an image names an object the loading binary already
//! has, reached through the platform (`docs/design/core-library.md`,
//! "Platform paths in the image"; `docs/design/image-format.md`, "The
//! platform path section").
//!
//! A path is the list of public members sent from the platform, one unary
//! send each: `transcript` is the platform's transcript, `collections` its
//! collections module. The encoder writes the paths its graph uses into a
//! section of their own and names each such object by its index there; the
//! loader sends each path's members to its own platform before the graph is
//! decoded, so the decoder only looks the answers up and still runs no code.
//!
//! Shipped objects are named rather than written because their code and
//! layout belong to the loading binary: a module instance holds the kernel,
//! as the platform holds the mirror, and a class compiled from shipped
//! source exists only once its accessor has evaluated its superclass
//! clause -- Newspeak the decoder must never run.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use ns_syntax::ast::AccessModifier;

use crate::dispatch::{self, Lookup};
use crate::heap::Heap;
use crate::mixin::{mixin_key, MethodEntry, MixinApplication};
use crate::types::{ObjectReference, Value};
use crate::Symbol;

use super::decode::{reserve_for, DecodeError, Reader};
use super::encode::{u32_field, Writer};
use super::symbols;

/// The most paths one image may list. A path costs the loader one send per
/// member, so the section's count is bounded before anything is sent, as
/// every other count an image carries is bounded before it allocates.
pub const MAX_PLATFORM_PATHS: usize = 4096;

/// The most members one path may have. The walk never records a longer one,
/// so an object reachable only further out is not named by a path.
pub const MAX_PLATFORM_PATH_LENGTH: usize = 32;

/// Every object `discover` found, with the path that reaches it. Keyed by
/// `Heap::object_identity_key`, so a handle is looked up by identity.
pub struct PlatformPaths<'gc> {
    paths: Vec<Vec<Symbol>>,
    objects: Vec<ObjectReference<'gc>>,
    ids: HashMap<usize, u32>,
}

impl<'gc> PlatformPaths<'gc> {
    /// No paths: what a world without a platform object is named with.
    pub fn empty() -> PlatformPaths<'gc> {
        PlatformPaths {
            paths: Vec::new(),
            objects: Vec::new(),
            ids: HashMap::new(),
        }
    }

    /// The id of the path that reaches `object`, if the walk reached it.
    pub fn id_of(&self, heap: &Heap<'gc>, object: ObjectReference<'gc>) -> Option<u32> {
        self.ids.get(&heap.object_identity_key(object)).copied()
    }

    /// The members of path `id`, which `id_of` answered.
    pub fn path(&self, id: u32) -> &[Symbol] {
        &self.paths[id as usize]
    }

    /// The object path `id` reaches, which `id_of` answered.
    pub fn object(&self, id: u32) -> ObjectReference<'gc> {
        self.objects[id as usize]
    }

    fn record(&mut self, heap: &Heap<'gc>, object: ObjectReference<'gc>, path: Vec<Symbol>) {
        let id = u32_field("platform path id", self.paths.len());
        self.ids.insert(heap.object_identity_key(object), id);
        self.paths.push(path);
        self.objects.push(object);
    }
}

/// Whether `object` is what a platform path may answer: a class object whose
/// own mixin is shipped, or an ordinary instance -- one with slots -- of a
/// shipped class. A string, an array or another object that carries its own
/// data is none, even when its class is a builtin compiled from `Kernel.ns`:
/// the image writes such an object as a record of its own, and a path that
/// answers one names no object the loading binary owns.
pub fn is_shipped_object<'gc>(heap: &Heap<'gc>, object: ObjectReference<'gc>) -> bool {
    if heap.is_class(object) {
        heap.class_is_shipped(object)
    } else {
        heap.instance_field_count(object).is_some()
            && heap.class_is_shipped(heap.class_of_object(object))
    }
}

/// Walks from `platform` over, for every level of an object's class, each
/// public slot's field -- a lazy slot's included, skipped while `nil` -- and
/// each public nested class's cache slot, skipped while empty, and then over
/// each public class accessor the object answers (`MixinMethod::
/// class_accessor`), reading, without running it, the cache slot of the
/// nested class it answers in its module root's instance, skipped while
/// empty. It records every shipped object it reaches (`is_shipped_object`,
/// the one rule the encoder and the loader apply too, so no path is written
/// that a loader must refuse). It descends into shipped instances only,
/// never into a class object.
///
/// Breadth-first, visiting each object once, so an object's recorded path is
/// its shortest, and among paths of one length the first in member order --
/// superclass level first, within a level its slots and then its nested
/// classes as declared, then the class accessors by selector -- which keeps
/// the encoding canonical.
pub fn discover<'gc>(heap: &Heap<'gc>, platform: ObjectReference<'gc>) -> PlatformPaths<'gc> {
    let mut found = PlatformPaths::empty();
    let mut visited: HashSet<usize> = HashSet::new();
    visited.insert(heap.object_identity_key(platform));
    let mut queue: VecDeque<(ObjectReference<'gc>, Vec<Symbol>)> = VecDeque::new();
    queue.push_back((platform, Vec::new()));
    while let Some((object, path)) = queue.pop_front() {
        if path.len() >= MAX_PLATFORM_PATH_LENGTH {
            continue;
        }
        for (member, value) in public_members(heap, object) {
            let Some(target) = value.as_object() else {
                continue;
            };
            let key = heap.object_identity_key(target);
            if visited.contains(&key) {
                continue;
            }
            if !is_shipped_object(heap, target) {
                continue;
            }
            let is_class = heap.is_class(target);
            visited.insert(key);
            let mut extended = path.clone();
            extended.push(member);
            found.record(heap, target, extended.clone());
            if !is_class {
                queue.push_back((target, extended));
            }
        }
    }
    found
}

/// Whether a path may take the step `member` from `object`: `object` is a
/// shipped instance, which is all `discover` descends into, and `member` is
/// one of its public members `discover` walks -- a public slot's getter, a
/// lazy slot's included, a public nested class's accessor, or a public
/// class accessor answering a nested class of its module's root. The loader
/// asks this before every send, so a crafted path can run no other method:
/// `transcript newLine` is refused rather than printed.
pub fn is_path_step<'gc>(heap: &Heap<'gc>, object: ObjectReference<'gc>, member: Symbol) -> bool {
    !heap.is_class(object)
        && is_shipped_object(heap, object)
        && public_members(heap, object)
            .iter()
            .any(|(name, _)| *name == member)
}

/// `object`'s public members a path may step through, with what each holds:
/// every level's public slots, then its public nested classes' cache slots,
/// superclass level first, and then its class accessors
/// (`class_accessor_members`). A member holding `nil` is answered as such,
/// and the caller skips it.
fn public_members<'gc>(
    heap: &Heap<'gc>,
    object: ObjectReference<'gc>,
) -> Vec<(Symbol, Value<'gc>)> {
    let mut members = state_members(heap, object);
    members.extend(class_accessor_members(heap, object));
    members
}

/// The public class accessors `object` answers, by selector, each with the
/// nested class it answers as its module root's instance caches it -- `nil`
/// until something has asked for the class. Each is decided as the send
/// would be, without running anything: the lookup a public send makes must
/// find a method the compiler recognized as a class accessor
/// (`MixinMethod::class_accessor`), whose `outer <Root>` must reach a module
/// root's shipped instance, and the lexical send of `<Nested>` there must
/// find that root's nested class. Anything else -- an override that is no
/// class accessor, a root that is none -- is no member at all.
fn class_accessor_members<'gc>(
    heap: &Heap<'gc>,
    object: ObjectReference<'gc>,
) -> Vec<(Symbol, Value<'gc>)> {
    let class = heap.class_of_object(object);
    let Some(application) = heap.class_application(class) else {
        return Vec::new();
    };
    let mut selectors: Vec<Symbol> = Vec::new();
    let mut level = Some(&application);
    while let Some(current) = level {
        for entry in current.mixin.methods.values() {
            if let MethodEntry::Compiled(method) = entry {
                if method.class_accessor.is_some() && method.access == AccessModifier::Public {
                    selectors.push(method.selector);
                }
            }
        }
        level = current.superclass.as_ref();
    }
    selectors.sort_by(|first, second| first.as_str().cmp(second.as_str()));
    selectors.dedup();
    selectors
        .into_iter()
        .filter_map(|selector| {
            class_accessor_target(heap, object, class, selector).map(|value| (selector, value))
        })
        .collect()
}

/// What the class accessor `selector` of `object` answers, read off its
/// module root's nested-class cache slot, when the send would run one
/// (`class_accessor_members`).
fn class_accessor_target<'gc>(
    heap: &Heap<'gc>,
    object: ObjectReference<'gc>,
    class: crate::types::ClassReference<'gc>,
    selector: Symbol,
) -> Option<Value<'gc>> {
    let Lookup::Method(method, defining_class) = dispatch::lookup_public(heap, class, selector)
    else {
        return None;
    };
    let accessor = method.class_accessor?;
    let root = dispatch::kth_enclosing_object(
        heap,
        Value::object(object),
        Some(&defining_class),
        accessor.depth,
    )
    .as_object()?;
    let root_class = heap.class_of_object(root);
    if heap.class_enclosing(root_class).is_some() || !is_shipped_object(heap, root) {
        return None;
    }
    // `outer <Root> <Nested>` is a lexical send (§5.9), looked up as
    // `Mutator::resolve_lexical` looks one up.
    let lexical_class = dispatch::kth_lexical_class(heap, Some(defining_class), accessor.depth);
    let found = dispatch::lexical_member(heap, lexical_class, root_class, accessor.class_name);
    let Lookup::NestedClass(nested, _) = found else {
        return None;
    };
    let index = *heap
        .class_application(root_class)?
        .nested_slot
        .get(&mixin_key(&nested))?;
    heap.instance_field(root, index)
}

/// `object`'s public members that hold state, with what each holds: every
/// level's public slots, then its public nested classes' cache slots,
/// superclass level first.
fn state_members<'gc>(heap: &Heap<'gc>, object: ObjectReference<'gc>) -> Vec<(Symbol, Value<'gc>)> {
    let Some(application) = heap.class_application(heap.class_of_object(object)) else {
        return Vec::new();
    };
    let mut levels: Vec<&Rc<MixinApplication>> = Vec::new();
    let mut level = Some(&application);
    while let Some(current) = level {
        levels.push(current);
        level = current.superclass.as_ref();
    }
    levels.reverse();
    let mut members = Vec::new();
    for level in levels {
        for (position, slot) in level.mixin.slots.iter().enumerate() {
            if slot.access != AccessModifier::Public {
                continue;
            }
            if let Some(value) = heap.instance_field(object, level.own_slot_base + position) {
                members.push((slot.name, value));
            }
        }
        for nested in &level.mixin.nested {
            if nested.access != AccessModifier::Public {
                continue;
            }
            let Some(&index) = application.nested_slot.get(&mixin_key(nested)) else {
                continue;
            };
            if let Some(value) = heap.instance_field(object, index) {
                members.push((nested.name, value));
            }
        }
    }
    members
}

/// The section: a `u32` count, then per path a `u32` member count and that
/// many symbol ids. `used` is what the graph named, in the order it first
/// reached each path, numbered densely from 0.
pub(crate) fn encode_section(
    writer: &mut Writer,
    used: &[Vec<Symbol>],
    symbols: &mut symbols::Builder,
) {
    writer.write_u32(u32_field("platform path count", used.len()));
    for path in used {
        writer.write_u32(u32_field("platform path length", path.len()));
        for member in path {
            writer.write_u32(symbols.intern(member.as_str()));
        }
    }
}

/// The inverse of `encode_section`, refusing what a loader cannot follow
/// before anything is sent: more than `MAX_PLATFORM_PATHS` paths, a path of
/// no members or of more than `MAX_PLATFORM_PATH_LENGTH`, a member that is no
/// id of the image's symbol table, and one that is no unary selector -- a
/// path's members are unary sends.
pub(crate) fn decode_section(
    reader: &mut Reader,
    symbols: &symbols::Table,
) -> Result<Vec<Vec<Symbol>>, DecodeError> {
    let count = reader.read_u32()? as usize;
    if count > MAX_PLATFORM_PATHS {
        return Err(DecodeError::TooManyPlatformPaths { count });
    }
    let mut paths = reserve_for(count, reader);
    for _ in 0..count {
        let length = reader.read_u32()? as usize;
        if length == 0 || length > MAX_PLATFORM_PATH_LENGTH {
            return Err(DecodeError::PlatformPathLength { length });
        }
        let mut path = reserve_for(length, reader);
        for _ in 0..length {
            let member = symbols.get(reader.read_u32()?)?;
            if !is_unary_selector(member.as_str()) {
                return Err(DecodeError::PlatformPathMemberNotUnary(
                    member.as_str().to_string(),
                ));
            }
            path.push(member);
        }
        paths.push(path);
    }
    Ok(paths)
}

/// A unary selector starts with a letter and has no `:` -- which also keeps
/// an operator, a keyword and the empty string out of a path.
fn is_unary_selector(text: &str) -> bool {
    text.chars().next().is_some_and(char::is_alphabetic) && !text.contains(':')
}

/// A path written out, members separated by spaces, as a send would read.
pub fn describe(path: &[Symbol]) -> String {
    path.iter()
        .map(|member| member.as_str().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HostResult, HostSignal, Interpreter, Root};

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// A module whose class `Inner`, two levels below the root, declares the
    /// class accessor `Other`, and an instance of `Inner` after `Other` has
    /// run once -- resolved with `resolve_kernel` when `shipped`, so that the
    /// accessor is recognized, and with plain `resolve` otherwise.
    fn an_inner_instance(shipped: bool) -> (Interpreter, Root) {
        let source = indoc::indoc! {"
            class Shipped = Object new () (
              public class Middle = Object new () (
                public class Inner = Object new () (
                  public Other = ( ^outer Shipped Other )
                )
                public makeInner = ( ^Inner new )
              )
              public class Other = Object new () ()
              public makeMiddle = ( ^Middle new )
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let origin = crate::SourceOrigin::File(std::path::PathBuf::from("t.ns"));
        let resolved = if shipped {
            crate::resolver::resolve_kernel(module, std::rc::Rc::from(source), origin)
        } else {
            crate::resolve(module, std::rc::Rc::from(source), origin)
        };
        let Ok(resolved) = resolved else {
            panic!("the module resolves");
        };
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let root = expect_ok(interpreter.instantiate(&class, &[]), "Shipped new");
        let middle = expect_ok(
            interpreter.send(&root, Symbol::intern("makeMiddle"), &[]),
            "makeMiddle",
        );
        let inner = expect_ok(
            interpreter.send(&middle, Symbol::intern("makeInner"), &[]),
            "makeInner",
        );
        expect_ok(
            interpreter.send(&inner, Symbol::intern("Other"), &[]),
            "Other",
        );
        (interpreter, inner)
    }

    fn other_method<'gc>(
        heap: &Heap<'gc>,
        inner: ObjectReference<'gc>,
    ) -> Rc<crate::mixin::MixinMethod> {
        let application = heap
            .class_application(heap.class_of_object(inner))
            .expect("Inner has an application");
        application
            .mixin
            .compiled_method(Symbol::intern("Other"))
            .expect("Inner declares Other")
            .clone()
    }

    /// A class accessor two levels below its root is recognized at the
    /// depth `outer Shipped` resolved to, and a path steps through it to the
    /// root instance's `Other`, the class the send answered.
    #[test]
    fn a_class_accessor_two_levels_down_is_a_path_step() {
        let (mut interpreter, inner) = an_inner_instance(true);
        interpreter.enter(|mutator| {
            let Some(inner) = mutator.fetch(&inner).as_object() else {
                panic!("Inner new is an object");
            };
            let heap = mutator.heap();
            assert_eq!(
                other_method(heap, inner).class_accessor,
                Some(crate::mixin::ClassAccessor {
                    depth: 2,
                    class_name: Symbol::intern("Other"),
                })
            );
            assert!(is_path_step(heap, inner, Symbol::intern("Other")));
            let members = class_accessor_members(heap, inner);
            let [(name, member)] = members.as_slice() else {
                panic!("one class accessor member, holding a class");
            };
            let Some(other) = member.as_object() else {
                panic!("one class accessor member, holding a class");
            };
            assert_eq!(name.as_str(), "Other");
            assert_eq!(heap.class_display_name(other), "Other");
        });
    }

    /// The same text in a module plain `resolve` resolved is an ordinary
    /// method: only shipped source is recognized, and a user module's
    /// objects are never a path step anyway.
    #[test]
    fn a_user_modules_class_accessor_is_unmarked() {
        let (mut interpreter, inner) = an_inner_instance(false);
        interpreter.enter(|mutator| {
            let Some(inner) = mutator.fetch(&inner).as_object() else {
                panic!("Inner new is an object");
            };
            let heap = mutator.heap();
            assert_eq!(other_method(heap, inner).class_accessor, None);
            assert!(class_accessor_members(heap, inner).is_empty());
            assert!(!is_path_step(heap, inner, Symbol::intern("Other")));
        });
    }
}

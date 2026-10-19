//! The module section: which top-level modules an artifact carries, where
//! each came from, and which record of the graph is its root class. A
//! module's root class, the levels its `<:` clauses add and their
//! metaclasses are ordinary class records; the section names each root by
//! its object id, and the loader holds the record it names to its module's
//! declaration and to the root rule (`root_record_defect`).

use std::rc::Rc;

use super::decode::{self, DecodeError, Reader};
use super::encode::{self, Writer};
use super::mixins;
use crate::heap::Heap;
use crate::mixin::Mixin;
use crate::types::{BuiltinClass, ClassReference, SourceOrigin};
use crate::Mutator;

/// One module an artifact carries, as a writer is handed it. `with_source`
/// asks for the module's text in the image, which the encoder reads from
/// the heap, each declaration's own (`Mixin::text`): after an edit the
/// host's file is no longer the program's text. Source travels for every
/// module or for none.
pub struct ModuleSource<'a, 'gc> {
    pub class: ClassReference<'gc>,
    pub origin: &'a SourceOrigin,
    pub with_source: bool,
}

/// Every module's origin and the object id of its root class's record
/// (`root_ids`, in the same order), in module order -- the order of the
/// mixin table's trees. A module's text is not here: the text table holds
/// its declarations' (`texts`), from which it is assembled.
pub fn encode_modules(writer: &mut Writer, modules: &[ModuleSource<'_, '_>], root_ids: &[u32]) {
    writer.write_u32(encode::u32_field("module count", modules.len()));
    for (module, root_id) in modules.iter().zip(root_ids) {
        encode::encode_source_origin(writer, module.origin);
        writer.write_u32(*root_id);
    }
}

/// The smallest a module record can be: an origin tag, an empty name's
/// length, and the root's object id.
const MIN_MODULE_BYTES: usize = 1 + 4 + 4;

/// One module as the module section describes it.
pub struct DecodedModule {
    pub origin: SourceOrigin,
    /// The object id of the graph record that is the module's root class,
    /// held to the module by `module_root_classes` once the graph exists.
    pub root: u32,
}

/// The inverse of `encode_modules`. The declared count must be the mixin
/// table's tree count, the length of `module_rows` -- each tree's first
/// row, as `mixins::read` found them; a surplus tree would be declarations
/// no module names, a missing one a module without a root -- and is
/// bounded by the bytes left before anything is reserved for it. A root's
/// id is only read here: the graph it indexes comes later
/// (`module_root_classes`).
pub fn decode_modules(
    reader: &mut Reader,
    module_rows: &[u32],
) -> Result<Vec<DecodedModule>, DecodeError> {
    let declared = reader.read_u32()?;
    let trees = u32::try_from(module_rows.len()).unwrap_or(u32::MAX);
    if declared != trees {
        return Err(DecodeError::ModuleCountMismatch { declared, trees });
    }
    let count = decode::bounded_count(declared as usize, MIN_MODULE_BYTES, reader)?;
    let mut modules = Vec::with_capacity(count);
    for _ in 0..count {
        let origin = decode::decode_source_origin(reader)?;
        let root = reader.read_u32()?;
        modules.push(DecodedModule { origin, root });
    }
    Ok(modules)
}

/// What disqualifies a class object as a module's root class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootRecordDefect {
    /// It is no class of the module's own root declaration as the
    /// outermost level: a metaclass, a level, a class of another
    /// declaration, or one with an enclosing object or declaring class.
    NotItsRecord,
    /// Its superclass chain is none the root rule builds
    /// (`resolver::root_clause_defect`).
    RootRule,
}

/// Why `class` cannot be the root class of the module whose root
/// declaration is `root_mixin`, if it cannot: the one check the encoder and
/// the decoder share, so the encoder never writes a root the loader
/// refuses.
///
/// A root class applies its module's root declaration and is the outermost
/// level: no metaclass, no level, neither an enclosing object nor a
/// declaring class. Its superclass is what the root rule builds and nothing
/// else -- `Kernel Object` or `Kernel Value` for a root with no `<:`
/// clause, and for one with its one permitted clause the level that clause
/// added, applying `Kernel Value` over `Kernel Object`. A crafted image
/// cannot bring back a root the resolver refuses to build.
pub fn root_record_defect<'gc>(
    mutator: &Mutator<'gc>,
    root_mixin: &Rc<Mixin>,
    class: ClassReference<'gc>,
) -> Option<RootRecordDefect> {
    let heap: &Heap<'gc> = &mutator.heap;
    let applies_the_root = heap
        .class_application(class)
        .is_some_and(|application| Rc::ptr_eq(&application.mixin, root_mixin));
    if !heap.is_class(class)
        || heap.class_this_class(class).is_some()
        || !applies_the_root
        || heap.class_applied_origin(class).is_some()
        || heap.class_enclosing(class).is_some()
        || heap.class_declaring(class).is_some()
    {
        return Some(RootRecordDefect::NotItsRecord);
    }
    let object = mutator.builtin_class(BuiltinClass::Object);
    let value = mutator.builtin_class(BuiltinClass::Value);
    let Some(superclass) = heap.class_superclass(class) else {
        return Some(RootRecordDefect::RootRule);
    };
    let obeys = match root_mixin.mixin_applications.len() {
        0 => superclass.same(object) || superclass.same(value),
        1 => {
            let value_mixin = heap
                .class_application(value)
                .map(|application| application.mixin.clone());
            let added_by_the_root = heap
                .class_applied_origin(superclass)
                .is_some_and(|applied| {
                    Rc::ptr_eq(&applied.declaration, root_mixin) && applied.index == 0
                });
            let applies_value = heap
                .class_application(superclass)
                .zip(value_mixin)
                .is_some_and(|(application, value_mixin)| {
                    Rc::ptr_eq(&application.mixin, &value_mixin)
                });
            added_by_the_root
                && applies_value
                && heap
                    .class_superclass(superclass)
                    .is_some_and(|below| below.same(object))
        }
        _ => false,
    };
    (!obeys).then_some(RootRecordDefect::RootRule)
}

/// Each module's root class: the record its id names in the decoded graph
/// (`objects`), held to the module's root declaration and the root rule
/// (`root_record_defect`). An id past the graph is `BadObjectId`, a record
/// that is no class of the module's declaration as its outermost level
/// `ModuleRootNotItsRecord`, a superclass chain the root rule does not
/// build `RootRule`.
pub fn module_root_classes<'gc>(
    mutator: &Mutator<'gc>,
    mixins: &mixins::Decoded,
    modules: &[DecodedModule],
    objects: &[crate::types::ObjectReference<'gc>],
) -> Result<Vec<ClassReference<'gc>>, DecodeError> {
    let mut roots = Vec::with_capacity(modules.len());
    for (index, module) in modules.iter().enumerate() {
        let module_index = u32::try_from(index).unwrap_or(u32::MAX);
        let class = objects
            .get(module.root as usize)
            .copied()
            .ok_or(DecodeError::BadObjectId(module.root))?;
        // `decode_modules` held the count to the trees, so every module has
        // its root row.
        let root_mixin = mixins.module_root(index).ok_or(DecodeError::NotAClass)?;
        match root_record_defect(mutator, root_mixin, class) {
            None => roots.push(class),
            Some(RootRecordDefect::NotItsRecord) => {
                return Err(DecodeError::ModuleRootNotItsRecord {
                    module: module_index,
                })
            }
            Some(RootRecordDefect::RootRule) => {
                return Err(DecodeError::RootRule {
                    module: module_index,
                })
            }
        }
    }
    Ok(roots)
}

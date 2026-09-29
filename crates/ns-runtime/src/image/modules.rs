//! The module section: which top-level modules an artifact carries, where
//! each came from, and what its root class inherits -- and the module
//! classes a graph names by tag (`CLASS_MODULE`, `CLASS_MODULE_LEVEL`)
//! rather than carries as records, because `load` rebuilds them from the
//! mixin table before the graph exists.

use super::decode::{self, DecodeError, Reader, RootSection};
use super::encode::{self, Writer};
use super::mixins;
use crate::heap::Heap;
use crate::types::{ClassReference, SourceOrigin};
use crate::Mutator;

/// One module an artifact carries, as a writer is handed it.
pub struct ModuleSource<'a, 'gc> {
    pub class: ClassReference<'gc>,
    pub origin: &'a SourceOrigin,
    pub source: Option<&'a str>,
}

/// The module classes an artifact names by tag rather than carries as
/// records: each module's root class and the levels its own `<:` clauses
/// added, lowest first, by module index.
pub struct ModuleClasses<'gc> {
    pub roots: Vec<ClassReference<'gc>>,
    pub levels: Vec<Vec<ClassReference<'gc>>>,
}

/// How a graph names a module class: its module's root, or one of that
/// root's levels by position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleClass {
    Root(u32),
    Level { module: u32, position: u32 },
}

impl<'gc> ModuleClasses<'gc> {
    /// The classes of the modules whose roots are `roots`, in that order.
    pub fn of(heap: &Heap<'gc>, roots: &[ClassReference<'gc>]) -> ModuleClasses<'gc> {
        ModuleClasses {
            roots: roots.to_vec(),
            levels: roots
                .iter()
                .map(|root| encode::root_levels(heap, *root))
                .collect(),
        }
    }

    /// Which module class `class` is, if it is one.
    pub fn find(&self, class: ClassReference<'gc>) -> Option<ModuleClass> {
        for (module, root) in self.roots.iter().enumerate() {
            let module = encode::u32_field("module index", module);
            if root.same(class) {
                return Some(ModuleClass::Root(module));
            }
            let levels = self
                .levels
                .get(module as usize)
                .map_or(&[][..], Vec::as_slice);
            if let Some(position) = levels.iter().position(|level| level.same(class)) {
                return Some(ModuleClass::Level {
                    module,
                    position: encode::u32_field("module level position", position),
                });
            }
        }
        None
    }

    /// Module `module`'s root class, or `None` past the modules.
    pub fn root(&self, module: u32) -> Option<ClassReference<'gc>> {
        self.roots.get(module as usize).copied()
    }

    /// Level `position` of module `module`'s root, or `None` past either.
    pub fn level(&self, module: u32, position: u32) -> Option<ClassReference<'gc>> {
        self.levels
            .get(module as usize)?
            .get(position as usize)
            .copied()
    }
}

/// Every module's origin, source when `with_source`, and root section, in
/// module order -- the order of the mixin table's trees.
///
/// Source travels for every module or for none: the header's one flag says
/// which, so a module handed none while another carries its own could only
/// be written by inventing an empty text for it.
pub fn encode_modules<'gc>(
    writer: &mut Writer,
    heap: &Heap<'gc>,
    modules: &[ModuleSource<'_, 'gc>],
    with_source: bool,
) -> Result<(), String> {
    writer.write_u32(encode::u32_field("module count", modules.len()));
    for module in modules {
        encode::encode_source_origin(writer, module.origin);
        if with_source {
            let Some(source) = module.source else {
                return Err(
                    "image encoding carries source for every module or for none: \
                            one module was handed none"
                        .to_string(),
                );
            };
            writer.write_str(source);
        }
        encode::encode_root_class(writer, heap, module.class)?;
    }
    Ok(())
}

/// The smallest a module record can be: an origin tag, an empty name's
/// length, and a root section with no level (presence flag, superclass tag,
/// level count).
const MIN_MODULE_BYTES: usize = 1 + 4 + 1 + 1 + 4;

/// One module as the module section describes it.
pub struct DecodedModule {
    pub origin: SourceOrigin,
    pub source: Option<String>,
    pub section: RootSection,
}

/// The inverse of `encode_modules`. The declared count must be the mixin
/// table's tree count -- a surplus tree would be declarations no module
/// names, a missing one a module without a root -- and is bounded by the
/// bytes left before anything is reserved for it. Each root section is held
/// to its own module's row (`decode::decode_root_section`).
pub fn decode_modules(
    reader: &mut Reader,
    mixins: &mixins::Decoded,
    with_source: bool,
) -> Result<Vec<DecodedModule>, DecodeError> {
    let declared = reader.read_u32()?;
    let trees = u32::try_from(mixins.module_rows.len()).unwrap_or(u32::MAX);
    if declared != trees {
        return Err(DecodeError::ModuleCountMismatch { declared, trees });
    }
    let count = decode::bounded_count(declared as usize, MIN_MODULE_BYTES, reader)?;
    let mut modules = Vec::with_capacity(count);
    for module_index in 0..count {
        let origin = decode::decode_source_origin(reader)?;
        let source = if with_source {
            Some(reader.read_str()?)
        } else {
            None
        };
        let section = decode::decode_root_section(reader, mixins, module_index)?;
        modules.push(DecodedModule {
            origin,
            source,
            section,
        });
    }
    Ok(modules)
}

/// Each module's root class and levels, rebuilt in module order -- the order
/// the program loaded them in, which is what keeps every identity hash the
/// bootstrap draws the same as the run that wrote the image.
pub fn build_module_classes<'gc>(
    mutator: &mut Mutator<'gc>,
    mixins: &mixins::Decoded,
    modules: &[DecodedModule],
) -> Result<ModuleClasses<'gc>, DecodeError> {
    let mut roots = Vec::with_capacity(modules.len());
    let mut levels = Vec::with_capacity(modules.len());
    for (module_index, module) in modules.iter().enumerate() {
        let (root, root_levels) =
            decode::build_root_class(mutator, mixins, module_index, &module.section)?;
        roots.push(root);
        levels.push(root_levels);
    }
    Ok(ModuleClasses { roots, levels })
}

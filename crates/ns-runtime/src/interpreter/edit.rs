//! Preparing an edit of a class: the Rust half of a MirrorBuilder. An edit
//! is spliced into its module's current text, the whole module parsed and
//! resolved again, and the edited declaration's subtree compiled fresh, so
//! that every binding on the lexical chain comes from one text
//! (`docs/design/liveness-and-reflection.md`, "Compiling: the module's text
//! is what an edit edits"). Preparing changes nothing a program observes;
//! the install replaces the old versions with what it answers.

use std::rc::Rc;

use ns_syntax::ast;

use super::Mutator;
use super::instantiate::{ModuleSource, method_name};
use crate::Symbol;
use crate::heap::{Heap, MixinId};
use crate::mixin::Mixin;
use crate::types::{SourceOrigin, Value};

/// A staged edit of one class, as a `ClassDeclarationBuilder` hands it
/// over: a new header, or `None` to keep it, the member operations of
/// each side, applied in order, and a new name, or `None` to keep it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Edit {
    pub(crate) header: Option<String>,
    pub(crate) instance_side: Vec<MemberEdit>,
    pub(crate) class_side: Vec<MemberEdit>,
    pub(crate) new_name: Option<Symbol>,
}

/// One builder's part of an install, as the install leaves read it from
/// its `{subject. edit}` pair (`entries_from_value`).
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum InstallEntry {
    /// An edit of the declaration whose mixin id this is.
    Edit(MixinId, Edit),
    /// A module declared from source, its text
    /// (`ClassDeclarationBuilder fromSource:`).
    Module(String),
}

/// The entries an install's `Array` stands for, `{ {subject. edit}. … }`
/// in the order the builders were given; `None` for anything else, which
/// the install leaves decline. A subject is a reified mixin of either side
/// of the edited declaration, a class side standing for its instance
/// side's, or `nil` for a module declared from source, whose edit is the
/// module's text, a String.
pub(crate) fn entries_from_value<'gc>(
    mutator: &Mutator<'gc>,
    value: Value<'gc>,
) -> Option<Vec<InstallEntry>> {
    let heap = &mutator.heap;
    heap.array_elements(value.as_object()?)?
        .into_iter()
        .map(|entry| {
            let pair = heap.array_elements(entry.as_object()?)?;
            let [subject, edit] = pair.as_slice() else {
                return None;
            };
            if subject.is_nil() {
                return Some(InstallEntry::Module(string_of(heap, *edit)?));
            }
            let id = heap.mixin_object_id(subject.as_object()?)?;
            let id = crate::builtins::reflection::instance_side_id(heap, id)?;
            Some(InstallEntry::Edit(id, Edit::from_value(heap, *edit)?))
        })
        .collect()
}

/// One member operation: a member added from its source text, replacing
/// one of the same kind and name, or one removed by name. A removed name is
/// the text of the Symbol the program handed over, never an interned
/// symbol: a program chooses it, and a name no member has -- a
/// program-made symbol's among them -- removes nothing. A nested class is
/// added, replaced or removed on the instance side only, where §6.3
/// declares classes; a replaced one keeps its id, its class objects and
/// its instances, and a removed one lives on obsolete
/// (`docs/design/liveness-and-reflection.md`, "The complete
/// install", decisions 2 and 3).
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum MemberEdit {
    AddMethod(String),
    RemoveMethod(String),
    AddLazySlot(String),
    RemoveLazySlot(String),
    AddClass(String),
    RemoveClass(String),
}

/// What an edit the VM accepts answers the MirrorBuilder: the edited
/// declaration's text and its header's, both sliced from the edited
/// module's text by the spans of its parse, so that the builder never
/// reads Newspeak syntax itself.
#[derive(Clone)]
pub(crate) struct EditedTexts {
    /// The edited declaration's text, from its start to its closing
    /// parenthesis.
    pub(crate) declaration: String,
    /// The edited declaration's header: from its start up to, not
    /// including, the parenthesis that opens its members; the whole
    /// declaration for a mixin application without a body.
    pub(crate) header: String,
}

/// One entry's edit spliced into its module's text (`Mutator::prepare_edit`).
pub(crate) struct SplicedEdit {
    /// The module's text with the edit in it.
    pub(crate) text: String,
    /// The declaration's new name, when the edit renames it.
    pub(crate) renamed: Option<Symbol>,
    /// Whether the edit renames a nested class or changes its access, so
    /// that its enclosing declaration is compiled again with it.
    pub(crate) widens: bool,
}

/// What preparing a module declared from source answers: its root mixin,
/// built and registered nowhere, with its identity, and the texts its
/// builder reads once it is installed.
pub(crate) struct PreparedSourceModule {
    pub(crate) root: Rc<Mixin>,
    pub(crate) texts: EditedTexts,
}

/// What preparing the edits of one module answers: everything the install
/// needs of it, built and registered nowhere.
pub(crate) struct PreparedModule {
    /// The whole edited module's text, from which each fresh mixin cut
    /// its declaration's: the install reads each fresh mixin's own, and
    /// the tests read the whole.
    #[cfg(test)]
    pub(crate) module_text: Rc<str>,
    /// Per entry, in the order given: the edited declaration's and its
    /// header's text, sliced from the edited module's text.
    pub(crate) texts: Vec<EditedTexts>,
    /// Per nested declaration a fresh mixin declares with no old
    /// counterpart, preorder: the id of the declaration it is added to and
    /// its fresh mixin, which the commit registers with its subtree under
    /// that id (`Heap::adopt_nested`).
    pub(crate) added: Vec<(MixinId, Rc<Mixin>)>,
    /// Every nested declaration's id whose enclosing declaration's fresh
    /// mixin no longer declares it, which the commit makes obsolete
    /// (`Heap::make_obsolete`).
    pub(crate) removed: Vec<MixinId>,
    /// Every declaration's id among `replacements` whose superclass clause
    /// or `<:` clauses the edit changed (`Mutator::clauses_changed`).
    pub(crate) clauses_changed: Vec<MixinId>,
    /// Every fresh mixin with the id it is installed under: per entry
    /// whose declaration no other entry's encloses, the edited
    /// declaration's instance side first, then its class side, then
    /// each nested mixin's two sides, preorder. A fresh mixin whose old
    /// counterpart never got an id is absent: nothing names it.
    pub(crate) replacements: Vec<(MixinId, Rc<Mixin>)>,
    /// For every pair of the subtree, the edited declaration's and each
    /// nested one's, whose fresh mixin declares no class side while its
    /// old version had a registered one: the instance side's id, that
    /// class side's id and the fresh synthesized class side replacing it,
    /// in `replacements`' order. A class side the fresh mixin declares
    /// replaces the old one in `replacements` instead.
    pub(crate) synthesized: Vec<(MixinId, MixinId, Rc<Mixin>)>,
}

impl Edit {
    /// The edit `value` stands for, `{ header. instanceOperations.
    /// classOperations. newName }`; `None` for anything else, which the
    /// leaf declines. A class side takes method operations only, since
    /// §6.3's class side declares nothing else: no lazy slot and no class. A new name is a Symbol or
    /// `nil`; it is interned, as the class's name it would become is.
    pub(crate) fn from_value<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<Edit> {
        let elements = heap.array_elements(value.as_object()?)?;
        let [header, instance_side, class_side, new_name] = elements.as_slice() else {
            return None;
        };
        let header = if header.is_nil() {
            None
        } else {
            Some(string_of(heap, *header)?)
        };
        let new_name = if new_name.is_nil() {
            None
        } else {
            Some(Symbol::intern(&symbol_text_of(heap, *new_name)?))
        };
        Some(Edit {
            header,
            instance_side: operations_of(heap, *instance_side, true)?,
            class_side: operations_of(heap, *class_side, false)?,
            new_name,
        })
    }
}

/// A String's text; `None` for anything else, a Symbol included.
fn string_of<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<String> {
    let object = value.as_object()?;
    if heap.is_symbol(object) {
        return None;
    }
    Some(heap.string_code_points(object)?.to_string())
}

/// A Symbol's text; `None` for anything else, a String included.
fn symbol_text_of<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<String> {
    heap.symbol_text(value.as_object()?)
}

/// One side's operations, each a two-element `Array` of its kind and its
/// argument; lazy slots and classes only on the instance side.
fn operations_of<'gc>(
    heap: &Heap<'gc>,
    value: Value<'gc>,
    instance_side: bool,
) -> Option<Vec<MemberEdit>> {
    heap.array_elements(value.as_object()?)?
        .into_iter()
        .map(|operation| {
            let pair = heap.array_elements(operation.as_object()?)?;
            let [kind, argument] = pair.as_slice() else {
                return None;
            };
            Some(match symbol_text_of(heap, *kind)?.as_str() {
                "addMethod" => MemberEdit::AddMethod(string_of(heap, *argument)?),
                "removeMethod" => MemberEdit::RemoveMethod(symbol_text_of(heap, *argument)?),
                "addLazySlot" if instance_side => {
                    MemberEdit::AddLazySlot(string_of(heap, *argument)?)
                }
                "removeLazySlot" if instance_side => {
                    MemberEdit::RemoveLazySlot(symbol_text_of(heap, *argument)?)
                }
                "addClass" if instance_side => MemberEdit::AddClass(string_of(heap, *argument)?),
                "removeClass" if instance_side => {
                    MemberEdit::RemoveClass(symbol_text_of(heap, *argument)?)
                }
                _ => return None,
            })
        })
        .collect()
}

/// Which side of the edited declaration an operation applies to.
#[derive(Clone, Copy)]
enum Side {
    Instance,
    Class,
}

/// The kind of member an operation adds or removes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberKind {
    Method,
    LazySlot,
    Class,
}

impl<'gc> Mutator<'gc> {
    /// The current version of the declaration the mixin `id` names and its
    /// reflected name, when an edit may change it; the diagnostic that
    /// refuses it otherwise.
    pub(crate) fn editable_declaration(&self, id: MixinId) -> Result<(Rc<Mixin>, String), String> {
        let Some(current) = self.heap.mixin(id) else {
            return Err("the edited mixin is unknown to this heap".to_string());
        };
        let name = current.reflected_name();
        if current.shipped {
            // Its code is the binary's: a replacement would bypass the
            // compile-time binding of its mirror sends and primitives.
            return Err(format!(
                "{name} belongs to the shipped core library, which no MirrorBuilder edits"
            ));
        }
        if self.heap.is_obsolete(id) {
            // Its declaration is in no module's text any more, so there is
            // no text to splice an edit into.
            return Err(format!(
                "{name} was removed by an install, and an obsolete class is not edited"
            ));
        }
        debug_assert!(
            !current.is_class_side(),
            "internal: an edit names a declaration's instance side"
        );
        if current.is_class_side() {
            return Err(format!(
                "{name} is a class side; an edit names its instance side"
            ));
        }
        Ok((current, name))
    }

    /// `text` with the edit `edit` of the mixin `id` spliced in, or the
    /// diagnostic that refuses it (`docs/design/liveness-and-reflection.md`,
    /// "Compiling: the module's text is what an edit edits"). `text` is
    /// the module's current text with every earlier entry of the install
    /// that edits the same module spliced in, so that the entries of one
    /// module compose on one text, in their order.
    ///
    /// `path` names the declaration in `text`, which an earlier entry's
    /// rename may have changed from the names the heap holds.
    ///
    /// Changes nothing: it only reads and splices text.
    pub(crate) fn prepare_edit(
        &self,
        id: MixinId,
        edit: &Edit,
        text: &str,
        path: &[Symbol],
    ) -> Result<SplicedEdit, String> {
        let (_, name) = self.editable_declaration(id)?;
        if let Some(new_name) = edit.new_name
            && !crate::modules::is_identifier(new_name.as_str())
        {
            return Err(format!(
                "{name} cannot be renamed to {new_name}, which is no identifier"
            ));
        }
        let original = parse_module(text, &name)?;
        let Some(original_declaration) = find_declaration(&original.root_class, path) else {
            return Err(format!(
                "{name}'s source text does not hold its declaration"
            ));
        };
        let header_start = header_start(&original, original_declaration, text);
        let mut spliced = text.to_string();
        for operation in &edit.instance_side {
            spliced = apply_member_edit(&spliced, path, Side::Instance, operation, &name)?;
        }
        for operation in &edit.class_side {
            spliced = apply_member_edit(&spliced, path, Side::Class, operation, &name)?;
        }
        if let Some(header) = &edit.header {
            // Last but for the name, because it may rename: every operation
            // before it finds the declaration by its old name.
            spliced = replace_header(&spliced, path, header, &name)?;
        }
        let edited = parse_module(&spliced, &name)?;
        let declaration = check_kept_header(
            original_declaration,
            &spliced,
            &edited,
            path,
            header_start,
            edit.header.as_ref().map(String::len),
            &name,
        )?;
        let access_changed = crate::dispatch::effective_access(declaration.access)
            != crate::dispatch::effective_access(original_declaration.access);
        let header_name = Symbol::intern(&declaration.name.name);
        let final_name = edit.new_name.unwrap_or(header_name);
        if final_name != header_name {
            spliced =
                rename_declaration(&spliced, &edited, declaration, path.is_empty(), final_name);
        }
        let renamed = final_name.as_str() != original_declaration.name.name;
        if renamed && let Some((_, enclosing_path)) = path.split_last() {
            let final_module = parse_module(&spliced, &name)?;
            let declared_twice = find_declaration(&final_module.root_class, enclosing_path)
                .is_some_and(|enclosing| {
                    crate::resolver::declared_members(&enclosing.instance_side)
                        .into_iter()
                        .filter(|member| *member == final_name)
                        .count()
                        > 1
                });
            if declared_twice {
                let enclosing_name = self
                    .heap
                    .enclosing_mixin_id(id)
                    .and_then(|enclosing| self.heap.mixin(enclosing))
                    .map_or_else(String::new, |enclosing| enclosing.reflected_name());
                return Err(format!(
                    "{enclosing_name} already has a member named {final_name}"
                ));
            }
        }
        Ok(SplicedEdit {
            text: spliced,
            renamed: renamed.then_some(final_name),
            // The accessor in the enclosing class's method table carries
            // the class's name and access, and every binding of the name
            // lies in the enclosing declaration's subtree.
            widens: !path.is_empty() && (renamed || access_changed),
        })
    }

    /// The edits `entries` of declarations of the module whose root is
    /// `module_root`, each spliced into the module's current text in the
    /// order given, then the whole module parsed and resolved once and
    /// every edited declaration's subtree compiled fresh; or the
    /// diagnostic that refuses one of them. An entry whose declaration
    /// another entry's encloses is compiled within that one's subtree.
    ///
    /// Changes nothing a program observes: no table, application or
    /// object. Registers no fresh mixin.
    pub(crate) fn prepare_module(
        &mut self,
        module_root: MixinId,
        entries: &[(MixinId, &Edit)],
    ) -> Result<PreparedModule, String> {
        let mut declarations = Vec::with_capacity(entries.len());
        for (id, _) in entries {
            let (_, name) = self.editable_declaration(*id)?;
            let (_, path) = self.path_from_module_root(*id);
            declarations.push((name, path));
        }
        let first_name = declarations
            .first()
            .map_or_else(String::new, |(name, _)| name.clone());
        let Some(original_text) = self.heap.module_source(module_root) else {
            return Err(format!("{first_name} has no source text to edit"));
        };
        let mut text = original_text.to_string();
        // Each entry's path in the text as it stands, which a rename
        // changes for the renamed declaration and every one nested in it.
        let mut paths: Vec<Vec<Symbol>> =
            declarations.iter().map(|(_, path)| path.clone()).collect();
        let mut renamed: Vec<(MixinId, Symbol)> = Vec::new();
        let mut widened = vec![false; entries.len()];
        // Each class an entry removes or replaces, with the entry's
        // position and the verb its diagnostic names.
        let mut taken: Vec<(MixinId, usize, &'static str)> = Vec::new();
        for (position, (id, edit)) in entries.iter().enumerate() {
            // Before the splice, which an entry of a taken class could fail
            // with a less telling diagnostic; by the names the text holds
            // when this entry is spliced, every earlier rename in effect.
            taken.extend(
                self.classes_taken(*id, edit, &renamed)
                    .into_iter()
                    .map(|(class, verb)| (class, position, verb)),
            );
            self.refuse_an_edit_of_a_taken_class(entries, &taken)?;
            let spliced = self.prepare_edit(*id, edit, &text, &paths[position])?;
            text = spliced.text;
            widened[position] = spliced.widens;
            if let Some(new_name) = spliced.renamed {
                let old_path = paths[position].clone();
                if let Some(last) = old_path.len().checked_sub(1) {
                    for path in &mut paths {
                        if path.starts_with(&old_path) {
                            path[last] = new_name;
                        }
                    }
                }
                renamed.push((*id, new_name));
            }
        }
        // The declaration each entry compiles: its own, or for a rename
        // or an access change its enclosing one, with that one's id.
        let mut compiled: Vec<(MixinId, Vec<Symbol>)> = Vec::with_capacity(entries.len());
        for (position, (id, _)) in entries.iter().enumerate() {
            let path = &paths[position];
            match (widened[position], self.heap.enclosing_mixin_id(*id)) {
                (true, Some(enclosing)) => {
                    compiled.push((enclosing, path[..path.len() - 1].to_vec()));
                }
                _ => compiled.push((*id, path.clone())),
            }
        }
        let edited = parse_module(&text, &first_name)?;
        // Each entry's declaration's extent, so that a resolver error is
        // named by the innermost edited declaration it falls in.
        let extents: Vec<Option<ns_syntax::Span>> = paths
            .iter()
            .map(|path| find_declaration(&edited.root_class, path).map(|found| found.span))
            .collect();
        let text: Rc<str> = Rc::from(text);
        // Never `resolve_kernel`: only shipped source records mirror sends.
        let resolved = crate::resolver::resolve(
            edited,
            Rc::clone(&text),
            SourceOrigin::Embedded("<edit>".into()),
        )
        .map_err(|errors| {
            let (message, start) = errors.first().map_or_else(
                || ("the resolver refused it".to_string(), 0),
                |error| (error.message.clone(), error.span.start),
            );
            let name = extents
                .iter()
                .zip(&declarations)
                .filter_map(|(extent, (name, _))| {
                    extent
                        .filter(|span| span.start <= start && start <= span.end)
                        .map(|span| (span.end - span.start, name))
                })
                .min_by_key(|(length, _)| *length)
                .map_or(&first_name, |(_, name)| name);
            format!(
                "{name}'s edited source does not resolve: {message} (line {})",
                line_of(&text, start)
            )
        })?;
        let mut texts = Vec::with_capacity(entries.len());
        let mut paired = Paired::default();
        for (position, (name, _)) in declarations.iter().enumerate() {
            let Some(declaration) = find_declaration(&resolved.module.root_class, &paths[position])
            else {
                return Err(format!(
                    "{name}'s edited source does not hold its declaration"
                ));
            };
            texts.push(declaration_texts(&resolved.module, &text, declaration));
            let (id, path) = &compiled[position];
            // Compiled within another entry's subtree, or with the earlier
            // entry that compiles the same declaration.
            let enclosed = compiled.iter().enumerate().any(|(other, (_, other_path))| {
                other != position
                    && path.starts_with(other_path)
                    && (other_path.len() < path.len() || other < position)
            });
            if enclosed {
                continue;
            }
            let Some(declaration) = find_declaration(&resolved.module.root_class, path) else {
                return Err(format!(
                    "{name}'s edited source does not hold its declaration"
                ));
            };
            let Some(current) = self.heap.mixin(*id) else {
                return Err("the edited mixin is unknown to this heap".to_string());
            };
            let fresh = if path.is_empty() {
                // The same module in a new version, its identity the one
                // its edited text declares, under the path it had.
                let identity = edited_identity(&current, &original_text, &resolved.module, &text)?;
                self.build_root_mixin(
                    declaration,
                    &resolved.bindings,
                    ModuleSource::User,
                    &text,
                    identity,
                )
            } else {
                self.build_mixin(
                    declaration,
                    &resolved.module.root_class,
                    &resolved.bindings,
                    ModuleSource::User,
                    &text,
                )
            };
            debug_assert!(
                self.nothing_registers(&fresh),
                "internal: a prepared mixin is registered under no id"
            );
            self.pair(*id, &current, &fresh, &renamed, &mut paired);
        }
        let clauses_changed = self.clauses_changed(&original_text, &text, &paired.replacements)?;
        Ok(PreparedModule {
            #[cfg(test)]
            module_text: text,
            texts,
            replacements: paired.replacements,
            synthesized: paired.synthesized,
            added: paired.added,
            removed: paired.removed,
            clauses_changed,
        })
    }

    /// The module `text` declares, compiled as the loader compiles a
    /// program's module, or the diagnostic that refuses it
    /// (`docs/design/liveness-and-reflection.md`, "The complete
    /// install", decision 7): a text that is not one class declaration,
    /// one that does not parse or resolve, and an id or version the
    /// loader's own rules refuse -- a malformed `:module:` and an id under
    /// `newspeak.` -- with the loader's messages. Its identity is what its
    /// `:module:` declares, its class's name without one, and it has no
    /// path, since no file holds it. Whether another module of the world
    /// has its identity is the install's question
    /// (`Mutator::refuse_a_colliding_identity`).
    ///
    /// Changes nothing a program observes. Registers nothing.
    pub(crate) fn prepare_source_module(
        &mut self,
        text: &str,
    ) -> Result<PreparedSourceModule, String> {
        let module = ns_syntax::parse(text)
            .map_err(|diagnostics| source_module_parse_refusal(text, &diagnostics))?;
        let (id, version) = crate::modules::declared_identity(&module, text)?;
        crate::modules::check_unreserved(&id)?;
        let text: Rc<str> = Rc::from(text);
        // Never `resolve_kernel`: only shipped source records mirror sends,
        // and a module a program declares is no shipped one.
        let resolved = crate::resolver::resolve(
            module,
            Rc::clone(&text),
            SourceOrigin::Embedded("<fromSource:>".into()),
        )
        .map_err(|errors| {
            let (message, start) = errors.first().map_or_else(
                || ("the resolver refused it".to_string(), 0),
                |error| (error.message.clone(), error.span.start),
            );
            format!(
                "the module's source does not resolve: {message} (line {})",
                line_of(&text, start)
            )
        })?;
        let identity = crate::modules::ModuleIdentity {
            path: None,
            id,
            version,
        };
        let root = self.build_root_mixin(
            &resolved.module.root_class,
            &resolved.bindings,
            ModuleSource::User,
            &text,
            Some(Rc::new(identity)),
        );
        debug_assert!(
            self.nothing_registers(&root),
            "internal: a prepared module is registered under no id"
        );
        let texts = declaration_texts(&resolved.module, &text, &resolved.module.root_class);
        Ok(PreparedSourceModule { root, texts })
    }

    /// The ids among `replacements` whose fresh declaration, in `text`,
    /// writes another superclass clause or other `<:` clauses than its
    /// current version does in `original_text`, its module's current text:
    /// the declarations whose class objects the install re-parents, a
    /// nested one's over what its new clauses answer in each enclosing
    /// instance, a root over its new static superclass. A mixin
    /// application without a body of its own is re-parented as any other
    /// declaration, and so is one that gains or loses its body: every class
    /// object has the fields of both shapes.
    fn clauses_changed(
        &self,
        original_text: &str,
        text: &str,
        replacements: &[(MixinId, Rc<Mixin>)],
    ) -> Result<Vec<MixinId>, String> {
        let does_not_hold = |id: MixinId| {
            let name = self
                .heap
                .mixin(id)
                .map_or_else(String::new, |mixin| mixin.reflected_name());
            format!("{name}'s source text does not hold its declaration")
        };
        // The text every entry of the module was spliced into: the first
        // entry's preparation parsed it already.
        let original = ns_syntax::parse(original_text).ok();
        debug_assert!(
            original.is_some(),
            "internal: the module's current text parsed for its first entry"
        );
        let mut changed = Vec::new();
        for (id, fresh) in replacements {
            if fresh.is_class_side() {
                continue;
            }
            let Some(current) = self.heap.mixin(*id) else {
                return Err(does_not_hold(*id));
            };
            // A fresh mixin is built from source, which always records its
            // declaration.
            let Some(edited) = fresh.declaration.as_deref() else {
                debug_assert!(false, "internal: a fresh mixin records its declaration");
                return Err(does_not_hold(*id));
            };
            let (_, path) = self.path_from_module_root(*id);
            let Some(written) = original
                .as_ref()
                .and_then(|original| find_declaration(&original.root_class, &path))
            else {
                return Err(does_not_hold(*id));
            };
            if current.has_body == fresh.has_body
                && clauses_as_written(original_text, written) == clauses_as_written(text, edited)
            {
                continue;
            }
            changed.push(*id);
        }
        Ok(changed)
    }

    /// The id of the module root the mixin `id` is declared in, and the
    /// names of the declarations from below the root down to `id`'s.
    pub(crate) fn path_from_module_root(&self, id: MixinId) -> (MixinId, Vec<Symbol>) {
        let mut path = Vec::new();
        let mut module_root = id;
        while let Some(enclosing) = self.heap.enclosing_mixin_id(module_root) {
            if let Some(mixin) = self.heap.mixin(module_root) {
                path.push(mixin.name);
            }
            module_root = enclosing;
        }
        path.reverse();
        (module_root, path)
    }

    /// Whether no mixin of `fresh`'s tree has an id: the install's
    /// `Heap::replace_mixin` refuses a replacement that has one.
    fn nothing_registers(&self, fresh: &Rc<Mixin>) -> bool {
        self.heap.known_mixin_id(fresh).is_none()
            && fresh
                .class_side
                .iter()
                .all(|class_side| self.heap.known_mixin_id(class_side).is_none())
            && fresh
                .declared_nested()
                .iter()
                .all(|nested| self.nothing_registers(nested))
    }

    /// The registered id of `old`'s class side, declared or synthesized;
    /// `None` while nothing registered one.
    fn old_class_side_id(&self, id: MixinId, old: &Rc<Mixin>) -> Option<MixinId> {
        match &old.class_side {
            Some(declared) => self.heap.known_mixin_id(declared),
            None => self
                .metaclass_mixins
                .get(&id)
                .and_then(|synthesized| self.heap.known_mixin_id(synthesized)),
        }
    }

    /// Pairs `fresh` with `old`, registered under `id`: the instance side,
    /// then a declared class side, then each nested mixin with the
    /// current version of the one of its name the id declares
    /// (`Heap::nested_of`), preorder. A fresh mixin that declares no class
    /// side gets a synthesized one in `synthesized`, under the id of the
    /// class side its old version had, so that every metaclass of the
    /// subtree keeps its mixin's id. A declaration `renamed` names is
    /// paired by its new name, so that a renamed class keeps its id, its
    /// class objects and its instances. Pairing by name is what makes an
    /// `addFromSource:` of an existing name a replacement; a fresh nested
    /// mixin no old one pairs with is added, and an old one no fresh one
    /// pairs with is removed.
    fn pair(
        &self,
        id: MixinId,
        old: &Rc<Mixin>,
        fresh: &Rc<Mixin>,
        renamed: &[(MixinId, Symbol)],
        paired: &mut Paired,
    ) {
        paired.replacements.push((id, Rc::clone(fresh)));
        if let Some(class_side_id) = self.old_class_side_id(id, old) {
            match &fresh.class_side {
                Some(class_side) => paired
                    .replacements
                    .push((class_side_id, Rc::clone(class_side))),
                None => paired.synthesized.push((
                    id,
                    class_side_id,
                    super::metaclass::synthesized_class_side(fresh),
                )),
            }
        }
        let mut kept: Vec<MixinId> = Vec::new();
        for nested in fresh.declared_nested().iter() {
            let counterpart = self
                .heap
                .nested_of(id)
                .iter()
                .copied()
                .find(|old_id| self.name_after(*old_id, renamed) == Some(nested.name));
            let Some(nested_id) = counterpart else {
                paired.added.push((id, Rc::clone(nested)));
                continue;
            };
            kept.push(nested_id);
            let current = self
                .heap
                .mixin(nested_id)
                .expect("internal: the counterpart was found by its row");
            self.pair(nested_id, &current, nested, renamed, paired);
        }
        paired.removed.extend(
            self.heap
                .nested_of(id)
                .iter()
                .copied()
                .filter(|old_id| !kept.contains(old_id)),
        );
    }

    /// The name the declaration `id` has after `renamed`, the renames of
    /// an install so far: what its module's text calls it, and what
    /// pairing finds it by.
    fn name_after(&self, id: MixinId, renamed: &[(MixinId, Symbol)]) -> Option<Symbol> {
        renamed
            .iter()
            .find(|(renamed_id, _)| *renamed_id == id)
            .map(|(_, new_name)| *new_name)
            .or_else(|| self.heap.mixin(id).map(|mixin| mixin.name))
    }

    /// Refuses the entries of a module when one edits a class another
    /// removes or replaces (`taken`), or one nested in it, in either
    /// order: the one would edit a class the other's install no longer
    /// has, or has anew from its text, so no order of the two is what
    /// both builders staged.
    fn refuse_an_edit_of_a_taken_class(
        &self,
        entries: &[(MixinId, &Edit)],
        taken: &[(MixinId, usize, &'static str)],
    ) -> Result<(), String> {
        let name = |id: MixinId| {
            self.heap
                .mixin(id)
                .map_or_else(String::new, |mixin| mixin.reflected_name())
        };
        for (class, taker, verb) in taken {
            for (position, (id, _)) in entries.iter().enumerate() {
                if position != *taker && self.is_declared_within(*id, *class) {
                    return Err(format!(
                        "the builder of {} edits a class the builder of {} {verb}",
                        name(*id),
                        name(entries[*taker].0)
                    ));
                }
            }
        }
        Ok(())
    }

    /// The nested declarations of `id` that `edit`, an entry's, removes
    /// or replaces, each with the verb a diagnostic names it by, found by
    /// the names they have after `renamed`, the renames of the entries
    /// spliced before it -- the names the text holds when its edit is
    /// spliced, and pairing finds them by. A text that is not one class
    /// declaration takes nothing; its own preparation refuses it.
    fn classes_taken(
        &self,
        id: MixinId,
        edit: &Edit,
        renamed: &[(MixinId, Symbol)],
    ) -> Vec<(MixinId, &'static str)> {
        edit.instance_side
            .iter()
            .filter_map(|operation| {
                let (name, verb) = match operation {
                    MemberEdit::RemoveClass(name) => (name.clone(), "removes"),
                    MemberEdit::AddClass(text) => match read_member_text(text) {
                        Ok(Some((MemberKind::Class, name))) => (name, "replaces"),
                        _ => return None,
                    },
                    _ => return None,
                };
                let taken = self.heap.nested_of(id).iter().copied().find(|nested| {
                    self.name_after(*nested, renamed)
                        .is_some_and(|after| after.as_str() == name)
                })?;
                Some((taken, verb))
            })
            .collect()
    }
}

/// What pairing a module's fresh mixins with their old versions answers
/// (`Mutator::pair`), as `PreparedModule` holds it.
#[derive(Default)]
struct Paired {
    replacements: Vec<(MixinId, Rc<Mixin>)>,
    synthesized: Vec<(MixinId, MixinId, Rc<Mixin>)>,
    added: Vec<(MixinId, Rc<Mixin>)>,
    removed: Vec<MixinId>,
}

/// What the builder of `declaration` reads after an install: its text and
/// its header's, sliced from `text`, the edited module's, by the spans of
/// its parse, the header of `module`'s root from its first metadata
/// comment (`header_start`).
fn declaration_texts(
    module: &ast::Module,
    text: &str,
    declaration: &ast::ClassDeclaration,
) -> EditedTexts {
    let declaration_text = crate::mixin::source_text(text, declaration.span)
        .unwrap_or_default()
        .to_string();
    let header_end = declaration
        .instance_side
        .members_delimiters
        .map_or(declaration.span.end, |members| members.open.start);
    let header_text = usize::try_from(header_start(module, declaration, text))
        .ok()
        .zip(usize::try_from(header_end).ok())
        .and_then(|(start, end)| text.get(start..end))
        .unwrap_or_default()
        .to_string();
    EditedTexts {
        declaration: declaration_text,
        header: header_text,
    }
}

/// The diagnostic of a module text `ns_syntax::parse` refused with
/// `diagnostics`: a text that writes no `class` at all -- an empty one, a
/// comment alone -- and one whose part before its first error is a whole
/// module -- a class declaration followed by a second one, or by a member
/// -- are not one class declaration; any other does not parse.
fn source_module_parse_refusal(text: &str, diagnostics: &[ns_syntax::Diagnostic]) -> String {
    let declares_no_class = ns_syntax::lex(text).is_ok_and(|tokens| {
        tokens
            .iter()
            .all(|token| token.kind != ns_syntax::TokenKind::Class)
    });
    if declares_no_class {
        return "fromSource: takes one class declaration".to_string();
    }
    let first_error = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == ns_syntax::Severity::Error)
        .min_by_key(|diagnostic| diagnostic.span.start);
    let whole_before = first_error
        .and_then(|error| text.get(..error.span.start as usize))
        .is_some_and(|before| ns_syntax::parse(before).is_ok());
    if whole_before {
        return "fromSource: takes one class declaration".to_string();
    }
    let (message, start) = first_error.or(diagnostics.first()).map_or_else(
        || ("the parser refused it".to_string(), 0),
        |diagnostic| (diagnostic.message.clone(), diagnostic.span.start),
    );
    format!(
        "the module's source does not parse: {message} (line {})",
        line_of(text, start)
    )
}

/// `text` parsed, or the syntax diagnostic of `name`'s edit.
fn parse_module(text: &str, name: &str) -> Result<ast::Module, String> {
    ns_syntax::parse(text).map_err(|diagnostics| {
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.severity == ns_syntax::Severity::Error)
            .or(diagnostics.first());
        let (message, start) = diagnostic.map_or_else(
            || ("the parser refused it".to_string(), 0),
            |diagnostic| (diagnostic.message.clone(), diagnostic.span.start),
        );
        syntax_diagnostic(name, &message, text, start)
    })
}

fn syntax_diagnostic(name: &str, message: &str, text: &str, offset: u32) -> String {
    format!(
        "{name}'s edited source does not parse: {message} (line {})",
        line_of(text, offset)
    )
}

/// The 1-based line `offset`, a byte offset, falls on in `text`.
fn line_of(text: &str, offset: u32) -> usize {
    let end = usize::try_from(offset).map_or(text.len(), |offset| offset.min(text.len()));
    text.as_bytes()[..end]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

/// The declaration `path` names from `root`: each name a nested class of
/// the one before, `root` itself for an empty path.
pub(crate) fn find_declaration<'module>(
    root: &'module ast::ClassDeclaration,
    path: &[Symbol],
) -> Option<&'module ast::ClassDeclaration> {
    path.iter().try_fold(root, |declaration, name| {
        declaration
            .instance_side
            .nested_classes()
            .find(|nested| nested.name.name == name.as_str())
    })
}

/// `text` with `replacement` in place of the bytes `start..end`.
fn splice(text: &str, start: u32, end: u32, replacement: &str) -> String {
    let start = start as usize;
    let end = end as usize;
    let mut spliced = String::with_capacity(text.len() - (end - start) + replacement.len());
    spliced.push_str(&text[..start]);
    spliced.push_str(replacement);
    spliced.push_str(&text[end..]);
    spliced
}

/// A member's range in its module's text: from its first attached comment,
/// which travels with it, to its end.
fn member_range(member: &ast::Member) -> (u32, u32) {
    let (comments, span) = match member {
        ast::Member::Class(class) => (&class.comments, class.span),
        ast::Member::LazySlot(slot) => (&slot.comments, slot.span),
        ast::Member::Method(method) => (&method.comments, method.span),
    };
    let start = comments
        .first()
        .map_or(span.start, |comment| comment.span.start.min(span.start));
    (start, span.end)
}

/// `start..end` widened to the whole lines it covers when nothing but
/// spaces and tabs stands before it on its first line and after it on its
/// last, so that a removed member leaves no line of indentation behind;
/// otherwise as it is. Spans are byte offsets, and the widening steps over
/// ASCII only, so it stays on character boundaries.
fn whole_lines(text: &str, start: u32, end: u32) -> (u32, u32) {
    let bytes = text.as_bytes();
    let (start_index, end_index) = (start as usize, end as usize);
    let blank = |byte: &u8| *byte == b' ' || *byte == b'\t';
    let line_start = bytes[..start_index]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    let line_end = bytes[end_index..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |position| end_index + position + 1);
    let before_blank = bytes[line_start..start_index].iter().all(blank);
    let after_blank = bytes[end_index..line_end]
        .iter()
        .all(|byte| blank(byte) || *byte == b'\n' || *byte == b'\r');
    if !(before_blank && after_blank) {
        return (start, end);
    }
    let offset =
        |index: usize| u32::try_from(index).expect("internal: a span's lines lie within its text");
    (offset(line_start), offset(line_end))
}

/// The kind and name of a member.
fn member_kind_and_name(member: &ast::Member) -> (MemberKind, String) {
    match member {
        ast::Member::Method(method) => (MemberKind::Method, method_name(&method.pattern)),
        ast::Member::LazySlot(slot) => (MemberKind::LazySlot, slot.name.name.clone()),
        ast::Member::Class(class) => (MemberKind::Class, class.name.name.clone()),
    }
}

/// The diagnostic of a member text that is not one member of `kind`.
fn wrong_kind(name: &str, kind: MemberKind) -> String {
    match kind {
        MemberKind::Method => {
            format!("addFromSource: of {name}'s methods takes one method declaration")
        }
        MemberKind::LazySlot => {
            format!("addFromSource: of {name}'s lazy slots takes one lazy slot declaration")
        }
        MemberKind::Class => {
            format!("addFromSource: of {name}'s nested classes takes one class declaration")
        }
    }
}

const SCRATCH_PREFIX: &str = "class Scratch = () (\n";

/// What `member_text` declares, read alone in a scratch module: its kind
/// and name when it is one member that ends where the members do, `None`
/// for anything else -- a text that closes the group early would splice
/// members, or a class side, of its own into the edited declaration. The
/// error is the scratch module's parse failure.
fn read_member_text(member_text: &str) -> Result<Option<(MemberKind, String)>, ()> {
    let scratch = format!("{SCRATCH_PREFIX}{member_text}\n)");
    let module = ns_syntax::parse(&scratch).map_err(|_| ())?;
    let side = &module.root_class.instance_side;
    let expected_close = SCRATCH_PREFIX.len() + member_text.len() + 1;
    let closes_where_expected = side
        .members_delimiters
        .and_then(|delimiters| delimiters.close)
        .is_some_and(|close| close.start as usize == expected_close);
    if module.root_class.class_side.is_some() || !closes_where_expected {
        return Ok(None);
    }
    let [member] = side.members.as_slice() else {
        return Ok(None);
    };
    Ok(Some(member_kind_and_name(member)))
}

/// `text` with `operation` applied to `side` of the declaration `path`
/// names.
fn apply_member_edit(
    text: &str,
    path: &[Symbol],
    side: Side,
    operation: &MemberEdit,
    name: &str,
) -> Result<String, String> {
    let module = parse_module(text, name)?;
    let Some(declaration) = find_declaration(&module.root_class, path) else {
        return Err(syntax_diagnostic(
            name,
            "the edited declaration is no longer where it was",
            text,
            0,
        ));
    };
    let (kind, member_text, removed) = match operation {
        MemberEdit::AddMethod(member_text) => (MemberKind::Method, Some(member_text), None),
        MemberEdit::AddLazySlot(member_text) => (MemberKind::LazySlot, Some(member_text), None),
        MemberEdit::RemoveMethod(selector) => (MemberKind::Method, None, Some(selector)),
        MemberEdit::RemoveLazySlot(slot) => (MemberKind::LazySlot, None, Some(slot)),
        MemberEdit::AddClass(member_text) => (MemberKind::Class, Some(member_text), None),
        MemberEdit::RemoveClass(class) => (MemberKind::Class, None, Some(class)),
    };
    let body = match side {
        Side::Instance => Some(&declaration.instance_side),
        Side::Class => declaration.class_side.as_ref(),
    };
    let existing = |member_name: &str| {
        body.and_then(|body| {
            body.members.iter().find(|member| {
                let (member_kind, declared) = member_kind_and_name(member);
                member_kind == kind && declared == member_name
            })
        })
    };
    if let Some(removed) = removed {
        return Ok(match existing(removed) {
            Some(member) => {
                let (start, end) = member_range(member);
                let (start, end) = whole_lines(text, start, end);
                splice(text, start, end, "")
            }
            None => text.to_string(),
        });
    }
    let Some(member_text) = member_text else {
        unreachable!("internal: an operation adds or removes");
    };
    let read = read_member_text(member_text);
    let replaced = match &read {
        Ok(Some((read_kind, member_name))) if *read_kind == kind => existing(member_name),
        Ok(_) => return Err(wrong_kind(name, kind)),
        // Not even a member alone: spliced in to have the module's own
        // parse say where it fails.
        Err(()) => None,
    };
    let spliced = match replaced {
        Some(member) => {
            let (start, end) = member_range(member);
            splice(text, start, end, member_text)
        }
        None => {
            let Some(instance_close) = declaration
                .instance_side
                .members_delimiters
                .and_then(|delimiters| delimiters.close)
            else {
                return Err(format!(
                    "{name} is a mixin application without a body, which an edit cannot add \
                     members to"
                ));
            };
            match (side, &declaration.class_side) {
                (Side::Instance, _) => splice(
                    text,
                    instance_close.start,
                    instance_close.start,
                    &format!("\n{member_text}\n"),
                ),
                (Side::Class, Some(class_side)) => {
                    let close = class_side
                        .delimiters
                        .and_then(|delimiters| delimiters.close)
                        .map_or(class_side.span.end, |close| close.start);
                    splice(text, close, close, &format!("\n{member_text}\n"))
                }
                (Side::Class, None) => splice(
                    text,
                    instance_close.end,
                    instance_close.end,
                    &format!(" : (\n{member_text}\n)"),
                ),
            }
        }
    };
    if read.is_err() {
        parse_module(&spliced, name)?;
        return Err(wrong_kind(name, kind));
    }
    Ok(spliced)
}

/// Where the header of `declaration`, a declaration of `module` parsed
/// from `text`, begins: at the class for a nested one, and for the root
/// at its first metadata comment, so that a root's header holds the
/// `:module:` that names the module and an edit may change it.
fn header_start(module: &ast::Module, declaration: &ast::ClassDeclaration, text: &str) -> u32 {
    if !std::ptr::eq(declaration, &module.root_class) {
        return declaration.span.start;
    }
    ns_syntax::metadata_of(module, text)
        .into_iter()
        .filter(|(target, _)| *target == ns_syntax::MetadataTarget::Module)
        .map(|(_, metadata)| metadata.span.start)
        .fold(declaration.span.start, u32::min)
}

/// `text` with the header of the declaration `path` names -- from its
/// start, a root's first metadata comment, to its members' `(` --
/// replaced by `header`.
fn replace_header(text: &str, path: &[Symbol], header: &str, name: &str) -> Result<String, String> {
    let module = parse_module(text, name)?;
    let Some(declaration) = find_declaration(&module.root_class, path) else {
        return Err(syntax_diagnostic(
            name,
            "the edited declaration is no longer where it was",
            text,
            0,
        ));
    };
    let Some(members) = declaration.instance_side.members_delimiters else {
        return Err(format!(
            "{name} is a mixin application without a body, whose header an edit cannot replace"
        ));
    };
    Ok(splice(
        text,
        header_start(&module, declaration, text),
        members.open.start,
        header,
    ))
}

/// `text`, parsed as `module`, with `declaration` named `new_name`, and
/// for the root the last part of the id its `:module:` declares too, so
/// that the id still ends in the class's name.
fn rename_declaration(
    text: &str,
    module: &ast::Module,
    declaration: &ast::ClassDeclaration,
    is_root: bool,
    new_name: Symbol,
) -> String {
    let name = declaration.name.span;
    let renamed = splice(text, name.start, name.end, new_name.as_str());
    if !is_root {
        return renamed;
    }
    let declared = ns_syntax::metadata_of(module, text)
        .into_iter()
        .find(|(target, metadata)| {
            *target == ns_syntax::MetadataTarget::Module
                && metadata.tag == crate::modules::MODULE_TAG
        });
    let Some((_, metadata)) = declared else {
        return renamed;
    };
    let Some(comment) = crate::mixin::source_text(text, metadata.span) else {
        return renamed;
    };
    // The payload follows the tag's closing colon; searched only there, so
    // that a payload that spells the tag, `(* :module: module *)`, is not
    // found in the tag.
    let Some(tag_end) = comment.find(':').and_then(|open| {
        comment[open + 1..]
            .find(':')
            .map(|close| open + 1 + close + 1)
    }) else {
        return renamed;
    };
    let Some(payload_start) = comment[tag_end..]
        .find(&metadata.payload)
        .map(|offset| tag_end + offset)
    else {
        return renamed;
    };
    let id = metadata
        .payload
        .split_once('@')
        .map_or(metadata.payload.as_str(), |(id, _)| id);
    let last_start = id.rfind('.').map_or(0, |dot| dot + 1);
    let (Ok(start), Ok(end)) = (
        u32::try_from(payload_start + last_start),
        u32::try_from(payload_start + id.len()),
    ) else {
        return renamed;
    };
    // The metadata stands before the class, so the name's splice after it
    // left its offsets as they were.
    splice(
        &renamed,
        metadata.span.start + start,
        metadata.span.start + end,
        new_name.as_str(),
    )
}

/// The identity of the new version of `current`, a module's root, whose
/// text was `original_text` and is `text`, parsed as `edited`: the one it
/// had while its text declares what it declared, so that an id the
/// project gave it (`as`) stays; otherwise one of the same path -- an
/// install changes a module's attributes, never which module it is
/// (`docs/design/liveness-and-reflection.md`, "The complete
/// install", decision 9) -- with the declared version and the declared
/// id, or for an id the project gave, that id ending in the class's new
/// name. The loader's own refusals refuse it, with their messages: a
/// malformed declaration and an id in the reserved namespace.
fn edited_identity(
    current: &Mixin,
    original_text: &str,
    edited: &ast::Module,
    text: &str,
) -> Result<Option<Rc<crate::modules::ModuleIdentity>>, String> {
    let (id, version) = crate::modules::declared_identity(edited, text)?;
    let Some(identity) = &current.module_identity else {
        return Ok(None);
    };
    let original = ns_syntax::parse(original_text)
        .ok()
        .and_then(|module| crate::modules::declared_identity(&module, original_text).ok());
    if original
        .as_ref()
        .is_some_and(|(original_id, original_version)| {
            *original_id == id && *original_version == version
        })
    {
        return Ok(Some(Rc::clone(identity)));
    }
    let given = original
        .as_ref()
        .is_some_and(|(original_id, _)| *original_id != identity.id);
    let id = if given {
        let class_name = id.class_name().to_string();
        let given_id = match identity.id.as_str().rsplit_once('.') {
            Some((prefix, _)) => format!("{prefix}.{class_name}"),
            None => class_name.clone(),
        };
        crate::modules::ModuleId::parse(&given_id, &class_name)?
    } else {
        id
    };
    crate::modules::check_unreserved(&id)?;
    Ok(Some(Rc::new(crate::modules::ModuleIdentity {
        path: identity.path.clone(),
        id,
        version,
    })))
}

/// The declaration the edited module holds in place of `original`, or a
/// refusal when it lost its place. Everything before the header's start,
/// `header_start`, is untouched by the splices, so the edited declaration
/// is the class that starts where the old one did, a root the module's; a
/// header edit must end where its members' `(` begins, or it spliced a
/// declaration of its own into the enclosing class. Its superclass and
/// `<:` clauses may change: the install evaluates the new ones
/// (`clauses_changed`).
fn check_kept_header<'module>(
    original: &ast::ClassDeclaration,
    text: &str,
    edited: &'module ast::Module,
    path: &[Symbol],
    header_start: u32,
    header_length: Option<usize>,
    name: &str,
) -> Result<&'module ast::ClassDeclaration, String> {
    let declaration_start = original.span.start;
    let declaration = match path.split_last() {
        None => Some(&edited.root_class),
        Some((_, enclosing)) => {
            find_declaration(&edited.root_class, enclosing).and_then(|parent| {
                parent
                    .instance_side
                    .nested_classes()
                    .find(|nested| nested.span.start == declaration_start)
            })
        }
    };
    let header_kept_in_place = |declaration: &ast::ClassDeclaration| {
        header_length.is_none_or(|length| {
            declaration
                .instance_side
                .members_delimiters
                .is_some_and(|members| {
                    members.open.start as usize == header_start as usize + length
                })
        })
    };
    declaration
        .filter(|declaration| header_kept_in_place(declaration))
        .ok_or_else(|| {
            syntax_diagnostic(
                name,
                "the header must declare the class and end where its members begin",
                text,
                declaration_start,
            )
        })
}

/// What decides a declaration's superclass, as its text writes it:
/// whether it writes a superclass clause, and the text of that clause and
/// of each `<:` clause.
fn clauses_as_written(text: &str, declaration: &ast::ClassDeclaration) -> (bool, Vec<String>) {
    let clauses = declaration
        .superclass
        .iter()
        .chain(&declaration.mixin_applications)
        .map(|clause| clause.span.text(text).to_string())
        .collect();
    (declaration.superclass.is_some(), clauses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Root;
    use crate::bytecode::Instruction;
    use crate::interpreter::install::prepared::PreparedInstall;
    use crate::interpreter::{HostResult, HostSignal, Interpreter};
    use crate::mixin::MethodEntry;
    use indoc::indoc;
    use std::path::PathBuf;

    fn load(source: &str) -> (Interpreter, Root) {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        (interpreter, class)
    }

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// The mixin of the module root `class` holds.
    fn root_mixin(mutator: &Mutator<'_>, class: &Root) -> Rc<Mixin> {
        let class = mutator
            .fetch(class)
            .as_object()
            .expect("a module's root is a class object");
        mutator
            .heap()
            .class_application(class)
            .expect("a module's root is a class object")
            .mixin
            .clone()
    }

    /// The ids of the module root `class` holds and of its first nested
    /// mixin, registering the nested one as a reflective reader would.
    fn root_and_inner_ids(interpreter: &mut Interpreter, class: &Root) -> (MixinId, MixinId) {
        interpreter.enter(|mutator| {
            let root = root_mixin(mutator, class);
            let root_id = mutator.heap.mixin_id(&root);
            (root_id, mutator.heap.nested_of(root_id)[0])
        })
    }

    fn instance_edit(operations: Vec<MemberEdit>) -> Edit {
        Edit {
            header: None,
            instance_side: operations,
            class_side: Vec::new(),
            new_name: None,
        }
    }

    fn add_method(text: &str) -> Edit {
        instance_edit(vec![MemberEdit::AddMethod(text.to_string())])
    }

    fn header_edit(text: &str) -> Edit {
        Edit {
            header: Some(text.to_string()),
            instance_side: Vec::new(),
            class_side: Vec::new(),
            new_name: None,
        }
    }

    /// An install of `edit` alone, prepared and not committed.
    fn prepared(
        interpreter: &mut Interpreter,
        id: MixinId,
        edit: &Edit,
    ) -> Result<PreparedInstall, String> {
        interpreter
            .enter(|mutator| mutator.prepare_entries(&[InstallEntry::Edit(id, edit.clone())]))
    }

    fn prepare(interpreter: &mut Interpreter, id: MixinId, edit: &Edit) -> PreparedInstall {
        match prepared(interpreter, id, edit) {
            Ok(prepared) => prepared,
            Err(diagnostic) => panic!("the edit was refused: {diagnostic}"),
        }
    }

    fn refusal(interpreter: &mut Interpreter, id: MixinId, edit: &Edit) -> String {
        match prepared(interpreter, id, edit) {
            Ok(prepared) => panic!("the edit was prepared: {}", prepared.module_texts[0]),
            Err(diagnostic) => diagnostic,
        }
    }

    fn compiled_method(mixin: &Mixin, selector: &str) -> Rc<crate::mixin::MixinMethod> {
        match mixin.methods.get(&Symbol::intern(selector)) {
            Some(MethodEntry::Compiled(method)) => method.clone(),
            _ => panic!(
                "{} has no compiled method {selector}",
                mixin.reflected_name()
            ),
        }
    }

    const OUTER: &str = indoc! {"
        class Outer = Object new () (
          public class Inner = () (
            public a = ( ^1 )
          )
        )
    "};

    /// An added method is appended to its side, and the rest of the
    /// module's text is kept byte for byte.
    #[test]
    fn adding_a_method_answers_the_declaration_with_it() {
        let (mut interpreter, outer) = load(OUTER);
        let (_, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        let prepared = prepare(&mut interpreter, inner_id, &add_method("public b = (^2)"));
        let close = OUTER.find("\n  )\n)").expect("Inner's members close") + 3;
        let expected = format!("{}\npublic b = (^2)\n{}", &OUTER[..close], &OUTER[close..]);
        assert_eq!(&*prepared.module_texts[0], expected);
        assert!(prepared.texts[0].declaration.contains("public a = ( ^1 )"));
        assert!(prepared.texts[0].declaration.contains("public b = (^2)"));
        assert!(
            prepared.texts[0]
                .declaration
                .starts_with("public class Inner")
        );
        assert!(
            prepared.texts[0]
                .declaration
                .ends_with("public b = (^2)\n)")
        );
        let (id, fresh) = &prepared.replacements[0];
        assert_eq!(*id, inner_id);
        assert!(fresh.methods.contains_key(&Symbol::intern("b")));
        assert!(fresh.methods.contains_key(&Symbol::intern("a")));
    }

    /// The header answered is the parse's, from the declaration's start to
    /// its members' parenthesis, whatever its text holds before it: a
    /// `///` comment ended by a carriage return alone, a block comment and
    /// a string, each holding a parenthesis.
    #[test]
    fn the_header_is_sliced_by_the_parse() {
        let header = "public class Inner = (\r    /// a comment ( ending at a CR\r    \
                      | public text = 'it''s ('. |\r    (* ( *)\r  ) ";
        let source =
            format!("class Outer = Object new () (\r  {header}(\r    public a = ( ^1 )\r  )\r)\r");
        let (mut interpreter, outer) = load(&source);
        let (_, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        let unchanged = prepare(&mut interpreter, inner_id, &instance_edit(Vec::new()));
        assert_eq!(unchanged.texts[0].header, header);
        let added = prepare(&mut interpreter, inner_id, &add_method("public b = (^2)"));
        assert_eq!(added.texts[0].header, header);
        assert!(added.texts[0].declaration.starts_with(header));
    }

    /// An added member replaces the one of its kind and name, a removed
    /// one goes with its comment, and removing a name the side does not
    /// declare changes nothing.
    #[test]
    fn replacing_and_removing_find_the_member_by_name() {
        let source = indoc! {"
            class Outer = Object new () (
              public class Inner = () (
                /// Answers one.
                public a = ( ^1 )
                public c = ( ^3 )
              )
            )
        "};
        let (mut interpreter, outer) = load(source);
        let (_, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        let replaced = prepare(&mut interpreter, inner_id, &add_method("public a = (^3)"));
        assert_eq!(
            &*replaced.module_texts[0],
            source.replace("/// Answers one.\n    public a = ( ^1 )", "public a = (^3)")
        );
        assert_eq!(replaced.module_texts[0].matches("public a").count(), 1);
        let removed = prepare(
            &mut interpreter,
            inner_id,
            &instance_edit(vec![MemberEdit::RemoveMethod("a".to_string())]),
        );
        // The removed member's lines go whole, their indentation with them.
        assert_eq!(
            &*removed.module_texts[0],
            source.replace("    /// Answers one.\n    public a = ( ^1 )\n", "")
        );
        let (_, fresh) = &removed.replacements[0];
        assert!(!fresh.methods.contains_key(&Symbol::intern("a")));
        assert!(fresh.methods.contains_key(&Symbol::intern("c")));
        let unchanged = prepare(
            &mut interpreter,
            inner_id,
            &instance_edit(vec![MemberEdit::RemoveMethod("missing".to_string())]),
        );
        assert_eq!(&*unchanged.module_texts[0], source);
    }

    /// A new header gives the fresh mixin the slots it declares.
    #[test]
    fn a_header_edit_changes_the_slots() {
        let (mut interpreter, outer) = load(OUTER);
        let (_, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        let edit = header_edit("public class Inner = (| public x = 1. public y = 2. |)");
        let prepared = prepare(&mut interpreter, inner_id, &edit);
        let (_, fresh) = &prepared.replacements[0];
        let slots: Vec<&str> = fresh.slots.iter().map(|slot| slot.name.as_str()).collect();
        assert_eq!(slots, ["x", "y"]);
        assert!(fresh.methods.contains_key(&Symbol::intern("a")));
    }

    /// Adding a method to an enclosing class rebinds the implicit send of
    /// an unchanged body nested in it: the whole subtree is compiled from
    /// the edited text.
    #[test]
    fn an_added_method_rebinds_a_nested_class_s_implicit_send() {
        let source = indoc! {"
            class Outer = Object new () (
              public class Inner = () (
                public probe = ( ^target )
              )
            )
        "};
        let (mut interpreter, outer) = load(source);
        let (root_id, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        let target_at_depth_one = |mixin: &Mixin| {
            compiled_method(mixin, "probe")
                .code
                .instructions()
                .iter()
                .any(|instruction| {
                    matches!(
                        instruction,
                        Instruction::SendLexical { depth: 1, selector, .. }
                            if selector.as_str() == "target"
                    )
                })
        };
        let old_inner = interpreter.enter(|mutator| {
            let root = root_mixin(mutator, &outer);
            mutator.heap.nested_mixins(&root)[0].clone()
        });
        assert!(!target_at_depth_one(&old_inner));
        let prepared = prepare(
            &mut interpreter,
            root_id,
            &add_method("public target = ( ^#outer )"),
        );
        let (id, fresh_outer) = &prepared.replacements[0];
        assert_eq!(*id, root_id);
        assert!(target_at_depth_one(&fresh_outer.declared_nested()[0]));
        let fresh_inner = prepared
            .replacements
            .iter()
            .find(|(id, _)| *id == inner_id)
            .map(|(_, fresh)| fresh.clone())
            .expect("the registered Inner is replaced");
        assert!(Rc::ptr_eq(&fresh_inner, &fresh_outer.declared_nested()[0]));
    }

    /// A class-side method added to a class that declares no class side
    /// gives it one, holding the method and the factory, under the id of
    /// the class side the old version had synthesized; an edit that
    /// leaves the class side undeclared synthesizes a fresh one instead.
    #[test]
    fn a_class_side_method_is_added_to_a_class_that_declares_none() {
        let (mut interpreter, outer) = load(OUTER);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "Outer");
        expect_ok(
            interpreter.send(&instance, Symbol::intern("Inner"), &[]),
            "Inner",
        );
        let (_, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        let synthesized_id = interpreter.enter(|mutator| {
            let synthesized = mutator
                .metaclass_mixins
                .get(&inner_id)
                .expect("Inner's class object synthesized its class side")
                .clone();
            mutator
                .heap
                .known_mixin_id(&synthesized)
                .expect("Inner's metaclass registered its class side")
        });
        let edit = Edit {
            header: None,
            instance_side: Vec::new(),
            class_side: vec![MemberEdit::AddMethod(
                "public make = ( ^self new )".to_string(),
            )],
            new_name: None,
        };
        let prepared = prepare(&mut interpreter, inner_id, &edit);
        assert!(prepared.module_texts[0].contains(") : (\npublic make = ( ^self new )\n)"));
        let (_, fresh) = &prepared.replacements[0];
        let class_side = fresh.class_side.clone().expect("a declared class side");
        assert!(class_side.methods.contains_key(&Symbol::intern("make")));
        assert!(class_side.methods.contains_key(&Symbol::NEW));
        let (id, replacement) = &prepared.replacements[1];
        assert_eq!(*id, synthesized_id);
        assert!(Rc::ptr_eq(replacement, &class_side));
        assert!(prepared.synthesized.is_empty());

        let prepared = prepare(&mut interpreter, inner_id, &add_method("public b = (^2)"));
        let (_, fresh) = &prepared.replacements[0];
        let [(instance_side_id, class_side_id, synthesized)] = prepared.synthesized.as_slice()
        else {
            panic!("the old version's synthesized class side is replaced");
        };
        assert_eq!(
            (*instance_side_id, *class_side_id),
            (inner_id, synthesized_id)
        );
        assert!(synthesized.is_class_side());
        assert!(synthesized.methods.contains_key(&Symbol::NEW));
        assert!(
            synthesized
                .enclosing_mixin()
                .is_some_and(|enclosing| Rc::ptr_eq(&enclosing, fresh))
        );
        assert_eq!(prepared.replacements.len(), 1);
    }

    /// An edit of an enclosing class pairs the synthesized class side of
    /// every nested class whose metaclass registered one, not only its own.
    #[test]
    fn an_enclosing_edit_pairs_a_nested_synthesized_class_side() {
        let (mut interpreter, outer) = load(OUTER);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "Outer");
        let inner = expect_ok(
            interpreter.send(&instance, Symbol::intern("Inner"), &[]),
            "Inner",
        );
        expect_ok(interpreter.send(&inner, Symbol::NEW, &[]), "Inner new");
        let (root_id, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        let synthesized_id = interpreter.enter(|mutator| {
            let synthesized = mutator
                .metaclass_mixins
                .get(&inner_id)
                .expect("Inner's class object synthesized its class side")
                .clone();
            mutator
                .heap
                .known_mixin_id(&synthesized)
                .expect("Inner's metaclass registered its class side")
        });
        let prepared = prepare(&mut interpreter, root_id, &add_method("public d = (^4)"));
        let (_, fresh_outer) = &prepared.replacements[0];
        let entry = prepared
            .synthesized
            .iter()
            .find(|(instance_side_id, _, _)| *instance_side_id == inner_id)
            .expect("Inner's synthesized class side is paired");
        assert_eq!(entry.1, synthesized_id);
        assert!(entry.2.is_class_side());
        assert!(
            entry
                .2
                .enclosing_mixin()
                .is_some_and(|enclosing| Rc::ptr_eq(&enclosing, &fresh_outer.declared_nested()[0]))
        );
    }

    /// The next edit of a module starts from its current text, so an
    /// enclosing class's edit keeps a nested class's earlier one.
    #[test]
    fn a_second_edit_keeps_the_first() {
        let (mut interpreter, outer) = load(OUTER);
        let (root_id, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        let first = prepare(&mut interpreter, inner_id, &add_method("public b = (^2)"));
        interpreter.enter(|mutator| {
            for (id, fresh) in &first.replacements {
                mutator.replace_mixin(*id, Rc::clone(fresh));
            }
        });
        let second = prepare(&mut interpreter, root_id, &add_method("public d = (^4)"));
        assert!(second.module_texts[0].contains("public b = (^2)"));
        assert!(second.module_texts[0].contains("public d = (^4)"));
        let (_, fresh_outer) = &second.replacements[0];
        assert!(
            fresh_outer.declared_nested()[0]
                .methods
                .contains_key(&Symbol::intern("b"))
        );
    }

    /// Spans are byte offsets: a splice between characters of several
    /// bytes keeps every one of them.
    #[test]
    fn a_splice_around_multibyte_text_keeps_every_character() {
        let source = indoc! {"
            class Outer = Object new () (
              public class Inner = () (
                /// Grüße 🙂
                public a = ( ^'ä🙂' )
                /// Ende ä
                public c = ( ^'🙂' )
              )
            )
        "};
        // The appended method's line break follows the indentation that
        // stood before the closing `)`, which indoc would trim.
        let expected = indoc! {"
            class Outer = Object new () (
              public class Inner = () (
                public a = ( ^'ö🙂ü' )
                /// Ende ä
                public c = ( ^'🙂' )
            INDENT
            public b = ( ^'é' )
            )
            )
        "}
        .replace("INDENT", "  ");
        let (mut interpreter, outer) = load(source);
        let (_, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        let edit = instance_edit(vec![
            MemberEdit::AddMethod("public a = ( ^'ö🙂ü' )".to_string()),
            MemberEdit::AddMethod("public b = ( ^'é' )".to_string()),
        ]);
        let prepared = prepare(&mut interpreter, inner_id, &edit);
        assert_eq!(&*prepared.module_texts[0], expected);
        let (_, fresh) = &prepared.replacements[0];
        let method_source = crate::mixin::source_text(
            &prepared.module_texts[0],
            compiled_method(fresh, "a").code.source,
        );
        assert_eq!(method_source, Some("public a = ( ^'ö🙂ü' )"));
    }

    fn rename(new_name: &str) -> Edit {
        Edit {
            header: None,
            instance_side: Vec::new(),
            class_side: Vec::new(),
            new_name: Some(Symbol::intern(new_name)),
        }
    }

    /// A rename and an access change compile the enclosing declaration
    /// again, whose accessor and bindings name the class: the enclosing
    /// mixin is replaced first, the renamed one within it.
    // spec: runtime.install
    #[test]
    fn a_rename_widens_the_edit_to_the_enclosing_declaration() {
        let (mut interpreter, outer) = load(OUTER);
        let (root_id, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        for (edit, name, access) in [
            (rename("Renamed"), "Renamed", ast::AccessModifier::Public),
            (
                header_edit("public class Renamed = ()"),
                "Renamed",
                ast::AccessModifier::Public,
            ),
            (
                header_edit("private class Inner = ()"),
                "Inner",
                ast::AccessModifier::Private,
            ),
        ] {
            let prepared = prepare(&mut interpreter, inner_id, &edit);
            let (id, fresh_outer) = &prepared.replacements[0];
            assert_eq!(*id, root_id);
            let fresh_inner = &fresh_outer.declared_nested()[0];
            assert_eq!(fresh_inner.name.as_str(), name);
            assert_eq!(fresh_inner.access, access);
        }
        let prepared = prepare(&mut interpreter, inner_id, &rename("Renamed"));
        assert!(prepared.module_texts[0].contains("public class Renamed = () ("));
        assert!(prepared.texts[0].header.starts_with("public class Renamed"));
        // An edit that keeps the name and the access compiles the class
        // alone.
        let kept = prepare(
            &mut interpreter,
            inner_id,
            &header_edit("public class Inner = ()"),
        );
        assert_eq!(kept.replacements[0].0, inner_id);
    }

    /// The renamed class is paired with its old version by the name it
    /// had, so it keeps its id.
    // spec: runtime.install
    #[test]
    fn a_renamed_child_keeps_its_id() {
        let (mut interpreter, outer) = load(OUTER);
        let (_, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        for edit in [rename("Renamed"), header_edit("public class Renamed = ()")] {
            let prepared = prepare(&mut interpreter, inner_id, &edit);
            let (_, fresh_outer) = &prepared.replacements[0];
            let fresh_inner = prepared
                .replacements
                .iter()
                .find(|(id, _)| *id == inner_id)
                .map(|(_, fresh)| fresh.clone())
                .expect("the renamed Inner is replaced under its id");
            assert_eq!(fresh_inner.name.as_str(), "Renamed");
            assert!(Rc::ptr_eq(&fresh_inner, &fresh_outer.declared_nested()[0]));
            assert!(fresh_inner.methods.contains_key(&Symbol::intern("a")));
        }
    }

    /// A rename to a name the enclosing class already declares is refused,
    /// naming the enclosing class.
    // spec: runtime.install
    #[test]
    fn a_rename_to_an_existing_member_is_refused() {
        let source = indoc! {"
            class Outer = Object new (| public slot = 1. |) (
              public class Inner = () ()
              public class Sibling = () ()
              public probe = ( ^1 )
            )
        "};
        let (mut interpreter, outer) = load(source);
        let (_, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        for taken in ["Sibling", "probe", "slot"] {
            assert_eq!(
                refusal(&mut interpreter, inner_id, &rename(taken)),
                format!("Outer already has a member named {taken}")
            );
        }
        assert_eq!(
            refusal(
                &mut interpreter,
                inner_id,
                &header_edit("public class Sibling = ()")
            ),
            "Outer already has a member named Sibling"
        );
    }

    /// A root's header begins at its first metadata comment, so that the
    /// header can change the module's id and version; a nested class's
    /// begins at the class.
    // spec: runtime.install
    #[test]
    fn a_root_header_holds_its_metadata() {
        let source = indoc! {"
            /// A module.
            (* :module: com.acme.Outer@1.0.0 *)
            class Outer = Object new () (
              public a = ( ^1 )
            )
        "};
        let (mut interpreter, outer) = load(source);
        let root_id = interpreter.enter(|mutator| {
            let root = root_mixin(mutator, &outer);
            mutator.heap.mixin_id(&root)
        });
        let unchanged = prepare(&mut interpreter, root_id, &instance_edit(Vec::new()));
        assert_eq!(
            unchanged.texts[0].header,
            "(* :module: com.acme.Outer@1.0.0 *)\nclass Outer = Object new () "
        );
        let edited = prepare(
            &mut interpreter,
            root_id,
            &header_edit("(* :module: com.acme.Outer@1.1.0 *)\nclass Outer = Object new () "),
        );
        assert!(
            edited.module_texts[0]
                .starts_with("/// A module.\n(* :module: com.acme.Outer@1.1.0 *)\nclass Outer")
        );
        let renamed = prepare(&mut interpreter, root_id, &rename("Codec"));
        assert_eq!(
            renamed.texts[0].header,
            "(* :module: com.acme.Codec@1.0.0 *)\nclass Codec = Object new () "
        );
    }

    /// A root's rename rewrites the id's last part in the `:module:`
    /// payload, never the tag, even where the payload spells the tag.
    // spec: runtime.install
    #[test]
    fn a_root_rename_rewrites_the_payload_not_the_tag() {
        let source = indoc! {"
            (* :module: module *)
            class module = Object new () ()
        "};
        let (mut interpreter, outer) = load(source);
        let root_id = interpreter.enter(|mutator| {
            let root = root_mixin(mutator, &outer);
            mutator.heap.mixin_id(&root)
        });
        let renamed = prepare(&mut interpreter, root_id, &rename("codec"));
        assert_eq!(
            &*renamed.module_texts[0],
            "(* :module: codec *)\nclass codec = Object new () ()\n"
        );
    }

    /// The edits of `OUTER`'s `Inner` that `every_refusal_answers_its_diagnostic`
    /// and `a_failed_preparation_changes_nothing` refuse, with their
    /// diagnostics.
    fn refused_edits() -> Vec<(Edit, &'static str)> {
        vec![
            (
                rename("+"),
                "Inner cannot be renamed to +, which is no identifier",
            ),
            (
                header_edit("public class Inner = () () class Sneaked = ()"),
                "Inner's edited source does not parse: the header must declare the class and \
                 end where its members begin (line 2)",
            ),
            (
                add_method("lazy x = 1."),
                "addFromSource: of Inner's methods takes one method declaration",
            ),
            (
                instance_edit(vec![MemberEdit::AddLazySlot("public b = (^2)".to_string())]),
                "addFromSource: of Inner's lazy slots takes one lazy slot declaration",
            ),
            (
                add_method("public b = ( ^ )"),
                "Inner's edited source does not parse: expected an expression, found RightParenthesis (line 5)",
            ),
            (
                instance_edit(vec![MemberEdit::AddLazySlot("lazy a = 1.".to_string())]),
                "Inner's edited source does not resolve: class `Inner` has multiple \
                 definitions of `a` (line 2)",
            ),
        ]
    }

    /// Every refusal answers its diagnostic, the line in the edited text
    /// included; a shipped mixin is refused before anything is read.
    #[test]
    fn every_refusal_answers_its_diagnostic() {
        let (mut interpreter, outer) = load(OUTER);
        let (_, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        for (edit, expected) in refused_edits() {
            assert_eq!(refusal(&mut interpreter, inner_id, &edit), expected);
        }
        let list_id = interpreter.enter(|mutator| {
            let collections = mutator.shipped_root_mixin("CollectionsFactory");
            let list = collections
                .declared_nested()
                .iter()
                .find(|nested| nested.name.as_str() == "List")
                .expect("CollectionsFactory declares List")
                .clone();
            mutator.heap.mixin_id(&list)
        });
        assert_eq!(
            refusal(&mut interpreter, list_id, &add_method("public b = (^2)")),
            "List belongs to the shipped core library, which no MirrorBuilder edits"
        );
    }

    /// A refused edit leaves the mixin's current version, the module's
    /// current text and the tables as they were.
    #[test]
    fn a_failed_preparation_changes_nothing() {
        let (mut interpreter, outer) = load(OUTER);
        let (root_id, inner_id) = root_and_inner_ids(&mut interpreter, &outer);
        let (version, text, mixin_count, application_count) = interpreter.enter(|mutator| {
            (
                mutator.heap.mixin(inner_id).expect("Inner's row"),
                mutator.heap.module_source(root_id).expect("Outer's text"),
                mixin_count_of(mutator.heap()),
                application_count_of(mutator.heap()),
            )
        });
        for (edit, _) in refused_edits() {
            refusal(&mut interpreter, inner_id, &edit);
            interpreter.enter(|mutator| {
                let current = mutator.heap.mixin(inner_id).expect("Inner's row");
                assert!(Rc::ptr_eq(&current, &version));
                let current_text = mutator.heap.module_source(root_id).expect("Outer's text");
                assert_eq!(current_text, text);
                assert_eq!(mixin_count_of(mutator.heap()), mixin_count);
                assert_eq!(application_count_of(mutator.heap()), application_count);
            });
        }
    }

    /// The number of ids `heap` has minted.
    fn mixin_count_of(heap: &Heap<'_>) -> usize {
        (0..)
            .take_while(|index| {
                MixinId::from_index(*index).is_some_and(|id| heap.mixin(id).is_some())
            })
            .count()
    }

    /// The number of applications `heap` holds: its rows but those a
    /// collection freed.
    fn application_count_of(heap: &Heap<'_>) -> usize {
        (0..heap.application_rows())
            .filter(|index| {
                crate::heap::ApplicationIndex::from_index(*index)
                    .is_some_and(|index| heap.application(index).is_some())
            })
            .count()
    }

    /// The operation `{kind. argument}` of an edit's `Array`.
    fn operation<'gc>(mutator: &mut Mutator<'gc>, kind: &str, argument: Value<'gc>) -> Value<'gc> {
        let kind = mutator.new_symbol(Symbol::intern(kind));
        Value::object(mutator.new_array(vec![kind, argument]))
    }

    /// An edit decodes from the `Array` a builder hands the leaves, and
    /// anything of another shape declines.
    #[test]
    fn an_edit_decodes_from_its_array() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let header = Value::object(mutator.new_string("class C = ()".to_string()));
            let method = Value::object(mutator.new_string("m = (^1)".to_string()));
            let lazy_slot = Value::object(mutator.new_string("lazy s = 1.".to_string()));
            let symbol = mutator.new_symbol(Symbol::intern("gone"));
            let instance_operations = vec![
                operation(mutator, "addMethod", method),
                operation(mutator, "addLazySlot", lazy_slot),
                operation(mutator, "removeLazySlot", symbol),
            ];
            let class_operations = vec![operation(mutator, "removeMethod", symbol)];
            let lazy_class_side = vec![operation(mutator, "addLazySlot", lazy_slot)];
            let instance_operations = Value::object(mutator.new_array(instance_operations));
            let class_operations = Value::object(mutator.new_array(class_operations));
            let lazy_class_side = Value::object(mutator.new_array(lazy_class_side));
            let edit = Value::object(mutator.new_array(vec![
                header,
                instance_operations,
                class_operations,
                Value::nil(),
            ]));
            let decoded = Edit::from_value(mutator.heap(), edit).expect("an edit");
            assert_eq!(decoded.header.as_deref(), Some("class C = ()"));
            assert!(decoded.new_name.is_none());
            assert!(matches!(
                decoded.instance_side.as_slice(),
                [
                    MemberEdit::AddMethod(method),
                    MemberEdit::AddLazySlot(slot),
                    MemberEdit::RemoveLazySlot(removed),
                ] if method == "m = (^1)" && slot == "lazy s = 1." && removed == "gone"
            ));
            assert!(matches!(
                decoded.class_side.as_slice(),
                [MemberEdit::RemoveMethod(removed)] if removed == "gone"
            ));

            let renamed = mutator.new_symbol(Symbol::intern("Renamed"));
            let unchanged_header = Value::object(mutator.new_array(vec![
                Value::nil(),
                instance_operations,
                class_operations,
                renamed,
            ]));
            let decoded = Edit::from_value(mutator.heap(), unchanged_header).expect("an edit");
            assert!(decoded.header.is_none());
            assert_eq!(decoded.new_name, Some(Symbol::intern("Renamed")));

            let symbol_header = mutator.new_symbol(Symbol::intern("header"));
            let string_name = Value::object(mutator.new_string("Renamed".to_string()));
            for declined in [
                vec![
                    Value::nil(),
                    instance_operations,
                    lazy_class_side,
                    Value::nil(),
                ],
                vec![
                    symbol_header,
                    instance_operations,
                    class_operations,
                    Value::nil(),
                ],
                vec![Value::nil(), instance_operations, class_operations],
                vec![
                    Value::nil(),
                    Value::small_integer(3),
                    class_operations,
                    Value::nil(),
                ],
                vec![
                    Value::nil(),
                    instance_operations,
                    class_operations,
                    string_name,
                ],
            ] {
                let value = Value::object(mutator.new_array(declined));
                assert!(Edit::from_value(mutator.heap(), value).is_none());
            }
            assert!(Edit::from_value(mutator.heap(), Value::small_integer(1)).is_none());
        });
    }
}

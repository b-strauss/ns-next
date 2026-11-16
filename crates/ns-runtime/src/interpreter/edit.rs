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
/// over: a new header, or `None` to keep it, and the member operations of
/// each side, applied in order.
pub(crate) struct Edit {
    pub(crate) header: Option<String>,
    pub(crate) instance_side: Vec<MemberEdit>,
    pub(crate) class_side: Vec<MemberEdit>,
}

/// One member operation: a member added from its source text, replacing
/// one of the same kind and name, or one removed by name. A removed name is
/// the text of the Symbol the program handed over, never an interned
/// symbol: a program chooses it, and a name no member has -- a
/// program-made symbol's among them -- removes nothing.
pub(crate) enum MemberEdit {
    AddMethod(String),
    RemoveMethod(String),
    AddLazySlot(String),
    RemoveLazySlot(String),
}

/// What an edit the VM accepts answers the MirrorBuilder: the edited
/// declaration's text and its header's, both sliced from the edited
/// module's text by the spans of its parse, so that the builder never
/// reads Newspeak syntax itself.
pub(crate) struct EditedTexts {
    /// The edited declaration's text, from its start to its closing
    /// parenthesis.
    pub(crate) declaration: String,
    /// The edited declaration's header: from its start up to, not
    /// including, the parenthesis that opens its members; the whole
    /// declaration for a mixin application without a body.
    pub(crate) header: String,
}

/// What preparing an edit answers: everything the install needs, built and
/// registered nowhere.
pub(crate) struct PreparedEdit {
    /// The whole edited module's text, from which each fresh mixin cut
    /// its declaration's: the install reads each fresh mixin's own, and
    /// the tests read the whole.
    #[cfg(test)]
    pub(crate) module_text: Rc<str>,
    /// The edited declaration's and its header's text, sliced from the
    /// edited module's text.
    pub(crate) texts: EditedTexts,
    /// Every fresh mixin with the id it is installed under: the edited
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
    /// classOperations }`; `None` for anything else, which the leaf
    /// declines. A class side takes method operations only, since §6.3's
    /// class side declares nothing else.
    pub(crate) fn from_value<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<Edit> {
        let elements = heap.array_elements(value.as_object()?)?;
        let [header, instance_side, class_side] = elements.as_slice() else {
            return None;
        };
        let header = if header.is_nil() {
            None
        } else {
            Some(string_of(heap, *header)?)
        };
        Some(Edit {
            header,
            instance_side: operations_of(heap, *instance_side, true)?,
            class_side: operations_of(heap, *class_side, false)?,
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
/// argument.
fn operations_of<'gc>(
    heap: &Heap<'gc>,
    value: Value<'gc>,
    lazy_slots: bool,
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
                "addLazySlot" if lazy_slots => MemberEdit::AddLazySlot(string_of(heap, *argument)?),
                "removeLazySlot" if lazy_slots => {
                    MemberEdit::RemoveLazySlot(symbol_text_of(heap, *argument)?)
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
}

impl<'gc> Mutator<'gc> {
    /// The edit `edit` of the mixin `id` names, compiled fresh from its
    /// module's current text with the edit spliced in, or the diagnostic
    /// that refuses it (`docs/design/liveness-and-reflection.md`,
    /// "Compiling: the module's text is what an edit edits").
    ///
    /// Changes nothing a program observes: no table, application or
    /// object. Registers no fresh mixin.
    pub(crate) fn prepare_edit(
        &mut self,
        id: MixinId,
        edit: &Edit,
    ) -> Result<PreparedEdit, String> {
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
        debug_assert!(
            !current.is_class_side(),
            "internal: an edit names a declaration's instance side"
        );
        if current.is_class_side() {
            return Err(format!(
                "{name} is a class side; an edit names its instance side"
            ));
        }
        let (module_root, path) = self.path_from_module_root(id);
        let Some(original_text) = self.heap.module_source(module_root) else {
            return Err(format!("{name} has no source text to edit"));
        };
        let original = parse_module(&original_text, &name)?;
        let Some(original_declaration) = find_declaration(&original.root_class, &path) else {
            return Err(format!(
                "{name}'s source text does not hold its declaration"
            ));
        };
        let mut text = original_text.to_string();
        for operation in &edit.instance_side {
            text = apply_member_edit(&text, &path, Side::Instance, operation, &name)?;
        }
        for operation in &edit.class_side {
            text = apply_member_edit(&text, &path, Side::Class, operation, &name)?;
        }
        if let Some(header) = &edit.header {
            // Last, because it may rename: every operation before it finds
            // the declaration by its old name.
            text = replace_header(&text, &path, header, &name)?;
        }
        let edited = parse_module(&text, &name)?;
        check_kept_header(
            &original_text,
            original_declaration,
            &text,
            &edited,
            &path,
            edit.header.as_ref().map(String::len),
            &name,
        )?;
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
            format!(
                "{name}'s edited source does not resolve: {message} (line {})",
                line_of(&text, start)
            )
        })?;
        let Some(declaration) = find_declaration(&resolved.module.root_class, &path) else {
            return Err(format!(
                "{name}'s edited source does not hold its declaration"
            ));
        };
        let fresh = if path.is_empty() {
            self.build_root_mixin(declaration, &resolved.bindings, ModuleSource::User, &text)
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
        let declaration_text = crate::mixin::source_text(&text, fresh.span)
            .unwrap_or_default()
            .to_string();
        let header_end = declaration
            .instance_side
            .members_delimiters
            .map_or(declaration.span.end, |members| members.open.start);
        let header_text = usize::try_from(declaration.span.start)
            .ok()
            .zip(usize::try_from(header_end).ok())
            .and_then(|(start, end)| text.get(start..end))
            .unwrap_or_default()
            .to_string();
        let mut replacements = Vec::new();
        let mut synthesized = Vec::new();
        self.pair(id, &current, &fresh, &mut replacements, &mut synthesized);
        Ok(PreparedEdit {
            #[cfg(test)]
            module_text: text,
            texts: EditedTexts {
                declaration: declaration_text,
                header: header_text,
            },
            replacements,
            synthesized,
        })
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
    /// subtree keeps its mixin's id.
    fn pair(
        &self,
        id: MixinId,
        old: &Rc<Mixin>,
        fresh: &Rc<Mixin>,
        replacements: &mut Vec<(MixinId, Rc<Mixin>)>,
        synthesized: &mut Vec<(MixinId, MixinId, Rc<Mixin>)>,
    ) {
        replacements.push((id, Rc::clone(fresh)));
        if let Some(class_side_id) = self.old_class_side_id(id, old) {
            match &fresh.class_side {
                Some(class_side) => replacements.push((class_side_id, Rc::clone(class_side))),
                None => synthesized.push((
                    id,
                    class_side_id,
                    super::metaclass::synthesized_class_side(fresh),
                )),
            }
        }
        for nested in fresh.declared_nested().iter() {
            let counterpart = self.heap.nested_of(id).iter().copied().find(|old_id| {
                self.heap
                    .mixin(*old_id)
                    .is_some_and(|old_nested| old_nested.name == nested.name)
            });
            let Some(nested_id) = counterpart else {
                continue;
            };
            let current = self
                .heap
                .mixin(nested_id)
                .expect("internal: the counterpart was found by its row");
            self.pair(nested_id, &current, nested, replacements, synthesized);
        }
    }
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

/// The kind and name of a method or a lazy slot; `None` for a class.
fn member_kind_and_name(member: &ast::Member) -> Option<(MemberKind, String)> {
    match member {
        ast::Member::Method(method) => Some((MemberKind::Method, method_name(&method.pattern))),
        ast::Member::LazySlot(slot) => Some((MemberKind::LazySlot, slot.name.name.clone())),
        ast::Member::Class(_) => None,
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
    Ok(member_kind_and_name(member))
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
    };
    let body = match side {
        Side::Instance => Some(&declaration.instance_side),
        Side::Class => declaration.class_side.as_ref(),
    };
    let existing = |member_name: &str| {
        body.and_then(|body| {
            body.members.iter().find(|member| {
                member_kind_and_name(member).is_some_and(|(member_kind, declared)| {
                    member_kind == kind && declared == member_name
                })
            })
        })
    };
    if let Some(removed) = removed {
        return Ok(match existing(removed) {
            Some(member) => {
                let (start, end) = member_range(member);
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

/// `text` with the header of the declaration `path` names -- from its
/// start to its members' `(` -- replaced by `header`.
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
        declaration.span.start,
        members.open.start,
        header,
    ))
}

/// Refuses an edited module whose declaration lost its place, its name,
/// its access, its superclass clause or its `<:` clauses. Everything before the
/// declaration's start is untouched by the splices, so the edited
/// declaration is the class that starts where the old one did; a header
/// edit must end where its members' `(` begins, or it spliced a
/// declaration of its own into the enclosing class.
fn check_kept_header(
    original_text: &str,
    original: &ast::ClassDeclaration,
    text: &str,
    edited: &ast::Module,
    path: &[Symbol],
    header_length: Option<usize>,
    name: &str,
) -> Result<(), String> {
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
                    members.open.start as usize == declaration_start as usize + length
                })
        })
    };
    let Some(declaration) = declaration.filter(|declaration| header_kept_in_place(declaration))
    else {
        return Err(syntax_diagnostic(
            name,
            "the header must declare the class and end where its members begin",
            text,
            declaration_start,
        ));
    };
    if declaration.name.name != original.name.name {
        return Err(format!(
            "the header renames {name} to {}; an edit keeps a class's name",
            declaration.name.name
        ));
    }
    // The class's accessor in its enclosing class's method table carries
    // the access, and the enclosing class keeps its version: an edit
    // that changed it would answer one access from the mirror and honour
    // another in every send.
    if crate::dispatch::effective_access(declaration.access)
        != crate::dispatch::effective_access(original.access)
    {
        return Err(format!(
            "the header changes the access of {name}; an edit keeps it"
        ));
    }
    let clause_text =
        |source: &'_ str, clause: &ast::SuperclassClause| clause.span.text(source).to_string();
    let original_clauses: Vec<String> = original
        .superclass
        .iter()
        .chain(&original.mixin_applications)
        .map(|clause| clause_text(original_text, clause))
        .collect();
    let edited_clauses: Vec<String> = declaration
        .superclass
        .iter()
        .chain(&declaration.mixin_applications)
        .map(|clause| clause_text(text, clause))
        .collect();
    if original.superclass.is_some() != declaration.superclass.is_some()
        || original_clauses != edited_clauses
    {
        return Err(format!(
            "the header changes the superclass of {name}; an edit keeps it"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Root;
    use crate::bytecode::Instruction;
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
        }
    }

    fn prepare(interpreter: &mut Interpreter, id: MixinId, edit: &Edit) -> PreparedEdit {
        match interpreter.enter(|mutator| mutator.prepare_edit(id, edit)) {
            Ok(prepared) => prepared,
            Err(diagnostic) => panic!("the edit was refused: {diagnostic}"),
        }
    }

    fn refusal(interpreter: &mut Interpreter, id: MixinId, edit: &Edit) -> String {
        match interpreter.enter(|mutator| mutator.prepare_edit(id, edit)) {
            Ok(prepared) => panic!("the edit was prepared: {}", prepared.module_text),
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
        assert_eq!(&*prepared.module_text, expected);
        assert!(prepared.texts.declaration.contains("public a = ( ^1 )"));
        assert!(prepared.texts.declaration.contains("public b = (^2)"));
        assert!(prepared.texts.declaration.starts_with("public class Inner"));
        assert!(prepared.texts.declaration.ends_with("public b = (^2)\n)"));
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
        assert_eq!(unchanged.texts.header, header);
        let added = prepare(&mut interpreter, inner_id, &add_method("public b = (^2)"));
        assert_eq!(added.texts.header, header);
        assert!(added.texts.declaration.starts_with(header));
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
            &*replaced.module_text,
            source.replace("/// Answers one.\n    public a = ( ^1 )", "public a = (^3)")
        );
        assert_eq!(replaced.module_text.matches("public a").count(), 1);
        let removed = prepare(
            &mut interpreter,
            inner_id,
            &instance_edit(vec![MemberEdit::RemoveMethod("a".to_string())]),
        );
        assert_eq!(
            &*removed.module_text,
            source.replace("/// Answers one.\n    public a = ( ^1 )", "")
        );
        let (_, fresh) = &removed.replacements[0];
        assert!(!fresh.methods.contains_key(&Symbol::intern("a")));
        assert!(fresh.methods.contains_key(&Symbol::intern("c")));
        let unchanged = prepare(
            &mut interpreter,
            inner_id,
            &instance_edit(vec![MemberEdit::RemoveMethod("missing".to_string())]),
        );
        assert_eq!(&*unchanged.module_text, source);
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
        };
        let prepared = prepare(&mut interpreter, inner_id, &edit);
        assert!(
            prepared
                .module_text
                .contains(") : (\npublic make = ( ^self new )\n)")
        );
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
        assert!(second.module_text.contains("public b = (^2)"));
        assert!(second.module_text.contains("public d = (^4)"));
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
        assert_eq!(&*prepared.module_text, expected);
        let (_, fresh) = &prepared.replacements[0];
        let method_source = crate::mixin::source_text(
            &prepared.module_text,
            compiled_method(fresh, "a").code.source,
        );
        assert_eq!(method_source, Some("public a = ( ^'ö🙂ü' )"));
    }

    /// The edits of `OUTER`'s `Inner` that `every_refusal_answers_its_diagnostic`
    /// and `a_failed_preparation_changes_nothing` refuse, with their
    /// diagnostics.
    fn refused_edits() -> Vec<(Edit, &'static str)> {
        vec![
            (
                header_edit("public class Renamed = ()"),
                "the header renames Inner to Renamed; an edit keeps a class's name",
            ),
            (
                header_edit("private class Inner = ()"),
                "the header changes the access of Inner; an edit keeps it",
            ),
            (
                header_edit("public class Inner = Object new ()"),
                "the header changes the superclass of Inner; an edit keeps it",
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
            ]));
            let decoded = Edit::from_value(mutator.heap(), edit).expect("an edit");
            assert_eq!(decoded.header.as_deref(), Some("class C = ()"));
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

            let unchanged_header = Value::object(mutator.new_array(vec![
                Value::nil(),
                instance_operations,
                class_operations,
            ]));
            let decoded = Edit::from_value(mutator.heap(), unchanged_header).expect("an edit");
            assert!(decoded.header.is_none());

            let symbol_header = mutator.new_symbol(Symbol::intern("header"));
            for declined in [
                vec![Value::nil(), instance_operations, lazy_class_side],
                vec![symbol_header, instance_operations, class_operations],
                vec![Value::nil(), instance_operations],
                vec![Value::nil(), Value::small_integer(3), class_operations],
            ] {
                let value = Value::object(mutator.new_array(declined));
                assert!(Edit::from_value(mutator.heap(), value).is_none());
            }
            assert!(Edit::from_value(mutator.heap(), Value::small_integer(1)).is_none());
        });
    }
}

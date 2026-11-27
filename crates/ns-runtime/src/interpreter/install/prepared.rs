//! Preparing an install of several builders' entries as one: the entries
//! refused together before any text is touched, grouped by module, each
//! module's entries spliced into one text in their order and compiled
//! once (`Mutator::prepare_module`), and each module declared from source
//! compiled on its own (`Mutator::prepare_source_module`); the install epoch that tells a
//! commit whether anything was installed since its preparation; and the
//! one prepared install kept between the leaves that prepare and commit
//! it (`docs/design/liveness-and-reflection.md`, "Planned: the complete
//! install", decision 8).

use std::rc::Rc;

use crate::heap::{ClassReference, MixinId};
use crate::interpreter::Mutator;
use crate::interpreter::edit::{Edit, EditedTexts, InstallEntry};
use crate::mixin::Mixin;

/// What preparing an install answers: everything its commit needs, built
/// and registered nowhere, and the epoch it was prepared at.
pub(crate) struct PreparedInstall {
    /// The install epoch the preparation read the world at.
    pub(crate) epoch: u64,
    /// Per entry, in order: the texts the builder reads afterwards.
    pub(crate) texts: Vec<EditedTexts>,
    /// Every fresh mixin with the id it is installed under, module by
    /// module (`PreparedModule::replacements`).
    pub(crate) replacements: Vec<(MixinId, Rc<Mixin>)>,
    /// Every fresh synthesized class side, module by module
    /// (`PreparedModule::synthesized`).
    pub(crate) synthesized: Vec<(MixinId, MixinId, Rc<Mixin>)>,
    /// Every nested declaration added, with the id of the declaration it
    /// is added to, module by module (`PreparedModule::added`).
    pub(crate) added: Vec<(MixinId, Rc<Mixin>)>,
    /// Every nested declaration removed, module by module
    /// (`PreparedModule::removed`).
    pub(crate) removed: Vec<MixinId>,
    /// Every module declared from source, its root mixin with the
    /// position of its entry, in entry order, which the commit registers
    /// with its tree of ids.
    pub(crate) modules: Vec<(usize, Rc<Mixin>)>,
    /// Every declaration whose superclass clause or `<:` clauses the
    /// install changes, module by module (`PreparedModule::clauses_changed`):
    /// a nested one's class objects each have their new clause evaluated
    /// before the commit (`Mutator::clause_class_objects`), and a root's
    /// class object is re-applied over its new static superclass.
    pub(crate) clauses_changed: Vec<MixinId>,
    /// Each edited module's whole text, in the order the modules were
    /// first named.
    #[cfg(test)]
    pub(crate) module_texts: Vec<Rc<str>>,
}

impl PreparedInstall {
    /// The fresh mixin the install puts under `id`.
    pub(crate) fn fresh_version(&self, id: MixinId) -> Option<&Rc<Mixin>> {
        self.replacements
            .iter()
            .find(|(replaced, _)| *replaced == id)
            .map(|(_, fresh)| fresh)
    }
}

/// The one prepared install a later preparation or commit of the same
/// entries at the same epoch takes instead of compiling again: the epoch,
/// the entries and what they prepared to. It holds ids, texts and
/// `Rc<Mixin>`s, never a handle, so it outlives any safepoint.
pub(crate) type PreparedInstallCache = Option<(u64, Vec<InstallEntry>, Rc<PreparedInstall>)>;

impl<'gc> Mutator<'gc> {
    /// The install of `entries`, prepared, or the diagnostic that refuses
    /// it; the cached one when the entries and the epoch are the cache's.
    ///
    /// Changes nothing a program observes.
    pub(crate) fn prepare_install(
        &mut self,
        entries: &[InstallEntry],
    ) -> Result<Rc<PreparedInstall>, String> {
        if let Some((epoch, cached, prepared)) = &self.prepared_install
            && *epoch == self.install_epoch
            && cached.as_slice() == entries
        {
            return Ok(Rc::clone(prepared));
        }
        let prepared = Rc::new(self.prepare_entries(entries)?);
        self.prepared_install = Some((self.install_epoch, entries.to_vec(), Rc::clone(&prepared)));
        Ok(prepared)
    }

    /// The prepared install a commit at `epoch` installs: refused when an
    /// install or a module load moved the epoch since the preparation the
    /// epoch came from, so that nothing evaluated against the world before
    /// it is committed against the world after.
    pub(crate) fn prepared_for_commit(
        &mut self,
        entries: &[InstallEntry],
        epoch: u64,
    ) -> Result<Rc<PreparedInstall>, String> {
        if epoch != self.install_epoch
            && let Some(first) = entries.first()
        {
            let name = self.entry_name(first);
            return Err(format!(
                "an install ran between the preparation and the commit of {name}'s install; \
                 nothing was installed"
            ));
        }
        self.prepare_install(entries)
    }

    /// The name a diagnostic gives the entry: its declaration's reflected
    /// name, a module's from source the name its text gives its class.
    fn entry_name(&self, entry: &InstallEntry) -> String {
        match entry {
            InstallEntry::Edit(id, _) => self
                .heap
                .mixin(*id)
                .map_or_else(String::new, |mixin| mixin.reflected_name()),
            InstallEntry::Module(text) => ns_syntax::parse(text).map_or_else(
                |_| "a module from source".to_string(),
                |module| module.root_class.name.name,
            ),
        }
    }

    /// Prepares `entries` without the cache. Every entry is checked, and
    /// two of one declaration refused, before any text is touched; then
    /// the entries are grouped by the module they edit, in the order each
    /// module is first named, and each module prepared once; then each
    /// module declared from source is compiled; and last every identity
    /// the install makes or changes is held to the world's.
    pub(crate) fn prepare_entries(
        &mut self,
        entries: &[InstallEntry],
    ) -> Result<PreparedInstall, String> {
        let mut edits: Vec<(MixinId, &Edit)> = Vec::with_capacity(entries.len());
        // The position in `entries` of each edit and of each module text.
        let mut edit_positions: Vec<usize> = Vec::with_capacity(entries.len());
        let mut source_modules: Vec<(usize, &str)> = Vec::new();
        for (position, entry) in entries.iter().enumerate() {
            match entry {
                InstallEntry::Edit(id, edit) => {
                    let (_, name) = self.editable_declaration(*id)?;
                    if edits.iter().any(|(other, _)| other == id) {
                        return Err(format!("two builders edit {name}"));
                    }
                    edits.push((*id, edit));
                    edit_positions.push(position);
                }
                InstallEntry::Module(text) => source_modules.push((position, text)),
            }
        }
        // Each module with the positions of its entries, in the order the
        // modules are first named.
        let mut modules: Vec<(MixinId, Vec<usize>)> = Vec::new();
        for (position, (id, _)) in edits.iter().enumerate() {
            let (module_root, _) = self.path_from_module_root(*id);
            match modules.iter_mut().find(|(root, _)| *root == module_root) {
                Some((_, positions)) => positions.push(position),
                None => modules.push((module_root, vec![position])),
            }
        }
        let mut texts: Vec<Option<EditedTexts>> = vec![None; entries.len()];
        let mut replacements = Vec::new();
        let mut synthesized = Vec::new();
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut clauses_changed = Vec::new();
        #[cfg(test)]
        let mut edited_texts = Vec::new();
        for (module_root, positions) in modules {
            let module_entries: Vec<(MixinId, &Edit)> =
                positions.iter().map(|position| edits[*position]).collect();
            let prepared = self.prepare_module(module_root, &module_entries)?;
            for (position, edited) in positions.into_iter().zip(prepared.texts) {
                texts[edit_positions[position]] = Some(edited);
            }
            replacements.extend(prepared.replacements);
            synthesized.extend(prepared.synthesized);
            added.extend(prepared.added);
            removed.extend(prepared.removed);
            clauses_changed.extend(prepared.clauses_changed);
            #[cfg(test)]
            edited_texts.push(prepared.module_text);
        }
        let mut modules = Vec::with_capacity(source_modules.len());
        for (position, text) in source_modules {
            let prepared = self.prepare_source_module(text)?;
            texts[position] = Some(prepared.texts);
            modules.push((position, prepared.root));
        }
        // Before the commit, and before a check answers, so that a new id
        // or version two modules would share is refused as the loader
        // refuses it.
        self.refuse_a_colliding_identity(&replacements, &modules)?;
        Ok(PreparedInstall {
            epoch: self.install_epoch,
            texts: texts
                .into_iter()
                .map(|edited| edited.expect("internal: every entry's module answers its texts"))
                .collect(),
            replacements,
            synthesized,
            added,
            removed,
            modules,
            clauses_changed,
            #[cfg(test)]
            module_texts: edited_texts,
        })
    }

    /// The class objects whose superclass clauses an install of `prepared`
    /// evaluates before its commit, in the order they were made
    /// (`Heap::class_creation_stamp`): every class
    /// object of a nested declaration whose clauses it changes -- no
    /// metaclass, and no level a `<:` clause added but a body-less
    /// declaration's class object, the level its last clause adds. A
    /// root's class object evaluates nothing: its superclass follows its
    /// fresh `root_shape`.
    ///
    /// The walk meets every class object the space holds, dead ones too,
    /// so the installer has a full collection served just before the leaf
    /// that answers them, and again before the commit: what remains is
    /// what the program reaches (`MirrorsFactory.ns`, `Installer>>install`).
    pub(crate) fn clause_class_objects(
        &self,
        prepared: &PreparedInstall,
    ) -> Vec<ClassReference<'gc>> {
        if !prepared
            .clauses_changed
            .iter()
            .any(|id| self.heap.enclosing_mixin_id(*id).is_some())
        {
            return Vec::new();
        }
        let mut classes = Vec::new();
        self.heap.each_object(|object| {
            if self.clause_declaration(prepared, object).is_some() {
                classes.push(object);
            }
        });
        // Creation order, which no collection changes, where the space's
        // order is the movers'.
        classes.sort_by_key(|class| self.heap.class_creation_stamp(*class));
        classes
    }

    /// The id of the nested declaration whose clauses `prepared` changes
    /// and of which `class` is a class object, as `clause_class_objects`
    /// collects them; `None` for any other object.
    pub(crate) fn clause_declaration(
        &self,
        prepared: &PreparedInstall,
        class: ClassReference<'gc>,
    ) -> Option<MixinId> {
        let id = self.declaration_of_class_object(class)?;
        (prepared.clauses_changed.contains(&id) && self.heap.enclosing_mixin_id(id).is_some())
            .then_some(id)
    }

    /// The id of the declaration `class` is the class object of: a
    /// body-less declaration's for the level its last clause adds, its
    /// application's mixin's for any other class object; `None` for a
    /// metaclass, for any other level a `<:` clause added and for an
    /// object that is no class.
    pub(crate) fn declaration_of_class_object(
        &self,
        class: ClassReference<'gc>,
    ) -> Option<MixinId> {
        if self.heap.class_this_class(class).is_some() {
            return None;
        }
        if let Some(declaration) = self.heap.class_body_less_declaration(class) {
            return self.heap.known_mixin_id(declaration);
        }
        if self.heap.class_applied_origin(class).is_some() {
            return None;
        }
        let application = self.heap.class_application(class)?;
        self.heap.known_mixin_id(&application.mixin)
    }

    /// Whether the declaration `id` is `ancestor` or declared in it, at
    /// any depth.
    pub(crate) fn is_declared_within(&self, id: MixinId, ancestor: MixinId) -> bool {
        let mut current = Some(id);
        while let Some(each) = current {
            if each == ancestor {
                return true;
            }
            current = self.heap.enclosing_mixin_id(each);
        }
        false
    }
}

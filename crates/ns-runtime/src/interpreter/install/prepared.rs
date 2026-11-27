//! Preparing an install of several builders' entries as one: the entries
//! refused together before any text is touched, grouped by module, each
//! module's entries spliced into one text in their order and compiled
//! once (`Mutator::prepare_module`); the install epoch that tells a
//! commit whether anything was installed since its preparation; and the
//! one prepared install kept between the leaves that prepare and commit
//! it (`docs/design/liveness-and-reflection.md`, "Planned: the complete
//! install", decision 8).

use std::rc::Rc;

use crate::heap::MixinId;
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
    /// Each edited module's whole text, in the order the modules were
    /// first named.
    #[cfg(test)]
    pub(crate) module_texts: Vec<Rc<str>>,
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
    /// name.
    fn entry_name(&self, entry: &InstallEntry) -> String {
        match entry {
            InstallEntry::Edit(id, _) => self
                .heap
                .mixin(*id)
                .map_or_else(String::new, |mixin| mixin.reflected_name()),
            InstallEntry::Module(_) => String::new(),
        }
    }

    /// Prepares `entries` without the cache. Every entry is checked, and
    /// two of one declaration refused, before any text is touched; then
    /// the entries are grouped by the module they edit, in the order each
    /// module is first named, and each module prepared once.
    pub(crate) fn prepare_entries(
        &mut self,
        entries: &[InstallEntry],
    ) -> Result<PreparedInstall, String> {
        let mut edits: Vec<(MixinId, &Edit)> = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry {
                InstallEntry::Edit(id, edit) => {
                    let (_, name) = self.editable_declaration(*id)?;
                    if edits.iter().any(|(other, _)| other == id) {
                        return Err(format!("two builders edit {name}"));
                    }
                    edits.push((*id, edit));
                }
                InstallEntry::Module(_) => {
                    return Err("a module declared from source is not installed yet".to_string());
                }
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
        let mut texts: Vec<Option<EditedTexts>> = vec![None; edits.len()];
        let mut replacements = Vec::new();
        let mut synthesized = Vec::new();
        let mut added = Vec::new();
        let mut removed = Vec::new();
        #[cfg(test)]
        let mut module_texts = Vec::new();
        for (module_root, positions) in modules {
            let module_entries: Vec<(MixinId, &Edit)> =
                positions.iter().map(|position| edits[*position]).collect();
            let prepared = self.prepare_module(module_root, &module_entries)?;
            for (position, edited) in positions.into_iter().zip(prepared.texts) {
                texts[position] = Some(edited);
            }
            replacements.extend(prepared.replacements);
            synthesized.extend(prepared.synthesized);
            added.extend(prepared.added);
            removed.extend(prepared.removed);
            #[cfg(test)]
            module_texts.push(prepared.module_text);
        }
        // Before the commit, and before a check answers, so that a new id
        // or version two modules would share is refused as the loader
        // refuses it.
        self.refuse_a_colliding_identity(&replacements)?;
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
            #[cfg(test)]
            module_texts,
        })
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

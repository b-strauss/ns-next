//! A module's `:module:` metadata in the editor (`docs/design/modules.md`,
//! decisions 2, 7 and 9): the one above the module's class, which declares
//! its id and version and which `ns-next run` refuses when malformed, and
//! those before a parameter, which document the module it expects. Each
//! malformed one is a finding on the metadata; each well-formed one is
//! what the hover shows over the declaration it belongs to.

use std::path::{Component, Path};

use ns_runtime::modules::{self, ModuleId, VersionRange};
use ns_syntax::{MetadataTarget, Severity, Span, ast};

use crate::analysis::Finding;

/// What a module's `:module:` metadata say.
#[derive(Default)]
pub struct ModuleMetadata {
    pub findings: Vec<Finding>,
    /// Each well-formed `:module:` by the span of the declaration's node it
    /// belongs to -- the root class's declaration or a parameter's name --
    /// and the metadata comment's span.
    pub shown: Vec<(Span, Span)>,
}

/// Checks every `:module:` metadata of `module`, parsed from `text`, the
/// document at `path` if its URI names a file.
pub fn check(module: &ast::Module, text: &str, path: Option<&Path>) -> ModuleMetadata {
    let mut checked = ModuleMetadata::default();
    let mut declarations = Vec::new();
    for (target, metadata) in ns_syntax::metadata_of(module, text) {
        if metadata.tag != modules::MODULE_TAG {
            continue;
        }
        match target {
            MetadataTarget::Module => declarations.push(metadata.span),
            MetadataTarget::Parameter(name) => match check_expectation(&metadata.payload) {
                Ok(()) => checked.shown.push((name, metadata.span)),
                // Nothing refuses an expectation at run time -- a mock must
                // always be passable -- so a malformed one only warns.
                Err(message) => checked.findings.push(Finding {
                    span: metadata.span,
                    message,
                    severity: Severity::Warning,
                }),
            },
            _ => {}
        }
    }
    let Some(&first) = declarations.first() else {
        return checked;
    };
    // `declared_identity` is the loader's own reading, so the editor
    // refuses exactly what `ns-next run` refuses, in its words.
    let refusal = match modules::declared_identity(module, text) {
        // Two declarations are refused at the second, which is the one
        // too many.
        Err(message) => Some((declarations.get(1).copied().unwrap_or(first), message)),
        Ok((id, _)) if !is_the_binary_s_own(&id, text, path) => modules::check_unreserved(&id)
            .err()
            .map(|message| (first, message)),
        Ok(_) => None,
    };
    match refusal {
        Some((span, message)) => checked.findings.push(Finding {
            span,
            message,
            severity: Severity::Error,
        }),
        None => checked.shown.push((module.root_class.span, first)),
    }
    checked
}

/// Whether the document is a copy of the binary's own source that
/// declares `id` -- a shipped module's or the manifest's, which lie in the
/// reserved namespace by right: its text is that source's byte for byte,
/// or it stands at that source's place in the core library's layout
/// (`core_library_root`). The place counts because the copy of
/// `Kernel.ns` being edited is no longer the binary's text, and a store
/// the plugin keeps may hold another binary's; a program's module
/// declaring such an id anywhere else is refused, as the loader refuses it.
fn is_the_binary_s_own(id: &ModuleId, text: &str, path: Option<&Path>) -> bool {
    ns_runtime::shipped_source_files()
        .into_iter()
        .filter(|file| ns_runtime::shipped_module_id(file.name) == id.as_str())
        .any(|file| {
            file.text == text || path.is_some_and(|path| stands_in_a_core_library(path, file.path))
        })
}

/// The name of the directory the core library's layout is rooted at in
/// the repository (`crates/ns-runtime/core-library/`), and of the one the
/// JetBrains plugin keeps the sources `ns-next shipped-sources` wrote in,
/// each binary's in a directory named by its fingerprint
/// (`CoreLibraryStore`).
const CORE_LIBRARY_DIRECTORY: &str = "core-library";

/// The length of a fingerprint `ns-next shipped-sources --fingerprint`
/// answers, in lowercase hexadecimal digits.
const FINGERPRINT_DIGITS: usize = 16;

/// Whether `path` ends in `relative` -- a shipped source's path within the
/// layout, `/` separated -- below a root that is a `core-library`
/// directory, or a fingerprint's directory within one.
fn stands_in_a_core_library(path: &Path, relative: &str) -> bool {
    let mut ancestors = path.components().rev();
    for segment in relative.rsplit('/') {
        match ancestors.next() {
            Some(component) if component.as_os_str() == segment => {}
            _ => return false,
        }
    }
    let is_core_library = |component: Option<Component<'_>>| {
        component.is_some_and(|component| component.as_os_str() == CORE_LIBRARY_DIRECTORY)
    };
    let root = ancestors.next();
    if is_core_library(root) {
        return true;
    }
    let is_fingerprint = root
        .and_then(|component| component.as_os_str().to_str())
        .is_some_and(|name| {
            name.len() == FINGERPRINT_DIGITS
                && name
                    .chars()
                    .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
        });
    is_fingerprint && is_core_library(ancestors.next())
}

/// Refuses a parameter's expectation that is no `<id>` or `<id>@<range>`.
/// No class stands beside it to compare the id's last segment with, so
/// only the segments' grammar is checked; nor is the reserved namespace
/// refused, since a library may expect a shipped module.
fn check_expectation(payload: &str) -> Result<(), String> {
    let (id, range) = match payload.split_once('@') {
        Some((id, range)) => (id, Some(range)),
        None => (payload, None),
    };
    // The id's own last segment as the class's name turns off only the
    // comparison with a class, which an expectation has none to make.
    let class_name = id.rsplit('.').next().unwrap_or(id);
    ModuleId::parse(id, class_name)?;
    if let Some(range) = range {
        VersionRange::parse(range)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    fn messages(text: &str) -> Vec<String> {
        let module = ns_syntax::parse(text).expect("the module parses");
        check(&module, text, None)
            .findings
            .into_iter()
            .map(|finding| finding.message)
            .collect()
    }

    #[test]
    fn two_declarations_are_refused_at_the_second() {
        let text = indoc! {"
            (* :module: com.acme.Json *)
            (* :module: com.acme.Json@1.0.0 *)
            class Json = Object new () ()
        "};
        let module = ns_syntax::parse(text).expect("the module parses");
        let checked = check(&module, text, None);
        assert_eq!(checked.findings.len(), 1);
        let second = text.find("(* :module: com.acme.Json@").expect("the second");
        assert_eq!(checked.findings[0].span.start as usize, second);
        assert!(checked.shown.is_empty());
    }

    /// The findings of `text` as the document at `path`.
    fn messages_at(text: &str, path: &str) -> Vec<String> {
        let module = ns_syntax::parse(text).expect("the module parses");
        check(&module, text, Some(Path::new(path)))
            .findings
            .into_iter()
            .map(|finding| finding.message)
            .collect()
    }

    fn shipped_text(name: &str) -> &'static str {
        ns_runtime::shipped_source_files()
            .into_iter()
            .find(|file| file.name == name)
            .expect("a shipped source")
            .text
    }

    const RESERVED_KERNEL: &str = "`newspeak.core.Kernel` lies in the namespace `newspeak.`, which belongs to the modules ns-next ships";

    /// The binary's own text is its own wherever it stands, the manifest's
    /// included, which no shipped module table names.
    #[test]
    fn the_binary_s_own_text_is_not_refused() {
        for name in ["Kernel", "Manifest", "TimeFactory"] {
            let text = shipped_text(name);
            assert!(messages(text).is_empty(), "{name}: {:?}", messages(text));
            assert!(
                messages_at(text, "/work/elsewhere/Copy.ns").is_empty(),
                "{name}"
            );
        }
    }

    /// The copy being edited stands at its place in the repository's
    /// layout or in a store of the plugin's, by fingerprint.
    #[test]
    fn an_edited_copy_at_its_place_in_the_core_library_is_not_refused() {
        let kernel = format!(
            "/// An edit.
{}",
            shipped_text("Kernel")
        );
        assert!(
            messages_at(
                &kernel,
                "/repository/crates/ns-runtime/core-library/Kernel.ns"
            )
            .is_empty()
        );
        let time = format!(
            "/// An edit.
{}",
            shipped_text("TimeFactory")
        );
        assert!(
            messages_at(
                &time,
                "/repository/crates/ns-runtime/core-library/platform/TimeFactory.ns"
            )
            .is_empty()
        );
        assert!(
            messages_at(
                &time,
                "/system/newspeak/core-library/0123456789abcdef/platform/TimeFactory.ns"
            )
            .is_empty()
        );
        let manifest = format!(
            "/// An edit.
{}",
            shipped_text("Manifest")
        );
        assert!(
            messages_at(
                &manifest,
                "/repository/crates/ns-runtime/core-library/Manifest.ns"
            )
            .is_empty()
        );
    }

    /// A program's module declaring a shipped module's id is refused, as
    /// the loader refuses it: elsewhere, at another source's place, or
    /// below a directory that is no core library or fingerprint.
    #[test]
    fn a_program_module_claiming_a_shipped_id_is_refused() {
        let text = indoc! {"
            (* :module: newspeak.core.Kernel *)
            class Kernel = Object new () ()
        "};
        for path in [
            "/work/Kernel.ns",
            "/repository/crates/ns-runtime/core-library/platform/Kernel.ns",
            "/repository/crates/ns-runtime/core-library/TimeFactory.ns",
            "/work/library/Kernel.ns",
            "/system/newspeak/core-library/0123456789ABCDEF/Kernel.ns",
            "/system/newspeak/other/0123456789abcdef/Kernel.ns",
        ] {
            assert_eq!(messages_at(text, path), vec![RESERVED_KERNEL], "{path}");
        }
        assert_eq!(messages(text), vec![RESERVED_KERNEL]);
    }

    #[test]
    fn a_module_without_metadata_and_a_well_formed_one_have_none() {
        let bare = indoc! {"
            class Json = Object new () ()
        "};
        assert!(messages(bare).is_empty());
        let declared = indoc! {"
            (* :module: com.acme.Json@1.0.0-beta.1 *)
            class Json usingPlatform: (* :module: a.B@1.2 *) platform = Object new () ()
        "};
        assert!(messages(declared).is_empty(), "{:?}", messages(declared));
    }

    #[test]
    fn another_tag_is_not_checked() {
        let text = indoc! {"
            (* :exemplar: anything at all *)
            class Json = Object new () ()
        "};
        assert!(messages(text).is_empty());
    }
}

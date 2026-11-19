//! A module's `:module:` metadata in the editor (`docs/design/modules.md`,
//! decisions 2, 7 and 9): the one above the module's class, which declares
//! its id and version and which `ns-next run` refuses when malformed, and
//! those before a parameter, which document the module it expects. Each
//! malformed one is a finding on the metadata; each well-formed one is
//! what the hover shows over the declaration it belongs to.

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

/// Checks every `:module:` metadata of `module`, parsed from `text`.
pub fn check(module: &ast::Module, text: &str) -> ModuleMetadata {
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
        Ok((id, _)) if !is_the_binary_s_own(&id) => modules::check_unreserved(&id)
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

/// Whether `id` is one the binary's own sources declare -- a shipped
/// module's or the manifest's -- which lie in the reserved namespace by
/// right. The analyzer sees only a document's text, and the copy of
/// `Kernel.ns` being edited is no longer the binary's text byte for byte;
/// what refuses a program's module claiming such an id is the loader.
fn is_the_binary_s_own(id: &ModuleId) -> bool {
    ns_runtime::shipped_source_files()
        .iter()
        .any(|file| ns_runtime::shipped_module_id(file.name) == id.as_str())
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
        check(&module, text)
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
        let checked = check(&module, text);
        assert_eq!(checked.findings.len(), 1);
        let second = text.find("(* :module: com.acme.Json@").expect("the second");
        assert_eq!(checked.findings[0].span.start as usize, second);
        assert!(checked.shown.is_empty());
    }

    #[test]
    fn the_binary_s_own_ids_are_not_refused() {
        let text = indoc! {"
            (* :module: newspeak.core.Manifest *)
            class Manifest = Object new () ()
        "};
        assert!(messages(text).is_empty(), "{:?}", messages(text));
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

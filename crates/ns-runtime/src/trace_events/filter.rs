//! The trace filter (`--trace-filter`; `docs/design/developer-tool.md`,
//! decision 5): patterns in the tools' method syntax, one each,
//! `<id>[@<version>][/<Class>(.<Class>)*][ class]>><selector>`, read by
//! the tools' one grammar (`tool_names`), a trailing `*` making a prefix.
//! A pattern is matched against a body's name by parts -- its module's id
//! and version, the path inside the module, the side and the selector --
//! never as one string, so that a pattern without a version covers every
//! version of its id, as a method breakpoint does (`ModuleName::covers`).

use crate::debugger::{ClassName, ModuleName};
use crate::modules;
use crate::tool_names::{self, ClassRefusal, KnownModule, MethodName};

/// The prefix of a block's name, before the name of the body it is
/// created in (`debugger/stack.rs`'s `frame_name`).
const BLOCK_PREFIX: &str = "[] in ";

/// The suffix of an initializer's name after its class.
const INITIALIZER_SUFFIX: &str = " initializer";

/// The suffix of a superclass clause's name after its class.
const SUPERCLASS_CLAUSE_SUFFIX: &str = " superclass clause";

/// Which bodies' events a trace delivers: every body's, or those whose
/// name one of the patterns matches.
pub struct TraceFilter {
    /// Each pattern; empty for every body.
    patterns: Vec<Pattern>,
}

/// A pattern as written and what it selects.
struct Pattern {
    /// The pattern's text, which a refusal names.
    text: String,
    selection: Selection,
}

/// What a pattern selects.
enum Selection {
    /// One method: `Demo/Account>>deposit:`.
    Method(MethodName),
    /// The methods of one class and side whose selectors begin with
    /// `prefix`: `Demo/Account>>dep*`, `Demo/Account class>>*`.
    Selectors {
        class: ClassName,
        class_side: bool,
        prefix: String,
    },
    /// Every body of the classes inside a module whose path begins with
    /// `path` and then a class whose name begins with `last`:
    /// `Demo/Account.En*`, `Demo/*`.
    Classes {
        module: ModuleName,
        path: Vec<String>,
        last: String,
    },
    /// Every body of the modules of the id whose versions begin with
    /// `prefix`: `com.acme.util.Util@1.*`.
    Versions { id: String, prefix: String },
    /// Every body of the modules whose ids begin with `prefix`: `Demo*`,
    /// `com.acme.*`.
    Ids { prefix: String },
}

/// The refusal of an empty pattern.
const EMPTY_PATTERN: &str = "an empty pattern selects nothing: a trace filter pattern names a \
     method, a class or a module";

/// A body's name read into its parts.
struct BodyParts {
    class: ClassName,
    class_side: bool,
    /// `None` for an initializer and a superclass clause.
    selector: Option<String>,
}

impl TraceFilter {
    /// The filter that delivers every body's events.
    pub fn everything() -> TraceFilter {
        TraceFilter {
            patterns: Vec::new(),
        }
    }

    /// The filter of `patterns`, one pattern each -- never split at a
    /// comma, since `,` is a binary selector (`String>>,`) -- that
    /// delivers every body's events with none; a pattern not of the form
    /// is refused, the refusal naming it. Whether a module has the id and
    /// version a pattern names is `check`'s question, which needs the
    /// program's modules.
    pub fn parse(patterns: &[&str]) -> Result<TraceFilter, String> {
        let patterns = patterns
            .iter()
            .map(|pattern| {
                let pattern = pattern.trim();
                if pattern.is_empty() {
                    return Err(EMPTY_PATTERN.to_string());
                }
                let selection = selection(pattern).map_err(|reason| {
                    // The grammar's own refusal names the text where it says
                    // it is not of the form; a version's names only the
                    // version.
                    if reason.contains(&format!("`{pattern}`")) {
                        reason
                    } else {
                        format!("`{pattern}`: {reason}")
                    }
                })?;
                Ok(Pattern {
                    text: pattern.to_string(),
                    selection,
                })
            })
            .collect::<Result<Vec<Pattern>, String>>()?;
        Ok(TraceFilter { patterns })
    }

    /// Refuses a pattern whose module no module of the program or the
    /// binary has, as the debugger refuses a method breakpoint's name
    /// (`tool_names::known_class`): an id or version none has, and the
    /// dotted form a nested class was written in before. A pattern that
    /// gives only a prefix of an id names no one module and is never
    /// refused. The refusal names the pattern.
    pub fn check(&self, module_of: &dyn Fn(&str) -> Option<KnownModule>) -> Result<(), String> {
        for Pattern { text, selection } in &self.patterns {
            let class = match selection {
                Selection::Method(method) => method.class.clone(),
                Selection::Selectors { class, .. } => class.clone(),
                Selection::Classes { module, path, .. } => ClassName {
                    module: module.clone(),
                    path: path.clone(),
                },
                Selection::Versions { id, .. } => ClassName {
                    module: ModuleName::named(id.clone()),
                    path: Vec::new(),
                },
                Selection::Ids { .. } => continue,
            };
            tool_names::known_class(class, module_of)
                .map_err(|reason| format!("`{text}`: {reason}"))?;
        }
        Ok(())
    }

    /// Whether the events of the body named `body` pass: every body's
    /// with no pattern, and otherwise a body one pattern matches, a block
    /// through the body it is created in. A body named `(unnamed)` -- one
    /// no mixin holds -- passes no pattern.
    pub fn matches(&self, body: &str) -> bool {
        if self.patterns.is_empty() {
            return true;
        }
        let Some(parts) = body_parts(body) else {
            return false;
        };
        self.patterns
            .iter()
            .any(|pattern| pattern.selection.matches(&parts))
    }
}

impl Selection {
    fn matches(&self, body: &BodyParts) -> bool {
        let module = &body.class.module;
        match self {
            Selection::Method(method) => {
                method.class.module.covers(module)
                    && method.class.path == body.class.path
                    && method.class_side == body.class_side
                    && body.selector.as_deref() == Some(method.selector.as_str())
            }
            Selection::Selectors {
                class,
                class_side,
                prefix,
            } => {
                class.module.covers(module)
                    && class.path == body.class.path
                    && *class_side == body.class_side
                    && body
                        .selector
                        .as_deref()
                        .is_some_and(|selector| selector.starts_with(prefix.as_str()))
            }
            Selection::Classes {
                module: named,
                path,
                last,
            } => {
                named.covers(module)
                    && body.class.path.len() > path.len()
                    && body.class.path[..path.len()] == path[..]
                    && body.class.path[path.len()].starts_with(last.as_str())
            }
            Selection::Versions { id, prefix } => {
                &module.id == id
                    && module
                        .version
                        .as_deref()
                        .is_some_and(|version| version.starts_with(prefix.as_str()))
            }
            Selection::Ids { prefix } => module.id.starts_with(prefix.as_str()),
        }
    }
}

/// What the pattern `text` selects, or why it is not of the form.
fn selection(text: &str) -> Result<Selection, String> {
    let Some(stem) = text.strip_suffix('*') else {
        if text.contains('*') {
            return Err(misplaced_star(text));
        }
        return tool_names::method_form(text).map(Selection::Method);
    };
    if stem.contains('*') {
        return Err(misplaced_star(text));
    }
    if let Some((class, prefix)) = stem.split_once(">>") {
        if prefix.contains(char::is_whitespace) {
            return Err(not_a_pattern(text));
        }
        let (class, class_side) =
            tool_names::side_form(class).map_err(|refusal| match refusal {
                ClassRefusal::Malformed => not_a_pattern(text),
                ClassRefusal::Explained(reason) => reason,
            })?;
        return Ok(Selection::Selectors {
            class,
            class_side,
            prefix: prefix.to_string(),
        });
    }
    // A class written whole before ` class` names one class's class side.
    if let Some(class) = stem.trim_end().strip_suffix(" class") {
        let (class, class_side) =
            tool_names::side_form(&format!("{class} class")).map_err(|refusal| match refusal {
                ClassRefusal::Malformed => not_a_pattern(text),
                ClassRefusal::Explained(reason) => reason,
            })?;
        return Ok(Selection::Selectors {
            class,
            class_side,
            prefix: String::new(),
        });
    }
    if stem.is_empty() || stem.contains(char::is_whitespace) {
        return Err(not_a_pattern(text));
    }
    if let Some((module, path)) = stem.split_once('/') {
        let module = tool_names::class_form(module)
            .map_err(|refusal| match refusal {
                ClassRefusal::Malformed => not_a_pattern(text),
                ClassRefusal::Explained(reason) => reason,
            })?
            .module;
        let mut path: Vec<String> = path.split('.').map(str::to_string).collect();
        let last = path.pop().unwrap_or_default();
        if !path.iter().all(|segment| modules::is_identifier(segment))
            || !is_identifier_prefix(&last)
        {
            return Err(not_a_pattern(text));
        }
        return Ok(Selection::Classes { module, path, last });
    }
    if let Some((id, prefix)) = stem.split_once('@') {
        if !id.split('.').all(modules::is_identifier) || !is_version_prefix(prefix) {
            return Err(not_a_pattern(text));
        }
        return Ok(Selection::Versions {
            id: id.to_string(),
            prefix: prefix.to_string(),
        });
    }
    let mut segments: Vec<&str> = stem.split('.').collect();
    let last = segments.pop().unwrap_or_default();
    if !segments.into_iter().all(modules::is_identifier) || !is_identifier_prefix(last) {
        return Err(not_a_pattern(text));
    }
    Ok(Selection::Ids {
        prefix: stem.to_string(),
    })
}

/// Whether `text` begins an identifier: empty, or an identifier.
fn is_identifier_prefix(text: &str) -> bool {
    text.is_empty() || modules::is_identifier(text)
}

/// Whether `text` begins a version: the characters a semver version
/// without build metadata is written in.
fn is_version_prefix(text: &str) -> bool {
    text.chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
}

/// The refusal of a pattern not of the form.
fn not_a_pattern(text: &str) -> String {
    format!(
        "`{text}` is no trace filter pattern: write {} followed by `>>selector`, or by \
         ` class>>selector` for the class side, a trailing `*` making a prefix",
        tool_names::CLASS_FORM
    )
}

/// The refusal of a pattern with a `*` before its end.
fn misplaced_star(text: &str) -> String {
    format!("`{text}` is no trace filter pattern: a `*` stands only at its end")
}

/// The parts of the body named `name`, a block's those of the body it is
/// created in; `None` for a name that names no member of a class, as
/// `(unnamed)` does.
fn body_parts(name: &str) -> Option<BodyParts> {
    let mut name = name;
    while let Some(inside) = name.strip_prefix(BLOCK_PREFIX) {
        name = inside;
    }
    let (class, selector) = if let Some((class, selector)) = name.split_once(">>") {
        (class, Some(selector.to_string()))
    } else if let Some(class) = name.strip_suffix(INITIALIZER_SUFFIX) {
        (class, None)
    } else {
        (name.strip_suffix(SUPERCLASS_CLAUSE_SUFFIX)?, None)
    };
    let (class, class_side) = tool_names::side_form(class).ok()?;
    Some(BodyParts {
        class,
        class_side,
        selector,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(text: &str) -> TraceFilter {
        match TraceFilter::parse(&[text]) {
            Ok(filter) => filter,
            Err(refusal) => panic!("{text}: {refusal}"),
        }
    }

    #[test]
    fn a_pattern_names_one_method() {
        let deposit = filter("Demo/Account>>deposit:");
        assert!(deposit.matches("Demo/Account>>deposit:"));
        assert!(!deposit.matches("Demo/Account>>withdraw:"));
        assert!(!deposit.matches("Demo/Account class>>deposit:"));
        assert!(!deposit.matches("Demo/Account.Entry>>deposit:"));
        assert!(!deposit.matches("Demo>>deposit:"));
    }

    #[test]
    fn a_star_makes_a_prefix() {
        let every_method = filter("Demo/Account>>*");
        assert!(every_method.matches("Demo/Account>>deposit:"));
        assert!(every_method.matches("Demo/Account>>withdraw:"));
        assert!(!every_method.matches("Demo/Account class>>named:"));
        let module = filter("Demo*");
        assert!(module.matches("Demo>>main:args:"));
        assert!(module.matches("Demo/Account class>>named:"));
        assert!(module.matches("Demo/Account initializer"));
        assert!(!module.matches("Other>>main:args:"));
        let selectors = filter("Demo/Account>>dep*");
        assert!(selectors.matches("Demo/Account>>deposit:"));
        assert!(!selectors.matches("Demo/Account>>withdraw:"));
        let classes = filter("Demo/Acc*");
        assert!(classes.matches("Demo/Account>>deposit:"));
        assert!(classes.matches("Demo/Account superclass clause"));
        assert!(!classes.matches("Demo>>main:args:"));
        let class_side = filter("Demo/Account class*");
        assert!(class_side.matches("Demo/Account class>>named:"));
        assert!(!class_side.matches("Demo/Account>>deposit:"));
    }

    #[test]
    fn a_pattern_without_a_version_covers_every_version() {
        let every_version = filter("com.acme.util.Util/Inner>>run");
        assert!(every_version.matches("com.acme.util.Util@1.0.0/Inner>>run"));
        assert!(every_version.matches("com.acme.util.Util@2.0.0/Inner>>run"));
        assert!(every_version.matches("com.acme.util.Util/Inner>>run"));
        let one_version = filter("com.acme.util.Util@1.0.0/Inner>>run");
        assert!(one_version.matches("com.acme.util.Util@1.0.0/Inner>>run"));
        assert!(!one_version.matches("com.acme.util.Util@2.0.0/Inner>>run"));
        assert!(!one_version.matches("com.acme.util.Util/Inner>>run"));
        let versions = filter("com.acme.util.Util@1.*");
        assert!(versions.matches("com.acme.util.Util@1.0.0/Inner>>run"));
        assert!(!versions.matches("com.acme.util.Util@2.0.0/Inner>>run"));
    }

    #[test]
    fn the_dotted_form_is_refused() {
        let module_of = |text: &str| {
            (text == "Demo").then(|| KnownModule {
                id: "Demo".to_string(),
                versions: Vec::new(),
            })
        };
        let dotted = filter("Demo.Account>>deposit:");
        assert_eq!(
            dotted.check(&module_of),
            Err(
                "`Demo.Account>>deposit:`: no module `Demo.Account`; a class inside a module \
                 is written `Demo/Account`"
                    .to_string()
            )
        );
        assert_eq!(filter("Demo/Account>>deposit:").check(&module_of), Ok(()));
        assert_eq!(
            filter("Other/Account>>*").check(&module_of),
            Err("`Other/Account>>*`: no module `Other`".to_string())
        );
        assert_eq!(filter("Oth*").check(&module_of), Ok(()), "a prefix of ids");
    }

    #[test]
    fn a_block_matches_through_its_owner() {
        let main = filter("Demo>>main:args:");
        assert!(main.matches("[] in Demo>>main:args:"));
        assert!(!main.matches("[] in Demo>>run"));
        let initializer = filter("Demo/Account*");
        assert!(initializer.matches("[] in Demo/Account initializer"));
    }

    #[test]
    fn several_patterns_are_alternatives() {
        let both = TraceFilter::parse(&["Demo/Account>>deposit:", " Demo>>main:args:"])
            .expect("two patterns");
        assert!(both.matches("Demo/Account>>deposit:"));
        assert!(both.matches("Demo>>main:args:"));
        assert!(!both.matches("Demo/Account>>withdraw:"));
        assert!(TraceFilter::everything().matches("Demo/Account>>withdraw:"));
        assert!(TraceFilter::everything().matches("(unnamed)"));
        assert!(!both.matches("(unnamed)"));
        assert!(
            TraceFilter::parse(&[])
                .expect("no pattern")
                .matches("Demo>>run")
        );
    }

    /// A pattern is never split at a comma, which is a binary selector.
    #[test]
    fn a_comma_is_a_selector() {
        let comma = filter("Demo>>,");
        assert!(comma.matches("Demo>>,"));
        assert!(comma.matches("[] in Demo>>,"));
        assert!(!comma.matches("Demo>>run"));
        let prefix = filter("newspeak.core.Kernel/String>>,*");
        assert!(prefix.matches("newspeak.core.Kernel/String>>,"));
    }

    #[test]
    fn a_malformed_pattern_is_refused() {
        for text in ["", " "] {
            assert_eq!(
                TraceFilter::parse(&[text]).err().as_deref(),
                Some(EMPTY_PATTERN),
                "{text}"
            );
        }
        for text in [
            ">>",
            "Demo>>",
            "*>>x",
            "*",
            "Demo/Account>>de*p",
            "Demo/Acc ount*",
            "Demo..Account>>x",
        ] {
            match TraceFilter::parse(&[text]) {
                Ok(_) => panic!("`{text}` was taken"),
                Err(refusal) => assert!(refusal.contains(&format!("`{text}`")), "{refusal}"),
            }
        }
        let version = TraceFilter::parse(&["Demo@1.0>>x"]).err().expect("refused");
        assert!(version.starts_with("`Demo@1.0>>x`: "), "{version}");
    }
}

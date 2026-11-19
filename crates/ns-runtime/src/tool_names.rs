//! The names every tool writes a module's class and method in
//! (`docs/design/modules.md`, decision 8): a class as
//! `<id>[@<version>][/<Class>(.<Class>)*]`, a method as that class
//! followed by `>><selector>`, or by ` class>><selector>` for the class
//! side. One grammar, so that the DAP agent, the MCP server and the trace
//! filter read a name alike and refuse a bad one in the same words.
//!
//! The grammar is read in two parts. The form (`class_form`,
//! `method_form`) needs nothing but the text; whether a module of the
//! program or the binary has the id and version a name gives
//! (`known_class`) needs the program's modules, which only the caller
//! knows -- the agent its sources', the trace filter the heap's roots.

use crate::debugger::{ClassName, ModuleName};
use crate::modules;

/// How a class is written: its module by its id and, for one version
/// only, `@` its version, then `/` and the path to a class inside it
/// (`docs/design/modules.md`, decision 8).
pub const CLASS_FORM: &str = "`<id>[@<version>][/<Class>(.<Class>)*]`";

/// A module of the program's or the binary's that a class name names:
/// its id, and the versions the program has of it -- none for an id
/// whose module has no version, every shipped module's among them.
pub struct KnownModule {
    pub id: String,
    pub versions: Vec<String>,
}

/// Why a class name is refused.
pub enum ClassRefusal {
    /// The text is not of the form at all.
    Malformed,
    /// The text is of the form, and the reason says what is wrong.
    Explained(String),
}

/// A method as a tool names it: its class, its side and its selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodName {
    pub class: ClassName,
    /// Whether the method is the class side's (` class>>`).
    pub class_side: bool,
    pub selector: String,
}

/// The class declaration `text` is of the form of,
/// `<id>[@<version>][/<Class>(.<Class>)*]`: a module's class when no `/`
/// follows the module, a class inside it otherwise. A name without a
/// version names the class in every version of the id. Whether any
/// module has the id and version is `known_class`'s question.
pub fn class_form(text: &str) -> Result<ClassName, ClassRefusal> {
    let text = text.trim();
    let (module, path) = match text.split_once('/') {
        Some((module, path)) => (module, Some(path)),
        None => (text, None),
    };
    let (id, version) = match module.split_once('@') {
        Some((id, version)) => (id, Some(version)),
        None => (module, None),
    };
    let path: Vec<String> = match path {
        Some(path) => path.split('.').map(str::to_string).collect(),
        None => Vec::new(),
    };
    if !id.split('.').all(modules::is_identifier)
        || !path.iter().all(|segment| modules::is_identifier(segment))
    {
        return Err(ClassRefusal::Malformed);
    }
    let version = match version {
        Some(version) => Some(
            modules::Version::parse(version)
                .map_err(ClassRefusal::Explained)?
                .to_string(),
        ),
        None => None,
    };
    Ok(ClassName {
        module: ModuleName {
            id: id.to_string(),
            version,
        },
        path,
    })
}

/// `class` if a module of the program or the binary has its id and
/// version. `module_of` answers the module the program or the binary has
/// that a text names: an id no module has is refused, since it would
/// wait, unverified, for a module that never comes, and so is a version
/// no module of the id has; a dotted one a prefix of which names a module
/// with the form that names the class inside that one -- it is the form a
/// nested class was written in before, `Demo.Account` for `Demo/Account`
/// -- and one that is the class's name of one module with that module's
/// id, `newspeak.core.Kernel/Array` for `Kernel/Array`.
pub fn known_class(
    class: ClassName,
    module_of: &dyn Fn(&str) -> Option<KnownModule>,
) -> Result<ClassName, String> {
    let ClassName { module, path } = &class;
    let ModuleName { id, version } = module;
    let named = module_of(id);
    if let Some(known) = named.as_ref().filter(|known| &known.id == id) {
        // A version the program has no module of names a module that
        // never comes, as an unknown id does.
        if let Some(version) = version
            && !known.versions.contains(version)
        {
            let versions = if known.versions.is_empty() {
                format!("`{id}` has no version")
            } else {
                format!("its versions are {}", known.versions.join(", "))
            };
            return Err(format!("no module `{id}@{version}`; {versions}"));
        }
        return Ok(class);
    }
    let written = |module: &str| match version {
        Some(version) => format!("{module}@{version}"),
        None => module.to_string(),
    };
    if id.contains('.') {
        let segments: Vec<&str> = id.split('.').collect();
        let prefix = (1..segments.len())
            .rev()
            .find_map(|length| Some((length, module_of(&segments[..length].join("."))?.id)));
        if let Some((length, prefix)) = prefix {
            let inside = segments[length..]
                .iter()
                .copied()
                .chain(path.iter().map(String::as_str))
                .collect::<Vec<&str>>()
                .join(".");
            let module = written(&prefix);
            return Err(format!(
                "no module `{id}`; a class inside a module is written `{module}/{inside}`"
            ));
        }
    }
    // Every module of the program and of the binary is known before the
    // program starts, so an id none of them has names a module that never
    // comes: the breakpoint would wait unverified for good. Where the id is
    // the class's name of one module -- `Kernel` for the binary's
    // `newspeak.core.Kernel` -- the refusal says how that one is written.
    Err(match named.map(|known| known.id) {
        Some(module) if path.is_empty() => {
            format!(
                "no module `{id}`; the module is written `{}`",
                written(&module)
            )
        }
        Some(module) => format!(
            "no module `{id}`; a class inside a module is written `{}/{}`",
            written(&module),
            path.join(".")
        ),
        None => format!("no module `{id}`"),
    })
}

/// The class declaration `text` names: of the form (`class_form`) and of
/// a module the program or the binary has (`known_class`).
pub fn parse_class(
    text: &str,
    module_of: &dyn Fn(&str) -> Option<KnownModule>,
) -> Result<ClassName, ClassRefusal> {
    known_class(class_form(text)?, module_of).map_err(ClassRefusal::Explained)
}

/// The class declaration named `<id>[@<version>][/<Class>(.<Class>)*]`, as
/// a function breakpoint, an exception filter's condition and the memory
/// view name one (`parse_class`).
pub fn parse_class_name(
    text: &str,
    module_of: &dyn Fn(&str) -> Option<KnownModule>,
) -> Result<ClassName, String> {
    parse_class(text, module_of).map_err(|refusal| match refusal {
        ClassRefusal::Malformed => format!("`{text}` names no class: write {CLASS_FORM}"),
        ClassRefusal::Explained(reason) => reason,
    })
}

/// Class names separated by commas, as an exception filter's condition
/// lists them; an empty entry is passed over.
pub fn parse_class_names(
    text: &str,
    module_of: &dyn Fn(&str) -> Option<KnownModule>,
) -> Result<Vec<ClassName>, String> {
    text.split(',')
        .filter(|name| !name.trim().is_empty())
        .map(|name| parse_class_name(name, module_of))
        .collect()
}

/// The refusal of a text that is not of a method's form.
fn not_a_method(text: &str) -> String {
    format!(
        "`{text}` names no method: write {CLASS_FORM} followed by `>>selector`, or by \
         ` class>>selector` for the class side"
    )
}

/// The method `text` is of the form of, `<class>>><selector>`, or
/// `<class> class>><selector>` for the class side's, the class written as
/// `class_form` reads it -- `com.acme.json.Json@2.1.0/Parser>>parse:`.
/// Whether any module has the class's id and version is `known_class`'s
/// question.
pub fn method_form(text: &str) -> Result<MethodName, String> {
    let (class, selector) = text.split_once(">>").ok_or_else(|| not_a_method(text))?;
    let selector = selector.trim();
    if selector.is_empty() || selector.contains(char::is_whitespace) {
        return Err(not_a_method(text));
    }
    let (class, class_side) = side_form(class).map_err(|refusal| match refusal {
        ClassRefusal::Malformed => not_a_method(text),
        ClassRefusal::Explained(reason) => reason,
    })?;
    Ok(MethodName {
        class,
        class_side,
        selector: selector.to_string(),
    })
}

/// The class and side `text` is of the form of, as a method's name writes
/// them before its `>>`: the class as `class_form` reads it, followed by
/// ` class` for the class side.
pub fn side_form(text: &str) -> Result<(ClassName, bool), ClassRefusal> {
    let class = text.trim();
    let (class, class_side) = match class.strip_suffix("class") {
        Some(rest) if rest.ends_with(char::is_whitespace) => (rest.trim_end(), true),
        _ => (class, false),
    };
    Ok((class_form(class)?, class_side))
}

/// The method `text` names: of the form (`method_form`) and of a class of
/// a module the program or the binary has (`known_class`).
pub fn parse_method_name(
    text: &str,
    module_of: &dyn Fn(&str) -> Option<KnownModule>,
) -> Result<MethodName, String> {
    let MethodName {
        class,
        class_side,
        selector,
    } = method_form(text)?;
    Ok(MethodName {
        class: known_class(class, module_of)?,
        class_side,
        selector,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ids of the modules the tests' program and binary have.
    const MODULES: [&str; 8] = [
        "Module",
        "Calls",
        "Demo",
        "Program",
        "Kernel",
        "com.acme.json.Json",
        "newspeak.core.Kernel",
        "newspeak.core.PlatformFactory",
    ];

    /// The versions the tests' program has of `com.acme.json.Json`; its
    /// other modules have none.
    const JSON_VERSIONS: [&str; 2] = ["2.1.0", "2.1.0-beta.1"];

    /// The module of `MODULES` `text` names, as the DAP agent's sources
    /// answer it.
    fn module_of(text: &str) -> Option<KnownModule> {
        let known = |id: &str| KnownModule {
            id: id.to_string(),
            versions: if id == "com.acme.json.Json" {
                JSON_VERSIONS.map(str::to_string).to_vec()
            } else {
                Vec::new()
            },
        };
        if MODULES.contains(&text) {
            return Some(known(text));
        }
        let mut by_class = MODULES
            .iter()
            .filter(|id| !text.contains('.') && id.rsplit('.').next() == Some(text));
        match (by_class.next(), by_class.next()) {
            (Some(only), None) => Some(known(only)),
            _ => None,
        }
    }

    /// The method a name names.
    fn method(text: &str) -> MethodName {
        parse_method_name(text, &module_of).expect("a method")
    }

    #[test]
    fn method_names_parse_with_and_without_class_side() {
        let instance = method("Module/Outer.Inner>>at:put:");
        assert_eq!(
            instance,
            MethodName {
                class: ClassName {
                    module: ModuleName::named("Module"),
                    path: vec!["Outer".to_string(), "Inner".to_string()],
                },
                class_side: false,
                selector: "at:put:".to_string(),
            }
        );
        let class_side = method(" Module/Inner class >> new ");
        assert!(class_side.class_side);
        assert_eq!(class_side.class.path, ["Inner"]);
        assert_eq!(class_side.selector, "new");
        let named_class = method("Module/Subclass>>run");
        assert!(!named_class.class_side, "`Subclass` is a name, not a side");
    }

    #[test]
    fn method_names_parse_with_id_version_and_inner_path() {
        let parsed = method("com.acme.json.Json@2.1.0/Parser.Token class>>at:put:");
        assert_eq!(
            parsed.class.module,
            ModuleName {
                id: "com.acme.json.Json".to_string(),
                version: Some("2.1.0".to_string()),
            }
        );
        assert_eq!(parsed.class.path, ["Parser", "Token"]);
        assert!(parsed.class_side);
        assert_eq!(parsed.selector, "at:put:");
        let every_version = method("com.acme.json.Json/Parser>>parse:");
        assert_eq!(
            every_version.class.module,
            ModuleName::named("com.acme.json.Json")
        );
        let pre_release = method("com.acme.json.Json@2.1.0-beta.1>>parse:");
        assert_eq!(
            pre_release.class.module.version.as_deref(),
            Some("2.1.0-beta.1")
        );
    }

    #[test]
    fn a_root_method_has_no_slash() {
        let root = method("Calls>>>>");
        assert_eq!(root.class.module, ModuleName::named("Calls"));
        assert!(root.class.path.is_empty());
        assert_eq!(root.selector, ">>");
        let dotted = method("com.acme.json.Json>>parse:");
        assert_eq!(dotted.class.module, ModuleName::named("com.acme.json.Json"));
        assert!(dotted.class.path.is_empty());
        let class_side = method("Calls class>>new");
        assert!(class_side.class_side);
        assert!(class_side.class.path.is_empty());
    }

    /// The form alone needs no module: a dotted id and an id no module
    /// has are of the form, and only `known_class` refuses them.
    #[test]
    fn the_form_needs_no_module() {
        let dotted = method_form("Demo.Account>>deposit:").expect("of the form");
        assert_eq!(dotted.class.module, ModuleName::named("Demo.Account"));
        assert!(dotted.class.path.is_empty());
        let unknown = method_form("org.example.Tool@1.0.0/Inner class>>run").expect("of the form");
        assert_eq!(unknown.class.module.version.as_deref(), Some("1.0.0"));
        assert!(unknown.class_side);
        assert!(method_form("Calls>>").is_err());
        assert_eq!(
            method_form("Calls@1.0>>run"),
            Err("`1.0` is no version: it needs MAJOR.MINOR.PATCH".to_string())
        );
    }

    /// A name in the old form, its nested class after a dot, names no
    /// module of the program but its prefix does: it is refused with the
    /// form that names the class, never read as a module of its own.
    #[test]
    fn a_dotted_nested_class_is_refused_with_the_new_form() {
        assert_eq!(
            parse_method_name("Demo.Account>>deposit:", &module_of),
            Err(
                "no module `Demo.Account`; a class inside a module is written `Demo/Account`"
                    .to_string()
            )
        );
        assert_eq!(
            parse_class_name("Demo.Account.Entry", &module_of),
            Err(
                "no module `Demo.Account.Entry`; a class inside a module is written \
                 `Demo/Account.Entry`"
                    .to_string()
            )
        );
        assert_eq!(
            parse_method_name("com.acme.json.Json.Parser>>parse:", &module_of),
            Err(
                "no module `com.acme.json.Json.Parser`; a class inside a module is written \
                 `com.acme.json.Json/Parser`"
                    .to_string()
            )
        );
        // A prefix that is the class's name of one module names that
        // module: the shipped modules were written by their class's name.
        assert_eq!(
            parse_class_name("PlatformFactory.Inner", &module_of),
            Err(
                "no module `PlatformFactory.Inner`; a class inside a module is written \
                 `newspeak.core.PlatformFactory/Inner`"
                    .to_string()
            )
        );
    }

    /// Every module is known before the program starts, so an id no
    /// module has is refused rather than left waiting for good; one that
    /// is the class's name of one module says how that module is written.
    #[test]
    fn an_id_no_module_has_is_refused() {
        assert_eq!(
            parse_method_name("org.example.Tool>>run", &module_of),
            Err("no module `org.example.Tool`".to_string())
        );
        assert_eq!(
            parse_class_name("Elsewhere/Inner", &module_of),
            Err("no module `Elsewhere`".to_string())
        );
        // Without a program module `Kernel`, `Kernel` is the class's name
        // of the binary's `newspeak.core.Kernel` alone.
        let without_kernel = |text: &str| match text {
            "Kernel" | "newspeak.core.Kernel" => Some(KnownModule {
                id: "newspeak.core.Kernel".to_string(),
                versions: Vec::new(),
            }),
            _ => None,
        };
        assert_eq!(
            parse_method_name("Kernel/Array>>do:", &without_kernel),
            Err("no module `Kernel`; a class inside a module is written \
                 `newspeak.core.Kernel/Array`"
                .to_string())
        );
        assert_eq!(
            parse_method_name("Kernel class>>new", &without_kernel),
            Err("no module `Kernel`; the module is written `newspeak.core.Kernel`".to_string())
        );
        assert_eq!(
            parse_method_name("newspeak.core.Kernel/Array>>do:", &without_kernel)
                .map(|method| method.class.module),
            Ok(ModuleName::named("newspeak.core.Kernel"))
        );
    }

    /// The program's own `Kernel` and the binary's are written apart, by
    /// their ids.
    #[test]
    fn a_program_kernel_and_the_shipped_kernel_are_named_apart() {
        let own = method("Kernel/Array>>do:");
        let shipped = method("newspeak.core.Kernel/Array>>do:");
        assert_eq!(own.class.module, ModuleName::named("Kernel"));
        assert_eq!(
            shipped.class.module,
            ModuleName::named("newspeak.core.Kernel")
        );
        assert_eq!(own.class.path, shipped.class.path);
    }

    #[test]
    fn a_malformed_method_name_is_refused() {
        for text in [
            "Calls double:",
            "Calls>>",
            ">>double:",
            "Calls/Inner..Deeper>>run",
            "Calls/>>run",
            "Calls//Inner>>run",
            "Calls..Inner>>run",
            "Calls@>>run",
            "@1.0.0>>run",
            "Calls>>at: put:",
        ] {
            assert!(parse_method_name(text, &module_of).is_err(), "{text}");
        }
        assert_eq!(
            parse_method_name("Calls@1.0>>run", &module_of),
            Err("`1.0` is no version: it needs MAJOR.MINOR.PATCH".to_string())
        );
    }

    /// A version no module of the id has names a module that never comes,
    /// as an unknown id does: refused, naming the versions there are,
    /// rather than left to wait unverified for good.
    #[test]
    fn a_version_no_module_of_the_id_has_is_refused() {
        assert_eq!(
            parse_method_name("com.acme.json.Json@9.9.9>>parse:", &module_of),
            Err(
                "no module `com.acme.json.Json@9.9.9`; its versions are 2.1.0, 2.1.0-beta.1"
                    .to_string()
            )
        );
        assert_eq!(
            parse_class_name("Calls@1.0.0/Inner", &module_of).map(|_| ()),
            Err("no module `Calls@1.0.0`; `Calls` has no version".to_string())
        );
        assert_eq!(
            parse_class_name("newspeak.core.Kernel@1.0.0/Array", &module_of).map(|_| ()),
            Err(
                "no module `newspeak.core.Kernel@1.0.0`; `newspeak.core.Kernel` has no version"
                    .to_string()
            )
        );
        assert!(parse_method_name("com.acme.json.Json@2.1.0>>parse:", &module_of).is_ok());
    }

    #[test]
    fn class_names_parse_from_a_list() {
        let names = parse_class_names(
            "newspeak.core.Kernel/Error, Program/Outer.Inner,",
            &module_of,
        )
        .expect("names");
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].module, ModuleName::named("newspeak.core.Kernel"));
        assert_eq!(names[0].path, ["Error"]);
        assert_eq!(names[1].module, ModuleName::named("Program"));
        assert_eq!(names[1].path, ["Outer", "Inner"]);
        assert!(parse_class_names("Kernel/Error, Kernel Exception", &module_of).is_err());
        assert_eq!(parse_class_names(" ", &module_of), Ok(Vec::new()));
    }
}

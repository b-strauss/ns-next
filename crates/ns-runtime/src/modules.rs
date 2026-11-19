//! Module identity: ids, semver versions, version ranges and the relative
//! paths that name a module's source (`docs/design/modules.md`, decisions 1,
//! 2, 4 and 9). Pure values, so the image decoder and the tools hold a module
//! to the same rules the loader does, and `declared_identity`, which reads
//! them from a module's `:module:` metadata.

use std::cmp::Ordering;
use std::fmt;

/// The namespace only the modules the binary ships may declare.
const RESERVED_PREFIX: &str = "newspeak.";

/// The prefix of a shipped module's path, followed by its file name.
const CORE_PATH_PREFIX: &str = "core:";

/// The extension every module's source file has.
const SOURCE_EXTENSION: &str = ".ns";

/// Message of an id whose last segment is not its class's name; `{id}` and
/// `{class}` are replaced.
const ID_DOES_NOT_END_IN_CLASS: &str = "`{id}` does not end in the class's name `{class}`";

/// Message of a text that is no version; `{text}` and `{reason}` are replaced.
const NOT_A_VERSION: &str = "`{text}` is no version: {reason}";

fn not_a_version(text: &str, reason: &str) -> String {
    NOT_A_VERSION
        .replace("{text}", text)
        .replace("{reason}", reason)
}

fn is_identifier(segment: &str) -> bool {
    let mut characters = segment.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {
            characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
        }
        _ => false,
    }
}

/// A module's id: identifiers separated by dots, the last the name of the
/// module's class.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(String);

impl ModuleId {
    /// Parses `text` as an id whose last segment must be `class_name`.
    pub fn parse(text: &str, class_name: &str) -> Result<ModuleId, String> {
        if text.is_empty() || !text.split('.').all(is_identifier) {
            return Err(format!(
                "`{text}` is no module id: it must be identifiers separated by dots"
            ));
        }
        let last = text.rsplit('.').next().unwrap_or(text);
        if last != class_name {
            return Err(ID_DOES_NOT_END_IN_CLASS
                .replace("{id}", text)
                .replace("{class}", class_name));
        }
        Ok(ModuleId(text.to_string()))
    }

    /// The id of a module that declares none: its class's name.
    pub fn of_class(class_name: &str) -> ModuleId {
        ModuleId(class_name.to_string())
    }

    /// Whether the id lies in the namespace reserved for shipped modules.
    pub fn is_reserved(&self) -> bool {
        self.0.starts_with(RESERVED_PREFIX)
    }

    /// The id's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One dot-separated part of a pre-release. The derived order is semver's:
/// a number sorts before text, numbers numerically, text in ASCII order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PreReleasePart {
    /// An all-digit part, without a leading zero.
    Number(u64),
    /// Any other part of digits, letters and hyphens.
    Text(String),
}

/// A semver 2.0.0 version without build metadata, which orders nothing.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre_release: Vec<PreReleasePart>,
}

fn parse_number(text: &str, what: &str) -> Result<u64, String> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{what} `{text}` is not a number"));
    }
    if text.len() > 1 && text.starts_with('0') {
        return Err(format!("{what} `{text}` has a leading zero"));
    }
    text.parse::<u64>()
        .map_err(|_| format!("{what} `{text}` is too large"))
}

fn parse_pre_release(text: &str) -> Result<Vec<PreReleasePart>, String> {
    text.split('.')
        .map(|part| {
            if part.is_empty() {
                return Err("a pre-release part is empty".to_string());
            }
            if !part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err(format!(
                    "pre-release part `{part}` holds a character other than letters, digits and `-`"
                ));
            }
            if part.bytes().all(|byte| byte.is_ascii_digit()) {
                parse_number(part, "pre-release part").map(PreReleasePart::Number)
            } else {
                Ok(PreReleasePart::Text(part.to_string()))
            }
        })
        .collect()
}

impl Version {
    /// Parses `MAJOR.MINOR.PATCH` with an optional `-pre.release`.
    pub fn parse(text: &str) -> Result<Version, String> {
        let refuse = |reason: &str| not_a_version(text, reason);
        if text.contains('+') {
            return Err(refuse("build metadata orders nothing and is refused"));
        }
        let (core, pre_release) = match text.split_once('-') {
            Some((core, pre_release)) => (core, Some(pre_release)),
            None => (text, None),
        };
        let numbers: Vec<&str> = core.split('.').collect();
        let [major, minor, patch] = numbers[..] else {
            return Err(refuse("it needs MAJOR.MINOR.PATCH"));
        };
        let major = parse_number(major, "major").map_err(|reason| refuse(&reason))?;
        let minor = parse_number(minor, "minor").map_err(|reason| refuse(&reason))?;
        let patch = parse_number(patch, "patch").map_err(|reason| refuse(&reason))?;
        let pre_release = match pre_release {
            Some(pre_release) => {
                parse_pre_release(pre_release).map_err(|reason| refuse(&reason))?
            }
            None => Vec::new(),
        };
        Ok(Version {
            major,
            minor,
            patch,
            pre_release,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(
                || match (self.pre_release.is_empty(), other.pre_release.is_empty()) {
                    // A version without a pre-release is above every one with.
                    (true, true) => Ordering::Equal,
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    // Vec's order puts a prefix before its extensions, as semver does.
                    (false, false) => self.pre_release.cmp(&other.pre_release),
                },
            )
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (index, part) in self.pre_release.iter().enumerate() {
            formatter.write_str(if index == 0 { "-" } else { "." })?;
            match part {
                PreReleasePart::Number(number) => write!(formatter, "{number}")?,
                PreReleasePart::Text(text) => formatter.write_str(text)?,
            }
        }
        Ok(())
    }
}

/// A version range: a prefix `MAJOR` or `MAJOR.MINOR`, or a full version,
/// pre-release included, which matches only itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionRange {
    major: u64,
    minor: Option<u64>,
    exact: Option<Version>,
}

impl VersionRange {
    /// Parses a range's text.
    pub fn parse(text: &str) -> Result<VersionRange, String> {
        let refuse = |reason: &str| not_a_version(text, reason);
        if text.contains('.') && text.split('.').count() >= 3 {
            let version = Version::parse(text)?;
            return Ok(VersionRange {
                major: version.major,
                minor: Some(version.minor),
                exact: Some(version),
            });
        }
        if text.contains(['-', '+']) {
            return Err(refuse("a range with a pre-release names all three numbers"));
        }
        let numbers: Vec<&str> = text.split('.').collect();
        match numbers[..] {
            [major] => Ok(VersionRange {
                major: parse_number(major, "major").map_err(|reason| refuse(&reason))?,
                minor: None,
                exact: None,
            }),
            [major, minor] => Ok(VersionRange {
                major: parse_number(major, "major").map_err(|reason| refuse(&reason))?,
                minor: Some(parse_number(minor, "minor").map_err(|reason| refuse(&reason))?),
                exact: None,
            }),
            _ => Err(refuse("it needs MAJOR, MAJOR.MINOR or MAJOR.MINOR.PATCH")),
        }
    }

    /// Whether `version` lies in the range. A prefix range matches no
    /// pre-release, so that `2` never picks a beta silently.
    pub fn matches(&self, version: &Version) -> bool {
        match &self.exact {
            Some(exact) => exact == version,
            None => {
                version.pre_release.is_empty()
                    && version.major == self.major
                    && self.minor.is_none_or(|minor| version.minor == minor)
            }
        }
    }
}

/// What tells one module from another: the path of its source, its id and
/// its version, if it declares one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleIdentity {
    /// Relative to the project with `/` separators, `core:<File>.ns` for a
    /// shipped module and `core:Manifest.ns` for the manifest.
    pub path: String,
    pub id: ModuleId,
    pub version: Option<Version>,
}

impl ModuleIdentity {
    /// The id, `@` the version when there is one.
    pub fn display_name(&self) -> String {
        match &self.version {
            Some(version) => format!("{}@{version}", self.id),
            None => self.id.to_string(),
        }
    }
}

/// The tag of the metadata that declares a module's id and version.
const MODULE_TAG: &str = "module";

/// Message of a module above whose class two `:module:` metadata stand;
/// `{first}` and `{second}` are replaced by their payloads.
const TWO_MODULE_METADATA: &str =
    "two `:module:` metadata declare the module: `{first}` and `{second}`";

/// The id and version `module`, parsed from `source`, declares: the
/// payload of the one `:module:` metadata above its class (§4.3's module
/// target), `<id>` or `<id>@<version>`, the id's last segment its class's
/// name. A module that declares none is its class's name, without a
/// version. Two such metadata are refused, naming both, since neither
/// could be preferred silently; so is a payload that is no id or no
/// version. Whether the id lies in the reserved namespace is the caller's
/// question: a shipped module's must, a program's may not.
pub fn declared_identity(
    module: &ns_syntax::ast::Module,
    source: &str,
) -> Result<(ModuleId, Option<Version>), String> {
    let class_name = module.root_class.name.name.as_str();
    let mut declared = ns_syntax::metadata_of(module, source)
        .into_iter()
        .filter(|(target, metadata)| {
            *target == ns_syntax::MetadataTarget::Module && metadata.tag == MODULE_TAG
        })
        .map(|(_, metadata)| metadata.payload);
    let Some(payload) = declared.next() else {
        return Ok((ModuleId::of_class(class_name), None));
    };
    if let Some(second) = declared.next() {
        return Err(TWO_MODULE_METADATA
            .replace("{first}", &payload)
            .replace("{second}", &second));
    }
    match payload.split_once('@') {
        Some((id, version)) => Ok((
            ModuleId::parse(id, class_name)?,
            Some(Version::parse(version)?),
        )),
        None => Ok((ModuleId::parse(&payload, class_name)?, None)),
    }
}

/// Message of a program's module whose id lies in the reserved namespace;
/// `{id}` is replaced.
const IN_RESERVED_NAMESPACE: &str =
    "`{id}` lies in the namespace `newspeak.`, which belongs to the modules ns-next ships";

/// Message of two modules of one id and one version; `{id}`, `{version}`,
/// `{first}` and `{second}` are replaced, the last two by their paths.
const TWO_OF_ONE_VERSION: &str = "two modules are `{id}@{version}`: {first} and {second}";

/// Message of two modules of one id, one of them without a version; `{id}`,
/// `{first}` and `{second}` are replaced, the last two by their paths.
const TWO_ONE_WITHOUT_VERSION: &str =
    "two modules are `{id}` and one of them has no version: {first} and {second}";

/// Refuses a program's module whose id lies in the namespace reserved for
/// the modules the binary ships (decision 9): a program's own `Kernel` is
/// `Kernel`, and no tool name meets a shipped module's.
pub fn check_unreserved(id: &ModuleId) -> Result<(), String> {
    if id.is_reserved() {
        return Err(IN_RESERVED_NAMESPACE.replace("{id}", id.as_str()));
    }
    Ok(())
}

/// Refuses two modules no request could pick between (decision 5): two of
/// one id and one version, and two of one id where either has no version.
/// Names the first such pair in the order given, the earlier module first,
/// by their paths.
pub fn check_distinct<'identity>(
    identities: impl IntoIterator<Item = &'identity ModuleIdentity>,
) -> Result<(), String> {
    let identities: Vec<&ModuleIdentity> = identities.into_iter().collect();
    for (index, later) in identities.iter().enumerate() {
        for earlier in &identities[..index] {
            if earlier.id != later.id {
                continue;
            }
            let message = match (&earlier.version, &later.version) {
                (Some(first), Some(second)) if first == second => TWO_OF_ONE_VERSION
                    .replace("{id}", later.id.as_str())
                    .replace("{version}", &first.to_string()),
                (Some(_), Some(_)) => continue,
                _ => TWO_ONE_WITHOUT_VERSION.replace("{id}", later.id.as_str()),
            };
            return Err(message
                .replace("{first}", &earlier.path)
                .replace("{second}", &later.path));
        }
    }
    Ok(())
}

/// Checks a module's path: relative, `/`-separated, without root, drive or
/// `\`, without an empty or `.` segment, `..` only leading, ending in `.ns`;
/// or `core:` and a file name.
pub fn check_relative_path(path: &str) -> Result<(), String> {
    if let Some(file) = path.strip_prefix(CORE_PATH_PREFIX) {
        let is_file_name = !file.is_empty()
            && file.ends_with(SOURCE_EXTENSION)
            && !file.contains(['/', '\\', ':']);
        return if is_file_name {
            Ok(())
        } else {
            Err(format!(
                "`{path}` is no shipped module's path: `core:` takes a file name ending in `.ns`"
            ))
        };
    }
    if path.is_empty() {
        return Err("a module's path is empty".to_string());
    }
    if path.contains('\\') {
        return Err(format!("`{path}` has a `\\`; paths are separated by `/`"));
    }
    if path.starts_with('/') || path.contains(':') {
        return Err(format!(
            "`{path}` has a root or a drive; it must be relative"
        ));
    }
    let mut leading = true;
    for segment in path.split('/') {
        match segment {
            "" => return Err(format!("`{path}` has an empty segment")),
            "." => return Err(format!("`{path}` has a `.` segment")),
            ".." if leading => {}
            ".." => {
                return Err(format!(
                    "`{path}` has a `..` after a name; `..` may only lead"
                ));
            }
            _ => leading = false,
        }
    }
    if !path.ends_with(SOURCE_EXTENSION) {
        return Err(format!("`{path}` does not end in `.ns`"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|error| panic!("{error}"))
    }

    fn range(text: &str) -> VersionRange {
        VersionRange::parse(text).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn an_id_ends_in_its_class() {
        let id = ModuleId::parse("com.acme.json.Json", "Json").unwrap();
        assert_eq!(id.as_str(), "com.acme.json.Json");
        assert_eq!(
            ModuleId::parse("com.acme.json.Json", "Parser").unwrap_err(),
            "`com.acme.json.Json` does not end in the class's name `Parser`"
        );
        for refused in [
            "com..Json",
            ".Json",
            "com.acme.1x.Json",
            "com.acme json.Json",
            "",
        ] {
            assert!(ModuleId::parse(refused, "Json").is_err(), "{refused}");
        }
        assert_eq!(ModuleId::of_class("Util").as_str(), "Util");
    }

    #[test]
    fn the_reserved_namespace() {
        assert!(
            ModuleId::parse("newspeak.core.Kernel", "Kernel")
                .unwrap()
                .is_reserved()
        );
        assert!(
            !ModuleId::parse("newspeakx.Json", "Json")
                .unwrap()
                .is_reserved()
        );
        assert!(
            !ModuleId::parse("org.newspeak.Json", "Json")
                .unwrap()
                .is_reserved()
        );
    }

    #[test]
    fn versions_parse_and_order_by_semver() {
        let ascending = [
            "1.0.0",
            "2.0.0-alpha",
            "2.0.0-alpha.1",
            "2.0.0-beta",
            "2.0.0-beta.2",
            "2.0.0-beta.11",
            "2.0.0-rc.1",
            "2.0.0",
            "2.1.0",
        ];
        for pair in ascending.windows(2) {
            assert!(
                version(pair[0]) < version(pair[1]),
                "{} < {}",
                pair[0],
                pair[1]
            );
        }
        for text in ascending {
            assert_eq!(version(text).to_string(), text);
        }
        for (text, reason) in [
            ("1.0", "it needs MAJOR.MINOR.PATCH"),
            ("01.0.0", "has a leading zero"),
            ("1.0.0+build", "build metadata"),
            ("1.0.0-", "a pre-release part is empty"),
            ("v1.0.0", "is not a number"),
        ] {
            let error = Version::parse(text).unwrap_err();
            assert!(
                error.starts_with(&format!("`{text}` is no version: ")),
                "{error}"
            );
            assert!(error.contains(reason), "{error}");
        }
    }

    #[test]
    fn ranges_are_prefixes() {
        assert!(range("2").matches(&version("2.0.0")));
        assert!(range("2").matches(&version("2.9.1")));
        assert!(!range("2").matches(&version("3.0.0")));
        assert!(range("2.1").matches(&version("2.1.7")));
        assert!(!range("2.1").matches(&version("2.2.0")));
        assert!(range("2.1.0").matches(&version("2.1.0")));
        assert!(!range("2.1.0").matches(&version("2.1.1")));
        assert!(VersionRange::parse("2.").is_err());
        assert!(VersionRange::parse("2-beta").is_err());
    }

    #[test]
    fn a_range_without_a_pre_release_matches_none() {
        assert!(!range("2").matches(&version("2.1.0-beta.1")));
        assert!(!range("2.1").matches(&version("2.1.0-beta.1")));
        assert!(!range("2.1.0").matches(&version("2.1.0-beta.1")));
        assert!(range("2.1.0-beta.1").matches(&version("2.1.0-beta.1")));
        assert!(!range("2.1.0-beta.1").matches(&version("2.1.0")));
    }

    #[test]
    fn relative_paths_are_checked() {
        for accepted in [
            "libs/a/Util.ns",
            "../shared/Util.ns",
            "core:Kernel.ns",
            "core:Manifest.ns",
        ] {
            assert_eq!(check_relative_path(accepted), Ok(()), "{accepted}");
        }
        for refused in [
            "/abs/Util.ns",
            "C:/x/Util.ns",
            "a\\Util.ns",
            "a/../Util.ns",
            "a//Util.ns",
            "a/./Util.ns",
            "a/Util.txt",
            "",
            "core:",
            "core:a/Kernel.ns",
        ] {
            assert!(check_relative_path(refused).is_err(), "{refused}");
        }
    }

    fn identity_of(source: &str) -> Result<(ModuleId, Option<Version>), String> {
        let module = ns_syntax::parse(source).unwrap_or_else(|_| panic!("the module parses"));
        declared_identity(&module, source)
    }

    #[test]
    fn an_unannotated_module_is_its_class() {
        let source = indoc! {"
            /// A helper.
            /// Nothing more.
            class Util = (
            ) (
            )
        "};
        assert_eq!(identity_of(source), Ok((ModuleId::of_class("Util"), None)));
    }

    #[test]
    fn a_declared_id_and_version() {
        let source = indoc! {"
            /// Reads and writes JSON.
            /// It knows nothing else.
            (* :module: com.acme.json.Json@2.1.0-beta.1 *)
            class Json usingPlatform: platform = (
            ) (
            )
        "};
        assert_eq!(
            identity_of(source),
            Ok((
                ModuleId::parse("com.acme.json.Json", "Json").unwrap(),
                Some(version("2.1.0-beta.1"))
            ))
        );
        let unversioned = indoc! {"
            (* :module:   com.acme.json.Json   *)
            class Json = (
            ) (
            )
        "};
        assert_eq!(
            identity_of(unversioned),
            Ok((ModuleId::parse("com.acme.json.Json", "Json").unwrap(), None))
        );
        // Another tag above the class, or a `:module:` elsewhere, declares
        // nothing.
        let elsewhere = indoc! {"
            (* :exemplar: Json new *)
            class Json reading: (* :module: com.acme.text.Text@1 *) text = (
            ) (
            )
        "};
        assert_eq!(
            identity_of(elsewhere),
            Ok((ModuleId::of_class("Json"), None))
        );
    }

    #[test]
    fn a_mismatched_id_is_refused() {
        let source = indoc! {"
            (* :module: com.acme.json.Json@2.1.0 *)
            class Parser = (
            ) (
            )
        "};
        assert_eq!(
            identity_of(source),
            Err("`com.acme.json.Json` does not end in the class's name `Parser`".to_string())
        );
        let malformed = indoc! {"
            (* :module: com.acme.json.Json@2.1 *)
            class Json = (
            ) (
            )
        "};
        assert_eq!(
            identity_of(malformed),
            Err("`2.1` is no version: it needs MAJOR.MINOR.PATCH".to_string())
        );
    }

    #[test]
    fn two_module_metadata_are_refused() {
        let source = indoc! {"
            (* :module: com.acme.json.Json@2.1.0 *)
            (* :module: org.other.Json *)
            class Json = (
            ) (
            )
        "};
        assert_eq!(
            identity_of(source),
            Err(
                "two `:module:` metadata declare the module: `com.acme.json.Json@2.1.0` and `org.other.Json`"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_program_s_id_may_not_lie_in_the_reserved_namespace() {
        assert_eq!(
            check_unreserved(&ModuleId::parse("newspeak.json.Json", "Json").unwrap()),
            Err(
                "`newspeak.json.Json` lies in the namespace `newspeak.`, which belongs to the \
                 modules ns-next ships"
                    .to_string()
            )
        );
        assert_eq!(
            check_unreserved(&ModuleId::parse("org.newspeak.Json", "Json").unwrap()),
            Ok(())
        );
    }

    fn identity(path: &str, id: &str, version_text: Option<&str>) -> ModuleIdentity {
        let class_name = id.rsplit('.').next().unwrap_or(id);
        ModuleIdentity {
            path: path.to_string(),
            id: ModuleId::parse(id, class_name).unwrap(),
            version: version_text.map(version),
        }
    }

    #[test]
    fn two_modules_no_request_could_pick_between_are_refused() {
        let first = identity("a/Util.ns", "com.acme.Util", Some("1.0.0"));
        let second = identity("b/Util.ns", "com.acme.Util", Some("2.0.0"));
        let same = identity("c/Util.ns", "com.acme.Util", Some("1.0.0"));
        let unversioned = identity("d/Util.ns", "com.acme.Util", None);
        let other = identity("e/Util.ns", "Util", None);
        assert_eq!(check_distinct([&first, &second, &other]), Ok(()));
        assert_eq!(
            check_distinct([&first, &second, &same]),
            Err("two modules are `com.acme.Util@1.0.0`: a/Util.ns and c/Util.ns".to_string())
        );
        assert_eq!(
            check_distinct([&unversioned, &second]),
            Err(
                "two modules are `com.acme.Util` and one of them has no version: d/Util.ns and \
                 b/Util.ns"
                    .to_string()
            )
        );
        assert_eq!(
            check_distinct([&other, &identity("f/Util.ns", "Util", None)]),
            Err(
                "two modules are `Util` and one of them has no version: e/Util.ns and f/Util.ns"
                    .to_string()
            )
        );
    }

    #[test]
    fn an_identity_displays_its_id_and_version() {
        let identity = ModuleIdentity {
            path: "libs/Json.ns".to_string(),
            id: ModuleId::of_class("Json"),
            version: Some(version("2.1.0")),
        };
        assert_eq!(identity.display_name(), "Json@2.1.0");
        let unversioned = ModuleIdentity {
            version: None,
            ..identity
        };
        assert_eq!(unversioned.display_name(), "Json");
    }
}

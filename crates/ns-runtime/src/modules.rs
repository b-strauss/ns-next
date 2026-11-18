//! Module identity: ids, semver versions, version ranges and the relative
//! paths that name a module's source (`docs/design/modules.md`, decisions 1,
//! 2, 4 and 9). Pure values with no dependency on the syntax crate, so the
//! image decoder and the tools hold a module to the same rules the loader does.

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

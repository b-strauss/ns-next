//! The file a `file` URI names, for a request about a document the client
//! never opened. `lsp-types` 0.97 wraps `fluent_uri::Uri`, which parses a
//! URI and percent-decodes a component but knows nothing of file paths, so
//! the conversion is written here: a scheme check, the decoded path, and a
//! drive letter or a host turned into a Windows path.

use std::path::PathBuf;

use fluent_uri::HostData;
use lsp_types::Uri;

/// How a URI's path becomes a file path; a parameter rather than a `cfg`
/// so that both are tested on every host.
#[derive(Clone, Copy)]
enum PathStyle {
    Windows,
    Posix,
}

const HOST_STYLE: PathStyle = if cfg!(windows) {
    PathStyle::Windows
} else {
    PathStyle::Posix
};

/// The absolute path of the file `uri` names on this host, or `None` for a
/// URI that names none: another scheme, a query or fragment, a user or port,
/// a path that is not UTF-8 once decoded, or one this host cannot address.
pub fn file_path_of(uri: &Uri) -> Option<PathBuf> {
    path_text_of(uri, HOST_STYLE).map(PathBuf::from)
}

fn path_text_of(uri: &Uri, style: PathStyle) -> Option<String> {
    if !uri.scheme()?.eq_lowercase("file") || uri.query().is_some() || uri.fragment().is_some() {
        return None;
    }
    let host = match uri.authority() {
        None => String::new(),
        Some(authority) => {
            if authority.userinfo().is_some() || authority.port().is_some() {
                return None;
            }
            match authority.host().data() {
                // A registered name may be percent-encoded like any other
                // component; the share is reached by its decoded name.
                HostData::RegName(name) => name.decode().into_string().ok()?.into_owned(),
                HostData::Ipv4(..) => authority.host().as_str().to_string(),
                // An IP literal in brackets (`[::1]`) is no server name a
                // UNC path can hold.
                _ => return None,
            }
        }
    };
    // RFC 8089: an empty host and `localhost` both mean this machine.
    let local = host.is_empty() || host.eq_ignore_ascii_case("localhost");
    // Decoded as a whole, so `c%3A` is the drive's colon and `%20` a space.
    let path = uri
        .path()
        .as_estr()
        .decode()
        .into_string()
        .ok()?
        .into_owned();
    // A NUL ends a path at the operating system's boundary; a path with one
    // would name a different file than the URI does. A separator in the host
    // would make the share's name a path of its own.
    if path.contains('\0') || !path.starts_with('/') || host.contains(['/', '\\', '\0']) {
        return None;
    }
    match style {
        PathStyle::Posix => local.then_some(path),
        PathStyle::Windows => {
            if !local {
                // `file://server/share/Module.ns` -> `\\server\share\Module.ns`.
                return Some(format!("\\\\{host}{}", path.replace('/', "\\")));
            }
            if let Some(share_path) = path.strip_prefix("//") {
                // `file:////server/share/Module.ns`, the form Java's
                // `File.toURI()` writes for a UNC path: the server is the
                // path's first segment, which must not be empty.
                let has_server = share_path
                    .split('/')
                    .next()
                    .is_some_and(|server| !server.is_empty());
                return has_server.then(|| format!("\\\\{}", share_path.replace('/', "\\")));
            }
            // `/C:/work/Module.ns` -> `C:\work\Module.ns`. Without a drive
            // letter the path would hang off whichever drive is current,
            // which is no file the client meant.
            let drive_path = path.strip_prefix('/')?;
            let bytes = drive_path.as_bytes();
            let has_drive = bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && bytes[2] == b'/';
            has_drive.then(|| drive_path.replace('/', "\\"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn path_of(uri: &str, style: PathStyle) -> Option<String> {
        path_text_of(&Uri::from_str(uri).expect("a valid URI"), style)
    }

    #[test]
    fn a_windows_drive_letter_becomes_a_drive_path() {
        assert_eq!(
            path_of("file:///C:/work/Module.ns", PathStyle::Windows).as_deref(),
            Some("C:\\work\\Module.ns")
        );
        // Visual Studio Code's form: lower-case drive, colon encoded.
        assert_eq!(
            path_of("file:///c%3A/work/Module.ns", PathStyle::Windows).as_deref(),
            Some("c:\\work\\Module.ns")
        );
        assert_eq!(
            path_of("file://localhost/D:/Module.ns", PathStyle::Windows).as_deref(),
            Some("D:\\Module.ns")
        );
    }

    #[test]
    fn percent_encoding_is_decoded() {
        assert_eq!(
            path_of(
                "file:///C:/my%20work/Gr%C3%BC%C3%9Fe.ns",
                PathStyle::Windows
            )
            .as_deref(),
            Some("C:\\my work\\Grüße.ns")
        );
        assert_eq!(
            path_of(
                "file:///home/me/my%20work/Gr%C3%BC%C3%9Fe.ns",
                PathStyle::Posix
            )
            .as_deref(),
            Some("/home/me/my work/Grüße.ns")
        );
    }

    #[test]
    fn a_posix_path_stays_as_it_is() {
        assert_eq!(
            path_of("file:///home/me/Module.ns", PathStyle::Posix).as_deref(),
            Some("/home/me/Module.ns")
        );
        assert_eq!(
            path_of("file://localhost/home/me/Module.ns", PathStyle::Posix).as_deref(),
            Some("/home/me/Module.ns")
        );
        assert_eq!(
            path_of("FILE:///home/me/Module.ns", PathStyle::Posix).as_deref(),
            Some("/home/me/Module.ns"),
            "a scheme is case-insensitive"
        );
    }

    #[test]
    fn a_host_is_a_unc_share_on_windows_and_no_file_on_posix() {
        assert_eq!(
            path_of("file://server/share/Module.ns", PathStyle::Windows).as_deref(),
            Some("\\\\server\\share\\Module.ns")
        );
        assert_eq!(
            path_of("file://server/share/Module.ns", PathStyle::Posix),
            None
        );
        assert_eq!(
            path_of("file://192.168.0.1/share/Module.ns", PathStyle::Windows).as_deref(),
            Some("\\\\192.168.0.1\\share\\Module.ns")
        );
    }

    /// Java's `File.toURI()` writes a UNC path with four slashes and no host.
    #[test]
    fn the_four_slash_form_is_a_unc_share_on_windows() {
        assert_eq!(
            path_of("file:////server/share/Module.ns", PathStyle::Windows).as_deref(),
            Some("\\\\server\\share\\Module.ns")
        );
        assert_eq!(
            path_of("file://///share/Module.ns", PathStyle::Windows),
            None,
            "a share needs its server"
        );
    }

    #[test]
    fn a_unc_host_is_decoded_and_an_ip_literal_refused() {
        assert_eq!(
            path_of("file://my%20server/share/Module.ns", PathStyle::Windows).as_deref(),
            Some("\\\\my server\\share\\Module.ns")
        );
        assert_eq!(
            path_of("file://[::1]/share/Module.ns", PathStyle::Windows),
            None
        );
        assert_eq!(
            path_of("file://a%2Fb/share/Module.ns", PathStyle::Windows),
            None,
            "a separator in the host"
        );
    }

    #[test]
    fn a_uri_that_names_no_file_answers_none() {
        for style in [PathStyle::Windows, PathStyle::Posix] {
            for uri in [
                "untitled:Module.ns",
                "https://example.org/Module.ns",
                "file:///C:/work/Module.ns?query",
                "file:///C:/work/Module.ns#fragment",
                "file://user@localhost/C:/Module.ns",
                "file://localhost:8080/C:/Module.ns",
                // Not UTF-8 once decoded.
                "file:///C:/%FF.ns",
                "file:///C:/Module%00.ns",
            ] {
                assert_eq!(path_of(uri, style), None, "{uri}");
            }
        }
        // A Windows path needs its drive.
        assert_eq!(path_of("file:///work/Module.ns", PathStyle::Windows), None);
        assert_eq!(path_of("file:///C:", PathStyle::Windows), None);
    }
}

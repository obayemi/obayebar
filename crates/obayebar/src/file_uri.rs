//! Resolving strings that name a local file: a `file:` URI as sent by MPRIS
//! art URLs and notification hints, or a plain absolute path.

use std::path::PathBuf;

use url::Url;

/// The local filesystem path named by `uri_or_path`, if it names one.
///
/// A `file:` URL is percent-decoded and rejected when it names another host.
/// A string starting with `/` is taken as an absolute path. Anything else —
/// a relative path, another URL scheme, or a bare icon name — is not a
/// local path and returns `None`.
#[must_use]
pub fn local_path(uri_or_path: &str) -> Option<PathBuf> {
    match Url::parse(uri_or_path) {
        Ok(url) if url.scheme() == "file" => url.to_file_path().ok(),
        Ok(_) => None,
        Err(_) => uri_or_path
            .starts_with('/')
            .then(|| PathBuf::from(uri_or_path)),
    }
}

#[cfg(test)]
mod tests {
    use super::local_path;
    use std::path::PathBuf;

    #[test]
    fn a_file_uri_is_percent_decoded() {
        assert_eq!(
            local_path("file:///home/me/My%20Pics/a.png"),
            Some(PathBuf::from("/home/me/My Pics/a.png"))
        );
    }

    #[test]
    fn a_plain_absolute_path_is_used_as_is() {
        assert_eq!(
            local_path("/home/me/pic.png"),
            Some(PathBuf::from("/home/me/pic.png"))
        );
    }

    #[test]
    fn a_file_uri_naming_another_host_is_rejected() {
        assert_eq!(local_path("file://otherhost/x"), None);
    }

    #[test]
    fn an_http_url_is_rejected() {
        assert_eq!(local_path("http://example.com/pic.png"), None);
    }

    #[test]
    fn a_relative_path_or_icon_name_is_rejected() {
        assert_eq!(local_path("icons/pic.png"), None);
        assert_eq!(local_path("firefox"), None);
    }
}

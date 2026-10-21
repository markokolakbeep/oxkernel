//! Path helpers shared by the kernel's name lookup and user tools.

/// Splits off the first component of `path`, skipping slashes. Returns `None`
/// once nothing is left.
///
/// `"a/bb/c"` gives `("a", "bb/c")`, `"///a//b"` gives `("a", "b")`.
pub fn split_first(path: &str) -> Option<(&str, &str)> {
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        return None;
    }
    match path.find('/') {
        Some(i) => Some((&path[..i], path[i..].trim_start_matches('/'))),
        None => Some((path, "")),
    }
}

/// The last component of `path`, or `""` for `/`.
pub fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits() {
        assert_eq!(split_first("a/bb/c"), Some(("a", "bb/c")));
        assert_eq!(split_first("///a//b"), Some(("a", "b")));
        assert_eq!(split_first("a"), Some(("a", "")));
        assert_eq!(split_first("a/"), Some(("a", "")));
        assert_eq!(split_first("/"), None);
        assert_eq!(split_first(""), None);
    }

    #[test]
    fn basenames() {
        assert_eq!(basename("/bin/sh"), "sh");
        assert_eq!(basename("sh"), "sh");
        assert_eq!(basename("/tmp/dir/"), "dir");
        assert_eq!(basename("/"), "");
    }
}

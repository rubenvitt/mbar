//! Version numbers. `CFBundleVersion` and Sparkle's `sparkle:version` must grow with
//! every release, so they are derived from the semver version:
//! `major * 1_000_000 + minor * 1_000 + patch` (minor and patch below 1000).

/// The workspace version (all crates share it).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `"1.2.3"` → `(1, 2, 3)`. Pre-release and build suffixes are rejected.
pub fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.split('.');
    let mut next = || it.next()?.parse::<u64>().ok();
    let v = (next()?, next()?, next()?);
    if s.split('.').count() != 3 {
        return None;
    }
    Some(v)
}

/// Build number for a semver version, see the module docs.
pub fn build_number(version: &str) -> Option<u64> {
    let (major, minor, patch) = parse_semver(version)?;
    if minor >= 1_000 || patch >= 1_000 {
        return None;
    }
    Some(major * 1_000_000 + minor * 1_000 + patch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_number_is_monotonic_encoding() {
        assert_eq!(build_number("0.1.0"), Some(1_000));
        assert_eq!(build_number("1.2.3"), Some(1_002_003));
        assert_eq!(build_number("0.10.0"), Some(10_000));
        assert!(build_number("0.9.999").unwrap() < build_number("0.10.0").unwrap());
    }

    #[test]
    fn build_number_rejects_garbage() {
        assert_eq!(build_number(""), None);
        assert_eq!(build_number("1.2"), None);
        assert_eq!(build_number("1.2.3-beta.1"), None);
        assert_eq!(build_number("1.1000.0"), None);
    }

    #[test]
    fn version_matches_cargo() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(build_number(VERSION).is_some());
    }
}

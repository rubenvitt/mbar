//! Running from `mbar.app`: locating the bundle from the executable path and reading
//! the few `Info.plist` values mbar needs. The plist is the XML file written by
//! `packaging/macos/build-app.sh`, so a small tag scanner is enough.

use std::path::{Path, PathBuf};

/// `PATH` used when the daemon's own `PATH` is empty (launchd gives agents only the
/// system directories); same list as the LaunchAgent template.
pub const DEFAULT_SCRIPT_PATH: &str =
    "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppBundle {
    pub root: PathBuf,
    pub short_version: String,
    pub build: u64,
    pub feed_url: Option<String>,
    /// `SUEnableAutomaticChecks` from `Info.plist` (a user default overrides it).
    pub auto_checks_default: bool,
}

impl AppBundle {
    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("Contents/Resources/bin")
    }

    pub fn info_plist(&self) -> PathBuf {
        self.root.join("Contents/Info.plist")
    }
}

/// `…/Name.app/Contents/MacOS/<exe>` → `…/Name.app`.
pub fn bundle_root_from_exe(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let root = contents.parent()?;
    let is_app = root.extension().and_then(|e| e.to_str()) == Some("app");
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && is_app)
        .then(|| root.to_path_buf())
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The text after `<key>key</key>` up to the next tag, if that tag is `<string>`.
pub fn plist_string(xml: &str, key: &str) -> Option<String> {
    let rest = &xml[xml.find(&format!("<key>{key}</key>"))? + key.len() + 11..];
    let rest = rest.trim_start().strip_prefix("<string>")?;
    Some(unescape(&rest[..rest.find("</string>")?]))
}

pub fn plist_bool(xml: &str, key: &str) -> Option<bool> {
    let rest = &xml[xml.find(&format!("<key>{key}</key>"))? + key.len() + 11..];
    let rest = rest.trim_start();
    if rest.starts_with("<true/>") {
        Some(true)
    } else if rest.starts_with("<false/>") {
        Some(false)
    } else {
        None
    }
}

pub fn read_bundle(root: &Path) -> Option<AppBundle> {
    let xml = std::fs::read_to_string(root.join("Contents/Info.plist")).ok()?;
    Some(AppBundle {
        root: root.to_path_buf(),
        short_version: plist_string(&xml, "CFBundleShortVersionString")?,
        build: plist_string(&xml, "CFBundleVersion")?.parse().ok()?,
        feed_url: plist_string(&xml, "SUFeedURL").filter(|s| !s.is_empty()),
        auto_checks_default: plist_bool(&xml, "SUEnableAutomaticChecks").unwrap_or(true),
    })
}

/// `bin_dir` first, then `current` (or [`DEFAULT_SCRIPT_PATH`] when empty), without
/// adding `bin_dir` twice.
pub fn script_path(bin_dir: &Path, current: &str) -> String {
    let bin = bin_dir.to_string_lossy();
    let base = if current.is_empty() {
        DEFAULT_SCRIPT_PATH
    } else {
        current
    };
    if base.split(':').next() == Some(&*bin) {
        return base.to_string();
    }
    format!("{bin}:{base}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
	<key>CFBundleShortVersionString</key>
	<string>0.2.0</string>
	<key>CFBundleVersion</key>
	<string>2000</string>
	<key>SUFeedURL</key>
	<string>https://example.org/a&amp;b.xml</string>
	<key>SUEnableAutomaticChecks</key>
	<true/>
</dict></plist>"#;

    #[test]
    fn root_from_exe() {
        assert_eq!(
            bundle_root_from_exe(Path::new("/Applications/mbar.app/Contents/MacOS/mbar")),
            Some(PathBuf::from("/Applications/mbar.app"))
        );
        assert_eq!(bundle_root_from_exe(Path::new("/usr/local/bin/mbar")), None);
        assert_eq!(
            bundle_root_from_exe(Path::new("/x/Contents/MacOS/mbar")),
            None
        );
    }

    #[test]
    fn plist_values() {
        assert_eq!(
            plist_string(PLIST, "CFBundleVersion").as_deref(),
            Some("2000")
        );
        assert_eq!(
            plist_string(PLIST, "SUFeedURL").as_deref(),
            Some("https://example.org/a&b.xml")
        );
        assert_eq!(plist_string(PLIST, "Missing"), None);
        assert_eq!(plist_bool(PLIST, "SUEnableAutomaticChecks"), Some(true));
        assert_eq!(plist_bool(PLIST, "CFBundleVersion"), None);
    }

    #[test]
    fn read_bundle_from_disk() {
        let root = std::env::temp_dir().join(format!("mbar-bundle-{}.app", std::process::id()));
        std::fs::create_dir_all(root.join("Contents")).unwrap();
        std::fs::write(root.join("Contents/Info.plist"), PLIST).unwrap();
        let b = read_bundle(&root).unwrap();
        assert_eq!(b.short_version, "0.2.0");
        assert_eq!(b.build, 2000);
        assert_eq!(b.feed_url.as_deref(), Some("https://example.org/a&b.xml"));
        assert!(b.auto_checks_default);
        assert_eq!(b.bin_dir(), root.join("Contents/Resources/bin"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn script_path_prepends_once() {
        let bin = Path::new("/Applications/mbar.app/Contents/Resources/bin");
        assert_eq!(
            script_path(bin, "/usr/bin:/bin"),
            "/Applications/mbar.app/Contents/Resources/bin:/usr/bin:/bin"
        );
        assert_eq!(
            script_path(
                bin,
                "/Applications/mbar.app/Contents/Resources/bin:/usr/bin"
            ),
            "/Applications/mbar.app/Contents/Resources/bin:/usr/bin"
        );
        assert_eq!(
            script_path(bin, ""),
            format!("/Applications/mbar.app/Contents/Resources/bin:{DEFAULT_SCRIPT_PATH}")
        );
    }
}

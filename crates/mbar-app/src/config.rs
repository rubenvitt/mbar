//! Config file lookup shared by the daemon and `mbar-ui`.

use std::path::{Path, PathBuf};

/// Default bar name (same value as `mbar_ipc::DEFAULT_BAR_NAME`; duplicated so this
/// crate stays dependency-free).
const DEFAULT_BAR_NAME: &str = "mbar";

/// Config lookup (`docs/ARCHITECTURE.md`, `docs/LUA.md`, `cli.md` §10.1): the first
/// existing regular file of
///
/// 1. `$XDG_CONFIG_HOME/<bar>/{init.lua,mbarrc,sketchybarrc}` (XDG set and non-empty),
/// 2. `$HOME/.config/<bar>/{init.lua,mbarrc,sketchybarrc}`,
/// 3. for the default bar name, the SketchyBar directories
///    `$XDG_CONFIG_HOME/sketchybar/…` and `$HOME/.config/sketchybar/…`
///    (`init.lua`, then `sketchybarrc`),
/// 4. `$HOME/.sketchybarrc`.
///
/// `init.lua` wins over the shell config in the same directory. An empty `HOME` skips
/// the `HOME` entries.
pub fn find_config(bar_name: &str, xdg: &str, home: &str) -> Option<PathBuf> {
    config_candidates(bar_name, xdg, home)
        .into_iter()
        .find(|p| p.is_file())
}

pub fn config_candidates(bar_name: &str, xdg: &str, home: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<(PathBuf, &[&str])> = Vec::new();
    const MBAR: &[&str] = &["init.lua", "mbarrc", "sketchybarrc"];
    const SKETCHYBAR: &[&str] = &["init.lua", "sketchybarrc"];
    if !xdg.is_empty() {
        dirs.push((Path::new(xdg).join(bar_name), MBAR));
    }
    if !home.is_empty() {
        dirs.push((Path::new(home).join(".config").join(bar_name), MBAR));
    }
    if bar_name == DEFAULT_BAR_NAME {
        if !xdg.is_empty() {
            dirs.push((Path::new(xdg).join("sketchybar"), SKETCHYBAR));
        }
        if !home.is_empty() {
            dirs.push((Path::new(home).join(".config/sketchybar"), SKETCHYBAR));
        }
    }
    let mut out: Vec<PathBuf> = dirs
        .into_iter()
        .flat_map(|(d, names)| names.iter().map(move |n| d.join(n)))
        .collect();
    if !home.is_empty() {
        out.push(Path::new(home).join(".sketchybarrc"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_order() {
        let c = config_candidates("mbar", "/x", "/h");
        let c: Vec<_> = c.iter().map(|p| p.to_str().unwrap()).collect();
        assert_eq!(
            c,
            [
                "/x/mbar/init.lua",
                "/x/mbar/mbarrc",
                "/x/mbar/sketchybarrc",
                "/h/.config/mbar/init.lua",
                "/h/.config/mbar/mbarrc",
                "/h/.config/mbar/sketchybarrc",
                "/x/sketchybar/init.lua",
                "/x/sketchybar/sketchybarrc",
                "/h/.config/sketchybar/init.lua",
                "/h/.config/sketchybar/sketchybarrc",
                "/h/.sketchybarrc",
            ]
        );
        let c = config_candidates("bottom", "", "");
        assert!(c.is_empty());
        let c = config_candidates("bottom", "", "/h");
        assert_eq!(c.len(), 4);
        assert_eq!(c[0], Path::new("/h/.config/bottom/init.lua"));
    }

    #[test]
    fn lookup_prefers_init_lua() {
        let home = std::env::temp_dir().join(format!("mbar-cfg-{}", std::process::id()));
        let dir = home.join(".config/mbar");
        std::fs::create_dir_all(&dir).unwrap();
        let sb = home.join(".config/sketchybar");
        std::fs::create_dir_all(&sb).unwrap();
        std::fs::write(sb.join("sketchybarrc"), "").unwrap();
        let h = home.to_str().unwrap();
        assert_eq!(find_config("mbar", "", h), Some(sb.join("sketchybarrc")));
        std::fs::write(dir.join("mbarrc"), "").unwrap();
        assert_eq!(find_config("mbar", "", h), Some(dir.join("mbarrc")));
        std::fs::write(dir.join("init.lua"), "").unwrap();
        assert_eq!(find_config("mbar", "", h), Some(dir.join("init.lua")));
        // A directory named like a config is skipped.
        std::fs::remove_file(dir.join("init.lua")).unwrap();
        std::fs::create_dir(dir.join("init.lua")).unwrap();
        assert_eq!(find_config("mbar", "", h), Some(dir.join("mbarrc")));
        std::fs::remove_dir_all(&home).unwrap();
    }
}

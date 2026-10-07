//! First-launch onboarding: what to clean up, what to convert, what to install. Pure
//! detection over the file system plus command/script builders; the GUI runs them.

use std::path::{Path, PathBuf};

pub const STARTER_INIT_LUA: &str = include_str!("../assets/starter-init.lua");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Applications,
    Translocated,
    DiskImage,
    Elsewhere,
}

pub fn location(bundle_root: &Path) -> Location {
    let s = bundle_root.to_string_lossy();
    if s.contains("/AppTranslocation/") {
        Location::Translocated
    } else if s.starts_with("/Volumes/") {
        Location::DiskImage
    } else if bundle_root.parent() == Some(Path::new("/Applications")) {
        Location::Applications
    } else {
        Location::Elsewhere
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathsD {
    Missing,
    Current,
    Stale(String),
}

pub fn paths_d_content(bin_dir: &Path) -> String {
    format!("{}\n", bin_dir.display())
}

pub fn paths_d_state(current: Option<&str>, bin_dir: &Path) -> PathsD {
    match current.map(str::trim) {
        None | Some("") => PathsD::Missing,
        Some(line) if Path::new(line) == bin_dir => PathsD::Current,
        Some(line) => PathsD::Stale(line.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OldKind {
    /// An mbar binary or `sketchybar -> mbar` link outside this app: removed.
    Binary,
    /// `~/Library/LaunchAgents/dev.rubeen.mbar.plist` from `make install-agent`: booted out, removed.
    LaunchAgent,
    /// Something else named `sketchybar` that shadows the app on the PATH: reported only.
    Foreign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldInstall {
    pub path: PathBuf,
    pub kind: OldKind,
    /// Needs the admin script (`/usr/local/bin`).
    pub admin: bool,
}

fn inside(path: &Path, root: Option<&Path>) -> bool {
    root.is_some_and(|r| {
        path.starts_with(r) || std::fs::canonicalize(r).is_ok_and(|r| path.starts_with(r))
    })
}

/// First symlink hop (relative links against their directory); `None` when not a link.
fn link_target(p: &Path) -> Option<PathBuf> {
    let t = std::fs::read_link(p).ok()?;
    Some(if t.is_absolute() {
        t
    } else {
        p.parent()?.join(t)
    })
}

fn classify(path: &Path, bundle_root: Option<&Path>) -> Option<OldKind> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let target = link_target(path);
    // The whole chain, resolved: `sketchybar -> mbar -> <app>` or `../../Applications/…`
    // must count as a link into the app, never as an old install to remove.
    let resolved = target
        .as_ref()
        .and_then(|_| std::fs::canonicalize(path).ok());
    if [&target, &resolved]
        .into_iter()
        .flatten()
        .any(|t| inside(t, bundle_root))
    {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let points_to_mbar = [&target, &resolved]
        .into_iter()
        .flatten()
        .filter_map(|t| t.file_name())
        .any(|n| n == "mbar" || n == "mbar-ui");
    match name {
        "mbar" | "mbar-ui" => Some(OldKind::Binary),
        "sketchybar" if points_to_mbar => Some(OldKind::Binary),
        "sketchybar" if meta.is_file() || target.is_some() => Some(OldKind::Foreign),
        _ => None,
    }
}

pub fn find_old_installs(
    home: &Path,
    usr_local_bin: &Path,
    bundle_root: Option<&Path>,
) -> Vec<OldInstall> {
    let mut out = Vec::new();
    for (dir, admin) in [
        (home.join(".local/bin"), false),
        (usr_local_bin.to_path_buf(), true),
    ] {
        for name in ["mbar", "sketchybar", "mbar-ui"] {
            let p = dir.join(name);
            if let Some(kind) = classify(&p, bundle_root) {
                out.push(OldInstall {
                    path: p,
                    kind,
                    admin,
                });
            }
        }
    }
    let agent = home.join("Library/LaunchAgents/dev.rubeen.mbar.plist");
    if agent.exists() {
        out.push(OldInstall {
            path: agent,
            kind: OldKind::LaunchAgent,
            admin: false,
        });
    }
    out
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrewState {
    pub sketchybar_installed: bool,
    pub sketchybar_running: bool,
    pub mbar_installed: bool,
}

/// `formulas` = `brew list --formula`, `services` = `brew services list`.
pub fn parse_brew(formulas: &str, services: &str) -> BrewState {
    let has = |n: &str| formulas.lines().any(|l| l.trim() == n);
    BrewState {
        sketchybar_installed: has("sketchybar"),
        sketchybar_running: services.lines().any(|l| {
            let mut f = l.split_whitespace();
            f.next() == Some("sketchybar") && f.next() == Some("started")
        }),
        mbar_installed: has("mbar"),
    }
}

/// SbarLua `sketchybarrc` (a `lua` shebang) → `init.lua` body without the shebang and
/// `package.cpath` lines (`docs/MIGRATING.md`).
pub fn sbarlua_init_lua(sketchybarrc: &str) -> Option<String> {
    let first = sketchybarrc.lines().next()?;
    if !(first.starts_with("#!") && first.contains("lua")) {
        return None;
    }
    let body: Vec<&str> = sketchybarrc
        .lines()
        .skip(1)
        .filter(|l| !l.contains("package.cpath"))
        .collect();
    Some(format!("{}\n", body.join("\n")))
}

/// Source files under the config dir that look up SketchyBar's mach service.
pub fn felix_helpers(config_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![config_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if matches!(
                p.extension().and_then(|x| x.to_str()),
                Some("c" | "h" | "m" | "swift" | "lua" | "sh")
            ) && std::fs::read_to_string(&p).is_ok_and(|s| s.contains("git.felix."))
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub fn needs_starter_config(home: &Path, xdg: &str) -> bool {
    mbar_app::config::find_config("mbar", xdg, &home.to_string_lossy()).is_none()
}

pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// One root shell command: write `/etc/paths.d/mbar`, remove admin-owned old binaries.
pub fn admin_shell(bin_dir: &Path, admin_removals: &[PathBuf]) -> String {
    let mut cmd = format!(
        "printf '%s\\n' {} > /etc/paths.d/mbar",
        shell_quote(&bin_dir.to_string_lossy())
    );
    for p in admin_removals {
        cmd.push_str(&format!(" && rm -f {}", shell_quote(&p.to_string_lossy())));
    }
    cmd
}

/// AppleScript that runs `shell` as root after the standard admin prompt.
pub fn applescript_admin(shell: &str) -> String {
    let escaped = shell.replace('\\', "\\\\").replace('"', "\\\"");
    format!("do shell script \"{escaped}\" with administrator privileges")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mbar-ob-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn location_kinds() {
        assert_eq!(
            location(Path::new("/Applications/mbar.app")),
            Location::Applications
        );
        assert_eq!(
            location(Path::new(
                "/private/var/folders/x/AppTranslocation/ABC/d/mbar.app"
            )),
            Location::Translocated
        );
        assert_eq!(
            location(Path::new("/Volumes/mbar/mbar.app")),
            Location::DiskImage
        );
        assert_eq!(
            location(Path::new("/Users/r/Downloads/mbar.app")),
            Location::Elsewhere
        );
    }

    #[test]
    fn paths_d_state_detects_stale_entry() {
        let bin = Path::new("/Applications/mbar.app/Contents/Resources/bin");
        assert_eq!(paths_d_state(None, bin), PathsD::Missing);
        assert_eq!(
            paths_d_state(Some("/Applications/mbar.app/Contents/Resources/bin\n"), bin),
            PathsD::Current
        );
        assert_eq!(
            paths_d_state(
                Some("/Users/r/Applications/mbar.app/Contents/Resources/bin\n"),
                bin
            ),
            PathsD::Stale("/Users/r/Applications/mbar.app/Contents/Resources/bin".into())
        );
        assert_eq!(
            paths_d_content(bin),
            "/Applications/mbar.app/Contents/Resources/bin\n"
        );
    }

    #[test]
    fn old_installs_found() {
        let home = tmp("home");
        let ulb = tmp("ulb");
        let lb = home.join(".local/bin");
        std::fs::create_dir_all(&lb).unwrap();
        std::fs::write(lb.join("mbar"), "bin").unwrap();
        symlink("mbar", lb.join("sketchybar")).unwrap();
        std::fs::write(lb.join("mbar-ui"), "bin").unwrap();
        let la = home.join("Library/LaunchAgents");
        std::fs::create_dir_all(&la).unwrap();
        std::fs::write(la.join("dev.rubeen.mbar.plist"), "<plist/>").unwrap();
        std::fs::write(ulb.join("mbar"), "bin").unwrap();

        let found = find_old_installs(&home, &ulb, Some(Path::new("/Applications/mbar.app")));
        let names: Vec<_> = found
            .iter()
            .map(|o| (o.path.clone(), o.kind.clone(), o.admin))
            .collect();
        assert!(names.contains(&(lb.join("mbar"), OldKind::Binary, false)));
        assert!(names.contains(&(lb.join("sketchybar"), OldKind::Binary, false)));
        assert!(names.contains(&(lb.join("mbar-ui"), OldKind::Binary, false)));
        assert!(names.contains(&(
            la.join("dev.rubeen.mbar.plist"),
            OldKind::LaunchAgent,
            false
        )));
        assert!(names.contains(&(ulb.join("mbar"), OldKind::Binary, true)));
    }

    #[test]
    fn foreign_sketchybar_is_reported_not_removed() {
        let home = tmp("foreign");
        let ulb = tmp("foreign-ulb");
        let lb = home.join(".local/bin");
        std::fs::create_dir_all(&lb).unwrap();
        std::fs::write(lb.join("sketchybar"), "#!/bin/sh\necho mine\n").unwrap();
        let found = find_old_installs(&home, &ulb, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, OldKind::Foreign);
    }

    #[test]
    fn links_into_the_app_are_kept() {
        let home = tmp("keep");
        let ulb = tmp("keep-ulb");
        symlink(
            "/Applications/mbar.app/Contents/MacOS/mbar",
            ulb.join("mbar"),
        )
        .unwrap();
        assert!(
            find_old_installs(&home, &ulb, Some(Path::new("/Applications/mbar.app"))).is_empty()
        );
    }

    #[test]
    fn chained_and_relative_links_into_the_app_are_kept() {
        let root = tmp("chain");
        let app = root.join("Applications/mbar.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/mbar"), "bin").unwrap();
        let home = root.join("home");
        let lb = home.join(".local/bin");
        std::fs::create_dir_all(&lb).unwrap();
        let ulb = root.join("ulb");
        std::fs::create_dir_all(&ulb).unwrap();
        // ~/.local/bin/mbar -> ../../../Applications/mbar.app/… (relative, with `..`)
        symlink(
            "../../../Applications/mbar.app/Contents/MacOS/mbar",
            lb.join("mbar"),
        )
        .unwrap();
        // ~/.local/bin/sketchybar -> mbar -> app (two hops)
        symlink("mbar", lb.join("sketchybar")).unwrap();
        assert_eq!(find_old_installs(&home, &ulb, Some(&app)), vec![]);
    }

    #[test]
    fn brew_parsing() {
        let s = parse_brew("lua\nsketchybar\njq\n", "Name Status User File\nsketchybar started r ~/Library/LaunchAgents/homebrew.mxcl.sketchybar.plist\n");
        assert_eq!(
            s,
            BrewState {
                sketchybar_installed: true,
                sketchybar_running: true,
                mbar_installed: false
            }
        );
        let s = parse_brew("mbar\n", "sketchybar none\n");
        assert_eq!(
            s,
            BrewState {
                sketchybar_installed: false,
                sketchybar_running: false,
                mbar_installed: true
            }
        );
    }

    #[test]
    fn sbarlua_conversion() {
        let rc = "#!/usr/bin/env lua\npackage.cpath = package.cpath .. \";/x/?.so\"\nsbar = require(\"sketchybar\")\nsbar.begin_config()\n";
        assert_eq!(
            sbarlua_init_lua(rc).unwrap(),
            "sbar = require(\"sketchybar\")\nsbar.begin_config()\n"
        );
        assert_eq!(
            sbarlua_init_lua("#!/bin/bash\nsketchybar --bar height=30\n"),
            None
        );
    }

    #[test]
    fn felix_helper_detection() {
        let dir = tmp("felix");
        std::fs::create_dir_all(dir.join("helpers/x")).unwrap();
        std::fs::write(
            dir.join("helpers/x/sketchybar.h"),
            "snprintf(b, n, \"git.felix.%s\", name);",
        )
        .unwrap();
        std::fs::write(dir.join("init.lua"), "-- nothing").unwrap();
        assert_eq!(
            felix_helpers(&dir),
            vec![dir.join("helpers/x/sketchybar.h")]
        );
    }

    #[test]
    fn starter_needed_only_without_config() {
        let home = tmp("starter");
        assert!(needs_starter_config(&home, ""));
        std::fs::create_dir_all(home.join(".config/sketchybar")).unwrap();
        std::fs::write(home.join(".config/sketchybar/sketchybarrc"), "").unwrap();
        assert!(!needs_starter_config(&home, ""));
    }

    #[test]
    fn admin_script_quoting() {
        let s = admin_shell(
            Path::new("/Applications/mbar's.app/Contents/Resources/bin"),
            &[PathBuf::from("/usr/local/bin/mbar")],
        );
        assert_eq!(
            s,
            "printf '%s\\n' '/Applications/mbar'\\''s.app/Contents/Resources/bin' > /etc/paths.d/mbar && rm -f '/usr/local/bin/mbar'"
        );
        let a = applescript_admin("echo \"hi\" \\ there");
        assert_eq!(
            a,
            "do shell script \"echo \\\"hi\\\" \\\\ there\" with administrator privileges"
        );
    }
}

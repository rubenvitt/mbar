//! First-launch onboarding: what to clean up, what to convert, what to install. Pure
//! detection over the file system plus command/script builders; the GUI runs them.
//!
//! Covers the SketchyBar take-over (`docs/MIGRATING.md`) and the JankyBorders one
//! (`docs/superpowers/specs/2026-10-09-borders-design.md` §5): the Homebrew `borders`
//! formula and service, `borders` binaries and links, `bordersrc` (read in place,
//! `docs/spec/borders.md` §4) and window-manager lines that launch `borders`.

use std::path::{Path, PathBuf};
use std::process::Command;

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
    /// An mbar binary or `sketchybar -> mbar` / `borders -> mbar` link outside this
    /// app: removed.
    Binary,
    /// `~/Library/LaunchAgents/dev.rubeen.mbar.plist` from `make install-agent`: booted out, removed.
    LaunchAgent,
    /// Something else named `sketchybar` or `borders` that shadows the app on the PATH:
    /// reported only.
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
        "sketchybar" | "borders" if points_to_mbar => Some(OldKind::Binary),
        "sketchybar" | "borders" if meta.is_file() || target.is_some() => Some(OldKind::Foreign),
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
        for name in ["mbar", "sketchybar", "borders", "mbar-ui"] {
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
    /// JankyBorders (`felixkratz/formulae/borders`; formula and service are both
    /// named `borders`).
    pub borders_installed: bool,
    pub borders_running: bool,
}

impl BrewState {
    /// The cleanup has something to do for Homebrew `borders`.
    pub fn borders_has_work(&self) -> bool {
        self.borders_installed || self.borders_running
    }
}

/// `formulas` = `brew list --formula`, `services` = `brew services list`.
pub fn parse_brew(formulas: &str, services: &str) -> BrewState {
    let has = |n: &str| formulas.lines().any(|l| l.trim() == n);
    let started = |n: &str| {
        services.lines().any(|l| {
            let mut f = l.split_whitespace();
            f.next() == Some(n) && f.next() == Some("started")
        })
    };
    BrewState {
        sketchybar_installed: has("sketchybar"),
        sketchybar_running: started("sketchybar"),
        mbar_installed: has("mbar"),
        borders_installed: has("borders"),
        borders_running: started("borders"),
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
    files_containing(config_dir, "git.felix.")
}

/// Source files under `~/.config/borders` that talk to JankyBorders' mach service
/// (`git.felix.borders`). mbar does not register it (design doc, non-goals): they have
/// to run `borders …` or `mbar --borders …` instead.
pub fn borders_helpers(config_dir: &Path) -> Vec<PathBuf> {
    files_containing(config_dir, "git.felix.borders")
}

/// Source files (by extension) under `dir`, recursively, whose text contains `needle`.
fn files_containing(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
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
            ) && std::fs::read_to_string(&p).is_ok_and(|s| s.contains(needle))
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// The JankyBorders config mbar runs in place after its own config:
/// `~/.config/borders/bordersrc`, else `~/.bordersrc`, if one is a regular file
/// (`docs/spec/borders.md` §4; `$XDG_CONFIG_HOME` is not consulted).
pub fn bordersrc(home: &Path) -> Option<PathBuf> {
    [
        home.join(".config/borders/bordersrc"),
        home.join(".bordersrc"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Window-manager configs that commonly start `borders`, in the order they are listed.
pub fn window_manager_configs(home: &Path) -> [PathBuf; 4] {
    [
        home.join(".aerospace.toml"),
        home.join(".config/aerospace/aerospace.toml"),
        home.join(".yabairc"),
        home.join(".config/yabai/yabairc"),
    ]
}

/// What the take-over step says about window-manager lines that start `borders`.
pub const LAUNCH_LINE_ADVICE: &str = "These keep working through mbar's `borders` command \
     when the window manager's PATH contains the /etc/paths.d entries (a login shell's \
     does). Otherwise remove them: mbar already runs your bordersrc.";

/// A line in a window-manager config that starts `borders`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchLine {
    pub file: PathBuf,
    /// 1-based.
    pub line: usize,
    /// The line as written, trimmed.
    pub text: String,
}

/// Lines that start `borders` in the window-manager configs that exist under `home`.
pub fn borders_launch_lines(home: &Path) -> Vec<LaunchLine> {
    let mut out = Vec::new();
    for file in window_manager_configs(home) {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (line, text) in borders_launch_lines_in(&content) {
            out.push(LaunchLine {
                file: file.clone(),
                line,
                text,
            });
        }
    }
    out
}

/// `(1-based line number, trimmed line)` of every line of a shell script (`yabairc`)
/// or TOML file (`aerospace.toml`) that runs `borders` as a command: `borders k=v &`,
/// `exec-and-forget borders …`, `/opt/homebrew/bin/borders …`,
/// `brew services start borders`. Comments and `borders` as an argument
/// (`sketchybar --set borders …`, `command -v borders`, `pkill borders`) do not count.
pub fn borders_launch_lines_in(content: &str) -> Vec<(usize, String)> {
    content
        .lines()
        .enumerate()
        .filter(|(_, l)| line_runs_borders(l))
        .map(|(i, l)| (i + 1, l.trim().to_string()))
        .collect()
}

fn line_runs_borders(line: &str) -> bool {
    // Command boundaries in shell and TOML: separators, subshells and quotes (TOML
    // strings and `action="…"` arguments hold whole commands).
    strip_comment(line)
        .split(|c: char| {
            matches!(
                c,
                ';' | '&' | '|' | '(' | ')' | '`' | '\'' | '"' | '[' | ']' | '{' | '}' | ','
            )
        })
        .any(segment_runs_borders)
}

/// The line up to a `#` that starts a comment (at the start or after whitespace).
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            return &line[..i];
        }
    }
    line
}

/// Whether the command of one segment is `borders` (after launchers and environment
/// assignments) or `brew services start|restart|run borders`.
fn segment_runs_borders(segment: &str) -> bool {
    let mut words = segment.split_whitespace().peekable();
    // `FOO=bar borders …`
    while words
        .peek()
        .is_some_and(|w| w.contains('=') && !w.starts_with('='))
    {
        words.next();
    }
    while let Some(word) = words.next() {
        match word {
            "exec-and-forget" | "exec" | "nohup" | "env" | "time" => {
                // The launcher's options and assignments.
                while words
                    .peek()
                    .is_some_and(|w| w.starts_with('-') || w.contains('='))
                {
                    words.next();
                }
            }
            // `command -v borders` only looks it up.
            "command" if words.peek().is_some_and(|w| w.starts_with('-')) => return false,
            "command" => {}
            w if w == "brew" || w.ends_with("/brew") => {
                let rest: Vec<&str> = words.take(3).collect();
                return matches!(
                    rest.as_slice(),
                    ["services", "start" | "restart" | "run", "borders"]
                );
            }
            w => return w == "borders" || w.ends_with("/borders"),
        }
    }
    false
}

/// Checks the result of `command -v <name>` in a new login shell: it must resolve into
/// the bundle's `bin` directory (a Homebrew `sketchybar` or `borders` earlier on the
/// PATH shadows mbar's links).
pub fn check_command(bin: &Path, shell: &str, name: &str, found: &str) -> Result<String, String> {
    let found = found.trim();
    if found.is_empty() {
        Err(format!(
            "A new {shell} login shell does not find {name}; open a new terminal or check \
             /etc/paths.d/mbar."
        ))
    } else if found.starts_with(&*bin.to_string_lossy()) {
        Ok(format!("{name} → {found}"))
    } else {
        Err(format!(
            "A new {shell} login shell finds {name} at '{found}'. Something earlier on \
             your PATH shadows mbar."
        ))
    }
}

/// The commands the command-line step checks with `check_command`.
pub const CHECKED_COMMANDS: [&str; 2] = ["sketchybar", "borders"];

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

/// The onboarding steps in the order setup runs them.
///
/// `Cleanup` must come before `LoginItem`: the legacy
/// `~/Library/LaunchAgents/dev.rubeen.mbar.plist` uses the same launchd label as the
/// bundled agent, so the cleanup's `launchctl bootout gui/<uid>/dev.rubeen.mbar` would
/// stop the bundled agent if it were already registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStep {
    Location,
    Cleanup,
    TakeOver,
    Starter,
    CommandLine,
    LoginItem,
    Permissions,
}

impl SetupStep {
    pub const ALL: [SetupStep; 7] = [
        SetupStep::Location,
        SetupStep::Cleanup,
        SetupStep::TakeOver,
        SetupStep::Starter,
        SetupStep::CommandLine,
        SetupStep::LoginItem,
        SetupStep::Permissions,
    ];
}

/// The cleanup boots out the launchd label `dev.rubeen.mbar`, which the bundled login
/// item shares. When the login item was already registered before the cleanup ran
/// (setup run again, or the cleanup retried after `LoginItem`), the executor must
/// register it again afterwards; `SetupStep::ALL` keeps the first run safe.
// The setup view re-registers the login item after such a cleanup when it was enabled.
pub fn cleanup_stops_login_item(items: &[OldInstall]) -> bool {
    items.iter().any(|i| i.kind == OldKind::LaunchAgent)
}

pub fn find_brew() -> Option<PathBuf> {
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

/// User-level cleanup commands, in order: Homebrew services and formulas (sketchybar,
/// borders, mbar; each service stops before its formula is uninstalled), the legacy
/// launch agent (bootout, then remove the plist), user-owned old binaries. Admin-owned
/// binaries go through `admin_shell`; `Foreign` entries are never touched.
///
/// `remove_brew_borders`: a Homebrew `borders` earlier on the PATH shadows mbar's
/// `borders` link, so the setup view defaults it to on.
pub fn cleanup_commands(
    items: &[OldInstall],
    brew: &BrewState,
    brew_bin: Option<&Path>,
    uid: u32,
    remove_brew_sketchybar: bool,
    remove_brew_borders: bool,
) -> Vec<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut out = Vec::new();
    if let Some(b) = brew_bin.map(|p| p.to_string_lossy().into_owned()) {
        if brew.sketchybar_installed || brew.sketchybar_running {
            out.push(s(&[&b, "services", "stop", "sketchybar"]));
            if remove_brew_sketchybar && brew.sketchybar_installed {
                out.push(s(&[&b, "uninstall", "sketchybar"]));
            }
        }
        if brew.borders_has_work() {
            out.push(s(&[&b, "services", "stop", "borders"]));
            if remove_brew_borders && brew.borders_installed {
                out.push(s(&[&b, "uninstall", "borders"]));
            }
        }
        if brew.mbar_installed {
            out.push(s(&[&b, "services", "stop", "mbar"]));
            out.push(s(&[&b, "uninstall", "mbar"]));
        }
    }
    for i in items.iter().filter(|i| i.kind == OldKind::LaunchAgent) {
        out.push(s(&[
            "/bin/launchctl",
            "bootout",
            &format!("gui/{uid}/dev.rubeen.mbar"),
        ]));
        out.push(s(&["/bin/rm", "-f", &i.path.to_string_lossy()]));
    }
    for i in items
        .iter()
        .filter(|i| i.kind == OldKind::Binary && !i.admin)
    {
        out.push(s(&["/bin/rm", "-f", &i.path.to_string_lossy()]));
    }
    out
}

/// `launchctl bootout` fails when the agent is not loaded, which is the goal anyway.
fn failure_tolerated(cmd: &[String]) -> bool {
    cmd.first().is_some_and(|p| p.ends_with("/launchctl"))
        && cmd.get(1).map(String::as_str) == Some("bootout")
}

/// Runs `cmds` in order and stops at the first failure; returns the combined output
/// (`Err` carries the output up to and including the failing command).
pub fn run_commands(cmds: &[Vec<String>]) -> Result<String, String> {
    let mut log = String::new();
    for c in cmds {
        let Some((prog, args)) = c.split_first() else {
            continue;
        };
        let line = c.join(" ");
        let out = Command::new(prog).args(args).output().map_err(|e| {
            log.push_str(&format!("$ {line}\n{e}\n"));
            log.clone()
        })?;
        log.push_str(&format!(
            "$ {line}\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
        if !out.status.success() && !failure_tolerated(c) {
            log.push_str(&format!("({line} exited with {})\n", out.status));
            return Err(log);
        }
    }
    Ok(log)
}

/// Runs `shell` as root after the standard macOS admin prompt (`osascript`).
pub fn run_admin(shell: &str) -> Result<(), String> {
    let out = Command::new("/usr/bin/osascript")
        .args(["-e", &applescript_admin(shell)])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
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
                ..BrewState::default()
            }
        );
        let s = parse_brew("mbar\n", "sketchybar none\n");
        assert_eq!(
            s,
            BrewState {
                mbar_installed: true,
                ..BrewState::default()
            }
        );
    }

    #[test]
    fn brew_parsing_borders() {
        // `brew list --formula` prints the short name of tap formulae.
        let s = parse_brew(
            "borders\nsketchybar\n",
            "Name       Status  User File\n\
             borders    started r    ~/Library/LaunchAgents/homebrew.mxcl.borders.plist\n\
             sketchybar none\n",
        );
        assert_eq!(
            s,
            BrewState {
                sketchybar_installed: true,
                borders_installed: true,
                borders_running: true,
                ..BrewState::default()
            }
        );
        assert!(s.borders_has_work());
        // Installed but stopped; and a service that still runs after the formula went.
        let s = parse_brew("borders\n", "borders none\n");
        assert!(s.borders_installed && !s.borders_running && s.borders_has_work());
        let s = parse_brew("", "borders started r x\n");
        assert!(!s.borders_installed && s.borders_running && s.borders_has_work());
        // Names that merely contain `borders` are something else.
        let s = parse_brew("borders-extra\n", "borders-extra started\n");
        assert_eq!(s, BrewState::default());
        assert!(!s.borders_has_work());
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
    #[test]
    fn cleanup_command_order() {
        let items = vec![
            OldInstall {
                path: "/h/Library/LaunchAgents/dev.rubeen.mbar.plist".into(),
                kind: OldKind::LaunchAgent,
                admin: false,
            },
            OldInstall {
                path: "/h/.local/bin/mbar".into(),
                kind: OldKind::Binary,
                admin: false,
            },
            OldInstall {
                path: "/h/.local/bin/sketchybar".into(),
                kind: OldKind::Foreign,
                admin: false,
            },
            OldInstall {
                path: "/usr/local/bin/mbar".into(),
                kind: OldKind::Binary,
                admin: true,
            },
        ];
        let brew = BrewState {
            sketchybar_installed: true,
            sketchybar_running: true,
            mbar_installed: true,
            borders_installed: true,
            borders_running: true,
        };
        let cmds = cleanup_commands(
            &items,
            &brew,
            Some(Path::new("/opt/homebrew/bin/brew")),
            501,
            true,
            true,
        );
        let s: Vec<String> = cmds.iter().map(|c| c.join(" ")).collect();
        assert_eq!(
            s,
            [
                "/opt/homebrew/bin/brew services stop sketchybar",
                "/opt/homebrew/bin/brew uninstall sketchybar",
                "/opt/homebrew/bin/brew services stop borders",
                "/opt/homebrew/bin/brew uninstall borders",
                "/opt/homebrew/bin/brew services stop mbar",
                "/opt/homebrew/bin/brew uninstall mbar",
                "/bin/launchctl bootout gui/501/dev.rubeen.mbar",
                "/bin/rm -f /h/Library/LaunchAgents/dev.rubeen.mbar.plist",
                "/bin/rm -f /h/.local/bin/mbar",
            ]
        );
        // Without consent the formula stays installed, only the service stops.
        let cmds = cleanup_commands(&[], &brew, Some(Path::new("/b/brew")), 501, false, true);
        assert_eq!(cmds[0].join(" "), "/b/brew services stop sketchybar");
        assert!(!cmds
            .iter()
            .any(|c| c.join(" ") == "/b/brew uninstall sketchybar"));
        // No brew binary: no brew commands at all.
        assert!(cleanup_commands(&[], &brew, None, 501, true, true).is_empty());
    }

    #[test]
    fn cleanup_commands_for_borders() {
        let brew_bin = Some(Path::new("/b/brew"));
        let run = |brew: &BrewState, remove: bool| -> Vec<String> {
            cleanup_commands(&[], brew, brew_bin, 501, true, remove)
                .iter()
                .map(|c| c.join(" "))
                .collect()
        };
        let installed = BrewState {
            borders_installed: true,
            ..BrewState::default()
        };
        // The service stops before the formula goes (stop is harmless when stopped).
        assert_eq!(
            run(&installed, true),
            ["/b/brew services stop borders", "/b/brew uninstall borders"]
        );
        // Switch off: only the service stops; the formula stays.
        assert_eq!(run(&installed, false), ["/b/brew services stop borders"]);
        // A service left running without the formula: stop only.
        let running = BrewState {
            borders_running: true,
            ..BrewState::default()
        };
        assert_eq!(run(&running, true), ["/b/brew services stop borders"]);
        assert!(run(&BrewState::default(), true).is_empty());
        // Homebrew always runs before the legacy agent and the binary removals.
        let items = vec![
            OldInstall {
                path: "/h/.local/bin/borders".into(),
                kind: OldKind::Binary,
                admin: false,
            },
            OldInstall {
                path: "/h/Library/LaunchAgents/dev.rubeen.mbar.plist".into(),
                kind: OldKind::LaunchAgent,
                admin: false,
            },
            OldInstall {
                path: "/usr/local/bin/borders".into(),
                kind: OldKind::Foreign,
                admin: true,
            },
        ];
        let s: Vec<String> = cleanup_commands(&items, &installed, brew_bin, 501, true, true)
            .iter()
            .map(|c| c.join(" "))
            .collect();
        assert_eq!(
            s,
            [
                "/b/brew services stop borders",
                "/b/brew uninstall borders",
                "/bin/launchctl bootout gui/501/dev.rubeen.mbar",
                "/bin/rm -f /h/Library/LaunchAgents/dev.rubeen.mbar.plist",
                "/bin/rm -f /h/.local/bin/borders",
            ]
        );
    }

    #[test]
    fn borders_links_and_binaries() {
        let root = tmp("borders-old");
        let home = root.join("home");
        let lb = home.join(".local/bin");
        std::fs::create_dir_all(&lb).unwrap();
        let ulb = root.join("ulb");
        std::fs::create_dir_all(&ulb).unwrap();
        // `make install BORDERS_LINK=1`: borders -> mbar (an old install).
        std::fs::write(lb.join("mbar"), "bin").unwrap();
        symlink("mbar", lb.join("borders")).unwrap();
        // Intel Homebrew's link or a self-built JankyBorders: not mbar, reported only.
        std::fs::write(ulb.join("borders"), "\u{7f}ELF").unwrap();
        let found = find_old_installs(&home, &ulb, Some(Path::new("/Applications/mbar.app")));
        let got: Vec<_> = found
            .iter()
            .map(|o| (o.path.clone(), o.kind.clone(), o.admin))
            .collect();
        assert!(got.contains(&(lb.join("borders"), OldKind::Binary, false)));
        assert!(got.contains(&(ulb.join("borders"), OldKind::Foreign, true)));
        // A foreign link (to a Homebrew Cellar) is reported, not removed.
        std::fs::remove_file(ulb.join("borders")).unwrap();
        symlink("../Cellar/borders/1.9.0/bin/borders", ulb.join("borders")).unwrap();
        let found = find_old_installs(&home, &ulb, None);
        assert!(found
            .iter()
            .any(|o| o.path == ulb.join("borders") && o.kind == OldKind::Foreign));
        // A `borders` link into the app is kept.
        let app = root.join("Applications/mbar.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/mbar"), "bin").unwrap();
        std::fs::remove_file(ulb.join("borders")).unwrap();
        symlink(app.join("Contents/MacOS/mbar"), ulb.join("borders")).unwrap();
        assert!(!find_old_installs(&home, &ulb, Some(&app))
            .iter()
            .any(|o| o.path == ulb.join("borders")));
        // A directory named `borders` is nothing.
        std::fs::remove_file(ulb.join("borders")).unwrap();
        std::fs::create_dir(ulb.join("borders")).unwrap();
        assert!(!find_old_installs(&home, &ulb, None)
            .iter()
            .any(|o| o.path == ulb.join("borders")));
    }

    #[test]
    fn bordersrc_lookup_order() {
        let home = tmp("bordersrc");
        assert_eq!(bordersrc(&home), None);
        std::fs::write(home.join(".bordersrc"), "borders width=5\n").unwrap();
        assert_eq!(bordersrc(&home), Some(home.join(".bordersrc")));
        // ~/.config/borders/bordersrc wins.
        std::fs::create_dir_all(home.join(".config/borders")).unwrap();
        std::fs::write(home.join(".config/borders/bordersrc"), "borders\n").unwrap();
        assert_eq!(
            bordersrc(&home),
            Some(home.join(".config/borders/bordersrc"))
        );
        // A directory is not a config file: falls through to ~/.bordersrc.
        let home = tmp("bordersrc-dir");
        std::fs::create_dir_all(home.join(".config/borders/bordersrc")).unwrap();
        assert_eq!(bordersrc(&home), None);
        std::fs::write(home.join(".bordersrc"), "").unwrap();
        assert_eq!(bordersrc(&home), Some(home.join(".bordersrc")));
    }

    #[test]
    fn launch_lines_in_aerospace_toml() {
        let toml = "\
# borders active_color=0xff00ff00
after-startup-command = [
  'exec-and-forget sketchybar',
  'exec-and-forget borders active_color=0xffe1e3e4 inactive_color=0xff494d64 width=5.0',
]
after-login-command = ['exec-and-forget /opt/homebrew/bin/borders width=6']
alt-b = 'exec-and-forget sketchybar --set borders label=x'
alt-c = \"exec-and-forget borders style=square\" # restyle
";
        let got = borders_launch_lines_in(toml);
        assert_eq!(
            got,
            vec![
                (
                    4,
                    "'exec-and-forget borders active_color=0xffe1e3e4 inactive_color=0xff494d64 width=5.0',"
                        .to_string()
                ),
                (
                    6,
                    "after-login-command = ['exec-and-forget /opt/homebrew/bin/borders width=6']"
                        .to_string()
                ),
                (
                    8,
                    "alt-c = \"exec-and-forget borders style=square\" # restyle".to_string()
                ),
            ]
        );
    }

    #[test]
    fn launch_lines_in_yabairc() {
        let rc = "\
#!/usr/bin/env sh
yabai -m config layout bsp
borders active_color=0xffe1e3e4 inactive_color=0xff494d64 width=5.0 &
  # borders width=8 &
sketchybar --add item borders left
sketchybar --set borders label=on # borders here
command -v borders >/dev/null 2>&1
pkill borders; killall borders
which borders && echo borders
[ -x /opt/homebrew/bin/borders ] && /opt/homebrew/bin/borders width=4 &
BORDERS_LOG=1 nohup borders hidpi=on > /dev/null 2>&1 &
(borders order=above &)
brew services start borders
brew services stop borders
yabai -m signal --add event=window_focused action=\"borders width=6\"
my-borders-wrapper width=4
";
        let lines: Vec<usize> = borders_launch_lines_in(rc).iter().map(|l| l.0).collect();
        assert_eq!(lines, vec![3, 10, 11, 12, 13, 15]);
        assert_eq!(
            borders_launch_lines_in(rc)[0].1,
            "borders active_color=0xffe1e3e4 inactive_color=0xff494d64 width=5.0 &"
        );
        assert!(borders_launch_lines_in("").is_empty());
        assert!(borders_launch_lines_in("bordersrc\nborders_x\n").is_empty());
        // `exec` with options, `env` with assignments, `command` without `-v`.
        assert_eq!(
            borders_launch_lines_in("exec -c borders\nenv -i A=1 borders\ncommand borders\n").len(),
            3
        );
    }

    #[test]
    fn launch_lines_scan_window_manager_configs() {
        let home = tmp("wm");
        std::fs::create_dir_all(home.join(".config/aerospace")).unwrap();
        std::fs::create_dir_all(home.join(".config/yabai")).unwrap();
        std::fs::write(
            home.join(".config/aerospace/aerospace.toml"),
            "after-startup-command = ['exec-and-forget borders']\n",
        )
        .unwrap();
        std::fs::write(home.join(".yabairc"), "yabai -m config layout bsp\n").unwrap();
        std::fs::write(
            home.join(".config/yabai/yabairc"),
            "#!/bin/sh\n\nborders width=5 &\n",
        )
        .unwrap();
        assert_eq!(
            borders_launch_lines(&home),
            vec![
                LaunchLine {
                    file: home.join(".config/aerospace/aerospace.toml"),
                    line: 1,
                    text: "after-startup-command = ['exec-and-forget borders']".into(),
                },
                LaunchLine {
                    file: home.join(".config/yabai/yabairc"),
                    line: 3,
                    text: "borders width=5 &".into(),
                },
            ]
        );
        assert!(borders_launch_lines(&tmp("wm-empty")).is_empty());
    }

    #[test]
    fn command_check_resolves_into_the_bundle() {
        let bin = Path::new("/Applications/mbar.app/Contents/Resources/bin");
        assert_eq!(
            check_command(
                bin,
                "/bin/zsh",
                "borders",
                "/Applications/mbar.app/Contents/Resources/bin/borders\n"
            ),
            Ok("borders → /Applications/mbar.app/Contents/Resources/bin/borders".into())
        );
        let shadowed =
            check_command(bin, "/bin/zsh", "borders", "/opt/homebrew/bin/borders\n").unwrap_err();
        assert!(shadowed.contains("finds borders at '/opt/homebrew/bin/borders'"));
        assert!(shadowed.contains("shadows mbar"));
        let missing = check_command(bin, "/bin/zsh", "borders", "").unwrap_err();
        assert!(missing.contains("does not find borders"));
        assert_eq!(CHECKED_COMMANDS, ["sketchybar", "borders"]);
    }

    #[test]
    fn borders_helper_detection() {
        let dir = tmp("felix-borders");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/client.c"),
            "bootstrap_look_up(bs_port, \"git.felix.borders\", &port);",
        )
        .unwrap();
        std::fs::write(dir.join("src/other.c"), "\"git.felix.sketchybar\"").unwrap();
        std::fs::write(dir.join("bordersrc"), "# git.felix.borders\n").unwrap();
        assert_eq!(borders_helpers(&dir), vec![dir.join("src/client.c")]);
        // The SketchyBar scan still finds every `git.felix.` user.
        assert_eq!(
            felix_helpers(&dir),
            vec![dir.join("src/client.c"), dir.join("src/other.c")]
        );
        assert!(borders_helpers(&dir.join("missing")).is_empty());
    }

    #[test]
    fn cleanup_runs_before_login_item() {
        // The legacy agent's bootout shares the bundled agent's label, so the first run
        // must clean up before it registers the login item.
        let pos = |s: SetupStep| SetupStep::ALL.iter().position(|x| *x == s).unwrap();
        assert!(pos(SetupStep::Cleanup) < pos(SetupStep::LoginItem));
        // And a later cleanup with a legacy agent tells the executor to re-register.
        let agent = OldInstall {
            path: "/h/Library/LaunchAgents/dev.rubeen.mbar.plist".into(),
            kind: OldKind::LaunchAgent,
            admin: false,
        };
        let bin = OldInstall {
            path: "/h/.local/bin/mbar".into(),
            kind: OldKind::Binary,
            admin: false,
        };
        assert!(cleanup_stops_login_item(&[bin.clone(), agent]));
        assert!(!cleanup_stops_login_item(&[bin]));
        assert!(!cleanup_stops_login_item(&[]));
    }

    #[test]
    fn run_commands_stops_at_first_failure() {
        let sh = |script: &str| vec!["/bin/sh".to_string(), "-c".into(), script.into()];
        assert_eq!(
            run_commands(&[sh("echo one"), sh("echo two >&2")]).unwrap(),
            "$ /bin/sh -c echo one\none\n$ /bin/sh -c echo two >&2\ntwo\n"
        );
        let err = run_commands(&[sh("echo a"), sh("exit 3"), sh("echo never")]).unwrap_err();
        assert!(err.contains("$ /bin/sh -c echo a\na\n"));
        assert!(err.contains("exit 3"));
        assert!(!err.contains("never"));
        assert!(run_commands(&[vec!["/definitely/not/a/binary".into()]]).is_err());
    }

    #[test]
    fn only_launchctl_bootout_failures_are_tolerated() {
        let v = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert!(failure_tolerated(&v(
            "/bin/launchctl bootout gui/501/dev.rubeen.mbar"
        )));
        assert!(!failure_tolerated(&v("/bin/launchctl kickstart -k x")));
        assert!(!failure_tolerated(&v("/bin/rm bootout")));
        assert!(!failure_tolerated(&[]));
    }
}

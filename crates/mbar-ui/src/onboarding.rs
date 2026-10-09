//! First-launch onboarding: what to clean up, what to convert, what to install. Pure
//! detection over the file system plus command/script builders; the GUI runs them.
//!
//! Covers the SketchyBar take-over (`docs/MIGRATING.md`) and the JankyBorders one
//! (`docs/superpowers/specs/2026-10-09-borders-design.md` §5): the Homebrew `borders`
//! formula and service, `borders` binaries and links, `bordersrc` (read in place,
//! `docs/spec/borders.md` §4) and window-manager lines that launch `borders`. For
//! AeroSpace (`docs/superpowers/specs/2026-10-09-aerospace-design.md`) it finds
//! `exec-on-workspace-change` / `on-*-changed` settings that run SketchyBar triggers
//! for the `aerospace_*` events mbar now delivers itself.

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
    /// A Homebrew link into its own formula (`/usr/local/bin/borders ->
    /// ../Cellar/borders/…` or `../opt/borders/…` on Intel Macs): the cleanup's
    /// `brew uninstall` removes it, otherwise it stays like `Foreign` (`removed_by_brew`).
    Homebrew,
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

/// Whether `target` lies in Homebrew's keg of `formula`: a `Cellar/<formula>` or
/// `opt/<formula>` pair of components (`../Cellar/borders/1.9.0/bin/borders`).
fn in_brew_keg(target: &Path, formula: &str) -> bool {
    let parts: Vec<_> = target.components().map(|c| c.as_os_str()).collect();
    parts
        .windows(2)
        .any(|w| (w[0] == "Cellar" || w[0] == "opt") && w[1] == formula)
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
        "sketchybar" | "borders" if target.as_deref().is_some_and(|t| in_brew_keg(t, name)) => {
            Some(OldKind::Homebrew)
        }
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
            if let Some(mut kind) = classify(&p, bundle_root) {
                // Homebrew links only its own prefix (`/usr/local/bin`); a link of yours
                // into a keg outlives `brew uninstall`.
                if kind == OldKind::Homebrew && !admin {
                    kind = OldKind::Foreign;
                }
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

/// Whether the cleanup's `brew uninstall` removes `o`: a `Homebrew` link whose formula
/// is installed and whose "Also uninstall Homebrew …" switch is on. Anything else that
/// is not mbar stays in place.
pub fn removed_by_brew(
    o: &OldInstall,
    brew: &BrewState,
    remove_brew_sketchybar: bool,
    remove_brew_borders: bool,
) -> bool {
    o.kind == OldKind::Homebrew
        && match o.path.file_name().and_then(|n| n.to_str()) {
            Some("sketchybar") => remove_brew_sketchybar && brew.sketchybar_installed,
            Some("borders") => remove_brew_borders && brew.borders_installed,
            _ => false,
        }
}

/// Whether the cleanup leaves `o` where it is (not mbar and not removed by
/// `brew uninstall`).
pub fn left_in_place(
    o: &OldInstall,
    brew: &BrewState,
    remove_brew_sketchybar: bool,
    remove_brew_borders: bool,
) -> bool {
    match o.kind {
        OldKind::Foreign => true,
        OldKind::Homebrew => !removed_by_brew(o, brew, remove_brew_sketchybar, remove_brew_borders),
        OldKind::Binary | OldKind::LaunchAgent => false,
    }
}

/// What the cleanup does with `o`, as the setup lists it.
pub fn old_install_fate(
    o: &OldInstall,
    brew: &BrewState,
    remove_brew_sketchybar: bool,
    remove_brew_borders: bool,
) -> &'static str {
    match o.kind {
        OldKind::Binary if o.admin => "removed with the commands step (admin)",
        OldKind::Binary => "removed",
        OldKind::LaunchAgent => "stopped and removed",
        _ if removed_by_brew(o, brew, remove_brew_sketchybar, remove_brew_borders) => {
            "removed by brew uninstall"
        }
        OldKind::Foreign | OldKind::Homebrew => "not mbar, left in place",
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

/// Where mbar.app's `bin` directory is when the running bundle is unknown.
pub const DEFAULT_BIN_DIR: &str = "/Applications/mbar.app/Contents/Resources/bin";

/// Shown after the launch lines and their advice.
pub const LAUNCH_LINE_DOCS: &str = "See docs/MIGRATING.md#migrating-from-jankyborders.";

/// How a window-manager line starts JankyBorders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    /// `borders …`; `args`: the line passes arguments (settings).
    Borders { args: bool },
    /// `brew services start|restart|run borders`.
    BrewService,
}

/// What the take-over step says about one launch line (`docs/MIGRATING.md`,
/// "Migrating from JankyBorders"). `bordersrc`: the config mbar runs in place, if any;
/// `bin_dir`: the running bundle's `bin` directory (`DEFAULT_BIN_DIR` when unknown).
///
/// Only a line that starts `borders` without settings, or Homebrew's service, while a
/// `bordersrc` exists is plain removal: mbar runs that `bordersrc` itself. Settings on
/// the line, or no `bordersrc` at all, have to move somewhere first.
pub fn launch_line_advice(
    launch: Launch,
    bordersrc: Option<&Path>,
    bin_dir: Option<&Path>,
) -> String {
    const CONFIGURE: &str =
        "put the settings in ~/.config/borders/bordersrc or in mbar.borders{…} in init.lua";
    let link = bin_dir
        .unwrap_or(Path::new(DEFAULT_BIN_DIR))
        .join("borders");
    match (launch, bordersrc) {
        (Launch::BrewService, Some(rc)) => format!(
            "Remove this line: it starts Homebrew's JankyBorders, which the cleanup \
             uninstalls, and mbar runs {} itself.",
            rc.display()
        ),
        (Launch::BrewService, None) => format!(
            "Remove this line: it starts Homebrew's JankyBorders, which the cleanup \
             uninstalls. mbar draws no borders until they are configured: {CONFIGURE}."
        ),
        (Launch::Borders { args: false }, Some(rc)) => format!(
            "Remove this line: mbar runs {} itself, and a bare `borders` now only reports \
             that borders are already running.",
            rc.display()
        ),
        (Launch::Borders { args: false }, None) => format!(
            "Without a bordersrc this line ran JankyBorders with its defaults; mbar draws \
             no borders until they are configured: {CONFIGURE}, then remove the line."
        ),
        (Launch::Borders { args: true }, rc) => {
            let mut s = format!(
                "This line passes settings. Keep them: move them into \
                 ~/.config/borders/bordersrc (or mbar.borders{{…}} in init.lua) and remove \
                 the line, or call mbar's link by its full path: {} … (a bare `borders` \
                 works only when the window manager's PATH contains the /etc/paths.d \
                 entries).",
                link.display()
            );
            if let Some(rc) = rc {
                s.push_str(&format!(
                    " mbar also runs {}, so keep each setting in one place.",
                    rc.display()
                ));
            }
            s
        }
    }
}

/// A line in a window-manager config that starts `borders`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchLine {
    pub file: PathBuf,
    /// 1-based.
    pub line: usize,
    /// The line as written, trimmed.
    pub text: String,
    pub launch: Launch,
}

/// Lines that start `borders` in the window-manager configs that exist under `home`.
pub fn borders_launch_lines(home: &Path) -> Vec<LaunchLine> {
    let mut out = Vec::new();
    for file in window_manager_configs(home) {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        let toml = file.extension().is_some_and(|e| e == "toml");
        for (line, text, launch) in borders_launch_lines_in(&content, toml) {
            out.push(LaunchLine {
                file: file.clone(),
                line,
                text,
                launch,
            });
        }
    }
    out
}

/// `(1-based line number, trimmed line, how)` of every line of a shell script
/// (`yabairc`) or, with `toml`, a TOML file (`aerospace.toml`) that runs `borders` as a
/// command: `borders k=v &`, `exec-and-forget borders …`, `/opt/homebrew/bin/borders …`,
/// `else borders …`, `sh -c 'borders …'`, `brew services start borders`. Comments and
/// `borders` as an argument (`sketchybar --set borders …`, `command -v borders`,
/// `pkill borders`, `echo "borders started"`) do not count.
pub fn borders_launch_lines_in(content: &str, toml: bool) -> Vec<(usize, String, Launch)> {
    content
        .lines()
        .enumerate()
        .filter_map(|(i, l)| line_launch(l, toml).map(|how| (i + 1, l.trim().to_string(), how)))
        .collect()
}

/// Splits `line` into simple commands (words, quotes removed) and returns the first
/// that starts `borders`. A quoted string is a command of its own only where a command
/// string starts: after `exec-and-forget`, a shell's `-c`, yabai's `action=` and, in
/// TOML, as a value or an array element. Anywhere else it is part of a word, i.e. an
/// argument (`echo "borders started"`).
fn line_launch(line: &str, toml: bool) -> Option<Launch> {
    let mut seg: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let end_word = |seg: &mut Vec<String>, word: &mut String, in_word: &mut bool| {
        if std::mem::take(in_word) {
            seg.push(std::mem::take(word));
        }
    };
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        let separator = match c {
            ';' | '&' | '|' | '(' | ')' | '`' => true,
            '[' | ']' | ',' => toml,
            // `{ borders & }`; `${HOME}` stays one word.
            '{' | '}' => !in_word,
            _ => false,
        };
        if separator {
            end_word(&mut seg, &mut word, &mut in_word);
            if let Some(how) = segment_launch(&seg) {
                return Some(how);
            }
            seg.clear();
            continue;
        }
        match c {
            '\'' | '"' => {
                let mut quoted = String::new();
                while let Some(d) = chars.next() {
                    match d {
                        _ if d == c => break,
                        '\\' if c == '"' => quoted.extend(chars.next()),
                        _ => quoted.push(d),
                    }
                }
                let at_word = in_word.then_some(word.as_str());
                if starts_command_string(&seg, at_word, toml) {
                    if let Some(how) = line_launch(&quoted, false) {
                        return Some(how);
                    }
                }
                word.push_str(&quoted);
                in_word = true;
            }
            '#' if !in_word => break,
            '\\' => {
                word.extend(chars.next());
                in_word = true;
            }
            c if c.is_whitespace() => end_word(&mut seg, &mut word, &mut in_word),
            c => {
                word.push(c);
                in_word = true;
            }
        }
    }
    end_word(&mut seg, &mut word, &mut in_word);
    segment_launch(&seg)
}

/// Whether a quote that follows the words `seg` (and the unfinished `word`) opens a
/// command string.
fn starts_command_string(seg: &[String], word: Option<&str>, toml: bool) -> bool {
    let is_shell = |w: &str| {
        let name = w.rsplit('/').next().unwrap_or(w);
        matches!(name, "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish")
    };
    // `-c`, `-lc`, `-ic` after a shell.
    let is_dash_c = |w: &str| {
        w.strip_prefix('-')
            .is_some_and(|o| o.ends_with('c') && o.chars().all(|c| c.is_ascii_alphabetic()))
    };
    match word {
        // yabai signals run `action="…"` with `sh -c`.
        Some("action=") => true,
        Some(w) => toml && toml_value_start(seg, w),
        None => match seg {
            // A TOML array element or a string at the start of a line.
            [] => toml,
            [.., last] if last == "exec-and-forget" => true,
            [before @ .., last] if is_dash_c(last) && before.iter().any(|w| is_shell(w)) => true,
            _ => toml && toml_value_start(seg, ""),
        },
    }
}

/// `key =` (or `key=`) before a TOML string value.
fn toml_value_start(seg: &[String], word: &str) -> bool {
    let mut prefix = seg.join(" ");
    prefix.push(' ');
    prefix.push_str(word);
    prefix.trim().strip_suffix('=').is_some_and(|key| {
        let key = key.trim();
        !key.is_empty() && !key.contains(char::is_whitespace)
    })
}

/// Shell words and launchers in front of the command word.
const PREFIX_KEYWORDS: [&str; 9] = [
    "if",
    "then",
    "else",
    "elif",
    "do",
    "while",
    "until",
    "!",
    "exec-and-forget",
];

/// How one simple command starts `borders`: its command word (after shell keywords,
/// launchers with their options and environment assignments) is `borders`, or it is
/// `brew services start|restart|run borders`.
fn segment_launch(words: &[String]) -> Option<Launch> {
    let is_assignment = |w: &str| {
        w.find('=').is_some_and(|p| {
            p > 0
                && w[..p]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    };
    // Launcher options that take a value (`sudo -u root`, `nice -n 5`, `exec -a name`).
    let takes_value = |launcher: &str, opt: &str| {
        matches!(
            (launcher, opt),
            (
                "sudo",
                "-u" | "-g" | "-p" | "-C" | "-h" | "-U" | "-r" | "-t" | "-D" | "-R" | "-T"
            ) | ("env", "-u" | "-C")
                | ("exec", "-a")
                | ("nice", "-n")
        )
    };
    let mut i = 0;
    loop {
        let w = words.get(i)?.as_str();
        match w {
            _ if PREFIX_KEYWORDS.contains(&w) || is_assignment(w) => i += 1,
            "time" | "nohup" | "nice" | "sudo" | "env" | "exec" | "command" => {
                i += 1;
                while let Some(opt) = words.get(i).filter(|o| o.len() > 1 && o.starts_with('-')) {
                    // `command -v borders` only looks it up.
                    if w == "command" && (opt == "-v" || opt == "-V") {
                        return None;
                    }
                    i += if takes_value(w, opt) { 2 } else { 1 };
                }
            }
            _ if w == "brew" || w.ends_with("/brew") => {
                return match words.get(i + 1..i + 4)? {
                    [a, b, c]
                        if a == "services"
                            && matches!(b.as_str(), "start" | "restart" | "run")
                            && c == "borders" =>
                    {
                        Some(Launch::BrewService)
                    }
                    _ => None,
                };
            }
            _ if w == "borders" || w.ends_with("/borders") => {
                return Some(Launch::Borders {
                    args: has_arguments(&words[i + 1..]),
                });
            }
            _ => return None,
        }
    }
}

/// Whether `words` hold anything but redirections (`>/dev/null`, `2>`, `> log`).
fn has_arguments(words: &[String]) -> bool {
    let mut it = words.iter();
    while let Some(w) = it.next() {
        let op = w.trim_start_matches(|c: char| c.is_ascii_digit());
        if !op.starts_with(['>', '<']) {
            return true;
        }
        // A bare operator: the target is the next word.
        if op.trim_start_matches(['>', '<', '&', '|']).is_empty() {
            it.next();
        }
    }
    false
}

// ---------------------------------------------------------------------------
// AeroSpace (`docs/superpowers/specs/2026-10-09-aerospace-design.md`, "mbar.app")
// ---------------------------------------------------------------------------
/// AeroSpace's config files: `~/.aerospace.toml`, `$XDG_CONFIG_HOME/aerospace/aerospace.toml`
/// (`xdg` is `$XDG_CONFIG_HOME`; empty or relative means unset) and
/// `~/.config/aerospace/aerospace.toml`. The default location stays in the list when
/// `XDG_CONFIG_HOME` is set: mbar.app and AeroSpace.app may see different environments.
pub fn aerospace_configs(home: &Path, xdg: &str) -> Vec<PathBuf> {
    let mut out = vec![home.join(".aerospace.toml")];
    if Path::new(xdg).is_absolute() {
        out.push(Path::new(xdg).join("aerospace/aerospace.toml"));
    }
    let default = home.join(".config/aerospace/aerospace.toml");
    if !out.contains(&default) {
        out.push(default);
    }
    out
}

/// Whether AeroSpace looks installed: one of its configs exists, or `AeroSpace.app`
/// does. mbar.app asks the daemon for `--query aerospace` only then.
pub fn aerospace_installed(home: &Path, xdg: &str) -> bool {
    aerospace_configs(home, xdg).iter().any(|p| p.is_file())
        || Path::new("/Applications/AeroSpace.app").exists()
        || home.join("Applications/AeroSpace.app").exists()
}

/// Shown after the trigger lines and their advice.
pub const AEROSPACE_TRIGGER_DOCS: &str = "See docs/MIGRATING.md#using-aerospace.";

/// The AeroSpace settings mbar replaces, and whether their array elements are separate
/// commands (AeroSpace commands such as `exec-and-forget …`) rather than one argv
/// (`exec-on-workspace-change`).
const AEROSPACE_SETTINGS: [(&str, bool); 4] = [
    ("exec-on-workspace-change", false),
    ("on-focus-changed", true),
    ("on-focused-monitor-changed", true),
    ("on-mode-changed", true),
];

/// mbar's built-in AeroSpace events and the variables mbar sets for them (besides
/// `NAME`, `SENDER` and `INFO`); design doc, "Events".
const AEROSPACE_EVENTS: [(&str, &[&str]); 6] = [
    (
        "aerospace_workspace_change",
        &[
            "FOCUSED_WORKSPACE",
            "PREV_WORKSPACE",
            "AEROSPACE_FOCUSED_WORKSPACE",
            "AEROSPACE_PREV_WORKSPACE",
        ],
    ),
    (
        "aerospace_focus_change",
        &["FOCUSED_WORKSPACE", "WINDOW_ID"],
    ),
    (
        "aerospace_monitor_change",
        &["FOCUSED_WORKSPACE", "MONITOR_ID"],
    ),
    ("aerospace_mode_change", &["MODE"]),
    (
        "aerospace_window_detected",
        &["WINDOW_ID", "WORKSPACE", "APP_BUNDLE_ID", "APP_NAME"],
    ),
    ("aerospace_binding_triggered", &["MODE", "BINDING"]),
];

/// The variables mbar sets for `event`, when it is one of its AeroSpace events.
fn aerospace_event_vars(event: &str) -> Option<&'static [&'static str]> {
    AEROSPACE_EVENTS
        .iter()
        .find(|(e, _)| *e == event)
        .map(|(_, vars)| *vars)
}

/// A `sketchybar --trigger aerospace_…` (or `mbar --trigger …`) in one of the AeroSpace
/// settings whose events mbar now receives itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AerospaceTrigger {
    pub file: PathBuf,
    /// 1-based line with the event name.
    pub line: usize,
    /// The setting's first line (the key) and last line (a TOML array may span lines).
    pub first_line: usize,
    pub last_line: usize,
    /// The trigger line as written, trimmed.
    pub text: String,
    /// The setting's key, e.g. `exec-on-workspace-change`.
    pub setting: String,
    /// The triggered event, e.g. `aerospace_workspace_change`.
    pub event: String,
    /// Variables the trigger passes that mbar's own event does not set.
    pub extra_vars: Vec<String>,
    /// The setting runs nothing but triggers mbar replaces, so all of it can go; else
    /// only the trigger command.
    pub whole_setting: bool,
}

impl AerospaceTrigger {
    /// Removing the trigger loses nothing: mbar's event carries all its variables.
    pub fn removable(&self) -> bool {
        self.extra_vars.is_empty()
    }

    /// The advice for the take-over step.
    pub fn advice(&self) -> String {
        if !self.removable() {
            let sent = if self.event == "aerospace_workspace_change" {
                "FOCUSED_WORKSPACE/PREV_WORKSPACE (and AEROSPACE_ aliases)".to_string()
            } else {
                aerospace_event_vars(&self.event)
                    .unwrap_or_default()
                    .join("/")
            };
            return format!(
                "mbar sends this event itself, but only with {sent}; your scripts also use \
                 {} — keep the line or change the scripts.",
                self.extra_vars.join(", ")
            );
        }
        let what = if !self.whole_setting {
            format!(
                "remove only the `--trigger {}` command and keep the rest of this setting \
                 (it runs other commands too)",
                self.event
            )
        } else if self.first_line == self.last_line {
            "remove this line".to_string()
        } else {
            format!(
                "remove this setting (lines {}–{})",
                self.first_line, self.last_line
            )
        };
        let older = match self.event.as_str() {
            "aerospace_mode_change" => "an older mode",
            "aerospace_window_detected" | "aerospace_binding_triggered" => "older values",
            _ => "an older workspace",
        };
        format!(
            "mbar sends {} itself; {what}. The forked trigger arrives 10–50 ms after mbar's \
             own event and can overwrite it with {older}.",
            self.event
        )
    }
}

/// The SketchyBar triggers for mbar's AeroSpace events in the AeroSpace configs that
/// exist under `home` (and `xdg`, see [`aerospace_configs`]).
pub fn aerospace_triggers(home: &Path, xdg: &str) -> Vec<AerospaceTrigger> {
    let mut out = Vec::new();
    for file in aerospace_configs(home, xdg) {
        if let Ok(content) = std::fs::read_to_string(&file) {
            out.extend(aerospace_triggers_in(&file, &content));
        }
    }
    out
}

/// The triggers of one `aerospace.toml` (`file` is only copied into the result):
/// `sketchybar --trigger <event>` or `mbar --trigger <event>` for one of mbar's six
/// AeroSpace events, run from `exec-on-workspace-change`, `on-focus-changed`,
/// `on-focused-monitor-changed` or `on-mode-changed`, directly, through `sh -c` or
/// `exec-and-forget`. The value may span several lines; comments and other keys do not
/// count.
pub fn aerospace_triggers_in(file: &Path, content: &str) -> Vec<AerospaceTrigger> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((setting, elements, value)) = aerospace_setting(lines[i]) else {
            i += 1;
            continue;
        };
        let (tokens, last) = value_tokens(&lines, i, value);
        let mut found = Vec::new();
        let mut other = false;
        for cmd in split_commands(&tokens, elements) {
            let (triggers, runs_more) = parse_trigger_command(&cmd);
            found.extend(triggers);
            other |= runs_more;
        }
        let whole_setting = !other && found.iter().all(|t| t.extra_vars.is_empty());
        for t in found {
            out.push(AerospaceTrigger {
                file: file.to_path_buf(),
                line: t.line + 1,
                first_line: i + 1,
                last_line: last + 1,
                text: lines[t.line].trim().to_string(),
                setting: setting.to_string(),
                event: t.event,
                extra_vars: t.extra_vars,
                whole_setting,
            });
        }
        i = last + 1;
    }
    out
}

/// The setting `line` starts (the key may be quoted): its key, whether its array
/// elements are separate commands, and the rest of the line after `=`.
fn aerospace_setting(line: &str) -> Option<(&'static str, bool, &str)> {
    let t = line.trim_start();
    AEROSPACE_SETTINGS.iter().find_map(|&(key, elements)| {
        let rest = [key.to_string(), format!("\"{key}\""), format!("'{key}'")]
            .iter()
            .find_map(|k| t.strip_prefix(k.as_str()))?;
        Some((key, elements, rest.trim_start().strip_prefix('=')?))
    })
}

/// One trigger found in a command.
struct FoundTrigger {
    /// 0-based line of the event name.
    line: usize,
    event: String,
    extra_vars: Vec<String>,
}

/// `NAME` of a `NAME=value` word.
fn assignment_name(word: &str) -> Option<&str> {
    let (name, _) = word.split_once('=')?;
    let mut chars = name.chars();
    let first = chars.next()?;
    ((first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then_some(name)
}

/// The command words after wrappers that only start it: `exec-and-forget`, `exec`,
/// `nohup`, `env` and shells with their options (`/bin/bash -c`), and leading
/// `NAME=value` assignments.
fn strip_wrappers(cmd: &[(String, usize)]) -> &[(String, usize)] {
    let mut i = 0;
    let mut options = false;
    while let Some((w, _)) = cmd.get(i) {
        match w.rsplit('/').next().unwrap_or("") {
            "exec-and-forget" | "exec" | "nohup" => options = false,
            "env" | "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" => options = true,
            _ if options && w.starts_with('-') => {}
            _ if assignment_name(w).is_some() => {}
            _ => break,
        }
        i += 1;
    }
    &cmd[i..]
}

/// The AeroSpace triggers of one command, and whether the command does anything else
/// (another program, another `sketchybar` command such as `--set`, another event).
fn parse_trigger_command(cmd: &[(String, usize)]) -> (Vec<FoundTrigger>, bool) {
    let cmd = strip_wrappers(cmd);
    let Some((first, _)) = cmd.first() else {
        return (Vec::new(), false);
    };
    let program = first.rsplit('/').next().unwrap_or("");
    if !matches!(program, "sketchybar" | "mbar") {
        return (Vec::new(), true);
    }
    let mut found = Vec::new();
    // `sketchybar` alone starts the bar; words before the first `--` flag are not a
    // trigger either.
    let mut other = cmd.len() == 1 || !cmd[1].0.starts_with("--");
    let mut i = 1;
    while i < cmd.len() {
        let event_vars = (cmd[i].0 == "--trigger")
            .then(|| cmd.get(i + 1))
            .flatten()
            .and_then(|(e, line)| Some((e, *line, aerospace_event_vars(e)?)));
        let Some((event, line, sent)) = event_vars else {
            other |= cmd[i].0.starts_with("--");
            i += 1;
            continue;
        };
        let mut extra_vars: Vec<String> = Vec::new();
        i += 2;
        while let Some((w, _)) = cmd.get(i).filter(|(w, _)| !w.starts_with("--")) {
            if let Some(name) = assignment_name(w) {
                if !sent.contains(&name) && !extra_vars.iter().any(|v| v == name) {
                    extra_vars.push(name.to_string());
                }
            }
            i += 1;
        }
        found.push(FoundTrigger {
            line,
            event: event.clone(),
            extra_vars,
        });
    }
    (found, other)
}

/// The commands of a setting's value: words split at shell separators, and at array
/// elements when `elements` (AeroSpace command lists). Empty commands are dropped.
fn split_commands(tokens: &[Tok], elements: bool) -> Vec<Vec<(String, usize)>> {
    let mut out = vec![Vec::new()];
    for t in tokens {
        match t {
            Tok::Word(w, line) => out.last_mut().unwrap().push((w.clone(), *line)),
            Tok::Sep => out.push(Vec::new()),
            Tok::Elem if elements => out.push(Vec::new()),
            Tok::Elem => {}
        }
    }
    out.retain(|c| !c.is_empty());
    out
}

/// A token of a TOML value scanned as shell text.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// A shell word and its 0-based line.
    Word(String, usize),
    /// Between two shell commands: `;`, `&`, `|`, `(`, `)` or a line break.
    Sep,
    /// The end of a string that is an array element.
    Elem,
}

/// Inside a string: a shell construct whose text stays in the current word.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Keep {
    None,
    /// `$(…)` with its open parentheses.
    Subst(usize),
    /// `` `…` ``.
    Backtick,
    /// A quoted value after `NAME=` (`FOCUSED_WORKSPACE="$X"`), up to this quote.
    Quote(char),
}

/// Splits the text of TOML strings into shell words and separators.
struct ShellWords {
    tokens: Vec<Tok>,
    word: String,
    line: usize,
    keep: Keep,
    prev: Option<char>,
}

impl ShellWords {
    fn flush(&mut self) {
        if !self.word.is_empty() {
            self.tokens
                .push(Tok::Word(std::mem::take(&mut self.word), self.line));
        }
    }

    fn sep(&mut self) {
        self.flush();
        self.tokens.push(Tok::Sep);
    }

    /// A string starts or ends: quotes and substitutions do not cross it.
    fn reset(&mut self) {
        self.flush();
        self.keep = Keep::None;
        self.prev = None;
    }

    /// One character of shell text; `next` is the one after it, if known.
    fn push(&mut self, c: char, next: Option<char>) {
        let prev = self.prev.replace(c);
        match self.keep {
            Keep::Subst(depth) => {
                self.word.push(c);
                self.keep = match c {
                    '(' => Keep::Subst(depth + 1),
                    ')' if depth <= 1 => Keep::None,
                    ')' => Keep::Subst(depth - 1),
                    _ => Keep::Subst(depth),
                };
                return;
            }
            Keep::Backtick => {
                self.word.push(c);
                if c == '`' {
                    self.keep = Keep::None;
                }
                return;
            }
            Keep::Quote(q) => {
                if c == q {
                    self.keep = Keep::None;
                } else {
                    self.word.push(c);
                }
                return;
            }
            Keep::None => {}
        }
        match c {
            '$' if next == Some('(') => {
                self.word.push(c);
                self.keep = Keep::Subst(0);
            }
            '`' => {
                self.word.push(c);
                self.keep = Keep::Backtick;
            }
            '"' | '\'' if self.word.ends_with('=') => self.keep = Keep::Quote(c),
            // `sh -c "…"`: the quoted script is scanned as words.
            '"' | '\'' => self.flush(),
            // A redirection (`2>&1`, `&>/dev/null`), not a background `&`.
            '&' if matches!(prev, Some('<' | '>')) || next == Some('>') => self.word.push(c),
            ';' | '&' | '|' | '(' | ')' | '\n' => self.sep(),
            c if c.is_whitespace() => self.flush(),
            _ => self.word.push(c),
        }
    }
}

/// The tokens of a TOML value that starts with `value` on line `start` (0-based), and
/// the value's last line. Strings are split into shell words (`'sh -c "sketchybar
/// --trigger x; y"'` gives `sh`, `-c`, `sketchybar`, `--trigger`, `x`, a separator and
/// `y`); the end of a string inside an array is an element boundary.
fn value_tokens(lines: &[&str], start: usize, value: &str) -> (Vec<Tok>, usize) {
    #[derive(Clone, Copy, PartialEq)]
    enum Str {
        Basic,
        Literal,
        MultiBasic,
        MultiLiteral,
    }
    let mut sh = ShellWords {
        tokens: Vec::new(),
        word: String::new(),
        line: start,
        keep: Keep::None,
        prev: None,
    };
    let mut depth = 0usize;
    let mut string: Option<Str> = None;
    let mut idx = start;
    let mut text = value;
    loop {
        sh.line = idx;
        let chars: Vec<char> = text.chars().collect();
        let mut j = 0;
        let mut done = false;
        // A backslash ends the line inside a multi-line string: no line break.
        let mut continued = false;
        while j < chars.len() {
            let c = chars[j];
            let next = chars.get(j + 1).copied();
            let triple = |q: char| {
                chars
                    .get(j..j + 3)
                    .is_some_and(|s| s.iter().all(|&x| x == q))
            };
            let Some(kind) = string else {
                match c {
                    '#' => break,
                    '[' => depth += 1,
                    ']' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            done = true;
                            break;
                        }
                    }
                    '"' | '\'' => {
                        let multi = triple(c);
                        string = Some(match (c, multi) {
                            ('"', false) => Str::Basic,
                            ('"', true) => Str::MultiBasic,
                            (_, false) => Str::Literal,
                            (_, true) => Str::MultiLiteral,
                        });
                        sh.reset();
                        j += if multi { 3 } else { 1 };
                        continue;
                    }
                    c if c.is_whitespace() || c == ',' => {}
                    _ if depth == 0 => {
                        // Not an array or a string: nothing to scan.
                        done = true;
                        break;
                    }
                    _ => {}
                }
                j += 1;
                continue;
            };
            let (quote, multi) = match kind {
                Str::Basic => ('"', false),
                Str::Literal => ('\'', false),
                Str::MultiBasic => ('"', true),
                Str::MultiLiteral => ('\'', true),
            };
            if c == '\\' && multi && chars[j + 1..].iter().all(|c| c.is_whitespace()) {
                // TOML's line-ending backslash, or the shell's line continuation.
                continued = true;
                break;
            }
            if c == '\\' && matches!(kind, Str::Basic | Str::MultiBasic) {
                // A TOML escape: the escaped character is shell text (`\"` a quote).
                let e = match next {
                    Some('n') => '\n',
                    Some('t') => '\t',
                    Some(e) => e,
                    None => ' ',
                };
                sh.push(e, chars.get(j + 2).copied());
                j += 2;
                continue;
            }
            if c == quote && (!multi || triple(quote)) {
                sh.reset();
                string = None;
                j += if multi { 3 } else { 1 };
                if depth == 0 {
                    done = true;
                    break;
                }
                sh.tokens.push(Tok::Elem);
                continue;
            }
            sh.push(c, next);
            j += 1;
        }
        match string {
            // A single-line string ends with its line.
            Some(Str::Basic | Str::Literal) => {
                sh.reset();
                string = None;
                if depth == 0 {
                    done = true;
                }
            }
            Some(Str::MultiBasic | Str::MultiLiteral) if !continued => sh.push('\n', None),
            _ => {}
        }
        if done || (depth == 0 && string.is_none()) || idx + 1 >= lines.len() {
            sh.flush();
            return (sh.tokens, idx);
        }
        // A word does not continue on the next line (a continued line ends with `\`).
        sh.flush();
        idx += 1;
        text = lines[idx];
    }
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

/// `brew list --formula` and `brew services list` through `brew_bin`, parsed; the
/// default state without Homebrew.
pub fn detect_brew(brew_bin: Option<&Path>) -> BrewState {
    let Some(b) = brew_bin else {
        return BrewState::default();
    };
    let out = |args: &[&str]| {
        Command::new(b)
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    parse_brew(&out(&["list", "--formula"]), &out(&["services", "list"]))
}

/// The JankyBorders part of the cleanup: stop the `borders` service (harmless when
/// stopped), then, with `uninstall` and the formula installed, `brew uninstall
/// borders`. Shared by the setup and the System page's "Stop and remove JankyBorders".
pub fn borders_cleanup_commands(
    brew: &BrewState,
    brew_bin: Option<&Path>,
    uninstall: bool,
) -> Vec<Vec<String>> {
    let Some(b) = brew_bin.map(|p| p.to_string_lossy().into_owned()) else {
        return Vec::new();
    };
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut out = Vec::new();
    if brew.borders_has_work() {
        out.push(s(&[&b, "services", "stop", "borders"]));
        if uninstall && brew.borders_installed {
            out.push(s(&[&b, "uninstall", "borders"]));
        }
    }
    out
}

/// The System page's warning while Homebrew JankyBorders is still around (it draws
/// a second set of borders over mbar's); `None` when it is gone.
pub fn brew_borders_warning(brew: &BrewState) -> Option<String> {
    let what = match (brew.borders_installed, brew.borders_running) {
        (true, true) => "installed and running",
        (true, false) => "installed",
        (false, true) => "running",
        (false, false) => return None,
    };
    Some(format!(
        "JankyBorders from Homebrew is still {what}; it draws borders too."
    ))
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
        out.extend(borders_cleanup_commands(
            brew,
            Some(Path::new(&b)),
            remove_brew_borders,
        ));
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
    fn borders_cleanup_from_the_system_page() {
        let brew_bin = Some(Path::new("/usr/local/bin/brew"));
        let run = |brew: &BrewState, bin: Option<&Path>| -> Vec<String> {
            borders_cleanup_commands(brew, bin, true)
                .iter()
                .map(|c| c.join(" "))
                .collect()
        };
        let both = BrewState {
            borders_installed: true,
            borders_running: true,
            ..BrewState::default()
        };
        assert_eq!(
            run(&both, brew_bin),
            [
                "/usr/local/bin/brew services stop borders",
                "/usr/local/bin/brew uninstall borders"
            ]
        );
        let running = BrewState {
            borders_running: true,
            ..BrewState::default()
        };
        assert_eq!(
            run(&running, brew_bin),
            ["/usr/local/bin/brew services stop borders"]
        );
        assert!(run(&BrewState::default(), brew_bin).is_empty());
        assert!(run(&both, None).is_empty());
        // SketchyBar and mbar formulae are not this button's business.
        let others = BrewState {
            sketchybar_installed: true,
            sketchybar_running: true,
            mbar_installed: true,
            ..BrewState::default()
        };
        assert!(run(&others, brew_bin).is_empty());
        // Without Homebrew there is nothing to detect.
        assert_eq!(detect_brew(None), BrewState::default());
    }

    #[test]
    fn brew_borders_warning_text() {
        let state = |installed, running| BrewState {
            borders_installed: installed,
            borders_running: running,
            ..BrewState::default()
        };
        assert_eq!(brew_borders_warning(&state(false, false)), None);
        assert_eq!(
            brew_borders_warning(&state(true, true)).unwrap(),
            "JankyBorders from Homebrew is still installed and running; it draws borders too."
        );
        assert!(brew_borders_warning(&state(true, false))
            .unwrap()
            .contains("still installed;"));
        assert!(brew_borders_warning(&state(false, true))
            .unwrap()
            .contains("still running;"));
    }

    #[test]
    fn homebrew_links_go_with_brew_uninstall() {
        let link = |name: &str| OldInstall {
            path: PathBuf::from("/usr/local/bin").join(name),
            kind: OldKind::Homebrew,
            admin: true,
        };
        let installed = BrewState {
            sketchybar_installed: true,
            borders_installed: true,
            ..BrewState::default()
        };
        let b = link("borders");
        let sb = link("sketchybar");
        assert!(removed_by_brew(&b, &installed, false, true));
        assert!(!removed_by_brew(&b, &installed, true, false));
        assert!(removed_by_brew(&sb, &installed, true, false));
        assert!(!removed_by_brew(&sb, &installed, false, true));
        // Not installed (only a stale link): `brew uninstall` does not run.
        assert!(!removed_by_brew(&b, &BrewState::default(), true, true));
        assert_eq!(
            old_install_fate(&b, &installed, true, true),
            "removed by brew uninstall"
        );
        assert!(!left_in_place(&b, &installed, true, true));
        assert_eq!(
            old_install_fate(&b, &installed, true, false),
            "not mbar, left in place"
        );
        assert!(left_in_place(&b, &installed, true, false));
        // Foreign entries stay whatever the switches say.
        let foreign = OldInstall {
            kind: OldKind::Foreign,
            ..link("borders")
        };
        assert!(!removed_by_brew(&foreign, &installed, true, true));
        assert!(left_in_place(&foreign, &installed, true, true));
        assert_eq!(
            old_install_fate(&foreign, &installed, true, true),
            "not mbar, left in place"
        );
        // mbar's own leftovers are never "left in place".
        let bin = OldInstall {
            kind: OldKind::Binary,
            ..link("mbar")
        };
        assert!(!left_in_place(&bin, &installed, false, false));
        assert_eq!(
            old_install_fate(&bin, &installed, false, false),
            "removed with the commands step (admin)"
        );
        // The cleanup itself never `rm`s a Homebrew link: `brew uninstall` does.
        let cmds = cleanup_commands(
            &[link("borders")],
            &installed,
            Some(Path::new("/usr/local/bin/brew")),
            501,
            true,
            true,
        );
        assert!(!cmds.iter().any(|c| c[0] == "/bin/rm"));
    }

    #[test]
    fn intel_homebrew_links_are_classified() {
        let root = tmp("intel-brew");
        let home = root.join("home");
        let ulb = root.join("usr/local/bin");
        std::fs::create_dir_all(&ulb).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        symlink("../Cellar/borders/1.9.0/bin/borders", ulb.join("borders")).unwrap();
        symlink("../opt/sketchybar/bin/sketchybar", ulb.join("sketchybar")).unwrap();
        let found = find_old_installs(&home, &ulb, None);
        let kinds: Vec<_> = found
            .iter()
            .map(|o| (o.path.clone(), o.kind.clone()))
            .collect();
        assert_eq!(
            kinds,
            [
                (ulb.join("sketchybar"), OldKind::Homebrew),
                (ulb.join("borders"), OldKind::Homebrew),
            ]
        );
        // The same link in ~/.local/bin is yours: `brew uninstall` leaves it dangling.
        let lb = home.join(".local/bin");
        std::fs::create_dir_all(&lb).unwrap();
        symlink("/usr/local/opt/borders/bin/borders", lb.join("borders")).unwrap();
        assert!(find_old_installs(&home, &ulb, None)
            .iter()
            .any(|o| o.path == lb.join("borders") && o.kind == OldKind::Foreign));
        assert!(in_brew_keg(
            Path::new("/usr/local/Cellar/borders/1.9.0/bin/borders"),
            "borders"
        ));
        assert!(!in_brew_keg(
            Path::new("/usr/local/Cellar/sketchybar/2.0/bin/borders"),
            "borders"
        ));
        assert!(!in_brew_keg(
            Path::new("/opt/homebrew/bin/borders"),
            "borders"
        ));
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
        // Intel Homebrew's link into the Cellar: `brew uninstall borders` removes it.
        std::fs::remove_file(ulb.join("borders")).unwrap();
        symlink("../Cellar/borders/1.9.0/bin/borders", ulb.join("borders")).unwrap();
        let found = find_old_installs(&home, &ulb, None);
        assert!(found
            .iter()
            .any(|o| o.path == ulb.join("borders") && o.kind == OldKind::Homebrew));
        // A link into another formula's keg is somebody else's: reported only.
        std::fs::remove_file(ulb.join("borders")).unwrap();
        symlink("../Cellar/other/1.0/bin/borders", ulb.join("borders")).unwrap();
        assert!(find_old_installs(&home, &ulb, None)
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
        let got = borders_launch_lines_in(toml, true);
        let args = Launch::Borders { args: true };
        assert_eq!(
            got,
            vec![
                (
                    4,
                    "'exec-and-forget borders active_color=0xffe1e3e4 inactive_color=0xff494d64 width=5.0',"
                        .to_string(),
                    args
                ),
                (
                    6,
                    "after-login-command = ['exec-and-forget /opt/homebrew/bin/borders width=6']"
                        .to_string(),
                    args
                ),
                (
                    8,
                    "alt-c = \"exec-and-forget borders style=square\" # restyle".to_string(),
                    args
                ),
            ]
        );
    }

    fn launches(content: &str, toml: bool) -> Vec<Launch> {
        borders_launch_lines_in(content, toml)
            .into_iter()
            .map(|l| l.2)
            .collect()
    }

    #[test]
    fn launch_lines_in_toml_strings() {
        let bare = Launch::Borders { args: false };
        let args = Launch::Borders { args: true };
        // Values, `key="…"` and array elements hold commands.
        assert_eq!(launches("a = 'exec-and-forget borders'", true), [bare]);
        assert_eq!(launches("a=\"exec-and-forget borders x=1\"", true), [args]);
        assert_eq!(
            launches(
                "a = ['exec-and-forget sketchybar', 'exec-and-forget borders']",
                true
            ),
            [bare]
        );
        // A shell inside the string.
        assert_eq!(
            launches(
                "a = ['exec-and-forget /bin/bash -c \"borders width=5\"']",
                true
            ),
            [args]
        );
        assert_eq!(
            launches("a = 'exec-and-forget brew services start borders'", true),
            [Launch::BrewService]
        );
        // Arguments to other commands, and comments.
        assert!(launches("a = 'exec-and-forget echo \"borders on\"'", true).is_empty());
        assert!(launches("a = 'exec-and-forget pgrep -x borders'", true).is_empty());
        assert!(launches("# a = 'exec-and-forget borders'", true).is_empty());
        // `#` inside a string is not a comment.
        assert_eq!(
            launches("a = 'exec-and-forget borders active_color=0xff#'", true),
            [args]
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
        let lines: Vec<usize> = borders_launch_lines_in(rc, false)
            .iter()
            .map(|l| l.0)
            .collect();
        assert_eq!(lines, vec![3, 10, 11, 12, 13, 15]);
        assert_eq!(
            borders_launch_lines_in(rc, false)[0].1,
            "borders active_color=0xffe1e3e4 inactive_color=0xff494d64 width=5.0 &"
        );
        assert!(borders_launch_lines_in("", false).is_empty());
        assert!(borders_launch_lines_in("bordersrc\nborders_x\n", false).is_empty());
        // `exec` with options, `env` with assignments, `command` without `-v`.
        assert_eq!(
            borders_launch_lines_in(
                "exec -c borders\nenv -i A=1 borders\ncommand borders\n",
                false
            )
            .len(),
            3
        );
    }

    #[test]
    fn launch_lines_skip_keywords_and_launchers() {
        let bare = Launch::Borders { args: false };
        let args = Launch::Borders { args: true };
        assert_eq!(
            launches(
                "if pgrep -x borders >/dev/null; then :; else borders width=5 & fi",
                false
            ),
            [args]
        );
        assert_eq!(
            launches("if ! pgrep borders; then borders; fi", false),
            [bare]
        );
        assert_eq!(
            launches("while true; do borders style=round; done", false),
            [args]
        );
        assert_eq!(launches("! borders", false), [bare]);
        assert_eq!(launches("time -p borders", false), [bare]);
        assert_eq!(
            launches("sudo -u me nice -n 5 borders width=3", false),
            [args]
        );
        assert_eq!(
            launches("nohup env A=1 exec -a jb borders &", false),
            [bare]
        );
        assert_eq!(launches("{ borders & }", false), [bare]);
        assert_eq!(launches("${HOME}/bin/borders hidpi=on", false), [args]);
        assert_eq!(launches("command -p borders", false), [bare]);
        assert!(launches("command -V borders", false).is_empty());
        // Redirections are no settings; anything else is.
        assert_eq!(
            launches("borders >/dev/null 2>&1 &\nborders > /tmp/b.log &", false),
            [bare, bare]
        );
        assert_eq!(launches("borders \"${options[@]}\"", false), [args]);
        assert_eq!(
            launches("\"/opt/homebrew/bin/borders\" width=5", false),
            [args]
        );
        assert_eq!(
            launches("/usr/local/bin/brew services restart borders", false),
            [Launch::BrewService]
        );
    }

    #[test]
    fn launch_lines_quoted_arguments_do_not_count() {
        for line in [
            "echo \"borders started\"",
            "killall \"borders\"",
            "pgrep -x 'borders'",
            "sketchybar --set x label=\"borders on\"",
            "FOO=\"borders x\"",
            "notify 'started' \"borders\"",
            "pgrep -c 'borders'",
            "printf '%s' '-c' 'borders'",
        ] {
            assert!(launches(line, false).is_empty(), "{line}");
        }
        // Command strings: a shell's `-c` and yabai's `action=`.
        let args = Launch::Borders { args: true };
        assert_eq!(launches("sh -c 'borders width=5'", false), [args]);
        assert_eq!(
            launches("/bin/zsh -lc \"borders width=5 &\"", false),
            [args]
        );
        assert_eq!(
            launches(
                "yabai -m signal --add event=x action='borders width=6'",
                false
            ),
            [args]
        );
        // TOML rules do not apply to a shell script.
        assert!(launches("a = 'borders width=5'", false).is_empty());
    }

    #[test]
    fn launch_line_advice_depends_on_the_line_and_bordersrc() {
        let rc = Path::new("/Users/r/.config/borders/bordersrc");
        let bin = Path::new("/Users/r/Applications/mbar.app/Contents/Resources/bin");
        let bare = Launch::Borders { args: false };
        let args = Launch::Borders { args: true };
        // Settings on the line: keep them; with the bundle's own full path.
        for rc in [None, Some(rc)] {
            let a = launch_line_advice(args, rc, Some(bin));
            assert!(a.contains("passes settings"), "{a}");
            assert!(a.contains("~/.config/borders/bordersrc"), "{a}");
            assert!(a.contains("mbar.borders"), "{a}");
            assert!(
                a.contains("/Users/r/Applications/mbar.app/Contents/Resources/bin/borders"),
                "{a}"
            );
            assert!(!a.starts_with("Remove"), "{a}");
            assert_eq!(a.contains("keep each setting in one place"), rc.is_some());
        }
        assert!(launch_line_advice(args, None, None)
            .contains("/Applications/mbar.app/Contents/Resources/bin/borders"));
        // A bare `borders` with a bordersrc: remove it.
        let a = launch_line_advice(bare, Some(rc), Some(bin));
        assert!(a.starts_with("Remove this line"), "{a}");
        assert!(a.contains(&*rc.to_string_lossy()), "{a}");
        // Without a bordersrc: configure first, never "mbar already runs your bordersrc".
        let a = launch_line_advice(bare, None, Some(bin));
        assert!(!a.starts_with("Remove"), "{a}");
        assert!(a.contains("mbar.borders"), "{a}");
        assert!(!a.contains("runs your bordersrc"), "{a}");
        // Homebrew's service never keeps working: always remove it.
        for rc in [None, Some(rc)] {
            let a = launch_line_advice(Launch::BrewService, rc, Some(bin));
            assert!(a.starts_with("Remove this line"), "{a}");
            assert!(a.contains("uninstalls"), "{a}");
            assert_eq!(a.contains("mbar.borders"), rc.is_none(), "{a}");
        }
        assert!(LAUNCH_LINE_DOCS.contains("docs/MIGRATING.md#migrating-from-jankyborders"));
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
                    launch: Launch::Borders { args: false },
                },
                LaunchLine {
                    file: home.join(".config/yabai/yabairc"),
                    line: 3,
                    text: "borders width=5 &".into(),
                    launch: Launch::Borders { args: true },
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

    fn triggers(content: &str) -> Vec<(usize, usize, usize, String)> {
        aerospace_triggers_in(Path::new("a.toml"), content)
            .into_iter()
            .map(|t| (t.line, t.first_line, t.last_line, t.text))
            .collect()
    }

    fn one_trigger(content: &str) -> AerospaceTrigger {
        let mut got = aerospace_triggers_in(Path::new("a.toml"), content);
        assert_eq!(got.len(), 1, "{content}: {got:?}");
        got.remove(0)
    }

    const WHY: &str = "The forked trigger arrives 10–50 ms after mbar's own event and can \
                       overwrite it with an older workspace.";

    #[test]
    fn aerospace_trigger_goodies_recipe() {
        // AeroSpace `docs/goodies.adoc`: the array spans two lines.
        let toml = "start-at-login = true

# Notify Sketchybar about workspace change
exec-on-workspace-change = ['/bin/bash', '-c',
    'sketchybar --trigger aerospace_workspace_change FOCUSED_WORKSPACE=$AEROSPACE_FOCUSED_WORKSPACE'
]

[mode.main.binding]
alt-1 = 'workspace 1'
";
        assert_eq!(
            triggers(toml),
            vec![(
                5,
                4,
                6,
                "'sketchybar --trigger aerospace_workspace_change FOCUSED_WORKSPACE=$AEROSPACE_FOCUSED_WORKSPACE'"
                    .to_string()
            )]
        );
        let t = one_trigger(toml);
        assert_eq!(t.setting, "exec-on-workspace-change");
        assert_eq!(t.event, "aerospace_workspace_change");
        assert!(t.removable() && t.whole_setting);
        assert_eq!(
            t.advice(),
            format!(
                "mbar sends aerospace_workspace_change itself; remove this setting (lines \
                 4–6). {WHY}"
            )
        );
    }

    #[test]
    fn aerospace_trigger_on_one_line() {
        let toml = "exec-on-workspace-change = [\"/bin/bash\", \"-c\", \"sketchybar --trigger aerospace_workspace_change FOCUSED_WORKSPACE=\\\"$AEROSPACE_FOCUSED_WORKSPACE\\\"\"] # bar\n";
        let got = aerospace_triggers_in(Path::new("/h/.aerospace.toml"), toml);
        assert_eq!(got.len(), 1);
        assert_eq!(
            (got[0].line, got[0].first_line, got[0].last_line),
            (1, 1, 1)
        );
        assert_eq!(got[0].file, PathBuf::from("/h/.aerospace.toml"));
        assert_eq!(
            got[0].advice(),
            format!("mbar sends aerospace_workspace_change itself; remove this line. {WHY}")
        );
        // mbar's own CLI, a full path, separate array elements, a quoted key, all four
        // variables mbar sets, quoted values, redirections and backgrounding.
        for toml in [
            "exec-on-workspace-change = ['/bin/sh', '-c', 'mbar --trigger aerospace_workspace_change']",
            "exec-on-workspace-change = ['/opt/homebrew/bin/sketchybar', '--trigger', 'aerospace_workspace_change']",
            "\"exec-on-workspace-change\" = ['/bin/zsh', '-lc', 'sketchybar --trigger aerospace_workspace_change &']",
            "  exec-on-workspace-change=['/bin/bash','-c','sketchybar --trigger aerospace_workspace_change']",
            "exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_change FOCUSED_WORKSPACE=\"$AEROSPACE_FOCUSED_WORKSPACE\" PREV_WORKSPACE=$AEROSPACE_PREV_WORKSPACE AEROSPACE_FOCUSED_WORKSPACE=$AEROSPACE_FOCUSED_WORKSPACE AEROSPACE_PREV_WORKSPACE=$AEROSPACE_PREV_WORKSPACE >/dev/null 2>&1']",
            "exec-on-workspace-change = ['/bin/bash', '-c', 'exec /opt/homebrew/bin/sketchybar --trigger aerospace_workspace_change FOCUSED_WORKSPACE=$(aerospace list-workspaces --focused)']",
        ] {
            let t = one_trigger(toml);
            assert!(t.removable() && t.whole_setting, "{toml}: {t:?}");
            assert!(t.advice().contains("remove this line."), "{toml}");
        }
    }

    #[test]
    fn aerospace_trigger_with_other_variables_stays() {
        let t = one_trigger(
            "exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_change FOCUSED_WORKSPACE=$AEROSPACE_FOCUSED_WORKSPACE MONITOR=\"$(aerospace list-monitors --focused)\" COUNT=3']",
        );
        assert!(!t.removable() && !t.whole_setting);
        assert_eq!(t.extra_vars, vec!["MONITOR", "COUNT"]);
        assert_eq!(
            t.advice(),
            "mbar sends this event itself, but only with FOCUSED_WORKSPACE/PREV_WORKSPACE \
             (and AEROSPACE_ aliases); your scripts also use MONITOR, COUNT — keep the line \
             or change the scripts."
        );
        // Another event: its own variables.
        let t = one_trigger(
            "on-mode-changed = ['exec-and-forget sketchybar --trigger aerospace_mode_change MODE=$(aerospace list-modes --current) COLOR=red']",
        );
        assert_eq!(t.extra_vars, vec!["COLOR"]);
        assert_eq!(
            t.advice(),
            "mbar sends this event itself, but only with MODE; your scripts also use COLOR — \
             keep the line or change the scripts."
        );
        // A setting with one removable trigger and one that stays: remove only the
        // first one's command.
        let toml = "exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_change; sketchybar --trigger aerospace_focus_change X=1']";
        let got = aerospace_triggers_in(Path::new("a.toml"), toml);
        assert_eq!(got.len(), 2);
        assert!(got[0].removable() && !got[0].whole_setting);
        assert!(!got[1].removable());
        assert_eq!(got[1].event, "aerospace_focus_change");
    }

    #[test]
    fn aerospace_trigger_among_other_commands() {
        for toml in [
            "exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_change; other-cmd']",
            "exec-on-workspace-change = ['/bin/zsh', '-c', 'echo; sketchybar --trigger aerospace_workspace_change &']",
            "exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_change && afplay x.aiff']",
            "exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_change | tee log']",
            // A chained sketchybar command, another event.
            "exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_change --set clock label=x']",
            "exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_change --trigger space_change']",
            // Several array elements of an AeroSpace command list.
            "on-focus-changed = ['move-mouse window-lazy-center', 'exec-and-forget sketchybar --trigger aerospace_focus_change']",
            "on-focused-monitor-changed = ['exec-and-forget sketchybar --trigger aerospace_monitor_change', 'move-mouse monitor-lazy-center']",
        ] {
            let t = one_trigger(toml);
            assert!(t.removable() && !t.whole_setting, "{toml}: {t:?}");
            assert_eq!(
                t.advice(),
                format!(
                    "mbar sends {} itself; remove only the `--trigger {}` command and keep the \
                     rest of this setting (it runs other commands too). The forked trigger \
                     arrives 10–50 ms after mbar's own event and can overwrite it with an \
                     older workspace.",
                    t.event, t.event
                ),
                "{toml}"
            );
        }
    }

    #[test]
    fn aerospace_trigger_all_events_and_settings() {
        for (setting, element) in [
            (
                "exec-on-workspace-change",
                "'/bin/bash', '-c', 'sketchybar --trigger EV'",
            ),
            (
                "on-focus-changed",
                "'exec-and-forget sketchybar --trigger EV'",
            ),
            (
                "on-focused-monitor-changed",
                "'exec-and-forget mbar --trigger EV'",
            ),
            (
                "on-mode-changed",
                "'exec-and-forget /opt/homebrew/bin/sketchybar --trigger EV'",
            ),
        ] {
            for (event, _) in AEROSPACE_EVENTS {
                let toml = format!("{setting} = [{}]\n", element.replace("EV", event));
                let t = one_trigger(&toml);
                assert_eq!((t.setting.as_str(), t.event.as_str()), (setting, event));
                assert!(t.removable() && t.whole_setting, "{toml}");
            }
        }
        // Every event's own variables count as sent.
        for toml in [
            "on-focus-changed = ['exec-and-forget sketchybar --trigger aerospace_focus_change FOCUSED_WORKSPACE=1 WINDOW_ID=2']",
            "on-focused-monitor-changed = ['exec-and-forget sketchybar --trigger aerospace_monitor_change MONITOR_ID=1']",
            "on-mode-changed = ['exec-and-forget sketchybar --trigger aerospace_mode_change MODE=main']",
            "on-mode-changed = ['exec-and-forget sketchybar --trigger aerospace_binding_triggered MODE=main BINDING=alt-1']",
            "on-focus-changed = ['exec-and-forget sketchybar --trigger aerospace_window_detected WINDOW_ID=1 WORKSPACE=2 APP_BUNDLE_ID=x APP_NAME=y']",
        ] {
            assert!(one_trigger(toml).whole_setting, "{toml}");
        }
        // PREV_WORKSPACE is only sent with the workspace change.
        let t = one_trigger(
            "on-focus-changed = ['exec-and-forget sketchybar --trigger aerospace_focus_change PREV_WORKSPACE=1']",
        );
        assert_eq!(t.extra_vars, vec!["PREV_WORKSPACE"]);
        assert!(t
            .advice()
            .contains("only with FOCUSED_WORKSPACE/WINDOW_ID;"));
        let t = one_trigger(
            "on-mode-changed = ['exec-and-forget sketchybar --trigger aerospace_mode_change']",
        );
        assert!(
            t.advice().ends_with("with an older mode."),
            "{}",
            t.advice()
        );
    }

    #[test]
    fn aerospace_trigger_reports_the_trigger_line() {
        let toml = "exec-on-workspace-change = [
  '/bin/bash',
  '-c',
  '''
  echo changed
  sketchybar --trigger aerospace_workspace_change \\
    FOCUSED_WORKSPACE=$AEROSPACE_FOCUSED_WORKSPACE OTHER=$X
  ''',
]
after-startup-command = []
";
        assert_eq!(
            triggers(toml),
            vec![(
                6,
                1,
                9,
                "sketchybar --trigger aerospace_workspace_change \\".to_string()
            )]
        );
        // The continued line still belongs to the trigger; `echo` is another command.
        let t = one_trigger(toml);
        assert_eq!(t.extra_vars, vec!["OTHER"]);
        assert!(!t.whole_setting);
        // Without `echo` and OTHER, the whole setting can go.
        let t = one_trigger(
            &toml
                .replace("  echo changed\n", "")
                .replace(" OTHER=$X", ""),
        );
        assert!(t.removable() && t.whole_setting, "{t:?}");
        assert!(t.advice().contains("remove this setting (lines 1–8)"));
        // A TOML basic multi-line string with a line-ending backslash.
        let toml = "exec-on-workspace-change = ['/bin/bash', '-c', \"\"\"\nsketchybar --trigger aerospace_workspace_change \\\n  FOCUSED_WORKSPACE=$AEROSPACE_FOCUSED_WORKSPACE\n\"\"\"]\n";
        let t = one_trigger(toml);
        assert_eq!((t.line, t.first_line, t.last_line), (2, 1, 4));
        assert!(t.whole_setting, "{t:?}");
    }

    #[test]
    fn aerospace_trigger_ignores_other_settings() {
        for toml in [
            // Commented out, the whole setting or the element.
            "# exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_change']",
            "exec-on-workspace-change = ['/bin/bash', '-c',\n  # 'sketchybar --trigger aerospace_workspace_change'\n  'true']",
            // Another key, another event, no trigger.
            "after-startup-command = ['exec-and-forget sketchybar --trigger aerospace_workspace_change']",
            "exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --trigger front_app_switched']",
            "exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_changed']",
            "exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --set space label=aerospace_workspace_change']",
            "exec-on-workspace-change = ['/bin/bash', '-c', 'echo sketchybar --trigger aerospace_workspace_change']",
            "exec-on-workspace-change = ['/bin/bash', '-c', 'notify-send x']",
            "exec-on-workspace-change-x = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_change']",
            "on-focus-changed-x = ['exec-and-forget sketchybar --trigger aerospace_focus_change']",
            "",
        ] {
            assert!(triggers(toml).is_empty(), "{toml}");
        }
        // The scan stops at the end of the setting.
        let toml = "exec-on-workspace-change = ['/bin/bash', '-c', 'true']
[mode.main.binding]
alt-1 = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_change']
";
        assert!(triggers(toml).is_empty());
        // `#` and `]` inside strings do not end the value.
        let toml = "on-focus-changed = ['exec-and-forget [ -x a ] # x',
  'exec-and-forget sketchybar --trigger aerospace_focus_change']
";
        let t = one_trigger(toml);
        assert_eq!((t.line, t.last_line), (2, 2));
        assert!(!t.whole_setting);
    }

    #[test]
    fn aerospace_configs_with_xdg() {
        let home = Path::new("/h");
        assert_eq!(
            aerospace_configs(home, ""),
            vec![
                PathBuf::from("/h/.aerospace.toml"),
                PathBuf::from("/h/.config/aerospace/aerospace.toml"),
            ]
        );
        assert_eq!(
            aerospace_configs(home, "/x/conf"),
            vec![
                PathBuf::from("/h/.aerospace.toml"),
                PathBuf::from("/x/conf/aerospace/aerospace.toml"),
                PathBuf::from("/h/.config/aerospace/aerospace.toml"),
            ]
        );
        // The default location once, a relative XDG_CONFIG_HOME is ignored.
        assert_eq!(aerospace_configs(home, "/h/.config/").len(), 2);
        assert_eq!(aerospace_configs(home, "conf").len(), 2);
    }

    #[test]
    fn aerospace_triggers_scan_all_configs() {
        let home = tmp("aerospace");
        let xdg = home.join("xdg");
        let xdg_s = xdg.to_string_lossy().to_string();
        let app = Path::new("/Applications/AeroSpace.app").exists();
        assert!(aerospace_triggers(&home, &xdg_s).is_empty());
        assert!(!aerospace_installed(&home, &xdg_s) || app);
        let recipe = "exec-on-workspace-change = ['/bin/bash', '-c', 'sketchybar --trigger aerospace_workspace_change']\n";
        // The XDG config alone makes AeroSpace look installed.
        std::fs::create_dir_all(xdg.join("aerospace")).unwrap();
        std::fs::write(
            xdg.join("aerospace/aerospace.toml"),
            format!("# x\n\n{recipe}"),
        )
        .unwrap();
        assert!(aerospace_installed(&home, &xdg_s));
        assert!(!aerospace_installed(&home, "") || app);
        std::fs::write(home.join(".aerospace.toml"), recipe).unwrap();
        std::fs::create_dir_all(home.join(".config/aerospace")).unwrap();
        std::fs::write(
            home.join(".config/aerospace/aerospace.toml"),
            format!("start-at-login = true\n{recipe}"),
        )
        .unwrap();
        assert!(aerospace_installed(&home, ""));
        let found: Vec<_> = aerospace_triggers(&home, &xdg_s)
            .into_iter()
            .map(|t| (t.file, t.line))
            .collect();
        assert_eq!(
            found,
            vec![
                (home.join(".aerospace.toml"), 1),
                (xdg.join("aerospace/aerospace.toml"), 3),
                (home.join(".config/aerospace/aerospace.toml"), 2),
            ]
        );
    }
}

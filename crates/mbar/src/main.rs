//! `mbar`: SketchyBar-compatible status bar.
//!
//! * `mbar` (no arguments) starts the daemon (`daemon`, `platform`).
//! * `mbar --set …` / `mbar -m --set …` sends the arguments to a running daemon and
//!   prints the response (`client`).
//! * Invoked through a `sketchybar -> mbar` symlink it behaves identically (the bar name
//!   `sketchybar` maps to `mbar`, see `mbar_ipc::bar_name_from_argv0`).
//!
//! Argument dispatch follows `docs/spec/cli.md` §1.2. Daemon-only flags (extensions):
//! `--headless` (no windows, deterministic metrics; the only platform on non-macOS).

mod client;
mod daemon;
mod driver;
mod hotload;
mod ipc;
mod logging;
mod platform;
mod scripts;

use std::path::PathBuf;

/// SketchyBar's version string (`cli.md` §1.2), printed when invoked as `sketchybar`.
const SKETCHYBAR_VERSION: &str = "sketchybar-v2.24.0";

/// Environment variable that allows running as root (containers, CI). SketchyBar refuses
/// root unconditionally (`cli.md` §1.1).
pub const ALLOW_ROOT_ENV: &str = "MBAR_ALLOW_ROOT";

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let argv0 = argv.first().cloned().unwrap_or_else(|| "mbar".to_string());
    let bar_name = mbar_ipc::bar_name_from_argv0(&argv0);

    logging::init();

    // cli.md §1.1 step 2: refuse root (client mode included).
    if is_root() && std::env::var_os(ALLOW_ROOT_ENV).is_none() {
        eprintln!("{bar_name}: running as root is not allowed! abort..");
        std::process::exit(1);
    }

    // §1.1 step 3: every child inherits BAR_NAME. Set before any thread exists.
    std::env::set_var("BAR_NAME", &bar_name);

    match dispatch(&argv[1..]) {
        Mode::Version => {
            let base = std::path::Path::new(&argv0)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if base == "sketchybar" {
                println!("{SKETCHYBAR_VERSION}");
            } else {
                println!("mbar-v{}", env!("CARGO_PKG_VERSION"));
            }
        }
        Mode::Help => print!("{}", help_text(&argv0)),
        Mode::Error(msg) => {
            print!("{msg}");
            std::process::exit(1);
        }
        Mode::Client(args) => std::process::exit(client::run(&bar_name, &args)),
        Mode::Daemon(opts) => daemon::run(bar_name, opts),
    }
}

/// Daemon start options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonOptions {
    /// `-c/--config <file>` (resolved with `realpath`).
    pub config: Option<PathBuf>,
    /// `--headless`: never create windows (always the case on non-macOS).
    pub headless: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Version,
    Help,
    Error(&'static str),
    Client(Vec<String>),
    Daemon(DaemonOptions),
}

/// `parse_arguments` (`cli.md` §1.2). Only the first argument decides; the daemon flags
/// `--headless` and `-c/--config <file>` may be combined in any order.
fn dispatch(args: &[String]) -> Mode {
    let Some(first) = args.first() else {
        return Mode::Daemon(DaemonOptions::default());
    };
    match first.as_str() {
        "-v" | "--version" => Mode::Version,
        "-h" | "--help" => Mode::Help,
        "-m" | "--message" => Mode::Client(args[1..].to_vec()),
        "-c" | "--config" | "--headless" => parse_daemon_options(args),
        _ => Mode::Client(args.to_vec()),
    }
}

fn parse_daemon_options(args: &[String]) -> Mode {
    let mut opts = DaemonOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--headless" => opts.headless = true,
            "-c" | "--config" => {
                let Some(path) = args.get(i + 1) else {
                    return Mode::Error("[!] Error: Too few arguments for argument 'config'.\n");
                };
                match std::fs::canonicalize(path) {
                    Ok(p) => opts.config = Some(p),
                    Err(_) => {
                        return Mode::Error("[!] Error: Specified config file path invalid.\n")
                    }
                }
                i += 1;
            }
            // SketchyBar ignores everything after `--config <file>`.
            _ => break,
        }
        i += 1;
    }
    Mode::Daemon(opts)
}

fn is_root() -> bool {
    // SAFETY: getuid/geteuid have no preconditions.
    unsafe { libc::getuid() == 0 || libc::geteuid() == 0 }
}

/// `misc/help.h` (`cli.md` §1.3) with mbar's config locations and extensions.
fn help_text(argv0: &str) -> String {
    format!(
        "Usage: {argv0} [options]\n\
\n\
Startup: \n\
\x20 -c, --config CONFIGFILE\tRead CONFIGFILE as the configuration file\n\
\x20                        \tDefault CONFIGFILE is ~/.config/mbar/init.lua or ~/.config/mbar/mbarrc\n\
\x20                        \t(then ~/.config/sketchybar/sketchybarrc)\n\
\x20     --headless          \tRun without windows (default on non-macOS systems)\n\
\n\
Set global bar properties, see https://felixkratz.github.io/SketchyBar/config/bar\n\
\x20     --bar <setting>=<value> ... <setting>=<value>\n\
\n\
Items and their properties, see https://felixkratz.github.io/SketchyBar/config/items\n\
\x20     --add item <name> <position>\tAdd item to bar\n\
\x20     --set <name> <property>=<value> ... <property>=<value>\n\
\x20                                 \tChange item properties\n\
\x20     --default <property>=<value> ... <property>=<value>\n\
\x20                                 \tChange default properties for new items\n\
\x20     --set <name> popup.<popup_property>=<value>\n\
\x20                                 \tConfigure item popup menu\n\
\x20                                 \tSee https://felixkratz.github.io/SketchyBar/config/popups\n\
\x20     --reorder <name> ... <name> \tReorder items\n\
\x20     --move <name> before <reference name>\n\
\x20     --move <name> after <reference name>\n\
\x20                                 \tMove item relative to reference item\n\
\x20     --clone <name> <parent name> [optional: before/after]\n\
\x20                                 \tClone parent to create new item\n\
\x20     --rename <old name> <new name>\tRename item\n\
\x20     --remove <name>             \tRemove item\n\
\n\
Special components, see https://felixkratz.github.io/SketchyBar/config/components\n\
\x20     --add graph <name> <position> <width in points>\n\
\x20                                 \tAdd graph component\n\
\x20     --push <name> <data point> ... <data point>\n\
\x20                                 \tPush data points to a graph\n\
\x20     --add space <name> <position>\tAdd space component\n\
\x20     --add bracket <name> <member name> ... <member name>\n\
\x20                                 \tAdd bracket component\n\
\x20     --add alias <application_name> <position>\n\
\x20                                 \tAdd alias component\n\
\x20     --add slider <name> <position> <width>\n\
\x20                                 \tAdd slider component\n\
\x20     --add app_menu <name> <position>\tAdd the front application's menus (mbar)\n\
\n\
Events and Scripting, see https://felixkratz.github.io/SketchyBar/config/events\n\
\x20     --subscribe <name> <event> ... <event>\n\
\x20                                 \tSubscribe to events\n\
\x20     --add event <name> [optional: <NSDistributedNotificationName>]\n\
\x20                                 \tCreate custom event\n\
\x20     --trigger <event> [optional: <envvar>=<value> ... <envvar>=<value>]\n\
\x20                                 \tTrigger custom event\n\
\n\
Querying information, see https://felixkratz.github.io/SketchyBar/config/querying\n\
\x20     --query bar               \tQuery bar properties\n\
\x20     --query <name>            \tQuery item properties\n\
\x20     --query defaults          \tQuery default properties\n\
\x20     --query events            \tQuery events\n\
\x20     --query default_menu_items\tQuery names of available items for aliases\n\
\x20     --query stats             \tQuery runtime statistics (mbar)\n\
\x20     --query menus             \tQuery the front application's menu titles (mbar)\n\
\x20     --monitor [events|stats|all]\tStream events / statistics as JSON lines (mbar)\n\
\n\
Animations, see https://felixkratz.github.io/SketchyBar/config/animations\n\
\x20     --animate <linear|quadratic|tanh|sin|exp|circ> <duration> \\\n\
\x20               --bar <property=value> ... <property=value>\\\n\
\x20               --set <name> <property=value> ... <property=value>\n\
\x20                        \tAnimate from given source to target property values\n\
\n\
Menus (mbar)\n\
\x20     --menu <index|title>       \tOpen a menu of the front application\n\
\x20     --menubar hide|show|toggle \tAuto-hide the native menu bar\n\
\n\
Reloading the config\n\
\x20     --hotload <boolean>        \tEnable or disable the config hotloader\n\
\x20     --reload [optional: <path>]\tReload the current or the given config\n\
\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn dispatch_modes() {
        assert_eq!(dispatch(&[]), Mode::Daemon(DaemonOptions::default()));
        assert_eq!(dispatch(&s(&["-v"])), Mode::Version);
        assert_eq!(dispatch(&s(&["--version", "x"])), Mode::Version);
        assert_eq!(dispatch(&s(&["-h"])), Mode::Help);
        assert_eq!(
            dispatch(&s(&["-m", "--set", "a", "x=1"])),
            Mode::Client(s(&["--set", "a", "x=1"]))
        );
        assert_eq!(dispatch(&s(&["-m"])), Mode::Client(vec![]));
        assert_eq!(
            dispatch(&s(&["--query", "bar"])),
            Mode::Client(s(&["--query", "bar"]))
        );
        assert_eq!(
            dispatch(&s(&["--config"])),
            Mode::Error("[!] Error: Too few arguments for argument 'config'.\n")
        );
        assert_eq!(
            dispatch(&s(&["-c", "/nonexistent/mbar/rc"])),
            Mode::Error("[!] Error: Specified config file path invalid.\n")
        );
        let Mode::Daemon(o) = dispatch(&s(&["--headless", "-c", "/"])) else {
            panic!()
        };
        assert!(o.headless);
        assert_eq!(o.config.as_deref(), Some(std::path::Path::new("/")));
        let Mode::Daemon(o) = dispatch(&s(&["-c", "/", "--headless"])) else {
            panic!()
        };
        assert!(o.headless);
    }

    #[test]
    fn help_mentions_argv0() {
        let h = help_text("/x/sketchybar");
        assert!(h.starts_with("Usage: /x/sketchybar [options]\n\nStartup: \n"));
        assert!(h.ends_with("config\n\n"));
    }
}

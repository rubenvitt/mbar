//! `mbar`: SketchyBar-compatible status bar.
//!
//! * `mbar` (no arguments) starts the daemon (`daemon`, `platform`).
//! * `mbar --set …` / `mbar -m --set …` sends the arguments to a running daemon and
//!   prints the response (`client`).
//! * Invoked through a `sketchybar -> mbar` symlink it behaves identically (the bar name
//!   `sketchybar` maps to `mbar`, see `mbar_ipc::bar_name_from_argv0`); only `BAR_NAME`
//!   keeps the invoked name (`mbar_ipc::program_name`).
//! * Invoked as `borders` (a `borders -> mbar` symlink) it is JankyBorders' client: the
//!   arguments are validated locally and sent to the default bar as `--borders …`
//!   (borders design §2, `docs/spec/borders.md` §2.1); it never starts a daemon.
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
mod signals;
mod updater;

use std::ffi::OsString;
use std::path::PathBuf;

/// SketchyBar's version string (`cli.md` §1.2), printed when invoked as `sketchybar`.
const SKETCHYBAR_VERSION: &str = "sketchybar-v2.24.0";

/// Environment variable that allows running as root (containers, CI). SketchyBar refuses
/// root unconditionally (`cli.md` §1.1).
pub const ALLOW_ROOT_ENV: &str = "MBAR_ALLOW_ROOT";

fn main() {
    // `args_os`: `std::env::args` panics on any argument that is not valid UTF-8, while
    // SketchyBar forwards raw bytes (plugins pass Latin-1 SSIDs, window titles, ...).
    let argv: Vec<OsString> = std::env::args_os().collect();
    let argv0 = argv
        .first()
        .map(|a| a.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mbar".to_string());
    // `g_name` as invoked (`sketchybar` stays `sketchybar`) vs. the IPC identity
    // (`sketchybar` maps to `mbar`).
    let program_name = mbar_ipc::program_name(&argv0);
    let bar_name = mbar_ipc::bar_name_from_argv0(&argv0);

    logging::init();

    // cli.md §1.1 step 2: refuse root (client mode included).
    if is_root() && std::env::var_os(ALLOW_ROOT_ENV).is_none() {
        eprintln!("{program_name}: running as root is not allowed! abort..");
        std::process::exit(1);
    }

    // §1.1 step 3: every child inherits BAR_NAME = basename(argv[0]) (examples.md §0: a
    // `sketchybar -> mbar` symlink yields `BAR_NAME=sketchybar`). Set before any thread
    // exists.
    std::env::set_var("BAR_NAME", &program_name);

    if program_name == "borders" {
        std::process::exit(borders_main(&bar_name, args_after_argv0(&argv)));
    }

    match dispatch(args_after_argv0(&argv)) {
        Mode::Version => {
            if program_name == "sketchybar" {
                println!("{SKETCHYBAR_VERSION}");
            } else {
                println!("mbar-v{}", env!("CARGO_PKG_VERSION"));
            }
        }
        Mode::Help => print!("{}", help_text(&argv0, &program_name)),
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

/// `argv[1..]`, or nothing when the process was exec'd with `argc == 0` (allowed on
/// macOS; slicing `[1..]` would panic).
fn args_after_argv0<T>(argv: &[T]) -> &[T] {
    argv.get(1..).unwrap_or(&[])
}

/// Client arguments: invalid UTF-8 is replaced lossily (U+FFFD), exactly like the wire
/// decoding (`mbar_ipc::decode_args`), instead of aborting the update.
fn lossy_args(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// `parse_arguments` (`cli.md` §1.2). Only the first argument decides; the daemon flags
/// `--headless` and `-c/--config <file>` may be combined in any order.
fn dispatch(args: &[OsString]) -> Mode {
    let Some(first) = args.first() else {
        return Mode::Daemon(DaemonOptions::default());
    };
    match first.to_string_lossy().as_ref() {
        "-v" | "--version" => Mode::Version,
        "-h" | "--help" => Mode::Help,
        "-m" | "--message" => Mode::Client(lossy_args(&args[1..])),
        "-c" | "--config" | "--headless" => parse_daemon_options(args),
        _ => Mode::Client(lossy_args(args)),
    }
}

/// `borders -h` (BR-CLI-02) plus where the command comes from.
const BORDERS_HELP: &str = "Refer to the man page for help: man borders\n\
borders is provided by mbar: see docs/MIGRATING.md\n";

/// What `borders <args>` does (borders design §2).
#[derive(Debug, PartialEq, Eq)]
enum BordersMode {
    /// `-v`/`--version` as `argv[1]` (BR-CLI-01).
    Version,
    /// `-h`/`--help` as `argv[1]` (BR-CLI-02).
    Help,
    /// Everything else: the arguments the daemon accepts and JankyBorders' error lines
    /// (`[?] Borders: …\n`) for the others (BR-CLI-03 step 3).
    Send {
        valid: Vec<String>,
        errors: Vec<String>,
    },
}

/// Only `argv[1]` is checked for `-v`/`-h`; anywhere else they are invalid arguments.
fn borders_dispatch(args: &[OsString]) -> BordersMode {
    match args.first().map(|a| a.to_string_lossy()).as_deref() {
        Some("-v" | "--version") => BordersMode::Version,
        Some("-h" | "--help") => BordersMode::Help,
        _ => {
            let (valid, errors) = mbar_core::borders::validate_args(&lossy_args(args));
            BordersMode::Send { valid, errors }
        }
    }
}

/// `mbar` invoked as `borders`; returns the exit code.
fn borders_main(bar_name: &str, args: &[OsString]) -> i32 {
    match borders_dispatch(args) {
        BordersMode::Version => {
            println!("{}", mbar_core::borders::BORDERS_VERSION);
            0
        }
        BordersMode::Help => {
            print!("{BORDERS_HELP}");
            0
        }
        BordersMode::Send { valid, errors } => {
            // Like JankyBorders' client: on stdout, each line already ends in `\n`.
            {
                use std::io::Write;
                let mut out = std::io::stdout().lock();
                for line in &errors {
                    let _ = out.write_all(line.as_bytes());
                }
                let _ = out.flush();
            }
            client::run_borders(bar_name, &valid)
        }
    }
}

fn parse_daemon_options(args: &[OsString]) -> Mode {
    let mut opts = DaemonOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].to_string_lossy().as_ref() {
            "--headless" => opts.headless = true,
            "-c" | "--config" => {
                // The path stays raw bytes (`realpath` in SketchyBar).
                let Some(path) = args.get(i + 1).map(OsString::as_os_str) else {
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

/// SketchyBar's `misc/help.h` (`cli.md` §1.3), verbatim; `%s` is `argv[0]`.
const SKETCHYBAR_HELP: &str = "Usage: %s [options]\n\
\n\
Startup: \n\
\x20 -c, --config CONFIGFILE\tRead CONFIGFILE as the configuration file\n\
\x20                        \tDefault CONFIGFILE is ~/.config/sketchybar/sketchybarrc\n\
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
\x20     --clone <parent name> <name> [optional: before/after]\n\
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
\n\
Animations, see https://felixkratz.github.io/SketchyBar/config/animations\n\
\x20     --animate <linear|quadratic|tanh|sin|exp|circ> <duration> \\\n\
\x20               --bar <property=value> ... <property=value>\\\n\
\x20               --set <name> <property=value> ... <property=value>\n\
\x20                        \tAnimate from given source to target property values\n\
\n\
Reloading the config\n\
\x20     --hotload <boolean>        \tEnable or disable the config hotloader\n\
\x20     --reload [optional: <path>]\tReload the current or the given config\n\
\n";

/// `-h/--help`. Invoked as `sketchybar` (the program name, like `-v`), this is
/// SketchyBar's `misc/help.h` verbatim (`cli.md` §1.3: `printf(help_str, argv[0])`).
/// Invoked as `mbar` it lists mbar's config locations and extensions instead
/// (`DEVIATIONS.md`).
fn help_text(argv0: &str, program_name: &str) -> String {
    if program_name == "sketchybar" {
        return SKETCHYBAR_HELP.replacen("%s", argv0, 1);
    }
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
\x20     --query borders           \tQuery the window borders configuration (mbar)\n\
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
Window borders (mbar), JankyBorders keys, see docs/EXTENSIONS.md\n\
\x20     --borders <key>=<value> ... <key>=<value>\n\
\x20                                 \tConfigure window borders (also drawing=on|off)\n\
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

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn dispatch_modes() {
        assert_eq!(dispatch(&[]), Mode::Daemon(DaemonOptions::default()));
        assert_eq!(dispatch(&os(&["-v"])), Mode::Version);
        assert_eq!(dispatch(&os(&["--version", "x"])), Mode::Version);
        assert_eq!(dispatch(&os(&["-h"])), Mode::Help);
        assert_eq!(
            dispatch(&os(&["-m", "--set", "a", "x=1"])),
            Mode::Client(s(&["--set", "a", "x=1"]))
        );
        assert_eq!(dispatch(&os(&["-m"])), Mode::Client(vec![]));
        assert_eq!(
            dispatch(&os(&["--query", "bar"])),
            Mode::Client(s(&["--query", "bar"]))
        );
        assert_eq!(
            dispatch(&os(&["--config"])),
            Mode::Error("[!] Error: Too few arguments for argument 'config'.\n")
        );
        assert_eq!(
            dispatch(&os(&["-c", "/nonexistent/mbar/rc"])),
            Mode::Error("[!] Error: Specified config file path invalid.\n")
        );
        let Mode::Daemon(o) = dispatch(&os(&["--headless", "-c", "/"])) else {
            panic!()
        };
        assert!(o.headless);
        assert_eq!(o.config.as_deref(), Some(std::path::Path::new("/")));
        let Mode::Daemon(o) = dispatch(&os(&["-c", "/", "--headless"])) else {
            panic!()
        };
        assert!(o.headless);
    }

    /// Borders design §2: `-v`/`-h` only as `argv[1]`, everything else is validated.
    #[test]
    fn borders_dispatch_modes() {
        assert_eq!(borders_dispatch(&os(&["-v"])), BordersMode::Version);
        assert_eq!(
            borders_dispatch(&os(&["--version", "x"])),
            BordersMode::Version
        );
        assert_eq!(borders_dispatch(&os(&["-h"])), BordersMode::Help);
        assert_eq!(borders_dispatch(&os(&["--help"])), BordersMode::Help);
        assert_eq!(
            borders_dispatch(&[]),
            BordersMode::Send {
                valid: vec![],
                errors: vec![]
            }
        );
        assert_eq!(
            borders_dispatch(&os(&[
                "width=5.0",
                "-v",
                "bogus",
                "active_color=0xffe1e3e4"
            ])),
            BordersMode::Send {
                valid: s(&["width=5.0", "active_color=0xffe1e3e4"]),
                errors: s(&[
                    "[?] Borders: Invalid argument '-v'\n",
                    "[?] Borders: Invalid argument 'bogus'\n"
                ]),
            }
        );
        assert!(BORDERS_HELP.starts_with("Refer to the man page for help: man borders\n"));
    }

    #[test]
    fn help_mentions_argv0() {
        let h = help_text("/x/mbar", "mbar");
        assert!(h.starts_with("Usage: /x/mbar [options]\n\nStartup: \n"));
        assert!(h.ends_with("config\n\n"));
        assert!(h.contains("--headless"));
        assert!(h.contains("~/.config/mbar/init.lua"));
        assert!(h.contains("\n      --borders <key>=<value> ... <key>=<value>\n"));
        assert!(h.contains("\n      --query borders           \tQuery the window borders"));
    }

    /// The fenced block of `cli.md` §1.3.
    fn spec_help() -> String {
        let spec = include_str!("../../../docs/spec/cli.md");
        let start = spec.find("### 1.3 Help text").expect("§1.3");
        let body = &spec[start..];
        let open = body.find("```\n").expect("opening fence") + 4;
        let len = body[open..].find("\n```\n").expect("closing fence");
        // The fence's own line break follows the help's final blank line.
        format!("{}\n", &body[open..open + len])
    }

    /// CLI-7: invoked as `sketchybar`, `-h` prints `misc/help.h` verbatim.
    #[test]
    fn sketchybar_help_is_verbatim() {
        let expected = spec_help().replacen("%s", "/opt/bin/sketchybar", 1);
        assert_eq!(help_text("/opt/bin/sketchybar", "sketchybar"), expected);
        assert!(expected.ends_with("config\n\n"));
        assert!(expected.contains("Startup: \n"));
    }

    /// R8: `argc == 0` (allowed on macOS) must not panic when slicing `argv[1..]`.
    #[test]
    fn empty_argv_is_daemon_mode() {
        let empty: Vec<OsString> = Vec::new();
        assert!(args_after_argv0(&empty).is_empty());
        assert_eq!(
            dispatch(args_after_argv0(&empty)),
            Mode::Daemon(DaemonOptions::default())
        );
        assert_eq!(args_after_argv0(&os(&["mbar", "-v"])), &os(&["-v"])[..]);
    }

    /// R7: arguments that are not valid UTF-8 are forwarded lossily instead of panicking.
    #[test]
    fn non_utf8_client_args_are_lossy() {
        use std::os::unix::ffi::OsStringExt;
        let args = vec![
            OsString::from("--set"),
            OsString::from("x"),
            OsString::from_vec(b"label=\xffab".to_vec()),
        ];
        assert_eq!(
            dispatch(&args),
            Mode::Client(s(&["--set", "x", "label=\u{FFFD}ab"]))
        );
        let mut m = vec![OsString::from("-m")];
        m.extend(args);
        assert_eq!(
            dispatch(&m),
            Mode::Client(s(&["--set", "x", "label=\u{FFFD}ab"]))
        );
    }

    /// R7: a config path that is not valid UTF-8 is resolved as raw bytes (`realpath`).
    #[test]
    fn non_utf8_config_path_is_raw() {
        use std::os::unix::ffi::OsStringExt;
        let dir = std::env::temp_dir().join(format!("mbar-r7-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut name = dir.clone().into_os_string().into_vec();
        name.extend_from_slice(b"/rc\xff");
        let path = OsString::from_vec(name);
        if let Err(e) = std::fs::write(&path, "") {
            // APFS/HFS+ (macOS) only accept UTF-8 file names and reject this one with
            // EILSEQ. Such a path cannot exist there, so `-c` must report the unresolvable
            // path as an error instead of panicking.
            assert_eq!(e.raw_os_error(), Some(libc::EILSEQ), "{e}");
            let _ = std::fs::remove_dir_all(&dir);
            let mode = dispatch(&[OsString::from("-c"), path]);
            assert!(matches!(mode, Mode::Error(_)), "{mode:?}");
            return;
        }
        let mode = dispatch(&[OsString::from("-c"), path.clone()]);
        let expected = std::fs::canonicalize(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let Mode::Daemon(o) = mode else {
            panic!("{mode:?}")
        };
        assert_eq!(o.config, Some(expected));
    }
}

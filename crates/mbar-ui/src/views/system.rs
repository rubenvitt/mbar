//! System: daemon status and start, reload, native menu bar auto-hide, window borders
//! (`--query borders` / `--borders drawing=…`, design doc
//! `docs/superpowers/specs/2026-10-09-borders-design.md` §5; plus stopping and removing
//! a Homebrew JankyBorders that is still there after setup), the AeroSpace connection
//! (`--query aerospace`, design doc `docs/superpowers/specs/2026-10-09-aerospace-design.md`),
//! permission hints and launch at login.

use std::time::Duration;

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    switch::Switch,
    v_flex, ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
};
use gpui_kit::*;
use mbar_ui_model::ipc::DaemonStatus;
use mbar_ui_model::model::{AerospaceInfo, BordersInfo};
use mbar_ui_model::onboarding as ob;
use mbar_ui_model::system::{
    self as sys, Permissions, ACCESSIBILITY_SETTINGS_URL, SCREEN_RECORDING_SETTINGS_URL,
};

use super::{interval, page_header, permission_row, section, status_color, status_text, Shared};

/// Title and (truncated) description on the left, a control on the right.
fn setting_row(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> Div {
    h_flex()
        .w_full()
        .gap_3()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().child(title.into()))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(description.into()),
                ),
        )
        .child(div().flex_shrink_0().child(control))
}

/// Asks the window to show the Setup page ("Run setup again").
pub struct ShowSetup;

impl EventEmitter<ShowSetup> for SystemView {}

pub struct SystemView {
    shared: Shared,
    status: DaemonStatus,
    checking: bool,
    starting: bool,
    reloading: bool,
    permissions: Option<Permissions>,
    menubar_hidden: Option<bool>,
    /// `--query borders`; `None` until read or while the daemon is gone.
    borders: Option<Result<BordersInfo, String>>,
    reading_borders: bool,
    /// Homebrew state, for JankyBorders left over after setup (setup does not run
    /// again once completed).
    brew: ob::BrewState,
    removing_brew_borders: bool,
    /// `--query aerospace`; `Ok(None)` when AeroSpace is not installed (not asked);
    /// `None` until read or while the daemon is gone.
    aerospace: Option<Result<Option<AerospaceInfo>, String>>,
    reading_aerospace: bool,
    launch_agent: bool,
    /// Running from mbar.app on macOS: login item, updates and setup instead of the
    /// source-build hints.
    #[cfg(target_os = "macos")]
    app: Option<AppState>,
    _poll: Task<()>,
}

#[cfg(target_os = "macos")]
struct AppState {
    bundle: mbar_app::bundle::AppBundle,
    login: crate::mac::login_item::LoginItem,
    paths_d: mbar_ui_model::onboarding::PathsD,
    fixing_paths: bool,
}

#[cfg(target_os = "macos")]
fn read_paths_d(bin: &std::path::Path) -> mbar_ui_model::onboarding::PathsD {
    mbar_ui_model::onboarding::paths_d_state(
        std::fs::read_to_string("/etc/paths.d/mbar").ok().as_deref(),
        bin,
    )
}

impl SystemView {
    pub fn new(shared: Shared, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let poll = interval(cx, Duration::from_secs(2), |this: &mut Self, cx| {
            this.check_status(cx)
        });
        let mut view = SystemView {
            shared,
            status: DaemonStatus::Unknown,
            checking: false,
            starting: false,
            reloading: false,
            permissions: None,
            menubar_hidden: None,
            borders: None,
            reading_borders: false,
            brew: ob::BrewState::default(),
            removing_brew_borders: false,
            aerospace: None,
            reading_aerospace: false,
            launch_agent: sys::launch_agent_path().is_some_and(|p| p.exists()),
            #[cfg(target_os = "macos")]
            app: sys::current_bundle().map(|bundle| AppState {
                login: crate::mac::login_item::status(),
                paths_d: read_paths_d(&bundle.bin_dir()),
                bundle,
                fixing_paths: false,
            }),
            _poll: poll,
        };
        view.check_status(cx);
        view.read_menubar(cx);
        view.read_brew(cx);
        view
    }

    pub fn status(&self) -> &DaemonStatus {
        &self.status
    }

    fn check_status(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        self.shared.spawn_blocking(
            cx,
            |client| client.status(),
            |this, status, cx| {
                this.checking = false;
                let became_connected =
                    status == DaemonStatus::Connected && this.status != DaemonStatus::Connected;
                let connected = status == DaemonStatus::Connected;
                if status != this.status {
                    if !connected {
                        // Probed through the daemon; stale once it is gone.
                        this.permissions = None;
                        this.borders = None;
                        this.aerospace = None;
                    }
                    this.status = status;
                    cx.notify();
                }
                if became_connected {
                    this.starting = false;
                    this.probe_permissions(cx);
                }
                // A reload or a `borders …` call elsewhere changes it: follow the poll.
                if connected {
                    this.read_borders(cx);
                    this.read_aerospace(cx);
                }
                None
            },
        );
    }

    fn read_borders(&mut self, cx: &mut Context<Self>) {
        if self.reading_borders {
            return;
        }
        self.reading_borders = true;
        self.shared.spawn_blocking(
            cx,
            |client| client.query_borders().map_err(|e| e.to_string()),
            |this, borders, cx| {
                this.reading_borders = false;
                if this.borders.as_ref() != Some(&borders) {
                    this.borders = Some(borders);
                    cx.notify();
                }
                None
            },
        );
    }

    fn read_aerospace(&mut self, cx: &mut Context<Self>) {
        if self.reading_aerospace {
            return;
        }
        self.reading_aerospace = true;
        self.shared.spawn_blocking(
            cx,
            |client| {
                let home = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default());
                let xdg = std::env::var("XDG_CONFIG_HOME").unwrap_or_default();
                if !ob::aerospace_installed(&home, &xdg) {
                    return Ok(None);
                }
                client
                    .query_aerospace()
                    .map(Some)
                    .map_err(|e| e.to_string())
            },
            |this, aerospace, cx| {
                this.reading_aerospace = false;
                if this.aerospace.as_ref() != Some(&aerospace) {
                    this.aerospace = Some(aerospace);
                    cx.notify();
                }
                None
            },
        );
    }

    fn set_borders(&mut self, on: bool, cx: &mut Context<Self>) {
        // Optimistic; re-read the real state afterwards.
        if let Some(Ok(b)) = &mut self.borders {
            b.drawing = on;
        }
        cx.notify();
        self.shared.spawn_blocking(
            cx,
            move |client| client.set_borders_drawing(on),
            |this, result, cx| {
                this.read_borders(cx);
                result
                    .err()
                    .map(|e| Notification::error(e.to_string()).title("Window borders"))
            },
        );
    }

    fn read_brew(&mut self, cx: &mut Context<Self>) {
        self.shared.spawn_blocking(
            cx,
            |_| ob::detect_brew(ob::find_brew().as_deref()),
            |this, brew, cx| {
                if this.brew != brew {
                    this.brew = brew;
                    cx.notify();
                }
                None
            },
        );
    }

    /// The JankyBorders part of the setup's cleanup: `brew services stop borders`,
    /// `brew uninstall borders`.
    fn remove_brew_borders(&mut self, cx: &mut Context<Self>) {
        self.removing_brew_borders = true;
        cx.notify();
        let brew = self.brew.clone();
        self.shared.spawn_blocking(
            cx,
            move |_| {
                let cmds = ob::borders_cleanup_commands(&brew, ob::find_brew().as_deref(), true);
                ob::run_commands(&cmds)
            },
            |this, result, cx| {
                this.removing_brew_borders = false;
                this.read_brew(cx);
                cx.notify();
                Some(match result {
                    Ok(_) => Notification::success("Stopped and removed JankyBorders"),
                    Err(e) => Notification::error(e).title("JankyBorders"),
                })
            },
        );
    }

    fn probe_permissions(&mut self, cx: &mut Context<Self>) {
        self.shared.spawn_blocking(
            cx,
            |client| sys::probe_permissions(&client),
            |this, permissions, cx| {
                this.permissions = Some(permissions);
                cx.notify();
                None
            },
        );
    }

    fn read_menubar(&mut self, cx: &mut Context<Self>) {
        self.shared.spawn_blocking(
            cx,
            |_| sys::menubar_autohide(),
            |this, hidden, cx| {
                this.menubar_hidden = hidden;
                cx.notify();
                None
            },
        );
    }

    fn start_daemon(&mut self, cx: &mut Context<Self>) {
        self.starting = true;
        cx.notify();
        self.shared.spawn_blocking(
            cx,
            {
                // Inside mbar.app the default bar belongs to launchd (login item); a
                // direct start would run beside a broken or stopped login item.
                #[cfg(target_os = "macos")]
                let via_launchd = self.app.is_some();
                #[cfg(not(target_os = "macos"))]
                let via_launchd = false;
                move |client| -> Result<String, String> {
                    #[cfg(target_os = "macos")]
                    if via_launchd && client.bar_name() == mbar_ipc::DEFAULT_BAR_NAME {
                        return super::onboarding::start_login_item();
                    }
                    let _ = via_launchd;
                    sys::start_daemon(client.bar_name())
                        .map(|path| format!("Started {}", path.display()))
                        .map_err(|e| e.to_string())
                }
            },
            |this, result, cx| {
                cx.notify();
                match result {
                    Ok(msg) => {
                        this.check_status(cx);
                        Some(Notification::info(msg))
                    }
                    Err(e) => {
                        this.starting = false;
                        Some(Notification::error(e).title("Could not start mbar"))
                    }
                }
            },
        );
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.reloading = true;
        cx.notify();
        self.shared.spawn_blocking(
            cx,
            |client| client.reload(),
            |this, result, cx| {
                this.reloading = false;
                cx.notify();
                Some(match result {
                    Ok(_) => Notification::success("Configuration reloaded"),
                    Err(e) => Notification::error(e.to_string()).title("Reload failed"),
                })
            },
        );
    }

    fn set_menubar(&mut self, hide: bool, cx: &mut Context<Self>) {
        // Optimistic; re-read the real value afterwards.
        self.menubar_hidden = Some(hide);
        cx.notify();
        self.shared.spawn_blocking(
            cx,
            move |client| client.menubar(if hide { "hide" } else { "show" }),
            |this, result, cx| {
                this.read_menubar(cx);
                result
                    .err()
                    .map(|e| Notification::error(e.to_string()).title("Menu bar"))
            },
        );
    }

    #[cfg(target_os = "macos")]
    fn set_login_item(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        use crate::mac::login_item;
        let result = if on {
            login_item::register()
        } else {
            login_item::unregister()
        };
        let status = login_item::status();
        if let Some(app) = &mut self.app {
            app.login = status;
        }
        if let Err(e) = result {
            window.push_notification(Notification::error(e).title("Launch at login"), cx);
        } else if status == login_item::LoginItem::RequiresApproval {
            login_item::open_settings();
        }
        cx.notify();
    }

    #[cfg(target_os = "macos")]
    fn fix_paths_d(&mut self, cx: &mut Context<Self>) {
        let Some(app) = &mut self.app else { return };
        app.fixing_paths = true;
        let bin = app.bundle.bin_dir();
        cx.notify();
        self.shared.spawn_blocking(
            cx,
            move |_| super::onboarding::install_command_line(&bin, &[]),
            |this, result, cx| {
                if let Some(app) = &mut this.app {
                    app.fixing_paths = false;
                    app.paths_d = read_paths_d(&app.bundle.bin_dir());
                }
                cx.notify();
                Some(match result {
                    Ok(msg) => Notification::success(msg),
                    Err(e) => Notification::error(e).title("Command line"),
                })
            },
        );
    }

    /// Login item, `/etc/paths.d`, updates and "Run setup again" (mbar.app only).
    #[cfg(target_os = "macos")]
    fn app_sections(&self, cx: &mut Context<Self>) -> Option<Vec<Div>> {
        use crate::mac::login_item::LoginItem;
        use mbar_ui_model::onboarding::PathsD;
        let app = self.app.as_ref()?;
        let login = section("Launch at login", cx).child(setting_row(
            "Start mbar at login",
            match app.login {
                LoginItem::Enabled => "Registered as a login item (dev.rubeen.mbar)",
                LoginItem::RequiresApproval => {
                    "Waiting for approval in System Settings → General → Login Items"
                }
                _ => "Not registered",
            },
            Switch::new("launch-at-login")
                .checked(app.login == LoginItem::Enabled)
                .on_change(
                    cx.listener(|this, on: &bool, window, cx| this.set_login_item(*on, window, cx)),
                ),
            cx,
        ));
        let (paths_text, needs_fix) = match &app.paths_d {
            PathsD::Current => ("/etc/paths.d/mbar points to this app".to_string(), false),
            PathsD::Missing => ("/etc/paths.d/mbar is missing".to_string(), true),
            PathsD::Stale(old) => (format!("/etc/paths.d/mbar is stale ({old})"), true),
        };
        let command_line = section("Command line", cx).child(setting_row(
            "`mbar`, `sketchybar` and `borders` in new terminals",
            paths_text,
            Button::new("fix-paths-d")
                .small()
                .outline()
                .label("Fix")
                .loading(app.fixing_paths)
                .disabled(!needs_fix || app.fixing_paths)
                .on_click(cx.listener(|this, _, _, cx| this.fix_paths_d(cx))),
            cx,
        ));
        let updater = cx
            .try_global::<crate::mac::UpdaterGlobal>()
            .map(|g| g.0.clone())
            .filter(|u| u.is_some());
        let auto = updater
            .as_ref()
            .and_then(|u| u.as_ref().as_ref().map(|u| u.auto_checks()))
            .unwrap_or(false);
        let available = updater.is_some();
        let toggle = updater.clone();
        let check = updater.clone();
        let updates = section("Updates", cx)
            .child(setting_row(
                "Automatically check for updates",
                format!(
                    "mbar {} · {}",
                    app.bundle.short_version,
                    if available {
                        "once a day"
                    } else {
                        "updater unavailable (see log)"
                    }
                ),
                Switch::new("auto-updates")
                    .checked(auto)
                    .disabled(!available)
                    .on_change(cx.listener(move |_, on: &bool, _, cx| {
                        if let Some(u) = toggle.as_ref().and_then(|u| u.as_ref().as_ref()) {
                            u.set_auto_checks(*on);
                        }
                        cx.notify();
                    })),
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("check-updates")
                            .small()
                            .outline()
                            .icon(IconName::RefreshCw)
                            .label("Check for Updates…")
                            .disabled(!available)
                            .on_click(move |_, _, _| {
                                if let Some(u) = check.as_ref().and_then(|u| u.as_ref().as_ref()) {
                                    u.check_now();
                                }
                            }),
                    )
                    .child(
                        Button::new("run-setup")
                            .small()
                            .ghost()
                            .label("Run setup again")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ShowSetup))),
                    ),
            );
        Some(vec![login, command_line, updates])
    }
}

impl Render for SystemView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = self.status == DaemonStatus::Connected;
        let color = status_color(&self.status, cx);

        let daemon = section("Daemon", cx).child(
            h_flex()
                .gap_3()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .text_sm()
                                .child(div().size_3().flex_shrink_0().rounded_full().bg(color))
                                .child(format!(
                                    "{} · bar name `{}`",
                                    status_text(&self.status),
                                    self.shared.client.bar_name(),
                                )),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(format!(
                                    "Socket: {}",
                                    mbar_ipc::socket_path(self.shared.client.bar_name()).display()
                                )),
                        ),
                )
                .child(
                    Button::new("start-daemon")
                        .small()
                        .primary()
                        .icon(IconName::Play)
                        .label("Start mbar")
                        .loading(self.starting)
                        .disabled(connected)
                        .on_click(cx.listener(|this, _, _, cx| this.start_daemon(cx))),
                )
                .child(
                    Button::new("reload")
                        .small()
                        .outline()
                        .icon(IconName::RotateCw)
                        .label("Reload config")
                        .loading(self.reloading)
                        .disabled(!connected)
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                ),
        );

        let menubar = section("Native menu bar", cx).child(setting_row(
            "Automatically hide and show the macOS menu bar",
            "Sends `--menubar hide|show`; mbar updates the system setting.",
            Switch::new("menubar-autohide")
                .checked(self.menubar_hidden.unwrap_or(false))
                .disabled(!connected)
                .on_change(cx.listener(|this, on: &bool, _, cx| this.set_menubar(*on, cx))),
            cx,
        ));

        let borders_info = match &self.borders {
            Some(Ok(b)) => Some(b),
            _ => None,
        };
        let borders_status = match &self.borders {
            Some(Ok(b)) => b.summary(),
            Some(Err(e)) => format!("Unavailable: {e}"),
            None if connected => "Reading…".to_string(),
            None => "mbar is not running".to_string(),
        };
        let brew_borders = ob::brew_borders_warning(&self.brew).map(|warning| {
            h_flex()
                .w_full()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(cx.theme().red)
                        .whitespace_normal()
                        .child(warning),
                )
                .child(
                    Button::new("remove-brew-borders")
                        .small()
                        .outline()
                        .label("Stop and remove JankyBorders")
                        .loading(self.removing_brew_borders)
                        .disabled(self.removing_brew_borders)
                        .on_click(cx.listener(|this, _, _, cx| this.remove_brew_borders(cx))),
                )
        });
        let borders = section("Window borders", cx)
            .children(brew_borders)
            .child(setting_row(
                "Draw borders around windows",
                "Sends `--borders drawing=on|off`; the focused window gets the active color.",
                Switch::new("borders-drawing")
                    .checked(borders_info.is_some_and(|b| b.drawing))
                    .disabled(!connected || borders_info.is_none())
                    .on_change(cx.listener(|this, on: &bool, _, cx| this.set_borders(*on, cx))),
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .font_family("Menlo")
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_normal()
                    .child(borders_status),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_normal()
                    .child(
                        "Colors, width and style come from `mbar --borders …`, \
                         `mbar.borders{}` in init.lua or your JankyBorders bordersrc.",
                    ),
            );

        let aerospace_status = match &self.aerospace {
            Some(Ok(Some(a))) => a.summary(),
            Some(Ok(None)) => "AeroSpace not found (no AeroSpace.app, no aerospace.toml)".into(),
            Some(Err(e)) => format!("Unavailable: {e}"),
            None if connected => "Reading…".to_string(),
            None => "mbar is not running".to_string(),
        };
        let aerospace = section("AeroSpace", cx)
            .child(
                div()
                    .text_xs()
                    .font_family("Menlo")
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_normal()
                    .child(aerospace_status),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_normal()
                    .child(
                        "mbar receives AeroSpace's events itself: `aerospace_workspace_change` \
                         and the other `aerospace_*` events, `provider=aerospace` and \
                         `mbar.aerospace` in init.lua. No `exec-on-workspace-change` needed.",
                    ),
            );

        let perms = self.permissions.as_ref();
        let permissions = section("Permissions", cx)
            .child(permission_row(
                "open-accessibility",
                "Accessibility",
                "Needed by `app_menu` items to read and open the front app's menus.",
                perms.map(|p| &p.accessibility),
                ACCESSIBILITY_SETTINGS_URL,
                cx,
            ))
            .child(permission_row(
                "open-screen-recording",
                "Screen Recording",
                "Needed by alias items to capture menu bar extras.",
                perms.map(|p| &p.screen_recording),
                SCREEN_RECORDING_SETTINGS_URL,
                cx,
            ))
            .child(
                h_flex().child(
                    Button::new("recheck-permissions")
                        .small()
                        .ghost()
                        .icon(IconName::RefreshCw)
                        .label("Check again")
                        .disabled(!connected)
                        .on_click(cx.listener(|this, _, _, cx| this.probe_permissions(cx))),
                ),
            );

        #[cfg(target_os = "macos")]
        let app_sections = self.app_sections(cx);
        #[cfg(not(target_os = "macos"))]
        let app_sections: Option<Vec<Div>> = None;
        let tail = match app_sections {
            Some(sections) => sections,
            None => {
                let login_path = sys::launch_agent_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~/Library/LaunchAgents/dev.rubeen.mbar.plist".into());
                let legacy_login = section("Launch at login", cx).child(setting_row(
                    "Start mbar when you log in",
                    format!(
                        "Managed by setup; from a source build use `make install-agent` \
                         (launch agent: {login_path})"
                    ),
                    Switch::new("launch-at-login")
                        .checked(self.launch_agent)
                        .disabled(true),
                    cx,
                ));
                vec![legacy_login]
            }
        };

        v_flex()
            .size_full()
            .gap_3()
            .child(page_header(
                "System",
                "Daemon, permissions and macOS integration",
                cx,
            ))
            .child(
                v_flex()
                    .id("system-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_3()
                    .child(daemon)
                    .child(menubar)
                    .child(borders)
                    .child(aerospace)
                    .child(permissions)
                    .children(tail),
            )
    }
}

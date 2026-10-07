//! System: daemon status and start, reload, native menu bar auto-hide, permission
//! hints and launch at login.

use std::time::Duration;

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    switch::Switch,
    v_flex, ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
};
use gpui_kit::*;
use mbar_ui_model::ipc::DaemonStatus;
use mbar_ui_model::system::{
    self as sys, Permission, Permissions, ACCESSIBILITY_SETTINGS_URL, SCREEN_RECORDING_SETTINGS_URL,
};

use super::{interval, page_header, section, status_color, status_text, Shared};

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

pub struct SystemView {
    shared: Shared,
    status: DaemonStatus,
    checking: bool,
    starting: bool,
    reloading: bool,
    permissions: Option<Permissions>,
    menubar_hidden: Option<bool>,
    launch_agent: bool,
    _poll: Task<()>,
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
            launch_agent: sys::launch_agent_path().is_some_and(|p| p.exists()),
            _poll: poll,
        };
        view.check_status(cx);
        view.read_menubar(cx);
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
                if status != this.status {
                    if status != DaemonStatus::Connected {
                        // Probed through the daemon; stale once it is gone.
                        this.permissions = None;
                    }
                    this.status = status;
                    cx.notify();
                }
                if became_connected {
                    this.starting = false;
                    this.probe_permissions(cx);
                }
                None
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
            |client| sys::start_daemon(client.bar_name()),
            |this, result, cx| {
                cx.notify();
                match result {
                    Ok(path) => {
                        this.check_status(cx);
                        Some(Notification::info(format!("Started {}", path.display())))
                    }
                    Err(e) => {
                        this.starting = false;
                        Some(Notification::error(e.to_string()).title("Could not start mbar"))
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

    // TODO(Task 12/16, macOS): replaced by the SMAppService login item (`set_login_item`).
    // Until then the switch only reports the legacy agent; setup manages launch at login.
    fn set_launch_agent(&mut self, _on: bool, window: &mut Window, cx: &mut Context<Self>) {
        window.push_notification(
            Notification::info("Login item is managed by onboarding").title("Launch at login"),
            cx,
        );
        cx.notify();
    }

    fn open_settings(url: &'static str, window: &mut Window, cx: &mut App) {
        if let Err(e) = sys::open_url(url) {
            window.push_notification(
                Notification::error(e.to_string()).title("Could not open System Settings"),
                cx,
            );
        }
    }

    fn permission_row(
        &self,
        id: &'static str,
        title: &'static str,
        purpose: &'static str,
        state: Option<&Permission>,
        url: &'static str,
        cx: &App,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (text, color) = match state {
            Some(Permission::Granted) => ("Granted".to_string(), theme.green),
            Some(Permission::Missing) => ("Not granted".to_string(), theme.red),
            Some(Permission::Unknown(why)) => (format!("Unknown ({why})"), theme.muted_foreground),
            None => (
                "Unknown (mbar not running)".to_string(),
                theme.muted_foreground,
            ),
        };
        h_flex()
            .w_full()
            .gap_3()
            .py_1()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().font_semibold().child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(purpose),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .text_sm()
                    .child(div().size_2().rounded_full().bg(color))
                    .child(text),
            )
            .child(
                Button::new(id)
                    .small()
                    .outline()
                    .icon(IconName::ExternalLink)
                    .label("Open Settings")
                    .on_click(move |_, window, cx| Self::open_settings(url, window, cx)),
            )
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

        let perms = self.permissions.as_ref();
        let permissions = section("Permissions", cx)
            .child(self.permission_row(
                "open-accessibility",
                "Accessibility",
                "Needed by `app_menu` items to read and open the front app's menus.",
                perms.map(|p| &p.accessibility),
                ACCESSIBILITY_SETTINGS_URL,
                cx,
            ))
            .child(self.permission_row(
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

        let login_path = sys::launch_agent_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/Library/LaunchAgents/dev.rubeen.mbar.plist".into());
        let login = section("Launch at login", cx).child(setting_row(
            "Start mbar when you log in",
            format!(
                "Managed by setup; from a source build use `make install-agent` \
                 (launch agent: {login_path})"
            ),
            Switch::new("launch-at-login")
                .checked(self.launch_agent)
                .disabled(true)
                .on_change(cx.listener(|this, on: &bool, window, cx| {
                    this.set_launch_agent(*on, window, cx)
                })),
            cx,
        ));

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
                    .child(permissions)
                    .child(login),
            )
    }
}

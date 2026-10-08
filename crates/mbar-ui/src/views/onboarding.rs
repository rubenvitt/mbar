//! First-launch setup inside mbar.app: one section per step with its state, a Run and a
//! Skip button and the output of the last run. Detection runs once in the background;
//! every step runs on the background executor.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    switch::Switch,
    v_flex, ActiveTheme as _, Disableable as _, Sizable as _,
};
use gpui_kit::*;
use mbar_ui_model::ipc::DaemonStatus;
use mbar_ui_model::onboarding as ob;
use mbar_ui_model::system::{
    self as sys, Permission, Permissions, ACCESSIBILITY_SETTINGS_URL, SCREEN_RECORDING_SETTINGS_URL,
};

use super::{interval, page_header, permission_row, section, Shared};
use crate::mac::login_item::{self, LoginItem};

/// `defaults` key set once every step is done or skipped.
const COMPLETED_KEY: &str = "OnboardingCompleted";

/// Whether setup finished on this Mac (`defaults read dev.rubeen.mbar OnboardingCompleted`).
pub fn completed() -> bool {
    std::process::Command::new("/usr/bin/defaults")
        .args(["read", mbar_app::BUNDLE_ID, COMPLETED_KEY])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| sys::parse_defaults_bool(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false)
}

fn mark_completed() {
    let _ = std::process::Command::new("/usr/bin/defaults")
        .args(["write", mbar_app::BUNDLE_ID, COMPLETED_KEY, "-bool", "true"])
        .status();
}

#[derive(Clone, Debug, PartialEq)]
enum StepState {
    Pending,
    Running,
    Done(String),
    Failed(String),
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Location,
    Cleanup,
    TakeOver,
    Starter,
    CommandLine,
    LoginItem,
    Permissions,
}

impl Step {
    /// Same order as `ob::SetupStep::ALL` (cleanup before the login item).
    const ALL: [Step; 7] = [
        Step::Location,
        Step::Cleanup,
        Step::TakeOver,
        Step::Starter,
        Step::CommandLine,
        Step::LoginItem,
        Step::Permissions,
    ];

    fn title(self) -> &'static str {
        match self {
            Step::Location => "Move to Applications",
            Step::Cleanup => "Remove SketchyBar and old mbar installs",
            Step::TakeOver => "Use your SketchyBar config",
            Step::Starter => "Create a starter config",
            Step::CommandLine => "Install the `mbar` and `sketchybar` commands",
            Step::LoginItem => "Start mbar at login",
            Step::Permissions => "Grant permissions",
        }
    }

    fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// Everything detected once when the page opens.
#[derive(Clone, Debug)]
struct SetupPlan {
    bundle_root: Option<PathBuf>,
    bin_dir: Option<PathBuf>,
    location: Option<ob::Location>,
    old: Vec<ob::OldInstall>,
    brew: ob::BrewState,
    paths_d: ob::PathsD,
    sbarlua: Option<(PathBuf, String)>,
    felix: Vec<PathBuf>,
    starter_needed: bool,
    login: LoginItem,
}

impl SetupPlan {
    fn detect() -> SetupPlan {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        let xdg = std::env::var("XDG_CONFIG_HOME").unwrap_or_default();
        let bundle = sys::current_bundle();
        let root = bundle.as_ref().map(|b| b.root.clone());
        let bin = bundle.as_ref().map(|b| b.bin_dir());
        let brew = ob::find_brew()
            .map(|b| {
                let out = |args: &[&str]| {
                    std::process::Command::new(&b)
                        .args(args)
                        .stderr(std::process::Stdio::null())
                        .output()
                        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                        .unwrap_or_default()
                };
                ob::parse_brew(&out(&["list", "--formula"]), &out(&["services", "list"]))
            })
            .unwrap_or_default();
        let sb = home.join(".config/sketchybar");
        let sbarlua = (!sb.join("init.lua").exists())
            .then(|| std::fs::read_to_string(sb.join("sketchybarrc")).ok())
            .flatten()
            .and_then(|rc| ob::sbarlua_init_lua(&rc))
            .map(|body| (sb.join("init.lua"), body));
        SetupPlan {
            location: root.as_deref().map(ob::location),
            old: ob::find_old_installs(
                &home,
                std::path::Path::new("/usr/local/bin"),
                root.as_deref(),
            ),
            paths_d: bin
                .as_deref()
                .map(|b| {
                    ob::paths_d_state(
                        std::fs::read_to_string("/etc/paths.d/mbar").ok().as_deref(),
                        b,
                    )
                })
                .unwrap_or(ob::PathsD::Missing),
            felix: ob::felix_helpers(&sb),
            starter_needed: ob::needs_starter_config(&home, &xdg),
            bundle_root: root,
            bin_dir: bin,
            brew,
            sbarlua,
            login: login_item::status(),
        }
    }

    fn brew_has_work(&self) -> bool {
        self.brew.sketchybar_installed || self.brew.sketchybar_running || self.brew.mbar_installed
    }

    /// Steps detection already finds done.
    fn nothing_to_do(&self, step: Step) -> bool {
        match step {
            Step::Location => self.location == Some(ob::Location::Applications),
            Step::Cleanup => {
                !self.brew_has_work() && self.old.iter().all(|o| o.kind == ob::OldKind::Foreign)
            }
            Step::TakeOver => self.sbarlua.is_none() && self.felix.is_empty(),
            Step::Starter => !self.starter_needed,
            Step::CommandLine => self.paths_d == ob::PathsD::Current,
            Step::LoginItem => self.login == LoginItem::Enabled,
            Step::Permissions => false,
        }
    }
}

/// The admin part of "Install the commands": `/etc/paths.d/mbar` plus admin-owned old
/// binaries, then a check that a new login shell finds `sketchybar` in the bundle.
/// Shared with the System page's "Fix" button.
pub fn install_command_line(
    bin: &std::path::Path,
    old: &[ob::OldInstall],
) -> Result<String, String> {
    let admin: Vec<PathBuf> = old
        .iter()
        .filter(|o| o.admin && o.kind == ob::OldKind::Binary)
        .map(|o| o.path.clone())
        .collect();
    ob::run_admin(&ob::admin_shell(bin, &admin))?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let out = std::process::Command::new(&shell)
        .args(["-lc", "command -v sketchybar"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let found = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if found.starts_with(&*bin.to_string_lossy()) {
        Ok(format!("sketchybar → {found}"))
    } else {
        Err(format!(
            "A new {shell} login shell finds sketchybar at '{found}'. Something earlier on \
             your PATH shadows mbar."
        ))
    }
}

pub struct SetupView {
    shared: Shared,
    states: Vec<(Step, StepState)>,
    plan: Option<SetupPlan>,
    remove_brew_sketchybar: bool,
    permissions: Option<Permissions>,
    probing: bool,
    /// `OnboardingCompleted` was written (once per view).
    completed_written: bool,
    _poll: Task<()>,
}

impl SetupView {
    pub fn new(shared: Shared, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Permissions change in System Settings; keep the row live.
        let poll = interval(cx, Duration::from_secs(2), |this: &mut Self, cx| {
            this.probe_permissions(cx)
        });
        let mut view = SetupView {
            shared,
            states: Step::ALL.iter().map(|s| (*s, StepState::Pending)).collect(),
            plan: None,
            remove_brew_sketchybar: true,
            permissions: None,
            probing: false,
            completed_written: false,
            _poll: poll,
        };
        view.redetect(cx);
        view
    }

    fn redetect(&mut self, cx: &mut Context<Self>) {
        self.shared.spawn_blocking(
            cx,
            |_| SetupPlan::detect(),
            |this, plan, cx| {
                for (step, state) in &mut this.states {
                    if *state == StepState::Pending && plan.nothing_to_do(*step) {
                        *state = StepState::Done("Nothing to do".into());
                    }
                }
                this.plan = Some(plan);
                this.check_completed();
                cx.notify();
                None
            },
        );
    }

    fn probe_permissions(&mut self, cx: &mut Context<Self>) {
        if self.probing {
            return;
        }
        self.probing = true;
        self.shared.spawn_blocking(
            cx,
            |client| {
                (client.status() == DaemonStatus::Connected)
                    .then(|| sys::probe_permissions(&client))
            },
            |this, permissions, cx| {
                this.probing = false;
                let granted = |p: &Permissions| {
                    p.accessibility == Permission::Granted
                        && p.screen_recording == Permission::Granted
                };
                if permissions.as_ref().is_some_and(granted)
                    && this.state(Step::Permissions) == &StepState::Pending
                {
                    this.set(
                        Step::Permissions,
                        StepState::Done("Both granted".into()),
                        cx,
                    );
                }
                if permissions != this.permissions {
                    this.permissions = permissions;
                    cx.notify();
                }
                None
            },
        );
    }

    fn state(&self, step: Step) -> &StepState {
        &self.states[step.index()].1
    }

    fn set(&mut self, step: Step, state: StepState, cx: &mut Context<Self>) {
        self.states[step.index()].1 = state;
        self.check_completed();
        cx.notify();
    }

    fn check_completed(&mut self) {
        let all = self
            .states
            .iter()
            .all(|(_, s)| matches!(s, StepState::Done(_) | StepState::Skipped));
        if all && !self.completed_written {
            self.completed_written = true;
            std::thread::spawn(mark_completed);
        }
    }

    fn run_step(&mut self, step: Step, cx: &mut Context<Self>) {
        let Some(plan) = self.plan.clone() else {
            return;
        };
        let remove_brew = self.remove_brew_sketchybar;
        self.set(step, StepState::Running, cx);
        self.shared.spawn_blocking(
            cx,
            move |_| run(step, &plan, remove_brew),
            move |this, result, cx| {
                this.set(
                    step,
                    match result {
                        Ok(log) => StepState::Done(log),
                        Err(e) => StepState::Failed(e),
                    },
                    cx,
                );
                // Later steps depend on what this one changed (old installs, paths.d).
                this.redetect(cx);
                None
            },
        );
    }

    fn render_step(&self, step: Step, cx: &mut Context<Self>) -> Div {
        let state = self.state(step);
        let theme = cx.theme();
        let (label, color) = match state {
            StepState::Pending => ("To do", theme.muted_foreground),
            StepState::Running => ("Running…", theme.yellow),
            StepState::Done(_) => ("Done", theme.green),
            StepState::Failed(_) => ("Failed", theme.red),
            StepState::Skipped => ("Skipped", theme.muted_foreground),
        };
        let output = match state {
            StepState::Done(s) | StepState::Failed(s) => s.trim().to_string(),
            _ => String::new(),
        };
        let busy = *state == StepState::Running;
        let done = matches!(state, StepState::Done(_));
        let i = step.index();
        let mut body = section(step.title(), cx)
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(div().size_2().rounded_full().bg(color))
                    .child(label),
            )
            .child(self.details(step, cx));
        if !output.is_empty() {
            body = body.child(
                div()
                    .text_xs()
                    .font_family("Menlo")
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_normal()
                    .child(output),
            );
        }
        let actions = if step == Step::Permissions {
            h_flex().gap_2().child(
                Button::new(("restart", i))
                    .small()
                    .outline()
                    .label("Restart mbar")
                    .tooltip("Screen Recording takes effect after a restart")
                    .on_click(cx.listener(|_, _, _, _| {
                        let _ = sys::kickstart_daemon();
                    })),
            )
        } else {
            h_flex().gap_2().child(
                Button::new(("run", i))
                    .small()
                    .primary()
                    .label(if matches!(state, StepState::Failed(_)) {
                        "Retry"
                    } else {
                        "Run"
                    })
                    .loading(busy)
                    .disabled(busy || done || self.plan.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| this.run_step(step, cx))),
            )
        };
        body.child(
            actions.child(
                Button::new(("skip", i))
                    .small()
                    .ghost()
                    .label("Skip")
                    .disabled(busy || done)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set(step, StepState::Skipped, cx)),
                    ),
            ),
        )
    }

    /// What the step will do, from the detection.
    fn details(&self, step: Step, cx: &mut Context<Self>) -> Div {
        let muted = cx.theme().muted_foreground;
        let line = |s: String| div().text_xs().text_color(muted).child(s);
        let Some(plan) = &self.plan else {
            return v_flex().child(line("Checking this Mac…".into()));
        };
        let mut col = v_flex().gap_1();
        match step {
            Step::Location => {
                let at = plan
                    .bundle_root
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                col = col.child(line(match plan.location {
                    Some(ob::Location::Applications) => {
                        format!("mbar.app is in /Applications ({at}).")
                    }
                    _ => {
                        format!("Copies mbar.app from {at} to /Applications and restarts it there.")
                    }
                }));
            }
            Step::Cleanup => {
                if plan.brew_has_work() {
                    col = col.child(line(format!(
                        "Homebrew: sketchybar {}{}, mbar formula {}",
                        if plan.brew.sketchybar_installed {
                            "installed"
                        } else {
                            "not installed"
                        },
                        if plan.brew.sketchybar_running {
                            " (service running)"
                        } else {
                            ""
                        },
                        if plan.brew.mbar_installed {
                            "installed"
                        } else {
                            "not installed"
                        },
                    )));
                    if plan.brew.sketchybar_installed {
                        col = col.child(
                            Switch::new("remove-brew-sketchybar")
                                .small()
                                .label("Also uninstall Homebrew sketchybar")
                                .checked(self.remove_brew_sketchybar)
                                .on_change(cx.listener(|this, on: &bool, _, cx| {
                                    this.remove_brew_sketchybar = *on;
                                    cx.notify();
                                })),
                        );
                    }
                }
                for o in &plan.old {
                    let what = match o.kind {
                        ob::OldKind::Binary if o.admin => "removed with the commands step (admin)",
                        ob::OldKind::Binary => "removed",
                        ob::OldKind::LaunchAgent => "stopped and removed",
                        ob::OldKind::Foreign => "not mbar, left in place",
                    };
                    col = col.child(line(format!("{}: {what}", o.path.display())));
                }
            }
            Step::TakeOver => {
                if let Some((path, _)) = &plan.sbarlua {
                    col = col.child(line(format!(
                        "Your sketchybarrc starts SbarLua; writes {} so mbar runs the Lua config directly.",
                        path.display()
                    )));
                }
                for f in &plan.felix {
                    col = col.child(line(format!(
                        "{} looks up SketchyBar's mach port (git.felix.*); change it to dev.rubeen.mbar.",
                        f.display()
                    )));
                }
            }
            Step::Starter => {
                col = col.child(line(
                    "Writes ~/.config/mbar/init.lua when no config exists.".into(),
                ));
            }
            Step::CommandLine => {
                col = col.child(line(match &plan.paths_d {
                    ob::PathsD::Current => "/etc/paths.d/mbar points to this app.".into(),
                    ob::PathsD::Missing => {
                        "Writes /etc/paths.d/mbar (asks for your password).".into()
                    }
                    ob::PathsD::Stale(old) => format!(
                        "/etc/paths.d/mbar points to {old}; updates it (asks for your password)."
                    ),
                }));
            }
            Step::LoginItem => {
                col = col.child(line(match plan.login {
                    LoginItem::Enabled => "mbar is registered as a login item.".into(),
                    LoginItem::RequiresApproval => {
                        "Waiting for approval in System Settings → General → Login Items.".into()
                    }
                    _ => "Registers mbar as a login item and starts it.".into(),
                }));
            }
            Step::Permissions => {
                let perms = self.permissions.as_ref();
                col = col
                    .child(permission_row(
                        "setup-accessibility",
                        "Accessibility",
                        "Needed by `app_menu` items to read and open the front app's menus.",
                        perms.map(|p| &p.accessibility),
                        ACCESSIBILITY_SETTINGS_URL,
                        cx,
                    ))
                    .child(permission_row(
                        "setup-screen-recording",
                        "Screen Recording",
                        "Needed by alias items to capture menu bar extras.",
                        perms.map(|p| &p.screen_recording),
                        SCREEN_RECORDING_SETTINGS_URL,
                        cx,
                    ));
            }
        }
        col
    }
}

/// Runs one step (background thread).
fn run(step: Step, plan: &SetupPlan, remove_brew: bool) -> Result<String, String> {
    match step {
        Step::Location => {
            let src = plan
                .bundle_root
                .as_ref()
                .ok_or("not running from mbar.app")?;
            let dst = crate::mac::app::move_to_applications(src)?;
            crate::mac::app::relaunch(&dst)
        }
        Step::Cleanup => {
            let uid = sys::current_uid();
            let was_registered = login_item::status() == LoginItem::Enabled;
            let cmds = ob::cleanup_commands(
                &plan.old,
                &plan.brew,
                ob::find_brew().as_deref(),
                uid,
                remove_brew,
            );
            let result = ob::run_commands(&cmds);
            // The legacy agent shares the launchd label; its bootout stopped ours too,
            // even when a later command failed.
            let reregister = (was_registered && ob::cleanup_stops_login_item(&plan.old))
                .then(login_item::register);
            let mut log = result?;
            if let Some(r) = reregister {
                r?;
                log.push_str("\nRe-registered the login item");
            }
            let foreign: Vec<_> = plan
                .old
                .iter()
                .filter(|o| o.kind == ob::OldKind::Foreign)
                .map(|o| o.path.display().to_string())
                .collect();
            if !foreign.is_empty() {
                log.push_str(&format!(
                    "\nLeft in place (not mbar): {}",
                    foreign.join(", ")
                ));
            }
            Ok(log)
        }
        Step::TakeOver => {
            let mut log = String::new();
            if let Some((path, body)) = &plan.sbarlua {
                std::fs::write(path, body).map_err(|e| format!("{}: {e}", path.display()))?;
                log.push_str(&format!("Wrote {}\n", path.display()));
            }
            for f in &plan.felix {
                log.push_str(&format!("Still to change by hand: {}\n", f.display()));
            }
            Ok(log)
        }
        Step::Starter => {
            let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
            let dir = home.join(".config/mbar");
            let file = dir.join("init.lua");
            if file.exists() {
                return Ok(format!("{} exists, left unchanged", file.display()));
            }
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(&file, ob::STARTER_INIT_LUA).map_err(|e| e.to_string())?;
            Ok(format!("Wrote {}", file.display()))
        }
        Step::CommandLine => {
            let bin = plan.bin_dir.as_ref().ok_or("not running from mbar.app")?;
            install_command_line(bin, &plan.old)
        }
        Step::LoginItem => {
            login_item::register()?;
            match login_item::status() {
                LoginItem::Enabled => Ok("Registered; mbar is running".into()),
                LoginItem::RequiresApproval => {
                    login_item::open_settings();
                    Err("Allow mbar in System Settings → General → Login Items, then retry".into())
                }
                s => Err(format!("Login item status: {s:?}")),
            }
        }
        // Rendered live from the permission probe.
        Step::Permissions => Ok(String::new()),
    }
}

impl Render for SetupView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let steps: Vec<Div> = Step::ALL.iter().map(|s| self.render_step(*s, cx)).collect();
        v_flex()
            .size_full()
            .gap_3()
            .child(page_header("Setup", "Get mbar running on this Mac", cx))
            .child(
                v_flex()
                    .id("setup-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_3()
                    .children(steps),
            )
    }
}

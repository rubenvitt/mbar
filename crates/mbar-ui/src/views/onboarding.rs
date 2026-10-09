//! First-launch setup inside mbar.app: one section per step with its state, a Run and a
//! Skip button and the output of the last run. Detection runs once in the background;
//! every step runs on the background executor.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
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
            Step::Cleanup => "Remove SketchyBar, JankyBorders and old mbar installs",
            Step::TakeOver => "Use your SketchyBar, JankyBorders and AeroSpace configs",
            Step::Starter => "Create a starter config",
            Step::CommandLine => "Install the `mbar`, `sketchybar` and `borders` commands",
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
    /// JankyBorders config, run in place by mbar.
    bordersrc: Option<PathBuf>,
    /// Files under `~/.config/borders` that use `git.felix.borders`.
    borders_helpers: Vec<PathBuf>,
    /// Window-manager config lines that start `borders`.
    launch_lines: Vec<ob::LaunchLine>,
    /// AeroSpace `exec-on-workspace-change` settings that run the SketchyBar trigger.
    aerospace_triggers: Vec<ob::WorkspaceTrigger>,
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
        let brew = ob::detect_brew(ob::find_brew().as_deref());
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
            bordersrc: ob::bordersrc(&home),
            borders_helpers: ob::borders_helpers(&home.join(".config/borders")),
            launch_lines: ob::borders_launch_lines(&home),
            aerospace_triggers: ob::aerospace_triggers(&home),
            starter_needed: ob::needs_starter_config(&home, &xdg),
            bundle_root: root,
            bin_dir: bin,
            brew,
            sbarlua,
            login: login_item::status(),
        }
    }

    fn brew_has_work(&self) -> bool {
        self.brew.sketchybar_installed
            || self.brew.sketchybar_running
            || self.brew.mbar_installed
            || self.brew.borders_has_work()
    }

    /// What to do with one window-manager line that starts `borders`.
    fn launch_line_advice(&self, l: &ob::LaunchLine) -> String {
        ob::launch_line_advice(l.launch, self.bordersrc.as_deref(), self.bin_dir.as_deref())
    }

    /// JankyBorders leftovers the user may have to edit by hand. A `bordersrc` alone
    /// needs nothing: mbar runs it in place (the details still say so).
    fn borders_needs_review(&self) -> bool {
        !self.borders_helpers.is_empty() || !self.launch_lines.is_empty()
    }

    /// Steps detection already finds done.
    fn nothing_to_do(&self, step: Step) -> bool {
        match step {
            Step::Location => self.location == Some(ob::Location::Applications),
            Step::Cleanup => {
                // A Homebrew link goes with its formula, which `brew_has_work` covers.
                !self.brew_has_work()
                    && self
                        .old
                        .iter()
                        .all(|o| matches!(o.kind, ob::OldKind::Foreign | ob::OldKind::Homebrew))
            }
            Step::TakeOver => {
                self.sbarlua.is_none()
                    && self.felix.is_empty()
                    && !self.borders_needs_review()
                    && self.aerospace_triggers.is_empty()
            }
            Step::Starter => !self.starter_needed,
            Step::CommandLine => self.paths_d == ob::PathsD::Current,
            Step::LoginItem => self.login == LoginItem::Enabled,
            Step::Permissions => false,
        }
    }
}

/// The admin part of "Install the commands": `/etc/paths.d/mbar` plus admin-owned old
/// binaries, then a check that a new login shell finds `sketchybar` and `borders` in
/// the bundle. Shared with the System page's "Fix" button.
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
    let mut found = Vec::new();
    for name in ob::CHECKED_COMMANDS {
        let out = std::process::Command::new(&shell)
            .args(["-lc", &format!("command -v {name}")])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| e.to_string())?;
        found.push(ob::check_command(
            bin,
            &shell,
            name,
            &String::from_utf8_lossy(&out.stdout),
        )?);
    }
    Ok(found.join("\n"))
}

pub struct SetupView {
    shared: Shared,
    states: Vec<(Step, StepState)>,
    plan: Option<SetupPlan>,
    remove_brew_sketchybar: bool,
    /// Default on: a Homebrew `borders` earlier on the PATH shadows mbar's link.
    remove_brew_borders: bool,
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
            remove_brew_borders: true,
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
        let remove_brew = RemoveBrew {
            sketchybar: self.remove_brew_sketchybar,
            borders: self.remove_brew_borders,
        };
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
                    // `launchctl kickstart -k` waits for the old daemon to exit (and
                    // for launchd's spawn throttle): never on the UI thread.
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.shared.spawn_blocking(
                            cx,
                            |_| sys::kickstart_daemon(),
                            |_, result, _| {
                                result.err().map(|e| {
                                    Notification::error(e.to_string()).title("Restart mbar")
                                })
                            },
                        );
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
                let b = &plan.brew;
                if b.sketchybar_installed || b.sketchybar_running || b.mbar_installed {
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
                if b.borders_has_work() {
                    col = col.child(line(format!(
                        "Homebrew: borders (JankyBorders) {}{}; mbar draws the window borders now",
                        if b.borders_installed {
                            "installed"
                        } else {
                            "not installed"
                        },
                        if b.borders_running {
                            " (service running; the cleanup stops it)"
                        } else {
                            ""
                        },
                    )));
                    if b.borders_installed {
                        col = col
                            .child(
                                Switch::new("remove-brew-borders")
                                    .small()
                                    .label("Also uninstall Homebrew borders")
                                    .checked(self.remove_brew_borders)
                                    .on_change(cx.listener(|this, on: &bool, _, cx| {
                                        this.remove_brew_borders = *on;
                                        cx.notify();
                                    })),
                            )
                            .child(line(
                                "Recommended: a Homebrew `borders` earlier on your PATH would \
                                 shadow mbar's `borders` command."
                                    .into(),
                            ));
                    }
                }
                for o in &plan.old {
                    let what = ob::old_install_fate(
                        o,
                        &plan.brew,
                        self.remove_brew_sketchybar,
                        self.remove_brew_borders,
                    );
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
                if let Some(rc) = &plan.bordersrc {
                    col = col.child(line(format!(
                        "{} is used in place: mbar runs it after its own config, nothing to copy.",
                        rc.display()
                    )));
                }
                for f in &plan.borders_helpers {
                    col = col.child(line(format!(
                        "{} talks to JankyBorders' mach port (git.felix.borders), which mbar does \
                         not provide; run `borders …` or `mbar --borders …` instead.",
                        f.display()
                    )));
                }
                if !plan.launch_lines.is_empty() {
                    col = col.child(line("Your window manager starts borders here:".into()));
                    for l in &plan.launch_lines {
                        col = col
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("Menlo")
                                    .text_color(muted)
                                    .whitespace_normal()
                                    .child(format!("{}:{}: {}", l.file.display(), l.line, l.text)),
                            )
                            .child(line(plan.launch_line_advice(l)));
                    }
                    col = col.child(line(ob::LAUNCH_LINE_DOCS.into()));
                }
                if !plan.aerospace_triggers.is_empty() {
                    col = col.child(line(
                        "AeroSpace runs the SketchyBar trigger on every workspace change here:"
                            .into(),
                    ));
                    for t in &plan.aerospace_triggers {
                        col = col
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("Menlo")
                                    .text_color(muted)
                                    .whitespace_normal()
                                    .child(format!("{}:{}: {}", t.file.display(), t.line, t.text)),
                            )
                            .child(line(t.advice()));
                    }
                    col = col.child(line(ob::AEROSPACE_TRIGGER_DOCS.into()));
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
                col = col.child(line(
                    "Then checks that a new login shell finds `sketchybar` and `borders` in this \
                     app."
                        .into(),
                ));
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

/// Starts the default bar through its login item: registers it when needed, kicks the
/// launchd job and registers it from scratch when launchd still cannot spawn it.
pub fn start_login_item() -> Result<String, String> {
    if login_item::status() != LoginItem::Enabled {
        login_item::register()?;
    }
    let _ = sys::kickstart_daemon();
    if daemon_running() {
        return Ok("Started mbar (login item)".into());
    }
    register_fresh()?;
    if daemon_running() {
        Ok("Registered the login item again; mbar is running".into())
    } else {
        Err("launchd does not start mbar; see ~/Library/Logs/mbar.log".into())
    }
}

/// Unregisters and registers the login item again, then gives launchd a moment.
fn register_fresh() -> Result<(), String> {
    let _ = login_item::unregister();
    login_item::register()?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    Ok(())
}

/// Whether launchd runs the login item's job (`launchctl print gui/<uid>/dev.rubeen.mbar`).
fn daemon_running() -> bool {
    std::thread::sleep(std::time::Duration::from_secs(1));
    std::process::Command::new("/bin/launchctl")
        .arg("print")
        .arg(format!(
            "gui/{}/{}",
            sys::current_uid(),
            mbar_app::BUNDLE_ID
        ))
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l.trim() == "state = running")
        })
}

/// The cleanup's "Also uninstall Homebrew …" switches.
#[derive(Clone, Copy, Debug)]
struct RemoveBrew {
    sketchybar: bool,
    borders: bool,
}

/// Runs one step (background thread).
fn run(step: Step, plan: &SetupPlan, remove_brew: RemoveBrew) -> Result<String, String> {
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
                remove_brew.sketchybar,
                remove_brew.borders,
            );
            let result = ob::run_commands(&cmds);
            // The legacy agent shares the launchd label; its bootout stopped ours too,
            // even when a later command failed.
            // Removing the legacy plist also drops launchd's background-task record
            // for the label, so a plain `register()` would leave a job that cannot
            // spawn ("Unable to resolve <BTM uuid>"): register from scratch.
            let reregister =
                (was_registered && ob::cleanup_stops_login_item(&plan.old)).then(register_fresh);
            let mut log = result?;
            if let Some(r) = reregister {
                r?;
                log.push_str("\nRe-registered the login item");
            }
            let foreign: Vec<_> = plan
                .old
                .iter()
                .filter(|o| {
                    ob::left_in_place(o, &plan.brew, remove_brew.sketchybar, remove_brew.borders)
                })
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
            if let Some(rc) = &plan.bordersrc {
                log.push_str(&format!("mbar runs {} in place\n", rc.display()));
            }
            for f in &plan.borders_helpers {
                log.push_str(&format!(
                    "Still to change by hand (git.felix.borders): {}\n",
                    f.display()
                ));
            }
            for l in &plan.launch_lines {
                log.push_str(&format!(
                    "Starts borders: {}:{}: {}\n  {}\n",
                    l.file.display(),
                    l.line,
                    l.text,
                    plan.launch_line_advice(l)
                ));
            }
            if !plan.launch_lines.is_empty() {
                log.push_str(ob::LAUNCH_LINE_DOCS);
                log.push('\n');
            }
            for t in &plan.aerospace_triggers {
                log.push_str(&format!(
                    "Runs the SketchyBar trigger: {}:{}: {}\n  {}\n",
                    t.file.display(),
                    t.line,
                    t.text,
                    t.advice()
                ));
            }
            if !plan.aerospace_triggers.is_empty() {
                log.push_str(ob::AEROSPACE_TRIGGER_DOCS);
                log.push('\n');
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
            if login_item::status() == LoginItem::Enabled && !daemon_running() {
                register_fresh()?;
            }
            match login_item::status() {
                LoginItem::Enabled if daemon_running() => Ok("Registered; mbar is running".into()),
                LoginItem::Enabled => Err(
                    "Registered, but launchd does not start mbar; see ~/Library/Logs/mbar.log"
                        .into(),
                ),
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

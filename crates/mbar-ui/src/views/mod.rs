//! Window content: sidebar navigation and the five pages.

mod bar;
mod events;
mod inspector;
mod perf;
mod props;
mod system;

use std::time::Duration;

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::{
    h_flex,
    notification::Notification,
    sidebar::{Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem},
    v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, Theme, WindowExt as _,
};
use gpui_kit::*;
use mbar_ui_model::ipc::{Client, DaemonStatus};

pub use bar::BarView;
pub use events::EventsView;
pub use inspector::InspectorView;
pub use perf::PerfView;
pub use system::SystemView;

/// What every page needs: the IPC client and the window (for notifications pushed
/// from background completions).
#[derive(Clone)]
pub struct Shared {
    pub client: Client,
    pub window: AnyWindowHandle,
}

impl Shared {
    /// Runs blocking `work` on the background executor, then `done` on the UI thread
    /// with the entity. A notification returned by `done` is shown in the window.
    pub fn spawn_blocking<T, R>(
        &self,
        cx: &mut Context<T>,
        work: impl FnOnce(Client) -> R + Send + 'static,
        done: impl FnOnce(&mut T, R, &mut Context<T>) -> Option<Notification> + 'static,
    ) where
        T: 'static,
        R: Send + 'static,
    {
        let client = self.client.clone();
        let window = self.window;
        let bg = cx.background_executor().spawn(async move { work(client) });
        cx.spawn(async move |this, cx| {
            let result = bg.await;
            if let Ok(Some(note)) = this.update(cx, |this, cx| done(this, result, cx)) {
                let _ = window.update(cx, |_, window, cx| window.push_notification(note, cx));
            }
        })
        .detach();
    }
}

/// Starts a loop calling `tick` every `every` while the returned task lives.
pub fn interval<T: 'static>(
    cx: &mut Context<T>,
    every: Duration,
    tick: impl Fn(&mut T, &mut Context<T>) + 'static,
) -> Task<()> {
    cx.spawn(async move |this, cx| loop {
        cx.background_executor().timer(every).await;
        if this.update(cx, |this, cx| tick(this, cx)).is_err() {
            break;
        }
    })
}

/// A titled block on a page.
pub fn section(title: impl Into<SharedString>, cx: &App) -> Div {
    v_flex()
        .gap_2()
        .p_3()
        .border_1()
        .border_color(cx.theme().border)
        .rounded(cx.theme().radius)
        .bg(cx.theme().background)
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(title.into()),
        )
}

/// A small "label / big value" tile.
pub fn stat_tile(label: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> Div {
    v_flex()
        .min_w(px(120.))
        .flex_1()
        .gap_1()
        .p_3()
        .border_1()
        .border_color(cx.theme().border)
        .rounded(cx.theme().radius)
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.into()),
        )
        .child(div().text_lg().font_semibold().child(value.into()))
}

/// Page header with a title, a muted subtitle and trailing controls.
pub fn page_header(
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    cx: &App,
) -> Div {
    h_flex()
        .w_full()
        .gap_3()
        .pb_2()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_lg().font_semibold().child(title.into()))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(subtitle.into()),
                ),
        )
}

pub fn status_color(status: &DaemonStatus, cx: &App) -> Hsla {
    match status {
        DaemonStatus::Connected => cx.theme().green,
        DaemonStatus::NotRunning => cx.theme().red,
        DaemonStatus::Error(_) => cx.theme().yellow,
        DaemonStatus::Unknown => cx.theme().muted_foreground,
    }
}

pub fn status_text(status: &DaemonStatus) -> String {
    match status {
        DaemonStatus::Connected => "Connected".into(),
        DaemonStatus::NotRunning => "Not running".into(),
        DaemonStatus::Error(e) => format!("Error: {e}"),
        DaemonStatus::Unknown => "Checking…".into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Inspector,
    Bar,
    Events,
    Performance,
    System,
}

impl Page {
    const ALL: [Page; 5] = [
        Page::Inspector,
        Page::Bar,
        Page::Events,
        Page::Performance,
        Page::System,
    ];

    fn label(self) -> &'static str {
        match self {
            Page::Inspector => "Inspector",
            Page::Bar => "Bar",
            Page::Events => "Events",
            Page::Performance => "Performance",
            Page::System => "System",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Page::Inspector => IconName::Inspector,
            Page::Bar => IconName::PanelBottom,
            Page::Events => IconName::Bell,
            Page::Performance => IconName::Cpu,
            Page::System => IconName::Settings,
        }
    }
}

pub struct AppView {
    shared: Shared,
    page: Page,
    inspector: Entity<InspectorView>,
    bar: Entity<BarView>,
    events: Entity<EventsView>,
    perf: Entity<PerfView>,
    system: Entity<SystemView>,
    _subscriptions: Vec<Subscription>,
}

impl AppView {
    pub fn new(client: Client, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shared = Shared {
            client,
            window: window.window_handle(),
        };
        let inspector = cx.new(|cx| InspectorView::new(shared.clone(), window, cx));
        let bar = cx.new(|cx| BarView::new(shared.clone(), window, cx));
        let events = cx.new(|cx| EventsView::new(shared.clone(), window, cx));
        let perf = cx.new(|cx| PerfView::new(shared.clone(), window, cx));
        let system = cx.new(|cx| SystemView::new(shared.clone(), window, cx));

        let mut subs = vec![
            cx.observe_window_appearance(window, |_, window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
            }),
            // Status shown in the sidebar footer.
            cx.observe(&system, |_, _, cx| cx.notify()),
        ];
        // Reconnect: refresh what depends on the daemon once it is reachable again.
        let mut last = DaemonStatus::Unknown;
        let inspector_handle = inspector.clone();
        let bar_handle = bar.clone();
        subs.push(cx.observe(&system, move |_, system, cx| {
            let status = system.read(cx).status().clone();
            let was_down = matches!(last, DaemonStatus::NotRunning | DaemonStatus::Error(_));
            if status == DaemonStatus::Connected && was_down {
                inspector_handle.update(cx, |v, cx| v.refresh(cx));
                bar_handle.update(cx, |v, cx| v.refresh(cx));
            }
            last = status;
        }));

        AppView {
            shared,
            page: Page::Inspector,
            inspector,
            bar,
            events,
            perf,
            system,
            _subscriptions: subs,
        }
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.system.read(cx).status().clone();
        let color = status_color(&status, cx);
        let menu = SidebarMenu::new().children(Page::ALL.map(|page| {
            SidebarMenuItem::new(page.label())
                .icon(page.icon())
                .active(self.page == page)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.page = page;
                    cx.notify();
                }))
        }));
        Sidebar::new("mbar-ui-sidebar")
            .w(px(210.))
            .header(
                SidebarHeader::new().child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size_8()
                                .flex_shrink_0()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().sidebar_primary)
                                .text_color(cx.theme().sidebar_primary_foreground)
                                .child(Icon::new(IconName::PanelBottom).small()),
                        )
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(div().font_semibold().child("mbar"))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("bar: {}", self.shared.client.bar_name())),
                                ),
                        ),
                ),
            )
            .child(SidebarGroup::new("Manage").child(menu))
            .footer(
                SidebarFooter::new().child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .child(div().size_2().rounded_full().bg(color))
                        .child(status_text(&status)),
                ),
            )
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content: AnyView = match self.page {
            Page::Inspector => self.inspector.clone().into(),
            Page::Bar => self.bar.clone().into(),
            Page::Events => self.events.clone().into(),
            Page::Performance => self.perf.clone().into(),
            Page::System => self.system.clone().into(),
        };
        h_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_sidebar(cx))
            .child(v_flex().flex_1().min_w_0().h_full().p_4().child(content))
    }
}

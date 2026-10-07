//! Events: live log from `--monitor events` and a `--trigger` form.

use std::ops::Range;

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    notification::Notification,
    v_flex, ActiveTheme as _, IconName, Sizable as _, WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::StyledExt as _;
use gpui_kit::*;
use mbar_ui_model::ipc::{spawn_monitor, MonitorHandle, MonitorUpdate};
use mbar_ui_model::model::{trigger_args, EventLog, EventRecord, MonitorMessage};

use super::{page_header, section, Shared};

#[derive(Clone, Debug, PartialEq)]
enum StreamStatus {
    Connecting,
    Connected,
    Disconnected(String),
}

pub struct EventsView {
    shared: Shared,
    log: EventLog,
    /// Events received while paused; appended on resume.
    held: Vec<EventRecord>,
    paused: bool,
    status: StreamStatus,
    filter: Entity<InputState>,
    /// Indices into `log` matching the filter, newest first.
    visible: Vec<usize>,
    scroll: UniformListScrollHandle,
    trigger_event: Entity<InputState>,
    trigger_vars: Entity<InputState>,
    triggering: bool,
    _monitor: Option<MonitorHandle>,
    _receiver: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventsView {
    pub fn new(shared: Shared, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Filter by event, sender, item or INFO")
        });
        let trigger_event = cx.new(|cx| InputState::new(window, cx).placeholder("event name"));
        let trigger_vars =
            cx.new(|cx| InputState::new(window, cx).placeholder("KEY=VALUE KEY2='with spaces'"));

        let subs = vec![
            cx.subscribe_in(&filter, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.update_visible(cx);
                }
            }),
            cx.subscribe_in(&trigger_vars, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.trigger(window, cx);
                }
            }),
            cx.subscribe_in(&trigger_event, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.trigger(window, cx);
                }
            }),
        ];

        // The monitor thread blocks on the socket; updates reach the UI via a channel.
        let (tx, rx) = async_channel::unbounded::<MonitorUpdate>();
        let monitor = spawn_monitor(shared.client.bar_name(), "events", move |update| {
            tx.send_blocking(update).is_ok()
        });
        let (monitor, status) = match monitor {
            Ok(h) => (Some(h), StreamStatus::Connecting),
            Err(e) => (None, StreamStatus::Disconnected(e.to_string())),
        };
        let receiver = cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                if this.update(cx, |this, cx| this.on_updates(batch, cx)).is_err() {
                    break;
                }
            }
        });

        EventsView {
            shared,
            log: EventLog::default(),
            held: Vec::new(),
            paused: false,
            status,
            filter,
            visible: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            trigger_event,
            trigger_vars,
            triggering: false,
            _monitor: monitor,
            _receiver: receiver,
            _subscriptions: subs,
        }
    }

    fn on_updates(&mut self, batch: Vec<MonitorUpdate>, cx: &mut Context<Self>) {
        let mut changed = false;
        for update in batch {
            match update {
                MonitorUpdate::Connected => self.status = StreamStatus::Connected,
                MonitorUpdate::Disconnected(msg) => self.status = StreamStatus::Disconnected(msg),
                MonitorUpdate::Message(MonitorMessage::Event(e)) => {
                    if self.paused {
                        self.held.push(e);
                        let cap = 5000;
                        if self.held.len() > cap {
                            self.held.drain(..self.held.len() - cap);
                        }
                    } else {
                        self.log.push(e);
                        changed = true;
                    }
                }
                MonitorUpdate::Message(_) => {}
            }
        }
        if changed {
            self.update_visible(cx);
        }
        cx.notify();
    }

    fn update_visible(&mut self, cx: &mut Context<Self>) {
        let filter = self.filter.read(cx).value().to_string();
        self.visible = self.log.filtered(&filter);
        cx.notify();
    }

    fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.paused = paused;
        if !paused {
            for e in std::mem::take(&mut self.held) {
                self.log.push(e);
            }
            self.update_visible(cx);
        }
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.log.clear();
        self.held.clear();
        self.update_visible(cx);
    }

    fn trigger(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.triggering {
            return;
        }
        let event = self.trigger_event.read(cx).value().to_string();
        let vars = self.trigger_vars.read(cx).value().to_string();
        if let Err(e) = trigger_args(&event, &vars) {
            window.push_notification(Notification::error(e).title("Invalid trigger"), cx);
            return;
        }
        self.triggering = true;
        cx.notify();
        let label = event.trim().to_string();
        self.shared.spawn_blocking(
            cx,
            move |client| client.trigger(&event, &vars),
            move |this, result, cx| {
                this.triggering = false;
                cx.notify();
                Some(match result {
                    Ok(_) => Notification::success(format!("Triggered `{label}`")),
                    Err(e) => Notification::error(e.to_string()).title("Trigger failed"),
                })
            },
        );
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        range
            .filter_map(|i| {
                let ix = *self.visible.get(i)?;
                let e = self.log.get(ix)?;
                let even = i % 2 == 0;
                Some(
                    h_flex()
                        .id(("event-row", ix))
                        .w_full()
                        .h(px(26.))
                        .px_2()
                        .gap_3()
                        .text_sm()
                        .when(even, |r| r.bg(theme.table_even))
                        .child(
                            div()
                                .w(px(96.))
                                .flex_shrink_0()
                                .font_family(theme.mono_font_family.clone())
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(e.time_label()),
                        )
                        .child(
                            div()
                                .w(px(190.))
                                .flex_shrink_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_semibold()
                                .child(e.name.clone()),
                        )
                        .child(
                            div()
                                .w(px(150.))
                                .flex_shrink_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(theme.muted_foreground)
                                .child(e.sender.clone()),
                        )
                        .child(
                            div()
                                .w(px(170.))
                                .flex_shrink_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(e.items.join(", ")),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_family(theme.mono_font_family.clone())
                                .text_xs()
                                .child(e.info.replace('\n', " ")),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }
}

impl Render for EventsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (status_label, status_color) = match &self.status {
            StreamStatus::Connecting => ("Connecting…".to_string(), theme.muted_foreground),
            StreamStatus::Connected => ("Live".to_string(), theme.green),
            StreamStatus::Disconnected(msg) => (format!("Disconnected: {msg}"), theme.red),
        };
        let subtitle = format!(
            "`--monitor events` · {} shown · {} received{}",
            self.visible.len(),
            self.log.received,
            if self.paused && !self.held.is_empty() {
                format!(" · {} new while paused", self.held.len())
            } else {
                String::new()
            }
        );

        let controls = h_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_1()
                    .text_xs()
                    .child(div().size_2().rounded_full().bg(status_color))
                    .child(status_label),
            )
            .child(
                Button::new("events-pause")
                    .small()
                    .outline()
                    .icon(if self.paused {
                        IconName::Play
                    } else {
                        IconName::Pause
                    })
                    .label(if self.paused { "Resume" } else { "Pause" })
                    .on_click(cx.listener(|this, _, _, cx| this.set_paused(!this.paused, cx))),
            )
            .child(
                Button::new("events-clear")
                    .small()
                    .outline()
                    .icon(IconName::Delete)
                    .label("Clear")
                    .on_click(cx.listener(|this, _, _, cx| this.clear(cx))),
            );

        let theme = cx.theme();
        let header = h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_3()
            .text_xs()
            .font_semibold()
            .bg(theme.table_head)
            .text_color(theme.table_head_foreground)
            .child(div().w(px(96.)).flex_shrink_0().child("Time (UTC)"))
            .child(div().w(px(190.)).flex_shrink_0().child("Event"))
            .child(div().w(px(150.)).flex_shrink_0().child("Sender"))
            .child(div().w(px(170.)).flex_shrink_0().child("Items run"))
            .child(div().flex_1().child("INFO"));

        let list: AnyElement = if self.visible.is_empty() {
            div()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(if self.log.is_empty() {
                    "No events yet. They appear here as mbar dispatches them."
                } else {
                    "No event matches the filter."
                })
                .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    uniform_list(
                        "event-log",
                        self.visible.len(),
                        cx.processor(|this, range: Range<usize>, _window, cx| {
                            this.render_rows(range, cx)
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .size_full(),
                )
                .vertical_scrollbar(&self.scroll)
                .into_any_element()
        };

        let trigger_form = section("Trigger an event", cx).child(
            h_flex()
                .gap_2()
                .child(div().w(px(200.)).child(Input::new(&self.trigger_event).small()))
                .child(div().flex_1().child(Input::new(&self.trigger_vars).small()))
                .child(
                    Button::new("events-trigger")
                        .small()
                        .primary()
                        .icon(IconName::Play)
                        .label("Trigger")
                        .loading(self.triggering)
                        .on_click(cx.listener(|this, _, window, cx| this.trigger(window, cx))),
                ),
        );

        let theme = cx.theme();
        v_flex()
            .size_full()
            .gap_3()
            .child(page_header("Events", subtitle, cx).child(controls))
            .child(Input::new(&self.filter).small().cleanable(true))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius)
                    .overflow_hidden()
                    .child(header)
                    .child(list),
            )
            .child(trigger_form)
    }
}


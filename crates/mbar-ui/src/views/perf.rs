//! Performance: `--query stats` every second.

use std::time::Duration;

use gpui_kit::component::{
    chart::LineChart,
    h_flex,
    table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
    v_flex, ActiveTheme as _, Sizable as _,
};
use gpui_kit::*;
use mbar_ui_model::ipc::IpcError;
use mbar_ui_model::model::{FrameSample, Stats, StatsHistory};

use super::{interval, page_header, section, stat_tile, Shared};

pub struct PerfView {
    shared: Shared,
    stats: Option<Stats>,
    history: StatsHistory,
    error: Option<String>,
    in_flight: bool,
    _poll: Task<()>,
}

/// `310` µs → `0.31 ms`.
fn ms_from_us(us: f64) -> String {
    format!("{:.2} ms", us / 1000.0)
}

fn uptime(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, (s / 60) % 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn table_of(headers: &[&'static str], rows: Vec<Vec<String>>) -> Table {
    let header = TableHeader::new().child(TableRow::new().children(headers.iter().enumerate().map(
        |(i, h)| {
            let head = TableHead::new().child(*h);
            if i == 0 {
                head
            } else {
                head.text_right()
            }
        },
    )));
    let body = TableBody::new().children(rows.into_iter().map(|cols| {
        TableRow::new().children(cols.into_iter().enumerate().map(|(i, c)| {
            let cell = TableCell::new().child(c);
            if i == 0 {
                cell
            } else {
                cell.text_right()
            }
        }))
    }));
    Table::new().small().child(header).child(body)
}

impl PerfView {
    pub fn new(shared: Shared, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let poll = interval(cx, Duration::from_secs(1), |this: &mut Self, cx| this.poll(cx));
        let mut view = PerfView {
            shared,
            stats: None,
            history: StatsHistory::default(),
            error: None,
            in_flight: false,
            _poll: poll,
        };
        view.poll(cx);
        view
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.in_flight {
            return;
        }
        self.in_flight = true;
        self.shared.spawn_blocking(
            cx,
            |client| client.query_stats(),
            |this, result, cx| {
                this.in_flight = false;
                match result {
                    Ok(stats) => {
                        this.history.push(&stats);
                        this.stats = Some(stats);
                        this.error = None;
                    }
                    Err(IpcError::NotRunning) => {
                        this.error = Some("mbar is not running.".into());
                        this.history.clear();
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
                None
            },
        );
    }

    fn render_sparkline(&self, cx: &App) -> impl IntoElement {
        let data: Vec<FrameSample> = self.history.samples.iter().cloned().collect();
        let fps = data.last().map(|s| s.frames).unwrap_or(0);
        section(
            format!("Frame time (avg per second) · {fps} frames in the last second"),
            cx,
        )
        .child(
            div().h(px(150.)).w_full().child(
                LineChart::new(data)
                    .x(|s: &FrameSample| s.t.clone())
                    .y(|s: &FrameSample| s.avg_us / 1000.0)
                    .stroke(cx.theme().chart_1)
                    .linear()
                    .x_axis(false)
                    .y_axis(true)
                    .y_tick_format(|v| format!("{v:.2} ms"))
                    .tick_margin(20)
                    .appear(false)
                    .name("avg ms")
                    .id("frame-time-chart"),
            ),
        )
    }
}

impl Render for PerfView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = page_header(
            "Performance",
            "`--query stats`, sampled every second",
            cx,
        );
        let Some(s) = self.stats.clone() else {
            return v_flex().size_full().gap_3().child(header).child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.error.clone().unwrap_or_else(|| "Loading…".into())),
            );
        };

        let tiles = h_flex()
            .flex_wrap()
            .gap_2()
            .child(stat_tile("Frame avg", ms_from_us(s.frame_time_us.avg), cx))
            .child(stat_tile("Frame p95", ms_from_us(s.frame_time_us.p95), cx))
            .child(stat_tile("Frame max", ms_from_us(s.frame_time_us.max), cx))
            .child(stat_tile("Layout avg", ms_from_us(s.layout_time_us.avg), cx))
            .child(stat_tile("Layout p95", ms_from_us(s.layout_time_us.p95), cx))
            .child(stat_tile("Layout max", ms_from_us(s.layout_time_us.max), cx));
        let counters = h_flex()
            .flex_wrap()
            .gap_2()
            .child(stat_tile("Uptime", uptime(s.uptime_s), cx))
            .child(stat_tile("Frames", s.frames.to_string(), cx))
            .child(stat_tile("Items", s.items.to_string(), cx))
            .child(stat_tile("Windows", s.windows.to_string(), cx))
            .child(stat_tile("IPC messages", s.ipc_messages.to_string(), cx));

        let scripts = section(
            format!(
                "Scripts · {} spawned · {} running · avg {:.1} ms · max {:.1} ms",
                s.scripts_spawned, s.scripts_running, s.scripts_avg_ms, s.scripts_max_ms
            ),
            cx,
        )
        .child(if s.scripts_by_item.is_empty() {
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No scripts have run.")
                .into_any_element()
        } else {
            table_of(
                &["Item", "Runs", "Avg ms", "Max ms", "Total ms"],
                s.scripts_by_item
                    .iter()
                    .map(|i| {
                        vec![
                            i.name.clone(),
                            i.runs.to_string(),
                            format!("{:.2}", i.avg_ms),
                            format!("{:.2}", i.max_ms),
                            format!("{:.1}", i.total_ms()),
                        ]
                    })
                    .collect(),
            )
            .into_any_element()
        });

        let lua = section("Lua callbacks", cx).child(table_of(
            &["Metric", "Value"],
            vec![
                vec!["Callbacks".into(), s.lua_callbacks.to_string()],
                vec!["Average".into(), format!("{:.0} µs", s.lua_avg_us)],
                vec!["Max".into(), format!("{:.0} µs", s.lua_max_us)],
            ],
        ));

        let redraws = section("Redraws per window", cx).child(if s.redraws_by_window.is_empty() {
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No redraws yet.")
                .into_any_element()
        } else {
            table_of(
                &["Window", "Redraws"],
                s.redraws_by_window
                    .iter()
                    .map(|(w, n)| vec![w.clone(), n.to_string()])
                    .collect(),
            )
            .into_any_element()
        });

        let events = section("Events dispatched", cx).child(if s.events.is_empty() {
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No events yet.")
                .into_any_element()
        } else {
            table_of(
                &["Event", "Count"],
                s.events
                    .iter()
                    .map(|(e, n)| vec![e.clone(), n.to_string()])
                    .collect(),
            )
            .into_any_element()
        });

        v_flex()
            .size_full()
            .gap_3()
            .child(header)
            .children(self.error.clone().map(|e| {
                div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(format!("{e} (showing the last sample)"))
            }))
            .child(
                v_flex()
                    .id("perf-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_3()
                    .child(tiles)
                    .child(self.render_sparkline(cx))
                    .child(counters)
                    .child(scripts)
                    .child(
                        h_flex()
                            .items_start()
                            .gap_3()
                            .child(v_flex().flex_1().gap_3().child(lua).child(redraws))
                            .child(div().flex_1().child(events)),
                    ),
            )
    }
}

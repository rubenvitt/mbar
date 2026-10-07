//! Bar: `--query bar` as an editable property table (`--bar key=value`).

use std::time::Duration;

use gpui_kit::component::{
    button::Button, h_flex, switch::Switch, v_flex, ActiveTheme as _, IconName, Sizable as _,
};
use gpui_kit::*;
use mbar_ui_model::ipc::IpcError;
use mbar_ui_model::model::Target;

use super::props::{PropertyEditor, PropertyEditorEvent};
use super::{interval, page_header, Shared};

pub struct BarView {
    shared: Shared,
    editor: Entity<PropertyEditor>,
    loading: bool,
    error: Option<String>,
    items: usize,
    auto_refresh: bool,
    _auto_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl BarView {
    pub fn new(shared: Shared, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            PropertyEditor::new(
                shared.clone(),
                "Bar properties are not available.",
                window,
                cx,
            )
        });
        let subs = vec![cx.subscribe(
            &editor,
            |this, _, event: &PropertyEditorEvent, cx| match event {
                PropertyEditorEvent::Applied => this.refresh(cx),
            },
        )];
        let mut view = BarView {
            shared,
            editor,
            loading: false,
            error: None,
            items: 0,
            auto_refresh: false,
            _auto_task: None,
            _subscriptions: subs,
        };
        view.refresh(cx);
        view
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();
        self.shared.spawn_blocking(
            cx,
            |client| client.query_bar(),
            |this, result, cx| {
                this.loading = false;
                match result {
                    Ok(bar) => {
                        this.error = None;
                        this.items = bar.items.len();
                        this.editor.update(cx, |e, cx| {
                            e.set_data(Some(Target::Bar), Some(&bar.json), cx)
                        });
                    }
                    Err(e) => {
                        this.error = Some(match e {
                            IpcError::NotRunning => {
                                "mbar is not running. Start it on the System page.".into()
                            }
                            other => other.to_string(),
                        });
                        this.editor.update(cx, |e, cx| e.set_data(None, None, cx));
                    }
                }
                cx.notify();
                None
            },
        );
    }

    fn set_auto_refresh(&mut self, on: bool, cx: &mut Context<Self>) {
        self.auto_refresh = on;
        self._auto_task =
            on.then(|| interval(cx, Duration::from_secs(1), |this, cx| this.refresh(cx)));
        cx.notify();
    }
}

impl Render for BarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let subtitle = format!(
            "`--bar` properties · {} items · `items` and other greyed values are read-only",
            self.items
        );
        let controls = h_flex()
            .gap_3()
            .child(
                Switch::new("bar-auto")
                    .label("Auto-refresh (1 s)")
                    .checked(self.auto_refresh)
                    .on_change(
                        cx.listener(|this, on: &bool, _, cx| this.set_auto_refresh(*on, cx)),
                    ),
            )
            .child(
                Button::new("bar-refresh")
                    .small()
                    .outline()
                    .icon(IconName::RefreshCw)
                    .label("Refresh")
                    .loading(self.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );
        v_flex()
            .size_full()
            .gap_3()
            .child(page_header("Bar", subtitle, cx).child(controls))
            .children(
                self.error
                    .clone()
                    .map(|e| div().text_sm().text_color(cx.theme().danger).child(e)),
            )
            .child(div().flex_1().min_h_0().child(self.editor.clone()))
    }
}

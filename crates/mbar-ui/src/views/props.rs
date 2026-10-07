//! Property table with inline editing, shared by the Inspector (items) and Bar pages.
//!
//! Selecting a row turns its value cell into an input; Enter (or "Apply") sends
//! `--set <item> key=value` / `--bar key=value`. Read-only values (geometry masks,
//! bounding rects, popup item lists, ...) are shown but cannot be selected for editing.

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    notification::Notification,
    v_flex, ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mbar_ui_model::model::{
    cli_command, export_pairs, filter_rows, lua_snippet, property_rows, PropRow, Target,
};
use serde_json::Value;

use super::Shared;

pub enum PropertyEditorEvent {
    /// A value was applied successfully; the owner should re-query.
    Applied,
}

pub struct PropertyEditor {
    shared: Shared,
    target: Option<Target>,
    rows: Vec<PropRow>,
    /// Path of the selected row.
    selected: Option<String>,
    filter: Entity<InputState>,
    edit: Entity<InputState>,
    /// The user typed into the edit field since it was last synced.
    edit_dirty: bool,
    /// Value to put into the edit field on the next render (needs a `Window`).
    pending_edit_value: Option<String>,
    applying: bool,
    empty_text: SharedString,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PropertyEditorEvent> for PropertyEditor {}

impl PropertyEditor {
    pub fn new(
        shared: Shared,
        empty_text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter properties"));
        let edit = cx.new(|cx| InputState::new(window, cx).placeholder("value"));
        let subs = vec![
            cx.subscribe_in(&filter, window, |_, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&edit, window, |this, _, event, _, cx| match event {
                InputEvent::Change => this.edit_dirty = true,
                InputEvent::PressEnter { .. } => this.apply(cx),
                _ => {}
            }),
        ];
        PropertyEditor {
            shared,
            target: None,
            rows: Vec::new(),
            selected: None,
            filter,
            edit,
            edit_dirty: false,
            pending_edit_value: None,
            applying: false,
            empty_text: empty_text.into(),
            _subscriptions: subs,
        }
    }

    /// Replaces the shown data. Keeps the selection (and an in-progress edit) while the
    /// target stays the same.
    pub fn set_data(
        &mut self,
        target: Option<Target>,
        json: Option<&Value>,
        cx: &mut Context<Self>,
    ) {
        let same_target = target == self.target;
        self.rows = match (&target, json) {
            (Some(t), Some(j)) => property_rows(t, j),
            _ => Vec::new(),
        };
        self.target = target;
        if !same_target {
            self.selected = None;
            self.edit_dirty = false;
            self.pending_edit_value = Some(String::new());
        } else if let Some(row) = self.selected_row() {
            if !self.edit_dirty && !self.applying {
                self.pending_edit_value = Some(row.value.clone());
            }
        } else {
            self.selected = None;
        }
        cx.notify();
    }

    fn selected_row(&self) -> Option<&PropRow> {
        let sel = self.selected.as_ref()?;
        self.rows.iter().find(|r| &r.path == sel)
    }

    fn select(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.iter().find(|r| r.path == path) else {
            return;
        };
        if row.set_key.is_none() {
            return;
        }
        let value = row.value.clone();
        self.selected = Some(path);
        self.edit_dirty = false;
        self.pending_edit_value = None;
        self.edit.update(cx, |state, cx| {
            state.set_value(value, window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(value) = self.selected_row().map(|r| r.value.clone()) {
            self.edit_dirty = false;
            self.edit
                .update(cx, |state, cx| state.set_value(value, window, cx));
            cx.notify();
        }
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let (Some(target), Some(row)) = (self.target.clone(), self.selected_row()) else {
            return;
        };
        let Some(key) = row.set_key.clone() else {
            return;
        };
        if self.applying {
            return;
        }
        let value = self.edit.read(cx).value().to_string();
        self.applying = true;
        cx.notify();
        let pair = vec![(key.clone(), value)];
        let label = match &target {
            Target::Bar => format!("--bar {key}"),
            Target::Item(name) => format!("{name}: {key}"),
        };
        self.shared.spawn_blocking(
            cx,
            move |client| client.set(&target, &pair),
            move |this, result, cx| {
                this.applying = false;
                cx.notify();
                match result {
                    Ok(_) => {
                        this.edit_dirty = false;
                        cx.emit(PropertyEditorEvent::Applied);
                        Some(Notification::success(format!("Applied {label}")))
                    }
                    Err(e) => Some(Notification::error(e.to_string()).title("Could not apply")),
                }
            },
        );
    }

    /// The selected property with the value currently in the editor, or every editable
    /// property when nothing is selected.
    fn copy_pairs(&self, cx: &App) -> Vec<(String, String)> {
        match self.selected_row().and_then(|r| r.set_key.clone()) {
            Some(key) => vec![(key, self.edit.read(cx).value().to_string())],
            None => export_pairs(&self.rows),
        }
    }

    fn copy(&self, lua: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let pairs = self.copy_pairs(cx);
        if pairs.is_empty() {
            return;
        }
        let text = if lua {
            lua_snippet(&target, &pairs)
        } else {
            cli_command("mbar", &target, &pairs)
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let what = if pairs.len() == 1 {
            format!("`{}`", pairs[0].0)
        } else {
            format!("{} properties", pairs.len())
        };
        let kind = if lua { "Lua" } else { "CLI command" };
        window.push_notification(Notification::info(format!("Copied {what} as {kind}")), cx);
    }

    fn render_row(&self, row: &PropRow, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.selected.as_deref() == Some(row.path.as_str());
        let editable = row.set_key.is_some();
        let path = row.path.clone();
        let theme = cx.theme();
        let key_cell = div()
            .w(px(260.))
            .flex_shrink_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(if editable {
                theme.foreground
            } else {
                theme.muted_foreground
            })
            .child(row.path.clone());
        let value_cell: AnyElement = if selected {
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.edit).small()),
                )
                .child(
                    Button::new("apply")
                        .small()
                        .primary()
                        .label("Apply")
                        .loading(self.applying)
                        .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
                )
                .child(
                    Button::new("revert")
                        .small()
                        .ghost()
                        .icon(IconName::Undo2)
                        .tooltip("Revert to the current value")
                        .on_click(cx.listener(|this, _, window, cx| this.revert(window, cx))),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(theme.mono_font_family.clone())
                .text_color(if editable {
                    theme.foreground
                } else {
                    theme.muted_foreground
                })
                .child(if row.value.is_empty() {
                    "\u{2014}".to_string()
                } else {
                    row.value.replace('\n', "⏎")
                })
                .into_any_element()
        };
        h_flex()
            .id(ElementId::Name(format!("prop:{}", row.path).into()))
            .w_full()
            .min_h(px(28.))
            .px_2()
            .gap_2()
            .text_sm()
            .border_b_1()
            .border_color(theme.table_row_border)
            .when(selected, |this| this.bg(theme.table_active))
            .when(editable && !selected, |this| {
                let hover = theme.table_hover;
                this.cursor_pointer().hover(move |s| s.bg(hover))
            })
            .child(key_cell)
            .child(value_cell)
            .when(editable && !selected, |this| {
                this.on_click(
                    cx.listener(move |this, _, window, cx| this.select(path.clone(), window, cx)),
                )
            })
            .into_any_element()
    }
}

impl Render for PropertyEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(value) = self.pending_edit_value.take() {
            self.edit
                .update(cx, |state, cx| state.set_value(value, window, cx));
        }
        let filter = self.filter.read(cx).value().to_string();
        let has_target = self.target.is_some();
        let selected_key = self
            .selected_row()
            .and_then(|r| r.set_key.clone())
            .unwrap_or_default();
        let rows: Vec<PropRow> = filter_rows(&self.rows, &filter)
            .into_iter()
            .cloned()
            .collect();
        let copy_hint = if self.selected.is_some() {
            "Copy the selected property"
        } else {
            "Copy all editable properties"
        };

        let toolbar = h_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .child(Input::new(&self.filter).small().cleanable(true)),
            )
            .child(
                Button::new("copy-cli")
                    .small()
                    .outline()
                    .icon(IconName::SquareTerminal)
                    .label("Copy as CLI")
                    .tooltip(copy_hint)
                    .disabled(!has_target)
                    .on_click(cx.listener(|this, _, window, cx| this.copy(false, window, cx))),
            )
            .child(
                Button::new("copy-lua")
                    .small()
                    .outline()
                    .icon(IconName::Copy)
                    .label("Copy as Lua")
                    .tooltip(copy_hint)
                    .disabled(!has_target)
                    .on_click(cx.listener(|this, _, window, cx| this.copy(true, window, cx))),
            );

        let theme = cx.theme();
        let header = h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .text_xs()
            .font_semibold()
            .bg(theme.table_head)
            .text_color(theme.table_head_foreground)
            .child(div().w(px(260.)).flex_shrink_0().child("Property"))
            .child(div().flex_1().child("Value"));

        let body: AnyElement = if !has_target {
            div()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(self.empty_text.clone())
                .into_any_element()
        } else {
            let elements: Vec<AnyElement> = rows.iter().map(|r| self.render_row(r, cx)).collect();
            v_flex()
                .id("prop-rows")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(elements)
                .into_any_element()
        };

        let theme = cx.theme();
        let footer = h_flex()
            .w_full()
            .gap_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(if selected_key.is_empty() {
                "Click a property to edit it. Greyed values are read-only.".to_string()
            } else {
                format!("Editing `{selected_key}` · Enter applies")
            });

        v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .child(toolbar)
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius)
                    .overflow_hidden()
                    .child(header)
                    .child(body),
            )
            .child(footer)
    }
}

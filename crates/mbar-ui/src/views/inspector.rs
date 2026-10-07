//! Inspector: tree of bar → items (by position, popups under their host, brackets with
//! their members) and the property table of the selected item.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::{
    button::Button,
    h_flex,
    list::ListItem,
    switch::Switch,
    tag::Tag,
    tree::{tree, TreeEvent, TreeItem, TreeState},
    v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::component::StyledExt as _;
use gpui_kit::*;
use mbar_ui_model::ipc::IpcError;
use mbar_ui_model::model::{build_tree, NodeKind, Snapshot, Target, TreeNode};

use super::props::{PropertyEditor, PropertyEditorEvent};
use super::{interval, page_header, Shared};

#[derive(Clone)]
struct NodeMeta {
    kind: NodeKind,
    item: Option<String>,
    item_type: Option<String>,
    drawing: bool,
}

pub struct InspectorView {
    shared: Shared,
    tree: Entity<TreeState>,
    nodes: Vec<TreeNode>,
    meta: Rc<HashMap<String, NodeMeta>>,
    snapshot: Option<Snapshot>,
    selected_node: Option<String>,
    collapsed: HashSet<String>,
    editor: Entity<PropertyEditor>,
    loading: bool,
    error: Option<String>,
    auto_refresh: bool,
    _auto_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

fn to_tree_items(nodes: &[TreeNode], collapsed: &HashSet<String>) -> Vec<TreeItem> {
    nodes
        .iter()
        .map(|n| {
            TreeItem::new(n.id.clone(), n.label.clone())
                .expanded(!collapsed.contains(&n.id))
                .children(to_tree_items(&n.children, collapsed))
        })
        .collect()
}

fn collect_meta(nodes: &[TreeNode]) -> HashMap<String, NodeMeta> {
    let mut map = HashMap::new();
    for n in nodes {
        n.walk(&mut |node| {
            map.insert(
                node.id.clone(),
                NodeMeta {
                    kind: node.kind,
                    item: node.item.clone(),
                    item_type: node.item_type.clone(),
                    drawing: node.drawing,
                },
            );
        });
    }
    map
}

fn type_icon(meta: &NodeMeta, expanded: bool) -> IconName {
    match meta.kind {
        NodeKind::Group if expanded => IconName::FolderOpen,
        NodeKind::Group => IconName::Folder,
        NodeKind::BracketMember => IconName::ArrowRight,
        NodeKind::Item => match meta.item_type.as_deref() {
            Some("bracket") => IconName::Frame,
            Some("graph") => IconName::ChartPie,
            Some("slider") => IconName::Minus,
            Some("alias") => IconName::Copy,
            Some("space") => IconName::LayoutDashboard,
            Some("app_menu") => IconName::Menu,
            _ => IconName::Square,
        },
    }
}

impl InspectorView {
    pub fn new(shared: Shared, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let editor = cx.new(|cx| {
            PropertyEditor::new(
                shared.clone(),
                "Select an item in the tree to see its properties.",
                window,
                cx,
            )
        });
        let subs = vec![
            cx.observe(&tree, |this, tree, cx| {
                let selected = tree.read(cx).selected_item().map(|i| i.id.to_string());
                if selected.is_some() && selected != this.selected_node {
                    this.selected_node = selected;
                    this.sync_editor(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(&tree, |this, _, event: &TreeEvent, _| match event {
                TreeEvent::Collapsed(id) => {
                    this.collapsed.insert(id.to_string());
                }
                TreeEvent::Expanded(id) => {
                    this.collapsed.remove(id.as_ref());
                }
            }),
            cx.subscribe(&editor, |this, _, event: &PropertyEditorEvent, cx| match event {
                PropertyEditorEvent::Applied => this.refresh(cx),
            }),
        ];
        let mut view = InspectorView {
            shared,
            tree,
            nodes: Vec::new(),
            meta: Rc::new(HashMap::new()),
            snapshot: None,
            selected_node: None,
            collapsed: HashSet::new(),
            editor,
            loading: false,
            error: None,
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
            |client| client.fetch_snapshot(),
            |this, result, cx| {
                this.loading = false;
                match result {
                    Ok(snapshot) => {
                        this.error = None;
                        this.apply_snapshot(snapshot, cx);
                    }
                    Err(e) => {
                        this.error = Some(match e {
                            IpcError::NotRunning => {
                                "mbar is not running. Start it on the System page.".to_string()
                            }
                            other => other.to_string(),
                        });
                    }
                }
                cx.notify();
                None
            },
        );
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        let nodes = build_tree(&snapshot.bar.items, &snapshot.items);
        if nodes != self.nodes {
            // Forget collapse state of nodes that no longer exist.
            let mut ids = HashSet::new();
            for n in &nodes {
                n.walk(&mut |node| {
                    ids.insert(node.id.clone());
                });
            }
            self.collapsed.retain(|id| ids.contains(id));
            if let Some(sel) = &self.selected_node {
                if !ids.contains(sel) {
                    self.selected_node = None;
                }
            }
            let items = to_tree_items(&nodes, &self.collapsed);
            let selected = self.selected_node.clone();
            self.tree.update(cx, |state, cx| {
                state.set_items(items, cx);
                if let Some(id) = selected {
                    state.set_selected_item(Some(&TreeItem::new(id, "")), cx);
                }
            });
            self.meta = Rc::new(collect_meta(&nodes));
            self.nodes = nodes;
        }
        self.snapshot = Some(snapshot);
        self.sync_editor(cx);
    }

    fn selected_item(&self) -> Option<String> {
        let id = self.selected_node.as_ref()?;
        self.meta.get(id)?.item.clone()
    }

    fn sync_editor(&mut self, cx: &mut Context<Self>) {
        let item = self.selected_item();
        let json = item
            .as_ref()
            .and_then(|name| self.snapshot.as_ref()?.items.get(name))
            .map(|info| info.json.clone());
        let target = match (&item, &json) {
            (Some(name), Some(_)) => Some(Target::Item(name.clone())),
            _ => None,
        };
        self.editor
            .update(cx, |editor, cx| editor.set_data(target, json.as_ref(), cx));
    }

    fn set_auto_refresh(&mut self, on: bool, cx: &mut Context<Self>) {
        self.auto_refresh = on;
        self._auto_task = on.then(|| interval(cx, Duration::from_secs(1), |this, cx| this.refresh(cx)));
        cx.notify();
    }

    fn render_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let meta = self.meta.clone();
        let theme = cx.theme();
        let (muted, fg) = (theme.muted_foreground, theme.foreground);
        tree(&self.tree, move |_ix, entry, _selected, _window, _cx| {
            let item = entry.item();
            let m = meta.get(item.id.as_ref()).cloned().unwrap_or(NodeMeta {
                kind: NodeKind::Item,
                item: None,
                item_type: None,
                drawing: true,
            });
            let icon = type_icon(&m, entry.is_expanded());
            let is_group = m.kind == NodeKind::Group;
            let type_tag = m
                .item_type
                .clone()
                .filter(|t| t != "item" && m.kind == NodeKind::Item);
            ListItem::new(ElementId::Name(item.id.clone()))
                .w_full()
                .py_0p5()
                .pl(px(14.) * entry.depth() as f32 + px(8.))
                .child(
                    h_flex()
                        .gap_2()
                        .w_full()
                        .min_w_0()
                        .text_sm()
                        .text_color(if m.drawing { fg } else { muted })
                        .child(Icon::new(icon).small())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .when(is_group, |d| d.font_semibold())
                                .child(item.label.clone()),
                        )
                        .when_some(type_tag, |row, t| {
                            row.child(Tag::secondary().small().child(t))
                        })
                        .when(!m.drawing, |row| row.child(Icon::new(IconName::EyeOff).small())),
                )
        })
    }
}

impl Render for InspectorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let subtitle = match &self.snapshot {
            Some(s) => format!(
                "{} items{}{}",
                s.items.len(),
                if s.errors.is_empty() {
                    String::new()
                } else {
                    format!(" · {} could not be queried", s.errors.len())
                },
                if self.loading { " · refreshing…" } else { "" }
            ),
            None => "Live tree of bars, items, popups and brackets".to_string(),
        };
        let controls = h_flex()
            .gap_3()
            .child(
                Switch::new("inspector-auto")
                    .label("Auto-refresh (1 s)")
                    .checked(self.auto_refresh)
                    .on_change(cx.listener(|this, on: &bool, _, cx| this.set_auto_refresh(*on, cx))),
            )
            .child(
                Button::new("inspector-refresh")
                    .small()
                    .outline()
                    .icon(IconName::RefreshCw)
                    .label("Refresh")
                    .loading(self.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );

        let theme = cx.theme();
        let left: AnyElement = if let Some(err) = &self.error {
            div()
                .p_3()
                .text_sm()
                .text_color(theme.danger)
                .child(err.clone())
                .into_any_element()
        } else if self.nodes.is_empty() {
            div()
                .p_3()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(if self.loading { "Loading…" } else { "The bar has no items." })
                .into_any_element()
        } else {
            self.render_tree(cx).into_any_element()
        };

        let theme = cx.theme();
        v_flex()
            .size_full()
            .gap_3()
            .child(page_header("Inspector", subtitle, cx).child(controls))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .gap_3()
                    .child(
                        v_flex()
                            .w(px(300.))
                            .h_full()
                            .flex_shrink_0()
                            .p_1()
                            .border_1()
                            .border_color(theme.border)
                            .rounded(theme.radius)
                            .child(left),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.editor.clone()),
                    ),
            )
    }
}

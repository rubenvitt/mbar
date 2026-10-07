//! The data model shared by layout (WP-A), command execution/query (WP-B/WP-C) and the
//! runtime: what `g_bar_manager` holds in SketchyBar, minus windows/timers/animator.
//!
//! Accessors and the visibility predicate (`bar_draws_item`) are complete; mutations that
//! involve several items (popup membership, brackets, removal, ordering) live in
//! `runtime.rs` (WP-C) and `group.rs`.

use crate::bar::{BarProps, BarState};
use crate::event::CustomEvents;
use crate::item::{BarItem, ItemId, ItemType, Position};
use crate::platform::{ImageInfo, Resources};

/// All bar state.
#[derive(Debug, Clone)]
pub struct Model {
    /// Global item order = layout/draw order (`bar_items`).
    pub items: Vec<BarItem>,
    /// `--default` template (`"defaults"`).
    pub default_item: BarItem,
    pub bar: BarProps,
    /// One per selected display, in creation order (`bars[]`).
    pub bars: Vec<BarState>,
    pub events: CustomEvents,
    /// `display_active_display_adid()` as of the last poll.
    pub active_adid: u32,
    /// Shared now-playing artwork (`current_artwork`) for `media.artwork` images.
    pub current_artwork: Option<ImageInfo>,
    /// Bar background must be redrawn and all bars re-laid-out.
    pub bar_needs_update: bool,
    /// Bar window frames must be recomputed.
    pub bar_needs_resize: bool,
    /// Some background uses `clip`.
    pub might_need_clipping: bool,
    /// Window z-order must be refreshed.
    pub needs_ordering: bool,
    next_id: u64,
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Model {
    /// `bar_manager_init` state (no bars yet; the runtime creates them with
    /// `Runtime::begin_bars`).
    pub fn new() -> Self {
        Model {
            items: Vec::new(),
            default_item: BarItem::new_default(),
            bar: BarProps::default(),
            bars: Vec::new(),
            events: CustomEvents::new(),
            active_adid: 0,
            current_artwork: None,
            bar_needs_update: false,
            bar_needs_resize: false,
            might_need_clipping: false,
            needs_ordering: false,
            next_id: 1,
        }
    }

    /// Fresh, unused item id.
    pub fn alloc_id(&mut self) -> ItemId {
        let id = ItemId(self.next_id);
        self.next_id += 1;
        id
    }

    /// `bar_manager_create_item`: appends a new item initialised from the default item and
    /// sets `needs_ordering`. Name/type/position are set by the caller (`--add`).
    pub fn create_item(&mut self, res: &mut dyn Resources) -> ItemId {
        let id = self.alloc_id();
        let item = BarItem::from_default(id, &self.default_item, res);
        self.items.push(item);
        self.needs_ordering = true;
        id
    }

    /// Position of an item in the global order.
    pub fn index_of(&self, id: ItemId) -> Option<usize> {
        self.items.iter().position(|i| i.id == id)
    }

    pub fn item(&self, id: ItemId) -> Option<&BarItem> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: ItemId) -> Option<&mut BarItem> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// `bar_manager_get_item_index_for_name`: exact name, first match.
    pub fn find(&self, name: &str) -> Option<ItemId> {
        self.items
            .iter()
            .find(|i| i.name.as_deref() == Some(name))
            .map(|i| i.id)
    }

    /// Item name (`None` for unknown ids or unnamed items).
    pub fn name_of(&self, id: ItemId) -> Option<String> {
        self.item(id).and_then(|i| i.name.clone())
    }

    /// All names in global order (for `--query bar`).
    pub fn item_names(&self) -> Vec<Option<&str>> {
        self.items.iter().map(|i| i.name.as_deref()).collect()
    }

    pub fn bar(&self, adid: u32) -> Option<&BarState> {
        self.bars.iter().find(|b| b.adid == adid)
    }

    /// `bar_draws_item(bar, item)` (`item.md` §7.1, `bar.md` §4.2), with D3: a popup member is
    /// only drawn while its host is drawn on that bar.
    pub fn draws_item(&self, bar: &BarState, item: &BarItem) -> bool {
        self.draws_item_depth(bar, item, 0)
    }

    fn draws_item_depth(&self, bar: &BarState, item: &BarItem, depth: u32) -> bool {
        if !item.drawing || !bar.shown || bar.hidden {
            return false;
        }
        if !item.ignore_association {
            let adid_bit = 1u32.checked_shl(bar.adid).unwrap_or(0);
            if item.associated_display != 0 && item.associated_display & adid_bit == 0 {
                return false;
            }
            if item.associated_to_active_display && bar.adid != self.active_adid {
                return false;
            }
            let sid_bit = 1u32.checked_shl(bar.sid).unwrap_or(0);
            if item.associated_space != 0
                && item.associated_space & sid_bit == 0
                && item.item_type != ItemType::Space
            {
                return false;
            }
        }
        if item.position == Position::Popup {
            let Some(parent) = item.parent.and_then(|p| self.item(p)) else {
                return false;
            };
            if !parent.popup.drawing || bar.adid != self.active_adid {
                return false;
            }
            // D3: hide the popup while its host is not drawn.
            if depth > 16 || !self.draws_item_depth(bar, parent, depth + 1) {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::HeadlessResources;

    #[test]
    fn create_find_and_draws() {
        let mut res = HeadlessResources::default();
        let mut m = Model::new();
        m.active_adid = 1;
        let a = m.create_item(&mut res);
        m.item_mut(a).unwrap().set_name("a");
        let b = m.create_item(&mut res);
        m.item_mut(b).unwrap().set_name("b");
        assert_eq!(m.find("b"), Some(b));
        assert_eq!(m.index_of(b), Some(1));
        let mut bar = BarState::new(1, 1);
        bar.sid = 2;
        assert!(m.draws_item(&bar, m.item(a).unwrap()));
        m.item_mut(a).unwrap().associated_space = 1 << 3;
        assert!(!m.draws_item(&bar, m.item(a).unwrap()));
        m.item_mut(a).unwrap().ignore_association = true;
        assert!(m.draws_item(&bar, m.item(a).unwrap()));
        // popup member: needs parent with popup drawing, active bar, host drawn (D3)
        {
            let bi = m.item_mut(b).unwrap();
            bi.position = Position::Popup;
            bi.parent = Some(a);
        }
        assert!(!m.draws_item(&bar, m.item(b).unwrap()));
        m.item_mut(a).unwrap().popup.drawing = true;
        assert!(m.draws_item(&bar, m.item(b).unwrap()));
        m.item_mut(a).unwrap().drawing = false;
        assert!(!m.draws_item(&bar, m.item(b).unwrap()));
    }
}

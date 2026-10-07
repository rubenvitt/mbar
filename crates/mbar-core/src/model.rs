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
use std::cell::RefCell;
use std::collections::HashMap;

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
    /// `ItemId` → index into `items` (PERF-8). `items` is a public `Vec` that the runtime
    /// reorders, inserts into and removes from directly, so the map is never trusted: every
    /// hit is validated against `items[idx].id` and a stale or missing entry triggers a full
    /// O(n) rebuild. Lookups are O(1) amortized between structural changes (SketchyBar
    /// follows direct pointers, e.g. `item->parent` in `bar_draws_item`).
    index: RefCell<ItemIndex>,
}

/// Lazily rebuilt id → position map backing `Model::index_of` / `item` / `item_mut`.
#[derive(Debug, Clone, Default)]
struct ItemIndex {
    map: HashMap<ItemId, usize>,
    /// Number of full rebuilds (observability for the PERF-8 regression tests).
    rebuilds: u64,
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
            index: RefCell::new(ItemIndex::default()),
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

    /// Position of an item in the global order. O(1) amortized: served from the validated
    /// id index, which is rebuilt only after `items` changed structurally.
    pub fn index_of(&self, id: ItemId) -> Option<usize> {
        let mut index = self.index.borrow_mut();
        if let Some(&idx) = index.map.get(&id) {
            if self.items.get(idx).is_some_and(|i| i.id == id) {
                return Some(idx);
            }
        }
        // Stale or missing entry. An id that is simply absent (removed item) costs one linear
        // scan, as before; any other miss means `items` was reordered or grown, so the whole
        // map is rebuilt once and later lookups are O(1) again.
        let found = self.items.iter().position(|i| i.id == id);
        if found.is_some() || index.map.contains_key(&id) {
            index.map.clear();
            index
                .map
                .extend(self.items.iter().enumerate().map(|(n, i)| (i.id, n)));
            index.rebuilds += 1;
        }
        found
    }

    pub fn item(&self, id: ItemId) -> Option<&BarItem> {
        self.index_of(id).map(|idx| &self.items[idx])
    }

    pub fn item_mut(&mut self, id: ItemId) -> Option<&mut BarItem> {
        self.index_of(id).map(|idx| &mut self.items[idx])
    }

    /// Full id-index rebuilds so far (PERF-8 regression tests).
    #[doc(hidden)]
    pub fn index_rebuilds(&self) -> u64 {
        self.index.borrow().rebuilds
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

    /// PERF-8: the id index stays correct whatever happens to the public `items` Vec.
    #[test]
    fn id_index_survives_direct_vec_mutation() {
        let mut res = HeadlessResources::default();
        let mut m = Model::new();
        let ids: Vec<ItemId> = (0..6).map(|_| m.create_item(&mut res)).collect();
        let check = |m: &Model| {
            for (n, it) in m.items.iter().enumerate() {
                assert_eq!(m.index_of(it.id), Some(n));
                assert_eq!(m.item(it.id).map(|i| i.id), Some(it.id));
            }
        };
        check(&m);
        m.items.swap(0, 5);
        check(&m);
        let removed = m.items.remove(2).id;
        assert_eq!(m.index_of(removed), None);
        assert!(m.item(removed).is_none());
        check(&m);
        let it = m.items.remove(0);
        m.items.insert(3, it);
        check(&m);
        m.items.reverse();
        check(&m);
        let fresh = m.create_item(&mut res);
        assert_eq!(m.index_of(fresh), Some(m.items.len() - 1));
        m.items = m.items.iter().rev().cloned().collect();
        check(&m);
        m.item_mut(ids[1]).unwrap().set_name("x");
        assert_eq!(m.find("x"), Some(ids[1]));
        assert_eq!(m.index_of(ItemId(9999)), None);
        let c = m.clone();
        check(&c);
    }

    /// PERF-8: `draws_item` resolves popup parents without rescanning `items`: once the
    /// index is warm, repeated visibility checks (refresh/layout on every input) and
    /// lookups of removed ids never rebuild it.
    #[test]
    fn popup_parent_lookup_uses_index() {
        let mut res = HeadlessResources::default();
        let mut m = Model::new();
        m.active_adid = 1;
        let bar = BarState::new(1, 1);
        for _ in 0..200 {
            m.create_item(&mut res);
        }
        let mut members = Vec::new();
        for _ in 0..50 {
            let host = m.create_item(&mut res);
            m.item_mut(host).unwrap().popup.drawing = true;
            for _ in 0..4 {
                let mi = m.create_item(&mut res);
                let it = m.item_mut(mi).unwrap();
                it.position = Position::Popup;
                it.parent = Some(host);
                members.push(mi);
            }
        }
        let gone = m.items.pop().unwrap().id;
        assert!(m.item(gone).is_none());
        let before = m.index_rebuilds();
        for _ in 0..100 {
            for it in &m.items {
                let drawn = m.draws_item(&bar, it);
                assert_eq!(drawn, it.id != gone);
            }
            assert!(m.item(gone).is_none());
        }
        assert_eq!(m.index_rebuilds(), before);
        // A structural change costs exactly one rebuild, then lookups are cached again.
        m.items.swap(0, 1);
        for it in &m.items {
            assert!(m.draws_item(&bar, it));
        }
        let first = m.items[0].id;
        assert_eq!(m.index_of(first), Some(0));
        assert_eq!(m.index_rebuilds(), before + 1);
        assert!(members
            .iter()
            .all(|id| m.item(*id).is_some() || *id == gone));
    }
}

//! Texture-atlas bookkeeping (pure logic, no GPU types).
//!
//! * [`ShelfPacker`]: shelf (row) packing of rectangles into one page.
//! * [`Atlas`]: multiple pages plus a key → placement map with page-granular LRU eviction.
//!   The owner mirrors page events ([`InsertOutcome`]) onto GPU textures.
//!
//! Eviction is per page: when no page has room and the page budget is exhausted, the
//! least recently used page that the GPU no longer reads (its last use is ≤ the last
//! *completed* frame serial) is cleared and reused; all its entries are dropped and will
//! be re-rasterized on demand. If every page is still in flight, a page is added beyond
//! the budget (trimmed again by later evictions).

use std::collections::HashMap;
use std::hash::Hash;

/// Gap (pixels) kept between packed rectangles so linear sampling never bleeds.
pub const GUTTER: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shelf {
    y: u32,
    height: u32,
    x: u32,
}

/// Shelf packer for one `width × height` page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShelfPacker {
    width: u32,
    height: u32,
    shelves: Vec<Shelf>,
    next_y: u32,
    used_area: u64,
}

impl ShelfPacker {
    pub fn new(width: u32, height: u32) -> Self {
        ShelfPacker {
            width,
            height,
            shelves: Vec::new(),
            next_y: 0,
            used_area: 0,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Fraction of the page area handed out.
    pub fn occupancy(&self) -> f32 {
        self.used_area as f32 / (self.width as f64 * self.height as f64).max(1.0) as f32
    }

    pub fn clear(&mut self) {
        self.shelves.clear();
        self.next_y = 0;
        self.used_area = 0;
    }

    /// Allocate `w × h`; returns the top-left corner. Picks the existing shelf with the
    /// least height waste (at most 50 % taller than `h`), else opens a new shelf.
    pub fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w == 0 || h == 0 || w > self.width || h > self.height {
            return None;
        }
        let gw = (w + GUTTER).min(self.width);
        let gh = (h + GUTTER).min(self.height);
        let mut best: Option<usize> = None;
        for (i, s) in self.shelves.iter().enumerate() {
            if s.height >= gh && s.height <= gh + gh / 2 && self.width - s.x >= gw {
                let better = match best {
                    None => true,
                    Some(b) => s.height < self.shelves[b].height,
                };
                if better {
                    best = Some(i);
                }
            }
        }
        if let Some(i) = best {
            let s = &mut self.shelves[i];
            let pos = (s.x, s.y);
            s.x += gw;
            self.used_area += w as u64 * h as u64;
            return Some(pos);
        }
        if self.height - self.next_y >= gh {
            let shelf = Shelf {
                y: self.next_y,
                height: gh,
                x: gw,
            };
            self.next_y += gh;
            self.shelves.push(shelf);
            self.used_area += w as u64 * h as u64;
            return Some((0, shelf.y));
        }
        None
    }
}

/// Where an entry lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub page: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Page-level side effects of [`Atlas::insert`], to be mirrored on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InsertOutcome {
    /// A new page `(index, width, height)` must get a texture.
    pub added: Option<(u32, u32, u32)>,
    /// This page was cleared (its texture is reused; old contents are garbage).
    pub cleared: Option<u32>,
    /// This page was removed (drop its texture).
    pub removed: Option<u32>,
}

#[derive(Debug)]
struct Page<K> {
    packer: ShelfPacker,
    /// Dedicated page holding a single oversized entry.
    dedicated: bool,
    last_used: u64,
    keys: Vec<K>,
}

/// Multi-page atlas with page-granular LRU eviction.
#[derive(Debug)]
pub struct Atlas<K> {
    page_width: u32,
    page_height: u32,
    max_pages: usize,
    pages: Vec<Option<Page<K>>>,
    entries: HashMap<K, Placement>,
}

impl<K: Hash + Eq + Clone> Atlas<K> {
    /// `max_pages` is the soft budget of regular pages.
    pub fn new(page_width: u32, page_height: u32, max_pages: usize) -> Self {
        Atlas {
            page_width,
            page_height,
            max_pages: max_pages.max(1),
            pages: Vec::new(),
            entries: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of live pages.
    pub fn page_count(&self) -> usize {
        self.pages.iter().filter(|p| p.is_some()).count()
    }

    /// Look up `key` and mark its page used by frame `serial`.
    pub fn get(&mut self, key: &K, serial: u64) -> Option<Placement> {
        let p = *self.entries.get(key)?;
        if let Some(Some(page)) = self.pages.get_mut(p.page as usize) {
            page.last_used = page.last_used.max(serial);
        }
        Some(p)
    }

    /// Allocate `w × h` for `key` (which must not be present) in frame `serial`.
    /// `completed` is the newest frame serial the GPU has finished; pages last used after
    /// it are never evicted. Returns `None` only for zero-sized requests.
    pub fn insert(
        &mut self,
        key: K,
        w: u32,
        h: u32,
        serial: u64,
        completed: u64,
    ) -> Option<(Placement, InsertOutcome)> {
        if w == 0 || h == 0 {
            return None;
        }
        let mut outcome = InsertOutcome::default();
        let oversized = w > self.page_width || h > self.page_height;
        if !oversized {
            // 1. Existing regular pages (most recently used first is unnecessary; any fit).
            for (i, page) in self.pages.iter_mut().enumerate() {
                let Some(page) = page else { continue };
                if page.dedicated {
                    continue;
                }
                if let Some((x, y)) = page.packer.alloc(w, h) {
                    let pl = Placement {
                        page: i as u32,
                        x,
                        y,
                        w,
                        h,
                    };
                    page.last_used = page.last_used.max(serial);
                    page.keys.push(key.clone());
                    self.entries.insert(key, pl);
                    return Some((pl, outcome));
                }
            }
            // 2. Evict the LRU idle regular page when over budget.
            let regular = self
                .pages
                .iter()
                .filter(|p| matches!(p, Some(p) if !p.dedicated))
                .count();
            if regular >= self.max_pages {
                if let Some(i) = self.lru_idle_page(completed, false) {
                    self.clear_page(i);
                    outcome.cleared = Some(i as u32);
                    let page = self.pages[i].as_mut().expect("live page");
                    let (x, y) = page.packer.alloc(w, h).expect("fits an empty regular page");
                    let pl = Placement {
                        page: i as u32,
                        x,
                        y,
                        w,
                        h,
                    };
                    page.last_used = serial;
                    page.keys.push(key.clone());
                    self.entries.insert(key, pl);
                    return Some((pl, outcome));
                }
            }
        } else {
            // Oversized entries get a dedicated page; drop one idle dedicated page so
            // they do not accumulate.
            if let Some(i) = self.lru_idle_page(completed, true) {
                self.clear_page(i);
                self.pages[i] = None;
                outcome.removed = Some(i as u32);
            }
        }
        // 3. New page.
        let (pw, ph) = if oversized {
            (w, h)
        } else {
            (self.page_width, self.page_height)
        };
        let index = match self.pages.iter().position(|p| p.is_none()) {
            Some(i) if outcome.removed != Some(i as u32) => i,
            _ => {
                self.pages.push(None);
                self.pages.len() - 1
            }
        };
        let mut packer = ShelfPacker::new(pw, ph);
        let (x, y) = packer.alloc(w, h).expect("fits an empty page");
        self.pages[index] = Some(Page {
            packer,
            dedicated: oversized,
            last_used: serial,
            keys: vec![key.clone()],
        });
        outcome.added = Some((index as u32, pw, ph));
        let pl = Placement {
            page: index as u32,
            x,
            y,
            w,
            h,
        };
        self.entries.insert(key, pl);
        Some((pl, outcome))
    }

    fn lru_idle_page(&self, completed: u64, dedicated: bool) -> Option<usize> {
        self.pages
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.as_ref().map(|p| (i, p)))
            .filter(|(_, p)| p.dedicated == dedicated && p.last_used <= completed)
            .min_by_key(|(_, p)| p.last_used)
            .map(|(i, _)| i)
    }

    fn clear_page(&mut self, i: usize) {
        if let Some(page) = self.pages[i].as_mut() {
            for k in page.keys.drain(..) {
                self.entries.remove(&k);
            }
            page.packer.clear();
        }
    }

    /// Drop everything (e.g. after a GPU device loss).
    pub fn clear(&mut self) {
        self.pages.clear();
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelf_packing_basic() {
        let mut p = ShelfPacker::new(100, 100);
        assert_eq!(p.alloc(10, 10), Some((0, 0)));
        assert_eq!(p.alloc(10, 10), Some((11, 0)));
        // Much shorter item still goes into the 10-high shelf? No: 4+1 = 5, shelf 11 > 5*1.5.
        assert_eq!(p.alloc(10, 4), Some((0, 11)));
        // A 9-high item fits the first shelf (11 <= 10 + 5).
        assert_eq!(p.alloc(10, 9), Some((22, 0)));
        assert_eq!(p.alloc(0, 5), None);
        assert_eq!(p.alloc(101, 5), None);
        assert!(p.occupancy() > 0.0);
    }

    #[test]
    fn shelf_packing_fills_up() {
        let mut p = ShelfPacker::new(64, 64);
        let mut n = 0;
        while p.alloc(15, 15).is_some() {
            n += 1;
        }
        // 64 / 16 = 4 per row, 4 rows.
        assert_eq!(n, 16);
        p.clear();
        assert_eq!(p.alloc(64, 64), Some((0, 0)));
        assert_eq!(p.alloc(1, 1), None);
    }

    #[test]
    fn shelf_no_overlap() {
        let mut p = ShelfPacker::new(256, 256);
        let mut rects = Vec::new();
        let sizes = [
            (30, 12),
            (7, 20),
            (100, 9),
            (50, 50),
            (3, 3),
            (80, 14),
            (12, 12),
        ];
        for i in 0..200 {
            let (w, h) = sizes[i % sizes.len()];
            if let Some((x, y)) = p.alloc(w, h) {
                assert!(x + w <= 256 && y + h <= 256);
                rects.push((x, y, w, h));
            }
        }
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                let disjoint =
                    a.0 + a.2 <= b.0 || b.0 + b.2 <= a.0 || a.1 + a.3 <= b.1 || b.1 + b.3 <= a.1;
                assert!(disjoint, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn atlas_pages_and_lookup() {
        let mut a: Atlas<u32> = Atlas::new(32, 32, 2);
        let (p0, o0) = a.insert(1, 31, 31, 1, 0).unwrap();
        assert_eq!(o0.added, Some((0, 32, 32)));
        assert_eq!((p0.page, p0.x, p0.y), (0, 0, 0));
        let (p1, o1) = a.insert(2, 31, 31, 1, 0).unwrap();
        assert_eq!(o1.added, Some((1, 32, 32)));
        assert_eq!(p1.page, 1);
        assert_eq!(a.get(&1, 2), Some(p0));
        assert_eq!(a.page_count(), 2);
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn atlas_evicts_lru_idle_page() {
        let mut a: Atlas<u32> = Atlas::new(32, 32, 2);
        a.insert(1, 31, 31, 1, 0).unwrap(); // page 0, used at 1
        a.insert(2, 31, 31, 2, 0).unwrap(); // page 1, used at 2
        a.get(&1, 3); // page 0 now used at 3
                      // Budget reached, GPU completed frame 2 → page 1 (last used 2) is evictable.
        let (p, o) = a.insert(3, 31, 31, 4, 2).unwrap();
        assert_eq!(o.cleared, Some(1));
        assert_eq!(o.added, None);
        assert_eq!(p.page, 1);
        assert_eq!(a.get(&2, 4), None);
        assert!(a.get(&1, 4).is_some());
    }

    #[test]
    fn atlas_grows_when_all_in_flight() {
        let mut a: Atlas<u32> = Atlas::new(32, 32, 1);
        a.insert(1, 31, 31, 5, 4).unwrap();
        // Page 0 used by frame 5, GPU only completed 4 → cannot evict; grow.
        let (p, o) = a.insert(2, 31, 31, 5, 4).unwrap();
        assert_eq!(o.added, Some((1, 32, 32)));
        assert_eq!(o.cleared, None);
        assert_eq!(p.page, 1);
    }

    #[test]
    fn atlas_dedicated_pages() {
        let mut a: Atlas<u32> = Atlas::new(32, 32, 4);
        let (p, o) = a.insert(1, 100, 10, 1, 0).unwrap();
        assert_eq!(o.added, Some((0, 100, 10)));
        assert_eq!((p.x, p.y), (0, 0));
        // Regular entries never land on a dedicated page.
        let (p2, _) = a.insert(2, 4, 4, 1, 0).unwrap();
        assert_eq!(p2.page, 1);
        // A second oversized entry removes the idle dedicated page and reuses its slot.
        let (p3, o3) = a.insert(3, 50, 50, 3, 2).unwrap();
        assert_eq!(o3.removed, Some(0));
        assert_eq!(o3.added, Some((2, 50, 50)));
        assert_eq!(p3.page, 2);
        assert_eq!(a.get(&1, 3), None);
        let (p4, o4) = a.insert(4, 60, 60, 4, 1).unwrap();
        assert_eq!(o4.removed, None); // page 2 used at 3 > completed 1
        assert_eq!(o4.added, Some((0, 60, 60)));
        assert_eq!(p4.page, 0);
    }

    #[test]
    fn atlas_zero_size() {
        let mut a: Atlas<u32> = Atlas::new(32, 32, 1);
        assert!(a.insert(1, 0, 3, 1, 0).is_none());
        assert!(a.is_empty());
    }
}

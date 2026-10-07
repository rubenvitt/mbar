//! Bracket membership (`group.c`, `docs/spec/item.md` §2.4, §5). Geometry is in
//! `layout.rs` (WP-A); these helpers are WP-C.

use crate::item::ItemId;
use crate::model::Model;

/// `group_add_member(bracket.group, member)` (`item.md` §2.4):
/// * already a member → no-op;
/// * `member` is itself a bracket → its members are added recursively (groups flatten);
/// * otherwise append it and set `member.group = bracket` (overwriting a previous group
///   pointer; the item stays in the old group's list, C behaviour).
pub fn add_member(model: &mut Model, bracket: ItemId, member: ItemId) {
    let _ = (model, bracket, member);
    todo!("WP-C: item.md §2.4")
}

/// Removes `member` from the bracket's list (item removal; `item.md` §10.5).
pub fn remove_member(model: &mut Model, bracket: ItemId, member: ItemId) {
    let _ = (model, bracket, member);
    todo!("WP-C: item.md §10.5")
}

/// Bracket removal: every member whose `group` is this bracket gets `group = None`.
pub fn destroy_group(model: &mut Model, bracket: ItemId) {
    let _ = (model, bracket);
    todo!("WP-C: item.md §10.5")
}

//! Bracket membership (`group.c`, `docs/spec/item.md` §2.4, §5). Geometry is in
//! `layout.rs` (WP-A); these helpers are WP-C.
//!
//! A bracket item is its own group: C stores the bracket as `members[0]`; mbar keeps the
//! bracket implicit and stores `members[1..]` in `BarItem::bracket_members`.

use crate::item::ItemId;
use crate::model::Model;

/// Recursion guard for flattening nested brackets (a bracket's member list never contains
/// brackets, but clones and odd configs should not be able to loop forever).
const MAX_DEPTH: u32 = 8;

/// `group_add_member(bracket.group, member)` (`item.md` §2.4):
/// * already a member → no-op;
/// * `member` is itself a bracket → its members are added recursively (groups flatten);
/// * otherwise append it and set `member.group = bracket` (overwriting a previous group
///   pointer; the item stays in the old group's list, C behaviour).
pub fn add_member(model: &mut Model, bracket: ItemId, member: ItemId) {
    add_member_depth(model, bracket, member, 0);
}

fn add_member_depth(model: &mut Model, bracket: ItemId, member: ItemId, depth: u32) {
    // `members[0]` is the bracket itself: adding it is "already a member".
    if member == bracket || depth > MAX_DEPTH {
        return;
    }
    let Some(b) = model.item(bracket) else { return };
    if b.bracket_members.contains(&member) {
        return;
    }
    let Some(m) = model.item(member) else { return };
    if m.is_bracket() {
        let nested = m.bracket_members.clone();
        for n in nested {
            add_member_depth(model, bracket, n, depth + 1);
        }
        return;
    }
    if let Some(b) = model.item_mut(bracket) {
        b.bracket_members.push(member);
    }
    if let Some(m) = model.item_mut(member) {
        m.group = Some(bracket);
    }
}

/// Removes `member` from the bracket's list (item removal; `item.md` §10.5).
pub fn remove_member(model: &mut Model, bracket: ItemId, member: ItemId) {
    if let Some(b) = model.item_mut(bracket) {
        b.bracket_members.retain(|m| *m != member);
    }
}

/// Bracket removal: every member whose `group` is this bracket gets `group = None`.
pub fn destroy_group(model: &mut Model, bracket: ItemId) {
    for it in &mut model.items {
        if it.group == Some(bracket) {
            it.group = None;
        }
    }
    if let Some(b) = model.item_mut(bracket) {
        b.bracket_members.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemType;
    use crate::platform::HeadlessResources;

    #[test]
    fn membership() {
        let mut res = HeadlessResources::default();
        let mut m = Model::new();
        let a = m.create_item(&mut res);
        let b = m.create_item(&mut res);
        let br = m.create_item(&mut res);
        m.item_mut(br).unwrap().item_type = ItemType::Bracket;
        add_member(&mut m, br, a);
        add_member(&mut m, br, a);
        add_member(&mut m, br, br);
        add_member(&mut m, br, b);
        assert_eq!(m.item(br).unwrap().bracket_members, vec![a, b]);
        assert_eq!(m.item(a).unwrap().group, Some(br));
        // nested bracket flattens
        let br2 = m.create_item(&mut res);
        m.item_mut(br2).unwrap().item_type = ItemType::Bracket;
        add_member(&mut m, br2, br);
        assert_eq!(m.item(br2).unwrap().bracket_members, vec![a, b]);
        assert_eq!(m.item(a).unwrap().group, Some(br2));
        remove_member(&mut m, br, a);
        assert_eq!(m.item(br).unwrap().bracket_members, vec![b]);
        destroy_group(&mut m, br2);
        assert_eq!(m.item(a).unwrap().group, None);
        assert!(m.item(br2).unwrap().bracket_members.is_empty());
    }
}

//! Slot layout engine (CD-09, D-0017) — pure geometry, no state.
//!
//! A **slot** is a fixed-width content column: [`Slots::width`] logical px wide,
//! as tall as the surf zone ([`Slots::height_frac`] of the window height,
//! vertically centered), with [`Slots::gutter`] between adjacent slots. The
//! group is horizontally centered and never comes within [`Slots::min_margin`]
//! of the screen edge; the Pulse Grid glows in the gutters and margins.
//!
//! These functions are the single source of truth for where slots sit — the
//! renderer draws each slot's page/placeholder at [`slot_rects`], and the shell
//! hit-tests the cursor against the same rects. They are deterministic and
//! side-effect-free so they can be unit-tested without a GPU or CEF (the CD-08
//! pattern).

use crate::theme::Slots;

/// Hard cap on live slots — the four-column product vision. The per-view arrays
/// in [`crate::browser`] are sized `MAX_SLOTS + 1` (the slots plus the one
/// shared internal overlay view), so this is also a compile-time bound.
pub const MAX_SLOTS: usize = 4;

/// A slot rectangle in device pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Does this rect contain the device-pixel point `(px, py)`? (Mouse router.)
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

/// How many slots of `t.width` (+ gutter) fit in `width_px` device pixels while
/// keeping at least `t.min_margin` on each side — clamped to `[1, MAX_SLOTS]`
/// (and to `t.max_count`). Never returns fewer than 1: a window narrower than a
/// single slot still shows one column (it just overflows the margins).
pub fn max_slots(width_px: u32, scale: f32, t: &Slots) -> usize {
    let unit = t.width * scale;
    let gutter = t.gutter * scale;
    let avail = width_px as f32 - 2.0 * t.min_margin * scale;
    let cap = (t.max_count as usize).clamp(1, MAX_SLOTS);
    if avail < unit {
        return 1;
    }
    // Largest n with n*unit + (n-1)*gutter <= avail.
    let n = ((avail + gutter) / (unit + gutter)).floor() as usize;
    n.clamp(1, cap)
}

/// The device-pixel rectangles for `n` slots on a `width`×`height` surface: each
/// `t.width` wide and `height_frac·height` tall (vertically centered), separated
/// by `t.gutter`, the whole group centered horizontally. `n` is clamped to at
/// least 1. Sizes are rounded to whole pixels so the columns stay crisp.
pub fn slot_rects(width: u32, height: u32, n: usize, scale: f32, t: &Slots) -> Vec<Rect> {
    let n = n.max(1);
    let unit = (t.width * scale).round();
    let gutter = (t.gutter * scale).round();
    let zh = (height as f32 * t.height_frac).round();
    let zy = ((height as f32 - zh) * 0.5).round();
    let total = unit * n as f32 + gutter * (n as f32 - 1.0);
    let x0 = ((width as f32 - total) * 0.5).round();
    (0..n)
        .map(|i| Rect {
            x: x0 + i as f32 * (unit + gutter),
            y: zy,
            w: unit,
            h: zh,
        })
        .collect()
}

// --- Slot-order management (CD-09 Stage B) ----------------------------------
// Pure helpers over the live-slot order list (`order`), separated from the shell
// so the Ctrl+T / Ctrl+W / Ctrl+1..4 / Ctrl+Tab index logic is unit-testable
// without a window or CEF. `order` holds live slot *ids* in display order; each
// id is a stable index into the fixed per-slot arrays.

/// The lowest free slot id not present in `order` (`0..MAX_SLOTS`), or `None`
/// when all `MAX_SLOTS` ids are in use.
pub fn free_id(order: &[usize]) -> Option<usize> {
    (0..MAX_SLOTS).find(|id| !order.contains(id))
}

/// The display position where Ctrl+T inserts a new slot: immediately right of
/// the active slot (or the end if `active` is somehow absent).
pub fn insert_position(order: &[usize], active: usize) -> usize {
    order
        .iter()
        .position(|&id| id == active)
        .map(|p| p + 1)
        .unwrap_or(order.len())
        .min(order.len())
}

/// After removing the slot at display position `pos`, the position of the slot
/// that should become active in the shortened order (length `len_after`): the
/// old right neighbor if there is one, else the new last (old left neighbor).
pub fn neighbor_position(pos: usize, len_after: usize) -> usize {
    pos.min(len_after.saturating_sub(1))
}

/// The next active display position when cycling (Ctrl+Tab forward /
/// Ctrl+Shift+Tab backward) from `pos` in an order of length `len`.
pub fn cycle_position(pos: usize, len: usize, forward: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if forward {
        (pos + 1) % len
    } else {
        (pos + len - 1) % len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The token values the "cyber" theme ships (theme.toml [slots]).
    fn slots() -> Slots {
        Slots {
            width: 1200.0,
            gutter: 24.0,
            min_margin: 48.0,
            height_frac: 0.70,
            max_count: 4,
            active_line: 2.0,
            placeholder_fill: 0.05,
            placeholder_glyph: 0.18,
        }
    }

    #[test]
    fn max_slots_matches_the_briefing_widths() {
        let t = slots();
        // "4 on 5120, 1 on 1920" — plus the intermediate ultrawide steps.
        assert_eq!(max_slots(1920, 1.0, &t), 1);
        assert_eq!(max_slots(2560, 1.0, &t), 2);
        assert_eq!(max_slots(3840, 1.0, &t), 3);
        assert_eq!(max_slots(5120, 1.0, &t), 4);
    }

    #[test]
    fn max_slots_never_below_one_and_capped_at_four() {
        let t = slots();
        // Narrower than a single slot -> still one column (never zero).
        assert_eq!(max_slots(800, 1.0, &t), 1);
        assert_eq!(max_slots(1, 1.0, &t), 1);
        // Absurdly wide -> capped at the four-column vision.
        assert_eq!(max_slots(20000, 1.0, &t), 4);
    }

    #[test]
    fn max_slots_honours_dpi_scale() {
        let t = slots();
        // At 2× DPI the slot needs twice the device px, so a 3840 panel that fits
        // three at 1× fits only one at 2× (1200·2 = 2400; 2·2400 + gutter > 3840).
        assert_eq!(max_slots(3840, 1.0, &t), 3);
        assert_eq!(max_slots(3840, 2.0, &t), 1);
    }

    #[test]
    fn max_count_token_can_lower_the_cap() {
        let mut t = slots();
        t.max_count = 2;
        assert_eq!(max_slots(5120, 1.0, &t), 2);
    }

    #[test]
    fn single_slot_is_centered_and_the_right_height() {
        let t = slots();
        let r = slot_rects(1600, 900, 1, 1.0, &t);
        assert_eq!(r.len(), 1);
        // 1200 wide, centered: x = (1600-1200)/2 = 200.
        assert_eq!(r[0].x, 200.0);
        assert_eq!(r[0].w, 1200.0);
        // 70% tall, vertically centered: h = 630, y = (900-630)/2 = 135.
        assert_eq!(r[0].h, 630.0);
        assert_eq!(r[0].y, 135.0);
    }

    #[test]
    fn four_slots_are_gutter_spaced_and_group_centered() {
        let t = slots();
        let r = slot_rects(5120, 1440, 4, 1.0, &t);
        assert_eq!(r.len(), 4);
        // Group width 4·1200 + 3·24 = 4872; x0 = (5120-4872)/2 = 124.
        assert_eq!(r[0].x, 124.0);
        // Each next slot is one unit + gutter to the right.
        assert_eq!(r[1].x, 124.0 + 1224.0);
        assert_eq!(r[2].x, 124.0 + 2.0 * 1224.0);
        assert_eq!(r[3].x, 124.0 + 3.0 * 1224.0);
        // All the same width, height and top.
        for slot in &r {
            assert_eq!(slot.w, 1200.0);
            assert_eq!(slot.h, (1440.0f32 * 0.70).round());
            assert_eq!(slot.y, r[0].y);
        }
        // Symmetric margins: left margin == right margin.
        let right_edge = r[3].x + r[3].w;
        assert_eq!(r[0].x, 5120.0 - right_edge);
    }

    #[test]
    fn gutter_between_slots_matches_the_token() {
        let t = slots();
        let r = slot_rects(5120, 1440, 3, 1.0, &t);
        let gap = r[1].x - (r[0].x + r[0].w);
        assert_eq!(gap, 24.0);
    }

    #[test]
    fn slot_rects_containment_maps_a_cursor_to_its_column() {
        let t = slots();
        let r = slot_rects(5120, 1440, 4, 1.0, &t);
        // Dead-centre of each slot is contained by exactly that slot.
        for (i, slot) in r.iter().enumerate() {
            let (cx, cy) = (slot.x + slot.w * 0.5, slot.y + slot.h * 0.5);
            let hit = r.iter().position(|q| q.contains(cx, cy));
            assert_eq!(hit, Some(i));
        }
        // A point in the first gutter is inside no column (routes nowhere).
        let gutter_x = r[0].x + r[0].w + 12.0;
        assert!(r.iter().all(|q| !q.contains(gutter_x, r[0].y + 10.0)));
    }

    #[test]
    fn rect_contains_is_inclusive_of_edges() {
        let r = Rect { x: 100.0, y: 50.0, w: 200.0, h: 400.0 };
        assert!(r.contains(100.0, 50.0));
        assert!(r.contains(300.0, 450.0));
        assert!(r.contains(200.0, 200.0));
        assert!(!r.contains(99.0, 200.0));
        assert!(!r.contains(200.0, 451.0));
    }

    #[test]
    fn free_id_picks_lowest_gap_then_none_when_full() {
        assert_eq!(free_id(&[0]), Some(1));
        assert_eq!(free_id(&[0, 1]), Some(2));
        // A hole in the middle is reused (stable ids, not contiguous).
        assert_eq!(free_id(&[0, 2]), Some(1));
        assert_eq!(free_id(&[0, 1, 2, 3]), None);
    }

    #[test]
    fn insert_position_is_right_of_active() {
        // active at the end -> append.
        assert_eq!(insert_position(&[0], 0), 1);
        assert_eq!(insert_position(&[0, 1, 2], 2), 3);
        // active in the middle -> insert just after it.
        assert_eq!(insert_position(&[0, 1, 2], 0), 1);
        assert_eq!(insert_position(&[0, 1, 2], 1), 2);
    }

    #[test]
    fn ctrl_t_inserts_a_free_id_right_of_active() {
        // order [0], active 0 -> add: free 1 at pos 1 -> [0,1], active 1.
        let mut order = vec![0usize];
        let free = free_id(&order).unwrap();
        let pos = insert_position(&order, 0);
        order.insert(pos, free);
        assert_eq!(order, vec![0, 1]);
        assert_eq!(free, 1);

        // Add again right of active 1 -> [0,1,2].
        let free = free_id(&order).unwrap();
        let pos = insert_position(&order, 1);
        order.insert(pos, free);
        assert_eq!(order, vec![0, 1, 2]);
    }

    #[test]
    fn neighbor_after_close_prefers_right_then_left() {
        // Close the middle of [a,b,c] at pos 1 -> [a,c]; the slot now at pos 1
        // (old right neighbor) becomes active.
        assert_eq!(neighbor_position(1, 2), 1);
        // Close the last of [a,b,c] at pos 2 -> [a,b]; the new last (pos 1,
        // old left neighbor) becomes active.
        assert_eq!(neighbor_position(2, 2), 1);
        // Close the first at pos 0 -> the new first (old right neighbor).
        assert_eq!(neighbor_position(0, 2), 0);
    }

    #[test]
    fn ctrl_w_removes_active_and_promotes_neighbor() {
        // [0,1,2], active 1 at pos 1 -> remove -> [0,2], new active pos 1 = id 2.
        let mut order = vec![0usize, 1, 2];
        let pos = order.iter().position(|&id| id == 1).unwrap();
        order.remove(pos);
        assert_eq!(order, vec![0, 2]);
        let new_pos = neighbor_position(pos, order.len());
        assert_eq!(order[new_pos], 2);
    }

    #[test]
    fn cycle_wraps_both_directions() {
        assert_eq!(cycle_position(0, 3, true), 1);
        assert_eq!(cycle_position(2, 3, true), 0); // wrap forward
        assert_eq!(cycle_position(0, 3, false), 2); // wrap backward
        assert_eq!(cycle_position(1, 3, false), 0);
        // Single slot: no movement.
        assert_eq!(cycle_position(0, 1, true), 0);
    }
}

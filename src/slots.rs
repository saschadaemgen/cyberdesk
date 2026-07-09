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

/// The largest slot-unit count that fits `budget` device pixels: the largest `n`
/// with `n·unit + (n-1)·gutter <= budget` (0 if not even one fits). Shared by
/// [`max_slots`] and [`frame_capacity`].
fn units_fitting(budget: f32, scale: f32, t: &Slots) -> usize {
    let unit = t.width * scale;
    let gutter = t.gutter * scale;
    if budget < unit {
        return 0;
    }
    ((budget + gutter) / (unit + gutter)).floor() as usize
}

/// How many slots of `t.width` (+ gutter) fit in `width_px` device pixels while
/// keeping at least `t.min_margin` on each side — clamped to `[1, MAX_SLOTS]`
/// (and to `t.max_count`). The pre-CD-11 no-side-zones fit; the shell now caps
/// against [`frame_capacity`] instead, but this remains as the tested building
/// block for the group math.
pub fn max_slots(width_px: u32, scale: f32, t: &Slots) -> usize {
    let cap = (t.max_count as usize).clamp(1, MAX_SLOTS);
    let avail = width_px as f32 - 2.0 * t.min_margin * scale;
    units_fitting(avail, scale, t).clamp(1, cap)
}

/// The device-pixel rectangles for `n` equal (single-unit) slots — a convenience
/// over [`slot_rects_units`]. The live code drives per-slot units directly; this
/// is exercised by the unit tests as the all-single oracle.
#[allow(dead_code)]
pub fn slot_rects(width: u32, height: u32, n: usize, scale: f32, t: &Slots) -> Vec<Rect> {
    let units = vec![1u32; n.max(1)];
    slot_rects_units(width, height, &units, scale, t)
}

/// The device-pixel rectangles for slots of the given per-slot width `units`
/// (1 or 2, CD-10): a `u`-unit slot spans `u·slot_width + (u-1)·gutter` (it
/// absorbs the internal gutter), slots are separated by `t.gutter`, and the whole
/// group is centered horizontally, `height_frac·height` tall (vertically
/// centered). A group of total units `U` occupies exactly the same extent as `U`
/// single-unit columns. `units` is treated as at least one 1-unit slot. Sizes are
/// rounded to whole pixels so the columns stay crisp.
pub fn slot_rects_units(width: u32, height: u32, units: &[u32], scale: f32, t: &Slots) -> Vec<Rect> {
    let unit = (t.width * scale).round();
    let gutter = (t.gutter * scale).round();
    let zh = (height as f32 * t.height_frac).round();
    let zy = ((height as f32 - zh) * 0.5).round();

    let widths: Vec<f32> = if units.is_empty() {
        vec![unit]
    } else {
        units
            .iter()
            .map(|&u| {
                let u = u.max(1) as f32;
                u * unit + (u - 1.0) * gutter
            })
            .collect()
    };
    let n = widths.len();
    let total: f32 = widths.iter().sum::<f32>() + gutter * (n as f32 - 1.0);
    let x0 = ((width as f32 - total) * 0.5).round();

    let mut out = Vec::with_capacity(n);
    let mut x = x0;
    for &w in &widths {
        out.push(Rect { x, y: zy, w, h: zh });
        x += w + gutter;
    }
    out
}

// --- The frame: side zones + reflow (CD-11, D-0020) --------------------------
// The slot group no longer owns the full width: a side zone flanks it left and
// right (placeholders now, the Spine / status-files-music rails later). When the
// slots demand the width the side zones retreat from `side_zone_width` (Full) to
// a thin `side_rail_width` (Rail). The whole frame — side | gutter | slots |
// gutter | side — is centered in the window; because it is symmetric, the slot
// group stays centered in the window (so `slot_rects_units` is reused unchanged),
// and the side zones flank it. Pure math: one function decides the state and all
// rects, so rendering and input read the same geometry (no incremental fudging).

/// Whether the side zones are shown at full width or retreated to thin rails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // consumed by the renderer + shell in Stage B
pub enum SideState {
    Full,
    Rail,
}

/// The full frame geometry for a given window size and slot-unit sequence.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // consumed by the renderer + shell in Stage B
pub struct FrameLayout {
    pub side_state: SideState,
    pub side_width: f32,
    pub slots: Vec<Rect>,
    pub left: Rect,
    pub right: Rect,
}

/// The largest total slot-unit budget the frame can hold at `width` — measured
/// against the **rail** center budget (the roomiest side state), so the shell
/// caps slots against the maximum the frame will ever fit. At least 1.
#[allow(dead_code)] // consumed by the shell in Stage B
pub fn frame_capacity(width: u32, scale: f32, t: &Slots) -> usize {
    let budget = width as f32 - 2.0 * t.side_rail_width * scale - 2.0 * t.gutter * scale;
    units_fitting(budget, scale, t).clamp(1, MAX_SLOTS * 2)
}

/// Decide the side state and lay out the whole frame for `slot_units`:
/// **Full** if the slot group plus full side zones (and their flanking gutters)
/// fits the window, else **Rail**. The slot rects are `slot_rects_units` (the
/// group is centered in the window either way); the side zones flank the group,
/// one gutter away, at the slot height. One call → all rects, so the animated
/// reflow (Stage B) drives a single interpolated `side_width` and both rendering
/// and input read the same per-frame geometry (desync-safe by construction).
#[allow(dead_code)] // consumed by the renderer + shell in Stage B
pub fn frame_layout(
    width: u32,
    height: u32,
    slot_units: &[u32],
    scale: f32,
    t: &Slots,
) -> FrameLayout {
    let u_total: u32 = slot_units.iter().map(|&u| u.max(1)).sum::<u32>().max(1);
    let unit = t.width * scale;
    let g = t.gutter * scale;
    // Group extent by the U-unit invariant (CD-10): a U-unit group spans as much
    // as U single columns.
    let group_w = u_total as f32 * unit + (u_total as f32 - 1.0) * g;
    let full_content = 2.0 * t.side_zone_width * scale + 2.0 * g + group_w;
    let (side_state, side_width) = if full_content <= width as f32 {
        (SideState::Full, (t.side_zone_width * scale).round())
    } else {
        (SideState::Rail, (t.side_rail_width * scale).round())
    };

    let slots = slot_rects_units(width, height, slot_units, scale, t);
    let (sy, sh) = slots.first().map(|r| (r.y, r.h)).unwrap_or((0.0, 0.0));
    let group_left = slots.first().map(|r| r.x).unwrap_or(width as f32 * 0.5);
    let group_right = slots
        .last()
        .map(|r| r.x + r.w)
        .unwrap_or(width as f32 * 0.5);
    let gutter = g.round();
    let left = Rect { x: group_left - gutter - side_width, y: sy, w: side_width, h: sh };
    let right = Rect { x: group_right + gutter, y: sy, w: side_width, h: sh };

    FrameLayout { side_state, side_width, slots, left, right }
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
            gutter: 40.0,
            min_margin: 48.0,
            height_frac: 0.70,
            max_count: 4,
            active_line: 2.0,
            placeholder_fill: 0.05,
            placeholder_glyph: 0.18,
            side_zone_width: 320.0,
            side_rail_width: 48.0,
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
        // Group width 4·1200 + 3·40 = 4920; x0 = (5120-4920)/2 = 100.
        assert_eq!(r[0].x, 100.0);
        // Each next slot is one unit + gutter (1240) to the right.
        assert_eq!(r[1].x, 100.0 + 1240.0);
        assert_eq!(r[2].x, 100.0 + 2.0 * 1240.0);
        assert_eq!(r[3].x, 100.0 + 3.0 * 1240.0);
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
        assert_eq!(gap, 40.0);
    }

    #[test]
    fn slot_rects_units_all_single_matches_slot_rects() {
        let t = slots();
        for n in 1..=4 {
            let units = vec![1u32; n];
            assert_eq!(
                slot_rects_units(5120, 1440, &units, 1.0, &t),
                slot_rects(5120, 1440, n, 1.0, &t),
                "units {n} all-single should match slot_rects"
            );
        }
    }

    #[test]
    fn double_slot_spans_two_columns_plus_gutter() {
        let t = slots();
        // A double slot absorbs the internal gutter: 2·1200 + 40 = 2440.
        let r = slot_rects_units(5120, 1440, &[2, 1], 1.0, &t);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].w, 2440.0);
        assert_eq!(r[1].w, 1200.0);
        // Gutter between the double and the single is the normal token gutter.
        assert_eq!(r[1].x - (r[0].x + r[0].w), 40.0);
    }

    #[test]
    fn mixed_units_occupy_the_same_extent_as_equal_columns() {
        let t = slots();
        // [2,1] (3 units) centers exactly like three single columns.
        let mixed = slot_rects_units(5120, 1440, &[2, 1], 1.0, &t);
        let three = slot_rects(5120, 1440, 3, 1.0, &t);
        assert_eq!(mixed[0].x, three[0].x, "group left edge aligns");
        assert_eq!(
            mixed[1].x + mixed[1].w,
            three[2].x + three[2].w,
            "group right edge aligns"
        );
        // Two doubles (4 units) span exactly like four columns.
        let doubles = slot_rects_units(5120, 1440, &[2, 2], 1.0, &t);
        let four = slot_rects(5120, 1440, 4, 1.0, &t);
        assert_eq!(doubles[0].x, four[0].x);
        assert_eq!(doubles[1].x + doubles[1].w, four[3].x + four[3].w);
        assert_eq!(doubles[0].w, 2440.0);
        assert_eq!(doubles[1].w, 2440.0);
    }

    #[test]
    fn mixed_units_stay_centered_and_symmetric() {
        let t = slots();
        let r = slot_rects_units(5120, 1440, &[1, 2], 1.0, &t);
        let left_margin = r[0].x;
        let right_margin = 5120.0 - (r[1].x + r[1].w);
        assert_eq!(left_margin, right_margin);
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

    // --- Frame math (CD-11) -------------------------------------------------

    fn units(n: usize) -> Vec<u32> {
        vec![1u32; n]
    }

    #[test]
    fn frame_state_full_until_the_slots_demand_the_width() {
        let t = slots();
        // 5120: 1..3 single slots fit with full side zones; the 4th forces rails.
        assert_eq!(frame_layout(5120, 1440, &units(1), 1.0, &t).side_state, SideState::Full);
        assert_eq!(frame_layout(5120, 1440, &units(2), 1.0, &t).side_state, SideState::Full);
        assert_eq!(frame_layout(5120, 1440, &units(3), 1.0, &t).side_state, SideState::Full);
        assert_eq!(frame_layout(5120, 1440, &units(4), 1.0, &t).side_state, SideState::Rail);
    }

    #[test]
    fn frame_state_depends_on_total_units_not_slot_count() {
        let t = slots();
        // Two double slots = 4 units — same as four singles → Rail on 5120.
        assert_eq!(frame_layout(5120, 1440, &[2, 2], 1.0, &t).side_state, SideState::Rail);
        // One double + one single = 3 units → still Full.
        assert_eq!(frame_layout(5120, 1440, &[2, 1], 1.0, &t).side_state, SideState::Full);
    }

    #[test]
    fn frame_capacity_matches_the_side_zone_budgets() {
        let t = slots();
        // Side zones eat the width, so mid-size panels hold fewer slots than the
        // pre-CD-11 max_slots; the 5120 ultrawide still reaches four.
        assert_eq!(frame_capacity(1920, 1.0, &t), 1);
        assert_eq!(frame_capacity(2560, 1.0, &t), 1);
        assert_eq!(frame_capacity(3840, 1.0, &t), 2);
        assert_eq!(frame_capacity(5120, 1.0, &t), 4);
    }

    #[test]
    fn four_slots_fit_at_rail_on_the_ultrawide_with_margin() {
        let t = slots();
        let f = frame_layout(5120, 1440, &units(4), 1.0, &t);
        assert_eq!(f.side_state, SideState::Rail);
        // The whole frame (left rail | gutter | slots | gutter | right rail) stays
        // on-screen with a non-negative edge margin.
        assert!(f.left.x >= 0.0, "left rail on-screen: x={}", f.left.x);
        assert!(f.right.x + f.right.w <= 5120.0, "right rail on-screen");
        assert_eq!(f.left.w, 48.0);
        assert_eq!(f.right.w, 48.0);
    }

    #[test]
    fn side_zones_flank_the_group_symmetrically_at_the_slot_height() {
        let t = slots();
        let f = frame_layout(5120, 1440, &units(2), 1.0, &t);
        assert_eq!(f.side_state, SideState::Full);
        assert_eq!(f.left.w, 320.0);
        assert_eq!(f.right.w, 320.0);
        // One gutter between each side zone and the slot group.
        let group_left = f.slots.first().unwrap().x;
        let group_right = f.slots.last().map(|r| r.x + r.w).unwrap();
        assert_eq!(group_left - (f.left.x + f.left.w), 40.0);
        assert_eq!(f.right.x - group_right, 40.0);
        // Side zones share the slot height/top; the frame is symmetric.
        assert_eq!(f.left.y, f.slots[0].y);
        assert_eq!(f.left.h, f.slots[0].h);
        assert_eq!(f.left.x, 5120.0 - (f.right.x + f.right.w));
    }

    #[test]
    fn full_to_rail_boundary_is_the_first_count_that_overflows_full() {
        let t = slots();
        // Construct the boundary directly from the tokens rather than hardcoding:
        // the largest U whose group + full sides fits is Full; U+1 is Rail.
        let g = t.gutter;
        let full_fits = |u: u32| {
            let group = u as f32 * t.width + (u as f32 - 1.0) * g;
            2.0 * t.side_zone_width + 2.0 * g + group <= 5120.0
        };
        // Find the boundary U where Full stops fitting.
        let mut boundary = 1u32;
        while full_fits(boundary + 1) {
            boundary += 1;
        }
        assert_eq!(frame_layout(5120, 1440, &units(boundary as usize), 1.0, &t).side_state, SideState::Full);
        // One more unit tips into Rail (and it is within rail capacity here).
        assert!((boundary as usize + 1) <= frame_capacity(5120, 1.0, &t));
        assert_eq!(frame_layout(5120, 1440, &units(boundary as usize + 1), 1.0, &t).side_state, SideState::Rail);
    }

    #[test]
    fn frame_slot_rects_match_the_bare_group_layout() {
        // The frame reuses slot_rects_units unchanged (group centered in the
        // window); the side zones are additive, they do not move the slots.
        let t = slots();
        for n in 1..=4 {
            let f = frame_layout(5120, 1440, &units(n), 1.0, &t);
            assert_eq!(f.slots, slot_rects_units(5120, 1440, &units(n), 1.0, &t));
        }
    }
}

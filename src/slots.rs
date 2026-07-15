//! Slot layout engine (CD-09, D-0017) — pure geometry, no state.
//!
//! A **slot** is a fixed-width content column: [`Slots::width`] logical px wide,
//! as tall as the surf zone (the window height minus the explicit
//! [`Slots::zone_top`] / [`Slots::zone_bottom`] margins — CD-30 Task A; the old
//! centered `height_frac` left 15% dead space above AND below), with
//! [`Slots::gutter`] between adjacent slots. The group is horizontally centered
//! and never comes within [`Slots::min_margin`] of the screen edge; the Pulse
//! Grid glows in the gutters and margins.
//!
//! These functions are the single source of truth for where slots sit — the
//! renderer draws each slot's page/placeholder at [`slot_rects`], and the shell
//! hit-tests the cursor against the same rects. They are deterministic and
//! side-effect-free so they can be unit-tested without a GPU or CEF (the CD-08
//! pattern).

use crate::theme::Slots;

/// Compile-time ceiling on live slots — the per-view arrays in [`crate::browser`]
/// are sized `MAX_SLOTS + 1` (the slots plus the one shared internal overlay
/// view), so this is a hard array bound. The **product** cap is the tunable
/// `slots.slot_max` token (D-0022: three), which must stay `<= MAX_SLOTS`; the
/// capacity / count math clamps against it, so a fourth column never opens while
/// `slot_max = 3`. Kept at 4 so the token can be raised without resizing arrays.
pub const MAX_SLOTS: usize = 4;

/// A slot rectangle in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
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
/// keeping at least `t.min_margin` on each side — clamped to `[1, slot_max]`
/// (and to `t.max_count`, and the `MAX_SLOTS` array ceiling). The no-side-zones
/// fit; the shell caps against [`frame_capacity`] instead (which accounts for the
/// zones), so this is now a tested building block for the group math only.
#[allow(dead_code)]
pub fn max_slots(width_px: u32, scale: f32, t: &Slots) -> usize {
    let cap = (t.max_count as usize)
        .min(t.slot_max as usize)
        .clamp(1, MAX_SLOTS);
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

/// The surf zone's vertical extent `(top, height)` in device px (CD-30 Task A):
/// explicit top/bottom margins instead of the old centered fraction, so the
/// browsing area is as tall as the frame allows. Degenerate (very short) windows
/// keep at least 40% of the height as zone rather than going negative.
pub fn zone_vertical(height: u32, scale: f32, t: &Slots) -> (f32, f32) {
    let h = height as f32;
    let top = (t.zone_top * scale).round();
    let bottom = (t.zone_bottom * scale).round();
    let zh = (h - top - bottom).max((h * 0.4).round());
    (top.min((h - zh).max(0.0)).round(), zh.round())
}

/// The device-pixel rectangles for slots of the given per-slot width `units`
/// (1 or 2, CD-10): a `u`-unit slot spans `u·slot_width + (u-1)·gutter` (it
/// absorbs the internal gutter), slots are separated by `t.gutter`, and the whole
/// group is centered horizontally, spanning the [`zone_vertical`] extent. A group
/// of total units `U` occupies exactly the same extent as `U` single-unit
/// columns. `units` is treated as at least one 1-unit slot. Sizes are rounded to
/// whole pixels so the columns stay crisp.
pub fn slot_rects_units(width: u32, height: u32, units: &[u32], scale: f32, t: &Slots) -> Vec<Rect> {
    let unit = (t.width * scale).round();
    let gutter = (t.gutter * scale).round();
    let (zy, zh) = zone_vertical(height, scale, t);

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

// --- The frame: asymmetric zones + reflow (CD-11 D-0020, revised D-0022) ------
// The slot group does not own the full width: a zone flanks it on each side. The
// REVISED law (D-0022) makes the frame ASYMMETRIC:
//   * The RIGHT zone is the **Multifunctional (MF) zone** — PERMANENT. It is
//     always `mf_zone_width`, at every resolution; it never rails.
//   * The LEFT zone (future Spine) is the **flexible** one: `side_zone_width`
//     (Full) when the slots leave room for it alongside the permanent MF zone,
//     else it retreats to a thin `side_rail_width` (Rail). The CD-11 reflow law
//     and its animation-safety now apply to the LEFT zone alone.
// The whole frame — left | gutter | slots | gutter | MF — is centered in the
// window. Because it is asymmetric, the slot group is NOT window-centered: it
// sits offset toward the smaller zone (equivalently: the frame block is centered
// and the group laid inside it). CD-30: the MF zone doubles while the Terminal
// tab is active, and the columns compress (floored) rather than close when the
// frame would overflow. One call → all rects, so rendering and input read the
// same geometry (desync-safe).

/// Whether the flexible (left / Spine) zone is shown at full width or retreated
/// to a thin rail. The right MF zone is permanent and has no such state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideState {
    Full,
    Rail,
}

/// The full frame geometry for a given window size and slot-unit sequence. The
/// shell drives the reflow off `left_width` + the rects (the MF/right zone is a
/// constant `mf_zone_width`); `left_state` is read by the tests and the control
/// surface (hence the allow).
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct FrameLayout {
    /// The flexible left (Spine) zone's state.
    pub left_state: SideState,
    /// The flexible left (Spine) zone's width (the animated one). The MF/right
    /// zone width is the constant `mf_zone_width` (read off `right.w`).
    pub left_width: f32,
    pub slots: Vec<Rect>,
    /// The left (Spine) zone rect — flexes Full ↔ Rail.
    pub left: Rect,
    /// The right (MF) zone rect — permanent, always `mf_zone_width` wide.
    pub right: Rect,
}

/// The largest total slot-unit budget the frame can hold at `width` — measured
/// against the roomiest state (the LEFT zone at **rail**, the MF zone always at
/// its permanent width), so the shell caps slots against the maximum the frame
/// will ever fit. At least 1, capped at the `slot_max` unit ceiling.
pub fn frame_capacity(width: u32, scale: f32, t: &Slots) -> usize {
    let budget = width as f32
        - (t.mf_zone_width + t.side_rail_width) * scale
        - 2.0 * t.gutter * scale;
    let unit_ceiling = (t.slot_max as usize).clamp(1, MAX_SLOTS) * 2;
    units_fitting(budget, scale, t).clamp(1, unit_ceiling)
}

/// The MF zone's current width in device px: the permanent `mf_zone_width`,
/// doubled while the Terminal tab is active (CD-30 Task A — "terminal twice as
/// wide"). The doubling is a live layout state, not a token, so `frame_capacity`
/// deliberately keeps using the NARROW width: opening the terminal must never
/// close a column, it only compresses them (see [`frame_layout`]).
pub fn mf_width_px(scale: f32, t: &Slots, mf_wide: bool) -> f32 {
    let w = if mf_wide { t.mf_zone_width * 2.0 } else { t.mf_zone_width };
    (w * scale).round()
}

/// Decide the left-zone state and lay out the whole (asymmetric) frame for
/// `slot_units`: the LEFT zone is **Full** if the slot group plus the full left
/// zone, the MF zone (2× wide while the terminal is shown — `mf_wide`), and
/// their flanking gutters fit the window, else **Rail**. The frame block
/// (left | gutter | slots | gutter | MF) is centered in the window; when even
/// the Rail state cannot hold the nominal columns (the wide terminal), the
/// columns COMPRESS proportionally toward `slot_min_width` instead of closing
/// (CD-30) — they return to nominal width when the terminal hides.
///
/// `locked` (CD-30 Task D, the red "bunker" mode) carries an optional FIXED
/// `(w, h)` in device px per display position: a locked column keeps exactly
/// that size — it neither compresses nor stretches — and is vertically centered
/// in the zone; only the UNLOCKED columns absorb compression. A lock the frame
/// genuinely cannot hold is clamped to what fits (the caller ladders the
/// requested size down so this is the last resort, not the norm). Missing
/// trailing entries mean "not locked".
///
/// One call → all rects, so the animated reflow drives the interpolated
/// `left_width` and both rendering and input read the same per-frame geometry
/// (desync-safe by construction).
pub fn frame_layout(
    width: u32,
    height: u32,
    slot_units: &[u32],
    scale: f32,
    t: &Slots,
    mf_wide: bool,
    locked: &[Option<(f32, f32)>],
) -> FrameLayout {
    let unit = (t.width * scale).round();
    let g = (t.gutter * scale).round();
    let mf_width = mf_width_px(scale, t, mf_wide);
    let (zy, zh) = zone_vertical(height, scale, t);
    let lock_at = |i: usize| locked.get(i).copied().flatten();

    // Nominal per-slot widths by the U-unit invariant (CD-10): a u-unit column
    // spans u·unit + (u-1)·gutter, so a U-unit group spans as much as U singles.
    // A locked column's width is its fixed lock width (capped to the zone).
    let n_slots = slot_units.len().max(1);
    let mut widths: Vec<f32> = (0..n_slots)
        .map(|i| {
            if let Some((lw, _)) = lock_at(i) {
                return lw.round();
            }
            let u = slot_units.get(i).copied().unwrap_or(1).max(1) as f32;
            u * unit + (u - 1.0) * g
        })
        .collect();
    let n = widths.len() as f32;
    let gutters_w = g * (n - 1.0);
    let group_w = widths.iter().sum::<f32>() + gutters_w;

    // The left (Spine) zone goes full only if the whole frame (full left + MF +
    // group + both gutters) fits; the MF zone is permanent either way.
    let full_content = (t.side_zone_width * scale).round() + mf_width + 2.0 * g + group_w;
    let (left_state, left_width) = if full_content <= width as f32 {
        (SideState::Full, (t.side_zone_width * scale).round())
    } else {
        (SideState::Rail, (t.side_rail_width * scale).round())
    };

    // Compression (CD-30): the group must fit between the zones. Squeeze the
    // UNLOCKED columns proportionally, floored at `slot_min_width` — never close
    // a column for a transient layout state, never touch a locked column (its
    // size is the point). If the locked columns alone still overflow, clamp them
    // as the last resort so the frame stays on-screen.
    let avail = (width as f32 - left_width - mf_width - 2.0 * g - gutters_w).max(0.0);
    let total: f32 = widths.iter().sum();
    if total > avail {
        let floor = (t.slot_min_width * scale).round();
        let locked_w: f32 = (0..n_slots).filter(|&i| lock_at(i).is_some()).map(|i| widths[i]).sum();
        let free_w: f32 = total - locked_w;
        let free_avail = (avail - locked_w).max(0.0);
        if free_w > 0.0 && free_w > free_avail {
            let f = free_avail / free_w;
            for (i, w) in widths.iter_mut().enumerate() {
                if lock_at(i).is_none() {
                    *w = (*w * f).round().max(floor);
                }
            }
        }
        // Last resort: locked columns wider than everything available shrink to
        // fit (the caller's ladder normally prevents this).
        let total_now: f32 = widths.iter().sum();
        if total_now > avail && locked_w > 0.0 {
            let over = total_now - avail;
            let f = ((locked_w - over) / locked_w).max(0.0);
            for (i, w) in widths.iter_mut().enumerate() {
                if lock_at(i).is_some() {
                    *w = (*w * f).round();
                }
            }
        }
    }
    let group_w = widths.iter().sum::<f32>() + gutters_w;

    // Center the frame block; clamp to the left edge if it (degenerately) still
    // overflows after the compression floor.
    let frame_w = left_width + g + group_w + g + mf_width;
    let fx = ((width as f32 - frame_w) * 0.5).max(0.0).round();

    // A locked column keeps its fixed height (capped to the zone), vertically
    // centered; unlocked columns span the full zone.
    let mut slots = Vec::with_capacity(widths.len());
    let mut x = fx + left_width + g;
    for (i, &w) in widths.iter().enumerate() {
        let (y, h) = match lock_at(i) {
            Some((_, lh)) => {
                let h = lh.round().min(zh);
                (zy + ((zh - h) * 0.5).round(), h)
            }
            None => (zy, zh),
        };
        slots.push(Rect { x, y, w, h });
        x += w + g;
    }

    let group_left = slots.first().map(|r| r.x).unwrap_or(width as f32 * 0.5);
    let group_right = slots.last().map(|r| r.x + r.w).unwrap_or(width as f32 * 0.5);
    let left = Rect { x: group_left - g - left_width, y: zy, w: left_width, h: zh };
    let right = Rect { x: group_right + g, y: zy, w: mf_width, h: zh };

    FrameLayout { left_state, left_width, slots, left, right }
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
            gutter: 56.0,
            min_margin: 48.0,
            zone_top: 118.0,
            zone_bottom: 44.0,
            slot_min_width: 480.0,
            max_count: 4,
            slot_max: 3,
            active_line: 2.0,
            placeholder_fill: 0.05,
            placeholder_glyph: 0.18,
            side_zone_width: 320.0,
            side_rail_width: 48.0,
            mf_zone_width: 320.0,
        }
    }

    #[test]
    fn max_slots_matches_the_revised_widths() {
        let t = slots();
        // The bare group fit (no zones), gutter 56, capped at slot_max = 3.
        assert_eq!(max_slots(1920, 1.0, &t), 1);
        assert_eq!(max_slots(2560, 1.0, &t), 2);
        assert_eq!(max_slots(3840, 1.0, &t), 3);
        assert_eq!(max_slots(5120, 1.0, &t), 3); // slot_max caps what would be 4
    }

    #[test]
    fn max_slots_never_below_one_and_capped_at_slot_max() {
        let t = slots();
        // Narrower than a single slot -> still one column (never zero).
        assert_eq!(max_slots(800, 1.0, &t), 1);
        assert_eq!(max_slots(1, 1.0, &t), 1);
        // Absurdly wide -> capped at the three-column product maximum.
        assert_eq!(max_slots(20000, 1.0, &t), 3);
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
        // CD-30: explicit margins — y = zone_top, h = 900 - 118 - 44 = 738
        // (the old 70% centering wasted 135 px top AND bottom here).
        assert_eq!(r[0].y, 118.0);
        assert_eq!(r[0].h, 738.0);
    }

    #[test]
    fn zone_fills_the_height_minus_the_explicit_margins() {
        let t = slots();
        let (zy, zh) = zone_vertical(1440, 1.0, &t);
        assert_eq!((zy, zh), (118.0, 1278.0));
        // DPI-scaled margins.
        let (zy2, zh2) = zone_vertical(2880, 2.0, &t);
        assert_eq!((zy2, zh2), (236.0, 2556.0));
        // Degenerate short window: at least 40% of the height stays zone, the
        // top clamps so the zone remains on-screen.
        let (zy3, zh3) = zone_vertical(200, 1.0, &t);
        assert_eq!(zh3, 80.0);
        assert!(zy3 + zh3 <= 200.0);
    }

    #[test]
    fn three_slots_are_gutter_spaced_and_group_centered() {
        let t = slots();
        let r = slot_rects(5120, 1440, 3, 1.0, &t);
        assert_eq!(r.len(), 3);
        // Group width 3·1200 + 2·56 = 3712; x0 = (5120-3712)/2 = 704.
        assert_eq!(r[0].x, 704.0);
        // Each next slot is one unit + gutter (1256) to the right.
        assert_eq!(r[1].x, 704.0 + 1256.0);
        assert_eq!(r[2].x, 704.0 + 2.0 * 1256.0);
        // All the same width, height and top (CD-30 explicit margins).
        for slot in &r {
            assert_eq!(slot.w, 1200.0);
            assert_eq!(slot.h, 1440.0 - 118.0 - 44.0);
            assert_eq!(slot.y, 118.0);
        }
        // Symmetric margins: left margin == right margin (the bare group, no zones).
        let right_edge = r[2].x + r[2].w;
        assert_eq!(r[0].x, 5120.0 - right_edge);
    }

    #[test]
    fn gutter_between_slots_matches_the_token() {
        let t = slots();
        let r = slot_rects(5120, 1440, 3, 1.0, &t);
        let gap = r[1].x - (r[0].x + r[0].w);
        assert_eq!(gap, 56.0);
    }

    #[test]
    fn slot_rects_units_all_single_matches_slot_rects() {
        let t = slots();
        for n in 1..=3 {
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
        // A double slot absorbs the internal gutter: 2·1200 + 56 = 2456.
        let r = slot_rects_units(5120, 1440, &[2, 1], 1.0, &t);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].w, 2456.0);
        assert_eq!(r[1].w, 1200.0);
        // Gutter between the double and the single is the normal token gutter.
        assert_eq!(r[1].x - (r[0].x + r[0].w), 56.0);
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
        // Two doubles (4 units) span exactly like four columns (pure U-invariant
        // math; four singles exceed the live slot_max but the extent still holds).
        let doubles = slot_rects_units(5120, 1440, &[2, 2], 1.0, &t);
        let four = slot_rects(5120, 1440, 4, 1.0, &t);
        assert_eq!(doubles[0].x, four[0].x);
        assert_eq!(doubles[1].x + doubles[1].w, four[3].x + four[3].w);
        assert_eq!(doubles[0].w, 2456.0);
        assert_eq!(doubles[1].w, 2456.0);
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
        let r = slot_rects(5120, 1440, 3, 1.0, &t);
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

    // --- Frame math (CD-11, revised D-0022) ---------------------------------

    fn units(n: usize) -> Vec<u32> {
        vec![1u32; n]
    }

    #[test]
    fn left_zone_full_when_it_fits_alongside_the_slots_else_rails() {
        let t = slots();
        // One slot: the full left zone + permanent MF doesn't fit at 1920 (Rail),
        // but does at 2560 (Full).
        assert_eq!(frame_layout(1920, 1440, &units(1), 1.0, &t, false, &[]).left_state, SideState::Rail);
        assert_eq!(frame_layout(2560, 1440, &units(1), 1.0, &t, false, &[]).left_state, SideState::Full);
        // On the ultrawide all three slots leave room for the full left zone.
        assert_eq!(frame_layout(5120, 1440, &units(1), 1.0, &t, false, &[]).left_state, SideState::Full);
        assert_eq!(frame_layout(5120, 1440, &units(2), 1.0, &t, false, &[]).left_state, SideState::Full);
        assert_eq!(frame_layout(5120, 1440, &units(3), 1.0, &t, false, &[]).left_state, SideState::Full);
    }

    #[test]
    fn mf_zone_is_permanent_at_every_resolution() {
        let t = slots();
        // The right MF zone is always mf_zone_width, whatever the left zone does.
        for w in [1920u32, 2560, 3440, 5120] {
            let f = frame_layout(w, 1440, &units(1), 1.0, &t, false, &[]);
            assert_eq!(f.right.w, 320.0, "MF permanent at {w}");
            assert!(f.right.x + f.right.w <= w as f32, "MF on-screen at {w}");
        }
    }

    #[test]
    fn frame_reflow_depends_on_total_units_not_slot_count() {
        let t = slots();
        // At 3000 a single slot's full left zone fits, but a double's (2 units)
        // does not — so it depends on the unit total, not the column count.
        assert_eq!(frame_layout(3000, 1440, &units(1), 1.0, &t, false, &[]).left_state, SideState::Full);
        assert_eq!(frame_layout(3000, 1440, &[2], 1.0, &t, false, &[]).left_state, SideState::Rail);
    }

    #[test]
    fn frame_capacity_matches_the_revised_budgets() {
        let t = slots();
        // The permanent MF zone + a left rail + wider gutters eat the width, so
        // mid-size panels hold fewer slots; two slots now need ~3000, three the
        // ultrawide. Capped at slot_max (three).
        assert_eq!(frame_capacity(1920, 1.0, &t), 1);
        assert_eq!(frame_capacity(2560, 1.0, &t), 1);
        assert_eq!(frame_capacity(3000, 1.0, &t), 2); // the ~3000 two-slot threshold
        assert_eq!(frame_capacity(3440, 1.0, &t), 2);
        assert_eq!(frame_capacity(5120, 1.0, &t), 3);
    }

    #[test]
    fn frame_capacity_never_exceeds_the_slot_max_unit_ceiling() {
        let t = slots();
        // Absurdly wide: capped at slot_max·2 units (three double-width columns).
        assert_eq!(frame_capacity(40000, 1.0, &t), (t.slot_max as usize) * 2);
    }

    #[test]
    fn three_slots_and_both_full_zones_fit_the_ultrawide() {
        let t = slots();
        let f = frame_layout(5120, 1440, &units(3), 1.0, &t, false, &[]);
        assert_eq!(f.left_state, SideState::Full);
        assert_eq!(f.left.w, 320.0);
        assert_eq!(f.right.w, 320.0);
        // The whole frame (left | gutter | 3 slots | gutter | MF) is on-screen.
        assert!(f.left.x >= 0.0, "left zone on-screen: x={}", f.left.x);
        assert!(f.right.x + f.right.w <= 5120.0, "MF on-screen");
        // Prove the token budget the briefing asked for: 320 + 56 + 3712 + 56 +
        // 320 = 4464 ≤ 5120.
        let group = 3.0 * t.width + 2.0 * t.gutter;
        assert_eq!(t.side_zone_width + t.gutter + group + t.gutter + t.mf_zone_width, 4464.0);
        assert!(4464.0 <= 5120.0);
    }

    #[test]
    fn floor_law_one_slot_mf_and_left_rail_at_1920() {
        let t = slots();
        // The minimum working set (D-0022): exactly one slot + the MF zone + the
        // left rail, all on-screen at 1920 with balanced margins.
        assert_eq!(frame_capacity(1920, 1.0, &t), 1);
        let f = frame_layout(1920, 1440, &units(1), 1.0, &t, false, &[]);
        assert_eq!(f.left_state, SideState::Rail);
        assert_eq!(f.left.w, 48.0);
        assert_eq!(f.right.w, 320.0);
        assert!(f.left.x >= 0.0, "left rail on-screen: x={}", f.left.x);
        assert!(f.right.x + f.right.w <= 1920.0, "MF on-screen");
        // The frame block is centered, so the outer margins balance.
        let left_margin = f.left.x;
        let right_margin = 1920.0 - (f.right.x + f.right.w);
        assert_eq!(left_margin, right_margin);
    }

    #[test]
    fn zones_flank_the_group_one_gutter_away_at_the_slot_height() {
        let t = slots();
        let f = frame_layout(5120, 1440, &units(2), 1.0, &t, false, &[]);
        let group_left = f.slots.first().unwrap().x;
        let group_right = f.slots.last().map(|r| r.x + r.w).unwrap();
        // One gutter between each zone and the slot group.
        assert_eq!(group_left - (f.left.x + f.left.w), 56.0);
        assert_eq!(f.right.x - group_right, 56.0);
        // Both zones share the slot height and top.
        assert_eq!(f.left.y, f.slots[0].y);
        assert_eq!(f.left.h, f.slots[0].h);
        assert_eq!(f.right.y, f.slots[0].y);
        assert_eq!(f.right.h, f.slots[0].h);
    }

    #[test]
    fn asymmetric_frame_shifts_the_group_toward_the_smaller_zone() {
        let t = slots();
        // With the left at rail (48) and the MF permanent (320), the group shifts
        // LEFT of window-center by (left_width - mf_width)/2 = -136.
        let f = frame_layout(1920, 1440, &units(1), 1.0, &t, false, &[]);
        let centered = slot_rects_units(1920, 1440, &units(1), 1.0, &t);
        let dx = (f.left_width - f.right.w) * 0.5; // (48 - 320)/2 = -136
        assert_eq!(dx, -136.0);
        assert_eq!(f.slots[0].x, centered[0].x + dx);
        // With both zones full (5120, 3 slots) the difference is zero → centered.
        let ff = frame_layout(5120, 1440, &units(3), 1.0, &t, false, &[]);
        let cc = slot_rects_units(5120, 1440, &units(3), 1.0, &t);
        assert_eq!(ff.slots, cc);
    }

    #[test]
    fn full_to_rail_boundary_within_capacity() {
        let t = slots();
        // At 3000 the boundary is reachable within capacity (2 units): a single
        // slot is Full, a two-unit group is Rail.
        assert!(frame_capacity(3000, 1.0, &t) >= 2);
        assert_eq!(frame_layout(3000, 1440, &units(1), 1.0, &t, false, &[]).left_state, SideState::Full);
        assert_eq!(frame_layout(3000, 1440, &[2], 1.0, &t, false, &[]).left_state, SideState::Rail);
    }

    #[test]
    fn frame_slots_are_the_centered_group_translated_by_the_zone_difference() {
        let t = slots();
        for n in 1..=3 {
            let f = frame_layout(5120, 1440, &units(n), 1.0, &t, false, &[]);
            let centered = slot_rects_units(5120, 1440, &units(n), 1.0, &t);
            let dx = ((f.left_width - f.right.w) * 0.5).round();
            for (a, b) in f.slots.iter().zip(centered.iter()) {
                assert_eq!(a.x, b.x + dx);
                assert_eq!(a.w, b.w);
                assert_eq!(a.y, b.y);
                assert_eq!(a.h, b.h);
            }
        }
    }

    // --- CD-30 Task A: wide terminal + column compression --------------------

    #[test]
    fn wide_mf_doubles_the_zone_and_compresses_columns_instead_of_closing() {
        let t = slots();
        // At 1920 a single 1200 column + narrow MF fits; the 2×-wide terminal
        // does not — the column compresses (1920 - 48 rail - 640 MF - 2 gutters
        // = 1120), it does NOT close, and the whole frame stays on-screen.
        let f = frame_layout(1920, 1440, &units(1), 1.0, &t, true, &[]);
        assert_eq!(f.right.w, 640.0, "MF zone is 2x wide while the terminal shows");
        assert_eq!(f.slots.len(), 1, "no column closes for a transient layout state");
        assert_eq!(f.slots[0].w, 1120.0);
        assert!(f.left.x >= 0.0);
        assert!(f.right.x + f.right.w <= 1920.0, "frame on-screen");
        // Hiding the terminal returns the column to its nominal width.
        let back = frame_layout(1920, 1440, &units(1), 1.0, &t, false, &[]);
        assert_eq!(back.right.w, 320.0);
        assert_eq!(back.slots[0].w, 1200.0);
    }

    #[test]
    fn wide_mf_leaves_columns_untouched_when_there_is_room() {
        let t = slots();
        // The ultrawide holds three 1200 columns + the full left zone + the wide
        // terminal — nothing compresses.
        let f = frame_layout(5120, 1440, &units(3), 1.0, &t, true, &[]);
        assert_eq!(f.left_state, SideState::Full);
        assert_eq!(f.right.w, 640.0);
        for s in &f.slots {
            assert_eq!(s.w, 1200.0);
        }
        assert!(f.right.x + f.right.w <= 5120.0);
    }

    #[test]
    fn compression_floors_at_slot_min_width() {
        let t = slots();
        // Degenerately narrow: two columns squeeze to the floor, never below,
        // and the frame clamps to the left edge instead of going negative.
        let f = frame_layout(1400, 900, &units(2), 1.0, &t, false, &[]);
        assert_eq!(f.slots.len(), 2);
        for s in &f.slots {
            assert_eq!(s.w, 480.0, "compression floors at slot_min_width");
        }
        assert!(f.left.x >= 0.0, "frame block clamps at the window edge");
    }

    // --- CD-30 Task D: the red bunker mode's viewport lock -------------------

    #[test]
    fn red_lock_pins_the_column_to_the_standard_size() {
        let t = slots();
        // One window at Red on a 2560×1440 display: the viewport locks to
        // exactly 1920×1080, vertically centered in the zone; the frame stays
        // on-screen with the left rail + permanent MF around it.
        let lock = [Some((1920.0, 1080.0))];
        let f = frame_layout(2560, 1440, &units(1), 1.0, &t, false, &lock);
        assert_eq!(f.slots[0].w, 1920.0);
        assert_eq!(f.slots[0].h, 1080.0);
        // Vertically centered in the zone (zy 118, zh 1278).
        assert_eq!(f.slots[0].y, 118.0 + ((1278.0 - 1080.0) / 2.0f32).round());
        assert!(f.left.x >= 0.0 && f.right.x + f.right.w <= 2560.0);
        // The zones keep the full zone height — only the locked column shrinks.
        assert_eq!(f.right.h, 1278.0);
    }

    #[test]
    fn red_lock_makes_the_neighbors_absorb_the_squeeze() {
        let t = slots();
        // Two columns at 3440, the first locked to 1920×1080: the locked column
        // keeps its exact size, the unlocked neighbor compresses to what remains
        // (never the locked one), nothing closes.
        let lock = [Some((1920.0, 1080.0)), None];
        let f = frame_layout(3440, 1440, &units(2), 1.0, &t, false, &lock);
        assert_eq!(f.slots.len(), 2);
        assert_eq!(f.slots[0].w, 1920.0, "locked column never compresses");
        assert_eq!(f.slots[0].h, 1080.0);
        // avail = 3440 − rail 48 − MF 320 − 2·56 − 56 = 2904; neighbor = 984.
        assert_eq!(f.slots[1].w, 984.0);
        assert_eq!(f.slots[1].h, 1278.0, "unlocked column keeps the zone height");
        assert!(f.right.x + f.right.w <= 3440.0);
    }

    #[test]
    fn red_lock_clamps_only_as_the_last_resort() {
        let t = slots();
        // A lock the display genuinely cannot hold (the caller's ladder normally
        // prevents this) is clamped to what fits instead of overflowing.
        let lock = [Some((1920.0, 1080.0))];
        let f = frame_layout(1500, 900, &units(1), 1.0, &t, false, &lock);
        // avail = 1500 − 48 − 320 − 112 = 1020; zone height = 738.
        assert_eq!(f.slots[0].w, 1020.0);
        assert_eq!(f.slots[0].h, 738.0);
        assert!(f.left.x >= 0.0 && f.right.x + f.right.w <= 1500.0);
    }

    #[test]
    fn unlocking_restores_the_nominal_layout_exactly() {
        let t = slots();
        // The lock is layout-only: the same units with no lock reproduce the
        // pre-Red geometry bit-for-bit (stepping down restores the layout).
        let before = frame_layout(2560, 1440, &units(1), 1.0, &t, false, &[]);
        let locked = frame_layout(2560, 1440, &units(1), 1.0, &t, false, &[Some((1920.0, 1080.0))]);
        let after = frame_layout(2560, 1440, &units(1), 1.0, &t, false, &[]);
        assert_ne!(before.slots[0], locked.slots[0]);
        assert_eq!(before, after);
    }

    #[test]
    fn compression_proportional_between_floor_and_nominal() {
        let t = slots();
        // Wide terminal at 3000 with a double+single group (3 units): avail =
        // 3000 - 48 - 640 - 2·56 - 56 = 2144, nominal 2456+1200 = 3656 → both
        // compress proportionally (f ≈ 0.5865) above the 480 floor.
        let f = frame_layout(3000, 1440, &[2, 1], 1.0, &t, true, &[]);
        let total: f32 = f.slots.iter().map(|r| r.w).sum();
        assert!(total <= 2144.0 + 2.0, "group fits the available span");
        assert!(f.slots[0].w > f.slots[1].w, "proportional: the double stays wider");
        for s in &f.slots {
            assert!(s.w >= 480.0);
        }
    }
}

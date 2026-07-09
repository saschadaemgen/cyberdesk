//! Session workspace persistence (CD-10, D-0019) — the domain layer over the
//! shared SQLite [`Store`] for the slot workspace, mirroring [`crate::memory`].
//!
//! One implicit session: the full ordered slot list (url + width + which was
//! active) is written wholesale on each meaningful change (debounced host-side)
//! and read back on startup. Internal `cyberdesk://` / blank / empty slots
//! persist as an empty URL; only real web pages carry a URL across a restart.
//! All local — no sync, no export.

use crate::store::{self, SessionSlot};

/// The URL a slot persists into the session: its real page, or empty for an
/// internal / blank / empty slot (same filter as history/favorites, D-0014).
pub fn persist_url(url: &str) -> String {
    if crate::memory::is_recordable(url) {
        url.to_string()
    } else {
        String::new()
    }
}

/// Load the saved slot workspace (empty on a fresh install / no session).
pub fn load() -> Vec<SessionSlot> {
    store::shared().lock().unwrap().load_session()
}

/// Persist the slot workspace (replaces the whole table).
pub fn save(slots: &[SessionSlot]) {
    store::shared().lock().unwrap().save_session(slots);
}

/// One displayed slot in a restore plan: its width and its pre-armed URL (`None`
/// for an empty slot).
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedSlot {
    pub width_units: u32,
    pub url: Option<String>,
}

/// The plan for restoring a saved session on startup.
#[derive(Debug, Clone, PartialEq)]
pub struct RestorePlan {
    /// Displayed slots, left to right; the shell assigns id = index.
    pub slots: Vec<PlannedSlot>,
    /// Display index of the slot to make active.
    pub active: usize,
    /// Saved slots that did not fit the current width (kept for a wider restart).
    pub overflow: Vec<SessionSlot>,
}

/// Plan how a saved (non-empty) session fits the current width: take slots from
/// the left while the running unit total stays within `cap` (and the count within
/// `max_slots`); the rest go to overflow. Pure and unit-tested — the shell does
/// the browser spawning from the result. If nothing fits (absurdly narrow), one
/// empty slot is planned so the shell always has a column.
pub fn plan_restore(saved: &[SessionSlot], cap: u32, max_slots: usize) -> RestorePlan {
    let mut slots = Vec::new();
    let mut overflow = Vec::new();
    let mut used = 0u32;
    let mut active = None;
    for s in saved {
        let u = s.width_units.clamp(1, 2);
        if slots.len() < max_slots && used + u <= cap {
            if s.active {
                active = Some(slots.len());
            }
            slots.push(PlannedSlot {
                width_units: u,
                url: if s.url.is_empty() { None } else { Some(s.url.clone()) },
            });
            used += u;
        } else {
            overflow.push(s.clone());
        }
    }
    if slots.is_empty() {
        slots.push(PlannedSlot { width_units: 1, url: None });
    }
    let active = active.unwrap_or(0).min(slots.len() - 1);
    RestorePlan { slots, active, overflow }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(url: &str, units: u32, active: bool) -> SessionSlot {
        SessionSlot { url: url.into(), width_units: units, active }
    }

    #[test]
    fn persist_url_keeps_web_pages_drops_internal_and_blank() {
        assert_eq!(persist_url("https://a.example/"), "https://a.example/");
        assert_eq!(persist_url(""), "");
        assert_eq!(persist_url("about:blank"), "");
        assert_eq!(persist_url("cyberdesk://command/"), "");
    }

    #[test]
    fn plan_restore_fits_all_when_they_fit() {
        let saved = vec![
            slot("https://a/", 1, false),
            slot("https://b/", 1, true),
            slot("", 1, false),
        ];
        let plan = plan_restore(&saved, 4, 4);
        assert_eq!(plan.slots.len(), 3);
        assert!(plan.overflow.is_empty());
        assert_eq!(plan.active, 1);
        assert_eq!(plan.slots[0].url.as_deref(), Some("https://a/"));
        assert_eq!(plan.slots[2].url, None); // empty slot
    }

    #[test]
    fn plan_restore_overflows_from_the_left_by_units() {
        // cap = 2 units: a 2-unit slot fills it, the rest overflow.
        let saved = vec![
            slot("https://wide/", 2, true),
            slot("https://b/", 1, false),
            slot("https://c/", 1, false),
        ];
        let plan = plan_restore(&saved, 2, 4);
        assert_eq!(plan.slots.len(), 1);
        assert_eq!(plan.slots[0].width_units, 2);
        assert_eq!(plan.active, 0);
        assert_eq!(plan.overflow.len(), 2);
        assert_eq!(plan.overflow[0].url, "https://b/");
    }

    #[test]
    fn plan_restore_respects_both_unit_cap_and_max_slots() {
        // Four 1-unit slots, cap 4 units but max_slots 2 -> only two fit.
        let saved = vec![
            slot("https://a/", 1, false),
            slot("https://b/", 1, false),
            slot("https://c/", 1, false),
            slot("https://d/", 1, false),
        ];
        let plan = plan_restore(&saved, 4, 2);
        assert_eq!(plan.slots.len(), 2);
        assert_eq!(plan.overflow.len(), 2);
    }

    #[test]
    fn plan_restore_active_falls_back_when_active_overflowed() {
        // The active slot doesn't fit -> active defaults to the first displayed.
        let saved = vec![slot("https://a/", 1, false), slot("https://b/", 1, true)];
        let plan = plan_restore(&saved, 1, 4);
        assert_eq!(plan.slots.len(), 1);
        assert_eq!(plan.active, 0);
        assert_eq!(plan.overflow.len(), 1);
        assert!(plan.overflow[0].active);
    }

    #[test]
    fn plan_restore_forces_one_slot_when_nothing_fits() {
        let saved = vec![slot("https://wide/", 2, true)];
        // cap 1 unit can't hold a 2-unit slot -> it overflows, one empty forced.
        let plan = plan_restore(&saved, 1, 4);
        assert_eq!(plan.slots.len(), 1);
        assert_eq!(plan.slots[0].url, None);
        assert_eq!(plan.active, 0);
        assert_eq!(plan.overflow.len(), 1);
    }
}

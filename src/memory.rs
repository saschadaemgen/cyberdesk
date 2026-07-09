//! History + favorites domain layer over the shared SQLite [`Store`].
//!
//! History records every surf-view page visit (URL + title, a visit count and a
//! last-visit time); favorites are the user's Ctrl+D / star toggles. Both live
//! only in the local `state.db` (D-0014) — no sync, no export. Internal
//! `cyberdesk://` pages and blank navigations never enter either table.

use crate::store::{self, Suggestion};

/// URLs that must never be recorded or favorited: the internal scheme and blank
/// navigations. Only real web pages from the surf view enter history/favorites.
/// Also the filter for what a slot persists into the session (CD-10): an
/// internal/blank/empty slot persists as an empty URL.
pub fn is_recordable(url: &str) -> bool {
    !url.is_empty() && url != "about:blank" && !url.starts_with("cyberdesk://")
}

/// Record a visit to `url` (bumping its visit count). No-op for internal/blank.
pub fn record_visit(url: &str, title: &str) {
    if is_recordable(url) {
        store::shared().lock().unwrap().record_visit(url, title);
    }
}

/// Refresh the stored title of `url`'s history row (the title arrives after the
/// address commit). No-op for internal/blank or an empty title.
pub fn update_title(url: &str, title: &str) {
    if is_recordable(url) && !title.is_empty() {
        store::shared()
            .lock()
            .unwrap()
            .update_history_title(url, title);
    }
}

/// Is `url` currently a favorite?
pub fn is_favorite(url: &str) -> bool {
    is_recordable(url) && store::shared().lock().unwrap().is_favorite(url)
}

/// Toggle `url`'s favorite state; returns the new state. No-op (returns false)
/// for internal/blank URLs.
pub fn toggle_favorite(url: &str, title: &str) -> bool {
    toggle_favorite_in(&store::shared().lock().unwrap(), url, title)
}

/// The guarded toggle against an explicit store — the exact logic the surf-view
/// Ctrl+D shortcut runs (`is_recordable` filter + [`store::Store::toggle_favorite`]).
/// Split out so the regression harness drives the shortcut's real path against a
/// throwaway store rather than reaching into the store directly.
fn toggle_favorite_in(store: &store::Store, url: &str, title: &str) -> bool {
    is_recordable(url) && store.toggle_favorite(url, title)
}

/// Command-palette suggestions for `input` (favorites first, then history by
/// frecency), capped at `limit`. Empty input returns the top favorites.
pub fn query_suggestions(input: &str, limit: usize) -> Vec<Suggestion> {
    store::shared().lock().unwrap().query_suggestions(input, limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    /// The CD-08 repro through the shortcut's own code path: favorite two
    /// different pages in a row (as surf-view Ctrl+D does), then read back the
    /// empty-input palette list (as the reveal shows). Both must be there — the
    /// bug was that only the current page's favorite was ever visible.
    #[test]
    fn ctrl_d_on_two_pages_keeps_both_favorites() {
        let s = Store::open_in_memory();
        assert!(toggle_favorite_in(&s, "https://one.example/", "One"));
        assert!(toggle_favorite_in(&s, "https://two.example/", "Two"));

        let favs = s.query_suggestions("", 6);
        assert_eq!(favs.len(), 2, "both favorites must survive the second Ctrl+D");
    }

    /// The guard keeps internal/blank URLs out of favorites (they are never a
    /// real page), so a Ctrl+D on `cyberdesk://` or a blank tab is a no-op.
    #[test]
    fn internal_and_blank_urls_are_not_favoritable() {
        let s = Store::open_in_memory();
        assert!(!toggle_favorite_in(&s, "", "empty"));
        assert!(!toggle_favorite_in(&s, "about:blank", "blank"));
        assert!(!toggle_favorite_in(&s, "cyberdesk://settings/", "settings"));
        assert_eq!(s.query_suggestions("", 6).len(), 0);
    }
}

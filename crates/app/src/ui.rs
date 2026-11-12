//! The layout kit: where things sit on screen, and how a tap finds them.
//!
//! The app has no `bevy_ui`. Every screen lays itself out in window
//! fractions — a horizontal *band* is `(top, bottom)` as fractions of the
//! window height, y-down — and every tap is tested against those bands.
//! Each screen used to carry its own copy of the shared pieces: four
//! identical BACK bands, two identical `in_band` functions, and the
//! band-to-anchor conversion written out by hand thirty-odd times. When one
//! copy moved and the others did not, taps landed on the wrong thing (SHARE
//! sat inside the strip every screen reads as BACK, and fired both).
//!
//! One kit now: the shared bands, the hit test, the anchor math, and each
//! screen's layout table, with tests that fail when two tap targets on one
//! screen overlap. Change a band here and every screen that uses it moves.

use bevy::prelude::*;

/// A horizontal strip of the window: `(top, bottom)` as fractions of its
/// height, y-down (0 is the top edge).
pub type Band = (f32, f32);

/// Is the window-fraction `y_down` inside `band`? Top inclusive, bottom
/// exclusive, so two bands that share an edge never both claim a tap.
pub const fn in_band(y_down: f32, band: Band) -> bool {
    y_down >= band.0 && y_down < band.1
}

/// Did any of `taps` (window pixels, y-down) land in `band`?
pub fn tapped(taps: impl IntoIterator<Item = Vec2>, band: Band, window: Vec2) -> bool {
    window.y > 0.0 && taps.into_iter().any(|p| in_band(p.y / window.y, band))
}

/// `ScreenAnchor`'s y for a window-fraction `y_down`: the anchor runs from
/// +1 at the top edge to -1 at the bottom.
pub const fn anchor_y(y_down: f32) -> f32 {
    1.0 - 2.0 * y_down
}

/// `ScreenAnchor`'s x for a window-fraction `x`: -1 at the left edge, +1 at
/// the right.
pub const fn anchor_x(x: f32) -> f32 {
    x * 2.0 - 1.0
}

/// The anchor y of a band's centre.
pub const fn band_anchor_y(band: Band) -> f32 {
    1.0 - (band.0 + band.1)
}

/// Do two bands overlap? Sharing an edge is not overlapping.
#[cfg_attr(not(test), allow(dead_code))]
pub const fn overlaps(a: Band, b: Band) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// One strip, one meaning, every screen: a tap here goes BACK — leave the
/// online match, back to the couch lobby, exit the tape. It starts below
/// any camera cutout or status bar (the v9 device pass moved every target
/// off the notch; keep it that way).
pub const TOP_EXIT: Band = (0.05, 0.16);

/// The menu screens' BACK button along the bottom (settings, the roster,
/// rivals, the replay list).
pub const BACK: Band = (0.86, 0.96);

/// The title's bands. The table is the pitch, so it owns the middle of the
/// screen: banner, then the diorama, then one column of actions with the
/// primary last and biggest. Never overlapping (the test below):
/// diorama 0.18–0.50 · subline · secondary row · mode toggle · private
/// dial · primary, which stops clear of the phone's nav-bar / gesture
/// strip (~last 6%).
pub mod title {
    use super::Band;

    /// The line under the diorama: the arena name (tap for the roster) or
    /// the career record.
    pub const SUBLINE: Band = (0.52, 0.58);
    /// PRACTICE · RIVALS · REPLAYS · SETTINGS, split on window-x.
    pub const SECONDARY_ROW: Band = (0.59, 0.66);
    /// QUICK MATCH | PRIVATE, directly on top of the button it relabels.
    pub const MODE_TOGGLE: Band = (0.68, 0.74);
    /// The private dial's row — empty while QUICK MATCH is selected.
    pub const DIAL: Band = (0.75, 0.81);
    /// The primary: PLAY / FIND OPPONENT / DUEL AT C-U-R-S.
    pub const PLAY: Band = (0.82, 0.93);

    /// Every title tap target, top to bottom.
    #[cfg_attr(not(test), allow(dead_code))]
    pub const ALL: [Band; 5] = [SUBLINE, SECONDARY_ROW, MODE_TOGGLE, DIAL, PLAY];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_edge_belongs_to_the_lower_band_only() {
        assert!(in_band(0.16, (0.16, 0.2)));
        assert!(!in_band(0.16, TOP_EXIT));
        assert!(!overlaps(TOP_EXIT, (0.16, 0.2)));
        assert!(overlaps(TOP_EXIT, (0.15, 0.2)));
    }

    #[test]
    fn anchors_run_from_top_to_bottom() {
        assert_eq!(anchor_y(0.0), 1.0);
        assert_eq!(anchor_y(1.0), -1.0);
        assert_eq!(anchor_x(0.5), 0.0);
        assert_eq!(band_anchor_y((0.4, 0.6)), 0.0);
        assert!((band_anchor_y(BACK) - anchor_y(0.91)).abs() < 1e-6);
    }

    #[test]
    fn taps_are_tested_in_window_pixels() {
        let window = Vec2::new(400.0, 1000.0);
        assert!(tapped([Vec2::new(10.0, 900.0)], BACK, window));
        assert!(!tapped([Vec2::new(10.0, 500.0)], BACK, window));
        assert!(!tapped([Vec2::new(10.0, 900.0)], BACK, Vec2::ZERO));
    }

    #[test]
    fn no_two_title_targets_overlap_and_none_reaches_the_exit_strip() {
        for (i, a) in title::ALL.iter().enumerate() {
            assert!(a.0 < a.1, "band {a:?} is upside down");
            assert!(!overlaps(*a, TOP_EXIT), "{a:?} reaches the exit strip");
            for b in &title::ALL[i + 1..] {
                assert!(!overlaps(*a, *b), "{a:?} overlaps {b:?}");
            }
        }
        // The primary stops clear of the gesture strip at the bottom.
        assert!(title::PLAY.1 <= 0.94);
    }

    #[test]
    fn the_menu_back_button_is_clear_of_the_exit_strip() {
        assert!(!overlaps(BACK, TOP_EXIT));
    }
}

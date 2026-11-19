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

// ---- Draw order: the named z layers ----

/// The z layers screen-space things draw on, by name. A visuals pass
/// reorders the stack here. Things within a layer sit at small offsets
/// from it (`MENU_TEXT - 0.5` is a menu text's backing).
pub mod z {
    /// The in-match HUD: pips, clock, held pickup.
    pub const HUD: f32 = 50.0;
    /// The desktop keyboard legend and other match-screen notes.
    pub const NOTE: f32 = 100.0;
    /// The round countdown glyphs.
    pub const COUNTDOWN: f32 = 150.0;
    /// A menu screen's boxes and plates (over the match).
    pub const MENU: f32 = 200.0;
    /// A menu screen's text and the top of the stack.
    pub const MENU_TEXT: f32 = 210.0;
}

// ---- Text roles: every text size in one table ----

/// What a piece of text is, and so how big it is drawn. `default_size` is
/// the one table of every text size in the game (today's values, so the
/// visuals audit starts from a pixel-identical baseline); `Tuning.text`
/// overrides any role by name and live-reloads (`TWOTOP_TUNING`), and
/// [`apply_text_roles`] resizes text already on screen. A role sits beside
/// its `TextFont` in the spawn bundle.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextRole {
    /// A full screen's title (REPLAYS, SETTINGS).
    ScreenTitle,
    /// The line under a screen title.
    ScreenSubtitle,
    /// A page heading (THE TABLES, THE RIVALS).
    PageTitle,
    /// Every BACK.
    Back,
    /// The Title's 2-TOP.
    Wordmark,
    /// The player's name on the Title.
    TitleName,
    /// The Title's status lines.
    TitleOverlay,
    /// The Title's main button.
    TitlePlay,
    /// The Title's smaller buttons.
    TitleMenu,
    /// The match summary card.
    SummaryCard,
    /// A summary's primary button.
    ButtonPrimary,
    /// A summary's LEAVE / LOBBY.
    ButtonExit,
    /// The in-match QUIT.
    ButtonQuit,
    /// The room-mode pills.
    PillLabel,
    /// A glyph of the private dial.
    CodeGlyph,
    /// The caption under a join QR.
    QrCaption,
    /// The name being dialed.
    NameEntry,
    /// A letter key of the name pad.
    NameKey,
    /// The name pad's prompt.
    NamePrompt,
    /// The name pad's SKIP.
    NameSkip,
    /// The name pad's DEL.
    NameDel,
    /// The name pad's DONE.
    NameDone,
    /// A coaching callout plate.
    Callout,
    /// The DASH zone label.
    TouchDash,
    /// The TAUNT zone label.
    TouchTaunt,
    /// A touch-control hint.
    TouchHint,
    /// A notice line on a list screen.
    Notice,
    /// A list row (a tape).
    ListRow,
    /// A list row's detail line.
    ListDetail,
    /// A list row's fine print.
    ListFine,
    /// The theater's marquee.
    Marquee,
    /// An arena's name on the roster.
    RosterName,
    /// An arena's rule line.
    RosterRule,
    /// The desktop keyboard legend.
    KeyLegend,
    /// The SHARE label.
    ShareLabel,
    /// A share link under its QR.
    ShareUrl,
    /// TAP TO CLOSE.
    ShareDismiss,
    /// Developer overlays.
    Debug,
    /// A settings group header.
    GroupHeader,
    /// A setting's name.
    SettingLabel,
    /// A setting's value.
    SettingValue,
    /// Settings' HOW TO PLAY.
    SettingsHowTo,
    /// The online wait's words.
    Summoning,
    /// The crash notice.
    CrashNotice,
    /// The intro card's arena name.
    IntroTitle,
    /// The intro card's rule line.
    IntroSub,
    /// NO RIVALS YET.
    RivalsEmpty,
    /// A rival's name in the list.
    RivalRowName,
    /// A rival row's standing.
    RivalRowDetail,
    /// A rival row's second line.
    RivalRowSub,
    /// A rival's name on their page.
    RivalName,
    /// A rival's meetings and standing.
    RivalStanding,
    /// A fact about a rivalry.
    RivalFact,
    /// SPAR THEIR SHADE.
    RivalShade,
    /// No tapes of a rivalry.
    RivalNoTapes,
    /// A rivalry's tape.
    RivalTape,
}

impl TextRole {
    pub const ALL: [TextRole; 56] = [
        TextRole::ScreenTitle,
        TextRole::ScreenSubtitle,
        TextRole::PageTitle,
        TextRole::Back,
        TextRole::Wordmark,
        TextRole::TitleName,
        TextRole::TitleOverlay,
        TextRole::TitlePlay,
        TextRole::TitleMenu,
        TextRole::SummaryCard,
        TextRole::ButtonPrimary,
        TextRole::ButtonExit,
        TextRole::ButtonQuit,
        TextRole::PillLabel,
        TextRole::CodeGlyph,
        TextRole::QrCaption,
        TextRole::NameEntry,
        TextRole::NameKey,
        TextRole::NamePrompt,
        TextRole::NameSkip,
        TextRole::NameDel,
        TextRole::NameDone,
        TextRole::Callout,
        TextRole::TouchDash,
        TextRole::TouchTaunt,
        TextRole::TouchHint,
        TextRole::Notice,
        TextRole::ListRow,
        TextRole::ListDetail,
        TextRole::ListFine,
        TextRole::Marquee,
        TextRole::RosterName,
        TextRole::RosterRule,
        TextRole::KeyLegend,
        TextRole::ShareLabel,
        TextRole::ShareUrl,
        TextRole::ShareDismiss,
        TextRole::Debug,
        TextRole::GroupHeader,
        TextRole::SettingLabel,
        TextRole::SettingValue,
        TextRole::SettingsHowTo,
        TextRole::Summoning,
        TextRole::CrashNotice,
        TextRole::IntroTitle,
        TextRole::IntroSub,
        TextRole::RivalsEmpty,
        TextRole::RivalRowName,
        TextRole::RivalRowDetail,
        TextRole::RivalRowSub,
        TextRole::RivalName,
        TextRole::RivalStanding,
        TextRole::RivalFact,
        TextRole::RivalShade,
        TextRole::RivalNoTapes,
        TextRole::RivalTape,
    ];

    /// The size this role is drawn at with no tuning override.
    pub const fn default_size(self) -> f32 {
        match self {
            TextRole::ScreenTitle => 84.0,
            TextRole::ScreenSubtitle => 32.0,
            TextRole::PageTitle => 72.0,
            TextRole::Back => 40.0,
            TextRole::Wordmark => 110.0,
            TextRole::TitleName => 44.0,
            TextRole::TitleOverlay => 34.0,
            TextRole::TitlePlay => 48.0,
            TextRole::TitleMenu => 22.0,
            TextRole::SummaryCard => 56.0,
            TextRole::ButtonPrimary => 44.0,
            TextRole::ButtonExit => 30.0,
            TextRole::ButtonQuit => 24.0,
            TextRole::PillLabel => 28.0,
            TextRole::CodeGlyph => 54.0,
            TextRole::QrCaption => 26.0,
            TextRole::NameEntry => 64.0,
            TextRole::NameKey => 46.0,
            TextRole::NamePrompt => 34.0,
            TextRole::NameSkip => 30.0,
            TextRole::NameDel => 34.0,
            TextRole::NameDone => 40.0,
            TextRole::Callout => 40.0,
            TextRole::TouchDash => 40.0,
            TextRole::TouchTaunt => 24.0,
            TextRole::TouchHint => 26.0,
            TextRole::Notice => 30.0,
            TextRole::ListRow => 34.0,
            TextRole::ListDetail => 24.0,
            TextRole::ListFine => 18.0,
            TextRole::Marquee => 34.0,
            TextRole::RosterName => 44.0,
            TextRole::RosterRule => 26.0,
            TextRole::KeyLegend => 26.0,
            TextRole::ShareLabel => 30.0,
            TextRole::ShareUrl => 22.0,
            TextRole::ShareDismiss => 20.0,
            TextRole::Debug => 13.0,
            TextRole::GroupHeader => 24.0,
            TextRole::SettingLabel => 40.0,
            TextRole::SettingValue => 46.0,
            TextRole::SettingsHowTo => 30.0,
            TextRole::Summoning => 54.0,
            TextRole::CrashNotice => 26.0,
            TextRole::IntroTitle => 56.0,
            TextRole::IntroSub => 32.0,
            TextRole::RivalsEmpty => 34.0,
            TextRole::RivalRowName => 40.0,
            TextRole::RivalRowDetail => 24.0,
            TextRole::RivalRowSub => 34.0,
            TextRole::RivalName => 64.0,
            TextRole::RivalStanding => 36.0,
            TextRole::RivalFact => 28.0,
            TextRole::RivalShade => 32.0,
            TextRole::RivalNoTapes => 26.0,
            TextRole::RivalTape => 28.0,
        }
    }

    /// The role's name, as `Tuning.text` keys it (`"ScreenTitle"`).
    pub fn name(self) -> String {
        format!("{self:?}")
    }

    /// The size under `tuning`: its override for this role, else the default.
    pub fn size(self, tuning: &render::tuning::Tuning) -> f32 {
        tuning
            .text
            .get(&self.name())
            .copied()
            .unwrap_or(self.default_size())
    }
}

/// Keys of `Tuning.text` that name no [`TextRole`] (a typo in a tuning
/// file would otherwise change nothing, silently). Pure.
pub fn unknown_text_roles(tuning: &render::tuning::Tuning) -> Vec<String> {
    let known: Vec<String> = TextRole::ALL.iter().map(|r| r.name()).collect();
    tuning
        .text
        .keys()
        .filter(|k| !known.contains(k))
        .cloned()
        .collect()
}

/// `Update`: size text by its role: on spawn, and for everything on screen
/// whenever `Tuning` changes (a live reload resizes text already shown).
pub fn apply_text_roles(
    tuning: Option<Res<render::tuning::Tuning>>,
    mut q: Query<(Ref<TextRole>, &mut TextFont)>,
) {
    let Some(tuning) = tuning else {
        return;
    };
    let all = tuning.is_changed();
    if all {
        for key in unknown_text_roles(&tuning) {
            tracing::warn!(target: "two_top::tuning", key, "tuning names a text role that does not exist");
        }
    }
    for (role, mut font) in &mut q {
        if !(all || role.is_added()) {
            continue;
        }
        let size = role.size(&tuning);
        if font.font_size != size {
            font.font_size = size;
        }
    }
}

#[cfg(test)]
mod text_role_tests {
    use super::*;

    #[test]
    fn every_role_is_listed_once_with_a_size() {
        let mut names = std::collections::BTreeSet::new();
        for role in TextRole::ALL {
            assert!(role.default_size() > 0.0, "{role:?}");
            assert!(names.insert(role.name()), "{role:?} twice");
        }
    }

    #[test]
    fn a_tuning_override_resizes_one_role_only() {
        let tuning =
            render::tuning::Tuning::from_overrides(r#"{ "text": { "ScreenTitle": 90.0 } }"#)
                .unwrap();
        assert_eq!(TextRole::ScreenTitle.size(&tuning), 90.0);
        assert_eq!(TextRole::Back.size(&tuning), TextRole::Back.default_size());
        assert!(unknown_text_roles(&tuning).is_empty());
        let typo = render::tuning::Tuning::from_overrides(r#"{ "text": { "ScreenTitel": 90.0 } }"#)
            .unwrap();
        assert_eq!(unknown_text_roles(&typo), ["ScreenTitel".to_string()]);
    }
}

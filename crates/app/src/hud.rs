//! Minimal in-match HUD (DESIGN_DIRECTION § 2).
//!
//! The v2 design panel's unanimous call: a best-of-5, 30s, one-hit duel must
//! show its win condition (score) and pace-maker (round clock), but as the
//! smallest possible *pixel-art* overlay — no vector font, no chrome, nothing
//! the body already says (one-hit kills → no health bar). Three elements only:
//!
//!   * **Score pips** — five per player, top corners (P0/Cur left, P1/Stag
//!     right), filled by `MatchScore`. The match-point pip pulses.
//!   * **Round clock** — a thin depleting bar, Cold Stone draining to Ember in
//!     the final 5 s (the wordless "hurry" cue).
//!   * **Countdown** — the big 3·2·1 glyphs at round start, a brief GO on the
//!     first round frame.
//!
//! Render-only: reads sim resources, never writes them. Every element is
//! pinned to the *screen* via [`ScreenAnchor`], so the HUD hugs the real
//! display edges on any phone aspect and stays rock-stable under the
//! kill-cam zoom / follow cam / screen shake.

use bevy::prelude::*;
use sim::{FrameCount, MATCH_WIN_THRESHOLD, MatchScore, MatchState, ROUND_DURATION_FRAMES};

use crate::anchor::ScreenAnchor;
use crate::screen::{AppScreen, AwaitingPeer};

const PIP_SIZE: f32 = 42.0;
const PIP_GAP: f32 = 10.0;
const HUD_MARGIN: f32 = 70.0;
const TIMER_WIDTH: f32 = 520.0;
const TIMER_HEIGHT: f32 = 14.0;
const COUNTDOWN_SIZE: f32 = 220.0;
/// Frames a "GO" glyph flashes at the top of a fresh round.
const GO_FLASH_FRAMES: u32 = 24;

#[derive(Component)]
struct ScorePip {
    player: u8,
    idx: u8,
}

#[derive(Component)]
struct CountdownGlyph;

#[derive(Component)]
struct TimerBar;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud).add_systems(
            Update,
            (update_score_pips, update_countdown, update_timer_bar),
        );
    }
}

fn spawn_hud(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
) {
    // Score pips — 3-cell 8x8 atlas: [empty, filled-P0, filled-P1].
    let pip_layout = atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(8),
        3,
        1,
        None,
        None,
    ));
    let pip_img = asset_server.load("hud/score_pips.png");
    for player in 0..2u8 {
        for idx in 0..MATCH_WIN_THRESHOLD {
            let step = (PIP_SIZE + PIP_GAP) * idx as f32;
            // P0 fills rightward from the screen's left edge, P1 leftward
            // from the right — true display corners on every aspect.
            let anchor = if player == 0 {
                ScreenAnchor::new(-1.0, 1.0, HUD_MARGIN + step, -HUD_MARGIN)
            } else {
                ScreenAnchor::new(1.0, 1.0, -(HUD_MARGIN + step), -HUD_MARGIN)
            };
            commands.spawn((
                ScorePip { player, idx },
                Sprite {
                    image: pip_img.clone(),
                    texture_atlas: Some(TextureAtlas {
                        layout: pip_layout.clone(),
                        index: 0,
                    }),
                    custom_size: Some(Vec2::splat(PIP_SIZE)),
                    ..default()
                },
                anchor,
                Transform::from_xyz(0.0, 0.0, 50.0),
                Visibility::Hidden,
            ));
        }
    }

    // Round clock — a thin depleting bar, top-center under the pip row.
    commands.spawn((
        TimerBar,
        Sprite {
            color: render::palette::COLD_STONE,
            custom_size: Some(Vec2::new(TIMER_WIDTH, TIMER_HEIGHT)),
            ..default()
        },
        ScreenAnchor::new(0.0, 1.0, 0.0, -(HUD_MARGIN + PIP_SIZE)),
        Transform::from_xyz(0.0, 0.0, 50.0),
        Visibility::Hidden,
    ));

    // Countdown glyphs — 5-cell 16x16 atlas: [3, 2, 1, G, O].
    let cd_layout = atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(16),
        5,
        1,
        None,
        None,
    ));
    commands.spawn((
        CountdownGlyph,
        Sprite {
            image: asset_server.load("hud/countdown_digits.png"),
            texture_atlas: Some(TextureAtlas {
                layout: cd_layout,
                index: 0,
            }),
            custom_size: Some(Vec2::splat(COUNTDOWN_SIZE)),
            ..default()
        },
        ScreenAnchor::new(0.0, 0.0, 0.0, 40.0),
        Transform::from_xyz(0.0, 40.0, 150.0),
        Visibility::Hidden,
    ));
}

fn update_score_pips(
    screen: Res<State<AppScreen>>,
    awaiting: Res<AwaitingPeer>,
    score: Res<MatchScore>,
    time: Res<Time<Real>>,
    mut q: Query<(&ScorePip, &mut Sprite, &mut Visibility)>,
) {
    // Pips are premature while sitting alone in an online room.
    let in_match = *screen.get() == AppScreen::InMatch && !awaiting.0;
    // Smooth pulse for the match-point pip (render-only, non-rollback clock).
    let pulse = 0.4 + 0.6 * (time.elapsed_secs() * 6.0).sin().mul_add(0.5, 0.5);
    for (pip, mut sprite, mut vis) in &mut q {
        if !in_match {
            *vis = Visibility::Hidden;
            continue;
        }
        *vis = Visibility::Visible;
        let won = if pip.player == 0 { score.p0 } else { score.p1 };
        let filled = pip.idx < won;
        // Match point = the player needs exactly one more; its next pip previews
        // filled and pulses (stakes felt, not read).
        let is_next = pip.idx == won;
        let at_match_point = won + 1 == MATCH_WIN_THRESHOLD;
        let team_cell = if pip.player == 0 { 1 } else { 2 };
        if let Some(atlas) = sprite.texture_atlas.as_mut() {
            atlas.index = if filled || (at_match_point && is_next) {
                team_cell
            } else {
                0
            };
        }
        sprite.color = if at_match_point && is_next && !filled {
            Color::WHITE.with_alpha(pulse)
        } else {
            Color::WHITE
        };
    }
}

fn update_countdown(
    screen: Res<State<AppScreen>>,
    awaiting: Res<AwaitingPeer>,
    state: Res<MatchState>,
    frame: Res<FrameCount>,
    mut q: Query<(&mut Sprite, &mut Visibility), With<CountdownGlyph>>,
) {
    let Ok((mut sprite, mut vis)) = q.single_mut() else {
        return;
    };
    // The idle session parks MatchState at Countdown{3}; don't freeze a "3"
    // over the empty room while waiting for the challenger to arrive.
    if awaiting.0 {
        *vis = Visibility::Hidden;
        return;
    }
    let show = |index: usize, sprite: &mut Sprite, vis: &mut Visibility| {
        if let Some(atlas) = sprite.texture_atlas.as_mut() {
            atlas.index = index;
        }
        *vis = Visibility::Visible;
    };
    match (*screen.get(), *state) {
        (AppScreen::InMatch, MatchState::Countdown { digit, .. }) => {
            // digit 3/2/1 → cells 0/1/2.
            show(3usize.saturating_sub(digit as usize), &mut sprite, &mut vis);
        }
        (AppScreen::InMatch, MatchState::InRound { expires_at_frame }) => {
            // Brief "GO" (cell 3) on the first frames of the round.
            let started = expires_at_frame.saturating_sub(ROUND_DURATION_FRAMES);
            if frame.0 < started + GO_FLASH_FRAMES {
                show(3, &mut sprite, &mut vis);
            } else {
                *vis = Visibility::Hidden;
            }
        }
        _ => *vis = Visibility::Hidden,
    }
}

fn update_timer_bar(
    screen: Res<State<AppScreen>>,
    state: Res<MatchState>,
    frame: Res<FrameCount>,
    mut q: Query<(&mut Sprite, &mut Visibility), With<TimerBar>>,
) {
    let Ok((mut sprite, mut vis)) = q.single_mut() else {
        return;
    };
    if let (AppScreen::InMatch, MatchState::InRound { expires_at_frame }) = (*screen.get(), *state)
    {
        *vis = Visibility::Visible;
        let remaining = expires_at_frame
            .saturating_sub(frame.0)
            .min(ROUND_DURATION_FRAMES);
        let frac = remaining as f32 / ROUND_DURATION_FRAMES as f32;
        sprite.custom_size = Some(Vec2::new((TIMER_WIDTH * frac).max(1.0), TIMER_HEIGHT));
        // Drain to Ember in the final 5 s — the only animation it ever does.
        sprite.color = if remaining <= 5 * 60 {
            render::palette::EMBER
        } else {
            render::palette::COLD_STONE
        };
    } else {
        *vis = Visibility::Hidden;
    }
}

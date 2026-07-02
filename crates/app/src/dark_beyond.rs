//! The dark beyond — demon eyes watching from the void outside the island.
//!
//! Task-1's overscan fairness rule made everything past the arena cosmetic
//! darkness; this fills it with menace. Dozens of dim ember eye-pairs sit
//! scattered in the blackness (visible mostly on tall/wide screens where
//! the view extends past the island), blinking on independent clocks,
//! tracking the flying fang, and flaring all at once on a kill.
//!
//! Entirely render-side: positions come from [`render::CosmeticRng`] (the
//! non-rollback cosmetic stream — never `SimRng`), animation runs on
//! `Time<Real>`, and nothing here reads back into sim. Kills are observed
//! by watching `MatchScore` grow, which is rollback-stable by the time
//! `Update` sees it.

use bevy::prelude::*;
use rand::Rng as _;
use render::CosmeticRng;
use sim::{Boomerang, BoomerangState, MatchScore, PositionF};

/// How many eye-pairs haunt the void.
const EYE_PAIRS: usize = 26;
/// Gap between the two dots of a pair (world units).
const EYE_GAP: f32 = 8.0;
/// Dot size (world units — chunky pixels, not points).
const EYE_W: f32 = 5.0;
const EYE_H: f32 = 3.5;
/// How far a pair's gaze shifts toward the flying fang.
const GAZE_SHIFT: f32 = 3.0;
/// Resting glow alpha range (breathes between blinks).
const ALPHA_DIM: f32 = 0.10;
const ALPHA_OPEN: f32 = 0.34;

/// One dot of an eye-pair. The pair shares `base`/`phase`/`speed`; `side`
/// places the left/right dot.
#[derive(Component)]
struct VoidEye {
    base: Vec2,
    phase: f32,
    speed: f32,
    side: f32,
}

/// Seconds of all-eyes flare remaining after a kill.
#[derive(Resource, Default)]
struct EyeFlare(f32);

pub struct DarkBeyondPlugin;

impl Plugin for DarkBeyondPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EyeFlare>()
            .add_systems(Startup, spawn_eyes)
            .add_systems(Update, animate_eyes);
    }
}

fn spawn_eyes(mut commands: Commands, mut rng: ResMut<CosmeticRng>) {
    // The island (foreshortened) spans ~±500 × ±562; keep every pair clear
    // of it plus a margin so eyes never read as on-stage actors. Scatter
    // out to the largest plausible overscan on a tall phone or wide desktop.
    for _ in 0..EYE_PAIRS {
        let (x, y) = loop {
            let x = rng.0.gen_range(-1500.0..1500.0f32);
            let y = rng.0.gen_range(-1350.0..1350.0f32);
            if x.abs() > 640.0 || y.abs() > 720.0 {
                break (x, y);
            }
        };
        let base = Vec2::new(x, y);
        let phase = rng.0.gen_range(0.0..std::f32::consts::TAU);
        let speed = rng.0.gen_range(0.25..0.8f32);
        for side in [-1.0f32, 1.0] {
            commands.spawn((
                VoidEye {
                    base,
                    phase,
                    speed,
                    side,
                },
                Sprite {
                    color: render::palette::EMBER.with_alpha(0.0),
                    custom_size: Some(Vec2::new(EYE_W, EYE_H)),
                    ..default()
                },
                // z −0.9: above the clear-color void, below the island floor
                // (−1 is under it, but the eyes sit outside its footprint)
                // and everything on stage — background creatures, not actors.
                Transform::from_xyz(base.x + side * EYE_GAP * 0.5, base.y, -0.9),
            ));
        }
    }
}

fn animate_eyes(
    time: Res<Time<Real>>,
    score: Res<MatchScore>,
    mut flare: ResMut<EyeFlare>,
    mut last_score: Local<Option<MatchScore>>,
    fangs: Query<(&PositionF, &Boomerang)>,
    mut eyes: Query<(&VoidEye, &mut Sprite, &mut Transform)>,
) {
    // A kill = the total went UP (a rematch reset going to 0-0 must not flare).
    let prev = last_score.unwrap_or(*score);
    if score.p0 + score.p1 > prev.p0 + prev.p1 {
        flare.0 = 1.0;
    }
    *last_score = Some(*score);
    flare.0 = (flare.0 - time.delta_secs() * 1.3).max(0.0);

    // The thing the void watches: the first flying fang, if any.
    let fang_pos = fangs
        .iter()
        .find(|(_, b)| matches!(b.state, BoomerangState::Flying))
        .map(|(p, _)| {
            let (x, y) = p.0.to_f32();
            Vec2::new(x, render::tilt_y(y))
        });

    let t = time.elapsed_secs();
    for (eye, mut sprite, mut tx) in &mut eyes {
        // Slow independent blink: mostly open, periodically squeezed shut.
        let wave = (t * eye.speed + eye.phase).sin();
        let open = ((wave + 0.9) / 1.6).clamp(0.0, 1.0);
        // Gaze: the whole pair leans a few pixels toward the fang.
        let gaze = fang_pos
            .map(|f| (f - eye.base).normalize_or_zero() * GAZE_SHIFT)
            .unwrap_or(Vec2::ZERO);
        tx.translation.x = eye.base.x + eye.side * EYE_GAP * 0.5 + gaze.x;
        tx.translation.y = eye.base.y + gaze.y;

        let alpha = ALPHA_DIM + (ALPHA_OPEN - ALPHA_DIM) * open;
        sprite.color = if flare.0 > 0.0 {
            // Every eye in the dark flares at once on a kill (HDR overdrive
            // pushes them into the bloom threshold), then breathes back down.
            render::scale_color(render::palette::EMBER, 1.0 + flare.0 * 1.6)
                .with_alpha((alpha + 0.5 * flare.0).min(1.0))
        } else {
            render::palette::EMBER.with_alpha(alpha)
        };
    }
}

//! Phase 7: simulation/render boundary.
//!
//! The sim writes `PositionF` (and friends) deterministically at 60 Hz.
//! The render layer's job is to produce a `Transform` for each rendered
//! entity at whatever rate the display refreshes — 30, 60, 120, 144 Hz
//! and everything in between. We do that by interpolating between the
//! last two simulated positions: at the start of each tick `sim` copies
//! `PositionF` into `PreviousPositionF` (`snapshot_previous`), and the
//! render system below lerps between them weighted by how far into the
//! current sim tick we are.
//!
//! The lerp is f32-based — we are over the sim/render boundary and floats
//! are explicitly allowed here. `Vec2F::to_f32` is the sanctioned bridge;
//! see CONVENTIONS § Render Layer Rules.

use bevy::prelude::*;
use fixed_math::Vec2F;
use sim::{LastSimTickTime, NoInterpolate, PositionF, PreviousPositionF, TICK_HZ};

// ===========================================================================
// Phase 15 cycle 3c — locked 16-color palette.
// Single source of truth for every render-side Color::srgb call across the
// workspace. Mirrors `assets/palettes/two_top_16.gpl` exactly. Future code
// references colors by name (`palette::HOT_BONE`) rather than reconstructing
// raw RGB triples that drift from the canonical palette.
// ===========================================================================

pub mod palette {
    use bevy::prelude::Color;

    const fn p(r: u8, g: u8, b: u8) -> Color {
        Color::srgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
    }

    pub const VOID: Color = p(11, 13, 18);
    pub const DEEP_ASH: Color = p(23, 25, 34);
    pub const BRUISE_SHADOW: Color = p(43, 37, 51);
    pub const CHARCOAL_LINE: Color = p(57, 52, 66);
    pub const COLD_STONE: Color = p(87, 90, 100);
    pub const WARM_BONE_SHADE: Color = p(122, 101, 88);
    pub const BONE: Color = p(203, 190, 148);
    pub const HOT_BONE: Color = p(255, 241, 194);
    pub const BLOOD_DARK: Color = p(110, 22, 50);
    pub const P0_BLOOD: Color = p(210, 47, 69);
    pub const EMBER: Color = p(240, 106, 58);
    pub const SPARK: Color = p(255, 216, 102);
    pub const DEEP_TEAL: Color = p(13, 101, 114);
    pub const P1_CYAN: Color = p(39, 199, 216);
    pub const RECALL_BLUE: Color = p(71, 108, 255);
    pub const HIT_WHITE: Color = p(248, 247, 232);
}

/// Lerp between two sim positions, returning a `Vec2` ready to drop into
/// `Transform.translation`. `alpha` is clamped to `[0.0, 1.0]`.
pub fn lerp_position(prev: Vec2F, curr: Vec2F, alpha: f32) -> Vec2 {
    let alpha = alpha.clamp(0.0, 1.0);
    let (px, py) = prev.to_f32();
    let (cx, cy) = curr.to_f32();
    Vec2::new(px + (cx - px) * alpha, py + (cy - py) * alpha)
}

/// Compute the interpolation alpha for the current frame. `now` is the
/// real-time clock's elapsed seconds; `last_tick` is when the most recent
/// sim tick finished. The result is `0.0` immediately after a sim tick
/// and approaches `1.0` as the next tick draws near.
pub fn interpolation_alpha(now_secs: f32, last_tick_secs: f32, tick_hz: usize) -> f32 {
    let tick_dt = 1.0 / tick_hz as f32;
    ((now_secs - last_tick_secs) / tick_dt).clamp(0.0, 1.0)
}

/// `Update`-schedule system: writes `Transform.translation.{x,y}` for any
/// entity that has both a sim position pair and a `Transform`. Entities
/// carrying `NoInterpolate` get the raw current position (no lerp); all
/// others lerp between previous and current sim positions weighted by
/// `interpolation_alpha`.
pub fn sync_transforms_from_sim(
    time: Res<Time<Real>>,
    last_tick: Res<LastSimTickTime>,
    mut q: Query<(
        &PositionF,
        &PreviousPositionF,
        Option<&NoInterpolate>,
        &mut Transform,
    )>,
) {
    let alpha = interpolation_alpha(
        time.elapsed_secs_f64() as f32,
        last_tick.0 as f32,
        TICK_HZ,
    );
    for (pos, prev, no_interp, mut xform) in &mut q {
        let v = if no_interp.is_some() {
            let (x, y) = pos.0.to_f32();
            Vec2::new(x, y)
        } else {
            lerp_position(prev.0, pos.0, alpha)
        };
        xform.translation.x = v.x;
        xform.translation.y = v.y;
    }
}

pub struct RenderSyncPlugin;

impl Plugin for RenderSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, sync_transforms_from_sim);
    }
}

// =========================================================================
// Phase 15 cycle 2 — effect sprites + ambient embers.
//
// The render layer reads sim component transitions (Dead, BoomerangState)
// in `Update` (post-rollback) and spawns short-lived effect sprites at the
// position of the event. Effect sprites are NOT rolled back — they're
// purely cosmetic and use Time<Real>. The cosmetic RNG (ambient ember
// scatter) is also Real-time + per-process, never SimRng (CONVENTIONS).
// =========================================================================

use bevy::sprite::Anchor;
use rand::Rng;
use rand::SeedableRng as _;
use rand::rngs::SmallRng;
use sim::{Boomerang, BoomerangState, Dead, MatchState, Player};

/// One-shot animated sprite spawned by [`spawn_effect_sprite`]. Lives
/// in render-time (not rolled back) — purely cosmetic, despawned by
/// [`advance_effect_sprites`] when its frame counter reaches
/// `frames`. Each frame is held for `seconds_per_frame` real seconds.
#[derive(Component, Clone, Copy, Debug)]
pub struct EffectSprite {
    pub frames: u16,
    pub seconds_per_frame: f32,
    /// Real-time accumulator since the last frame swap. When this
    /// crosses `seconds_per_frame`, the sprite advances and the
    /// accumulator wraps.
    pub elapsed: f32,
    /// Current frame index. Despawn fires when this hits `frames`.
    pub current: u16,
}

impl EffectSprite {
    pub fn new(frames: u16, seconds_per_frame: f32) -> Self {
        Self {
            frames,
            seconds_per_frame,
            elapsed: 0.0,
            current: 0,
        }
    }
}

/// Render-side advance for [`EffectSprite`]. Walks every effect sprite,
/// advances its frame counter against `Time<Real>`, syncs the
/// TextureAtlas index, and despawns when the animation finishes. Per
/// CONVENTIONS § Animation: pixel-art frames snap (no fractional
/// blending between source frames).
pub fn advance_effect_sprites(
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut q: Query<(Entity, &mut EffectSprite, &mut Sprite)>,
) {
    let dt = time.delta_secs();
    for (entity, mut effect, mut sprite) in &mut q {
        effect.elapsed += dt;
        while effect.elapsed >= effect.seconds_per_frame {
            effect.elapsed -= effect.seconds_per_frame;
            effect.current = effect.current.saturating_add(1);
        }
        if effect.current >= effect.frames {
            commands.entity(entity).despawn();
            continue;
        }
        if let Some(atlas) = sprite.texture_atlas.as_mut() {
            atlas.index = effect.current as usize;
        }
    }
}

/// Spawn a one-shot effect sprite at `world_pos`. The sheet must
/// already be loaded (typically via the [`EffectAssets`] resource
/// prepared by [`EffectsPlugin`]).
#[allow(clippy::too_many_arguments)]
pub fn spawn_effect(
    commands: &mut Commands,
    image: Handle<Image>,
    layout: Handle<TextureAtlasLayout>,
    frames: u16,
    seconds_per_frame: f32,
    world_pos: Vec2,
    pixel_size: f32,
    z_layer: f32,
) {
    commands.spawn((
        Sprite {
            image,
            texture_atlas: Some(TextureAtlas { layout, index: 0 }),
            custom_size: Some(Vec2::splat(pixel_size)),
            ..default()
        },
        Anchor::CENTER,
        Transform::from_xyz(world_pos.x, world_pos.y, z_layer),
        EffectSprite::new(frames, seconds_per_frame),
    ));
}

/// Pre-loaded sprite-sheet handles + atlas layouts for the four
/// Phase 15 effect sprites + per-side floor-stain sheets. Loaded once
/// in `EffectsPlugin::startup` so the per-event spawners don't pay an
/// asset-server hit each time.
#[derive(Resource, Clone)]
pub struct EffectAssets {
    pub hit_burst: (Handle<Image>, Handle<TextureAtlasLayout>),
    pub death_burst: (Handle<Image>, Handle<TextureAtlasLayout>),
    pub recall_pulse: (Handle<Image>, Handle<TextureAtlasLayout>),
    pub ambient_ember: (Handle<Image>, Handle<TextureAtlasLayout>),
    /// 4-cell stain sheets per side (small / medium / heavy / corpse-mark).
    /// Cycle 3a uses cell 3 (corpse-mark) at the death position; later
    /// cycles can vary the cell selection for visual variety.
    pub p0_stain: (Handle<Image>, Handle<TextureAtlasLayout>),
    pub p1_stain: (Handle<Image>, Handle<TextureAtlasLayout>),
}

fn load_effect_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
) {
    // Sheet dimensions documented in assets/README.md:
    //   hit_burst:     4 frames @ 24x24
    //   death_burst:   6 frames @ 24x24
    //   recall_pulse:  4 frames @ 16x16
    //   ambient_ember: 4 frames @ 8x8
    let hit_layout = atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(24),
        4,
        1,
        None,
        None,
    ));
    let death_layout = atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(24),
        6,
        1,
        None,
        None,
    ));
    let recall_layout = atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(16),
        4,
        1,
        None,
        None,
    ));
    let ember_layout = atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(8),
        4,
        1,
        None,
        None,
    ));
    // Stain sheets: 4 cells x 16x16 each. Same layout for both sides.
    let stain_layout = atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(16),
        4,
        1,
        None,
        None,
    ));
    commands.insert_resource(EffectAssets {
        hit_burst: (
            asset_server.load("sprites/particles/hit_burst_sheet.png"),
            hit_layout,
        ),
        death_burst: (
            asset_server.load("sprites/particles/death_burst_sheet.png"),
            death_layout,
        ),
        recall_pulse: (
            asset_server.load("sprites/particles/recall_pulse_sheet.png"),
            recall_layout,
        ),
        ambient_ember: (
            asset_server.load("sprites/particles/ambient_ember_sheet.png"),
            ember_layout,
        ),
        p0_stain: (
            asset_server.load("sprites/stains/p0_stain_sheet.png"),
            stain_layout.clone(),
        ),
        p1_stain: (
            asset_server.load("sprites/stains/p1_stain_sheet.png"),
            stain_layout,
        ),
    });
}

/// Tracks each Player handle's last-observed `Dead.is_dying()` so
/// [`spawn_hit_and_death_bursts`] can fire effect sprites only on the
/// rising edge — the tick the player transitions from alive to dying.
/// Local to that system to keep the visual-state cache out of the
/// global resource graph.
#[derive(Default)]
pub struct PrevDying(pub bevy::platform::collections::HashMap<usize, bool>);

/// Render-side detector: spawns hit_burst + death_burst + persistent
/// FloorStain at each player's position the moment they become
/// `is_dying`. The two short bursts are layered at slightly different
/// Z so the hit flash reads on top of the longer death burst; the
/// stain commits as a permanent corpse-mark cell that persists till
/// the round resets.
pub fn spawn_hit_and_death_bursts(
    mut commands: Commands,
    assets: Res<EffectAssets>,
    players: Query<(&Player, &Dead, &Transform)>,
    mut prev: Local<PrevDying>,
) {
    for (player, dead, xform) in &players {
        let now_dying = dead.is_dying();
        let was_dying = prev.0.get(&player.handle).copied().unwrap_or(false);
        if now_dying && !was_dying {
            let pos = xform.translation.truncate();
            // Hit burst — quick 4-frame flash, ~70 ms total. Above
            // gameplay z so it pops over the player sprite that just
            // got hit.
            spawn_effect(
                &mut commands,
                assets.hit_burst.0.clone(),
                assets.hit_burst.1.clone(),
                4,
                0.018,
                pos,
                64.0,
                10.0,
            );
            // Death burst — 6 frames, ~150 ms total, slightly larger
            // footprint. Ends with the corpse-mark commit per
            // VISUAL_TARGET_PACK.md.
            spawn_effect(
                &mut commands,
                assets.death_burst.0.clone(),
                assets.death_burst.1.clone(),
                6,
                0.025,
                pos,
                80.0,
                9.5,
            );
            // Persistent floor stain — the synthesis primitive from
            // VISUAL_TARGET_PACK.md. Cell 3 is the corpse-mark, the
            // continuation of the death burst's final frame. Sized
            // 32 px so it reads as a real footprint without dominating
            // the arena.
            let (stain_image, stain_layout) = if player.handle == 0 {
                assets.p0_stain.clone()
            } else {
                assets.p1_stain.clone()
            };
            commands.spawn((
                Sprite {
                    image: stain_image,
                    texture_atlas: Some(TextureAtlas {
                        layout: stain_layout,
                        index: 3,
                    }),
                    custom_size: Some(Vec2::splat(32.0)),
                    ..default()
                },
                Anchor::CENTER,
                // Z below players (0.0) but above the floor (-1.0) so
                // players step over the stains visually.
                Transform::from_xyz(pos.x, pos.y, -0.5),
                FloorStain {
                    owner_handle: player.handle,
                },
            ));
        }
        prev.0.insert(player.handle, now_dying);
    }
}

/// Persistent floor stain — the Bone-Cathedral synthesis primitive
/// (VISUAL_TARGET_PACK.md). Spawned at every kill position; cleared
/// only when the next round starts (`MatchState` enters `Countdown`
/// from a non-Countdown predecessor). The arena remembers each
/// round's violence; this is the gore-revival pole's resting state
/// in composition mode.
#[derive(Component, Clone, Copy, Debug)]
pub struct FloorStain {
    /// Which player produced the stain. Used for cosmetic
    /// distinguishability (P0 = blood-dark, P1 = deep-teal); the sim
    /// itself never reads this — stains are render-only.
    pub owner_handle: usize,
}

/// Render-side detector: clears every [`FloorStain`] when a NEW
/// MATCH starts (transition out of `MatchOver`). Stains accumulate
/// across every round of a match — by round 5 of a BO5 the arena
/// is a wreckage of the entire match's violence, readable as a
/// visual scoreboard. Round transitions DO NOT clear stains.
///
/// This is the synthesis primitive (VISUAL_TARGET_PACK.md, "the
/// arena remembers each round's violence") cranked to match-scope —
/// the most aesthetically aligned scope for the gore-revival pole.
///
/// Currently `MatchOver` is terminal in sim, so this trigger never
/// fires within a single app session — stains effectively persist
/// for the lifetime of the process. The system is wired up in
/// advance for the eventual Phase 18 "play again" flow, where the
/// MatchOver → Countdown transition becomes a real signal.
pub fn clear_stains_on_match_reset(
    state: Res<MatchState>,
    mut prev: Local<Option<MatchState>>,
    mut commands: Commands,
    stains: Query<Entity, With<FloorStain>>,
) {
    let was_match_over = matches!(*prev, Some(MatchState::MatchOver));
    let now_post_match_over = !matches!(*state, MatchState::MatchOver);
    if was_match_over && now_post_match_over {
        for entity in &stains {
            commands.entity(entity).despawn();
        }
    }
    *prev = Some(*state);
}

/// Tracks each Boomerang owner's last-observed `BoomerangState`.
/// Same per-handle-edge pattern as [`PrevDying`].
#[derive(Default)]
pub struct PrevBoomerangState(pub bevy::platform::collections::HashMap<usize, BoomerangState>);

/// Render-side detector: spawns recall_pulse at the boomerang's
/// position the tick its state transitions from `Flying` to
/// `Returning`. The pulse reads as the recall-energy emanation.
pub fn spawn_recall_pulses(
    mut commands: Commands,
    assets: Res<EffectAssets>,
    boomerangs: Query<(&Boomerang, &Transform)>,
    mut prev: Local<PrevBoomerangState>,
) {
    for (boom, xform) in &boomerangs {
        let curr = boom.state;
        let was = prev
            .0
            .get(&boom.owner_handle)
            .copied()
            .unwrap_or(BoomerangState::Flying);
        if matches!(was, BoomerangState::Flying) && matches!(curr, BoomerangState::Returning) {
            spawn_effect(
                &mut commands,
                assets.recall_pulse.0.clone(),
                assets.recall_pulse.1.clone(),
                4,
                0.040,
                xform.translation.truncate(),
                48.0,
                8.0,
            );
        }
        prev.0.insert(boom.owner_handle, curr);
    }
}

/// Cosmetic RNG seeded once at startup. CONVENTIONS § Render Layer
/// Rules forbids using `SimRng` here — visual jitter must never feed
/// back into the rolled-back state.
#[derive(Resource)]
pub struct CosmeticRng(pub SmallRng);

/// Drives the ambient-ember spawner: roughly every
/// `1.0 / EMBER_RATE_HZ` real seconds, spawn an ember sprite at a
/// random arena-interior position. The arena is 1500x1000 cm
/// (per sim::ARENA_HALF_WIDTH/HEIGHT) so we sample a centered range
/// with a 100 cm padding so embers don't clip into the wall sprites.
#[derive(Resource)]
pub struct EmberAccumulator {
    pub elapsed: f32,
}

const EMBER_RATE_HZ: f32 = 4.0;
const EMBER_ARENA_HALF_W: f32 = 650.0;
const EMBER_ARENA_HALF_H: f32 = 400.0;

pub fn spawn_ambient_embers(
    time: Res<Time<Real>>,
    mut commands: Commands,
    assets: Res<EffectAssets>,
    mut rng: ResMut<CosmeticRng>,
    mut acc: ResMut<EmberAccumulator>,
) {
    acc.elapsed += time.delta_secs();
    let interval = 1.0 / EMBER_RATE_HZ;
    while acc.elapsed >= interval {
        acc.elapsed -= interval;
        let x = rng.0.gen_range(-EMBER_ARENA_HALF_W..=EMBER_ARENA_HALF_W);
        let y = rng.0.gen_range(-EMBER_ARENA_HALF_H..=EMBER_ARENA_HALF_H);
        spawn_effect(
            &mut commands,
            assets.ambient_ember.0.clone(),
            assets.ambient_ember.1.clone(),
            4,
            0.080,
            Vec2::new(x, y),
            16.0,
            -0.5,
        );
    }
}

/// Plugin: registers the effect-sprite infrastructure plus all four
/// Phase 15 cycle 2 spawners. Add alongside [`RenderSyncPlugin`] in
/// any binary that wants to render the polished effects (the live
/// app + the replay viewer).
pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CosmeticRng(SmallRng::seed_from_u64(0x00b0_07ed_2709)))
            .insert_resource(EmberAccumulator { elapsed: 0.0 })
            .add_systems(Startup, load_effect_assets)
            .add_systems(
                Update,
                (
                    advance_effect_sprites,
                    spawn_hit_and_death_bursts,
                    spawn_recall_pulses,
                    spawn_ambient_embers,
                    clear_stains_on_match_reset,
                ),
            );
    }
}

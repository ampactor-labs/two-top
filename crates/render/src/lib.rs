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
use sim::{AnimState, LastSimTickTime, NoInterpolate, PositionF, PreviousPositionF, TICK_HZ};

/// 3-row × 45-column atlas (48×48 cells, 2160×144 px sheet):
/// row 0 = side-facing, row 1 = back (walking away), row 2 = front (walking toward).
/// Each row: IDLE6 RUN6 THROW8 DASH4 HIT4 CATCH3 DEATH10 CHARGE4.
/// The engine selects the row from the character's movement direction.
pub fn player_atlas_layout(atlases: &mut Assets<TextureAtlasLayout>) -> Handle<TextureAtlasLayout> {
    atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(48),
        AnimState::TOTAL_ATLAS_FRAMES as u32,
        FACING_DIR_COUNT as u32,
        None,
        None,
    ))
}

/// Facing direction rows in the player atlas.
pub const FACING_SIDE: u16 = 0;
pub const FACING_BACK: u16 = 1;
pub const FACING_FRONT: u16 = 2;
pub const FACING_DIR_COUNT: u16 = 3;

/// Onscreen render size for player sprites: 64 world units. Unchanged by the
/// 48 px source bump — the drifter occupies the same gameplay footprint and
/// just renders at finer texel density (≈1.33 world units/texel) than the old
/// 32 px rig, per ART_DIRECTION.md v2 rationale.
pub const PLAYER_RENDER_SIZE: f32 = 80.0;

/// Vertical foreshorten of the world on screen — the "camera tilt" that turns
/// the flat top-down floor into a Boomerang-Fu/HLD tabletop you look *into*.
/// 1.0 = dead-flat top-down; lower = more tilt. 0.75 sits between HLD's subtle
/// tilt and BFu's stronger one. Applied to every world-space Y at render time
/// (positions foreshorten; sprite HEIGHTS stay full so actors stand upright).
/// Render-only — sim stays in true coordinates, so determinism is untouched.
pub const WORLD_TILT_Y: f32 = 0.75;

/// Foreshorten a world-space Y for rendering (the single tilt hook).
/// For SIZE foreshortening (tile heights, view extents), pass raw Y.
/// For POSITION foreshortening, multiply `y` by `PerspectiveFlip.0` first
/// so P1's client sees the world Y-mirrored (their character at the bottom).
#[inline]
pub fn tilt_y(y: f32) -> f32 {
    y * WORLD_TILT_Y
}

/// Per-client Y-sign for the depth-duel perspective. `1.0` on P0's device
/// (or couch/observer), `-1.0` on P1's device — makes each player see
/// themselves at the bottom of their phone, opponent at the top.
/// Render-only; sim stays in true coordinates.
#[derive(Resource, Clone, Copy)]
pub struct PerspectiveFlip(pub f32);

impl Default for PerspectiveFlip {
    fn default() -> Self {
        Self(1.0)
    }
}

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

    pub const VOID: Color = p(16, 14, 34);
    pub const DEEP_ASH: Color = p(32, 28, 66);
    pub const BRUISE_SHADOW: Color = p(66, 36, 92);
    pub const CHARCOAL_LINE: Color = p(94, 64, 132);
    pub const COLD_STONE: Color = p(104, 126, 168);
    pub const WARM_BONE_SHADE: Color = p(130, 96, 102);
    pub const BONE: Color = p(210, 196, 156);
    pub const HOT_BONE: Color = p(255, 243, 202);
    pub const BLOOD_DARK: Color = p(122, 28, 66);
    pub const P0_BLOOD: Color = p(226, 52, 84);
    pub const EMBER: Color = p(245, 112, 60);
    pub const SPARK: Color = p(255, 220, 112);
    pub const DEEP_TEAL: Color = p(16, 118, 132);
    pub const P1_CYAN: Color = p(52, 212, 226);
    pub const RECALL_BLUE: Color = p(86, 120, 255);
    pub const HIT_WHITE: Color = p(250, 248, 240);
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
    flip: Res<PerspectiveFlip>,
    mut q: Query<(
        &PositionF,
        &PreviousPositionF,
        Option<&NoInterpolate>,
        &mut Transform,
    )>,
) {
    let alpha = interpolation_alpha(time.elapsed_secs_f64() as f32, last_tick.0 as f32, TICK_HZ);
    let f = flip.0;
    for (pos, prev, no_interp, mut xform) in &mut q {
        let v = if no_interp.is_some() {
            let (x, y) = pos.0.to_f32();
            Vec2::new(x, y)
        } else {
            lerp_position(prev.0, pos.0, alpha)
        };
        xform.translation.x = v.x;
        xform.translation.y = tilt_y(v.y * f);
    }
}

// =========================================================================
// 2.5D depth (render-only). Hyper Light Drifter sells "3D" inside a 2D engine
// with three cues: actors stand ON the ground (drop shadows), nearer actors
// draw OVER farther ones (y-sort), and cover has HEIGHT (the app's raised
// obstacle composite). This module owns the first two. All of it is render-
// only — it reads the render-derived `Transform` and writes only
// `Transform.z` + cosmetic shadow entities, never sim state (CONVENTIONS
// § Render Layer Rules), so the determinism matrix never sees it.
// =========================================================================

/// Ground actors (the duelists) and raised cover sort within this z band:
/// lower-on-screen (smaller world-y — nearer the camera in the 3/4 tilt) →
/// higher z → drawn in front. Held strictly *below* the boomerang (z=0.5) and
/// its trail (z=0.45) so the priority-#1 fang read always sits cleanest on top
/// (DESIGN_DIRECTION § 3); floor stains / pickups / arena props sit below the
/// band (z ≤ -0.45).
pub const GROUND_Z_BACK: f32 = 0.0;
pub const GROUND_Z_FRONT: f32 = 0.4;
/// World-y mapped to the front/back edges of the band — roughly the arena half-
/// height, so the whole field uses the band. Past it the z clamps (an out-of-
/// bounds actor just pins to the nearest/farthest layer rather than inverting).
const GROUND_Z_HALF_SPAN_CM: f32 = 800.0;

/// Map a ground-contact world-y to a draw z: smaller y (nearer) → larger z.
/// The single source of the painter's order shared by the y-sort system, the
/// drop shadows, and the app's static obstacle blocks.
pub fn ground_z(foot_y: f32) -> f32 {
    let mid = (GROUND_Z_BACK + GROUND_Z_FRONT) * 0.5;
    let half = (GROUND_Z_FRONT - GROUND_Z_BACK) * 0.5;
    let t = (foot_y / GROUND_Z_HALF_SPAN_CM).clamp(-1.0, 1.0);
    mid - t * half
}

/// Marker: a *moving* ground actor (the duelists) whose z must track its foot
/// line every frame. `foot_offset` is how far below the centre-anchored
/// sprite's origin the ground-contact point sits (≈ the feet).
#[derive(Component, Clone, Copy, Debug)]
pub struct YSorted {
    pub foot_offset: f32,
}

/// Render-side: write each [`YSorted`] entity's draw z from its foot line.
/// Ordered after `sync_transforms_from_sim` so it reads the interpolated y.
pub fn apply_ground_ysort(mut q: Query<(&YSorted, &mut Transform)>) {
    for (ys, mut xform) in &mut q {
        xform.translation.z = ground_z(xform.translation.y - ys.foot_offset);
    }
}

/// A cosmetic drop shadow that tracks `target`'s foot point each frame —
/// the single cheapest "actors stand on the ground" cue. Render-only; it
/// despawns itself the frame its target is gone, so a boomerang's shadow is
/// cleaned when the fang despawns with zero lifecycle plumbing at the spawn
/// site.
#[derive(Component, Clone, Copy, Debug)]
pub struct GroundShadow {
    pub target: Entity,
    pub foot_offset: f32,
}

/// Render-side: park each [`GroundShadow`] under its target's foot point and
/// just below it in z so the actor always reads on top. Orphaned shadows
/// (target despawned) remove themselves. Disjoint from its target query via
/// `Without<GroundShadow>`.
pub fn sync_ground_shadows(
    mut commands: Commands,
    targets: Query<&Transform, Without<GroundShadow>>,
    mut shadows: Query<(Entity, &GroundShadow, &mut Transform)>,
) {
    for (entity, shadow, mut xform) in &mut shadows {
        let Ok(target) = targets.get(shadow.target) else {
            commands.entity(entity).despawn();
            continue;
        };
        let foot_y = target.translation.y - shadow.foot_offset;
        xform.translation.x = target.translation.x;
        xform.translation.y = foot_y;
        xform.translation.z = ground_z(foot_y) - 0.01;
    }
}

/// Soft alpha the void shadow texture is tinted to when it lands on the floor.
pub const GROUND_SHADOW_ALPHA: f32 = 0.5;

/// Spawn a cosmetic drop shadow that tracks `target`. `width` is the world-unit
/// shadow diameter (height is half — the 3/4-tilt foreshortening); `foot_offset`
/// is how far below the target's origin its ground point sits. Returns the
/// shadow entity. Render-only; cleaned by [`sync_ground_shadows`] when the
/// target dies (and torn down with the match by the app's despawn filter).
pub fn spawn_ground_shadow(
    commands: &mut Commands,
    image: Handle<Image>,
    target: Entity,
    foot_offset: f32,
    width: f32,
    at: Vec2,
) -> Entity {
    let mut color = Color::WHITE; // multiplies the opaque void texture
    color.set_alpha(GROUND_SHADOW_ALPHA);
    let foot_y = at.y - foot_offset;
    commands
        .spawn((
            Sprite {
                image,
                color,
                custom_size: Some(Vec2::new(width, width * 0.5)),
                ..default()
            },
            Transform::from_xyz(at.x, foot_y, ground_z(foot_y) - 0.01),
            GroundShadow {
                target,
                foot_offset,
            },
        ))
        .id()
}

pub struct RenderSyncPlugin;

impl Plugin for RenderSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                sync_transforms_from_sim,
                sync_pyre_visuals,
                sync_crossing_visuals,
                sync_reliquary_visuals,
                ensure_pickup_visuals,
                tint_boomerangs_by_modifier,
                // Depth pass: z from the foot line + shadows, after x/y is set.
                apply_ground_ysort.after(sync_transforms_from_sim),
                sync_ground_shadows.after(sync_transforms_from_sim),
                sync_charge_auras.after(sync_transforms_from_sim),
            ),
        );
    }
}

/// Per-kind tint for a boomerang sprite so a modified throw reads at a
/// glance (matches the pickup-icon colors). `None` → no tint (white
/// multiplier keeps the bone fang's own color).
fn boomerang_tint(modifier: Option<sim::PickupKind>) -> Color {
    use sim::PickupKind::*;
    match modifier {
        None => Color::WHITE,
        Some(Fire) => palette::EMBER,
        Some(Heavy) => palette::COLD_STONE,
        Some(Bouncy) => palette::SPARK,
        Some(Curve) => palette::RECALL_BLUE,
        Some(Multishot) => palette::HOT_BONE,
        Some(Phantom) => palette::BRUISE_SHADOW,
    }
}

/// Scale a color's linear RGB by `f` (alpha preserved). `f > 1.0` overdrives
/// past white for HDR bloom; `f < 1.0` dims. The single knob the FX use to push
/// an accent into (or pull it out of) the bloom threshold.
pub fn scale_color(c: Color, f: f32) -> Color {
    let l = c.to_linear();
    Color::linear_rgba(l.red * f, l.green * f, l.blue * f, l.alpha)
}

/// Mild HDR overdrive on an in-flight fang so it reads as a glowing weapon.
const FANG_GLOW: f32 = 1.3;

/// `Update` system: tint every boomerang sprite by its active modifier, and
/// distinguish a dropped (Loose) fang — dimmed with a slow "grab me" pulse so
/// it reads as an item on the ground, not an in-flight threat. In-flight fangs
/// get a mild overdrive so they glow. Cheap (a handful of live fangs).
pub fn tint_boomerangs_by_modifier(
    time: Res<Time<Real>>,
    mut q: Query<(&sim::Boomerang, &sim::BoomerangMods, &mut Sprite)>,
) {
    for (boom, mods, mut sprite) in &mut q {
        let base = boomerang_tint(mods.modifier);
        sprite.color = match boom.state {
            sim::BoomerangState::Loose => {
                let pulse = 0.55 + 0.18 * (time.elapsed_secs() * 4.5).sin();
                scale_color(base, pulse)
            }
            _ => scale_color(base, FANG_GLOW),
        };
    }
}

// =========================================================================
// Throw-charge aura (render-only). A bright energy ring gathers under a
// charging duelist and tightens + brightens toward full charge (overdriven
// past white so it blooms harder as the throw peaks). Reads the rolled-back
// `sim::ThrowCharge` (read-only) and drives a tracked cosmetic ring — never
// writes sim (CONVENTIONS § Render Layer Rules).
// =========================================================================

/// A charge ring that tracks `target`'s feet, sized/brightened by the target's
/// `sim::ThrowCharge`. Hidden while not charging. Render-only.
#[derive(Component, Clone, Copy, Debug)]
pub struct ChargeAura {
    pub target: Entity,
    pub foot_offset: f32,
}

/// Ring diameter (world units) at zero vs full charge — it TIGHTENS inward as
/// the throw builds (energy gathering).
pub const CHARGE_AURA_MAX_SIZE: f32 = 84.0;
pub const CHARGE_AURA_MIN_SIZE: f32 = 46.0;
/// Ring spin rate (rad/s) — a slow rotation for the "gathering energy" read.
pub const CHARGE_AURA_SPIN: f32 = 2.2;

/// Spawn a hidden charge ring tracking `target`. Returns the entity.
pub fn spawn_charge_aura(
    commands: &mut Commands,
    image: Handle<Image>,
    target: Entity,
    foot_offset: f32,
) -> Entity {
    commands
        .spawn((
            Sprite {
                image,
                custom_size: Some(Vec2::splat(CHARGE_AURA_MAX_SIZE)),
                ..default()
            },
            Transform::default(),
            Visibility::Hidden,
            ChargeAura {
                target,
                foot_offset,
            },
        ))
        .id()
}

/// `Update` system: drive each [`ChargeAura`] from its target's `ThrowCharge` —
/// size tightens and brightness overdrives toward full charge; hidden at zero.
pub fn sync_charge_auras(
    time: Res<Time<Real>>,
    targets: Query<(&Transform, &sim::ThrowCharge), Without<ChargeAura>>,
    mut auras: Query<(&ChargeAura, &mut Transform, &mut Sprite, &mut Visibility)>,
) {
    for (aura, mut xf, mut sprite, mut vis) in &mut auras {
        let Ok((target, charge)) = targets.get(aura.target) else {
            *vis = Visibility::Hidden;
            continue;
        };
        if charge.0 == 0 {
            *vis = Visibility::Hidden;
            continue;
        }
        *vis = Visibility::Visible;
        let power = (charge.0 as f32 / sim::CHARGE_MAX_FRAMES as f32).clamp(0.0, 1.0);
        let size = CHARGE_AURA_MAX_SIZE + (CHARGE_AURA_MIN_SIZE - CHARGE_AURA_MAX_SIZE) * power;
        sprite.custom_size = Some(Vec2::splat(size));
        // Overdrive past white toward full charge → the ring blooms harder.
        let bright = 1.0 + power * 1.1;
        sprite.color = Color::linear_rgb(bright, bright, bright);
        let foot_y = target.translation.y - aura.foot_offset;
        xf.translation.x = target.translation.x;
        xf.translation.y = foot_y;
        // Just under the actor, above its ground shadow.
        xf.translation.z = ground_z(foot_y) - 0.005;
        xf.rotation = Quat::from_rotation_z(time.elapsed_secs() * CHARGE_AURA_SPIN);
    }
}

/// `Update` system: give each freshly-spawned floor `Pickup` (sim) entity a
/// sprite from the 6-cell pickup atlas, indexed by kind. Pickups don't
/// move, so the transform is set once here; when sim despawns the pickup
/// (collected / expired) the sprite goes with the entity. A `Local` caches
/// the atlas layout so we don't leak a layout asset per spawn.
pub fn ensure_pickup_visuals(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    flip: Res<PerspectiveFlip>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
    mut layout: Local<Option<Handle<TextureAtlasLayout>>>,
    q: Query<(Entity, &sim::Pickup, &sim::PositionF), Without<Sprite>>,
) {
    if q.is_empty() {
        return;
    }
    let layout = layout
        .get_or_insert_with(|| {
            atlases.add(TextureAtlasLayout::from_grid(
                UVec2::splat(24),
                6,
                1,
                None,
                None,
            ))
        })
        .clone();
    let image = asset_server.load("sprites/pickups/pickup_sheet.png");
    // Render a touch larger than the 32 cm hitbox so it pops on the floor.
    let size = (sim::PICKUP_HALF_EXTENT_CM * 2) as f32 * 1.8;
    for (entity, pickup, pos) in &q {
        let (x, y) = pos.0.to_f32();
        commands.entity(entity).insert((
            Sprite {
                image: image.clone(),
                texture_atlas: Some(TextureAtlas {
                    layout: layout.clone(),
                    index: pickup.kind.as_u8() as usize,
                }),
                custom_size: Some(Vec2::splat(size)),
                ..default()
            },
            // On the floor (below the ground-actor y-sort band) so a duelist
            // always steps cleanly *over* a pickup, above stains/props.
            Transform::from_xyz(x, tilt_y(y * flip.0), -0.45),
        ));
    }
}

// ---- Phase 16: arena prop visuals ----

/// Marker: the bone-bridge overlay (Crossing) — visible only while raised.
#[derive(Component)]
pub struct BridgeVisual;

/// Marker: an altar sigil quad (Crossing) — lit while the bridge is raised.
#[derive(Component)]
pub struct SigilVisual;

/// Marker: a sigil door (Reliquary) — dimmed while on cooldown.
#[derive(Component)]
pub struct DoorVisual;

/// Marker on *every* entity [`spawn_arena_props`] creates (pyres, chasm tiles,
/// bridge, sigils, doors). Lets the app despawn a whole arena's props in one
/// query when tearing a match down (Phase 18 back-to-lobby / arena switch),
/// regardless of which per-prop marker an entity also carries.
#[derive(Component)]
pub struct ArenaProp;

pub fn spawn_arena_props(
    commands: &mut Commands,
    asset_server: &AssetServer,
    atlases: &mut Assets<TextureAtlasLayout>,
    selected: &sim::SelectedArena,
    flip: f32,
) {
    spawn_pyres(commands, asset_server, atlases, selected, flip);
    match selected.0 {
        sim::ArenaId::Crossing => spawn_crossing(commands, asset_server, atlases, flip),
        sim::ArenaId::Reliquary => spawn_reliquary(commands, asset_server, atlases, flip),
        sim::ArenaId::Anchor => {}
    }
}

fn rect_center_size(rect: fixed_math::RectF, flip: f32) -> (Vec2, Vec2) {
    let (min_x, min_y) = rect.min.to_f32();
    let (max_x, max_y) = rect.max.to_f32();
    (
        Vec2::new((min_x + max_x) * 0.5, tilt_y((min_y + max_y) * 0.5 * flip)),
        Vec2::new(max_x - min_x, (max_y - min_y) * WORLD_TILT_Y),
    )
}

fn spawn_pyres(
    commands: &mut Commands,
    asset_server: &AssetServer,
    atlases: &mut Assets<TextureAtlasLayout>,
    selected: &sim::SelectedArena,
    flip: f32,
) {
    let layout = atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(32),
        3,
        1,
        None,
        None,
    ));
    let image = asset_server.load("sprites/arena/bone_pyre_sheet.png");
    for pyre in sim::arena_pyres_for(selected.0) {
        let (center, size) = rect_center_size(pyre.rect, flip);
        commands.spawn((
            ArenaProp,
            pyre,
            Sprite {
                image: image.clone(),
                texture_atlas: Some(TextureAtlas {
                    layout: layout.clone(),
                    index: 0,
                }),
                custom_size: Some(size),
                ..default()
            },
            // Just above the floor (z=-1), below players/boomerangs (z=0).
            Transform::from_xyz(center.x, center.y, -0.5),
        ));
    }
}

fn spawn_crossing(
    commands: &mut Commands,
    asset_server: &AssetServer,
    atlases: &mut Assets<TextureAtlasLayout>,
    flip: f32,
) {
    let (chasm_c, chasm_sz) = rect_center_size(sim::crossing_chasm(), flip);
    let tile_h = chasm_sz.x * 2.0 * WORLD_TILT_Y; // 1:2 tiles, Y foreshortened to the tabletop
    let n = (chasm_sz.y / tile_h).ceil() as i32 + 1;
    let chasm_img = asset_server.load("sprites/arena/chasm_strip.png");
    let bridge_img = asset_server.load("sprites/arena/bone_bridge_tile.png");
    let start = chasm_c.y - (n as f32 - 1.0) * tile_h * 0.5;
    for i in 0..n {
        let y = start + i as f32 * tile_h;
        // Chasm pit (z=-0.9, just above floor).
        commands.spawn((
            ArenaProp,
            Sprite {
                image: chasm_img.clone(),
                custom_size: Some(Vec2::new(chasm_sz.x, tile_h)),
                ..default()
            },
            Transform::from_xyz(chasm_c.x, y, -0.9),
        ));
        // Bone bridge overlay (z=-0.8), hidden until a sigil raises it.
        commands.spawn((
            ArenaProp,
            BridgeVisual,
            Sprite {
                image: bridge_img.clone(),
                custom_size: Some(Vec2::new(chasm_sz.x, tile_h)),
                ..default()
            },
            Transform::from_xyz(chasm_c.x, y, -0.8),
            Visibility::Hidden,
        ));
    }
    // Altar sigils (2-cell sheet: 0 idle / 1 lit) on each side.
    let sigil_layout = atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(32),
        2,
        1,
        None,
        None,
    ));
    let sigil_img = asset_server.load("sprites/arena/altar_sigil_sheet.png");
    for sigil in sim::crossing_sigils() {
        let (center, size) = rect_center_size(sigil, flip);
        commands.spawn((
            ArenaProp,
            SigilVisual,
            Sprite {
                image: sigil_img.clone(),
                texture_atlas: Some(TextureAtlas {
                    layout: sigil_layout.clone(),
                    index: 0,
                }),
                custom_size: Some(size * 1.6),
                ..default()
            },
            Transform::from_xyz(center.x, center.y, -0.7),
        ));
    }
}

fn spawn_reliquary(
    commands: &mut Commands,
    asset_server: &AssetServer,
    atlases: &mut Assets<TextureAtlasLayout>,
    flip: f32,
) {
    let layout = atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(32),
        2,
        1,
        None,
        None,
    ));
    let image = asset_server.load("sprites/arena/sigil_door_sheet.png");
    for (footprint, _exit) in sim::reliquary_doors() {
        let (center, size) = rect_center_size(footprint, flip);
        commands.spawn((
            ArenaProp,
            DoorVisual,
            Sprite {
                image: image.clone(),
                texture_atlas: Some(TextureAtlas {
                    layout: layout.clone(),
                    index: 0,
                }),
                custom_size: Some(size * 1.5),
                ..default()
            },
            Transform::from_xyz(center.x, center.y, -0.6),
        ));
    }
}

/// Dim the Reliquary sigil doors while they're on cooldown (cell 1), active
/// otherwise (cell 0). Reads the rolled-back `DoorCooldown` in `Update`.
pub fn sync_reliquary_visuals(
    frame: Res<sim::FrameCount>,
    cooldown: Res<sim::DoorCooldown>,
    mut doors: Query<&mut Sprite, With<DoorVisual>>,
) {
    let on_cooldown = frame.0 < cooldown.until_frame;
    for mut sprite in &mut doors {
        if let Some(atlas) = sprite.texture_atlas.as_mut() {
            atlas.index = if on_cooldown { 1 } else { 0 };
        }
    }
}

/// Toggle the Crossing bone-bridge overlay + sigil lit-state from the
/// rolled-back `BridgeState`. Runs in `Update` (post-rollback) so it reads
/// the authoritative bridge timer. A no-op on other arenas (no markers).
pub fn sync_crossing_visuals(
    frame: Res<sim::FrameCount>,
    bridge: Res<sim::BridgeState>,
    mut bridges: Query<&mut Visibility, With<BridgeVisual>>,
    mut sigils: Query<&mut Sprite, With<SigilVisual>>,
) {
    let active = bridge.is_active(frame.0);
    for mut vis in &mut bridges {
        *vis = if active {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    for mut sprite in &mut sigils {
        if let Some(atlas) = sprite.texture_atlas.as_mut() {
            atlas.index = if active { 1 } else { 0 };
        }
    }
}

/// Swap a pyre's atlas frame to match its shatter state: intact (cell 0)
/// while whole, shattered rubble (cell 2) once broken. Runs unconditionally
/// over every pyre each frame (idempotent) rather than filtering on
/// `Changed<BonePyre>`: per CONVENTIONS § Render Layer Rules, rollback re-sim
/// makes `Changed`/`Added` fire unreliably, so an edge-filtered visual can
/// miss — or wrongly replay — a shatter after a rollback. There are only a
/// handful of pyres, so the per-frame write is free.
pub fn sync_pyre_visuals(mut q: Query<(&sim::BonePyre, &mut Sprite)>) {
    for (pyre, mut sprite) in &mut q {
        if let Some(atlas) = sprite.texture_atlas.as_mut() {
            atlas.index = if pyre.shattered { 2 } else { 0 };
        }
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

/// Phase 18 Task 5.6 — hard ceiling on live [`EffectSprite`]s. A fire-trail
/// Multishot slugfest can spray hundreds of bursts per second; this bounds the
/// render-side particle count so a stress scene can't unbounded-grow the
/// entity count (and the per-frame `advance_effect_sprites` cost) on a phone.
pub const EFFECT_SPRITE_CAP: usize = 500;

/// Pure cull selector (testable): given each live sprite's `(id, progress)`
/// where progress is `current / frames` in `[0, 1]`, return the ids to despawn
/// to get back to `cap` — the *most-finished* first (nearest to auto-despawn,
/// so the least visually disruptive to drop, and effectively oldest-first
/// since a sprite's progress rises monotonically with age). Empty at/under cap.
fn select_effect_culls<T: Copy>(mut sprites: Vec<(T, f32)>, cap: usize) -> Vec<T> {
    if sprites.len() <= cap {
        return Vec::new();
    }
    let excess = sprites.len() - cap;
    // Most-progressed first; NaN sorts as "least" so it survives over real work.
    sprites.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sprites.into_iter().take(excess).map(|(id, _)| id).collect()
}

/// Render-side: enforce [`EFFECT_SPRITE_CAP`]. Free on every frame the count is
/// under the cap (one query, no allocation); only collects + sorts when over.
pub fn cull_excess_effects(mut commands: Commands, q: Query<(Entity, &EffectSprite)>) {
    if q.iter().count() <= EFFECT_SPRITE_CAP {
        return;
    }
    let sprites: Vec<(Entity, f32)> = q
        .iter()
        .map(|(entity, effect)| {
            let progress = if effect.frames == 0 {
                1.0
            } else {
                effect.current as f32 / effect.frames as f32
            };
            (entity, progress)
        })
        .collect();
    for entity in select_effect_culls(sprites, EFFECT_SPRITE_CAP) {
        commands.entity(entity).despawn();
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
    /// 4-cell ground-dust burst (14×14) kicked up on a dash.
    pub dust_puff: (Handle<Image>, Handle<TextureAtlasLayout>),
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
    // Dash dust: 4 cells x 14x14.
    let dust_layout = atlases.add(TextureAtlasLayout::from_grid(
        UVec2::splat(14),
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
        dust_puff: (
            asset_server.load("sprites/fx/dust_puff_sheet.png"),
            dust_layout,
        ),
    });
}

/// Approx. world units below a duelist's centre-anchored origin to its feet —
/// where dash dust kicks up + effects ground (mirrors the app's foot offset).
const RENDER_FOOT_OFFSET: f32 = 26.0;

/// Tracks each player's last-seen `Dashing` state so [`spawn_dash_dust`] fires
/// dust once, on the tick a dash begins.
#[derive(Default)]
pub struct PrevDashing(pub bevy::platform::collections::HashMap<usize, bool>);

/// Render-side: kick up a ground-dust puff at a duelist's feet the tick it
/// enters a dash. Reads the rolled-back `DashState` edge (render-only; the dust
/// is a cosmetic `EffectSprite`, never rolled back).
pub fn spawn_dash_dust(
    mut commands: Commands,
    assets: Res<EffectAssets>,
    players: Query<(&Player, &sim::DashState, &Transform)>,
    mut prev: Local<PrevDashing>,
) {
    for (player, dash, xform) in &players {
        let now = matches!(dash, sim::DashState::Dashing { .. });
        let was = prev.0.get(&player.handle).copied().unwrap_or(false);
        if now && !was {
            let feet = xform.translation.truncate() - Vec2::new(0.0, RENDER_FOOT_OFFSET);
            spawn_effect(
                &mut commands,
                assets.dust_puff.0.clone(),
                assets.dust_puff.1.clone(),
                4,
                0.045,
                feet,
                44.0,
                -0.4, // on the floor, above stains
            );
        }
        prev.0.insert(player.handle, now);
    }
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
    mut shake: ResMut<ScreenShake>,
    mut last_kill: ResMut<LastKillPos>,
    players: Query<(&Player, &Dead, &Transform)>,
    mut prev: Local<PrevDying>,
) {
    for (player, dead, xform) in &players {
        let now_dying = dead.is_dying();
        let was_dying = prev.0.get(&player.handle).copied().unwrap_or(false);
        if now_dying && !was_dying {
            let pos = xform.translation.truncate();
            // Kill feedback: camera kick + a hard 2-frame white flash, and
            // record where it happened so the kill-cam can punch in on the
            // round/match-ending blow.
            shake.add_trauma(TRAUMA_KILL);
            last_kill.0 = pos;
            spawn_kill_flash(&mut commands);
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

/// Tracks each live Boomerang *entity's* last-observed `BoomerangState`.
/// Keyed by `Entity`, not `owner_handle`: Multishot gives several fangs the
/// same owner, so an owner-keyed cache lets same-owner fangs clobber each
/// other's edge within a single frame — masking or duplicating recall pulses.
/// The map is rebuilt from the live boomerangs each frame, so despawned fangs
/// drop out and it can't grow unbounded. Mirrors the Entity-keyed
/// [`PrevShattered`].
#[derive(Default)]
pub struct PrevBoomerangState(pub bevy::platform::collections::HashMap<Entity, BoomerangState>);

/// Render-side detector: spawns recall_pulse at the boomerang's
/// position the tick its state transitions from `Flying` to
/// `Returning`. The pulse reads as the recall-energy emanation.
pub fn spawn_recall_pulses(
    mut commands: Commands,
    assets: Res<EffectAssets>,
    boomerangs: Query<(Entity, &Boomerang, &Transform)>,
    mut prev: Local<PrevBoomerangState>,
) {
    let mut next = bevy::platform::collections::HashMap::default();
    for (entity, boom, xform) in &boomerangs {
        let curr = boom.state;
        let was = prev
            .0
            .get(&entity)
            .copied()
            .unwrap_or(BoomerangState::Flying);
        if matches!(was, BoomerangState::Flying) && matches!(curr, BoomerangState::Returning { .. })
        {
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
        next.insert(entity, curr);
    }
    prev.0 = next;
}

// ---- Boomerang flight trail (DESIGN_DIRECTION § 3) ----
//
// Short, hard-edged ghost-stamps of the live fang along its path — a
// readability aid for priority-#1, NOT a decorative smear. Color encodes
// state: Recall Blue while returning (the "it's coming back to me" read),
// the active modifier's color, else a quiet owner channel. Render-only,
// sampled from the interpolated transform; never feeds sim. The on-fang
// blood-marks were cut (the floor stains carry violence-memory instead).

/// One faded ghost-stamp of the boomerang. Fades to nothing over `ttl`
/// real seconds, then despawns — so the trail is always short.
#[derive(Component)]
pub struct TrailGhost {
    pub age: f32,
    pub ttl: f32,
}

/// Per-boomerang last-stamp position, so stamps are spaced by distance
/// (framerate-independent) rather than per-frame. Rebuilt from live
/// boomerangs each frame, so despawned fangs drop out (no unbounded growth).
#[derive(Default)]
pub struct TrailStampPos(pub bevy::platform::collections::HashMap<Entity, Vec2>);

/// World-units of travel between ghost-stamps (~half a fang).
pub const TRAIL_STAMP_SPACING: f32 = 22.0;
/// Real seconds a ghost-stamp lives — short so the trail never walls off
/// the arena.
pub const TRAIL_GHOST_TTL: f32 = 0.13;
/// Starting opacity of a fresh stamp (the wake stays quieter than the fang).
pub const TRAIL_GHOST_ALPHA: f32 = 0.5;

/// State→color for a trail stamp. Returning overrides everything (the return
/// read is the one players most need); else the active modifier's color; else
/// a quiet owner channel that doubles as a who-threw-it cue.
pub fn trail_tint(
    returning: bool,
    owner_handle: usize,
    modifier: Option<sim::PickupKind>,
) -> Color {
    if returning {
        palette::RECALL_BLUE
    } else if modifier.is_some() {
        boomerang_tint(modifier)
    } else if owner_handle == 0 {
        palette::BLOOD_DARK
    } else {
        palette::DEEP_TEAL
    }
}

/// Render-side: stamp a faded fang ghost each time a live boomerang has
/// travelled `TRAIL_STAMP_SPACING` since its last stamp.
pub fn spawn_boomerang_trail(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut image: Local<Option<Handle<Image>>>,
    mut last: Local<TrailStampPos>,
    boomerangs: Query<(Entity, &Boomerang, &Transform, &sim::BoomerangMods)>,
) {
    let img = image
        .get_or_insert_with(|| asset_server.load("sprites/projectiles/bone_fang.png"))
        .clone();
    let mut next = bevy::platform::collections::HashMap::default();
    for (entity, boom, xform, mods) in &boomerangs {
        let pos = xform.translation.truncate();
        let prev = last.0.get(&entity).copied();
        let stamp = prev.is_none_or(|p| pos.distance(p) >= TRAIL_STAMP_SPACING);
        if stamp {
            let returning = matches!(boom.state, BoomerangState::Returning { .. });
            let tint = trail_tint(returning, boom.owner_handle, mods.modifier);
            commands.spawn((
                Sprite {
                    image: img.clone(),
                    color: tint.with_alpha(TRAIL_GHOST_ALPHA),
                    custom_size: Some(Vec2::splat(32.0)),
                    ..default()
                },
                // Just under the boomerang (z=0.5) so the live fang stays the
                // brightest, cleanest read.
                Transform::from_xyz(pos.x, pos.y, 0.45),
                TrailGhost {
                    age: 0.0,
                    ttl: TRAIL_GHOST_TTL,
                },
            ));
            next.insert(entity, pos);
        } else {
            // keep the *old* anchor so distance keeps accumulating to threshold
            next.insert(entity, prev.unwrap_or(pos));
        }
    }
    last.0 = next;
}

/// Render-side: fade + despawn trail ghosts.
pub fn advance_trail_ghosts(
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut q: Query<(Entity, &mut TrailGhost, &mut Sprite)>,
) {
    let dt = time.delta_secs();
    for (entity, mut ghost, mut sprite) in &mut q {
        ghost.age += dt;
        if ghost.age >= ghost.ttl {
            commands.entity(entity).despawn();
            continue;
        }
        let k = 1.0 - ghost.age / ghost.ttl;
        sprite.color = sprite.color.with_alpha(TRAIL_GHOST_ALPHA * k);
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
    flip: Res<PerspectiveFlip>,
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
            Vec2::new(x, tilt_y(y * flip.0)),
            16.0,
            -0.5,
        );
    }
}

// =========================================================================
// Phase 18 Task 5.1 — screen shake + kill flash.
//
// Cosmetic camera kick + a brief fullscreen white flash on impactful
// events. Both read the SAME sim edges the effect sprites already detect
// (a player turning `is_dying`, a `BonePyre` shattering, an `Empowered`
// flag rising), so they compose for the replay viewer too. All render-
// only: `ScreenShake` is never rolled back, the offset is sampled from
// the cosmetic RNG (never `SimRng`), and the flash is a plain sprite
// (CONVENTIONS § Render Layer Rules).
// =========================================================================

/// Cosmetic screen-shake state. `trauma` is 0..1 energy that decays over
/// real time; the applied pixel offset scales with `trauma²` (the Vlambeer
/// trauma curve) so a graze barely nudges and a kill kicks hard. The camera
/// rig (app crate, which owns the `Transform`) decays this and samples a
/// fresh offset each frame; render's only job is to *add* trauma on impact
/// events. The rig recomputes the camera position from its base every
/// frame, so no applied-offset bookkeeping is needed here.
#[derive(Resource, Default, Debug)]
pub struct ScreenShake {
    pub trauma: f32,
}

/// Last kill position in world space, updated on every kill edge by
/// [`spawn_hit_and_death_bursts`]. The kill-cam (app crate) reads this when
/// a round/match ends to know where to punch in. Render-only.
#[derive(Resource, Default, Debug)]
pub struct LastKillPos(pub Vec2);

impl ScreenShake {
    /// Add `amount` of trauma, saturating at 1.0. Trauma is additive so a
    /// kill landed mid-shatter shakes harder, but the cap bounds the kick.
    pub fn add_trauma(&mut self, amount: f32) {
        self.trauma = (self.trauma + amount).clamp(0.0, 1.0);
    }
}

/// Trauma lost per real-time second (linear decay).
pub const SHAKE_DECAY_PER_SEC: f32 = 1.8;
/// Camera offset in world units at `trauma == 1.0` (the source sprites are
/// 32 px at 2× world scale, so 6 world units ≈ 3 source texels of kick).
pub const SHAKE_MAX_OFFSET: f32 = 6.0;
/// Kill shake. This is a one-hit-kill game — every landed contact is a
/// death (`award_kill`), so the plan's separate "hit" and "death" events
/// coincide; the kill edge gets the stronger death-magnitude kick.
pub const TRAUMA_KILL: f32 = 0.7;
/// Pyre-shatter shake (arena geometry breaking).
pub const TRAUMA_SHATTER: f32 = 0.3;
/// Perfect-catch shake (the signature skill beat).
pub const TRAUMA_PERFECT_CATCH: f32 = 0.25;

/// Pure offset sampler: magnitude `trauma² × max`, direction `angle`
/// radians. Split out so the curve is unit-testable; the camera system
/// supplies a random angle from the cosmetic RNG each frame.
pub fn shake_offset(trauma: f32, max: f32, angle: f32) -> Vec2 {
    let mag = trauma * trauma * max;
    Vec2::new(angle.cos() * mag, angle.sin() * mag)
}

/// A brief fullscreen white flash quad (kill feedback). Lives `frames`
/// render frames at a constant alpha, then despawns. Spawned on the kill
/// edge by [`spawn_hit_and_death_bursts`].
#[derive(Component)]
pub struct KillFlash {
    pub frames_left: u8,
}

/// How many render frames the kill flash holds before despawning.
pub const KILL_FLASH_FRAMES: u8 = 2;
/// Flash alpha (contact-mode accent — feedback, not information).
pub const KILL_FLASH_ALPHA: f32 = 0.6;
/// Spawn a kill flash covering the whole view. A single large quad at the
/// arena origin (z above gameplay + effects, below the HUD legend) covers
/// both the static desktop camera and the zoomed mobile follow cam, which
/// always centres well inside the arena — no per-frame reposition needed.
fn spawn_kill_flash(commands: &mut Commands) {
    let mut color = palette::HIT_WHITE;
    color.set_alpha(KILL_FLASH_ALPHA);
    commands.spawn((
        Sprite {
            color,
            custom_size: Some(Vec2::splat(8000.0)),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 50.0),
        KillFlash {
            frames_left: KILL_FLASH_FRAMES,
        },
    ));
}

/// Render-side: tick every [`KillFlash`] down one render frame and despawn
/// when spent. Constant alpha (no fade) per the plan — a hard 2-frame
/// strobe reads as impact, not a dissolve.
pub fn advance_kill_flash(mut commands: Commands, mut q: Query<(Entity, &mut KillFlash)>) {
    for (entity, mut flash) in &mut q {
        if flash.frames_left <= 1 {
            commands.entity(entity).despawn();
        } else {
            flash.frames_left -= 1;
        }
    }
}

/// Tracks each player's last-seen `Empowered` flag so the shake fires once
/// on the rising edge of a perfect catch (same per-handle edge pattern as
/// [`PrevDying`]).
#[derive(Default)]
pub struct PrevEmpowered(pub bevy::platform::collections::HashMap<usize, bool>);

/// Render-side: add perfect-catch trauma the tick a player's `Empowered`
/// flag rises (a catch inside the perfect window). Falling edges (the
/// empowered throw consuming the flag) don't shake.
pub fn shake_on_perfect_catch(
    mut shake: ResMut<ScreenShake>,
    players: Query<(&Player, &sim::Empowered)>,
    mut prev: Local<PrevEmpowered>,
) {
    for (player, emp) in &players {
        let now = emp.0;
        let was = prev.0.get(&player.handle).copied().unwrap_or(false);
        if now && !was {
            shake.add_trauma(TRAUMA_PERFECT_CATCH);
        }
        prev.0.insert(player.handle, now);
    }
}

/// Tracks each pyre entity's last-seen `shattered` flag so the shake fires
/// once on the shatter edge — `Changed<BonePyre>` also fires on spawn, so
/// an explicit false→true edge is required to avoid a startup kick.
#[derive(Default)]
pub struct PrevShattered(pub bevy::platform::collections::HashMap<Entity, bool>);

/// Render-side: add shatter trauma the tick a `BonePyre` breaks.
pub fn shake_on_pyre_shatter(
    mut shake: ResMut<ScreenShake>,
    pyres: Query<(Entity, &sim::BonePyre)>,
    mut prev: Local<PrevShattered>,
) {
    for (entity, pyre) in &pyres {
        let was = prev.0.get(&entity).copied().unwrap_or(false);
        if pyre.shattered && !was {
            shake.add_trauma(TRAUMA_SHATTER);
        }
        prev.0.insert(entity, pyre.shattered);
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
            .init_resource::<ScreenShake>()
            .init_resource::<LastKillPos>()
            .add_systems(Startup, load_effect_assets)
            .add_systems(
                Update,
                (
                    advance_effect_sprites,
                    cull_excess_effects,
                    spawn_hit_and_death_bursts,
                    spawn_recall_pulses,
                    spawn_ambient_embers,
                    spawn_boomerang_trail,
                    advance_trail_ghosts,
                    clear_stains_on_match_reset,
                    advance_kill_flash,
                    shake_on_perfect_catch,
                    shake_on_pyre_shatter,
                    spawn_dash_dust,
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shake_offset_zero_trauma_is_zero() {
        let v = shake_offset(0.0, SHAKE_MAX_OFFSET, 1.234);
        assert!(v.length() < 1e-6);
    }

    #[test]
    fn shake_offset_full_trauma_hits_max_magnitude() {
        // trauma == 1.0 → magnitude == max, regardless of angle.
        for &angle in &[0.0, 0.7, 1.6, 3.1, 5.5] {
            let v = shake_offset(1.0, SHAKE_MAX_OFFSET, angle);
            assert!((v.length() - SHAKE_MAX_OFFSET).abs() < 1e-4);
        }
    }

    #[test]
    fn shake_offset_scales_quadratically() {
        // Half trauma → quarter magnitude (the trauma² curve).
        let half = shake_offset(0.5, SHAKE_MAX_OFFSET, 0.0).length();
        let full = shake_offset(1.0, SHAKE_MAX_OFFSET, 0.0).length();
        assert!((half - full * 0.25).abs() < 1e-4);
    }

    #[test]
    fn add_trauma_accumulates_and_clamps() {
        let mut s = ScreenShake::default();
        s.add_trauma(TRAUMA_PERFECT_CATCH);
        assert!((s.trauma - TRAUMA_PERFECT_CATCH).abs() < 1e-6);
        // Stacking past 1.0 saturates rather than overflowing the curve.
        s.add_trauma(1.0);
        assert!((s.trauma - 1.0).abs() < 1e-6);
    }

    #[test]
    fn effect_cull_is_noop_at_or_under_cap() {
        let items: Vec<(u32, f32)> = (0..5).map(|i| (i, 0.5)).collect();
        assert!(select_effect_culls(items, 5).is_empty());
        let items: Vec<(u32, f32)> = (0..3).map(|i| (i, 0.5)).collect();
        assert!(select_effect_culls(items, 5).is_empty());
    }

    #[test]
    fn effect_cull_drops_most_finished_first_back_to_cap() {
        // ids 0..6 with rising progress; cap 4 → drop the 2 most-progressed (5, 4).
        let items: Vec<(u32, f32)> = (0..6).map(|i| (i, i as f32 / 6.0)).collect();
        let mut culled = select_effect_culls(items, 4);
        culled.sort_unstable();
        assert_eq!(
            culled,
            vec![4, 5],
            "the two nearest-done sprites are culled"
        );
    }

    #[test]
    fn effect_cull_brings_count_to_exactly_cap() {
        let items: Vec<(u32, f32)> = (0..500).map(|i| (i, (i % 7) as f32 / 7.0)).collect();
        let culled = select_effect_culls(items, 120);
        assert_eq!(culled.len(), 500 - 120);
    }

    #[test]
    fn ground_z_orders_nearer_actors_in_front() {
        // Nearer the camera (smaller world-y) draws in front (higher z).
        let near = ground_z(-500.0);
        let far = ground_z(500.0);
        assert!(near > far, "lower-on-screen actor must sort in front");
        // The whole band stays under the boomerang/trail layer (0.45–0.5).
        for &y in &[-2000.0, -750.0, 0.0, 750.0, 2000.0] {
            let z = ground_z(y);
            assert!(
                (GROUND_Z_BACK..=GROUND_Z_FRONT).contains(&z),
                "z {z} for y {y} escaped the ground band",
            );
        }
        // Symmetric about the midline, and clamps past the half-span.
        assert!((ground_z(0.0) - (GROUND_Z_BACK + GROUND_Z_FRONT) * 0.5).abs() < 1e-6);
        assert_eq!(ground_z(-5000.0), GROUND_Z_FRONT, "clamps to nearest layer");
        assert_eq!(ground_z(5000.0), GROUND_Z_BACK, "clamps to farthest layer");
    }

    #[test]
    fn trail_tint_encodes_state() {
        // Returning overrides everything — the load-bearing "coming back" read.
        assert_eq!(
            trail_tint(true, 0, Some(sim::PickupKind::Fire)),
            palette::RECALL_BLUE
        );
        // Outbound with a modifier shows that modifier's color.
        assert_eq!(
            trail_tint(false, 0, Some(sim::PickupKind::Fire)),
            palette::EMBER
        );
        // Outbound, no modifier → a quiet per-owner channel (who threw it).
        assert_eq!(trail_tint(false, 0, None), palette::BLOOD_DARK);
        assert_eq!(trail_tint(false, 1, None), palette::DEEP_TEAL);
    }
}

//! The fangs on screen: sprites for new boomerangs and their growth.

use crate::*;

/// Render mirror of GROW-SLOW: the fang sprite swells with flight progress in
/// lockstep with its lethal rect (`sim::grown_half_extent`), so what you see
/// is what kills. Render-only, f32 (reads sim state, never writes).
#[allow(clippy::type_complexity)]
pub(crate) fn grow_boomerang_sprites(
    flip: Res<render::PerspectiveFlip>,
    mut q: Query<
        (
            &PositionF,
            Option<&sim::ThrowOrigin>,
            Option<&sim::ThrowReach>,
            &mut Sprite,
        ),
        With<Boomerang>,
    >,
) {
    let base_px = ((BOOMERANG_HALF_EXTENT_CM * 2) as f32) * 2.6;
    for (pos, origin, reach, mut sprite) in &mut q {
        let factor = match (origin, reach) {
            (Some(o), Some(r)) if r.0 > Fix::ZERO => {
                let half = sim::grown_half_extent((pos.0 - o.0).length(), r.0);
                half.to_num::<f32>() / BOOMERANG_HALF_EXTENT_CM as f32
            }
            _ => 1.0,
        };
        let (_, y) = pos.0.to_f32();
        let depth = render::depth_scale(y * flip.0);
        sprite.custom_size = Some(Vec2::splat(base_px * factor * depth));
    }
}

/// `Update`-schedule system: attaches a placeholder sprite + transform
/// to any `Boomerang` entity that doesn't already have a `Sprite`. The
/// initial `Transform` is seeded from the boomerang's `PositionF` so
/// the first rendered frame appears at the spawn position rather than
/// blinking through origin before `render::sync_transforms_from_sim`
/// catches up. Subsequent frames are driven by the render-side
/// interpolator.
///
/// Z-order: 0.5 — above the arena walls (z=-1.0) and above the players
/// (z=0.0 by default), so a recalled boomerang reads cleanly when it
/// passes over the player on its way home.
///
/// Lives in the `app` crate (not `render`) because `render` is built
/// with a minimal Bevy feature set (no `bevy_sprite`); only `app`
/// pulls in `DefaultPlugins`.
pub(crate) type NewBoomerangs<'w, 's> =
    Query<'w, 's, (Entity, &'static PositionF), (With<Boomerang>, Without<Sprite>)>;

/// The fang's two textures, loaded once at startup and held for the run.
///
/// `ensure_boomerang_visuals` used to call `asset_server.load` for both at
/// the top of EVERY frame. That costs nothing while something else holds
/// the asset (`load` hands back the live handle), but on the Title and the
/// other menus nothing held `shadow_blob.png`: the handle dropped at the
/// end of the system, Bevy freed the texture, and the next frame loaded it
/// from scratch. A file read and PNG decode per frame on the phone; a
/// network request per frame in the browser (measured on the web Title:
/// ~26 requests a second, one per frame, for as long as the page is open).
#[derive(Resource)]
pub(crate) struct FangTextures {
    fang: Handle<Image>,
    shadow: Handle<Image>,
}

impl FromWorld for FangTextures {
    fn from_world(world: &mut World) -> Self {
        let assets = world.resource::<AssetServer>();
        Self {
            fang: assets.load("sprites/projectiles/bone_fang.png"),
            shadow: assets.load("sprites/fx/shadow_blob.png"),
        }
    }
}

pub(crate) fn ensure_boomerang_visuals(
    mut commands: Commands,
    textures: Res<FangTextures>,
    flip: Res<render::PerspectiveFlip>,
    q: NewBoomerangs,
) {
    // Phase 15: render the bone fang as the polished 12x12 sprite,
    // upscaled 2x to match the placeholder's original 20-px footprint
    // (BOOMERANG_HALF_EXTENT_CM * 2 = 20). Marked variants will land
    // when per-round blood accumulation is wired in cycle 3.
    // Larger weapon sprite (2.6× the 20 cm fang) — the fang reads as a bigger,
    // more present threat per the portrait-fighter scale-up.
    let size_px = ((BOOMERANG_HALF_EXTENT_CM * 2) as f32) * 2.6;
    let image = &textures.fang;
    let shadow_img = &textures.shadow;
    for (entity, pos) in &q {
        let (x, y) = pos.0.to_f32();
        let ty = render::tilt_y(y * flip.0);
        commands.entity(entity).insert((
            Sprite {
                image: image.clone(),
                custom_size: Some(Vec2::new(size_px, size_px)),
                ..default()
            },
            Transform::from_xyz(x, ty, 0.5),
        ));
        // A ground shadow so the fang reads as flying *over* the floor; it
        // self-cleans when the boomerang despawns (render::sync_ground_shadows).
        render::spawn_ground_shadow(
            &mut commands,
            shadow_img.clone(),
            entity,
            0.0,
            size_px * 0.7,
            Vec2::new(x, ty),
        );
    }
}

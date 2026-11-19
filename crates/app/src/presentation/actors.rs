//! The duelists on screen: the depth projection, pose squash and
//! stretch, the spawn-guard blink and the atlas frame from the sim's anim.

use crate::*;

/// Publish the perspective-table projection for this frame: the island maps
/// onto the device's fitted view height (minus a small UI reserve), so every
/// pixel row of every phone is table. Uses the AutoMin math at ortho scale 1
/// — the kill-cam zoom then magnifies the projected scene as intended
/// instead of the projection re-fitting the zoomed view.
pub(crate) fn publish_depth_projection(window: Res<WindowSize>) {
    let win = window.0;
    if win.x <= 0.0 || win.y <= 0.0 {
        return;
    }
    const VIEW_MARGIN_CM: f32 = 80.0;
    let min_width = (2 * sim::ARENA_HALF_WIDTH_CM) as f32 + 2.0 * VIEW_MARGIN_CM;
    let min_height =
        (2 * sim::ARENA_HALF_HEIGHT_CM) as f32 * render::WORLD_TILT_Y + 2.0 * VIEW_MARGIN_CM;
    let wpp = (min_width / win.x).max(min_height / win.y);
    // 8% reserve keeps the table clear of the pip row and the bottom hint.
    let span = win.y * wpp * 0.92;
    render::publish_depth_projection(span, render::DEPTH_FOCAL_DEFAULT);
}

/// Squash/stretch multipliers (x, y) for a duelist — the exaggeration layer
/// over the flat atlas poses. Cartoon rules: stretch INTO the motion axis
/// with a counter-squash on the other (roughly area-preserving), a hard
/// overstretch on dash, a coiled-wide anticipation stance while winding up a
/// throw or a taunt, and a flattened pop during hit-stop. `gait` is a free
/// phase for the run bounce. Pure for tests; render-only.
pub(crate) fn actor_pose(
    speed_frac: f32,
    horizontal: bool,
    dashing: bool,
    winding_up: bool,
    stunned: bool,
    gait: f32,
) -> Vec2 {
    if stunned {
        return Vec2::new(1.14, 0.88);
    }
    if dashing {
        return if horizontal {
            Vec2::new(1.22, 0.85)
        } else {
            Vec2::new(0.88, 1.18)
        };
    }
    if winding_up {
        return Vec2::new(1.08, 0.93);
    }
    let run = speed_frac.min(1.0);
    if run > 0.05 {
        // Stretch along the motion + a gait-synced hop that scales with speed.
        let k = 0.09 * run;
        let bounce = 1.0 + 0.05 * run * gait.sin().abs();
        return if horizontal {
            Vec2::new(1.0 + k, (1.0 - 0.6 * k) * bounce)
        } else {
            Vec2::new(1.0 - 0.6 * k, (1.0 + k) * bounce)
        };
    }
    Vec2::ONE
}

/// Scale ground actors by their table depth so the perspective read is
/// carried by the bodies, not just the row spacing: your duelist looms,
/// the far one recedes. Fangs get theirs inside `grow_boomerang_sprites`.
/// The duelists also wear [`actor_pose`]'s squash/stretch on top, so the
/// bodies telegraph what they're doing (run, dash, wind-up, hit-stop) —
/// the exaggeration composes with (never replaces) the depth scale.
#[allow(clippy::type_complexity)]
pub(crate) fn scale_actors_by_depth(
    time: Res<Time<Real>>,
    flip: Res<render::PerspectiveFlip>,
    mut players: Query<
        (
            &Player,
            &PositionF,
            &VelocityF,
            &sim::DashState,
            &sim::ThrowCharge,
            &sim::Taunt,
            &sim::StunFrames,
            &mut Sprite,
        ),
        Without<sim::Pickup>,
    >,
    mut pickups: Query<(&PositionF, &mut Sprite), (With<sim::Pickup>, Without<Player>)>,
) {
    for (player, pos, vel, dash, charge, taunt, stun, mut sprite) in &mut players {
        let (_, y) = pos.0.to_f32();
        let s = render::depth_scale(y * flip.0);
        let (vx, vy) = vel.0.to_f32();
        let speed_frac = (vx.abs().max(vy.abs())) / sim::WALK_SPEED_CM_PER_TICK as f32;
        // Per-player gait phase offset so the two never hop in sync.
        let gait = time.elapsed_secs() * 11.0 + player.handle as f32 * 1.7;
        let pose = actor_pose(
            speed_frac,
            vx.abs() >= vy.abs(),
            matches!(dash, sim::DashState::Dashing { .. }),
            charge.0 > 0 || taunt.0 > 0,
            stun.0 > 0,
            gait,
        );
        sprite.custom_size = Some(Vec2::new(
            render::PLAYER_RENDER_SIZE * s * pose.x,
            render::PLAYER_RENDER_SIZE * s * pose.y,
        ));
    }
    for (pos, mut sprite) in &mut pickups {
        let (_, y) = pos.0.to_f32();
        let s = render::depth_scale(y * flip.0);
        sprite.custom_size = Some(Vec2::splat(render::pickup_render_size() * s));
    }
}

/// Flicker a freshly-respawned duelist while its `SpawnGuard` holds — the
/// arcade "can't touch me yet" read. Both guards are public information
/// (the killer needs to know the camp won't pay), so both players blink.
/// This system owns the player sprite's alpha; nothing else writes it.
pub(crate) fn blink_spawn_guard(
    time: Res<Time<Real>>,
    mut q: Query<(&sim::SpawnGuard, &mut Sprite), With<Player>>,
) {
    for (guard, mut sprite) in &mut q {
        let alpha = if guard.0 > 0 {
            // ~7 Hz square-wave flicker: unmistakably "protected", never
            // strobe-fast (the window is only 0.75 s).
            if (time.elapsed_secs() * 7.0).fract() < 0.5 {
                0.35
            } else {
                0.9
            }
        } else {
            1.0
        };
        sprite.color.set_alpha(alpha);
    }
}

/// Phase 15 cycle 1: drive each player's TextureAtlas index from the
/// Selects the atlas frame from the rolled-back AnimState AND the facing
/// direction (side/back/front row) from the player's velocity. The 3-row
/// atlas layout: row 0 = side, row 1 = back (away), row 2 = front (toward).
///
/// Direction logic: if |vy| > |vx| the character is moving vertically —
/// positive Y = back (walking away from camera), negative Y = front (toward).
/// Otherwise side-facing with flip_x for left/right. Idle defaults: P0
/// (near/bottom) shows back (facing away toward the opponent), P1 (far/top)
/// shows front (facing toward the camera/opponent).
pub(crate) fn sync_sprite_atlas_from_anim(
    mut q: Query<(&Player, &AnimState, &VelocityF, &mut Sprite)>,
    persp: Res<render::PerspectiveFlip>,
    victory: Res<screen::VictoryPose>,
    mut facing: Local<HashMap<usize, (bool, u16)>>,
) {
    let deadzone = Fix::const_from_int(3);
    let frames_per_row = AnimState::TOTAL_ATLAS_FRAMES as usize;
    let flipped = persp.0 < 0.0;
    let (away, toward) = if flipped {
        (render::FACING_FRONT, render::FACING_BACK)
    } else {
        (render::FACING_BACK, render::FACING_FRONT)
    };
    // The decided match's winner stands as a statue: pinned on the CHARGE
    // pose, facing the camera, through the whole summary. Render-only.
    let victory_frame = AnimState {
        anim_id: AnimState::CHARGE,
        ticks: 0,
    }
    .display_index();
    for (player, anim, vel, mut sprite) in &mut q {
        if victory.0 == Some(player.handle) {
            if let Some(atlas) = sprite.texture_atlas.as_mut() {
                atlas.index =
                    (render::FACING_FRONT as usize) * frames_per_row + victory_frame as usize;
            }
            sprite.flip_x = false;
            continue;
        }
        let (flip, dir) = facing.entry(player.handle).or_insert_with(|| {
            let default_dir = if (player.handle == 0) ^ flipped {
                render::FACING_BACK
            } else {
                render::FACING_FRONT
            };
            (false, default_dir)
        });

        let ax = vel.0.x.abs();
        let ay = vel.0.y.abs();
        if ax > deadzone || ay > deadzone {
            if ay > ax {
                if vel.0.y > Fix::ZERO {
                    *dir = away;
                } else {
                    *dir = toward;
                }
                *flip = false;
            } else {
                *dir = render::FACING_SIDE;
                if vel.0.x > deadzone {
                    *flip = false;
                } else if vel.0.x < -deadzone {
                    *flip = true;
                }
            }
        }

        if let Some(atlas) = sprite.texture_atlas.as_mut() {
            atlas.index = (*dir as usize) * frames_per_row + anim.display_index() as usize;
        }
        sprite.flip_x = *flip;
    }
}

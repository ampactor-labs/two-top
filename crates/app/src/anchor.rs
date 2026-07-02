//! Screen-anchored UI: pin overlays/HUD to the *actual* visible rect.
//!
//! The camera keeps competitive fairness with `AutoMin` scaling — every
//! device is guaranteed the same minimum world view, and any extra aspect
//! only ever reveals cosmetic void beyond the arena island. That means the
//! *screen edges* land at different world coordinates on every device (and
//! move every frame under the kill-cam zoom + follow cam). Anything that
//! should live at a screen position — score pips, timer bar, title text,
//! debug telemetry, the vignette — must therefore be re-anchored from the
//! real visible rect each frame instead of being parked at arena-relative
//! world coordinates.
//!
//! [`ViewRect`] is that rect, computed once per frame from the camera
//! transform + projection (after the camera rig composes follow/kill-cam/
//! shake). [`ScreenAnchor`] pins an entity's translation to a normalized
//! screen position; [`FullScreenSprite`] stretches a sprite to cover the
//! whole view (vignette, ambience washes).
//!
//! Because the rect is derived from the *shaken* camera, anchored UI moves
//! with the camera and therefore appears rock-stable on screen — exactly
//! what screen-space UI should do while the world jolts underneath it.

use bevy::camera::ScalingMode;
use bevy::prelude::*;
use input_touch::WindowSize;

use crate::camera::CameraRigSet;

/// The world-space rectangle the camera shows this frame.
#[derive(Resource, Default, Clone, Copy)]
pub struct ViewRect {
    pub center: Vec2,
    pub half: Vec2,
}

impl ViewRect {
    /// World position for a normalized anchor. `frac` runs (-1,-1) at the
    /// bottom-left corner to (1,1) at the top-right; `offset` is a world-unit
    /// nudge from that point (so an inward margin on a positive edge is a
    /// negative offset).
    pub fn anchor_pos(&self, frac: Vec2, offset: Vec2) -> Vec2 {
        self.center + self.half * frac + offset
    }
}

/// Pins an entity's XY translation to a normalized screen position each
/// frame (Z is left alone — layering stays the spawner's choice).
#[derive(Component)]
pub struct ScreenAnchor {
    pub frac: Vec2,
    pub offset: Vec2,
}

impl ScreenAnchor {
    pub fn new(fx: f32, fy: f32, ox: f32, oy: f32) -> Self {
        Self {
            frac: Vec2::new(fx, fy),
            offset: Vec2::new(ox, oy),
        }
    }
}

/// Stretches the sprite to cover the full view every frame and parks it at
/// the view center. `cover` > 1.0 overscans (useful for washes that must
/// never show a seam at the edge mid-shake).
#[derive(Component)]
pub struct FullScreenSprite {
    pub cover: f32,
}

/// Visible world half-extents for a window + our orthographic projection.
/// Replicates `AutoMin` by hand instead of trusting `ortho.area`, because
/// Bevy only refreshes `area` on resize/projection-change events — the
/// kill-cam animates `scale` every frame and the area would lag it.
/// Pure for testing; only the window *aspect* matters, so logical vs
/// physical pixels are interchangeable.
pub fn view_half_extents(window: Vec2, ortho: &OrthographicProjection) -> Vec2 {
    if window.x <= 0.0 || window.y <= 0.0 {
        return Vec2::ZERO;
    }
    match ortho.scaling_mode {
        ScalingMode::AutoMin {
            min_width,
            min_height,
        } => {
            // World units per pixel that guarantees BOTH minimums visible.
            let wpp = (min_width / window.x).max(min_height / window.y) * ortho.scale;
            window * wpp * 0.5
        }
        // Any other scaling mode: fall back to Bevy's computed area.
        _ => ortho.area.half_size() * ortho.scale,
    }
}

fn compute_view_rect(
    window: Res<WindowSize>,
    cam: Query<(&Transform, &Projection), With<Camera2d>>,
    mut rect: ResMut<ViewRect>,
) {
    let Ok((tx, projection)) = cam.single() else {
        return;
    };
    let Projection::Orthographic(ortho) = projection else {
        return;
    };
    let half = view_half_extents(window.0, ortho);
    if half == Vec2::ZERO {
        return; // window metrics not populated yet — keep last good rect
    }
    rect.center = tx.translation.truncate();
    rect.half = half;
}

fn apply_screen_anchors(rect: Res<ViewRect>, mut q: Query<(&ScreenAnchor, &mut Transform)>) {
    if rect.half == Vec2::ZERO {
        return; // first frames before the rect exists: leave spawn positions
    }
    for (anchor, mut tx) in &mut q {
        let p = rect.anchor_pos(anchor.frac, anchor.offset);
        tx.translation.x = p.x;
        tx.translation.y = p.y;
    }
}

fn size_fullscreen_sprites(
    rect: Res<ViewRect>,
    mut q: Query<(&FullScreenSprite, &mut Sprite, &mut Transform)>,
) {
    if rect.half == Vec2::ZERO {
        return;
    }
    for (fs, mut sprite, mut tx) in &mut q {
        sprite.custom_size = Some(rect.half * 2.0 * fs.cover);
        tx.translation.x = rect.center.x;
        tx.translation.y = rect.center.y;
    }
}

pub struct ScreenAnchorPlugin;

impl Plugin for ScreenAnchorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewRect>().add_systems(
            Update,
            (
                compute_view_rect,
                (apply_screen_anchors, size_fullscreen_sprites),
            )
                .chain()
                .after(CameraRigSet),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn automin(min_width: f32, min_height: f32, scale: f32) -> OrthographicProjection {
        OrthographicProjection {
            scaling_mode: ScalingMode::AutoMin {
                min_width,
                min_height,
            },
            scale,
            ..OrthographicProjection::default_2d()
        }
    }

    #[test]
    fn view_covers_both_minimums_on_any_aspect() {
        let ortho = automin(1160.0, 1285.0, 1.0);
        for window in [
            Vec2::new(600.0, 900.0),   // desktop portrait
            Vec2::new(1080.0, 2400.0), // tall phone
            Vec2::new(1920.0, 1080.0), // landscape monitor
            Vec2::new(1160.0, 1285.0), // exact min aspect
        ] {
            let half = view_half_extents(window, &ortho);
            assert!(half.x * 2.0 >= 1160.0 - 1e-3, "width short on {window:?}");
            assert!(half.y * 2.0 >= 1285.0 - 1e-3, "height short on {window:?}");
        }
    }

    #[test]
    fn one_axis_is_exactly_the_minimum() {
        // AutoMin never over-expands: the constraining axis sits exactly at
        // its minimum, only the other axis overscans.
        let ortho = automin(1160.0, 1285.0, 1.0);
        let half = view_half_extents(Vec2::new(1080.0, 2400.0), &ortho);
        assert!((half.x * 2.0 - 1160.0).abs() < 1e-3); // tall: width pinned
        let half = view_half_extents(Vec2::new(1920.0, 1080.0), &ortho);
        assert!((half.y * 2.0 - 1285.0).abs() < 1e-3); // wide: height pinned
    }

    #[test]
    fn kill_cam_scale_shrinks_the_rect() {
        let neutral = view_half_extents(Vec2::new(600.0, 900.0), &automin(1160.0, 1285.0, 1.0));
        let zoomed = view_half_extents(Vec2::new(600.0, 900.0), &automin(1160.0, 1285.0, 0.625));
        assert!(zoomed.x < neutral.x && zoomed.y < neutral.y);
        assert!((zoomed.x / neutral.x - 0.625).abs() < 1e-4);
    }

    #[test]
    fn zero_window_yields_zero_rect() {
        let ortho = automin(1160.0, 1285.0, 1.0);
        assert_eq!(view_half_extents(Vec2::ZERO, &ortho), Vec2::ZERO);
    }

    #[test]
    fn anchor_pos_hits_corners_and_center() {
        let rect = ViewRect {
            center: Vec2::new(10.0, -20.0),
            half: Vec2::new(100.0, 200.0),
        };
        assert_eq!(rect.anchor_pos(Vec2::ZERO, Vec2::ZERO), rect.center);
        assert_eq!(
            rect.anchor_pos(Vec2::new(-1.0, 1.0), Vec2::new(5.0, -5.0)),
            Vec2::new(10.0 - 100.0 + 5.0, -20.0 + 200.0 - 5.0)
        );
    }
}

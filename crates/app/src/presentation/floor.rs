//! The arena floor: the composed texture in strips under the depth tilt,
//! and its crumble with the sudden-death ring.

use crate::*;

/// `Startup`: spawn the *persistent* scene — camera, the desktop control
/// legend, and the floor backdrop. Match entities (players, walls, arena
/// props) are spawned per-match by [`screen::spawn_match`] on entering
/// `AppScreen::InMatch`, so the title screen sits over an empty floor.
/// Marker on the persistent arena-floor backdrop so [`update_arena_floor`] can
/// swap its texture when the lobby arena picker changes [`SelectedArena`].
#[derive(Component)]
pub(crate) struct ArenaFloor;

/// Swap the backdrop texture when the lobby arena picker changes the selection,
/// so the Title screen previews the chosen arena's floor and the match starts on
/// the right one. Render-only; never touches sim.
pub(crate) fn update_arena_floor(
    selected: Res<SelectedArena>,
    asset_server: Res<AssetServer>,
    mut q: Query<&mut Sprite, With<ArenaFloor>>,
) {
    if !selected.is_changed() {
        return;
    }
    let path = render::arena::info(selected.0).floor;
    for mut sprite in &mut q {
        sprite.image = asset_server.load(path);
    }
}

/// Render mirror of the sim's SUDDEN-DEATH CRUMBLE: over the round's final
/// seconds the island's safe bounds shrink (`sim::sudden_death_factor`), so
/// the floor art shrinks with them — the world visibly falling away into the
/// void is the wordless "get to the centre" cue. Render-only; the lethal
/// bounds live in `sim::oob_death`.
pub(crate) fn crumble_arena_floor(
    state: Res<sim::MatchState>,
    frame: Res<sim::FrameCount>,
    selected: Res<SelectedArena>,
    mut q: Query<(&FloorStrip, &mut Sprite, &mut Transform), With<ArenaFloor>>,
) {
    // No-storm arenas (Pit / Vigil) never shrink — mirror the sim's rule
    // exactly or the art would lie about the lethal bounds.
    let remaining = match *state {
        sim::MatchState::InRound { expires_at_frame } if selected.0.crumbles() => {
            expires_at_frame.saturating_sub(frame.0)
        }
        _ => u32::MAX,
    };
    // Sudden-death crumble scales the island in WORLD space; the projection
    // then bends the shrunken island like everything else. The island is
    // centre-symmetric, so PerspectiveFlip never changes it.
    let factor = sim::sudden_death_factor(remaining).to_num::<f32>();
    let half_w = sim::ARENA_HALF_WIDTH_CM as f32 * factor;
    let half_h = sim::ARENA_HALF_HEIGHT_CM as f32 * factor;
    for (strip, mut sprite, mut tx) in &mut q {
        // Source row 0 is the art's top = the far (+y) court edge.
        let f0 = strip.0 as f32 / FLOOR_STRIPS as f32;
        let f1 = (strip.0 + 1) as f32 / FLOOR_STRIPS as f32;
        let (y0, y1) = (half_h - 2.0 * half_h * f0, half_h - 2.0 * half_h * f1);
        let e0 = render::tilt_y(y0);
        let e1 = render::tilt_y(y1);
        // A hair of overlap between bands hides projection seams.
        let h = (e0 - e1).abs() + 1.0;
        sprite.custom_size = Some(Vec2::new(half_w * 2.0, h));
        tx.translation.x = 0.0;
        tx.translation.y = (e0 + e1) * 0.5;
    }
}

/// Horizontal floor bands (source texture rows per strip: 480 / 20 = 24 px).
pub(crate) const FLOOR_STRIPS: u32 = 20;

/// The composed floor source art is 320×480.
pub(crate) const FLOOR_SRC_W: f32 = 320.0;

pub(crate) const FLOOR_SRC_H: f32 = 480.0;

/// Which band of the floor a strip draws (0 = the TOP source row = the far
/// court's +y edge in world space; source v grows downward while world y
/// grows upward).
#[derive(Component)]
pub(crate) struct FloorStrip(pub(crate) u32);

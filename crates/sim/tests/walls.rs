//! Phase 9 cycle 1: arena geometry sanity.
//!
//! Asserts the four boundary walls form a closed border around the
//! player spawn region and don't accidentally overlap each other in
//! ways that would make collision resolution ambiguous.

use fixed_math::Vec2F;
use sim::{
    ARENA_HALF_HEIGHT_CM, ARENA_HALF_WIDTH_CM, PLAYER_HALF_EXTENT_CM, Wall, WallKind, arena_walls,
    player_rect,
};

#[test]
fn arena_has_four_solid_walls() {
    let walls = arena_walls();
    assert_eq!(walls.len(), 4);
    for w in walls {
        assert_eq!(w.kind, WallKind::Solid);
    }
}

#[test]
fn arena_interior_origin_is_clear() {
    // (0, 0) — dead center — must be inside the playable area, not
    // overlapping any wall.
    let player = player_rect(Vec2F::ZERO);
    for w in arena_walls() {
        assert!(
            !player.overlaps(w.rect),
            "player at origin overlaps wall {:?}",
            w
        );
    }
}

#[test]
fn arena_interior_near_corners_is_clear() {
    // A player tucked into the inner corner (just inside the wall
    // by the player's half-extent) should still be clear of all
    // walls. This is the worst-case interior position.
    let dx = ARENA_HALF_WIDTH_CM - PLAYER_HALF_EXTENT_CM - 1;
    let dy = ARENA_HALF_HEIGHT_CM - PLAYER_HALF_EXTENT_CM - 1;
    for &(sx, sy) in &[(1, 1), (-1, 1), (1, -1), (-1, -1)] {
        let pos = Vec2F::from_cm(sx * dx, sy * dy);
        let player = player_rect(pos);
        for w in arena_walls() {
            assert!(
                !player.overlaps(w.rect),
                "interior corner pos ({}, {}) overlaps wall {:?}",
                sx * dx,
                sy * dy,
                w
            );
        }
    }
}

#[test]
fn point_outside_arena_overlaps_a_wall() {
    // Pick a point well outside the arena on each side; at least one
    // wall must contain that point. This is the closure check —
    // proves the boundary is sealed.
    let outside_points = [
        Vec2F::from_cm(0, ARENA_HALF_HEIGHT_CM + 25),  // above
        Vec2F::from_cm(0, -ARENA_HALF_HEIGHT_CM - 25), // below
        Vec2F::from_cm(-ARENA_HALF_WIDTH_CM - 25, 0),  // left
        Vec2F::from_cm(ARENA_HALF_WIDTH_CM + 25, 0),   // right
    ];
    for p in outside_points {
        let touched = arena_walls().iter().any(|w| w.rect.contains(p));
        assert!(touched, "outside point {:?} not covered by any wall", p);
    }
}

#[test]
fn walls_are_disjoint() {
    // No wall pair should overlap each other — they share at most an
    // edge. Strict overlap means corners are covered by exactly one
    // wall (the vertical ones extend full corner-to-corner height).
    let walls: Vec<Wall> = arena_walls().to_vec();
    for i in 0..walls.len() {
        for j in (i + 1)..walls.len() {
            assert!(
                !walls[i].rect.overlaps(walls[j].rect),
                "walls {i} and {j} overlap: {:?} vs {:?}",
                walls[i].rect,
                walls[j].rect,
            );
        }
    }
}

#[test]
fn default_player_spawn_positions_are_clear() {
    // Match the spawn positions from app::setup. A regression here
    // would mean spawning a player on top of a wall.
    let p0 = Vec2F::from_cm(-100, 60);
    let p1 = Vec2F::from_cm(100, -60);
    for pos in [p0, p1] {
        let player = player_rect(pos);
        for w in arena_walls() {
            assert!(
                !player.overlaps(w.rect),
                "player spawn {:?} overlaps wall {:?}",
                pos,
                w
            );
        }
    }
}

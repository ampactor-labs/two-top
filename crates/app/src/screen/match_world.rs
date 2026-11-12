//! Laying the table: the session a local match runs on, the duelists, the
//! arena and its cover, and rebuilding all of it for an agreed table.

use super::*;

/// Build a fresh local SyncTest session (2 local players, distinct per-handle
/// inputs, input-delay 0) — the couch-versus / dev session. Inserted on match
/// start; a fresh one each time so the rollback frame count restarts at 0.
pub(super) fn build_synctest_session() -> Session<GgrsCfg> {
    let mut sb = SessionBuilder::<GgrsCfg>::new()
        .with_num_players(2)
        .expect("with_num_players")
        .with_check_distance(2)
        .with_input_delay(0);
    for i in 0..2 {
        sb = sb.add_player(PlayerType::Local, i).expect("add_player");
    }
    Session::SyncTest(sb.start_synctest_session().expect("synctest"))
}

/// `OnEnter(InMatch)`: spawn the two duelists, the arena walls, and the
/// selected arena's props, then (couch only) install a fresh SyncTest session.
/// Spawn order is fixed so rollback entity ids are bit-identical across hosts
/// (CONVENTIONS § Determinism). Online leaves the session to `perform_swap`.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_match(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
    selected: Res<SelectedArena>,
    netplay: Res<NetplayConfig>,
    practice: Res<crate::bot::PracticeMode>,
    theater: Res<crate::theater::TheaterMode>,
    flip: Res<render::PerspectiveFlip>,
) {
    // Rollback numbering restarts with every match. bevy_ggrs hashes each
    // entity's registration index into the desync checksum and never
    // forgets one, so without this two phones with different histories
    // (one had played the bot first) disagree from frame 0 and the first
    // checksum exchange calls a healthy duel a DESYNC.
    commands.insert_resource(bevy_ggrs::RollbackOrdered::default());
    let shadow_img = spawn_duelists(&mut commands, &asset_server, &mut atlases, flip.0);
    spawn_arena(
        &mut commands,
        &asset_server,
        &mut atlases,
        &selected,
        flip.0,
        shadow_img,
    );

    // Theater: the tape drives a playback session (check_distance 0 +
    // input_delay 0 — the snapshot-scrub requirements). Couch + practice:
    // a fresh local session starts the sim from frame 0 (practice forces
    // local even on an online build — the bot supplies handle 1). Online:
    // the matchbox driver inserts the P2P session once the peer connects.
    if theater.active() {
        commands.insert_resource(crate::theater::build_playback_session());
    } else if netplay.room_url.is_none() || practice.0 {
        commands.insert_resource(build_synctest_session());
    }
}

/// The two duelists (sprite, shadow, charge ring, aim telegraph), in the
/// fixed handle order the rollback numbering depends on. Returns the
/// shadow texture for the table's cover to share.
pub(super) fn spawn_duelists(
    commands: &mut Commands,
    asset_server: &AssetServer,
    atlases: &mut Assets<TextureAtlasLayout>,
    flip: f32,
) -> Handle<Image> {
    let layout = player_atlas_layout(atlases);
    let sheets = [
        (
            0usize,
            "sprites/players/duelist_a_sheet.png",
            Vec2F::from_cm(0, -300),
        ),
        (
            1usize,
            "sprites/players/duelist_b_sheet.png",
            Vec2F::from_cm(0, 300),
        ),
    ];
    let shadow_img = asset_server.load("sprites/fx/shadow_blob.png");
    let charge_ring_img = asset_server.load("sprites/fx/charge_ring.png");
    for (handle, sheet, spawn) in sheets {
        let (sx, sy) = spawn.to_f32();
        let player = commands
            .spawn((
                MatchEntity,
                Player { handle },
                PositionF(spawn),
                PreviousPositionF(spawn),
                VelocityF(Vec2F::ZERO),
                Sprite {
                    image: asset_server.load(sheet),
                    texture_atlas: Some(TextureAtlas {
                        layout: layout.clone(),
                        index: 0,
                    }),
                    custom_size: Some(Vec2::splat(PLAYER_RENDER_SIZE)),
                    ..default()
                },
                Transform::default(),
                // 2.5D: sort the duelist by its feet so a closer player draws
                // over a farther one (and gets occluded by raised cover behind).
                render::YSorted {
                    foot_offset: PLAYER_FOOT_OFFSET,
                },
            ))
            .id();
        // A drop shadow under the feet — the cheapest "stands on the floor" cue.
        render::spawn_ground_shadow(
            commands,
            shadow_img.clone(),
            player,
            PLAYER_FOOT_OFFSET,
            PLAYER_RENDER_SIZE * 0.72,
            Vec2::new(sx, render::tilt_y(sy * flip)),
        );
        // A charge ring under the feet — hidden until they wind up a throw,
        // then it tightens + blooms toward full charge.
        let aura = render::spawn_charge_aura(
            commands,
            charge_ring_img.clone(),
            player,
            PLAYER_FOOT_OFFSET,
        );
        commands.entity(aura).insert(MatchEntity);
        // The aim telegraph — the public plant both duelists can read.
        let beam = render::spawn_aim_telegraph(commands, player, handle);
        commands.entity(beam).insert(MatchEntity);
    }
    shadow_img
}

/// The table itself: boundary walls, the arena's cover (collision body +
/// raised 2.5D composite) and its props. Split from [`spawn_match`] so an
/// online pair can lay it again once they agree on the table and know
/// which edge each one sits at ([`rebuild_arena`]).
pub(super) fn spawn_arena(
    commands: &mut Commands,
    asset_server: &AssetServer,
    atlases: &mut Assets<TextureAtlasLayout>,
    selected: &SelectedArena,
    flip: f32,
    shadow_img: Handle<Image>,
) {
    // Arena boundary walls — fixed order for bit-identical entity ids.
    for wall in arena_walls() {
        commands.spawn((MatchEntity, wall));
    }

    // Inner cover (paintball-style Obstacle blocks): the invisible collision
    // body + a raised 2.5D composite so the cover has real height. Fixed order
    // (after the boundary, before props) keeps entity ids bit-identical across
    // hosts — the visual composite spawns no rollback components, so its
    // entities never enter the rollback id space.
    for wall in arena_obstacles_for(selected.0) {
        let (min_x, min_y_raw) = wall.rect.min.to_f32();
        let (max_x, max_y_raw) = wall.rect.max.to_f32();
        // Foreshorten the footprint Y into the tabletop tilt (visual only — the
        // collision `wall` keeps its true coords). The block's RISE is a screen-
        // vertical height scaled by its base row's depth, so far cover stands
        // shorter than near cover — the same perspective the duelists obey.
        // The footprint's WIDTH stays collision-true (the table's X axis is
        // not projected), so a fang that visually clears a corner really does.
        let y0 = render::tilt_y(min_y_raw * flip);
        let y1 = render::tilt_y(max_y_raw * flip);
        let (min_y, max_y) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
        let near_row = (min_y_raw * flip).min(max_y_raw * flip);
        let rise = OBSTACLE_RISE * render::depth_scale(near_row);
        commands.spawn((MatchEntity, wall));
        spawn_obstacle_block(
            commands,
            shadow_img.clone(),
            selected.0,
            min_x,
            min_y,
            max_x,
            max_y,
            rise,
        );
    }

    // Selected arena's props (pyres / chasm / doors) — tagged `ArenaProp`.
    render::spawn_arena_props(commands, asset_server, atlases, selected, flip);
}

/// Online, after the table handshake and before the session starts: tear
/// down what match entry laid and lay it again on the agreed table, now
/// with this seat's `PerspectiveFlip` (the entry-time layout predates
/// knowing which edge we sit at). The duelists are rebuilt too and the
/// rollback numbering restarted, because entry numbered the props of
/// whichever table THIS phone had picked — the two sides must number the
/// agreed table identically. Runs on both peers, table changed or not; no
/// session exists yet, so nothing here is rolled back.
#[allow(clippy::type_complexity)]
pub(crate) fn rebuild_match(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut atlases: ResMut<Assets<TextureAtlasLayout>>,
    selected: Res<SelectedArena>,
    flip: Res<render::PerspectiveFlip>,
    old: Query<
        Entity,
        Or<(
            With<MatchEntity>,
            With<render::ArenaProp>,
            With<render::GroundShadow>,
        )>,
    >,
) {
    for entity in &old {
        commands.entity(entity).despawn();
    }
    // Applied after the despawns, before the spawns' registration hooks.
    commands.insert_resource(bevy_ggrs::RollbackOrdered::default());
    let shadow_img = spawn_duelists(&mut commands, &asset_server, &mut atlases, flip.0);
    spawn_arena(
        &mut commands,
        &asset_server,
        &mut atlases,
        &selected,
        flip.0,
        shadow_img,
    );
}

/// Spawn the raised-cover composite for one obstacle footprint: a cast shadow,
/// a void silhouette, a shaded front face, a contact/AO band, and the lit top
/// face — colored and marked per arena by its `render::arena` cover skin. Flat-color quads;
/// the *height* + outline + shadow are what sell the 2.5D read. Sorted by the
/// front (nearest) base via [`render::ground_z`] so a duelist behind the block
/// is occluded and one in front draws over it. All quads are tagged
/// `MatchEntity` for teardown and carry no rollback components.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_obstacle_block(
    commands: &mut Commands,
    shadow_img: Handle<Image>,
    arena: sim::ArenaId,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    rise: f32,
) {
    let w = max_x - min_x;
    let d = max_y - min_y;
    let cx = (min_x + max_x) * 0.5;
    let total_h = rise + d;
    let z = render::ground_z(min_y);
    let skin = render::arena::info(arena).cover;

    let mut quad = |color: Color, size: Vec2, center: Vec2, zz: f32, angle: f32| {
        commands.spawn((
            MatchEntity,
            Sprite {
                color,
                custom_size: Some(size),
                ..default()
            },
            Transform::from_xyz(center.x, center.y, zz).with_rotation(Quat::from_rotation_z(angle)),
        ));
    };
    // Void silhouette → front → contact → top → motif; explicit z orders it.
    quad(
        render::palette::VOID,
        Vec2::new(w + 6.0, total_h + 6.0),
        Vec2::new(cx, min_y + total_h * 0.5),
        z - 0.002,
        0.0,
    );
    quad(
        skin.front,
        Vec2::new(w, rise),
        Vec2::new(cx, min_y + rise * 0.5),
        z,
        0.0,
    );
    quad(
        render::palette::BRUISE_SHADOW,
        Vec2::new(w, 4.0),
        Vec2::new(cx, min_y + 2.0),
        z + 0.001,
        0.0,
    );
    let lid_c = Vec2::new(cx, min_y + rise + d * 0.5);
    quad(skin.top, Vec2::new(w, d), lid_c, z + 0.001, 0.0);
    // The motif rides just above the lid (and the flutes above the front).
    let mz = z + 0.0015;
    match skin.motif {
        render::arena::CoverMotif::Lashing => {
            let len = (w * w + d * d).sqrt() - 6.0;
            let angle = (d / w).atan();
            quad(
                render::palette::CHARCOAL_LINE,
                Vec2::new(len, 3.0),
                lid_c,
                mz,
                angle,
            );
            quad(
                render::palette::CHARCOAL_LINE,
                Vec2::new(len, 3.0),
                lid_c,
                mz,
                -angle,
            );
        }
        render::arena::CoverMotif::Flutes => {
            for side in [-1.0f32, 1.0] {
                quad(
                    render::palette::BRUISE_SHADOW,
                    Vec2::new((w / 6.0).max(2.0), rise - 4.0),
                    Vec2::new(cx + side * w * 0.25, min_y + rise * 0.5),
                    mz,
                    0.0,
                );
            }
        }
        render::arena::CoverMotif::Seam => {
            let along_x = w >= d;
            let size = if along_x {
                Vec2::new(w - 6.0, 2.0)
            } else {
                Vec2::new(2.0, d - 6.0)
            };
            quad(render::palette::DEEP_TEAL, size, lid_c, mz, 0.0);
        }
        render::arena::CoverMotif::Crack => {
            quad(
                render::palette::EMBER,
                Vec2::new(w.min(d) * 0.9, 2.0),
                lid_c,
                mz,
                0.6,
            );
        }
        render::arena::CoverMotif::PlinthRim => {
            quad(
                render::palette::BONE,
                Vec2::new(w, 2.0),
                Vec2::new(cx, min_y + rise + 1.0),
                mz,
                0.0,
            );
        }
    }

    // Cast shadow on the floor, nudged toward the bottom-right (light top-left),
    // below the ground-actor band so a duelist in front steps over it.
    let mut shadow_color = Color::WHITE;
    shadow_color.set_alpha(render::GROUND_SHADOW_ALPHA);
    commands.spawn((
        MatchEntity,
        Sprite {
            image: shadow_img,
            color: shadow_color,
            custom_size: Some(Vec2::new(w * 1.15, d * 0.9)),
            ..default()
        },
        Transform::from_xyz(cx + 8.0, min_y + d * 0.15, -0.45),
    ));
}

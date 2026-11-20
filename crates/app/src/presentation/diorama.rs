//! The Title screen's diorama.

use crate::*;

/// The title's table. Rather than describe the game in prose, the title
/// shows it: your demon stands at the near seat with your name over its
/// head, and the far seat is empty — which is exactly what the button
/// underneath offers to fix. Couch fills both seats instead (the second
/// player is sitting next to you).
///
/// Cosmetic sprites, screen-anchored into the diorama band: they are not
/// rollback entities, they never touch sim, and they exist only while the
/// title is up. The arena floor behind them (visible through the title's
/// lighter scrim) is the tabletop.
#[derive(Component, Clone, Copy, PartialEq)]
pub(crate) enum DioramaPart {
    NearDemon,
    FarDemon,
    FarSeat,
}

/// The idle atlas frame, facing the camera — a portrait pose, not the
/// back-of-the-head the near seat wears in a real match.
pub(crate) fn diorama_atlas_index() -> usize {
    let idle = AnimState {
        anim_id: AnimState::IDLE,
        ticks: 0,
    }
    .display_index() as usize;
    (render::FACING_FRONT as usize) * AnimState::TOTAL_ATLAS_FRAMES as usize + idle
}

pub(crate) fn spawn_title_diorama(
    commands: &mut Commands,
    asset_server: &AssetServer,
    atlases: &mut Assets<TextureAtlasLayout>,
) {
    let layout = render::player_atlas_layout(atlases);
    let index = diorama_atlas_index();
    // Near seat: bigger and lower, far seat smaller and higher — the same
    // depth read the perspective table gives in play.
    for (part, sheet, anchor_y, size) in [
        (
            DioramaPart::NearDemon,
            "sprites/players/duelist_a_sheet.png",
            crate::ui::anchor_y(0.42),
            render::PLAYER_RENDER_SIZE * 3.1,
        ),
        (
            DioramaPart::FarDemon,
            "sprites/players/duelist_b_sheet.png",
            crate::ui::anchor_y(0.26),
            render::PLAYER_RENDER_SIZE * 2.1,
        ),
    ] {
        commands.spawn((
            part,
            Sprite {
                image: asset_server.load(sheet),
                texture_atlas: Some(TextureAtlas {
                    layout: layout.clone(),
                    index,
                }),
                custom_size: Some(Vec2::splat(size)),
                ..default()
            },
            anchor::ScreenAnchor::new(0.0, anchor_y, 0.0, 0.0),
            // Above the menu scrim (z=100): the demons stay lit while the
            // floor behind them drops a register.
            Transform::from_xyz(0.0, 0.0, 101.0),
            Visibility::Hidden,
        ));
    }
    // The empty seat: a slow-breathing ring where the challenger will
    // stand. Nobody has to be told what an empty chair means.
    commands.spawn((
        DioramaPart::FarSeat,
        Sprite {
            image: asset_server.load("sprites/fx/charge_ring.png"),
            color: render::palette::COLD_STONE.with_alpha(0.3),
            custom_size: Some(Vec2::splat(render::PLAYER_RENDER_SIZE * 1.9)),
            ..default()
        },
        anchor::ScreenAnchor::at(0.5, 0.26),
        Transform::from_xyz(0.0, 0.0, 101.0),
        Visibility::Hidden,
    ));
}

/// Show the right seats for the mode, and keep the near demon breathing —
/// a still portrait reads as a screenshot; a bobbing one reads as someone
/// waiting for you to press the button.
pub(crate) fn update_title_diorama(
    screen: Res<State<AppScreen>>,
    netplay: Res<NetplayConfig>,
    time: Res<Time<Real>>,
    mut q: Query<(
        &DioramaPart,
        &mut Visibility,
        &mut Sprite,
        &mut anchor::ScreenAnchor,
    )>,
) {
    let on_title = *screen.get() == AppScreen::Title;
    let online = netplay.room_url.is_some();
    let t = time.elapsed_secs();
    for (part, mut vis, mut sprite, mut anchor) in &mut q {
        let show = on_title
            && match part {
                DioramaPart::NearDemon => true,
                DioramaPart::FarDemon => !online,
                DioramaPart::FarSeat => online,
            };
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !show {
            continue;
        }
        match part {
            DioramaPart::NearDemon => anchor.offset.y = 5.0 * (t * 1.6).sin(),
            DioramaPart::FarDemon => anchor.offset.y = 3.0 * (t * 1.6 + 1.1).sin(),
            DioramaPart::FarSeat => {
                let pulse = 0.22 + 0.14 * (t * 1.8).sin().abs();
                sprite.color = render::palette::COLD_STONE.with_alpha(pulse);
                anchor.offset.y = 0.0;
            }
        }
    }
}

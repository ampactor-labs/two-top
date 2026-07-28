//! The mint — the install-id's demon, made visibly yours (NORTH N5).
//!
//! Two axes, both derived from identity that already travels (your own
//! install-id, the peer's from the side channel, or a tape's header name
//! so a ghost stays consistent across viewings):
//!
//!   * **The rune**: eight atelier-baked sheet variants per side carry a
//!     quiet chest mark (rune 0 is the unmarked classic).
//!   * **The shade**: the sheet's body-shadow register (Bruise Shadow —
//!     the `D`/`o` step the generator paints both sides' deep shadows in)
//!     remaps at load to one of eight dark palette roles. The shadow you
//!     cast is yours. Team hues, Hit White, and the contact channels
//!     never move, every target is one of the sixteen palette roles, and
//!     the swap lives entirely inside the silhouette — so the flood test,
//!     the palette gate, and the readability hierarchy hold by
//!     construction. Shade 0 is the classic bruise and skips the clone.
//!
//! Composed with the sting variants this yields ~512 audible-visible
//! identities from sixteen shipped PNGs. Nothing here touches the sim,
//! the wire, or rollback: minted sheets are render-side clones cached in
//! [`MintedSheets`].

use bevy::prelude::*;
use sim::Player;

use crate::screen::AppScreen;

/// How many rune variants each side's atlas family carries (0..=7).
const RUNE_COUNT: u8 = 8;

/// The rune an install wears.
pub fn my_rune(install_id: u128) -> u8 {
    (install_id % RUNE_COUNT as u128) as u8
}

/// A stand-in opponent's rune (couch P2, the bot, an identity-less peer):
/// derived from yours but never equal to it, so the far seat's demon is
/// always visibly not-you.
pub fn foil_rune(mine: u8) -> u8 {
    (mine + 3) % RUNE_COUNT
}

/// A tape header name's rune — deterministic so a rival's ghost wears one
/// mark across every viewing.
pub fn name_rune(name: Option<&str>) -> u8 {
    let Some(name) = name else {
        return 0;
    };
    let mut h: u32 = 0;
    for b in name.bytes() {
        h = h.wrapping_mul(131).wrapping_add(b as u32);
    }
    (h % RUNE_COUNT as u32) as u8
}

/// Sheet path for a side + rune. Rune 0 keeps the classic filenames so
/// every pre-rune load site stays valid.
pub fn rune_sheet_path(handle: usize, rune: u8) -> String {
    let side = if handle == 0 { 'a' } else { 'b' };
    if rune == 0 {
        format!("sprites/players/duelist_{side}_sheet.png")
    } else {
        format!("sprites/players/duelist_{side}_v{rune}.png")
    }
}

/// How many shadow shades the mint carries (0 = the classic bruise).
const SHADE_COUNT: u8 = 8;

/// The shade an install wears — drawn from bits the rune doesn't use, so
/// the two axes vary independently.
pub fn my_shade(install_id: u128) -> u8 {
    ((install_id >> 8) % SHADE_COUNT as u128) as u8
}

/// A stand-in opponent's shade: never yours (same contract as the rune's
/// foil).
pub fn foil_shade(mine: u8) -> u8 {
    (mine + 5) % SHADE_COUNT
}

/// A tape header name's shade — a different fold than [`name_rune`] so
/// ghosts vary on both axes.
pub fn name_shade(name: Option<&str>) -> u8 {
    let Some(name) = name else {
        return 0;
    };
    let mut h: u32 = 0x9e37;
    for b in name.bytes() {
        h = h.wrapping_mul(197).wrapping_add(b as u32);
    }
    (h % SHADE_COUNT as u32) as u8
}

/// The eight shadow registers a demon can be tempered in. All dark, all
/// palette roles, none of them a team or contact read; index 0 is the
/// sheet's own Bruise Shadow (no swap at all).
pub fn shade_target(shade: u8) -> bevy::color::Color {
    use render::palette as p;
    match shade % SHADE_COUNT {
        0 => p::BRUISE_SHADOW,
        1 => p::DEEP_TEAL,
        2 => p::BLOOD_DARK,
        3 => p::CHARCOAL_LINE,
        4 => p::DEEP_ASH,
        5 => p::VOID,
        6 => p::WARM_BONE_SHADE,
        _ => p::COLD_STONE,
    }
}

/// How many kill-sting voicings the mint carries (0 = the classic hit).
pub const STING_COUNT: u8 = 8;

/// The sting an install strikes with — bits above the shade's.
pub fn my_sting(install_id: u128) -> u8 {
    ((install_id >> 16) % STING_COUNT as u128) as u8
}

/// A stand-in opponent's sting: never yours.
pub fn foil_sting(mine: u8) -> u8 {
    (mine + 7) % STING_COUNT
}

/// A tape header name's sting — its own fold, so ghosts vary on all three
/// axes.
pub fn name_sting(name: Option<&str>) -> u8 {
    let Some(name) = name else {
        return 0;
    };
    let mut h: u32 = 0x51ab;
    for b in name.bytes() {
        h = h.wrapping_mul(167).wrapping_add(b as u32);
    }
    (h % STING_COUNT as u32) as u8
}

/// The sting a handle strikes with, mirroring [`rune_for`].
pub fn sting_for(
    handle: usize,
    local_handle: usize,
    mine: u8,
    peer_sting: Option<u8>,
    theater_name: Option<Option<&str>>,
) -> u8 {
    if let Some(name) = theater_name {
        return name_sting(name);
    }
    if handle == local_handle {
        mine
    } else {
        peer_sting.unwrap_or_else(|| foil_sting(mine))
    }
}

/// The rune a handle should wear right now. Pure for tests.
pub fn rune_for(
    handle: usize,
    local_handle: usize,
    mine: u8,
    peer_rune: Option<u8>,
    theater_name: Option<Option<&str>>,
) -> u8 {
    if let Some(name) = theater_name {
        return name_rune(name);
    }
    if handle == local_handle {
        mine
    } else {
        peer_rune.unwrap_or_else(|| foil_rune(mine))
    }
}

/// The shade a handle should wear, mirroring [`rune_for`]'s derivation.
pub fn shade_for(
    handle: usize,
    local_handle: usize,
    mine: u8,
    peer_shade: Option<u8>,
    theater_name: Option<Option<&str>>,
) -> u8 {
    if let Some(name) = theater_name {
        return name_shade(name);
    }
    if handle == local_handle {
        mine
    } else {
        peer_shade.unwrap_or_else(|| foil_shade(mine))
    }
}

/// Minted sheet cache: (sheet path, shade) → the recolored clone, so a
/// rematch chain never re-clones and every seat sharing a mint shares the
/// texture.
#[derive(Resource, Default)]
struct MintedSheets(std::collections::BTreeMap<(String, u8), Handle<Image>>);

/// Clone `image` with every Bruise Shadow pixel re-tempered to the
/// shade's target role. `None` if the image has no CPU-side data (never
/// true for the PNG loader's output).
fn remint(image: &Image, shade: u8) -> Option<Image> {
    let src = render::palette::BRUISE_SHADOW.to_srgba().to_u8_array();
    let dst = shade_target(shade).to_srgba().to_u8_array();
    let mut out = image.clone();
    let data = out.data.as_mut()?;
    for px in data.chunks_exact_mut(4) {
        if px == src {
            px.copy_from_slice(&dst);
        }
    }
    Some(out)
}

/// The mint currently baked into this body's sprite sheet.
#[derive(Component)]
struct WornMint {
    rune: u8,
    shade: u8,
}

/// Keep every duelist in the right cloak. Runs each InMatch frame: cheap
/// (two entities, an integer compare) and self-correcting for every late
/// arrival — the peer's profile lands mid-handshake while the challenger
/// is still hidden, so the swap is never visible as a pop.
#[allow(clippy::too_many_arguments)]
fn sync_demon_mints(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut minted: ResMut<MintedSheets>,
    profile: Res<crate::profile::LocalProfile>,
    peer: Res<net::PeerProfile>,
    local: Res<crate::netplay::LocalPlayerHandle>,
    theater: Res<crate::theater::TheaterMode>,
    mut players: Query<(Entity, &Player, &mut Sprite, Option<&WornMint>)>,
) {
    let mine = my_rune(profile.install_id);
    let my_shade_ = my_shade(profile.install_id);
    let peer_rune = peer.0.map(|p| my_rune(p.install_id));
    let peer_shade = peer.0.map(|p| my_shade(p.install_id));
    let local_handle = local.0.unwrap_or(0);
    let theater_names = theater.active().then(|| theater.header_names());
    for (entity, player, mut sprite, worn) in &mut players {
        let theater_name = theater_names
            .as_ref()
            .map(|names| names[player.handle % 2].as_deref());
        let rune = rune_for(player.handle, local_handle, mine, peer_rune, theater_name);
        let shade = shade_for(
            player.handle,
            local_handle,
            my_shade_,
            peer_shade,
            theater_name,
        );
        if worn.map(|w| (w.rune, w.shade)) == Some((rune, shade)) {
            continue;
        }
        let path = rune_sheet_path(player.handle, rune);
        let base = asset_server.load::<Image>(path.clone());
        if shade == 0 {
            sprite.image = base;
            commands.entity(entity).insert(WornMint { rune, shade });
            continue;
        }
        let key = (path, shade);
        if let Some(handle) = minted.0.get(&key) {
            sprite.image = handle.clone();
            commands.entity(entity).insert(WornMint { rune, shade });
            continue;
        }
        // The clone needs the base's pixels; until the PNG lands the body
        // wears the classic and this system re-runs (same self-correcting
        // shape as the rune swap always had).
        let Some(source) = images.get(&base) else {
            sprite.image = base;
            continue;
        };
        let Some(recolored) = remint(source, shade) else {
            sprite.image = base;
            continue;
        };
        let handle = images.add(recolored);
        minted.0.insert(key, handle.clone());
        sprite.image = handle;
        commands.entity(entity).insert(WornMint { rune, shade });
    }
}

pub struct RunesPlugin;

impl Plugin for RunesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MintedSheets>().add_systems(
            Update,
            sync_demon_mints.run_if(in_state(AppScreen::InMatch)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn your_rune_is_yours_and_the_foil_never_matches() {
        for id in [0u128, 1, 7, 8, u128::MAX, 0x2b992ddf] {
            let mine = my_rune(id);
            assert!(mine < RUNE_COUNT);
            assert_ne!(foil_rune(mine), mine, "the far seat must not mirror you");
        }
    }

    #[test]
    fn rune_zero_keeps_the_classic_sheet_paths() {
        assert_eq!(rune_sheet_path(0, 0), "sprites/players/duelist_a_sheet.png");
        assert_eq!(rune_sheet_path(1, 0), "sprites/players/duelist_b_sheet.png");
        assert_eq!(rune_sheet_path(1, 5), "sprites/players/duelist_b_v5.png");
    }

    #[test]
    fn online_peer_wears_their_own_mark_and_a_ghost_wears_its_name() {
        // My seat wears my rune; a known peer wears theirs.
        assert_eq!(rune_for(0, 0, 4, Some(6), None), 4);
        assert_eq!(rune_for(1, 0, 4, Some(6), None), 6);
        // No identity yet: the foil, never a mirror.
        assert_eq!(rune_for(1, 0, 4, None, None), foil_rune(4));
        // A tape derives from the header name and beats everything else.
        let ghost = rune_for(1, 0, 4, Some(6), Some(Some("TAGA")));
        assert_eq!(ghost, name_rune(Some("TAGA")));
        // A nameless header side falls to the unmarked classic.
        assert_eq!(rune_for(0, 0, 4, None, Some(None)), 0);
    }

    #[test]
    fn shades_derive_like_runes_but_on_their_own_bits() {
        for id in [0u128, 1, 0xa5a5, u128::MAX, 0x2b992ddf00ff] {
            let s = my_shade(id);
            assert!(s < SHADE_COUNT);
            assert_ne!(foil_shade(s), s, "the far seat's shadow is not yours");
        }
        // The two axes move independently: ids sharing a rune need not
        // share a shade.
        assert_eq!(my_rune(0x005), my_rune(0x105));
        assert_ne!(my_shade(0x005), my_shade(0x105));
        assert_eq!(shade_for(1, 0, 3, Some(6), None), 6);
        assert_eq!(shade_for(1, 0, 3, None, None), foil_shade(3));
        assert_eq!(
            shade_for(0, 0, 3, None, Some(None)),
            0,
            "nameless ghost: classic"
        );
    }

    #[test]
    fn reminting_moves_only_the_bruise_register() {
        use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
        let bruise = render::palette::BRUISE_SHADOW.to_srgba().to_u8_array();
        let team = render::palette::P0_BLOOD.to_srgba().to_u8_array();
        let mut data = Vec::new();
        data.extend_from_slice(&bruise);
        data.extend_from_slice(&team);
        let image = Image::new(
            Extent3d {
                width: 2,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            bevy::asset::RenderAssetUsages::default(),
        );
        let minted = remint(&image, 1).expect("cpu data present");
        let out = minted.data.as_ref().unwrap();
        assert_eq!(
            &out[0..4],
            &shade_target(1).to_srgba().to_u8_array(),
            "the shadow re-tempers"
        );
        assert_eq!(&out[4..8], &team, "the team read never moves");
        // Every shade's target is a palette role distinct from the rest.
        let mut seen = std::collections::BTreeSet::new();
        for s in 0..SHADE_COUNT {
            assert!(seen.insert(shade_target(s).to_srgba().to_u8_array()));
        }
    }
}

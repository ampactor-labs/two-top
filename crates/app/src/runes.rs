//! The cloak rune — the install-id's demon, made visibly yours.
//!
//! Eight atelier-baked sheet variants per side carry a quiet chest mark
//! (rune 0 is the unmarked classic). Which mark a body wears is pure
//! cosmetics derived from identity that already travels: your own
//! install-id, the opponent's install-id from the side-channel profile,
//! or — for a tape — the header name, so a rival's tape shows the same
//! mark they wore live (names are dialed, ids aren't in the header; a
//! renamed rival changes marks, which is honest enough for a ghost).
//! Nothing here touches the sim, the wire, or the readability laws: the
//! marks live inside the cloak's silhouette by construction.

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

/// The rune currently baked into this body's sprite sheet.
#[derive(Component)]
struct WornRune(u8);

/// Keep every duelist in the right cloak. Runs each InMatch frame: cheap
/// (two entities, an integer compare) and self-correcting for every late
/// arrival — the peer's profile lands mid-handshake while the challenger
/// is still hidden, so the swap is never visible as a pop.
fn sync_demon_runes(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    profile: Res<crate::profile::LocalProfile>,
    peer: Res<net::PeerProfile>,
    local: Res<crate::netplay::LocalPlayerHandle>,
    theater: Res<crate::theater::TheaterMode>,
    mut players: Query<(Entity, &Player, &mut Sprite, Option<&WornRune>)>,
) {
    let mine = my_rune(profile.install_id);
    let peer_rune = peer.0.map(|p| my_rune(p.install_id));
    let local_handle = local.0.unwrap_or(0);
    let theater_names = theater.active().then(|| theater.header_names());
    for (entity, player, mut sprite, worn) in &mut players {
        let theater_name = theater_names
            .as_ref()
            .map(|names| names[player.handle % 2].as_deref());
        let want = rune_for(player.handle, local_handle, mine, peer_rune, theater_name);
        if worn.map(|w| w.0) != Some(want) {
            sprite.image = asset_server.load(rune_sheet_path(player.handle, want));
            commands.entity(entity).insert(WornRune(want));
        }
    }
}

pub struct RunesPlugin;

impl Plugin for RunesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            sync_demon_runes.run_if(in_state(AppScreen::InMatch)),
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
}

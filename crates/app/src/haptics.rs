//! Phase 18 Task 5.4 — Android haptics (best-effort).
//!
//! Short vibration one-shots on the punchy moments: your throw, a kill, your
//! perfect catch. Android-only — the `vibrate` call is a JNI hop to the
//! platform `Vibrator`; on every other target it compiles to a no-op, so the
//! edge-detector systems are identical across platforms and only the leaf call
//! differs.
//!
//! Like audio, this lives in `app` (the device-owning crate): a vibrator is a
//! device, exactly as the audio sink and the window are. The triggers reuse
//! the same `Local` prev-state edge pattern the audio/effect systems use.
//!
//! **Locality:** throw and perfect-catch are *your-action* feedback, so they
//! fire only for the local player ([`LocalPlayerHandle`]). A kill is a
//! round-defining impact and fires for either player. In couch/SyncTest mode
//! the local handle is `None` (both players share the device) and every
//! handle counts as local — moot anyway, since couch play is desktop, where
//! `vibrate` is a no-op.
//!
//! **Verification:** the desktop no-op path is covered by the normal gate; the
//! Android JNI path and on-device feel are operator-batched (M6 checklist),
//! consistent with the determinism matrix excluding `app` on Android by
//! design — the full `bevy_winit`/`wgpu`/`cpal` Android build is the APK
//! packaging story (SIDELOAD.md), not a CI gate.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use sim::{Boomerang, BoomerangMods, Dead, Empowered, Player};

use crate::netplay::LocalPlayerHandle;
use crate::settings::Settings;

/// Throw buzz — a light tick as the fang leaves your hand.
pub const HAPTIC_THROW_MS: i64 = 10;
/// Kill buzz — the heaviest, the one-hit-kill impact.
pub const HAPTIC_KILL_MS: i64 = 60;
/// Perfect-catch buzz — a crisp confirm of the skill beat.
pub const HAPTIC_PERFECT_CATCH_MS: i64 = 15;

/// Is `handle` a player holding this phone? Online, the one local handle.
/// Practice runs a local session too (`None`), but only one of its two
/// seats has a thumb on the screen: the bot's throws and catches used to
/// buzz the player's hand as if they were the player's own. Couch (`None`,
/// not practice): both players share the device, both buzz.
fn is_local(local: &LocalPlayerHandle, practice: bool, handle: usize) -> bool {
    if practice {
        return handle != crate::bot::BOT_HANDLE;
    }
    local.0.is_none_or(|lh| lh == handle)
}

/// Fire a `ms`-millisecond vibration one-shot on the device: the
/// platform's business (`platform::vibrate` — JNI on Android, a guarded
/// `navigator.vibrate` in the browser, nothing on desktop).
fn vibrate(ms: i64) {
    crate::platform::vibrate(ms);
}

/// Throw haptic: a local player's primary fang appearing (same edge the throw
/// SFX uses). Side-fangs (Multishot) and the remote opponent are excluded.
fn haptic_on_throw(
    booms: Query<(&Boomerang, &BoomerangMods)>,
    players: Query<&Player>,
    local: Res<LocalPlayerHandle>,
    practice: Res<crate::bot::PracticeMode>,
    settings: Res<Settings>,
    mut had_primary: Local<HashMap<usize, bool>>,
) {
    let mut present: HashMap<usize, bool> = HashMap::default();
    for (boom, mods) in &booms {
        if !mods.is_secondary {
            present.insert(boom.owner_handle, true);
        }
    }
    for player in &players {
        let handle = player.handle;
        let now = present.get(&handle).copied().unwrap_or(false);
        let was = had_primary.get(&handle).copied().unwrap_or(false);
        if now && !was && settings.haptics && is_local(&local, practice.0, handle) {
            vibrate(HAPTIC_THROW_MS);
        }
        had_primary.insert(handle, now);
    }
}

/// Kill haptic: any player entering `is_dying` (the round-defining impact).
fn haptic_on_kill(
    players: Query<(&Player, &Dead)>,
    settings: Res<Settings>,
    mut prev: Local<HashMap<usize, bool>>,
) {
    for (player, dead) in &players {
        let now = dead.is_dying();
        let was = prev.get(&player.handle).copied().unwrap_or(false);
        if now && !was && settings.haptics {
            vibrate(HAPTIC_KILL_MS);
        }
        prev.insert(player.handle, now);
    }
}

/// Perfect-catch haptic: a local player's `Empowered` flag rising (the same
/// edge the perfect-catch bell + shake use).
fn haptic_on_perfect_catch(
    players: Query<(&Player, &Empowered)>,
    local: Res<LocalPlayerHandle>,
    practice: Res<crate::bot::PracticeMode>,
    settings: Res<Settings>,
    mut prev: Local<HashMap<usize, bool>>,
) {
    for (player, emp) in &players {
        let now = emp.0;
        let was = prev.get(&player.handle).copied().unwrap_or(false);
        if now && !was && settings.haptics && is_local(&local, practice.0, player.handle) {
            vibrate(HAPTIC_PERFECT_CATCH_MS);
        }
        prev.insert(player.handle, now);
    }
}

/// Plugin: runs the three haptic edge-detectors in `Update`. Added in
/// `app::run` on every platform; the `vibrate` leaf is the only thing that's
/// Android-specific.
pub struct HapticsPlugin;

impl Plugin for HapticsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (haptic_on_throw, haptic_on_kill, haptic_on_perfect_catch),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_filter_matches_only_the_local_handle() {
        let local = LocalPlayerHandle(Some(1));
        assert!(!is_local(&local, false, 0));
        assert!(is_local(&local, false, 1));
    }

    #[test]
    fn in_practice_only_the_player_buzzes() {
        let local = LocalPlayerHandle(None);
        let bot = crate::bot::BOT_HANDLE;
        assert!(!is_local(&local, true, bot));
        assert!(is_local(&local, true, 1 - bot));
    }

    #[test]
    fn local_filter_none_treats_every_handle_as_local() {
        // Couch/SyncTest: both players share the device.
        let local = LocalPlayerHandle(None);
        assert!(is_local(&local, false, 0));
        assert!(is_local(&local, false, 1));
    }
}

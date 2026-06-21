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

use bevy::prelude::*;
use bevy::platform::collections::HashMap;

use sim::{Boomerang, BoomerangMods, Dead, Empowered, Player};

use crate::netplay::LocalPlayerHandle;

/// Throw buzz — a light tick as the fang leaves your hand.
pub const HAPTIC_THROW_MS: i64 = 10;
/// Kill buzz — the heaviest, the one-hit-kill impact.
pub const HAPTIC_KILL_MS: i64 = 60;
/// Perfect-catch buzz — a crisp confirm of the skill beat.
pub const HAPTIC_PERFECT_CATCH_MS: i64 = 15;

/// Is `handle` a local player? `None` (couch/SyncTest — both players on one
/// device) treats every handle as local.
fn is_local(local: &LocalPlayerHandle, handle: usize) -> bool {
    local.0.is_none_or(|lh| lh == handle)
}

/// Fire a `ms`-millisecond vibration one-shot on the device.
///
/// Android: hop into the JVM via the activity's `ndk_context`, fetch the
/// `Vibrator` system service, and call the (legacy, API-24-safe) `vibrate(long)`
/// overload. Best-effort — any JNI error is logged and swallowed; a missing or
/// permission-denied vibrator must never take down the game loop.
#[cfg(target_os = "android")]
fn vibrate(ms: i64) {
    use jni::objects::{JObject, JValue};
    use jni::JavaVM;

    let ctx = ndk_context::android_context();
    let vm = match unsafe { JavaVM::from_raw(ctx.vm().cast()) } {
        Ok(vm) => vm,
        Err(e) => {
            tracing::warn!(target: "two_top::haptics", error = %e, "JavaVM::from_raw failed");
            return;
        }
    };
    let mut env = match vm.attach_current_thread() {
        Ok(env) => env,
        Err(e) => {
            tracing::warn!(target: "two_top::haptics", error = %e, "attach_current_thread failed");
            return;
        }
    };
    // The activity is the JNI Context we call getSystemService on.
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    let result = (|| -> jni::errors::Result<()> {
        let service_name = env.new_string("vibrator")?;
        let vibrator = env
            .call_method(
                &activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&service_name)],
            )?
            .l()?;
        env.call_method(&vibrator, "vibrate", "(J)V", &[JValue::Long(ms)])?;
        Ok(())
    })();

    if let Err(e) = result {
        // Clear any pending Java exception so the thread's JNIEnv stays usable.
        let _ = env.exception_clear();
        tracing::warn!(target: "two_top::haptics", error = %e, "vibrate failed");
    }
}

/// Desktop / web / everything-not-Android: vibration is a no-op.
#[cfg(not(target_os = "android"))]
fn vibrate(_ms: i64) {}

/// Throw haptic: a local player's primary fang appearing (same edge the throw
/// SFX uses). Side-fangs (Multishot) and the remote opponent are excluded.
fn haptic_on_throw(
    booms: Query<(&Boomerang, &BoomerangMods)>,
    players: Query<&Player>,
    local: Res<LocalPlayerHandle>,
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
        if now && !was && is_local(&local, handle) {
            vibrate(HAPTIC_THROW_MS);
        }
        had_primary.insert(handle, now);
    }
}

/// Kill haptic: any player entering `is_dying` (the round-defining impact).
fn haptic_on_kill(
    players: Query<(&Player, &Dead)>,
    mut prev: Local<HashMap<usize, bool>>,
) {
    for (player, dead) in &players {
        let now = dead.is_dying();
        let was = prev.get(&player.handle).copied().unwrap_or(false);
        if now && !was {
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
    mut prev: Local<HashMap<usize, bool>>,
) {
    for (player, emp) in &players {
        let now = emp.0;
        let was = prev.get(&player.handle).copied().unwrap_or(false);
        if now && !was && is_local(&local, player.handle) {
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
        assert!(!is_local(&local, 0));
        assert!(is_local(&local, 1));
    }

    #[test]
    fn local_filter_none_treats_every_handle_as_local() {
        // Couch/SyncTest: both players share the device.
        let local = LocalPlayerHandle(None);
        assert!(is_local(&local, 0));
        assert!(is_local(&local, 1));
    }
}

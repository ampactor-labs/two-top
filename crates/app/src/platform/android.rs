//! Android: the process, the activity, and the one JNI seam.
//!
//! ## One process, one `android_main`
//!
//! Three facts about the stack under us, each read off its source:
//!
//! * `android-activity` runs `android_main` on its own thread and wraps it
//!   in `catch_unwind`. A panic is logged, the activity is finished, and
//!   the **process stays alive** with nothing in it.
//! * winit builds one event loop per process, ever
//!   (`EventLoopError::RecreationAttempt`), and `bevy::android::ANDROID_APP`
//!   is a `OnceLock` that keeps the first activity's handle.
//! * winit ignores `MainEvent::Destroy`, while `android-activity`'s
//!   `onDestroy` blocks the Java UI thread until `android_main` returns.
//!
//! Put together: any panic on the phone, including one taken while the app
//! was already on its way to the background and nobody was looking, left a
//! live process that could never run the game again. The next tap on the
//! icon started a second `android_main` inside it, bevy_winit's
//! `expect("Failed to build event loop")` fired, and the app closed the
//! instant it opened. That repeated on every launch until Android happened
//! to reclaim the cached process: "sometimes it crashes right after
//! opening". And an activity the system destroyed under us (a config change
//! the manifest does not absorb, "Don't keep activities") hung the UI
//! thread for good, which the user meets as a frozen launch and an ANR.
//!
//! So this module owns the entry point, and no way out of [`main`] leaves a
//! process behind:
//!
//! * a panic that unwinds out of the game aborts, here, after the hook has
//!   written `crash.log` and `run` has marked the run as ended by it:
//!   Android sees an honest crash and the next launch is a cold start. A
//!   panic something below catches (a detached task's, which the executor
//!   swallows — those run on this thread too) is logged and survived;
//! * a clean return finishes the activity and exits;
//! * a destroyed activity is noticed by a watchdog and the process exits,
//!   so the system relaunches into a fresh one;
//! * a second entry, which nothing should be able to reach any more, exits
//!   instead of panicking.
//!
//! ## JNI
//!
//! Every Java call in the app goes through [`with_activity`]. It hands out
//! the real `Activity` (`ndk_context`'s context has been the `Application`
//! since `android-activity` 0.6.1, which has no `getIntent`), runs the call
//! in a local reference frame (the main thread never returns to Java, so a
//! bare local reference lived until the process died; the oldest Android
//! we install on caps the table at 512), and never leaves a Java exception
//! pending.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bevy::android::ANDROID_APP;
use bevy::android::android_activity::{AndroidApp, WindowManagerFlags};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use jni::JNIEnv;
use jni::objects::{JObject, JString, JValue};

use crate::lifecycle::LastExit;
use crate::screen::{AppScreen, AwaitingPeer};

/// Set on the first entry, for the life of the process.
static ENTERED: AtomicBool = AtomicBool::new(false);

/// The native entry (`lib.rs` exports `android_main` and calls this).
pub fn main(android_app: AndroidApp) {
    if ENTERED.swap(true, Ordering::SeqCst) {
        // A second activity in a process that already ran the game. winit
        // cannot build a second event loop and `ANDROID_APP` still names
        // the first activity, so there is nothing to run here. Leaving
        // makes Android start this launch again in a fresh process.
        eprintln!("two-top: android_main entered twice in one process; restarting clean");
        exit_process(0);
    }
    let _ = ANDROID_APP.set(android_app);
    spawn_destroy_watchdog();

    if std::panic::catch_unwind(crate::run).is_err() {
        // Unwinding on out of `android_main` would land in
        // `android-activity`'s own catch, which finishes the activity and
        // leaves a process that can never start the game again. Abort
        // instead: the crash Android understands.
        crate::logging::shutdown();
        std::process::abort();
    }

    // The event loop ended without a panic (an `AppExit`). Close the
    // activity so the system does not bring it straight back, then go.
    tracing::info!(target: "two_top::lifecycle", "event loop ended; leaving the process");
    finish_activity();
    std::thread::sleep(Duration::from_millis(150));
    exit_process(0);
}

/// End the process now, without running static destructors: other threads
/// (audio, the logger) are still alive, and C++ teardown under them is its
/// own crash. The log file is flushed first.
pub fn exit_process(code: i32) -> ! {
    crate::logging::shutdown();
    unsafe extern "C" {
        fn _exit(status: std::ffi::c_int) -> !;
    }
    // SAFETY: bionic's `_exit` takes any status and never returns.
    unsafe { _exit(code) }
}

/// How often the watchdog asks whether the activity is still there.
const DESTROY_POLL: Duration = Duration::from_millis(250);

/// Unanswered polls after which the watchdog stops asking. A JVM that will
/// not answer is not going to start answering.
const DESTROY_POLL_GIVE_UP: u32 = 8;

/// Exit when the activity is destroyed while the event loop still runs.
///
/// winit drops `MainEvent::Destroy`, so nothing inside the loop can react
/// to it, and the Java UI thread sits in `onDestroy` waiting for
/// `android_main` to return. `Activity.isDestroyed()` turns true before
/// `onDestroy` is called, which makes it readable from here for exactly as
/// long as that wait lasts.
fn spawn_destroy_watchdog() {
    let spawned = std::thread::Builder::new()
        .name("twotop-lifecycle".into())
        .spawn(|| {
            let mut unanswered = 0;
            while unanswered < DESTROY_POLL_GIVE_UP {
                std::thread::sleep(DESTROY_POLL);
                match activity_bool("isDestroyed") {
                    Some(true) => {
                        tracing::warn!(
                            target: "two_top::lifecycle",
                            "activity destroyed under a live event loop; exiting so the next launch starts clean",
                        );
                        exit_process(0);
                    }
                    Some(false) => unanswered = 0,
                    None => unanswered += 1,
                }
            }
            tracing::warn!(target: "two_top::lifecycle", "lifecycle watchdog stood down");
        });
    if let Err(e) = spawned {
        tracing::warn!(target: "two_top::lifecycle", error = %e, "no lifecycle watchdog");
    }
}

/// Local references one [`with_activity`] call may create before the JVM
/// has to grow the frame. Every caller here stays well under it.
const LOCAL_FRAME: i32 = 32;

/// Run a JNI closure against the activity.
///
/// `None` when the JVM or the activity is unavailable, or the closure
/// failed; the failure is logged under `what` and any Java exception is
/// described to logcat and cleared. Safe from any thread: a thread that is
/// not attached yet is attached for its lifetime.
pub fn with_activity<T>(
    what: &'static str,
    f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<T>,
) -> Option<T> {
    let app = ANDROID_APP.get()?;
    // SAFETY: the process's one JavaVM, valid for as long as it runs.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }.ok()?;
    let mut env = match vm.attach_current_thread_permanently() {
        Ok(env) => env,
        Err(e) => {
            tracing::warn!(target: "two_top::android", what, error = %e, "jni attach failed");
            return None;
        }
    };
    // SAFETY: `android-activity`'s own global reference to the Activity,
    // valid until the `AndroidApp` in `ANDROID_APP` is dropped, which is
    // never. `JObject` has no destructor, so wrapping it deletes nothing.
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let result = env.with_local_frame(LOCAL_FRAME, |env| {
        let result = f(env, &activity);
        // A pending exception poisons every later JNI call on this thread
        // and kills the process if the thread ever detaches with it.
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        result
    });
    match result {
        Ok(value) => Some(value),
        Err(e) => {
            tracing::warn!(target: "two_top::android", what, error = %e, "jni call failed");
            None
        }
    }
}

/// Run a JNI closure against the activity on the Java UI thread, where
/// anything that touches the window or starts another activity belongs.
fn on_ui_thread(
    what: &'static str,
    f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<()> + Send + 'static,
) {
    let Some(app) = ANDROID_APP.get() else {
        return;
    };
    app.run_on_java_main_thread(Box::new(move || {
        with_activity(what, f);
    }));
}

/// A no-argument `boolean` method on the activity (`isDestroyed`, ...).
fn activity_bool(method: &'static str) -> Option<bool> {
    with_activity(method, |env, activity| {
        env.call_method(activity, method, "()Z", &[])?.z()
    })
}

/// A Java `String` reference as a Rust `String`; empty for `null`.
fn java_string(env: &mut JNIEnv, value: JObject) -> jni::errors::Result<String> {
    if value.is_null() {
        return Ok(String::new());
    }
    Ok(env.get_string(&JString::from(value))?.into())
}

/// `activity.finish()`.
fn finish_activity() {
    on_ui_thread("finish", |env, activity| {
        env.call_method(activity, "finish", "()V", &[])?;
        Ok(())
    });
}

/// Send the app to the background, the way the Home button would: what
/// Back means on a game's first screen.
pub fn move_task_to_back() {
    on_ui_thread("moveTaskToBack", |env, activity| {
        env.call_method(activity, "moveTaskToBack", "(Z)Z", &[JValue::from(true)])?;
        Ok(())
    });
}

/// Hold the screen on, or hand it back to the system's timeout.
pub fn keep_screen_on(on: bool) {
    let Some(app) = ANDROID_APP.get() else {
        return;
    };
    let flag = WindowManagerFlags::KEEP_SCREEN_ON;
    if on {
        app.set_window_flags(flag, WindowManagerFlags::empty());
    } else {
        app.set_window_flags(WindowManagerFlags::empty(), flag);
    }
}

/// The URI this launch was opened with, if any (`twotop://join/...`).
pub fn launch_uri() -> Option<String> {
    with_activity("launch intent", |env, activity| {
        let intent = env
            .call_method(activity, "getIntent", "()Landroid/content/Intent;", &[])?
            .l()?;
        if intent.is_null() {
            return Ok(None);
        }
        let data = env
            .call_method(&intent, "getDataString", "()Ljava/lang/String;", &[])?
            .l()?;
        if data.is_null() {
            return Ok(None);
        }
        Ok(Some(java_string(env, data)?))
    })
    .flatten()
}

/// One vibration of `ms` milliseconds. The legacy `vibrate(long)` overload
/// exists on every API level we ship to.
pub fn vibrate(ms: i64) {
    with_activity("vibrate", |env, activity| {
        let name = env.new_string("vibrator")?;
        let vibrator = env
            .call_method(
                activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        if vibrator.is_null() {
            return Ok(());
        }
        env.call_method(&vibrator, "vibrate", "(J)V", &[JValue::Long(ms)])?;
        Ok(())
    });
}

/// "samsung SM-A165F, Android 14 (sdk 34)". Empty parts are left out.
pub fn device_summary() -> String {
    let sdk = AndroidApp::sdk_version();
    let names = with_activity("device", |env, _| {
        let mut read = |class: &str, field: &str| -> jni::errors::Result<String> {
            let value = env
                .get_static_field(class, field, "Ljava/lang/String;")?
                .l()?;
            java_string(env, value)
        };
        Ok((
            read("android/os/Build", "MANUFACTURER")?,
            read("android/os/Build", "MODEL")?,
            read("android/os/Build$VERSION", "RELEASE")?,
        ))
    });
    match names {
        Some((maker, model, release)) => {
            format!("{maker} {model}, Android {release} (sdk {sdk})")
        }
        None => format!("Android (sdk {sdk})"),
    }
}

/// The most recent exit record for this app. Android 11+ only; older
/// phones have no such record and answer `None`.
pub fn last_exit() -> Option<LastExit> {
    if AndroidApp::sdk_version() < 30 {
        return None;
    }
    with_activity("exit reasons", |env, activity| {
        let name = env.new_string("activity")?;
        let manager = env
            .call_method(
                activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        if manager.is_null() {
            return Ok(None);
        }
        // (null, 0, 1): any process of ours, any pid, the newest record.
        let records = env
            .call_method(
                &manager,
                "getHistoricalProcessExitReasons",
                "(Ljava/lang/String;II)Ljava/util/List;",
                &[
                    JValue::Object(&JObject::null()),
                    JValue::Int(0),
                    JValue::Int(1),
                ],
            )?
            .l()?;
        if records.is_null() || env.call_method(&records, "size", "()I", &[])?.i()? < 1 {
            return Ok(None);
        }
        let info = env
            .call_method(&records, "get", "(I)Ljava/lang/Object;", &[JValue::Int(0)])?
            .l()?;
        if info.is_null() {
            return Ok(None);
        }
        let reason = env.call_method(&info, "getReason", "()I", &[])?.i()?;
        let status = env.call_method(&info, "getStatus", "()I", &[])?.i()?;
        let timestamp_ms = env.call_method(&info, "getTimestamp", "()J", &[])?.j()?;
        let description = env
            .call_method(&info, "getDescription", "()Ljava/lang/String;", &[])?
            .l()?;
        let description = java_string(env, description)?;
        Ok(Some(LastExit {
            reason,
            status,
            timestamp_ms,
            description,
        }))
    })
    .flatten()
}

/// Open the system share sheet on a piece of text.
pub fn share_text(title: &str, text: &str) {
    let (title, text) = (title.to_owned(), text.to_owned());
    on_ui_thread("share", move |env, activity| {
        let intent_class = env.find_class("android/content/Intent")?;
        let action = env.new_string("android.intent.action.SEND")?;
        let intent = env.new_object(
            &intent_class,
            "(Ljava/lang/String;)V",
            &[JValue::Object(&action)],
        )?;
        let mime = env.new_string("text/plain")?;
        env.call_method(
            &intent,
            "setType",
            "(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&mime)],
        )?;
        for (key, value) in [
            ("android.intent.extra.SUBJECT", &title),
            ("android.intent.extra.TEXT", &text),
        ] {
            let key = env.new_string(key)?;
            let value = env.new_string(value)?;
            env.call_method(
                &intent,
                "putExtra",
                "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
                &[JValue::Object(&key), JValue::Object(&value)],
            )?;
        }
        let chooser_title = env.new_string(&title)?;
        let chooser = env
            .call_static_method(
                &intent_class,
                "createChooser",
                "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;",
                &[JValue::Object(&intent), JValue::Object(&chooser_title)],
            )?
            .l()?;
        env.call_method(
            activity,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&chooser)],
        )?;
        Ok(())
    });
}

// ---- The sit-down ritual's warm start: a join link on the clipboard ----

/// The prefix the join page writes to the clipboard. Only this exact form
/// is ever acted on — anything else on the clipboard is not ours to read.
const JOIN_SCHEME: &str = "twotop://join/";

/// How long after the join page writes the clipboard its link still
/// counts as meant for us. The tap that writes it is the tap that brings
/// 2-Top to the front, so this only has to cover a slow phone.
const CLIP_FRESH_MS: i64 = 2 * 60 * 1000;

/// Whether the primary clip is plain text set within [`CLIP_FRESH_MS`],
/// judged from `getPrimaryClipDescription` alone — the clipboard read
/// Android 12+ does not announce with a toast. A description with no
/// usable timestamp (before API 26, or a vendor that reports 0) counts as
/// fresh so the link still works there; those phones pay the toast they
/// already paid before this check existed.
fn clip_is_fresh_text(
    env: &mut jni::JNIEnv,
    cm: &jni::objects::JObject,
) -> jni::errors::Result<bool> {
    use jni::objects::JValue;
    let desc = env
        .call_method(
            cm,
            "getPrimaryClipDescription",
            "()Landroid/content/ClipDescription;",
            &[],
        )?
        .l()?;
    if desc.is_null() {
        return Ok(false);
    }
    let text = env.new_string("text/*")?;
    let is_text = env
        .call_method(
            &desc,
            "hasMimeType",
            "(Ljava/lang/String;)Z",
            &[JValue::Object(&text)],
        )?
        .z()?;
    if !is_text {
        return Ok(false);
    }
    // getTimestamp is API 26+ and min_sdk is 24: a missing method is an
    // unknown age, not a stale clip.
    let set_at = match env.call_method(&desc, "getTimestamp", "()J", &[]) {
        Ok(v) => v.j()?,
        Err(_) => {
            let _ = env.exception_clear();
            0
        }
    };
    if set_at <= 0 {
        return Ok(true);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(set_at);
    Ok(now - set_at <= CLIP_FRESH_MS)
}

/// Android, warm start: the join page's OPEN IN 2-TOP button also copies
/// its `twotop://join/...` link to the clipboard, because a running app
/// never sees the new intent (see [`launch_uri`]). When 2-Top regains
/// focus on the Title, a join link on the clipboard is claimed — applied
/// to the dial, then wiped so it fires once. Nothing else on the
/// clipboard is ever kept or logged.
pub fn clipboard_join_link() -> Option<String> {
    use jni::objects::{JObject, JValue};
    with_activity("clipboard", |env, activity| {
        let name = env.new_string("clipboard")?;
        let cm = env
            .call_method(
                activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        if cm.is_null() || !env.call_method(&cm, "hasPrimaryClip", "()Z", &[])?.z()? {
            return Ok(None);
        }
        // Android 12+ toasts "2-Top pasted from your clipboard" on every
        // getPrimaryClip, and this runs on every return to the Title. The
        // clip's DESCRIPTION is the one read that toast does not cover, so
        // it decides first: only fresh plain text (the join page writes it
        // on the very tap that brings 2-Top forward) is worth reading.
        // Anything older, or of another kind, is someone else's clipboard
        // and stays unread.
        if !clip_is_fresh_text(env, &cm)? {
            return Ok(None);
        }
        let clip = env
            .call_method(&cm, "getPrimaryClip", "()Landroid/content/ClipData;", &[])?
            .l()?;
        if clip.is_null() || env.call_method(&clip, "getItemCount", "()I", &[])?.i()? < 1 {
            return Ok(None);
        }
        let item = env
            .call_method(
                &clip,
                "getItemAt",
                "(I)Landroid/content/ClipData$Item;",
                &[JValue::Int(0)],
            )?
            .l()?;
        if item.is_null() {
            return Ok(None);
        }
        let text = env
            .call_method(&item, "getText", "()Ljava/lang/CharSequence;", &[])?
            .l()?;
        if text.is_null() {
            return Ok(None);
        }
        let text = env
            .call_method(&text, "toString", "()Ljava/lang/String;", &[])?
            .l()?;
        let text: String = env.get_string(&jni::objects::JString::from(text))?.into();
        if !text.trim().starts_with(JOIN_SCHEME) {
            return Ok(None);
        }
        // Claimed: wipe it so the next focus doesn't re-apply it. API 28+
        // has clearPrimaryClip; older phones get an empty clip instead.
        if env
            .call_method(&cm, "clearPrimaryClip", "()V", &[])
            .is_err()
        {
            let _ = env.exception_clear();
            let empty = env.new_string("")?;
            let label = JObject::from(env.new_string("")?);
            let blank = env
                .call_static_method(
                    "android/content/ClipData",
                    "newPlainText",
                    "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                    &[
                        JValue::Object(&label),
                        JValue::Object(&JObject::from(empty)),
                    ],
                )?
                .l()?;
            env.call_method(
                &cm,
                "setPrimaryClip",
                "(Landroid/content/ClipData;)V",
                &[JValue::Object(&blank)],
            )?;
        }
        Ok(Some(text))
    })
    .flatten()
}

/// Android's Back, as the key every screen already reads as "back".
///
/// winit reports Back as a handled key, so the system never acts on it and
/// the button did nothing at all. Every screen treats Escape as "back"
/// (leave the tape, close settings, leave the summary), so Back is pressed
/// into that key and each screen's own handler does the rest.
///
/// Except during live play (`crate::lifecycle::back_reaches_the_game`).
/// On gesture navigation, the default on a Pixel, a thumb that drags the
/// move stick off the left edge IS the system Back gesture; mid-round
/// Escape arms QUIT, and a second stray swipe would quit a ranked match.
/// The round's own QUIT button is the way out of a live match.
fn back_is_escape(
    mut events: MessageReader<KeyboardInput>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    screen: Res<State<AppScreen>>,
    state: Res<sim::MatchState>,
    awaiting: Res<AwaitingPeer>,
    theater: Res<crate::theater::TheaterMode>,
) {
    let reaches = crate::lifecycle::back_reaches_the_game(
        *screen.get() == AppScreen::InMatch,
        awaiting.0 || theater.active(),
        matches!(*state, sim::MatchState::MatchOver),
    );
    for event in events.read() {
        if event.key_code != KeyCode::BrowserBack && event.logical_key != Key::BrowserBack {
            continue;
        }
        match event.state {
            ButtonState::Pressed if reaches => keys.press(KeyCode::Escape),
            ButtonState::Pressed => {}
            ButtonState::Released => keys.release(KeyCode::Escape),
        }
    }
}

/// Back on the title leaves the app, as it does in every other one.
fn back_on_title_goes_home(screen: Res<State<AppScreen>>, keys: Res<ButtonInput<KeyCode>>) {
    if *screen.get() == AppScreen::Title && keys.just_pressed(KeyCode::Escape) {
        move_task_to_back();
    }
}

/// The Android-only systems: the Back button. (The screen-on policy is
/// shared with the browser and lives in `crate::lifecycle`.)
pub struct AndroidPlugin;

impl Plugin for AndroidPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, back_is_escape.after(bevy::input::InputSystems))
            .add_systems(Update, back_on_title_goes_home);
    }
}

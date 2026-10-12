//! How you choose your opponent: a stranger, or the person next to you.
//!
//! Online, the title carries a two-state toggle — QUICK MATCH or PRIVATE —
//! sitting directly on top of the button it changes. QUICK MATCH is the
//! public room (the base `--room`/`MATCHBOX_ROOM`/`TWOTOP_ROOM` URL, where
//! strangers pair on `?next=2`) and the button below reads FIND OPPONENT.
//! PRIVATE unfolds a four-glyph dial and the button relabels itself to
//! `DUEL AT C-U-R-S`: both phones dial the same code, both press, and only
//! those two ever meet. 7⁴ = 2401 rooms without a keyboard, an account, or
//! a camera.
//!
//! The dial keeps the 7-glyph CURSTAG wheel (tap-cycling is cheap at seven
//! and a friend's code should be four quick taps away) — unlike the NAME,
//! which is A-Z on a grid because a name has to be worth reading.
//!
//! Desktop (dev): keys 1-4 cycle the slots, 0 back to QUICK MATCH.
//!
//! The chosen code persists (`room_code.json` beside settings) and is
//! applied by rewriting `NetplayConfig.room_url` — `start_matchbox` reads
//! that resource at match entry, so no connect-path changes are needed.

use bevy::prelude::*;
use input_touch::WindowSize;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::anchor::ScreenAnchor;
use crate::netplay::NetplayConfig;
use crate::screen::{
    AppScreen, BtnRole, DIAL_ANCHOR_Y, DIAL_RECT, MODE_TOGGLE_ANCHOR_Y, MODE_TOGGLE_RECT,
    spawn_button_part,
};

/// The glyph wheel each slot cycles through — the duelists' own letters.
pub const CODE_ALPHABET: [char; 7] = ['C', 'U', 'R', 'S', 'T', 'A', 'G'];
pub const CODE_LEN: usize = 4;

/// "CURS" → "C-U-R-S": the primary button's code reads as something you
/// dial, not as a word.
pub fn code_dashed(code: &str) -> String {
    code.chars().map(String::from).collect::<Vec<_>>().join("-")
}

/// The dial's four cells, centered under the toggle (window-x fractions).
const DIAL_LEFT: f32 = 0.18;
const CELL_W: f32 = 0.16;

#[derive(Resource, Serialize, Deserialize, Clone, Debug)]
pub struct RoomCode {
    pub slots: [u8; CODE_LEN],
    pub custom: bool,
    /// The quickmatch room URL the code decorates (captured at boot).
    #[serde(skip)]
    base_url: Option<String>,
}

impl Default for RoomCode {
    fn default() -> Self {
        Self {
            slots: [0; CODE_LEN],
            custom: false,
            base_url: None,
        }
    }
}

impl RoomCode {
    pub fn code_string(&self) -> String {
        self.slots
            .iter()
            .map(|&i| CODE_ALPHABET[i as usize % CODE_ALPHABET.len()])
            .collect()
    }

    /// The room URL this code selects. The table is NOT part of the room
    /// name: two people who dial the same code (or both tap FIND
    /// OPPONENT) meet whatever table each last looked at, and agree on
    /// one after pairing (`netplay`'s table handshake — the lower peer
    /// id's pick wins). It used to ride the name, which split a phone on
    /// the Pit and a fresh install on the Anchor into two rooms that
    /// never saw each other: "we both tapped play and nothing happened."
    /// The sim version still rides the name: two APKs sideloaded a week
    /// apart on different `SIM_VERSION`s land in different rooms instead
    /// of pairing, diverging, and each recording its own winner.
    pub fn room_url(&self) -> Option<String> {
        let base = self.base_url.as_ref()?;
        let code = self.custom.then(|| self.code_string());
        Some(room_url_with_parts(base, code.as_deref(), sim::SIM_VERSION))
    }
}

/// The arena's join-link token (`join.html#CURS-pit`). Lowercase so it
/// reads as one clean path segment.
pub fn arena_room_tag(arena: sim::ArenaId) -> &'static str {
    match arena {
        sim::ArenaId::Anchor => "anchor",
        sim::ArenaId::Crossing => "crossing",
        sim::ArenaId::Reliquary => "reliquary",
        sim::ArenaId::Pit => "pit",
        sim::ArenaId::Vigil => "vigil",
        sim::ArenaId::Gallery => "gallery",
        sim::ArenaId::Forest => "forest",
    }
}

/// Append the (optional) code and the sim version to the room *name*,
/// preserving any query string: `ws://host/two-top?next=2` + `CURS` +
/// `15` → `ws://host/two-top-CURS-v15?next=2`. Pure for testing. The
/// version segment is what keeps mismatched builds apart — the live wire
/// has no other version check (the handshake carries identity and keys
/// only), and a desync between two builds is otherwise a log line nobody
/// reads.
pub fn room_url_with_parts(base: &str, code: Option<&str>, sim_version: u32) -> String {
    let suffix = match code {
        Some(c) => format!("-{c}-v{sim_version}"),
        None => format!("-v{sim_version}"),
    };
    match base.split_once('?') {
        Some((path, query)) => format!("{path}{suffix}?{query}"),
        None => format!("{base}{suffix}"),
    }
}

/// Parse a join reference — `twotop://join/CURS-pit`, a join page's
/// `#CURS-pit` fragment, or the bare `CURS-pit` — into dial slots plus
/// the arena. `None` for anything that isn't exactly a code and a known
/// table: a QR is typed by nobody, so there is no fuzziness to forgive.
// Desktop has no launch link to read; the parser still runs in tests.
#[cfg_attr(
    not(any(target_os = "android", target_family = "wasm")),
    allow(dead_code)
)]
pub fn parse_join(uri: &str) -> Option<([u8; CODE_LEN], sim::ArenaId)> {
    let tail = uri.trim().rsplit(['/', '#']).next()?;
    let (code, tag) = tail.split_once('-')?;
    if code.chars().count() != CODE_LEN {
        return None;
    }
    let mut slots = [0u8; CODE_LEN];
    for (i, ch) in code.chars().enumerate() {
        slots[i] = CODE_ALPHABET
            .iter()
            .position(|c| c.eq_ignore_ascii_case(&ch))? as u8;
    }
    let tag = tag.to_ascii_lowercase();
    let arena = sim::ALL_ARENAS
        .iter()
        .copied()
        .find(|a| arena_room_tag(*a) == tag)?;
    Some((slots, arena))
}

/// The join link a dialed code shares: the web join page carries the code
/// for humans and the `twotop://` button for installed phones.
pub fn join_link(watch_base: &str, code: &str, arena: sim::ArenaId) -> String {
    format!(
        "{}/join.html#{}-{}",
        watch_base.trim_end_matches('/'),
        code,
        arena_room_tag(arena),
    )
}

/// Android: the URI this launch was opened with, if any — the deep-link
/// half of the sit-down ritual (`twotop://join/...` from the join page's
/// button). One JNI hop: activity.getIntent().getDataString().
///
/// Only a COLD start reaches us this way. The activity is `singleTask`
/// and NativeActivity has no `onNewIntent`, so a link tapped while 2-Top
/// is already running just brings it forward with the old intent — see
/// [`claim_clipboard_join`] for that case.
#[cfg(target_os = "android")]
fn launch_uri() -> Option<String> {
    with_activity(|env, activity| {
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
        Ok(Some(
            env.get_string(&jni::objects::JString::from(data))?.into(),
        ))
    })
}

/// Run a JNI closure against the activity, clearing any pending Java
/// exception on failure so the thread's JNIEnv stays usable (a pending
/// exception aborts the process on the next JNI call).
#[cfg(target_os = "android")]
fn with_activity<T>(
    f: impl FnOnce(&mut jni::JNIEnv, &jni::objects::JObject) -> jni::errors::Result<Option<T>>,
) -> Option<T> {
    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { jni::objects::JObject::from_raw(ctx.context().cast()) };
    match f(&mut env, &activity) {
        Ok(v) => v,
        Err(e) => {
            let _ = env.exception_clear();
            tracing::warn!(target: "two_top::room_code", error = %e, "jni call failed");
            None
        }
    }
}

/// The prefix the join page writes to the clipboard. Only this exact form
/// is ever acted on — anything else on the clipboard is not ours to read.
#[cfg(target_os = "android")]
const JOIN_SCHEME: &str = "twotop://join/";

/// How long after the join page writes the clipboard its link still
/// counts as meant for us. The tap that writes it is the tap that brings
/// 2-Top to the front, so this only has to cover a slow phone.
#[cfg(target_os = "android")]
const CLIP_FRESH_MS: i64 = 2 * 60 * 1000;

/// Whether the primary clip is plain text set within [`CLIP_FRESH_MS`],
/// judged from `getPrimaryClipDescription` alone — the clipboard read
/// Android 12+ does not announce with a toast. A description with no
/// usable timestamp (before API 26, or a vendor that reports 0) counts as
/// fresh so the link still works there; those phones pay the toast they
/// already paid before this check existed.
#[cfg(target_os = "android")]
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
#[cfg(target_os = "android")]
fn clipboard_join_link() -> Option<String> {
    use jni::objects::{JObject, JValue};
    with_activity(|env, activity| {
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
}

/// Point the dial and the table at a join link. `false` (and nothing
/// changed) for anything that isn't one.
#[cfg(any(target_os = "android", target_family = "wasm"))]
fn apply_join(
    uri: &str,
    code: &mut RoomCode,
    selected: &mut sim::SelectedArena,
    settings: &mut crate::settings::Settings,
) -> bool {
    let Some((slots, arena)) = parse_join(uri) else {
        tracing::info!(target: "two_top::room_code", "launch link is not a join link");
        return false;
    };
    code.slots = slots;
    code.custom = true;
    selected.0 = arena;
    settings.arena = arena.as_u8();
    crate::settings::persist(settings);
    save_room_code(code);
    tracing::info!(
        target: "two_top::room_code",
        code = %code.code_string(),
        arena = arena_room_tag(arena),
        "join link accepted — the table is set",
    );
    true
}

/// A scanned join link lands both phones at the same table with zero
/// typing — parse the launch link into the dial and the arena pick. Runs
/// once in PostStartup (after the persisted arena restore, so the link's
/// pick wins the boot it arrived on). Android reads the launch intent;
/// the browser reads the page's `#join=` fragment (`web::join_link`).
#[cfg(any(target_os = "android", target_family = "wasm"))]
fn apply_launch_join(
    mut code: ResMut<RoomCode>,
    mut selected: ResMut<sim::SelectedArena>,
    mut settings: ResMut<crate::settings::Settings>,
) {
    #[cfg(target_os = "android")]
    let uri = launch_uri();
    #[cfg(target_family = "wasm")]
    let uri = crate::web::join_link();
    if let Some(uri) = uri {
        apply_join(&uri, &mut code, &mut selected, &mut settings);
    }
}

/// Android warm start: claim a join link from the clipboard when 2-Top
/// regains focus on the Title (see [`clipboard_join_link`]).
#[cfg(target_os = "android")]
fn claim_clipboard_join(
    mut focus: MessageReader<bevy::window::WindowFocused>,
    screen: Res<State<AppScreen>>,
    mut code: ResMut<RoomCode>,
    mut selected: ResMut<sim::SelectedArena>,
    mut settings: ResMut<crate::settings::Settings>,
) {
    // Drain every focus message this frame (not `any`, which would stop
    // early and leave the rest to re-trigger next frame).
    let mut gained = false;
    for f in focus.read() {
        gained |= f.focused;
    }
    if !gained || *screen.get() != AppScreen::Title {
        return;
    }
    if let Some(uri) = clipboard_join_link() {
        apply_join(&uri, &mut code, &mut selected, &mut settings);
    }
}

fn room_code_path() -> Option<PathBuf> {
    crate::paths::config_file("room_code.json")
}

fn load_room_code() -> RoomCode {
    let Some(path) = room_code_path() else {
        return RoomCode::default();
    };
    let mut code: RoomCode = crate::paths::read_document(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    for slot in &mut code.slots {
        *slot %= CODE_ALPHABET.len() as u8;
    }
    code
}

fn save_room_code(code: &RoomCode) {
    let Some(path) = room_code_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(code)
        && let Err(e) = crate::paths::write_atomic(&path, json.as_bytes())
    {
        tracing::warn!(target: "two_top::room_code", error = %e, "failed to save room code");
    }
}

/// One glyph slot of the private dial (0..CODE_LEN).
#[derive(Component)]
struct CodeCell(usize);

/// One piece of a mode pill. `private` picks which of the two.
#[derive(Component)]
struct ModePill {
    private: bool,
    role: BtnRole,
}

fn spawn_pad(mut commands: Commands, netplay: Res<NetplayConfig>) {
    // Couch builds have no room to dial.
    if netplay.room_url.is_none() {
        return;
    }
    // The toggle: two pills, selected one filled. Same bordered-box
    // language as every other button on the screen.
    // Anchors sized so the two boxes sit side by side with a real gap: a
    // pill is 480 wide plus its 22 of border on a ~1160-unit-wide screen,
    // so anything tighter than ±0.46 merges them into one box.
    for (private, anchor_x) in [(false, -0.47), (true, 0.47)] {
        for role in [BtnRole::Border, BtnRole::Fill, BtnRole::Label] {
            spawn_button_part(
                &mut commands,
                role,
                Vec2::new(anchor_x, MODE_TOGGLE_ANCHOR_Y),
                Vec2::new(480.0, 76.0),
                28.0,
                &mut |ec, role| {
                    ec.insert(ModePill { private, role });
                },
            );
        }
    }
    // The dial, hidden until PRIVATE is selected.
    for cell in 0..CODE_LEN {
        let fx = DIAL_LEFT + (cell as f32 + 0.5) * CELL_W;
        commands.spawn((
            CodeCell(cell),
            Text2d::new(String::new()),
            TextFont {
                font_size: 54.0,
                ..default()
            },
            TextColor(render::palette::P1_CYAN),
            ScreenAnchor::new(fx * 2.0 - 1.0, DIAL_ANCHOR_Y, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 200.0),
            Visibility::Hidden,
        ));
    }
}

/// Boot: capture the quickmatch base URL. Composition happens in
/// [`sync_room_url`].
fn init_room_code(mut code: ResMut<RoomCode>, netplay: Res<NetplayConfig>) {
    code.base_url = netplay.room_url.clone();
}

/// Recompose the live room URL whenever the dialed code changes.
fn sync_room_url(code: Res<RoomCode>, mut netplay: ResMut<NetplayConfig>) {
    if code.base_url.is_none() || !code.is_changed() {
        return;
    }
    netplay.room_url = code.room_url();
}

/// Title-screen taps/keys on the pad. Any change rewrites the live
/// `NetplayConfig.room_url` and persists the code.
fn room_code_input(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    window: Res<WindowSize>,
    mut code: ResMut<RoomCode>,
) {
    if code.base_url.is_none() {
        return;
    }
    let mut changed = false;

    let win = window.0;
    if win.x > 0.0 && win.y > 0.0 {
        for t in touches.iter_just_pressed() {
            let p = t.position();
            let (fx, fy) = (p.x / win.x, p.y / win.y);
            if (MODE_TOGGLE_RECT.0..MODE_TOGGLE_RECT.1).contains(&fy) {
                // Left pill quick, right pill private — the halves are the
                // pills, so a fat-fingered tap still lands on a real mode.
                code.custom = fx >= 0.5;
                changed = true;
            } else if code.custom && (DIAL_RECT.0..DIAL_RECT.1).contains(&fy) && fx >= DIAL_LEFT {
                let cell = ((fx - DIAL_LEFT) / CELL_W) as usize;
                if cell < CODE_LEN {
                    let slot = &mut code.slots[cell];
                    *slot = (*slot + 1) % CODE_ALPHABET.len() as u8;
                    changed = true;
                }
            }
        }
    }
    // Desktop dev path: 0 back to QUICK, 1-4 cycle a slot (and arm PRIVATE).
    if keys.just_pressed(KeyCode::Digit0) {
        code.custom = false;
        changed = true;
    }
    for (key, cell) in [
        (KeyCode::Digit1, 0usize),
        (KeyCode::Digit2, 1),
        (KeyCode::Digit3, 2),
        (KeyCode::Digit4, 3),
    ] {
        if keys.just_pressed(key) {
            code.custom = true;
            let slot = &mut code.slots[cell];
            *slot = (*slot + 1) % CODE_ALPHABET.len() as u8;
            changed = true;
        }
    }

    if !changed {
        return;
    }
    // `sync_room_url` sees the change and recomposes the live URL.
    save_room_code(&code);
}

/// Show the toggle on the online title, and the dial only once PRIVATE is
/// selected: the row is simply empty in QUICK MATCH, so nothing on screen
/// asks a stranger to think about room codes.
#[allow(clippy::type_complexity)]
fn update_pad(
    screen: Res<State<AppScreen>>,
    netplay: Res<NetplayConfig>,
    code: Res<RoomCode>,
    mut cells: Query<(&CodeCell, &mut Text2d, &mut Visibility), Without<ModePill>>,
    mut pills: Query<
        (
            &ModePill,
            &mut Visibility,
            Option<&mut Sprite>,
            Option<&mut Text2d>,
            Option<&mut TextColor>,
        ),
        Without<CodeCell>,
    >,
) {
    let show = *screen.get() == AppScreen::Title && netplay.room_url.is_some();
    let glyphs = code.code_string();
    for (cell, mut text, mut vis) in &mut cells {
        *vis = if show && code.custom {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        text.0 = glyphs
            .chars()
            .nth(cell.0)
            .map(String::from)
            .unwrap_or_default();
    }
    for (pill, mut vis, sprite, text, color) in &mut pills {
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !show {
            continue;
        }
        let selected = pill.private == code.custom;
        match pill.role {
            BtnRole::Border => {
                if let Some(mut s) = sprite {
                    s.color =
                        render::palette::HOT_BONE.with_alpha(if selected { 1.0 } else { 0.45 });
                }
            }
            BtnRole::Fill => {
                if let Some(mut s) = sprite {
                    s.color = if selected {
                        render::palette::HOT_BONE
                    } else {
                        render::palette::DEEP_ASH
                    };
                }
            }
            BtnRole::Label => {
                if let Some(mut t) = text {
                    t.0 = if pill.private {
                        "PRIVATE".to_string()
                    } else {
                        "QUICK MATCH".to_string()
                    };
                }
                if let Some(mut c) = color {
                    c.0 = if selected {
                        render::palette::VOID
                    } else {
                        render::palette::HOT_BONE.with_alpha(0.7)
                    };
                }
            }
        }
    }
}

/// The dial's join QR: the sit-down ritual's display half. While PRIVATE
/// is selected (and a watch host is baked), the code's join link renders
/// as a QR beside the dial — the other phone's system camera opens the
/// join page, whose OPEN IN 2-TOP button deep-links the app straight to
/// this code and table. Reuses the share module's renderer.
/// The join QR's side, in world units. A code this size lands near ten
/// pixels per module on a 1080-wide phone, which is what a second phone's
/// camera needs to lock on across a table.
const QR_SIDE: f32 = 330.0;
/// The caption under the code. The ritual asks a stranger to point their
/// camera at a square of noise; it should say so.
const QR_CAPTION: &str = "SCAN TO JOIN";

#[derive(Component)]
struct JoinQr {
    /// The link currently rendered, so the image only re-mints on change.
    link: Option<String>,
    handle: Option<Handle<Image>>,
}

/// The one-line instruction under the code.
#[derive(Component)]
struct JoinQrCaption;

#[allow(clippy::too_many_arguments)]
fn update_join_qr(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    share: Res<crate::share::ShareConfig>,
    netplay: Res<NetplayConfig>,
    code: Res<RoomCode>,
    selected: Res<sim::SelectedArena>,
    screen: Res<State<AppScreen>>,
    mut q: Query<(&mut JoinQr, &mut Sprite, &mut Visibility)>,
    mut caption: Query<&mut Visibility, (With<JoinQrCaption>, Without<JoinQr>)>,
) {
    let Some(watch) = share.watch_url.as_deref() else {
        return;
    };
    if netplay.room_url.is_none() {
        return;
    }
    if q.is_empty() {
        commands.spawn((
            JoinQr {
                link: None,
                handle: None,
            },
            // custom_size, never Transform::scale: apply_screen_anchors
            // overwrites anchored scale every frame with the view scale
            // (that is how text stays screen-constant), which silently
            // shrank the first three placements of this code to a chip.
            // 110 was about 100 px on a 1080-wide phone: roughly three
            // pixels per module, which another phone's camera could not
            // hold focus on. The right-hand backdrop has room for three
            // times that — the portrait's left edge is the only neighbour,
            // and QR_SIDE keeps clear of it at every aspect.
            Sprite {
                custom_size: Some(Vec2::splat(QR_SIDE)),
                ..default()
            },
            // Upper-right arena space: the one region of the Title that is
            // guaranteed dead backdrop at every aspect (first placement sat
            // inside the QUICK MATCH pill, unscannable at 2.4x — the
            // capture harness caught it).
            ScreenAnchor::new(0.55, 0.30, 0.0, 0.0),
            Transform::from_xyz(0.0, 0.0, 204.0),
            Visibility::Hidden,
        ));
        commands.spawn((
            JoinQrCaption,
            Text2d::new(QR_CAPTION.to_string()),
            TextFont {
                font_size: 26.0,
                ..default()
            },
            TextColor(render::palette::BONE),
            TextLayout {
                justify: Justify::Center,
                linebreak: bevy::text::LineBreak::NoWrap,
            },
            ScreenAnchor::new(0.55, 0.30, 0.0, -(QR_SIDE * 0.5 + 34.0)),
            Transform::from_xyz(0.0, 0.0, 204.0),
            Visibility::Hidden,
        ));
        return;
    }
    let show = *screen.get() == AppScreen::Title && code.custom;
    for mut vis in &mut caption {
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    for (mut qr, mut sprite, mut vis) in &mut q {
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if !show {
            continue;
        }
        let link = join_link(watch, &code.code_string(), selected.0);
        if qr.link.as_deref() == Some(link.as_str()) {
            continue;
        }
        let Some((image, _side)) = crate::share::qr_image(&link) else {
            continue;
        };
        if let Some(old) = qr.handle.take() {
            images.remove(&old);
        }
        let handle = images.add(image);
        sprite.image = handle.clone();
        qr.link = Some(link);
        qr.handle = Some(handle);
    }
}

pub struct RoomCodePlugin;

impl Plugin for RoomCodePlugin {
    fn build(&self, app: &mut App) {
        // The scanned join link lands after the persisted picks restore,
        // so the boot it arrived on is the boot it configures.
        #[cfg(any(target_os = "android", target_family = "wasm"))]
        app.add_systems(PostStartup, apply_launch_join);
        #[cfg(target_os = "android")]
        app.add_systems(Update, claim_clipboard_join);
        app.insert_resource(load_room_code())
            .add_systems(Startup, (init_room_code, spawn_pad))
            .add_systems(Update, update_join_qr)
            .add_systems(
                Update,
                (
                    room_code_input.run_if(in_state(AppScreen::Title)),
                    // After the input so a dial tap recomposes the same frame.
                    sync_room_url,
                    update_pad,
                )
                    .chain(),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_links_round_trip_through_the_parser() {
        let link = join_link("https://ampactor.dev/two-top/", "CURS", sim::ArenaId::Pit);
        assert_eq!(link, "https://ampactor.dev/two-top/join.html#CURS-pit");
        let (slots, arena) = parse_join(&link).expect("own link parses");
        assert_eq!(arena, sim::ArenaId::Pit);
        let code: String = slots.iter().map(|&i| CODE_ALPHABET[i as usize]).collect();
        assert_eq!(code, "CURS");
        // The deep-link form and the bare form parse identically.
        assert_eq!(parse_join("twotop://join/curs-pit"), parse_join("CURS-PIT"));
        // Garbage is refused, never guessed at.
        assert_eq!(parse_join("twotop://join/CURS-atlantis"), None);
        assert_eq!(parse_join("twotop://join/CURSX-pit"), None);
        assert_eq!(
            parse_join("twotop://join/CXRS-pit"),
            None,
            "X is not on the wheel"
        );
        assert_eq!(parse_join(""), None);
    }

    #[test]
    fn code_suffixes_the_room_name_not_the_query() {
        assert_eq!(
            room_url_with_parts("ws://h:3536/two-top?next=2", Some("CURS"), 7),
            "ws://h:3536/two-top-CURS-v7?next=2"
        );
        assert_eq!(
            room_url_with_parts("ws://h/two-top", None, 7),
            "ws://h/two-top-v7"
        );
    }

    #[test]
    fn different_sim_versions_never_share_a_room() {
        // The whole point: a stale sideload and a fresh one, same code,
        // must land in two rooms rather than one desync.
        let a = room_url_with_parts("ws://h/two-top?next=2", Some("CURS"), 13);
        let b = room_url_with_parts("ws://h/two-top?next=2", Some("CURS"), 14);
        assert_ne!(a, b);
        assert!(a.ends_with("-v13?next=2"));
        assert!(b.ends_with("-v14?next=2"));
    }

    #[test]
    fn code_string_wraps_indices_into_the_alphabet() {
        let code = RoomCode {
            slots: [0, 1, 6, 7], // 7 wraps back to 'C'
            custom: true,
            base_url: None,
        };
        assert_eq!(code.code_string(), "CUGC");
    }

    #[test]
    fn quick_room_is_one_room_whatever_the_table() {
        // Regression: the arena tag used to ride the room name, so two
        // phones on different tables tapped FIND OPPONENT into two rooms
        // and never met. The quick room is the base plus the version.
        let code = RoomCode {
            slots: [1, 2, 3, 4],
            custom: false,
            base_url: Some("ws://h/two-top?next=2".into()),
        };
        assert_eq!(
            code.room_url(),
            Some(format!("ws://h/two-top-v{}?next=2", sim::SIM_VERSION))
        );
    }

    #[test]
    fn custom_room_carries_the_code_and_no_table() {
        let code = RoomCode {
            slots: [0, 1, 2, 3],
            custom: true,
            base_url: Some("ws://h/two-top?next=2".into()),
        };
        assert_eq!(
            code.room_url(),
            Some(format!("ws://h/two-top-CURS-v{}?next=2", sim::SIM_VERSION))
        );
    }

    #[test]
    fn every_arena_tag_is_a_clean_path_token() {
        for &a in sim::ALL_ARENAS.iter() {
            let tag = arena_room_tag(a);
            assert!(!tag.is_empty());
            assert!(
                tag.chars().all(|c| c.is_ascii_lowercase()),
                "tag {tag:?} must stay a lowercase path segment"
            );
        }
    }
}

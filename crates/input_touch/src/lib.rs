//! Local touch input. Per CONVENTIONS, `TouchState` is never rolled back
//! and never serialized — it lives in PreUpdate and is consumed each
//! frame by GGRS's ReadInputs schedule via `read_local_inputs` (cycle 5)
//! to produce the wire-format `PlayerInput`.
//!
//! Phase 8 cycles to date:
//!   * cycle 1: raw touch ingest (`TouchState`, `update_touch_state`).
//!   * cycle 2: floating virtual stick + radial deadzone, output as a
//!     normalized [-1, 1]^2 vector in `TouchState.stick`. Quantization
//!     to wire-format i8 happens at the boundary in cycle 5's
//!     `read_local_inputs` system. The stick's anchor FOLLOWS a thumb
//!     that runs past the ring's edge (`STICK_FOLLOW_RADIUS_PX`), so a
//!     long swipe tows the stick instead of pinning it to the landing.
//!   * cycle 3: throw interaction state machine. Any right-half touch
//!     is the throw BUTTON (`throw_held`); while it's held, the LEFT
//!     stick aims — `aim_active` tracks its deflection and
//!     `aim_angle_rad` its heading, mirroring the desktop throw-key +
//!     d-pad model. Tap-vs-hold detection itself is left to sim's
//!     `released_within(THROW_DOWN, …)` edge derivation (cycle 6).
//!   * cycle 4: mouse-drag desktop fallback. Left mouse button is
//!     synthesized into a single virtual touch with sentinel id
//!     `MOUSE_TOUCH_ID`, merged into the touch event stream so the
//!     same stick/throw logic drives it. Lets developers and CI
//!     exercise the input layer without a touchscreen.
//!   * cycle 5: quantization to wire-format `PlayerInput` and the
//!     `TouchInputsPlugin` that wires `read_local_touch_inputs` into
//!     GGRS's `ReadInputs` schedule. This is the moment touch state
//!     becomes a 4-byte deterministic input that flows into sim.

use bevy::input::mouse::MouseButton;
use bevy::input::touch::Touches;
use bevy::prelude::*;
use bevy_ggrs::prelude::ReadInputs;
use bevy_ggrs::{LocalInputs, LocalPlayers};
use sim::{GgrsCfg, PlayerInput};

// ---- Cycle 2 tunables ----

/// Inside this normalized magnitude, the stick collapses to (0, 0).
/// Eats jitter from the user's resting thumb.
pub const STICK_DEADZONE_INNER: f32 = 0.12;

/// At or beyond this normalized magnitude, the stick saturates at
/// magnitude 1.0. 75% means the user only has to drag 75% of the way
/// to the edge of the virtual stick's circle to feel "max push".
pub const STICK_DEADZONE_SATURATION: f32 = 0.75;

/// Radius in logical pixels at which the floating stick is fully
/// extended. ~80px is standard for mobile thumb sticks; smaller feels
/// twitchy, larger forces the user to stretch.
pub const STICK_MAX_RADIUS_PX: f32 = 80.0;

/// The follow radius: how far the thumb may sit from the stick's anchor
/// before the anchor gets dragged along behind it. This is the saturation
/// circle — the ring the control layer actually draws — so the base starts
/// following at the exact moment the thumb collides with its visible edge.
/// Deflection therefore never exceeds saturation, which keeps a followed
/// stick pinned at full push, and reversing direction responds immediately
/// instead of asking the thumb to swim back across a dead 2×radius span.
pub const STICK_FOLLOW_RADIUS_PX: f32 = STICK_MAX_RADIUS_PX * STICK_DEADZONE_SATURATION;

/// Window size in logical pixels. Populated by the app each frame
/// (cycle 5 wires it up from `Single<&Window>`); kept as a resource so
/// `input_touch` doesn't need `bevy_window` as a hard dep.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct WindowSize(pub Vec2);

/// Cursor position in logical pixels, for the desktop mouse-drag
/// fallback. Same population pattern as `WindowSize`. ZERO when the
/// cursor is outside the window or the app hasn't populated it yet.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct CursorPosition(pub Vec2);

/// Runtime-adjustable inner deadzone for the virtual stick (Phase 18 Task
/// 5.5c). Defaults to [`STICK_DEADZONE_INNER`]; the app overwrites it from the
/// persisted `Settings` (the deadzone feeds the stick *before* quantization to
/// the wire format — a legal pre-wire input change, never post-wire). Read by
/// [`update_virtual_stick`].
#[derive(Resource, Debug, Clone, Copy)]
pub struct StickDeadzone(pub f32);

impl Default for StickDeadzone {
    fn default() -> Self {
        Self(STICK_DEADZONE_INNER)
    }
}

/// Southpaw layout: mirror every touch ZONE left-for-right — move stick on
/// the right half, throw on the left, dash in the bottom-LEFT corner. Set
/// from the persisted `Settings` like the deadzone. Zone tests are the only
/// thing mirrored; drag math is untouched, and like every input-shaping
/// setting this acts strictly pre-wire (the peer never knows).
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct Southpaw(pub bool);

/// Reflect a position's X across the window's vertical centerline —
/// the southpaw transform applied to zone probes.
pub fn mirror_x(pos: Vec2, window: Vec2) -> Vec2 {
    Vec2::new(window.x - pos.x, pos.y)
}

/// Sentinel id for the synthesized left-mouse-button touch. `u64::MAX`
/// is well outside any real touchscreen id space (Bevy/Android emit
/// small monotonic ids), so it cannot collide with a real touch.
pub const MOUSE_TOUCH_ID: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackedTouch {
    pub id: u64,
    pub start_pos: Vec2,
    pub current_pos: Vec2,
    /// Where the virtual stick is anchored NOW. Starts at `start_pos` and
    /// gets dragged along whenever the thumb pulls more than
    /// [`STICK_FOLLOW_RADIUS_PX`] away — the stick base follows the thumb
    /// across the screen instead of pinning the input to the landing spot.
    /// Zone tests keep probing the immutable `start_pos`, so a dragged
    /// anchor can never re-classify the touch (e.g. a move stick crossing
    /// the centerline never becomes a throw candidate).
    pub anchor_pos: Vec2,
    pub start_frame: u64,
}

#[derive(Resource, Default, Debug, Clone)]
pub struct TouchState {
    pub touches: Vec<TrackedTouch>,
    pub frame: u64,
    /// Resolved virtual stick output in [-1.0, 1.0]^2. Magnitude up to
    /// 1.0; sign encodes direction. `None` when no stick touch is
    /// currently active.
    pub stick: Option<Vec2>,
    /// Id of the active virtual-stick touch, if any. Sticky: once a
    /// touch becomes the stick, we keep tracking it even if the finger
    /// drags out of the left half.
    pub stick_touch: Option<u64>,
    /// Id of the active throw/aim touch, if any. Sticky in the same way
    /// as `stick_touch`: once a touch starts in the right half, we keep
    /// tracking it even if it drags elsewhere.
    pub right_touch: Option<u64>,
    /// True iff a right-side touch is currently held. Read by sim each
    /// frame (level signal) — edges are derived in sim by diffing
    /// against `PreviousInputs`.
    pub throw_held: bool,
    /// True while the throw button is held AND the LEFT stick is
    /// deflected — the move stick aims the throw (same model as the
    /// desktop path: throw key + d-pad heading). Drops back to false if
    /// the stick re-centers, so releasing then is a plain tap-throw.
    pub aim_active: bool,
    /// Game-space (y-up) heading of the aiming left stick, in radians,
    /// when `aim_active`. Held at 0.0 otherwise.
    pub aim_angle_rad: f32,
    /// The left stick's deadzone-curved vector in bevy y-down space
    /// (direction = throw heading, magnitude in [0,1] = throw power),
    /// snapshotted while `aim_active`. `quantize_inputs` writes this into
    /// the wire stick so sim throws along it. `None` when not aiming.
    /// Kept separate from `stick` so the one sticky-release frame can
    /// retain the aim even if both thumbs lift at once.
    pub aim_vec: Option<Vec2>,
    /// One-frame latch: when an *aimed* throw touch releases, the aim is
    /// held for exactly one more frame (with `throw_held` cleared, so the
    /// THROW_DOWN release edge fires) so sim sees the aim direction/power
    /// on the frame it spawns the boomerang. This is the "sticky-on-release
    /// frame" the input model promises.
    pub aim_release_sticky: bool,
    /// Id of the active dash touch, if any. Sticky like `stick_touch`.
    pub dash_touch: Option<u64>,
    /// True iff a dash-zone touch is currently held.
    pub dash_held: bool,
    /// Id of the active taunt touch, if any. Sticky like `stick_touch`.
    pub taunt_touch: Option<u64>,
    /// True iff a taunt-strip touch is currently held.
    pub taunt_held: bool,
}

impl TouchState {
    pub fn find(&self, id: u64) -> Option<&TrackedTouch> {
        self.touches.iter().find(|t| t.id == id)
    }
}

/// Map a magnitude in [0, +∞) through the radial deadzone curve.
/// Returns the smoothed magnitude in [0, 1]:
///   - input <= `inner` → 0.0 (deadzone)
///   - input >= `saturation` → 1.0 (clamped)
///   - linear ramp in between
pub fn apply_radial_deadzone(magnitude: f32, inner: f32, saturation: f32) -> f32 {
    if magnitude <= inner {
        0.0
    } else if magnitude >= saturation {
        1.0
    } else {
        (magnitude - inner) / (saturation - inner)
    }
}

/// Resolve an (anchor, current) drag into a stick vector. Output
/// magnitude is in [0, 1] after the deadzone curve; direction is the
/// drag's. (0, 0) means "stick centered" (drag too small or zero).
pub fn compute_stick(
    start: Vec2,
    current: Vec2,
    max_radius_px: f32,
    inner: f32,
    saturation: f32,
) -> Vec2 {
    let delta = current - start;
    let mag_px = delta.length();
    if mag_px == 0.0 {
        return Vec2::ZERO;
    }
    let mag_norm = (mag_px / max_radius_px).clamp(0.0, 1.0);
    let smoothed = apply_radial_deadzone(mag_norm, inner, saturation);
    delta * (smoothed / mag_px)
}

/// Fraction of the window height (from the top) that is the TAUNT strip.
/// Both thumbs live at the bottom, so a top-of-screen tap is always a
/// deliberate reach — exactly the ergonomics a taunt deserves. The strip
/// is carved OUT of the move/throw zones so it can never steal a stick.
pub const TAUNT_ZONE_Y_FRAC: f32 = 0.24;

/// Fraction of the window width where the QUIT corner begins (mirrors the
/// dash corner's X so the two reserved corners rhyme).
pub const QUIT_ZONE_X_FRAC: f32 = 0.78;

/// Fraction of the window height where the QUIT corner ends.
pub const QUIT_ZONE_Y_FRAC: f32 = 0.16;

/// The QUIT corner: a small rectangle in the top-right, reserved for the
/// app's in-match QUIT button. Carved out of the taunt strip (and dead to
/// every other zone), so a tap there reaches the app's button handler
/// without flexing a taunt on the way through. Southpaw mirrors it to the
/// top-left along with every other zone probe.
pub fn is_quit_zone(pos: Vec2, window: Vec2) -> bool {
    window.x > 0.0
        && pos.x >= window.x * QUIT_ZONE_X_FRAC
        && pos.y < window.y * QUIT_ZONE_Y_FRAC
}

/// The TAUNT strip: the top of the screen, full width minus the QUIT corner.
pub fn is_taunt_zone(pos: Vec2, window: Vec2) -> bool {
    window.x > 0.0
        && pos.y < window.y * TAUNT_ZONE_Y_FRAC
        && !is_quit_zone(pos, window)
}

/// Bevy's window coords are top-left origin, y-down. The match screen is
/// split down the CENTER: the whole left half owns the floating MOVE
/// stick — wherever the thumb lands, that's where the stick spawns.
/// The taunt strip at the top is excluded.
pub fn is_move_zone(pos: Vec2, window: Vec2) -> bool {
    window.x > 0.0 && pos.x < window.x * 0.5 && !is_taunt_zone(pos, window)
}

/// Fraction of the window width where the DASH corner begins. The corner
/// was 0.78/0.86 originally; the 2026-07-16 pass doubled its AREA (each
/// span × √2) — dash is the one control that must never be missed blind.
pub const DASH_ZONE_X_FRAC: f32 = 0.69;

/// Fraction of the window height where the DASH corner begins.
pub const DASH_ZONE_Y_FRAC: f32 = 0.80;

/// The DASH button: a fixed thumb-sized rectangle in the BOTTOM-RIGHT
/// corner. The only fixed control on the screen — both sticks float, so
/// dash needs a home the right thumb can find without looking.
pub fn is_dash_zone(pos: Vec2, window: Vec2) -> bool {
    window.x > 0.0
        && pos.x >= window.x * DASH_ZONE_X_FRAC
        && pos.y >= window.y * DASH_ZONE_Y_FRAC
}

/// The throw/aim zone: the whole right half, MINUS the dash corner, the
/// taunt strip, and the QUIT corner. Floats like the move stick — hold to
/// charge, drag to aim, wherever the thumb lands.
pub fn is_throw_zone(pos: Vec2, window: Vec2) -> bool {
    window.x > 0.0
        && pos.x >= window.x * 0.5
        && !is_dash_zone(pos, window)
        && !is_taunt_zone(pos, window)
        && !is_quit_zone(pos, window)
}

/// A touch's zone-probe position: the raw start position, or its mirror
/// for the southpaw layout.
#[inline]
fn zone_probe(t: &TrackedTouch, window: Vec2, southpaw: bool) -> Vec2 {
    if southpaw {
        mirror_x(t.start_pos, window)
    } else {
        t.start_pos
    }
}

/// Pick the touch driving the virtual stick. Sticky: if the
/// `current` choice is still in the active list, keep it. Otherwise
/// promote the first touch that STARTED in the move zone (the left
/// half; the right for southpaw). Returns `None` if no candidate
/// exists.
pub fn select_stick_touch(
    current: Option<u64>,
    touches: &[TrackedTouch],
    window: Vec2,
    southpaw: bool,
) -> Option<u64> {
    if let Some(id) = current
        && touches.iter().any(|t| t.id == id)
    {
        return Some(id);
    }
    touches
        .iter()
        .find(|t| is_move_zone(zone_probe(t, window, southpaw), window))
        .map(|t| t.id)
}

/// Pick the touch driving throw/aim. Same sticky pattern as
/// `select_stick_touch`: keep the current id while it's still active,
/// else promote the first touch that started in the throw zone.
pub fn select_throw_touch(
    current: Option<u64>,
    touches: &[TrackedTouch],
    window: Vec2,
    southpaw: bool,
) -> Option<u64> {
    if let Some(id) = current
        && touches.iter().any(|t| t.id == id)
    {
        return Some(id);
    }
    touches
        .iter()
        .find(|t| is_throw_zone(zone_probe(t, window, southpaw), window))
        .map(|t| t.id)
}

/// Pick the touch driving taunt. Same sticky pattern as stick/throw.
/// (The taunt strip is full-width, so southpaw changes nothing here —
/// the probe mirror is applied for uniformity.)
pub fn select_taunt_touch(
    current: Option<u64>,
    touches: &[TrackedTouch],
    window: Vec2,
    southpaw: bool,
) -> Option<u64> {
    if let Some(id) = current
        && touches.iter().any(|t| t.id == id)
    {
        return Some(id);
    }
    touches
        .iter()
        .find(|t| is_taunt_zone(zone_probe(t, window, southpaw), window))
        .map(|t| t.id)
}

/// Pick the touch driving dash. Same sticky pattern as stick/throw.
pub fn select_dash_touch(
    current: Option<u64>,
    touches: &[TrackedTouch],
    window: Vec2,
    southpaw: bool,
) -> Option<u64> {
    if let Some(id) = current
        && touches.iter().any(|t| t.id == id)
    {
        return Some(id);
    }
    touches
        .iter()
        .find(|t| is_dash_zone(zone_probe(t, window, southpaw), window))
        .map(|t| t.id)
}

/// Game-space (y-up) heading of a bevy y-down stick vector. Zero
/// vectors return 0.0 instead of NaN-ing through atan2.
pub fn stick_aim_angle(stick: Vec2) -> f32 {
    if stick == Vec2::ZERO {
        0.0
    } else {
        (-stick.y).atan2(stick.x)
    }
}

/// Pure transition over `TouchState`. Bevy-free so tests don't need an
/// app fixture; the Bevy system below is a thin shim that pulls
/// iterators out of `Res<Touches>` and forwards them here.
pub fn apply_touch_events(
    state: &mut TouchState,
    frame: u64,
    just_pressed: impl IntoIterator<Item = (u64, Vec2)>,
    just_released: impl IntoIterator<Item = u64>,
    just_canceled: impl IntoIterator<Item = u64>,
    active: impl IntoIterator<Item = (u64, Vec2)>,
) {
    state.frame = frame;

    let mut dropped: Vec<u64> = just_released.into_iter().collect();
    dropped.extend(just_canceled);
    state.touches.retain(|t| !dropped.contains(&t.id));

    for (id, pos) in just_pressed {
        // Bevy reuses ids after a touch ends; treat reuse as a brand-new
        // finger by dropping any stale entry first.
        state.touches.retain(|tracked| tracked.id != id);
        state.touches.push(TrackedTouch {
            id,
            start_pos: pos,
            current_pos: pos,
            anchor_pos: pos,
            start_frame: frame,
        });
    }

    for (id, pos) in active {
        if let Some(tracked) = state.touches.iter_mut().find(|tracked| tracked.id == id) {
            tracked.current_pos = pos;
        }
    }

    // Anchor follow: once the thumb is past the follow radius, the anchor
    // trails it at exactly that radius — the base ring is "dragged along by
    // the edge the thumb collided with". Applied to every touch uniformly;
    // only the stick math consumes `anchor_pos`.
    for tracked in &mut state.touches {
        let delta = tracked.current_pos - tracked.anchor_pos;
        let len = delta.length();
        if len > STICK_FOLLOW_RADIUS_PX {
            tracked.anchor_pos = tracked.current_pos - delta * (STICK_FOLLOW_RADIUS_PX / len);
        }
    }
}

/// PreUpdate system: syncs `TouchState` against Bevy's `Touches` each
/// frame, plus a synthesized virtual touch driven by the left mouse
/// button (cycle 4 desktop fallback). Frame counter advances even when
/// no touches are active so future cycles can measure
/// elapsed-frames-since-touch-start without interleaved gaps.
pub fn update_touch_state(
    mut state: ResMut<TouchState>,
    bevy_touches: Res<Touches>,
    mouse_btn: Res<ButtonInput<MouseButton>>,
    cursor: Res<CursorPosition>,
) {
    let frame = state.frame.wrapping_add(1);

    let cursor_pos = cursor.0;
    let mouse_just_pressed = mouse_btn.just_pressed(MouseButton::Left);
    let mouse_just_released = mouse_btn.just_released(MouseButton::Left);
    let mouse_held = mouse_btn.pressed(MouseButton::Left);

    let mouse_pressed_iter = mouse_just_pressed
        .then_some((MOUSE_TOUCH_ID, cursor_pos))
        .into_iter();
    let mouse_released_iter = mouse_just_released.then_some(MOUSE_TOUCH_ID).into_iter();
    // `pressed()` is false on the just_released frame, so this naturally
    // excludes the release frame from active updates.
    let mouse_active_iter = mouse_held
        .then_some((MOUSE_TOUCH_ID, cursor_pos))
        .into_iter();

    apply_touch_events(
        &mut state,
        frame,
        bevy_touches
            .iter_just_pressed()
            .map(|t| (t.id(), t.position()))
            .chain(mouse_pressed_iter),
        bevy_touches
            .iter_just_released()
            .map(|t| t.id())
            .chain(mouse_released_iter),
        bevy_touches.iter_just_canceled().map(|t| t.id()),
        bevy_touches
            .iter()
            .map(|t| (t.id(), t.position()))
            .chain(mouse_active_iter),
    );
}

/// PreUpdate system: resolves the virtual stick after `update_touch_state`
/// has synced the touch list. Reads `WindowSize` to gate the left half;
/// the app populates `WindowSize` each frame from its primary `Window`.
/// With a zero-sized window (initial frame, no window yet) `is_move_zone`
/// answers false everywhere and no stick is selected, which is the right
/// behavior — we'd rather wait one frame than guess.
pub fn update_virtual_stick(
    mut state: ResMut<TouchState>,
    window: Res<WindowSize>,
    deadzone: Res<StickDeadzone>,
    southpaw: Res<Southpaw>,
) {
    let window_size = window.0;
    state.stick_touch =
        select_stick_touch(state.stick_touch, &state.touches, window_size, southpaw.0);
    let inner = deadzone.0;
    state.stick = state.stick_touch.and_then(|id| {
        state.find(id).map(|t| {
            compute_stick(
                t.anchor_pos,
                t.current_pos,
                STICK_MAX_RADIUS_PX,
                inner,
                STICK_DEADZONE_SATURATION,
            )
        })
    });
}

/// Pure transition for dash + throw/aim state. The move stick aims while
/// the throw button (any throw-zone touch) is held: `state.stick` must
/// already be resolved for this frame — the plugin chains
/// `update_virtual_stick` first. Bevy-free so tests can drive it directly;
/// [`update_throw_state`] is the system shim.
pub fn apply_throw_state(state: &mut TouchState, window: Vec2, southpaw: bool) {
    state.dash_touch = select_dash_touch(state.dash_touch, &state.touches, window, southpaw);
    state.dash_held = state.dash_touch.is_some();

    state.taunt_touch = select_taunt_touch(state.taunt_touch, &state.touches, window, southpaw);
    state.taunt_held = state.taunt_touch.is_some();

    let prev_throw = state.right_touch;
    let new_throw = select_throw_touch(prev_throw, &state.touches, window, southpaw);
    state.right_touch = new_throw;

    match new_throw {
        Some(_) => {
            // Throw button down: charging. The deadzone-curved left stick
            // is the aim — direction is the throw heading, magnitude the
            // power. A centered stick means no aim, so releasing then is a
            // tap-throw along the facing direction, exactly like the
            // desktop d-pad path. Movement isn't lost: sim roots the
            // character while charging, so the left thumb is free to aim.
            let aim = state.stick.filter(|v| *v != Vec2::ZERO);
            state.throw_held = true;
            state.aim_release_sticky = false;
            state.aim_active = aim.is_some();
            state.aim_vec = aim;
            state.aim_angle_rad = aim.map_or(0.0, stick_aim_angle);
        }
        None if prev_throw.is_some() && state.aim_active && !state.aim_release_sticky => {
            // First frame after an *aimed* throw touch released: hold the aim
            // for exactly one frame with THROW_DOWN cleared, so sim sees the
            // release edge AND the aim direction/power on the spawn frame —
            // even if both thumbs lifted at once.
            state.throw_held = false;
            state.aim_release_sticky = true;
            // aim_active / aim_angle_rad / aim_vec retain last frame's values.
        }
        None => {
            // No throw touch (and no pending sticky-release): fully clear.
            state.throw_held = false;
            state.aim_active = false;
            state.aim_angle_rad = 0.0;
            state.aim_vec = None;
            state.aim_release_sticky = false;
        }
    }
}

/// PreUpdate system: resolves dash + throw/aim after the stick has been
/// resolved. Thin shim over [`apply_throw_state`].
pub fn update_throw_state(
    mut state: ResMut<TouchState>,
    window: Res<WindowSize>,
    southpaw: Res<Southpaw>,
) {
    let window_size = window.0;
    apply_throw_state(&mut state, window_size, southpaw.0);
}

/// Quantize an aim angle from radians (atan2 range, [-π, π]) to a u8
/// covering one full turn. Wraps so π and -π collapse onto the same
/// byte, matching the cyclic nature of the angle. Out-of-range inputs
/// are clamped — atan2 itself never produces them, but defensive
/// math here is cheap and prevents pathological wire values.
pub fn quantize_angle(rad: f32) -> u8 {
    use core::f32::consts::PI;
    let normalized = (rad + PI) / (2.0 * PI); // [0, 1] for rad in [-π, π]
    let scaled = (normalized * 256.0).floor();
    scaled.clamp(0.0, 255.0) as u8
}

/// Pure quantization from `TouchState` to the 4-byte wire-format
/// `PlayerInput`. Stick components are scaled to i8 in [-127, 127];
/// stick_y is negated because Bevy reports y-down screen coords but
/// the wire format follows game-space (y-up) convention so sim's
/// movement code can use stick_y as a velocity multiplier directly.
pub fn quantize_inputs(state: &TouchState) -> PlayerInput {
    // While aiming (incl. the one sticky-release frame), the aim vector
    // occupies the wire stick — direction is the throw heading and magnitude
    // is throw power. The aim IS the left stick (snapshotted), so this is
    // usually an identity swap; the separate field matters on the sticky-
    // release frame, when the live stick may already be gone. Sim roots the
    // character while charging, so no movement is lost to the repurposing.
    let stick = if state.aim_active {
        state.aim_vec.unwrap_or(Vec2::ZERO)
    } else {
        state.stick.unwrap_or(Vec2::ZERO)
    };
    let stick_x = (stick.x * 127.0).round().clamp(-127.0, 127.0) as i8;
    let stick_y = (-stick.y * 127.0).round().clamp(-127.0, 127.0) as i8;

    let aim_angle = if state.aim_active {
        quantize_angle(state.aim_angle_rad)
    } else {
        0
    };

    let mut buttons = 0u8;
    if state.throw_held {
        buttons |= PlayerInput::THROW_DOWN;
    }
    if state.aim_active {
        buttons |= PlayerInput::AIM_ACTIVE;
    }
    if state.dash_held {
        buttons |= PlayerInput::DASH_DOWN;
    }
    if state.taunt_held {
        buttons |= PlayerInput::TAUNT_DOWN;
    }

    PlayerInput {
        stick_x,
        stick_y,
        aim_angle,
        buttons,
    }
}

/// Mirror a quantized input across the X axis (world-Y negation), for the
/// client whose render runs with `PerspectiveFlip = -1`: that phone draws
/// the table upside down so its own player sits at the near edge, and a
/// screen drag must be reflected into world space before it hits the wire —
/// otherwise dragging down walks the character up (and aim inverts with
/// it). Pure integer reflection applied PRE-wire, so both peers still
/// exchange plain world-space inputs and the sim stays deterministic.
/// `stick_y` covers both movement and aim (the aim vector rides the wire
/// stick while AIM_ACTIVE); the `aim_angle` byte reflects cyclically
/// (theta -> -theta) so it stays consistent with the stick.
pub fn mirror_input_y(input: PlayerInput) -> PlayerInput {
    PlayerInput {
        // The quantizer clamps to [-127, 127], so negation cannot hit i8::MIN.
        stick_y: -input.stick_y,
        aim_angle: input.aim_angle.wrapping_neg(),
        ..input
    }
}

/// `ReadInputs` system: reads the local `TouchState` and writes the
/// same quantized `PlayerInput` for every local handle into
/// `LocalInputs<GgrsCfg>`. SyncTest mode has both players local; in
/// online mode only the actual local player is in `LocalPlayers`.
/// Mirrors `sim::read_local_inputs` shape so it's a drop-in
/// replacement for `DefaultInputsPlugin` at the app boundary.
pub fn read_local_touch_inputs(
    mut commands: Commands,
    touch_state: Res<TouchState>,
    local_players: Res<LocalPlayers>,
) {
    let input = quantize_inputs(&touch_state);
    let mut map = bevy::platform::collections::HashMap::default();
    for handle in &local_players.0 {
        map.insert(*handle, input);
    }
    commands.insert_resource(LocalInputs::<GgrsCfg>(map));
}

pub struct InputTouchPlugin;

impl Plugin for InputTouchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TouchState>()
            .init_resource::<WindowSize>()
            .init_resource::<CursorPosition>()
            .init_resource::<StickDeadzone>()
            .init_resource::<Southpaw>()
            .add_systems(
                PreUpdate,
                (update_touch_state, update_virtual_stick, update_throw_state).chain(),
            );
    }
}

/// Production input source: `InputTouchPlugin` plus the GGRS
/// `ReadInputs` hookup. The app installs this in place of
/// `sim::DefaultInputsPlugin`. Two input plugins must not coexist —
/// they would race over `LocalInputs<GgrsCfg>`.
pub struct TouchInputsPlugin;

impl Plugin for TouchInputsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(InputTouchPlugin)
            .add_systems(ReadInputs, read_local_touch_inputs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_pressed() -> std::iter::Empty<(u64, Vec2)> {
        std::iter::empty()
    }
    fn empty_id() -> std::iter::Empty<u64> {
        std::iter::empty()
    }
    fn empty_active() -> std::iter::Empty<(u64, Vec2)> {
        std::iter::empty()
    }

    #[test]
    fn defaults_are_empty() {
        let s = TouchState::default();
        assert!(s.touches.is_empty());
        assert_eq!(s.frame, 0);
    }

    #[test]
    fn no_input_advances_frame_only() {
        let mut s = TouchState::default();
        apply_touch_events(
            &mut s,
            1,
            empty_pressed(),
            empty_id(),
            empty_canceled_helper(),
            empty_active(),
        );
        assert_eq!(s.frame, 1);
        assert!(s.touches.is_empty());
    }

    fn empty_canceled_helper() -> std::iter::Empty<u64> {
        std::iter::empty()
    }

    #[test]
    fn just_pressed_adds_tracked_touch() {
        let mut s = TouchState::default();
        let pos = Vec2::new(120.0, 480.0);
        apply_touch_events(
            &mut s,
            5,
            vec![(7u64, pos)],
            empty_id(),
            empty_canceled_helper(),
            vec![(7u64, pos)],
        );
        assert_eq!(s.touches.len(), 1);
        let t = &s.touches[0];
        assert_eq!(t.id, 7);
        assert_eq!(t.start_pos, pos);
        assert_eq!(t.current_pos, pos);
        assert_eq!(t.start_frame, 5);
    }

    #[test]
    fn just_released_removes_tracked_touch() {
        let mut s = TouchState::default();
        apply_touch_events(
            &mut s,
            1,
            vec![(7u64, Vec2::ZERO)],
            empty_id(),
            empty_canceled_helper(),
            vec![(7u64, Vec2::ZERO)],
        );
        assert_eq!(s.touches.len(), 1);
        apply_touch_events(
            &mut s,
            2,
            empty_pressed(),
            vec![7u64],
            empty_canceled_helper(),
            empty_active(),
        );
        assert!(s.touches.is_empty());
    }

    #[test]
    fn just_canceled_removes_tracked_touch() {
        let mut s = TouchState::default();
        apply_touch_events(
            &mut s,
            1,
            vec![(9u64, Vec2::ZERO)],
            empty_id(),
            empty_canceled_helper(),
            vec![(9u64, Vec2::ZERO)],
        );
        apply_touch_events(
            &mut s,
            2,
            empty_pressed(),
            empty_id(),
            vec![9u64],
            empty_active(),
        );
        assert!(s.touches.is_empty());
    }

    #[test]
    fn active_updates_current_pos_preserves_start_pos() {
        let mut s = TouchState::default();
        let start = Vec2::new(100.0, 200.0);
        apply_touch_events(
            &mut s,
            1,
            vec![(3u64, start)],
            empty_id(),
            empty_canceled_helper(),
            vec![(3u64, start)],
        );
        let drift = Vec2::new(150.0, 220.0);
        apply_touch_events(
            &mut s,
            2,
            empty_pressed(),
            empty_id(),
            empty_canceled_helper(),
            vec![(3u64, drift)],
        );
        let t = &s.touches[0];
        assert_eq!(t.start_pos, start);
        assert_eq!(t.current_pos, drift);
        assert_eq!(t.start_frame, 1);
    }

    #[test]
    fn id_reuse_drops_stale_entry_and_starts_fresh() {
        let mut s = TouchState::default();
        let first = Vec2::new(50.0, 50.0);
        apply_touch_events(
            &mut s,
            1,
            vec![(11u64, first)],
            empty_id(),
            empty_canceled_helper(),
            vec![(11u64, first)],
        );
        // Bevy ends the first touch and reuses the id immediately.
        // We expect the stale entry to be dropped and a new one to
        // appear with the new start_frame.
        let second = Vec2::new(300.0, 300.0);
        apply_touch_events(
            &mut s,
            2,
            vec![(11u64, second)],
            vec![11u64],
            empty_canceled_helper(),
            vec![(11u64, second)],
        );
        assert_eq!(s.touches.len(), 1);
        let t = &s.touches[0];
        assert_eq!(t.id, 11);
        assert_eq!(t.start_pos, second);
        assert_eq!(t.start_frame, 2);
    }

    #[test]
    fn multi_touch_tracks_each_independently() {
        let mut s = TouchState::default();
        let a = Vec2::new(50.0, 700.0);
        let b = Vec2::new(700.0, 700.0);
        apply_touch_events(
            &mut s,
            1,
            vec![(1u64, a), (2u64, b)],
            empty_id(),
            empty_canceled_helper(),
            vec![(1u64, a), (2u64, b)],
        );
        assert_eq!(s.touches.len(), 2);
        assert_eq!(s.find(1).unwrap().start_pos, a);
        assert_eq!(s.find(2).unwrap().start_pos, b);
    }

    #[test]
    fn find_returns_none_for_unknown_id() {
        let s = TouchState::default();
        assert!(s.find(99).is_none());
    }

    // ---- Cycle 2: deadzone curve ----

    #[test]
    fn deadzone_collapses_below_inner() {
        assert_eq!(apply_radial_deadzone(0.0, 0.12, 0.75), 0.0);
        assert_eq!(apply_radial_deadzone(0.05, 0.12, 0.75), 0.0);
        assert_eq!(apply_radial_deadzone(0.12, 0.12, 0.75), 0.0);
    }

    #[test]
    fn deadzone_saturates_at_or_above_saturation() {
        assert_eq!(apply_radial_deadzone(0.75, 0.12, 0.75), 1.0);
        assert_eq!(apply_radial_deadzone(1.0, 0.12, 0.75), 1.0);
        assert_eq!(apply_radial_deadzone(2.5, 0.12, 0.75), 1.0);
    }

    #[test]
    fn deadzone_linear_remap_between_inner_and_saturation() {
        // Midpoint of [0.12, 0.75] is 0.435 → smoothed should be 0.5.
        let mid = (0.12 + 0.75) * 0.5;
        let v = apply_radial_deadzone(mid, 0.12, 0.75);
        assert!((v - 0.5).abs() < 1e-6, "expected ~0.5, got {v}");
    }

    // ---- Cycle 2: compute_stick ----

    #[test]
    fn compute_stick_zero_delta_is_zero() {
        let p = Vec2::new(123.0, 456.0);
        assert_eq!(
            compute_stick(
                p,
                p,
                STICK_MAX_RADIUS_PX,
                STICK_DEADZONE_INNER,
                STICK_DEADZONE_SATURATION
            ),
            Vec2::ZERO
        );
    }

    #[test]
    fn compute_stick_below_deadzone_is_zero() {
        // Drag of 5px; deadzone inner is 12% of 80px = 9.6px.
        let v = compute_stick(
            Vec2::ZERO,
            Vec2::new(5.0, 0.0),
            STICK_MAX_RADIUS_PX,
            STICK_DEADZONE_INNER,
            STICK_DEADZONE_SATURATION,
        );
        assert_eq!(v, Vec2::ZERO);
    }

    #[test]
    fn compute_stick_at_saturation_is_unit_vector() {
        // Drag of 60px (= 75% of 80px max radius).
        let v = compute_stick(
            Vec2::ZERO,
            Vec2::new(60.0, 0.0),
            STICK_MAX_RADIUS_PX,
            STICK_DEADZONE_INNER,
            STICK_DEADZONE_SATURATION,
        );
        assert!((v.length() - 1.0).abs() < 1e-5, "expected unit, got {v:?}");
        assert!(v.x > 0.99); // direction preserved
        assert!(v.y.abs() < 1e-5);
    }

    #[test]
    fn compute_stick_clamps_past_saturation() {
        // Drag of 200px — way past 75% saturation point.
        let v = compute_stick(
            Vec2::ZERO,
            Vec2::new(200.0, 0.0),
            STICK_MAX_RADIUS_PX,
            STICK_DEADZONE_INNER,
            STICK_DEADZONE_SATURATION,
        );
        assert!((v.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn compute_stick_preserves_diagonal_direction() {
        // 45° drag at saturation should produce a unit diagonal.
        let r = 60.0; // 75% of 80px
        let drag = Vec2::new(r, r) * (1.0 / 2.0_f32.sqrt());
        let v = compute_stick(
            Vec2::ZERO,
            drag,
            STICK_MAX_RADIUS_PX,
            STICK_DEADZONE_INNER,
            STICK_DEADZONE_SATURATION,
        );
        // Magnitude should be ~1.0; x and y components equal.
        assert!((v.length() - 1.0).abs() < 1e-4, "len={}", v.length());
        assert!((v.x - v.y).abs() < 1e-5);
    }

    // ---- Anchor follow: the stick base trails a runaway thumb ----

    /// Drive one touch through press + a sequence of moves, returning the
    /// final state. Each move is one frame's active update.
    fn drag(start: Vec2, moves: &[Vec2]) -> TouchState {
        let mut s = TouchState::default();
        apply_touch_events(
            &mut s,
            1,
            vec![(1u64, start)],
            empty_id(),
            empty_canceled_helper(),
            vec![(1u64, start)],
        );
        for (i, pos) in moves.iter().enumerate() {
            apply_touch_events(
                &mut s,
                2 + i as u64,
                empty_pressed(),
                empty_id(),
                empty_canceled_helper(),
                vec![(1u64, *pos)],
            );
        }
        s
    }

    #[test]
    fn anchor_starts_at_the_landing_spot() {
        let start = Vec2::new(200.0, 1800.0);
        let s = drag(start, &[]);
        assert_eq!(s.touches[0].anchor_pos, start);
    }

    #[test]
    fn anchor_stays_put_inside_the_follow_radius() {
        let start = Vec2::new(200.0, 1800.0);
        // 59px < the 60px follow radius: classic floating stick, no drag.
        let s = drag(start, &[start + Vec2::new(59.0, 0.0)]);
        assert_eq!(s.touches[0].anchor_pos, start);
    }

    #[test]
    fn anchor_gets_dragged_along_by_the_edge() {
        let start = Vec2::new(200.0, 1800.0);
        // Thumb runs 200px right: the anchor trails it at exactly the
        // follow radius, along the drag direction.
        let end = start + Vec2::new(200.0, 0.0);
        let s = drag(start, &[end]);
        let anchor = s.touches[0].anchor_pos;
        assert_eq!(anchor, end - Vec2::new(STICK_FOLLOW_RADIUS_PX, 0.0));
        // And the resolved deflection sits at saturation = full push.
        let stick = compute_stick(
            anchor,
            end,
            STICK_MAX_RADIUS_PX,
            STICK_DEADZONE_INNER,
            STICK_DEADZONE_SATURATION,
        );
        assert!((stick.length() - 1.0).abs() < 1e-5, "followed stick stays saturated");
        assert!(stick.x > 0.99);
    }

    #[test]
    fn followed_stick_reverses_quickly() {
        let start = Vec2::new(400.0, 1800.0);
        let far_right = start + Vec2::new(300.0, 0.0);
        // Run far right (anchor follows at 60px), then pull 90px back left:
        // the thumb crosses the trailing anchor and the deflection flips.
        // Without follow the anchor would still sit 300px away and the same
        // 90px of return would read as a hard-right push.
        let back = far_right - Vec2::new(90.0, 0.0);
        let s = drag(start, &[far_right, back]);
        let t = &s.touches[0];
        let stick = compute_stick(
            t.anchor_pos,
            t.current_pos,
            STICK_MAX_RADIUS_PX,
            STICK_DEADZONE_INNER,
            STICK_DEADZONE_SATURATION,
        );
        assert!(stick.x < 0.0, "reversal reads within one radius, got {stick:?}");
    }

    #[test]
    fn follow_preserves_direction_changes() {
        let start = Vec2::new(400.0, 1800.0);
        // Sweep right then straight up well past the radius: the anchor
        // ends trailing directly BELOW the thumb.
        let s = drag(
            start,
            &[start + Vec2::new(200.0, 0.0), start + Vec2::new(200.0, -300.0)],
        );
        let t = &s.touches[0];
        let delta = t.current_pos - t.anchor_pos;
        assert!((delta.length() - STICK_FOLLOW_RADIUS_PX).abs() < 1e-3);
        assert!(delta.y < 0.0 && delta.x.abs() < delta.y.abs() * 0.5, "anchor trails the new heading, delta {delta:?}");
        // start_pos never moved — zone semantics stay pinned to the landing.
        assert_eq!(t.start_pos, start);
    }

    // ---- Cycle 2: move-zone gate (the left half below the taunt strip) ----

    #[test]
    fn move_zone_is_the_left_half_below_the_taunt_strip() {
        let win = Vec2::new(1080.0, 2400.0);
        assert!(is_move_zone(Vec2::new(100.0, 2000.0), win)); // bottom-left
        assert!(is_move_zone(Vec2::new(100.0, 600.0), win)); // just below the strip
        assert!(is_move_zone(Vec2::new(539.0, 1201.0), win)); // just left of center
        assert!(!is_move_zone(Vec2::new(100.0, 100.0), win)); // top-left = taunt strip
    }

    #[test]
    fn move_zone_rejects_the_right_half() {
        let win = Vec2::new(1080.0, 2400.0);
        assert!(!is_move_zone(Vec2::new(540.0, 2000.0), win)); // center line
        assert!(!is_move_zone(Vec2::new(900.0, 100.0), win)); // top-right
        assert!(!is_move_zone(Vec2::new(900.0, 2300.0), win)); // bottom-right
    }

    #[test]
    fn move_zone_rejects_zero_window() {
        // Initial frame before WindowSize is populated.
        assert!(!is_move_zone(Vec2::new(50.0, 50.0), Vec2::ZERO));
    }

    // ---- Cycle 2: stick selection ----

    fn t(id: u64, start: Vec2) -> TrackedTouch {
        TrackedTouch {
            id,
            start_pos: start,
            current_pos: start,
            anchor_pos: start,
            start_frame: 0,
        }
    }

    #[test]
    fn select_stick_picks_first_left_half_touch() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![
            t(1, Vec2::new(900.0, 100.0)),  // right half (no)
            t(2, Vec2::new(200.0, 2000.0)), // left half (yes)
            t(3, Vec2::new(100.0, 2200.0)), // also left half (would tie but order wins)
        ];
        assert_eq!(select_stick_touch(None, &touches, win, false), Some(2));
    }

    #[test]
    fn select_stick_keeps_current_if_still_active() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![
            t(2, Vec2::new(900.0, 100.0)), // dragged to top-right
            t(3, Vec2::new(100.0, 2200.0)),
        ];
        // Touch 2 was selected when it started in the lower-left and
        // since drifted to the top-right; we still want to keep it.
        assert_eq!(select_stick_touch(Some(2), &touches, win, false), Some(2));
    }

    #[test]
    fn select_stick_replaces_when_current_ended() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![t(3, Vec2::new(100.0, 2200.0))];
        // Touch 2 ended; touch 3 is in the left half so it gets promoted.
        assert_eq!(select_stick_touch(Some(2), &touches, win, false), Some(3));
    }

    #[test]
    fn select_stick_returns_none_when_no_left_half_candidate() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![
            t(1, Vec2::new(900.0, 100.0)),
            t(2, Vec2::new(900.0, 2300.0)),
        ];
        assert_eq!(select_stick_touch(None, &touches, win, false), None);
    }

    #[test]
    fn select_stick_no_touches_returns_none() {
        let win = Vec2::new(1080.0, 2400.0);
        assert_eq!(select_stick_touch(None, &[], win, false), None);
        assert_eq!(select_stick_touch(Some(99), &[], win, false), None);
    }

    // ---- Cycle 3: throw-zone gate (right half minus the dash corner) ----

    // ---- Taunt strip: the top of the screen, exclusive ----

    #[test]
    fn taunt_zone_is_the_top_strip_minus_the_quit_corner() {
        let win = Vec2::new(1080.0, 2400.0);
        assert!(is_taunt_zone(Vec2::new(100.0, 100.0), win)); // top-left
        assert!(is_taunt_zone(Vec2::new(800.0, 100.0), win)); // top, left of the quit corner
        assert!(is_taunt_zone(Vec2::new(540.0, 575.0), win)); // just inside
        assert!(!is_taunt_zone(Vec2::new(900.0, 100.0), win)); // quit corner
        assert!(!is_taunt_zone(Vec2::new(540.0, 576.0), win)); // at the boundary
        assert!(!is_taunt_zone(Vec2::new(100.0, 2000.0), win)); // thumb country
        assert!(!is_taunt_zone(Vec2::new(50.0, 50.0), Vec2::ZERO)); // zero window
    }

    #[test]
    fn quit_corner_is_reserved_and_dead_to_every_stick_zone() {
        let win = Vec2::new(1080.0, 2400.0);
        // Corner starts at x >= 0.78 * 1080 = 842.4, ends at y < 0.16 * 2400 = 384.
        for p in [
            Vec2::new(900.0, 100.0),
            Vec2::new(843.0, 383.0),  // just inside
            Vec2::new(1079.0, 10.0),  // corner pixel
        ] {
            assert!(is_quit_zone(p, win), "{p:?} is the quit corner");
            assert!(!is_taunt_zone(p, win));
            assert!(!is_throw_zone(p, win));
            assert!(!is_move_zone(p, win));
            assert!(!is_dash_zone(p, win));
        }
        assert!(!is_quit_zone(Vec2::new(800.0, 100.0), win)); // left of it = taunt
        assert!(!is_quit_zone(Vec2::new(900.0, 400.0), win)); // below it = throw
        assert!(!is_quit_zone(Vec2::new(50.0, 50.0), Vec2::ZERO)); // zero window
    }

    #[test]
    fn select_taunt_touch_picks_strip_starts_and_is_sticky() {
        let win = Vec2::new(1080.0, 2400.0);
        // A move-zone start never becomes the taunt touch.
        let low = [t(1, Vec2::new(100.0, 2000.0))];
        assert_eq!(select_taunt_touch(None, &low, win, false), None);
        // A strip start does, and stays selected while alive.
        let strip = [t(2, Vec2::new(540.0, 200.0))];
        assert_eq!(select_taunt_touch(None, &strip, win, false), Some(2));
        assert_eq!(select_taunt_touch(Some(2), &strip, win, false), Some(2));
        // Gone touch: selection clears.
        assert_eq!(select_taunt_touch(Some(2), &low, win, false), None);
    }

    #[test]
    fn quantized_taunt_bit_follows_taunt_held() {
        let mut state = TouchState::default();
        assert_eq!(
            quantize_inputs(&state).buttons & PlayerInput::TAUNT_DOWN,
            0
        );
        state.taunt_held = true;
        assert_ne!(
            quantize_inputs(&state).buttons & PlayerInput::TAUNT_DOWN,
            0
        );
    }

    #[test]
    fn throw_zone_is_the_right_half_minus_the_dash_corner() {
        let win = Vec2::new(1080.0, 2400.0);
        // Dash corner starts at x >= 745.2, y >= 1920.
        assert!(is_throw_zone(Vec2::new(540.0, 1201.0), win)); // center line
        assert!(!is_throw_zone(Vec2::new(900.0, 100.0), win)); // top-right = quit corner
        assert!(is_throw_zone(Vec2::new(1079.0, 1500.0), win));
        assert!(is_throw_zone(Vec2::new(700.0, 2300.0), win)); // left of the dash corner
        assert!(is_throw_zone(Vec2::new(900.0, 1900.0), win)); // above the dash corner
    }

    #[test]
    fn throw_zone_rejects_left_half_and_dash_corner() {
        let win = Vec2::new(1080.0, 2400.0);
        assert!(!is_throw_zone(Vec2::new(539.0, 1800.0), win)); // left half
        assert!(!is_throw_zone(Vec2::new(0.0, 0.0), win));
        assert!(!is_throw_zone(Vec2::new(900.0, 2100.0), win)); // dash corner
    }

    #[test]
    fn dash_zone_recognizes_the_bottom_right_corner() {
        let win = Vec2::new(1080.0, 2400.0);
        // Corner starts at x >= 0.69 * 1080 = 745.2, y >= 0.80 * 2400 = 1920.
        assert!(is_dash_zone(Vec2::new(900.0, 2100.0), win));
        assert!(is_dash_zone(Vec2::new(746.0, 1921.0), win)); // just inside
        assert!(is_dash_zone(Vec2::new(1079.0, 2399.0), win)); // corner pixel
    }

    #[test]
    fn dash_zone_rejects_outside_the_corner() {
        let win = Vec2::new(1080.0, 2400.0);
        assert!(!is_dash_zone(Vec2::new(100.0, 2000.0), win)); // left half
        assert!(!is_dash_zone(Vec2::new(700.0, 2300.0), win)); // left of it = throw
        assert!(!is_dash_zone(Vec2::new(900.0, 1900.0), win)); // above it = throw
    }

    #[test]
    fn throw_zone_rejects_zero_window() {
        // Without the guard, x >= 0 would match every touch.
        assert!(!is_throw_zone(Vec2::new(50.0, 50.0), Vec2::ZERO));
        assert!(!is_throw_zone(Vec2::new(0.0, 0.0), Vec2::ZERO));
    }

    // ---- Cycle 3: throw selection ----

    #[test]
    fn select_throw_picks_first_right_half_touch() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![
            t(1, Vec2::new(100.0, 2000.0)), // left half (no)
            t(2, Vec2::new(900.0, 1800.0)), // right half (yes)
            t(3, Vec2::new(800.0, 1900.0)), // right half too, but order wins
        ];
        assert_eq!(select_throw_touch(None, &touches, win, false), Some(2));
    }

    #[test]
    fn select_throw_keeps_current_if_still_active() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![
            t(2, Vec2::new(100.0, 1800.0)), // dragged into the left half
            t(3, Vec2::new(900.0, 1800.0)),
        ];
        assert_eq!(select_throw_touch(Some(2), &touches, win, false), Some(2));
    }

    #[test]
    fn select_throw_replaces_when_current_ended() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![t(3, Vec2::new(900.0, 1800.0))];
        assert_eq!(select_throw_touch(Some(2), &touches, win, false), Some(3));
    }

    #[test]
    fn select_throw_returns_none_when_no_throw_zone_candidate() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![
            t(1, Vec2::new(100.0, 100.0)),
            t(2, Vec2::new(200.0, 2300.0)),
        ];
        assert_eq!(select_throw_touch(None, &touches, win, false), None);
    }

    #[test]
    fn select_throw_no_touches_returns_none() {
        let win = Vec2::new(1080.0, 2400.0);
        assert_eq!(select_throw_touch(None, &[], win, false), None);
        assert_eq!(select_throw_touch(Some(99), &[], win, false), None);
    }

    // ---- Cycle 3: stick heading ----

    #[test]
    fn stick_aim_angle_zero_stick_is_zero() {
        assert_eq!(stick_aim_angle(Vec2::ZERO), 0.0);
    }

    #[test]
    fn stick_aim_angle_covers_the_compass() {
        // Bevy y-down stick vectors → game-space (y-up) radians.
        assert!(stick_aim_angle(Vec2::new(1.0, 0.0)).abs() < 1e-6);
        let up = stick_aim_angle(Vec2::new(0.0, -1.0)); // pushed visually up
        assert!((up - std::f32::consts::FRAC_PI_2).abs() < 1e-6, "got {up}");
        let down = stick_aim_angle(Vec2::new(0.0, 1.0));
        assert!((down + std::f32::consts::FRAC_PI_2).abs() < 1e-6, "got {down}");
        let left = stick_aim_angle(Vec2::new(-1.0, 0.0));
        assert!((left.abs() - std::f32::consts::PI).abs() < 1e-6, "got {left}");
        let diag = stick_aim_angle(Vec2::new(1.0, -1.0)); // up-right
        assert!((diag - std::f32::consts::FRAC_PI_4).abs() < 1e-6, "got {diag}");
    }

    // ---- Cycle 3: left-stick aiming (throw button + move stick heading) ----

    /// A throw touch resting on the right half of a 1080x2400 window.
    fn hold_throw_button(s: &mut TouchState, frame: u64) {
        let pos = Vec2::new(900.0, 1800.0);
        apply_touch_events(
            s,
            frame,
            vec![(7u64, pos)],
            empty_id(),
            empty_canceled_helper(),
            vec![(7u64, pos)],
        );
    }

    #[test]
    fn throw_hold_with_centered_stick_is_not_aiming() {
        let win = Vec2::new(1080.0, 2400.0);
        let mut s = TouchState::default();
        hold_throw_button(&mut s, 1);
        apply_throw_state(&mut s, win, false);
        assert!(s.throw_held);
        assert!(!s.aim_active, "no left stick → tap-throw on release");
        assert_eq!(s.aim_vec, None);
        assert_eq!(s.aim_angle_rad, 0.0);
    }

    #[test]
    fn left_stick_aims_while_throw_held() {
        let win = Vec2::new(1080.0, 2400.0);
        let mut s = TouchState {
            stick: Some(Vec2::new(0.0, -1.0)), // left thumb pushed visually up
            ..Default::default()
        };
        hold_throw_button(&mut s, 1);
        apply_throw_state(&mut s, win, false);
        assert!(s.throw_held);
        assert!(s.aim_active);
        assert_eq!(s.aim_vec, Some(Vec2::new(0.0, -1.0)));
        assert!(
            (s.aim_angle_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-6,
            "game-space up, got {}",
            s.aim_angle_rad
        );
    }

    #[test]
    fn recentering_the_stick_cancels_the_aim_mid_hold() {
        let win = Vec2::new(1080.0, 2400.0);
        let mut s = TouchState {
            stick: Some(Vec2::new(1.0, 0.0)),
            ..Default::default()
        };
        hold_throw_button(&mut s, 1);
        apply_throw_state(&mut s, win, false);
        assert!(s.aim_active);
        // Next frame the left thumb re-centers (or lifts): back to tap mode.
        s.stick = Some(Vec2::ZERO);
        apply_throw_state(&mut s, win, false);
        assert!(s.throw_held);
        assert!(!s.aim_active);
        assert_eq!(s.aim_vec, None);
    }

    #[test]
    fn aim_survives_release_for_one_sticky_frame_even_if_both_thumbs_lift() {
        let win = Vec2::new(1080.0, 2400.0);
        let mut s = TouchState {
            stick: Some(Vec2::new(0.0, -1.0)),
            ..Default::default()
        };
        hold_throw_button(&mut s, 1);
        apply_throw_state(&mut s, win, false);
        assert!(s.aim_active);

        // Frame 2: both thumbs lift at once.
        s.stick = None;
        apply_touch_events(
            &mut s,
            2,
            empty_pressed(),
            vec![7u64],
            empty_canceled_helper(),
            empty_active(),
        );
        apply_throw_state(&mut s, win, false);
        assert!(!s.throw_held, "release edge must fire");
        assert!(s.aim_active, "aim held for the spawn frame");
        assert_eq!(s.aim_vec, Some(Vec2::new(0.0, -1.0)));
        assert!(s.aim_release_sticky);

        // Frame 3: fully cleared.
        apply_touch_events(
            &mut s,
            3,
            empty_pressed(),
            empty_id(),
            empty_canceled_helper(),
            empty_active(),
        );
        apply_throw_state(&mut s, win, false);
        assert!(!s.aim_active);
        assert_eq!(s.aim_vec, None);
        assert!(!s.aim_release_sticky);
    }

    #[test]
    fn unaimed_tap_release_has_no_sticky_frame() {
        let win = Vec2::new(1080.0, 2400.0);
        let mut s = TouchState::default();
        hold_throw_button(&mut s, 1);
        apply_throw_state(&mut s, win, false);
        assert!(s.throw_held && !s.aim_active);
        apply_touch_events(
            &mut s,
            2,
            empty_pressed(),
            vec![7u64],
            empty_canceled_helper(),
            empty_active(),
        );
        apply_throw_state(&mut s, win, false);
        assert!(!s.throw_held);
        assert!(!s.aim_active);
        assert!(!s.aim_release_sticky);
    }

    // ---- Cycle 4: mouse-drag fallback ----
    //
    // These tests exercise the mouse-synth iterators against the same
    // `apply_touch_events` pipeline `update_touch_state` uses, so they
    // verify the desktop fallback without needing a Bevy app fixture.

    fn mouse_pressed(pos: Vec2) -> std::iter::Once<(u64, Vec2)> {
        std::iter::once((MOUSE_TOUCH_ID, pos))
    }
    fn mouse_active(pos: Vec2) -> std::iter::Once<(u64, Vec2)> {
        std::iter::once((MOUSE_TOUCH_ID, pos))
    }
    fn mouse_released() -> std::iter::Once<u64> {
        std::iter::once(MOUSE_TOUCH_ID)
    }

    #[test]
    fn mouse_press_creates_synth_touch() {
        let mut s = TouchState::default();
        let pos = Vec2::new(640.0, 480.0);
        apply_touch_events(
            &mut s,
            1,
            mouse_pressed(pos),
            empty_id(),
            empty_canceled_helper(),
            mouse_active(pos),
        );
        assert_eq!(s.touches.len(), 1);
        let t = &s.touches[0];
        assert_eq!(t.id, MOUSE_TOUCH_ID);
        assert_eq!(t.start_pos, pos);
        assert_eq!(t.current_pos, pos);
        assert_eq!(t.start_frame, 1);
    }

    #[test]
    fn mouse_drag_then_release_round_trip() {
        let mut s = TouchState::default();
        let start = Vec2::new(200.0, 1800.0); // lower-left of a 1080x2400 window
        apply_touch_events(
            &mut s,
            1,
            mouse_pressed(start),
            empty_id(),
            empty_canceled_helper(),
            mouse_active(start),
        );
        // Frame 2: drag 30px right
        let drift = Vec2::new(230.0, 1800.0);
        apply_touch_events(
            &mut s,
            2,
            empty_pressed(),
            empty_id(),
            empty_canceled_helper(),
            mouse_active(drift),
        );
        let t = &s.touches[0];
        assert_eq!(t.start_pos, start);
        assert_eq!(t.current_pos, drift);
        assert_eq!(t.start_frame, 1);
        // Frame 3: release — note `pressed()` is false on release frame,
        // so the active iter is empty.
        apply_touch_events(
            &mut s,
            3,
            empty_pressed(),
            mouse_released(),
            empty_canceled_helper(),
            empty_active(),
        );
        assert!(s.touches.is_empty());
    }

    #[test]
    fn mouse_synth_drives_virtual_stick_through_pipeline() {
        // Simulates a left-half mouse drag end-to-end: press in the
        // lower-left, drag 60px (= 75% saturation) right, expect the
        // stick to come out unit-x.
        let mut s = TouchState::default();
        let win = Vec2::new(1080.0, 2400.0);
        let start = Vec2::new(200.0, 1800.0);
        apply_touch_events(
            &mut s,
            1,
            mouse_pressed(start),
            empty_id(),
            empty_canceled_helper(),
            mouse_active(start),
        );
        let drift = Vec2::new(260.0, 1800.0); // +60px on x
        apply_touch_events(
            &mut s,
            2,
            empty_pressed(),
            empty_id(),
            empty_canceled_helper(),
            mouse_active(drift),
        );
        // Now run the same select+compute the system would.
        s.stick_touch = select_stick_touch(s.stick_touch, &s.touches, win, false);
        assert_eq!(s.stick_touch, Some(MOUSE_TOUCH_ID));
        let stick = compute_stick(
            start,
            drift,
            STICK_MAX_RADIUS_PX,
            STICK_DEADZONE_INNER,
            STICK_DEADZONE_SATURATION,
        );
        assert!((stick.length() - 1.0).abs() < 1e-5);
        assert!(stick.x > 0.99);
    }

    #[test]
    fn mouse_id_does_not_collide_with_real_touch_ids() {
        // Real touchscreen ids are small monotonic integers; the
        // sentinel sits at u64::MAX so it cannot collide. The
        // headroom check is a const block so clippy doesn't flag it
        // as an assertion-on-constant — the point of the test is to
        // freeze that headroom into the compile-time contract.
        const _HEADROOM: () = assert!(MOUSE_TOUCH_ID > 1_000_000_000);
        assert_eq!(MOUSE_TOUCH_ID, u64::MAX);
    }

    // ---- Cycle 5: angle quantization ----

    #[test]
    fn quantize_angle_wraps_negative_pi_to_zero() {
        // rad = -π → normalized 0 → byte 0.
        assert_eq!(quantize_angle(-core::f32::consts::PI), 0);
    }

    #[test]
    fn quantize_angle_zero_is_midpoint() {
        // rad = 0 → normalized 0.5 → byte 128.
        assert_eq!(quantize_angle(0.0), 128);
    }

    #[test]
    fn quantize_angle_half_pi_is_three_quarter() {
        // rad = π/2 → normalized 0.75 → byte 192.
        assert_eq!(quantize_angle(core::f32::consts::FRAC_PI_2), 192);
    }

    #[test]
    fn quantize_angle_just_below_pi_saturates_at_max() {
        // rad just below π → normalized just below 1 → byte 255.
        let v = quantize_angle(core::f32::consts::PI - 1e-3);
        assert_eq!(v, 255);
    }

    #[test]
    fn quantize_angle_clamps_out_of_range() {
        // atan2 cannot produce these in practice, but defensive
        // clamping prevents pathological wire values.
        assert_eq!(quantize_angle(-10.0), 0);
        assert_eq!(quantize_angle(10.0), 255);
    }

    // ---- Cycle 5: input quantization ----

    #[test]
    fn quantize_inputs_idle_state_is_all_zero() {
        let s = TouchState::default();
        let p = quantize_inputs(&s);
        assert_eq!(p.stick_x, 0);
        assert_eq!(p.stick_y, 0);
        assert_eq!(p.aim_angle, 0);
        assert_eq!(p.buttons, 0);
    }

    #[test]
    fn quantize_inputs_unit_right_stick() {
        let s = TouchState {
            stick: Some(Vec2::new(1.0, 0.0)),
            ..Default::default()
        };
        let p = quantize_inputs(&s);
        assert_eq!(p.stick_x, 127);
        assert_eq!(p.stick_y, 0);
    }

    #[test]
    fn quantize_inputs_negates_y_for_game_space() {
        // Bevy y-down: positive bevy stick.y means finger dragged
        // downward. Wire convention is y-up, so this should flip to
        // a negative stick_y.
        let s = TouchState {
            stick: Some(Vec2::new(0.0, 1.0)),
            ..Default::default()
        };
        let p = quantize_inputs(&s);
        assert_eq!(p.stick_y, -127);
    }

    #[test]
    fn quantize_inputs_throw_held_sets_throw_down_bit() {
        let s = TouchState {
            throw_held: true,
            ..Default::default()
        };
        let p = quantize_inputs(&s);
        assert!(p.buttons & PlayerInput::THROW_DOWN != 0);
        assert!(p.buttons & PlayerInput::AIM_ACTIVE == 0);
    }

    #[test]
    fn quantize_inputs_aim_active_sets_aim_active_bit_and_angle() {
        let s = TouchState {
            throw_held: true,
            aim_active: true,
            aim_angle_rad: 0.0, // Game-space "right" → byte 128.
            ..Default::default()
        };
        let p = quantize_inputs(&s);
        assert!(p.buttons & PlayerInput::THROW_DOWN != 0);
        assert!(p.buttons & PlayerInput::AIM_ACTIVE != 0);
        assert_eq!(p.aim_angle, 128);
    }

    #[test]
    fn quantize_inputs_aim_active_writes_aim_vec_into_stick() {
        // When aiming, the wire stick carries the aim vector (here +x at full
        // magnitude), NOT the move stick (which is pulled the other way).
        let s = TouchState {
            stick: Some(Vec2::new(-1.0, 0.0)), // move stick: full left
            aim_active: true,
            aim_vec: Some(Vec2::new(1.0, 0.0)), // aim: full right
            aim_angle_rad: 0.0,
            throw_held: true,
            ..Default::default()
        };
        let p = quantize_inputs(&s);
        assert_eq!(p.stick_x, 127, "aim (+x) should win over move stick (-x)");
        assert_eq!(p.stick_y, 0);
        assert!(p.buttons & PlayerInput::AIM_ACTIVE != 0);
    }

    #[test]
    fn quantize_inputs_aim_vec_negates_y_for_game_space() {
        // bevy y-down aim_vec.y = +1 (drag down) → game-space -127.
        let s = TouchState {
            aim_active: true,
            aim_vec: Some(Vec2::new(0.0, 1.0)),
            ..Default::default()
        };
        let p = quantize_inputs(&s);
        assert_eq!(p.stick_y, -127);
    }

    #[test]
    fn quantize_inputs_aim_inactive_zeroes_angle() {
        // Even if aim_angle_rad has lingering data, when aim_active
        // is false the wire byte is 0 to keep the message clean.
        let s = TouchState {
            aim_angle_rad: 1.5,
            aim_active: false,
            ..Default::default()
        };
        let p = quantize_inputs(&s);
        assert_eq!(p.aim_angle, 0);
    }

    // ---- Perspective mirror (the PerspectiveFlip = -1 client) ----

    #[test]
    fn mirror_input_y_reflects_stick_and_angle() {
        let p = PlayerInput {
            stick_x: 40,
            stick_y: -90,
            aim_angle: 192, // pi/2
            buttons: PlayerInput::AIM_ACTIVE | PlayerInput::THROW_DOWN,
        };
        let m = mirror_input_y(p);
        assert_eq!(m.stick_x, 40, "x axis is not mirrored");
        assert_eq!(m.stick_y, 90);
        assert_eq!(m.aim_angle, 64, "pi/2 reflects to -pi/2");
        assert_eq!(m.buttons, p.buttons);
    }

    #[test]
    fn mirror_input_y_twice_is_identity() {
        let p = PlayerInput {
            stick_x: -7,
            stick_y: 33,
            aim_angle: 5,
            buttons: PlayerInput::DASH_DOWN,
        };
        assert_eq!(mirror_input_y(mirror_input_y(p)), p);
    }

    #[test]
    fn mirror_input_y_fixes_neutral_and_the_pi_seam() {
        // Neutral input stays neutral; the +/-pi seam (byte 0) is its own
        // reflection, as is game-space 0 rad (byte 128).
        let idle = PlayerInput::default();
        assert_eq!(mirror_input_y(idle), idle);
        let seam = PlayerInput {
            aim_angle: 0,
            ..idle
        };
        assert_eq!(mirror_input_y(seam).aim_angle, 0);
        let zero_rad = PlayerInput {
            aim_angle: 128,
            ..idle
        };
        assert_eq!(mirror_input_y(zero_rad).aim_angle, 128);
    }
}

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
//!     `read_local_inputs` system.
//!   * cycle 3: throw interaction state machine. Right-side touches
//!     drive `throw_held`; after AIM_HOLD_FRAMES of holding or
//!     AIM_DRAG_PX of motion, `aim_active` flips on and `aim_angle_rad`
//!     reports the drag angle. Tap-vs-hold detection itself is left to
//!     sim's `released_within(THROW_DOWN, …)` edge derivation (cycle 6).
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

/// Sentinel id for the synthesized left-mouse-button touch. `u64::MAX`
/// is well outside any real touchscreen id space (Bevy/Android emit
/// small monotonic ids), so it cannot collide with a real touch.
pub const MOUSE_TOUCH_ID: u64 = u64::MAX;

// ---- Cycle 3 tunables ----

/// Frames a right-side touch must be held before flipping `aim_active`
/// on. 6 frames at 60 Hz = 100 ms — the standard "tap vs hold" gate.
pub const AIM_HOLD_FRAMES: u64 = 6;

/// Pixels the right-side touch must drag before flipping `aim_active`
/// on (independent of hold time). Lets fast flick-throws read as aim
/// without waiting out the full hold timer.
pub const AIM_DRAG_PX: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackedTouch {
    pub id: u64,
    pub start_pos: Vec2,
    pub current_pos: Vec2,
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
    /// drags out of the lower-left quadrant.
    pub stick_touch: Option<u64>,
    /// Id of the active throw/aim touch, if any. Sticky in the same way
    /// as `stick_touch`: once a touch starts in the right half, we keep
    /// tracking it even if it drags elsewhere.
    pub right_touch: Option<u64>,
    /// True iff a right-side touch is currently held. Read by sim each
    /// frame (level signal) — edges are derived in sim by diffing
    /// against `PreviousInputs`.
    pub throw_held: bool,
    /// Sticky once set: stays true for the lifetime of the throw touch
    /// once the user has either held long enough or dragged far enough.
    /// Resets when the throw touch ends.
    pub aim_active: bool,
    /// Game-space (y-up) drag angle in radians, when `aim_active`. Held
    /// at 0.0 when no throw touch is active.
    pub aim_angle_rad: f32,
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

/// Resolve a (start, current) drag into a stick vector. Output
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

/// Bevy's window coords are top-left origin, y-down. "Lower-left of
/// the screen" = small x, large y. The threshold is the midpoint of
/// each axis — touches anywhere in the bottom-left quadrant qualify.
pub fn is_lower_left(pos: Vec2, window: Vec2) -> bool {
    pos.x < window.x * 0.5 && pos.y > window.y * 0.5
}

/// Right half of the screen (x >= w/2). Disjoint from the lower-left
/// stick zone by construction, so the same touch cannot be both stick
/// and throw. Guarded against a zero-sized window so the throw button
/// doesn't activate phantomly before `WindowSize` is populated.
pub fn is_right_side(pos: Vec2, window: Vec2) -> bool {
    window.x > 0.0 && pos.x >= window.x * 0.5
}

/// Pick the touch driving the virtual stick. Sticky: if the
/// `current` choice is still in the active list, keep it. Otherwise
/// promote the first lower-left touch we find. Returns `None` if no
/// candidate exists.
pub fn select_stick_touch(
    current: Option<u64>,
    touches: &[TrackedTouch],
    window: Vec2,
) -> Option<u64> {
    if let Some(id) = current
        && touches.iter().any(|t| t.id == id)
    {
        return Some(id);
    }
    touches
        .iter()
        .find(|t| is_lower_left(t.start_pos, window))
        .map(|t| t.id)
}

/// Pick the touch driving throw/aim. Same sticky pattern as
/// `select_stick_touch`: keep the current id while it's still active,
/// else promote the first right-side candidate.
pub fn select_throw_touch(
    current: Option<u64>,
    touches: &[TrackedTouch],
    window: Vec2,
) -> Option<u64> {
    if let Some(id) = current
        && touches.iter().any(|t| t.id == id)
    {
        return Some(id);
    }
    touches
        .iter()
        .find(|t| is_right_side(t.start_pos, window))
        .map(|t| t.id)
}

/// Drag angle in radians, in natural game-space (y-up) coordinates.
/// Bevy reports touch positions as y-down, so we negate dy here.
/// Zero-length drags return 0.0 instead of NaN-ing through atan2.
pub fn compute_aim_angle(start: Vec2, current: Vec2) -> f32 {
    let dx = current.x - start.x;
    let dy = -(current.y - start.y);
    if dx == 0.0 && dy == 0.0 {
        0.0
    } else {
        dy.atan2(dx)
    }
}

/// Decide if aim mode should be active this frame. Sticky once on
/// (the caller resets `was_active` to false when the throw touch
/// ends). Flips on either when the touch has been held long enough
/// or dragged far enough — whichever fires first.
pub fn should_aim_be_active(
    was_active: bool,
    frames_held: u64,
    drag_px: f32,
    hold_threshold: u64,
    drag_threshold: f32,
) -> bool {
    was_active || frames_held >= hold_threshold || drag_px >= drag_threshold
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
            start_frame: frame,
        });
    }

    for (id, pos) in active {
        if let Some(tracked) = state.touches.iter_mut().find(|tracked| tracked.id == id) {
            tracked.current_pos = pos;
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
/// has synced the touch list. Reads `WindowSize` to gate the lower-left
/// quadrant; the app populates `WindowSize` each frame from its primary
/// `Window`. With a zero-sized window (initial frame, no window yet)
/// `is_lower_left` answers false everywhere and no stick is selected,
/// which is the right behavior — we'd rather wait one frame than guess.
pub fn update_virtual_stick(
    mut state: ResMut<TouchState>,
    window: Res<WindowSize>,
    deadzone: Res<StickDeadzone>,
) {
    let window_size = window.0;
    state.stick_touch = select_stick_touch(state.stick_touch, &state.touches, window_size);
    let inner = deadzone.0;
    state.stick = state.stick_touch.and_then(|id| {
        state.find(id).map(|t| {
            compute_stick(
                t.start_pos,
                t.current_pos,
                STICK_MAX_RADIUS_PX,
                inner,
                STICK_DEADZONE_SATURATION,
            )
        })
    });
}

/// PreUpdate system: resolves throw/aim state after the stick has been
/// resolved. `aim_active` is sticky for the lifetime of the throw
/// touch — it resets only when the touch identity changes (touch ends
/// or a new right-side touch takes over). `aim_angle_rad` is reported
/// in game-space (y-up) radians so it composes naturally with sim's
/// trig conventions.
pub fn update_throw_state(mut state: ResMut<TouchState>, window: Res<WindowSize>) {
    let window_size = window.0;
    let new_throw = select_throw_touch(state.right_touch, &state.touches, window_size);

    let identity_changed = new_throw != state.right_touch;
    state.right_touch = new_throw;
    state.throw_held = new_throw.is_some();

    if identity_changed {
        state.aim_active = false;
        state.aim_angle_rad = 0.0;
    }

    match new_throw.and_then(|id| state.find(id).copied()) {
        Some(t) => {
            let frames_held = state.frame.saturating_sub(t.start_frame);
            let drag_px = (t.current_pos - t.start_pos).length();
            state.aim_active = should_aim_be_active(
                state.aim_active,
                frames_held,
                drag_px,
                AIM_HOLD_FRAMES,
                AIM_DRAG_PX,
            );
            state.aim_angle_rad = compute_aim_angle(t.start_pos, t.current_pos);
        }
        None => {
            state.aim_active = false;
            state.aim_angle_rad = 0.0;
        }
    }
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
    let stick = state.stick.unwrap_or(Vec2::ZERO);
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
    // DASH_DOWN, TAUNT_DOWN: deferred — no UI affordance yet.

    PlayerInput {
        stick_x,
        stick_y,
        aim_angle,
        buttons,
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
            compute_stick(p, p, STICK_MAX_RADIUS_PX, STICK_DEADZONE_INNER, STICK_DEADZONE_SATURATION),
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

    // ---- Cycle 2: lower-left gate ----

    #[test]
    fn lower_left_recognizes_bottom_left_quadrant() {
        let win = Vec2::new(1080.0, 2400.0);
        // Bevy y-down: y > h/2 = 1200 is "lower"; x < w/2 = 540 is "left".
        assert!(is_lower_left(Vec2::new(100.0, 2000.0), win));
        assert!(is_lower_left(Vec2::new(539.0, 1201.0), win));
    }

    #[test]
    fn lower_left_rejects_other_quadrants() {
        let win = Vec2::new(1080.0, 2400.0);
        assert!(!is_lower_left(Vec2::new(100.0, 100.0), win)); // top-left
        assert!(!is_lower_left(Vec2::new(900.0, 100.0), win)); // top-right
        assert!(!is_lower_left(Vec2::new(900.0, 2300.0), win)); // bottom-right
    }

    #[test]
    fn lower_left_rejects_zero_window() {
        // Initial frame before WindowSize is populated.
        assert!(!is_lower_left(Vec2::new(50.0, 50.0), Vec2::ZERO));
    }

    // ---- Cycle 2: stick selection ----

    fn t(id: u64, start: Vec2) -> TrackedTouch {
        TrackedTouch {
            id,
            start_pos: start,
            current_pos: start,
            start_frame: 0,
        }
    }

    #[test]
    fn select_stick_picks_first_lower_left_touch() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![
            t(1, Vec2::new(900.0, 100.0)), // top-right (no)
            t(2, Vec2::new(200.0, 2000.0)), // lower-left (yes)
            t(3, Vec2::new(100.0, 2200.0)), // also lower-left (would tie but order wins)
        ];
        assert_eq!(select_stick_touch(None, &touches, win), Some(2));
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
        assert_eq!(select_stick_touch(Some(2), &touches, win), Some(2));
    }

    #[test]
    fn select_stick_replaces_when_current_ended() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![t(3, Vec2::new(100.0, 2200.0))];
        // Touch 2 ended; touch 3 is in the lower-left so it gets promoted.
        assert_eq!(select_stick_touch(Some(2), &touches, win), Some(3));
    }

    #[test]
    fn select_stick_returns_none_when_no_lower_left_candidate() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![
            t(1, Vec2::new(900.0, 100.0)),
            t(2, Vec2::new(900.0, 2300.0)),
        ];
        assert_eq!(select_stick_touch(None, &touches, win), None);
    }

    #[test]
    fn select_stick_no_touches_returns_none() {
        let win = Vec2::new(1080.0, 2400.0);
        assert_eq!(select_stick_touch(None, &[], win), None);
        assert_eq!(select_stick_touch(Some(99), &[], win), None);
    }

    // ---- Cycle 3: right-side gate ----

    #[test]
    fn right_side_recognizes_right_half() {
        let win = Vec2::new(1080.0, 2400.0);
        assert!(is_right_side(Vec2::new(540.0, 100.0), win)); // exact midpoint
        assert!(is_right_side(Vec2::new(900.0, 1200.0), win));
        assert!(is_right_side(Vec2::new(1079.0, 2300.0), win));
    }

    #[test]
    fn right_side_rejects_left_half() {
        let win = Vec2::new(1080.0, 2400.0);
        assert!(!is_right_side(Vec2::new(539.0, 1200.0), win));
        assert!(!is_right_side(Vec2::new(0.0, 0.0), win));
    }

    #[test]
    fn right_side_rejects_zero_window() {
        // Without the guard, x >= 0 would match every touch.
        assert!(!is_right_side(Vec2::new(50.0, 50.0), Vec2::ZERO));
        assert!(!is_right_side(Vec2::new(0.0, 0.0), Vec2::ZERO));
    }

    // ---- Cycle 3: throw selection ----

    #[test]
    fn select_throw_picks_first_right_side_touch() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![
            t(1, Vec2::new(100.0, 2000.0)), // lower-left (no)
            t(2, Vec2::new(900.0, 200.0)),  // right side (yes)
            t(3, Vec2::new(800.0, 1900.0)), // right side too, but order wins
        ];
        assert_eq!(select_throw_touch(None, &touches, win), Some(2));
    }

    #[test]
    fn select_throw_keeps_current_if_still_active() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![
            t(2, Vec2::new(100.0, 100.0)), // dragged into the left half
            t(3, Vec2::new(900.0, 200.0)),
        ];
        assert_eq!(select_throw_touch(Some(2), &touches, win), Some(2));
    }

    #[test]
    fn select_throw_replaces_when_current_ended() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![t(3, Vec2::new(900.0, 200.0))];
        assert_eq!(select_throw_touch(Some(2), &touches, win), Some(3));
    }

    #[test]
    fn select_throw_returns_none_when_no_right_side_candidate() {
        let win = Vec2::new(1080.0, 2400.0);
        let touches = vec![
            t(1, Vec2::new(100.0, 100.0)),
            t(2, Vec2::new(200.0, 2300.0)),
        ];
        assert_eq!(select_throw_touch(None, &touches, win), None);
    }

    #[test]
    fn select_throw_no_touches_returns_none() {
        let win = Vec2::new(1080.0, 2400.0);
        assert_eq!(select_throw_touch(None, &[], win), None);
        assert_eq!(select_throw_touch(Some(99), &[], win), None);
    }

    // ---- Cycle 3: aim angle ----

    #[test]
    fn aim_angle_zero_drag_is_zero() {
        let p = Vec2::new(500.0, 500.0);
        assert_eq!(compute_aim_angle(p, p), 0.0);
    }

    #[test]
    fn aim_angle_pure_right_is_zero() {
        let v = compute_aim_angle(Vec2::ZERO, Vec2::new(50.0, 0.0));
        assert!(v.abs() < 1e-6, "expected ~0, got {v}");
    }

    #[test]
    fn aim_angle_pure_up_is_half_pi() {
        // Bevy y-down: finger swiping visually upward means current.y < start.y.
        let v = compute_aim_angle(Vec2::new(0.0, 100.0), Vec2::new(0.0, 0.0));
        assert!((v - std::f32::consts::FRAC_PI_2).abs() < 1e-6, "got {v}");
    }

    #[test]
    fn aim_angle_pure_left_is_pi() {
        let v = compute_aim_angle(Vec2::new(50.0, 0.0), Vec2::ZERO);
        assert!((v.abs() - std::f32::consts::PI).abs() < 1e-6, "got {v}");
    }

    #[test]
    fn aim_angle_pure_down_is_negative_half_pi() {
        let v = compute_aim_angle(Vec2::new(0.0, 0.0), Vec2::new(0.0, 100.0));
        assert!((v + std::f32::consts::FRAC_PI_2).abs() < 1e-6, "got {v}");
    }

    #[test]
    fn aim_angle_diagonal_up_right_is_quarter_pi() {
        // Swipe from (0, 100) to (100, 0): right and visually up.
        let v = compute_aim_angle(Vec2::new(0.0, 100.0), Vec2::new(100.0, 0.0));
        assert!((v - std::f32::consts::FRAC_PI_4).abs() < 1e-6, "got {v}");
    }

    // ---- Cycle 3: aim-active threshold ----

    #[test]
    fn aim_active_false_below_both_thresholds() {
        assert!(!should_aim_be_active(false, 0, 0.0, AIM_HOLD_FRAMES, AIM_DRAG_PX));
        assert!(!should_aim_be_active(
            false,
            AIM_HOLD_FRAMES - 1,
            AIM_DRAG_PX - 0.01,
            AIM_HOLD_FRAMES,
            AIM_DRAG_PX,
        ));
    }

    #[test]
    fn aim_active_true_at_hold_threshold() {
        assert!(should_aim_be_active(
            false,
            AIM_HOLD_FRAMES,
            0.0,
            AIM_HOLD_FRAMES,
            AIM_DRAG_PX,
        ));
    }

    #[test]
    fn aim_active_true_at_drag_threshold() {
        assert!(should_aim_be_active(
            false,
            0,
            AIM_DRAG_PX,
            AIM_HOLD_FRAMES,
            AIM_DRAG_PX,
        ));
    }

    #[test]
    fn aim_active_is_sticky_once_set() {
        // Was active last frame; thresholds slipped back below — still active.
        assert!(should_aim_be_active(true, 0, 0.0, AIM_HOLD_FRAMES, AIM_DRAG_PX));
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
        s.stick_touch = select_stick_touch(s.stick_touch, &s.touches, win);
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
}

//! Pause, speed and fast seek for anything that plays the sim back at a
//! pace other than real time: the replay theater today, the feel lab and
//! the live viewer next. All of it is `Time<Virtual>`: the sim's ticks are
//! driven by virtual time, so slowing it slows the match and pausing it
//! freezes it, while real-time UI (taps, the scrub bar) keeps running.

use bevy::prelude::*;

/// Playback speeds a thumb picks from. 0.25x is a desktop-scrubbing speed;
/// the phone gets the four that matter.
pub const SPEED_PRESETS: [f32; 4] = [0.5, 1.0, 2.0, 4.0];

/// Virtual-time multiplier while a seek runs the sim forward to its target.
pub const SEEK_SPEED: f32 = 64.0;

/// Play at `speed`.
pub fn play(world: &mut World, speed: f32) {
    let mut vt = world.resource_mut::<Time<Virtual>>();
    vt.unpause();
    vt.set_relative_speed(speed);
}

/// Freeze the sim where it is.
pub fn pause(world: &mut World) {
    world.resource_mut::<Time<Virtual>>().pause();
}

/// Paused, or playing at `speed`.
pub fn apply(world: &mut World, paused: bool, speed: f32) {
    if paused {
        pause(world);
    } else {
        play(world, speed);
    }
}

/// Run as fast as a seek runs.
pub fn seek(world: &mut World) {
    play(world, SEEK_SPEED);
}

/// Back to real time, unpaused: what every other screen expects.
pub fn realtime(world: &mut World) {
    play(world, 1.0);
}

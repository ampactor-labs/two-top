//! What the sim's state looks like on screen, beyond the `render` crate's
//! own systems: the duelists, the floor, the fangs, the washes and the
//! Title's diorama. Split out of `lib.rs`, which keeps `run()`, `setup`
//! and the plugin wiring.

mod actors;
mod diorama;
mod fangs;
mod floor;
mod scrims;

pub(crate) use actors::*;
pub(crate) use diorama::*;
pub(crate) use fangs::*;
pub(crate) use floor::*;
pub(crate) use scrims::*;

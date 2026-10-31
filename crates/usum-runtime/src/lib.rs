//! The native runtime that translated game code runs on.
//!
//! Bring-up stage (milestone 5): [`boot`] builds the application's address
//! space from the dump, [`kernel`] implements the SVCs the game has reached
//! so far, and [`probe`] runs translated code from the entry point until an
//! SVC that is not implemented yet, reporting the calls it made.

pub mod audio;
pub mod boot;
pub mod kernel;
pub mod native_timing;
pub mod pica;
#[cfg(feature = "window")]
pub mod play;
pub mod probe;
pub mod ro;
pub mod services;
#[cfg(feature = "window")]
pub mod sound;
pub mod timing;

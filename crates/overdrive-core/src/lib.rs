//! Overdrive core — platform-free engine logic.
//!
//! Phase 3: Module A (Hardware & Environment Profiler) + BIOS Coach.
//! Everything here consumes *raw* facts from a [`source::HardwareSource`] and turns
//! them into a [`profile::HardwareProfile`] with an explicit [`profile::Confidence`]
//! per field. Nothing here touches Win32: the real source lives in `overdrive-win`,
//! fakes live in [`testing`].

pub mod catalog;
pub mod coach;
pub mod drift;
pub mod engine;
pub mod experiment;
pub mod issues;
pub mod memspeed;
pub mod profile;
pub mod profiler;
pub mod safety;
pub mod session;
pub mod source;
pub mod synth;
pub mod telemetry;
pub mod testing;
pub mod tier;

pub use profile::{Confidence, Field, HardwareProfile};
pub use profiler::build_profile;
pub use source::HardwareSource;

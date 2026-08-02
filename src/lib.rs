//! CircuitC compiler foundation.
//!
//! The M1B frontend parses the deliberately small, unreleased CircuitC
//! language and elaborates it into the same canonical Design IR used by the
//! programmatic regression fixtures. Until the first release, the language,
//! schema, and Rust construction API evolve in place without compatibility
//! guarantees or migrations.

mod compile;
pub mod demo;
pub mod design;
pub mod frontend;
mod kicad;
mod library;
pub mod quantity;
mod spice;

pub use compile::{CompileError, CompiledArtifacts, KicadIdentity, compile};
pub use spice::{SpiceComponentNameMapping, SpiceNameMap, SpiceNetNameMapping};

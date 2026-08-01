//! CircuitC compiler foundation.
//!
//! The bootstrap API is intentionally programmatic. It establishes the
//! canonical semantic and backend boundaries before a user-facing language
//! commits the project to grammar and syntax choices.
//! Until the first released schema, this Rust construction API evolves in
//! place without source-compatibility guarantees or migrations.

mod compile;
pub mod demo;
pub mod design;
mod kicad;
pub mod quantity;
mod spice;

pub use compile::{CompileError, CompiledArtifacts, compile};
pub use spice::{SpiceComponentNameMapping, SpiceNameMap, SpiceNetNameMapping};

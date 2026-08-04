//! Versioned CircuitC-to-APGAR routing process boundary.

pub(crate) mod contract;
pub(crate) mod import;
pub(crate) mod lower;
pub(crate) mod project;

pub(crate) const PINNED_APGAR_SOURCE_REVISION: &str = "85a4f75b8c0c6142d319a8a743087f65ef9e9796";
pub(crate) const APGAR_CONTRACT_IDENTITY: &str =
    "apgar-board-ir-v1+geometry-compiler-v1+candidate-policy-v1+route-candidate-v1.0";
pub(crate) const APGAR_TOOL_NAME: &str = "circuitc-apgar-route";
pub(crate) const APGAR_TOOL_VERSION: &str = "1";
pub(crate) const APGAR_CPU_DEVICE_CLASS: &str = "cpu-reference-v1";

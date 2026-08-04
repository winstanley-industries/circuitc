//! Capability-declared, Design-bound board-analysis evidence.

mod bind;
mod contract;
mod normalize;

pub use bind::{
    bind_kicad10_board_analysis, prepare_kicad10_board_analysis_request,
    record_kicad10_board_analysis_noncompletion, verify_kicad10_board_analysis,
    verify_kicad10_board_analysis_noncompletion,
};
pub use contract::{
    BoardAnalysisBundle, BoardAnalysisDiagnostic, BoardAnalysisFile, BoardAnalysisHostEvidence,
    BoardAnalysisNoncompletion, BoardAnalysisNoncompletionKind, BoardAnalysisRequestBundle,
};

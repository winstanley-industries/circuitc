mod diagnostic;
mod elaborate;
mod lexer;
mod parser;
mod quantity;
mod syntax;

pub use diagnostic::{DiagnosticFormat, RelatedLocation, SourceDiagnostic, render_diagnostics};
pub use elaborate::{ElaboratedDesign, ProvenanceMap};
pub use syntax::{SourceFile, Span, SyntaxTree};

use crate::CompiledArtifacts;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledSource {
    pub elaborated: ElaboratedDesign,
    pub artifacts: CompiledArtifacts,
}

pub fn parse_source(
    filename: impl Into<String>,
    source: impl Into<String>,
) -> Result<SyntaxTree, Vec<SourceDiagnostic>> {
    parser::parse(SourceFile::new(filename, source))
}

pub fn elaborate_syntax(syntax: &SyntaxTree) -> Result<ElaboratedDesign, Vec<SourceDiagnostic>> {
    elaborate::elaborate(syntax)
}

pub fn elaborate_source(
    filename: impl Into<String>,
    source: impl Into<String>,
) -> Result<ElaboratedDesign, Vec<SourceDiagnostic>> {
    let syntax = parse_source(filename, source)?;
    elaborate_syntax(&syntax)
}

pub fn compile_source(
    filename: impl Into<String>,
    source: impl Into<String>,
) -> Result<CompiledSource, Vec<SourceDiagnostic>> {
    let syntax = parse_source(filename, source)?;
    let elaborated = elaborate_syntax(&syntax)?;
    let artifacts = crate::compile(&elaborated.design).map_err(|error| {
        elaborate::map_ir_diagnostics(&syntax.source, &elaborated.provenance, error.diagnostics)
    })?;
    Ok(CompiledSource {
        elaborated,
        artifacts,
    })
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticFormat, compile_source, render_diagnostics};

    #[test]
    fn backend_diagnostics_map_to_authored_route_span() {
        let source = "design d { ground GND; dc_source input V1 { voltage 1 V; terminals p n; connect p GND; connect n GND; } board { rectangle at (0 mm, 0 mm) size (1 mm, 1 mm); route bad net GND from (0 mm, 0 mm) to (2 mm, 0 mm) width 1 nm layer front; } }";
        let diagnostics = compile_source("bad-route.circuitc", source)
            .expect_err("out-of-board route must fail through Design validation");
        let route = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-ROUTE-004")
            .expect("mapped route diagnostic must exist");
        assert_eq!(
            route.semantic_path.as_deref(),
            Some("design.board.routes[0]")
        );
        assert_eq!(&source[route.start..route.start + "route".len()], "route");
    }

    #[test]
    fn structural_provenance_cannot_be_overwritten_by_component_paths() {
        let source = r#"design d {
  ground GND;
  dc_source design.board.outline V1 {
    voltage 1 V;
    terminals p n;
    connect p GND;
    connect n GND;
  }
  board {
    rectangle at (999999 mm, 0 mm) size (2 mm, 1 mm);
  }
}"#;
        let diagnostics = compile_source("collision.circuitc", source)
            .expect_err("outline beyond the coordinate envelope must fail");
        let outline = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-BOARD-004")
            .expect("mapped outline diagnostic must exist");
        assert_eq!(outline.line, 10);
        assert_eq!(
            &source[outline.start..outline.start + "rectangle".len()],
            "rectangle"
        );
    }

    #[test]
    fn component_provenance_cannot_be_overwritten_by_synthesized_semantic_paths() {
        let source = r#"design d {
  net N;
  ground GND;
  resistor x R1 {
    resistance 1 kohm;
    terminals 1 2;
    connect 1 N;
    connect 2 GND;
    footprint "CircuitC:R" {
      pad 1 at (0 mm, 0 mm) size (1 mm, 1 mm) shape rect;
      pad 2 at (1 mm, 0 mm) size (1 mm, 1 mm) shape rect;
      bind 1 1;
      bind 2 2;
    }
  }
  dc_source x.footprint X1 {
    voltage 1 V;
    terminals p n;
    connect p N;
    connect n GND;
  }
  board {
    rectangle at (0 mm, 0 mm) size (10 mm, 10 mm);
    place R1 at (2 mm, 2 mm) rotation 0 deg layer front;
  }
}"#;
        let diagnostics = compile_source("semantic-collision.circuitc", source)
            .expect_err("an invalid voltage-source reference must fail");
        let source_reference = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-SIM-007")
            .expect("mapped voltage-source diagnostic must exist");
        assert!(source[source_reference.start..].starts_with("dc_source x.footprint"));
    }

    #[test]
    fn json_diagnostics_are_stable_and_escape_content() {
        let source =
            "design d { net VIN; net VIN; board { rectangle at (0 mm, 0 mm) size (1 mm, 1 mm); } }";
        let diagnostics =
            compile_source("quoted\"name.circuitc", source).expect_err("duplicate net must fail");
        let rendered = render_diagnostics(&diagnostics, DiagnosticFormat::Json);
        assert!(rendered.starts_with("[\n  {\n    \"code\": \"CC-LANG-NET-002\""));
        assert!(rendered.contains("\"filename\": \"quoted\\\"name.circuitc\""));
        assert!(rendered.contains("\"related\": [\n"));
        assert_eq!(
            rendered,
            render_diagnostics(&diagnostics, DiagnosticFormat::Json)
        );
    }

    #[test]
    fn requested_filename_does_not_change_artifacts() {
        let source = "design d { ground GND; dc_source input V1 { voltage 1 V; terminals p n; connect p GND; connect n GND; } board { rectangle at (0 mm, 0 mm) size (1 mm, 1 mm); } }";
        let first = compile_source("relative.circuitc", source).expect("source must compile");
        let second = compile_source("/absolute/elsewhere/input.circuitc", source)
            .expect("source path must not affect compilation");
        assert_eq!(first.elaborated.design, second.elaborated.design);
        assert_eq!(first.artifacts, second.artifacts);
    }

    #[test]
    fn digit_leading_identifiers_resolve_end_to_end() {
        let source = "design d { ground 1G; dc_source input V1 { voltage 1 V; terminals p n; connect p 1G; connect n 1G; } board { rectangle at (0 mm, 0 mm) size (1 mm, 1 mm); } }";
        let compiled = compile_source("digits.circuitc", source)
            .expect("digit-leading canonical identifiers must compile");
        assert!(
            compiled
                .elaborated
                .design
                .nets
                .iter()
                .any(|net| net.name == "1G")
        );
    }

    #[test]
    fn adjacent_line_comments_do_not_enter_semantic_identity() {
        let source = "design d { ground GND// adjacent comment\n; board { rectangle at (0 mm, 0 mm) size (1 mm, 1 mm); } }";
        let compiled = compile_source("comment.circuitc", source)
            .expect("an adjacent line comment must remain trivia");
        assert_eq!(compiled.elaborated.design.nets[0].name, "GND");
    }

    #[test]
    fn requested_filename_is_the_only_path_dependent_diagnostic_field() {
        let source =
            "design d { net VIN; net VIN; board { rectangle at (0 mm, 0 mm) size (1 mm, 1 mm); } }";
        let mut first =
            compile_source("relative.circuitc", source).expect_err("duplicate source must fail");
        let mut second = compile_source("/absolute/elsewhere/input.circuitc", source)
            .expect_err("duplicate source must fail");
        for diagnostic in &mut first {
            diagnostic.filename.clear();
            for related in &mut diagnostic.related {
                related.filename.clear();
            }
        }
        for diagnostic in &mut second {
            diagnostic.filename.clear();
            for related in &mut diagnostic.related {
                related.filename.clear();
            }
        }
        assert_eq!(first, second);
    }
}

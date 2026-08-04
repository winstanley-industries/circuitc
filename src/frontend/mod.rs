mod diagnostic;
mod elaborate;
mod lexer;
mod parser;
mod quantity;
mod syntax;

use std::path::Path;

pub use diagnostic::{DiagnosticFormat, RelatedLocation, SourceDiagnostic, render_diagnostics};
pub use elaborate::{ElaboratedDesign, ProvenanceMap};
pub use syntax::{SourceFile, Span, SyntaxTree};

use crate::{CheckedCompiledArtifacts, CompiledArtifacts, CompiledSimulation};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledSource {
    pub elaborated: ElaboratedDesign,
    pub artifacts: CompiledArtifacts,
    pub kicad_identity_map: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedCompiledSource {
    pub elaborated: ElaboratedDesign,
    pub artifacts: CheckedCompiledArtifacts,
    pub kicad_identity_map: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedSourceError {
    pub diagnostics: Vec<SourceDiagnostic>,
    pub simulations: Vec<CompiledSimulation>,
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
    let logical_source_name = format!("{}.circuitc", elaborated.design.name);
    let kicad_identity_map = render_kicad_identity_map(
        &syntax.source,
        &logical_source_name,
        &elaborated.provenance,
        &artifacts.kicad_identities,
    );
    Ok(CompiledSource {
        elaborated,
        artifacts,
        kicad_identity_map,
    })
}

pub fn compile_source_checked(
    filename: impl Into<String>,
    source: impl Into<String>,
    work_root: &Path,
) -> Result<CheckedCompiledSource, CheckedSourceError> {
    let syntax = parse_source(filename, source).map_err(|diagnostics| CheckedSourceError {
        diagnostics,
        simulations: Vec::new(),
    })?;
    let elaborated = elaborate_syntax(&syntax).map_err(|diagnostics| CheckedSourceError {
        diagnostics,
        simulations: Vec::new(),
    })?;
    let artifacts = crate::compile_checked(&elaborated.design, work_root).map_err(|error| {
        CheckedSourceError {
            diagnostics: elaborate::map_ir_diagnostics(
                &syntax.source,
                &elaborated.provenance,
                error.diagnostics,
            ),
            simulations: error.simulations,
        }
    })?;
    let logical_source_name = format!("{}.circuitc", elaborated.design.name);
    let kicad_identity_map = render_kicad_identity_map(
        &syntax.source,
        &logical_source_name,
        &elaborated.provenance,
        &artifacts.static_artifacts().kicad_identities,
    );
    Ok(CheckedCompiledSource {
        elaborated,
        artifacts,
        kicad_identity_map,
    })
}

fn render_kicad_identity_map(
    source: &SourceFile,
    logical_source_name: &str,
    provenance: &ProvenanceMap,
    identities: &[crate::KicadIdentity],
) -> String {
    let mut output = String::from("{\n  \"schema_version\": 1,\n  \"source\": ");
    write_json_string(&mut output, logical_source_name);
    output.push_str(",\n  \"identities\": [\n");
    for (index, identity) in identities.iter().enumerate() {
        output.push_str("    {\n      \"uuid\": ");
        write_json_string(&mut output, &identity.uuid);
        output.push_str(",\n      \"semantic_path\": ");
        write_json_string(&mut output, &identity.semantic_path);
        if let Some(span) = provenance.span_for_identity(&identity.semantic_path) {
            let (line, column) = source.line_column(span.start);
            output.push_str(",\n      \"location\": {\"start\": ");
            output.push_str(&span.start.to_string());
            output.push_str(", \"end\": ");
            output.push_str(&span.end.to_string());
            output.push_str(", \"line\": ");
            output.push_str(&line.to_string());
            output.push_str(", \"column\": ");
            output.push_str(&column.to_string());
            output.push('}');
        } else {
            output.push_str(",\n      \"location\": null");
        }
        output.push_str("\n    }");
        if index + 1 != identities.len() {
            output.push(',');
        }
        output.push('\n');
    }
    output.push_str("  ]\n}\n");
    output
}

pub(crate) fn elaborate_source_with_kicad_identity_map(
    filename: impl Into<String>,
    source: impl Into<String>,
    identities: &[crate::KicadIdentity],
) -> Result<(ElaboratedDesign, String), Vec<SourceDiagnostic>> {
    let syntax = parse_source(filename, source)?;
    let elaborated = elaborate_syntax(&syntax)?;
    let logical_source_name = format!("{}.circuitc", elaborated.design.name);
    let identity_map = render_kicad_identity_map(
        &syntax.source,
        &logical_source_name,
        &elaborated.provenance,
        identities,
    );
    Ok((elaborated, identity_map))
}

pub(crate) fn write_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                write!(output, "\\u{:04x}", u32::from(character)).unwrap();
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticFormat, compile_source, elaborate_source, render_diagnostics};

    const MINIMAL_VIRTUAL_SOURCE: &str = r#"design d {
  ground GND;
  module root {}
  dc_source root.input V1 {
    part "dc_voltage_source" virtual;
    symbol "CircuitC:VDC" {
      bind p 1 passive;
      bind n 2 passive;
    }
    model "spice:Vdc";
    schematic at (60.96 mm, 81.28 mm) rotation 0 deg;
    voltage 1 V;
    terminals p n;
    connect p GND;
    connect n GND;
  }
  board {
    rectangle at (0 mm, 0 mm) size (1 mm, 1 mm);
  }
}"#;

    #[test]
    fn backend_diagnostics_map_to_authored_route_span() {
        let source = MINIMAL_VIRTUAL_SOURCE.replace(
            "    rectangle at (0 mm, 0 mm) size (1 mm, 1 mm);",
            "    rectangle at (0 mm, 0 mm) size (1 mm, 1 mm);\n    route bad net GND from (0 mm, 0 mm) to (2 mm, 0 mm) width 1 nm layer front;",
        );
        let diagnostics = compile_source("bad-route.circuitc", &source)
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
    fn authored_routing_intent_is_distinct_and_fails_closed_before_execution() {
        let source = include_str!("../../examples/voltage_divider.circuitc").replace(
            "route board.routes.vout_bridge net VOUT from (16 mm, 10 mm) to (24 mm, 10 mm) width 0.25 mm layer front;",
            "autoroute board.autoroute.vout net VOUT width 0.25 mm clearance 0.2 mm grid 0.25 mm layer front;",
        );
        let elaborated = elaborate_source("autoroute.circuitc", &source)
            .expect("valid routing intent must elaborate");
        assert!(elaborated.design.board.routes.is_empty());
        assert_eq!(elaborated.design.board.routing_requests.len(), 1);

        let diagnostics = compile_source("autoroute.circuitc", &source)
            .expect_err("routing intent must not be emitted without checked APGAR execution");
        let phase = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-AUTOROUTE-PHASE-001")
            .expect("fail-closed phase diagnostic must exist");
        assert!(source[phase.start..].starts_with("autoroute board.autoroute.vout"));
    }

    #[test]
    fn structural_identity_collision_reports_outline_and_component_origins() {
        let source = r#"design d {
  ground GND;
  module design {}
  module design.board {}
  dc_source design.board.outline V1 {
    part "dc_voltage_source" virtual;
    symbol "CircuitC:VDC" {
      bind p 1 passive;
      bind n 2 passive;
    }
    model "spice:Vdc";
    schematic at (60.96 mm, 81.28 mm) rotation 0 deg;
    voltage 1 V;
    terminals p n;
    connect p GND;
    connect n GND;
  }
  board {
    rectangle at (0 mm, 0 mm) size (1 mm, 1 mm);
  }
}"#;
        let diagnostics = compile_source("collision.circuitc", source)
            .expect_err("component path colliding with the outline identity must fail");
        let collision = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-KICAD-ID-002")
            .expect("semantic collision diagnostic must exist");
        assert!(source[collision.start..].starts_with("dc_source design.board.outline"));
        assert_eq!(collision.related.len(), 1);
        assert!(source[collision.related[0].start..].starts_with("rectangle"));
    }

    #[test]
    fn synthesized_footprint_collision_reports_both_component_origins() {
        let source = r#"design d {
  net N;
  ground GND;
  module root {}
  module root.x {}
  resistor root.x R1 {
    part "resistor" manufacturer "Yageo" number "RC0603FR-0710KL" package "0603_1608Metric";
    lifecycle active;
    sourcing minimum_available 1 maximum_lead_time_days 365 region "global";
    symbol "CircuitC:R" {
      bind 1 1 passive;
      bind 2 2 passive;
    }
    model "spice:R";
    schematic at (81.28 mm, 81.28 mm) rotation 0 deg;
    resistance 1 kohm;
    terminals 1 2;
    connect 1 N;
    connect 2 GND;
    footprint "CircuitC:R_0603_1608Metric" {
      bind 1 1;
      bind 2 2;
    }
  }
  dc_source root.x.footprint V1 {
    part "dc_voltage_source" virtual;
    symbol "CircuitC:VDC" {
      bind p 1 passive;
      bind n 2 passive;
    }
    model "spice:Vdc";
    schematic at (60.96 mm, 81.28 mm) rotation 0 deg;
    voltage 1 V;
    terminals p n;
    connect p N;
    connect n GND;
  }
  catalog_snapshot "reference-catalog-2026-08-04" sha256 "1631bcee4da9ee39aa8af85f1f80c79331b22bff390a15d4e02b7e3decc2c69e" evaluated_on "2026-08-04";
  variant production build_quantity 1 {
    fit root.x;
  }
  board {
    rectangle at (0 mm, 0 mm) size (10 mm, 10 mm);
    place R1 at (2 mm, 2 mm) rotation 0 deg layer front;
  }
}"#;
        let diagnostics = compile_source("semantic-collision.circuitc", source)
            .expect_err("component path colliding with a generated footprint must fail");
        let collision = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-KICAD-ID-002")
            .expect("semantic collision diagnostic must exist");
        assert!(source[collision.start..].starts_with("dc_source root.x.footprint"));
        assert_eq!(collision.related.len(), 1);
        assert!(source[collision.related[0].start..].starts_with("footprint \"CircuitC:R"));
    }

    #[test]
    fn synthesized_footprint_graphic_collision_reports_both_component_origins() {
        let source = MINIMAL_VIRTUAL_SOURCE
            .replace(
                "  module root {}",
                concat!(
                    "  module root {}\n",
                    "  module root.r {}\n",
                    "  module root.r.footprint {}\n",
                    "  module root.r.footprint.graphic {}",
                ),
            )
            .replace(
                "dc_source root.input V1",
                "dc_source root.r.footprint.graphic.courtyard V1",
            )
            .replace(
                "  board {",
                concat!(
                    "  resistor root.r R1 {\n",
                    "    part \"resistor\" manufacturer \"Yageo\" number \"RC0603FR-0710KL\" package \"0603_1608Metric\";\n",
                    "    lifecycle active;\n",
                    "    sourcing minimum_available 1 maximum_lead_time_days 365 region \"global\";\n",
                    "    symbol \"CircuitC:R\" {\n",
                    "      bind 1 1 passive;\n",
                    "      bind 2 2 passive;\n",
                    "    }\n",
                    "    schematic at (81.28 mm, 81.28 mm) rotation 0 deg;\n",
                    "    resistance 1 kohm;\n",
                    "    connect 1 GND;\n",
                    "    connect 2 GND;\n",
                    "    footprint \"CircuitC:R_0603_1608Metric\" {\n",
                    "      bind 1 1;\n",
                    "      bind 2 2;\n",
                    "    }\n",
                    "  }\n",
                    "  catalog_snapshot \"reference-catalog-2026-08-04\" sha256 \"1631bcee4da9ee39aa8af85f1f80c79331b22bff390a15d4e02b7e3decc2c69e\" evaluated_on \"2026-08-04\";\n",
                    "  variant production build_quantity 1 {\n",
                    "    fit root.r;\n",
                    "  }\n",
                    "  board {",
                ),
            )
            .replace(
                "    rectangle at (0 mm, 0 mm) size (1 mm, 1 mm);",
                concat!(
                    "    rectangle at (0 mm, 0 mm) size (1 mm, 1 mm);\n",
                    "    place R1 at (0.5 mm, 0.5 mm) rotation 0 deg layer front;",
                ),
            );

        let diagnostics = compile_source("graphic-collision.circuitc", &source)
            .expect_err("component path colliding with a graphic identity must fail");
        let collision = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-KICAD-ID-002")
            .expect("graphic collision diagnostic must exist");
        assert_eq!(collision.related.len(), 1);
        let primary = &source[collision.start..];
        let related = &source[collision.related[0].start..];
        assert!(
            (primary.starts_with("dc_source root.r.footprint.graphic.courtyard")
                && related.starts_with("footprint \"CircuitC:R"))
                || (related.starts_with("dc_source root.r.footprint.graphic.courtyard")
                    && primary.starts_with("footprint \"CircuitC:R")),
            "graphic collision must retain both authored locations: {collision:#?}"
        );
    }

    #[test]
    fn ambiguous_generated_identity_provenance_emits_a_null_location() {
        let source = include_str!("../../examples/voltage_divider.circuitc")
            .replace("module divider.analysis", "module divider.r_top")
            .replace(
                "dc_source divider.analysis.input",
                "dc_source divider.r_top.placement",
            );
        let compiled = compile_source("ambiguous-provenance.circuitc", &source)
            .expect("a provenance-only rendered-path collision must compile");

        assert_eq!(
            compiled
                .elaborated
                .provenance
                .span_for_identity("divider.r_top.placement"),
            None,
            "the generated placement owner and authored component must collapse to ambiguity",
        );
        assert!(
            compiled.kicad_identity_map.contains(
                "\"semantic_path\": \"divider.r_top.placement\",\n      \"location\": null"
            )
        );
    }

    #[test]
    fn schematic_anchor_collision_maps_both_authored_components() {
        let source = include_str!("../../examples/voltage_divider.circuitc").replacen(
            "schematic at (101.6 mm, 81.28 mm)",
            "schematic at (81.28 mm, 81.28 mm)",
            1,
        );
        let diagnostics = compile_source("schematic-anchor-collision.circuitc", &source)
            .expect_err("duplicate schematic anchors must fail source compilation");
        let collision = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-SCHEMATIC-003")
            .expect("mapped schematic anchor collision must exist");
        assert!(source[collision.start..].starts_with("resistor divider.r_top R1"));
        assert_eq!(collision.related.len(), 1);
        assert!(source[collision.related[0].start..].starts_with("resistor divider.r_bottom R2"));
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
        let first = compile_source("relative.circuitc", MINIMAL_VIRTUAL_SOURCE)
            .expect("source must compile");
        let second = compile_source("/absolute/elsewhere/input.circuitc", MINIMAL_VIRTUAL_SOURCE)
            .expect("source path must not affect compilation");
        assert_eq!(first.elaborated, second.elaborated);
        assert_eq!(first.artifacts, second.artifacts);
        assert_eq!(first.kicad_identity_map, second.kicad_identity_map);
        assert!(
            first
                .artifacts
                .kicad_project
                .contains("\"filename\": \"d.kicad_pro\"")
        );
        assert!(
            first
                .kicad_identity_map
                .contains("\"source\": \"d.circuitc\"")
        );
    }

    #[test]
    fn virtual_only_bundle_tables_reference_only_published_libraries() {
        let compiled = compile_source("virtual.circuitc", MINIMAL_VIRTUAL_SOURCE)
            .expect("virtual-only source must compile");
        assert_eq!(
            compiled
                .artifacts
                .kicad_library_files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["CircuitC.kicad_sym"]
        );
        assert!(
            compiled
                .artifacts
                .kicad_symbol_table
                .contains("${KIPRJMOD}/CircuitC.kicad_sym")
        );
        assert_eq!(
            compiled.artifacts.kicad_footprint_table,
            "(fp_lib_table\n  (version 7)\n)\n"
        );
        assert!(
            !compiled
                .artifacts
                .kicad_footprint_table
                .contains("CircuitC.pretty")
        );
    }

    #[test]
    fn source_semantic_collision_reports_both_authored_entities() {
        let source = include_str!("../../examples/voltage_divider.circuitc")
            .replace("board.routes.vout_bridge", "divider.r_top");
        let diagnostics = compile_source("collision.circuitc", &source)
            .expect_err("route/component identity collision must fail before emission");
        let collision = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-KICAD-ID-002")
            .expect("source collision diagnostic must exist");
        assert!(source[collision.start..].starts_with("route divider.r_top"));
        assert_eq!(collision.related.len(), 1);
        assert!(source[collision.related[0].start..].starts_with("resistor divider.r_top R1"));
    }

    #[test]
    fn kicad_identity_manifest_maps_uuid_to_authored_component_span() {
        let source = include_str!("../../examples/voltage_divider.circuitc");
        let compiled = compile_source("examples/voltage_divider.circuitc", source)
            .expect("reference source must compile");
        let component_start = source
            .find("resistor divider.r_top R1")
            .expect("reference component exists");
        let span = compiled
            .elaborated
            .provenance
            .span_for("divider.r_top")
            .expect("component provenance exists");
        assert_eq!(span.start, component_start);
        let prefix = &source[..span.start];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let column = prefix
            .rsplit_once('\n')
            .map_or(prefix.len(), |(_, tail)| tail.len())
            + 1;
        assert!(
            compiled
                .kicad_identity_map
                .contains("\"semantic_path\": \"divider.r_top\"")
        );
        assert!(
            compiled
                .kicad_identity_map
                .contains(&format!("\"start\": {component_start}, \"end\":"))
        );
        assert!(
            compiled
                .kicad_identity_map
                .contains(&format!("\"line\": {line}, \"column\": {column}"))
        );
        assert!(
            compiled
                .kicad_identity_map
                .contains("\"semantic_path\": \"board.routes.vout_bridge\"")
        );
        assert!(
            !compiled
                .kicad_identity_map
                .contains("design.board.routes.board.routes.vout_bridge")
        );
    }

    #[test]
    fn digit_leading_identifiers_resolve_end_to_end() {
        let source = MINIMAL_VIRTUAL_SOURCE
            .replace("ground GND", "ground 1G")
            .replace("connect p GND", "connect p 1G")
            .replace("connect n GND", "connect n 1G");
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
        let source = "design d { ground GND// adjacent comment\n; module root {} board { rectangle at (0 mm, 0 mm) size (1 mm, 1 mm); } }";
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

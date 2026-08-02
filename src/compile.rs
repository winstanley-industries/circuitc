use std::collections::BTreeMap;
use std::fmt;

use crate::design::{Design, Diagnostic};
use crate::spice::SpiceNameMap;
use crate::{kicad, spice};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KicadIdentity {
    pub uuid: String,
    pub semantic_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KicadLibraryFile {
    pub relative_path: String,
    pub contents: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledArtifacts {
    pub kicad_schematic: String,
    pub kicad_pcb: String,
    pub kicad_project: String,
    pub kicad_library_files: Vec<KicadLibraryFile>,
    pub kicad_symbol_table: String,
    pub kicad_footprint_table: String,
    pub kicad_identities: Vec<KicadIdentity>,
    pub spice: String,
    pub spice_name_map: SpiceNameMap,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileError {
    pub diagnostics: Vec<Diagnostic>,
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index != 0 {
                formatter.write_str("\n")?;
            }
            write!(formatter, "{diagnostic}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CompileError {}

pub fn compile(design: &Design) -> Result<CompiledArtifacts, CompileError> {
    design
        .validate()
        .map_err(|diagnostics| CompileError { diagnostics })?;
    let backend_diagnostics = kicad::validate(design);
    if !backend_diagnostics.is_empty() {
        return Err(CompileError {
            diagnostics: backend_diagnostics,
        });
    }
    let lowered_spice = spice::lower_netlist(design);
    let kicad_library_files = kicad_library_files(design);
    let project = kicad::emit_project(design, &kicad_library_files);
    Ok(CompiledArtifacts {
        kicad_schematic: project.schematic,
        kicad_pcb: project.board,
        kicad_project: project.project,
        kicad_library_files,
        kicad_symbol_table: project.symbol_table,
        kicad_footprint_table: project.footprint_table,
        kicad_identities: project.identities,
        spice: lowered_spice.netlist,
        spice_name_map: lowered_spice.name_map,
    })
}

fn kicad_library_files(design: &Design) -> Vec<KicadLibraryFile> {
    let mut files = BTreeMap::new();
    for component in &design.components {
        let symbol = crate::library::symbol_library_file(&component.symbol.library_id)
            .expect("validated catalog symbol must have a publishable library file");
        files.insert(symbol.relative_path, symbol.contents);
        if let Some(physical) = &component.physical {
            let footprint = crate::library::footprint_library_file(&physical.footprint.library_id)
                .expect("validated catalog footprint must have a publishable library file");
            files.insert(footprint.relative_path, footprint.contents);
        }
    }
    files
        .into_iter()
        .map(|(relative_path, contents)| KicadLibraryFile {
            relative_path: relative_path.to_owned(),
            contents: contents.to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::panic::catch_unwind;

    use crate::demo::voltage_divider;
    use crate::design::{ConnectionState, ModuleInstance};

    use super::compile;

    #[test]
    fn compiles_reference_design_deterministically() {
        let design = voltage_divider();
        let first = compile(&design).expect("reference design must compile");
        let second = compile(&design).expect("reference design must compile repeatedly");
        assert_eq!(first, second);

        assert!(first.kicad_pcb.starts_with("(kicad_pcb\n"));
        assert!(first.kicad_pcb.contains("(generator \"circuitc\")"));
        assert!(first.kicad_pcb.contains("(net \"VOUT\")"));
        assert!(first.spice.contains("R1 VIN VOUT 10e3"));
        assert!(first.spice.contains("V1 VIN 0 DC 10"));
        assert_eq!(
            first
                .kicad_library_files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "CircuitC.kicad_sym",
                "CircuitC.pretty/R_0603_1608Metric.kicad_mod"
            ]
        );
    }

    #[test]
    fn schematic_embeds_catalog_symbols_and_links_board_footprints_by_uuid() {
        let artifacts = compile(&voltage_divider()).expect("reference design must compile");
        let component_identity = artifacts
            .kicad_identities
            .iter()
            .find(|identity| identity.semantic_path == "divider.r_top")
            .expect("reference component identity must exist");
        assert!(
            artifacts
                .kicad_schematic
                .contains(&format!("    (uuid \"{}\")", component_identity.uuid)),
            "schematic symbol must carry the component identity UUID"
        );
        assert!(
            artifacts
                .kicad_pcb
                .contains(&format!("    (path \"/{}\")", component_identity.uuid)),
            "board footprint must link back to the schematic symbol UUID"
        );

        let embedded = balanced_block(&artifacts.kicad_schematic, "  (lib_symbols\n");
        assert!(embedded.contains("(symbol \"CircuitC:R\""));
        assert!(
            embedded.matches("(pin passive line").count() >= 2,
            "embedded resistor definition must retain both catalog pins"
        );
    }

    #[test]
    fn schematic_connectivity_labels_cover_every_connected_symbol_pin() {
        let design = voltage_divider();
        let connected_pin_count = design
            .components
            .iter()
            .flat_map(|component| &component.connections)
            .filter(|connection| matches!(connection.state, ConnectionState::Connected(_)))
            .count();
        let artifacts = compile(&design).expect("reference design must compile");
        let label_count = artifacts
            .kicad_schematic
            .lines()
            .filter(|line| line.trim_start().starts_with("(global_label "))
            .count();

        assert_eq!(label_count, connected_pin_count);
        for net in ["VIN", "VOUT", "GND"] {
            assert!(
                artifacts
                    .kicad_schematic
                    .contains(&format!("  (global_label \"{net}\"")),
                "missing schematic label for {net}"
            );
        }
        assert!(global_label_at(
            &artifacts.kicad_schematic,
            "VIN",
            "81.28 77.47"
        ));
    }

    #[test]
    fn schematic_pin_coordinates_cover_every_orthogonal_rotation() {
        for (rotation, no_connect_at, connected_at) in [
            (90, "77.47 81.28", "85.09 81.28"),
            (180, "81.28 85.09", "81.28 77.47"),
            (270, "85.09 81.28", "77.47 81.28"),
        ] {
            let mut design = voltage_divider();
            let component = design
                .components
                .iter_mut()
                .find(|component| component.reference == "R1")
                .expect("reference resistor exists");
            component.simulation = None;
            component.schematic_placement.rotation_degrees = rotation;
            component
                .connections
                .iter_mut()
                .find(|connection| connection.pin == "1")
                .expect("pin 1 connection exists")
                .state = ConnectionState::NoConnect;

            let artifacts = compile(&design).expect("orthogonally rotated design must compile");
            assert!(
                artifacts
                    .kicad_schematic
                    .contains(&format!("  (no_connect (at {no_connect_at})")),
                "rotation {rotation} emitted the wrong no-connect coordinate"
            );
            assert!(
                global_label_at(&artifacts.kicad_schematic, "VOUT", connected_at),
                "rotation {rotation} emitted the wrong connected-pin coordinate"
            );
        }
    }

    #[test]
    fn schematic_connection_point_collisions_fail_before_emission() {
        let mut design = voltage_divider();
        let r1_position = design.components[0].schematic_placement.position;
        let r2 = design
            .components
            .iter_mut()
            .find(|component| component.reference == "R2")
            .expect("reference resistor exists");
        r2.schematic_placement.position =
            crate::design::PointNm::new(r1_position.x, r1_position.y + 7_620_000);
        r2.connections
            .iter_mut()
            .find(|connection| connection.pin == "1")
            .expect("R2 pin 1 connection exists")
            .state = ConnectionState::Connected("GND".to_owned());

        let diagnostics = compile(&design)
            .expect_err("differently connected schematic pins may not share a point")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-SCHEMATIC-002"),
            "missing schematic collision diagnostic: {diagnostics:#?}"
        );
    }

    #[test]
    fn declaration_permutations_do_not_change_artifacts() {
        let design = voltage_divider();
        let expected = compile(&design).expect("reference design must compile");
        let mut permuted = design;
        permuted.nets.reverse();
        permuted.modules.reverse();
        permuted.components.reverse();
        permuted.board.routes.reverse();
        for module in &mut permuted.modules {
            module.ports.reverse();
        }
        for component in &mut permuted.components {
            component.connections.reverse();
            component.symbol.pins.reverse();
            if let Some(physical) = &mut component.physical {
                physical.footprint.pads.reverse();
                physical.pin_pad_bindings.reverse();
            }
        }
        assert_eq!(
            compile(&permuted).expect("permuted design must compile"),
            expected
        );
    }

    #[test]
    fn explicit_no_connect_emits_schematic_and_kicad_board_intent() {
        let mut design = voltage_divider();
        let component = design
            .components
            .iter_mut()
            .find(|component| component.reference == "R1")
            .expect("reference resistor exists");
        component.simulation = None;
        component.connections[0].state = ConnectionState::NoConnect;
        design.canonicalize();

        let artifacts = compile(&design).expect("physical-only no-connect must compile");
        assert!(artifacts.kicad_schematic.contains("  (no_connect (at "));
        assert!(artifacts.kicad_identities.iter().any(|identity| {
            identity.semantic_path == "divider.r_top.connection.1"
                && artifacts.kicad_schematic.contains(&identity.uuid)
        }));

        let footprint_start = artifacts
            .kicad_pcb
            .find("(property \"Reference\" \"R1\"")
            .expect("R1 footprint must exist");
        let footprint_end = artifacts.kicad_pcb[footprint_start..]
            .find("\n  )")
            .map(|offset| footprint_start + offset)
            .expect("R1 footprint must terminate");
        let footprint = &artifacts.kicad_pcb[footprint_start..footprint_end];
        let pad = pad_stanza(footprint, "1");
        assert!(
            pad.contains("(net \"unconnected-(R1-Pad1)\")"),
            "no-connect pad must receive KiCad's deterministic parity-only net"
        );
        assert!(
            !design
                .nets
                .iter()
                .any(|net| net.name.contains("unconnected-"))
        );

        let connected_pad = pad_stanza(footprint, "2");
        assert!(connected_pad.contains("(net \"VOUT\")"));
    }

    #[test]
    fn source_authored_physical_no_connect_fixture_compiles() {
        let source = include_str!("../examples/physical_no_connect.circuitc");
        let compiled = crate::frontend::compile_source("physical_no_connect.circuitc", source)
            .expect("source-authored physical no-connect fixture must compile");
        let component = compiled
            .elaborated
            .design
            .components
            .iter()
            .find(|component| component.reference == "R1")
            .expect("physical-only resistor must exist");
        assert!(component.simulation.is_none());
        assert!(
            component
                .connections
                .iter()
                .any(|connection| matches!(&connection.state, ConnectionState::Connected(net) if net == "TEST"))
        );
        assert!(
            component
                .connections
                .iter()
                .any(|connection| connection.state == ConnectionState::NoConnect)
        );
        assert!(!compiled.artifacts.spice.contains("R1 "));
        assert!(global_label_at(
            &compiled.artifacts.kicad_schematic,
            "TEST",
            "77.47 81.28"
        ));
        assert!(
            compiled
                .artifacts
                .kicad_schematic
                .contains("  (no_connect (at 85.09 81.28)")
        );

        let footprint_start = compiled
            .artifacts
            .kicad_pcb
            .find("(property \"Reference\" \"R1\"")
            .expect("R1 footprint must exist");
        let footprint = &compiled.artifacts.kicad_pcb[footprint_start..];
        assert!(pad_stanza(footprint, "1").contains("(net \"TEST\")"));
        assert!(pad_stanza(footprint, "2").contains("(net \"unconnected-(R1-Pad2)\")"));
    }

    #[test]
    fn compile_returns_diagnostics_instead_of_panicking_on_extreme_coordinates() {
        let mut design = voltage_divider();
        let physical = design.components[0]
            .physical
            .as_mut()
            .expect("reference resistor is physical");
        physical.placement.rotation_degrees = 90;
        physical.footprint.pads[0].offset.x = i64::MIN;
        let result = catch_unwind(|| compile(&design));
        assert!(
            result.is_ok(),
            "compile must be total over public IR values"
        );
        assert!(result.expect("checked above").is_err());

        let mut design = voltage_divider();
        design.board.outline.origin.x = i64::MAX;
        design.board.outline.size.width = i64::MAX;
        let result = catch_unwind(|| compile(&design));
        assert!(result.is_ok(), "outline overflow must not panic");
        assert!(result.expect("checked above").is_err());

        let mut design = voltage_divider();
        design.components[0].schematic_placement.position.y = crate::design::MAX_ABS_COORDINATE_NM;
        let result = catch_unwind(|| compile(&design));
        let diagnostics = result
            .expect("derived schematic pin overflow must not panic")
            .expect_err("derived schematic pin beyond the envelope must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-SCHEMATIC-001")
        );
    }

    #[test]
    fn invalid_kicad_catalog_bindings_return_diagnostics_without_panicking() {
        let mut design = voltage_divider();
        design.components[0].part.manufacturer = Some("Texas Instruments".to_owned());
        design.components[0].part.manufacturer_part_number = Some("CC3551EN0UNRGER".to_owned());
        let diagnostics = compile(&design)
            .expect_err("incoherent manufacturer part identity must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-PART-001")
        );

        let mut design = voltage_divider();
        design.components[0].symbol.library_id = "CircuitC:UNKNOWN".to_owned();
        let result = catch_unwind(|| compile(&design));
        let diagnostics = result
            .expect("unknown symbols must not panic")
            .expect_err("unknown symbols must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-SYMBOL-006")
        );

        let mut design = voltage_divider();
        design.components[0].symbol.pins[0].electrical_type =
            crate::design::ElectricalPinType::PowerOutput;
        let diagnostics = compile(&design)
            .expect_err("catalog electrical-type drift must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-SYMBOL-002")
        );

        let mut design = voltage_divider();
        design.components[0].symbol.pins[0].symbol_pin = "3".to_owned();
        let diagnostics = compile(&design)
            .expect_err("missing catalog pin binding must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-SYMBOL-003")
        );

        let mut design = voltage_divider();
        let physical = design.components[0]
            .physical
            .as_mut()
            .expect("reference resistor is physical");
        physical.footprint.pads[0].size.width += 1;
        let diagnostics = compile(&design)
            .expect_err("catalog geometry drift must fail")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-FOOTPRINT-001")
        );

        let mut design = voltage_divider();
        let component = &mut design.components[0];
        let physical = component
            .physical
            .as_mut()
            .expect("reference resistor is physical");
        physical.pin_pad_bindings[0].pad = "2".to_owned();
        physical.pin_pad_bindings[1].pad = "1".to_owned();
        let diagnostics = compile(&design)
            .expect_err("cross-mapped symbol pins and pads must fail closed")
            .diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "CC-KICAD-BIND-001")
        );
    }

    #[test]
    fn coordinate_boundary_matrix_never_panics() {
        let values = [
            i64::MIN,
            -crate::design::MAX_ABS_COORDINATE_NM - 1,
            -crate::design::MAX_ABS_COORDINATE_NM,
            0,
            crate::design::MAX_ABS_COORDINATE_NM,
            crate::design::MAX_ABS_COORDINATE_NM + 1,
            i64::MAX,
        ];
        for rotation in [0, 90, 180, 270] {
            for &value in &values {
                let mut design = voltage_divider();
                let physical = design.components[0]
                    .physical
                    .as_mut()
                    .expect("reference resistor is physical");
                physical.placement.rotation_degrees = rotation;
                physical.placement.position.x = value;
                physical.footprint.pads[0].offset.y = value;
                let result = catch_unwind(|| compile(&design));
                assert!(
                    result.is_ok(),
                    "compile panicked for rotation {rotation} and coordinate {value}"
                );
            }
        }
    }

    #[test]
    fn rejects_component_paths_that_collide_with_generated_kicad_paths() {
        let mut design = voltage_divider();
        design.modules.extend([
            ModuleInstance {
                path: "root".to_owned(),
                ports: Vec::new(),
            },
            ModuleInstance {
                path: "root.x".to_owned(),
                ports: Vec::new(),
            },
            ModuleInstance {
                path: "root.x.footprint".to_owned(),
                ports: Vec::new(),
            },
            ModuleInstance {
                path: "root.x.footprint.pad".to_owned(),
                ports: Vec::new(),
            },
        ]);
        design.components[0].path = "root.x".to_owned();
        design.components[0].module_path = "root".to_owned();
        design.components[1].path = "root.x.footprint.pad.1".to_owned();
        design.components[1].module_path = "root.x.footprint.pad".to_owned();

        let diagnostics = compile(&design)
            .expect_err("rendered KiCad semantic paths must be globally unique")
            .diagnostics;
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "CC-KICAD-ID-002" && diagnostic.path == "root.x.footprint.pad.1"
        }));
    }

    #[test]
    fn rejects_route_paths_that_collide_with_component_paths() {
        let mut design = voltage_divider();
        design.board.routes[0].path = design.components[0].path.clone();

        let diagnostics = compile(&design)
            .expect_err("component and route semantic paths must not collide")
            .diagnostics;
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "CC-KICAD-ID-002" && diagnostic.path == "divider.r_top"
        }));
    }

    #[test]
    fn route_uuid_is_stable_when_geometry_changes() {
        let design = voltage_divider();
        let first = compile(&design).expect("reference design must compile");
        let first_uuid = segment_uuid(&first.kicad_pcb);

        let mut moved = design;
        moved.board.routes[0].start.x += 1;
        let second = compile(&moved).expect("moved route must compile");
        assert_eq!(first_uuid, segment_uuid(&second.kicad_pcb));
    }

    fn board_uuids(board: &str) -> Vec<&str> {
        board
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix("(uuid \"")
                    .and_then(|value| value.strip_suffix("\")"))
            })
            .collect()
    }

    fn pad_stanza<'a>(footprint: &'a str, pad: &str) -> &'a str {
        let marker = format!("    (pad \"{pad}\"");
        let start = footprint.find(&marker).expect("pad stanza must exist");
        let end = footprint[start..]
            .find("\n    )")
            .map(|offset| start + offset + "\n    )".len())
            .expect("pad stanza must terminate");
        &footprint[start..end]
    }

    fn segment_uuid(board: &str) -> &str {
        board
            .split("  (segment\n")
            .nth(1)
            .and_then(|segment| board_uuids(segment).into_iter().next())
            .expect("board must contain a routed segment UUID")
    }

    fn global_label_at(schematic: &str, net: &str, coordinates: &str) -> bool {
        schematic.contains(&format!(
            "  (global_label \"{net}\"\n    (shape bidirectional)\n    (at {coordinates} 0)"
        ))
    }

    fn balanced_block<'a>(text: &'a str, needle: &str) -> &'a str {
        let start = text.find(needle).expect("requested block must exist");
        let mut depth = 0_i32;
        let mut in_string = false;
        let mut escaped = false;
        for (offset, character) in text[start..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    in_string = false;
                }
                continue;
            }
            match character {
                '"' => in_string = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return &text[start..start + offset + character.len_utf8()];
                    }
                }
                _ => {}
            }
        }
        panic!("requested block must be balanced")
    }
}

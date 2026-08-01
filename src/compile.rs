use std::fmt;

use crate::design::{Design, Diagnostic};
use crate::spice::SpiceNameMap;
use crate::{kicad, spice};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledArtifacts {
    pub kicad_pcb: String,
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
    Ok(CompiledArtifacts {
        kicad_pcb: kicad::emit_board(design),
        spice: lowered_spice.netlist,
        spice_name_map: lowered_spice.name_map,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::panic::catch_unwind;

    use crate::demo::voltage_divider;

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
    }

    #[test]
    fn declaration_permutations_do_not_change_artifacts() {
        let design = voltage_divider();
        let expected = compile(&design).expect("reference design must compile");
        let mut permuted = design;
        permuted.nets.reverse();
        permuted.components.reverse();
        permuted.board.routes.reverse();
        for component in &mut permuted.components {
            component.connections.reverse();
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
    fn compile_returns_diagnostics_instead_of_panicking_on_extreme_coordinates() {
        let mut design = voltage_divider();
        let physical = design.components[0]
            .physical
            .as_mut()
            .expect("reference resistor is physical");
        physical.placement.rotation_degrees = 90;
        physical.footprint.pads[0].offset.y = i64::MIN;
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
    fn emitted_kicad_uuids_are_globally_unique_for_adversarial_paths() {
        let mut design = voltage_divider();
        design.components[0].path = "x".to_owned();
        let first_physical = design.components[0]
            .physical
            .as_mut()
            .expect("reference resistor is physical");
        first_physical.footprint.pads[0].number = "1.footprint".to_owned();
        first_physical.pin_pad_bindings[0].pad = "1.footprint".to_owned();
        design.components[1].path = "x.footprint.pad.1".to_owned();

        let artifacts = compile(&design).expect("adversarial identities must remain distinct");
        let uuids = board_uuids(&artifacts.kicad_pcb);
        let unique: BTreeSet<_> = uuids.iter().copied().collect();
        assert_eq!(uuids.len(), unique.len());
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

    fn segment_uuid(board: &str) -> &str {
        board
            .split("  (segment\n")
            .nth(1)
            .and_then(|segment| board_uuids(segment).into_iter().next())
            .expect("board must contain a routed segment UUID")
    }
}

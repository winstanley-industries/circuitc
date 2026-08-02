use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::compile::KicadIdentity;
use crate::design::{
    Component, ConnectionState, CopperLayer, Design, Diagnostic, PadShape, PointNm,
};

const KICAD_BOARD_FORMAT_VERSION: u32 = 20_260_206;
const KICAD_SCHEMATIC_FORMAT_VERSION: u32 = 20_250_114;
const CIRCUITC_VERSION: &str = "0.1.0";

pub(crate) struct ProjectArtifacts {
    pub schematic: String,
    pub board: String,
    pub project: String,
    pub symbol_table: String,
    pub footprint_table: String,
    pub identities: Vec<KicadIdentity>,
}

pub(crate) fn emit_project(design: &Design) -> ProjectArtifacts {
    ProjectArtifacts {
        schematic: emit_schematic(design),
        board: emit_board(design),
        project: emit_project_file(design),
        symbol_table: "(sym_lib_table\n  (version 7)\n  (lib (name \"CircuitC\")(type \"KiCad\")(uri \"${KIPRJMOD}/CircuitC.kicad_sym\")(options \"\")(descr \"CircuitC vendored symbols\"))\n)\n".to_owned(),
        footprint_table: "(fp_lib_table\n  (version 7)\n  (lib (name \"CircuitC\")(type \"KiCad\")(uri \"${KIPRJMOD}/CircuitC.pretty\")(options \"\")(descr \"CircuitC vendored footprints\"))\n)\n".to_owned(),
        identities: identities(design),
    }
}

pub(crate) fn validate(design: &Design) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    validate_catalog_bindings(design, &mut diagnostics);
    let mut uuids = BTreeMap::new();
    let mut semantic_paths = BTreeMap::new();
    for identity in identities(design) {
        if let Some(first_uuid) =
            semantic_paths.insert(identity.semantic_path.clone(), identity.uuid.clone())
        {
            diagnostics.push(Diagnostic {
                code: "CC-KICAD-ID-002",
                path: identity.semantic_path.clone(),
                message: format!(
                    "generated KiCad semantic path is shared by UUIDs {first_uuid} and {}",
                    identity.uuid
                ),
            });
        }
        if let Some(first_path) =
            uuids.insert(identity.uuid.clone(), identity.semantic_path.clone())
        {
            diagnostics.push(Diagnostic {
                code: "CC-KICAD-ID-001",
                path: identity.semantic_path,
                message: format!(
                    "generated KiCad UUID {} collides with entity {first_path}",
                    identity.uuid
                ),
            });
        }
    }
    diagnostics
}

fn validate_catalog_bindings(design: &Design, diagnostics: &mut Vec<Diagnostic>) {
    for component in &design.components {
        let path = component.path.as_str();
        let Some(part) = crate::library::part(&component.part) else {
            diagnostics.push(Diagnostic {
                code: "CC-KICAD-PART-001",
                path: path.to_owned(),
                message: format!(
                    "part identity {} / {} / {} has no vendored KiCad binding",
                    component.part.logical_device,
                    component
                        .part
                        .manufacturer
                        .as_deref()
                        .unwrap_or("<virtual>"),
                    component
                        .part
                        .manufacturer_part_number
                        .as_deref()
                        .unwrap_or("<virtual>")
                ),
            });
            continue;
        };
        if component.symbol.library_id != part.symbol_library_id {
            diagnostics.push(Diagnostic {
                code: "CC-KICAD-SYMBOL-001",
                path: path.to_owned(),
                message: format!(
                    "logical device {} requires symbol {}",
                    part.logical_device, part.symbol_library_id
                ),
            });
        }
        if let Some(symbol) = crate::library::symbol(&component.symbol.library_id) {
            let bound_pins: BTreeMap<_, _> = component
                .symbol
                .pins
                .iter()
                .map(|binding| (binding.symbol_pin.as_str(), binding))
                .collect();
            for catalog_pin in symbol.pins {
                match bound_pins.get(catalog_pin.number) {
                    Some(binding) if binding.electrical_type == catalog_pin.electrical_type => {}
                    Some(_) => diagnostics.push(Diagnostic {
                        code: "CC-KICAD-SYMBOL-002",
                        path: path.to_owned(),
                        message: format!(
                            "symbol pin {} electrical type differs from the vendored catalog",
                            catalog_pin.number
                        ),
                    }),
                    None => diagnostics.push(Diagnostic {
                        code: "CC-KICAD-SYMBOL-003",
                        path: path.to_owned(),
                        message: format!(
                            "vendored symbol pin {} has no logical binding",
                            catalog_pin.number
                        ),
                    }),
                }
            }
            for binding in &component.symbol.pins {
                if !symbol
                    .pins
                    .iter()
                    .any(|catalog_pin| catalog_pin.number == binding.symbol_pin)
                {
                    diagnostics.push(Diagnostic {
                        code: "CC-KICAD-SYMBOL-004",
                        path: path.to_owned(),
                        message: format!(
                            "symbol pin {} is absent from the vendored catalog",
                            binding.symbol_pin
                        ),
                    });
                }
                match schematic_pin_position(component, &binding.symbol_pin) {
                    Some(position)
                        if position.x.unsigned_abs()
                            <= crate::design::MAX_ABS_COORDINATE_NM as u64
                            && position.y.unsigned_abs()
                                <= crate::design::MAX_ABS_COORDINATE_NM as u64 => {}
                    _ => diagnostics.push(Diagnostic {
                        code: "CC-KICAD-SCHEMATIC-001",
                        path: path.to_owned(),
                        message: format!(
                            "transformed schematic pin {} is outside the coordinate envelope",
                            binding.symbol_pin
                        ),
                    }),
                }
            }
            if symbol.on_board != component.physical.is_some() {
                diagnostics.push(Diagnostic {
                    code: "CC-KICAD-SYMBOL-005",
                    path: path.to_owned(),
                    message: "symbol board participation does not match physical implementation"
                        .to_owned(),
                });
            }
        } else {
            diagnostics.push(Diagnostic {
                code: "CC-KICAD-SYMBOL-006",
                path: path.to_owned(),
                message: format!(
                    "symbol {} is absent from the vendored catalog",
                    component.symbol.library_id
                ),
            });
        }

        match (&component.physical, part.footprint_library_id) {
            (Some(physical), Some(expected)) if physical.footprint.library_id == expected => {
                let mut actual_footprint = physical.footprint.clone();
                actual_footprint
                    .pads
                    .sort_by(|left, right| left.number.cmp(&right.number));
                let Some(mut expected_footprint) = crate::library::footprint(expected) else {
                    diagnostics.push(Diagnostic {
                        code: "CC-KICAD-FOOTPRINT-005",
                        path: path.to_owned(),
                        message: format!("part catalog footprint {expected} is unavailable"),
                    });
                    continue;
                };
                expected_footprint
                    .pads
                    .sort_by(|left, right| left.number.cmp(&right.number));
                if actual_footprint != expected_footprint {
                    diagnostics.push(Diagnostic {
                        code: "CC-KICAD-FOOTPRINT-001",
                        path: path.to_owned(),
                        message: format!(
                            "footprint {expected} geometry differs from the vendored catalog"
                        ),
                    });
                }
                for binding in &physical.pin_pad_bindings {
                    if let Some(symbol_binding) = component
                        .symbol
                        .pins
                        .iter()
                        .find(|candidate| candidate.pin == binding.pin)
                        && symbol_binding.symbol_pin != binding.pad
                    {
                        diagnostics.push(Diagnostic {
                            code: "CC-KICAD-BIND-001",
                            path: format!("{}.footprint.pad.{}", path, binding.pad),
                            message: format!(
                                "KiCad 10 catalog lowering requires logical pin {} to use matching symbol pin and pad numbers; found symbol pin {} and pad {}",
                                binding.pin, symbol_binding.symbol_pin, binding.pad
                            ),
                        });
                    }
                }
            }
            (Some(_), Some(expected)) => diagnostics.push(Diagnostic {
                code: "CC-KICAD-FOOTPRINT-002",
                path: path.to_owned(),
                message: format!(
                    "logical device {} requires footprint {expected}",
                    part.logical_device
                ),
            }),
            (Some(_), None) => diagnostics.push(Diagnostic {
                code: "CC-KICAD-FOOTPRINT-003",
                path: path.to_owned(),
                message: format!(
                    "logical device {} does not support a physical footprint",
                    part.logical_device
                ),
            }),
            (None, Some(expected)) => diagnostics.push(Diagnostic {
                code: "CC-KICAD-FOOTPRINT-004",
                path: path.to_owned(),
                message: format!(
                    "logical device {} requires footprint {expected}",
                    part.logical_device
                ),
            }),
            (None, None) => {}
        }
    }
}

fn identities(design: &Design) -> Vec<KicadIdentity> {
    let mut result = Vec::new();
    let mut register = |semantic_path: String, kind: &str, fields: &[&str]| {
        result.push(KicadIdentity {
            uuid: stable_uuid(&design.name, kind, fields),
            semantic_path,
        });
    };
    register("design.schematic".to_owned(), "schematic-root", &[]);
    register("design.board.outline".to_owned(), "board-outline", &[]);
    for component in &design.components {
        register(
            component.path.clone(),
            "schematic-symbol",
            &[&component.path],
        );
        for pin in &component.symbol.pins {
            register(
                format!("{}.symbol.pin.{}", component.path, pin.pin),
                "schematic-pin",
                &[&component.path, &pin.pin],
            );
            let kind = match component.connection_for_pin(&pin.pin) {
                Some(ConnectionState::Connected(_)) => "schematic-global-label",
                Some(ConnectionState::NoConnect) => "schematic-no-connect",
                None => "schematic-missing-connection",
            };
            register(
                format!("{}.connection.{}", component.path, pin.pin),
                kind,
                &[&component.path, &pin.pin],
            );
        }
        if let Some(physical) = &component.physical {
            register(
                format!("{}.footprint", component.path),
                "footprint",
                &[&component.path],
            );
            for property in [
                "Reference",
                "Value",
                "Datasheet",
                "Description",
                "Manufacturer",
                "MPN",
            ] {
                register(
                    format!("{}.footprint.property.{property}", component.path),
                    "footprint-property",
                    &[&component.path, property],
                );
            }
            for pad in &physical.footprint.pads {
                register(
                    format!("{}.footprint.pad.{}", component.path, pad.number),
                    "footprint-pad",
                    &[&component.path, &pad.number],
                );
            }
            if let Some(graphics) =
                crate::library::footprint_graphics(&physical.footprint.library_id)
            {
                for line in graphics.silkscreen_lines {
                    register(
                        format!(
                            "{}.footprint.graphic.silkscreen.{}",
                            component.path, line.semantic_name
                        ),
                        "footprint-silkscreen-line",
                        &[&component.path, line.semantic_name],
                    );
                }
                register(
                    format!("{}.footprint.graphic.courtyard", component.path),
                    "footprint-courtyard",
                    &[&component.path],
                );
            }
        }
    }
    for route in &design.board.routes {
        register(route.path.clone(), "route-segment", &[&route.path]);
    }
    result.sort_by(|left, right| {
        (&left.semantic_path, &left.uuid).cmp(&(&right.semantic_path, &right.uuid))
    });
    result
}

fn emit_schematic(design: &Design) -> String {
    let root_uuid = stable_uuid(&design.name, "schematic-root", &[]);
    let mut output = String::new();
    writeln!(output, "(kicad_sch").unwrap();
    writeln!(output, "  (version {KICAD_SCHEMATIC_FORMAT_VERSION})").unwrap();
    output.push_str("  (generator \"circuitc\")\n");
    writeln!(output, "  (generator_version \"{CIRCUITC_VERSION}\")").unwrap();
    writeln!(output, "  (uuid \"{root_uuid}\")").unwrap();
    output.push_str("  (paper \"A4\")\n");
    output.push_str("  (lib_symbols\n");
    let used_symbols: BTreeSet<_> = design
        .components
        .iter()
        .map(|component| component.symbol.library_id.as_str())
        .collect();
    for library_id in used_symbols {
        let definition = crate::library::symbol(library_id)
            .expect("validated symbol binding must resolve in the vendored catalog");
        let block = extract_symbol_definition(crate::library::SYMBOL_LIBRARY, definition.name)
            .expect("vendored catalog symbol must be extractable");
        let qualified = block.replacen(
            &format!("(symbol \"{}\"", definition.name),
            &format!("(symbol {library_id}", library_id = quoted(library_id)),
            1,
        );
        for line in qualified.lines() {
            writeln!(output, "    {line}").unwrap();
        }
    }
    output.push_str("  )\n");

    let mut components: Vec<_> = design.components.iter().collect();
    components.sort_by(|left, right| left.path.cmp(&right.path));
    for component in &components {
        let definition = crate::library::symbol(&component.symbol.library_id)
            .expect("validated symbol binding must resolve");
        let mut bindings: Vec<_> = component.symbol.pins.iter().collect();
        bindings.sort_by(|left, right| left.pin.cmp(&right.pin));
        for binding in bindings {
            let position = schematic_pin_position(component, binding.symbol_pin.as_str())
                .expect("validated symbol pin must resolve and transform");
            match component
                .connection_for_pin(&binding.pin)
                .expect("validated symbol pin must have a connection state")
            {
                ConnectionState::Connected(net) => {
                    writeln!(output, "  (global_label {}", quoted(net)).unwrap();
                    output.push_str("    (shape bidirectional)\n");
                    writeln!(
                        output,
                        "    (at {} {} 0)",
                        millimeters(position.x),
                        millimeters(position.y)
                    )
                    .unwrap();
                    output.push_str("    (fields_autoplaced yes)\n");
                    output.push_str("    (effects (font (size 1.27 1.27)) (justify left))\n");
                    writeln!(
                        output,
                        "    (uuid \"{}\")",
                        stable_uuid(
                            &design.name,
                            "schematic-global-label",
                            &[&component.path, &binding.pin]
                        )
                    )
                    .unwrap();
                    writeln!(
                        output,
                        "    (property \"Intersheetrefs\" \"${{INTERSHEET_REFS}}\" (at {} {} 0) (effects (font (size 1.27 1.27)) (hide yes)))",
                        millimeters(position.x),
                        millimeters(position.y)
                    )
                    .unwrap();
                    output.push_str("  )\n");
                }
                ConnectionState::NoConnect => {
                    writeln!(
                        output,
                        "  (no_connect (at {} {}) (uuid \"{}\"))",
                        millimeters(position.x),
                        millimeters(position.y),
                        stable_uuid(
                            &design.name,
                            "schematic-no-connect",
                            &[&component.path, &binding.pin]
                        )
                    )
                    .unwrap();
                }
            }
        }
        debug_assert_eq!(definition.on_board, component.physical.is_some());
    }

    for component in components {
        emit_schematic_symbol(&mut output, design, component, &root_uuid);
    }
    output.push_str("  (sheet_instances\n    (path \"/\" (page \"1\"))\n  )\n");
    output.push_str("  (embedded_fonts no)\n)\n");
    output
}

fn emit_schematic_symbol(
    output: &mut String,
    design: &Design,
    component: &Component,
    root_uuid: &str,
) {
    let position = component.schematic_placement.position;
    let on_board = if component.physical.is_some() {
        "yes"
    } else {
        "no"
    };
    let in_bom = if component.physical.is_some() {
        "yes"
    } else {
        "no"
    };
    writeln!(output, "  (symbol").unwrap();
    writeln!(
        output,
        "    (lib_id {})",
        quoted(&component.symbol.library_id)
    )
    .unwrap();
    writeln!(
        output,
        "    (at {} {} {})",
        millimeters(position.x),
        millimeters(position.y),
        component
            .schematic_placement
            .rotation_degrees
            .rem_euclid(360)
    )
    .unwrap();
    output.push_str("    (unit 1)\n    (exclude_from_sim no)\n");
    writeln!(output, "    (in_bom {in_bom})").unwrap();
    writeln!(output, "    (on_board {on_board})").unwrap();
    output.push_str("    (dnp no)\n");
    writeln!(
        output,
        "    (uuid \"{}\")",
        stable_uuid(&design.name, "schematic-symbol", &[&component.path])
    )
    .unwrap();
    emit_schematic_property(output, "Reference", &component.reference, position, false);
    emit_schematic_property(output, "Value", &component.value_label(), position, false);
    let footprint = component
        .physical
        .as_ref()
        .map_or("", |physical| physical.footprint.library_id.as_str());
    emit_schematic_property(output, "Footprint", footprint, position, true);
    emit_schematic_property(output, "Datasheet", "", position, true);
    emit_schematic_property(
        output,
        "Description",
        &component.part.logical_device,
        position,
        true,
    );
    if let Some(manufacturer) = &component.part.manufacturer {
        emit_schematic_property(output, "Manufacturer", manufacturer, position, true);
    }
    if let Some(number) = &component.part.manufacturer_part_number {
        emit_schematic_property(output, "MPN", number, position, true);
    }
    let mut pins: Vec<_> = component.symbol.pins.iter().collect();
    pins.sort_by(|left, right| left.symbol_pin.cmp(&right.symbol_pin));
    for pin in pins {
        writeln!(output, "    (pin {}", quoted(&pin.symbol_pin)).unwrap();
        writeln!(
            output,
            "      (uuid \"{}\")",
            stable_uuid(&design.name, "schematic-pin", &[&component.path, &pin.pin])
        )
        .unwrap();
        output.push_str("    )\n");
    }
    output.push_str("    (instances\n");
    writeln!(output, "      (project {}", quoted(&design.name)).unwrap();
    writeln!(output, "        (path \"/{root_uuid}\"").unwrap();
    writeln!(
        output,
        "          (reference {})",
        quoted(&component.reference)
    )
    .unwrap();
    output.push_str("          (unit 1)\n        )\n      )\n    )\n  )\n");
}

fn emit_schematic_property(
    output: &mut String,
    name: &str,
    value: &str,
    position: PointNm,
    hidden: bool,
) {
    writeln!(output, "    (property {} {}", quoted(name), quoted(value)).unwrap();
    writeln!(
        output,
        "      (at {} {} 0)",
        millimeters(position.x),
        millimeters(position.y)
    )
    .unwrap();
    output.push_str("      (effects (font (size 1.27 1.27))");
    if hidden {
        output.push_str(" (hide yes)");
    }
    output.push_str(")\n    )\n");
}

fn schematic_pin_position(component: &Component, symbol_pin: &str) -> Option<PointNm> {
    let definition = crate::library::symbol(&component.symbol.library_id)?;
    let offset = definition
        .pins
        .iter()
        .find(|pin| pin.number == symbol_pin)?
        .offset;
    let rotated = match component
        .schematic_placement
        .rotation_degrees
        .rem_euclid(360)
    {
        0 => offset,
        90 => PointNm::new(offset.y.checked_neg()?, offset.x),
        180 => PointNm::new(offset.x.checked_neg()?, offset.y.checked_neg()?),
        270 => PointNm::new(offset.y, offset.x.checked_neg()?),
        _ => return None,
    };
    Some(PointNm::new(
        component
            .schematic_placement
            .position
            .x
            .checked_add(rotated.x)?,
        component
            .schematic_placement
            .position
            .y
            .checked_add(rotated.y)?,
    ))
}

fn extract_symbol_definition<'a>(library: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("(symbol \"{name}\"");
    let start = library.find(&needle)?;
    let bytes = library.as_bytes();
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for index in start..bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&library[start..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

fn emit_project_file(design: &Design) -> String {
    format!(
        "{{\n  \"board\": {{}},\n  \"boards\": [],\n  \"cvpcb\": {{}},\n  \"erc\": {{}},\n  \"libraries\": {{\"pinned_footprint_libs\": [], \"pinned_symbol_libs\": []}},\n  \"meta\": {{\"filename\": \"{}.kicad_pro\", \"version\": 1}},\n  \"net_settings\": {{}},\n  \"pcbnew\": {{}},\n  \"schematic\": {{}},\n  \"sheets\": [],\n  \"text_variables\": {{}}\n}}\n",
        design.name
    )
}

pub(crate) fn emit_board(design: &Design) -> String {
    let mut output = String::new();
    writeln!(output, "(kicad_pcb").unwrap();
    writeln!(output, "  (version {KICAD_BOARD_FORMAT_VERSION})").unwrap();
    writeln!(output, "  (generator \"circuitc\")").unwrap();
    writeln!(output, "  (generator_version \"{CIRCUITC_VERSION}\")").unwrap();
    output.push_str("  (general\n    (thickness 1.6)\n    (legacy_teardrops no)\n  )\n");
    output.push_str("  (paper \"A4\")\n");
    output.push_str(
        "  (layers\n    (0 \"F.Cu\" signal)\n    (2 \"B.Cu\" signal)\n    (25 \"Edge.Cuts\" user)\n    (27 \"Margin\" user)\n    (31 \"F.CrtYd\" user \"F.Courtyard\")\n    (29 \"B.CrtYd\" user \"B.Courtyard\")\n  )\n",
    );
    output.push_str(
        "  (setup\n    (pad_to_mask_clearance 0)\n    (allow_soldermask_bridges_in_footprints no)\n  )\n",
    );

    let outline = design.board.outline;
    let outline_end = PointNm::new(
        outline
            .origin
            .x
            .checked_add(outline.size.width)
            .expect("validated outline x coordinate must not overflow"),
        outline
            .origin
            .y
            .checked_add(outline.size.height)
            .expect("validated outline y coordinate must not overflow"),
    );
    writeln!(output, "  (gr_rect").unwrap();
    writeln!(
        output,
        "    (start {} {})",
        millimeters(outline.origin.x),
        millimeters(outline.origin.y)
    )
    .unwrap();
    writeln!(
        output,
        "    (end {} {})",
        millimeters(outline_end.x),
        millimeters(outline_end.y)
    )
    .unwrap();
    output.push_str("    (stroke (width 0.05) (type default))\n");
    output.push_str("    (fill none)\n");
    output.push_str("    (layer \"Edge.Cuts\")\n");
    writeln!(
        output,
        "    (uuid \"{}\")",
        stable_uuid(&design.name, "board-outline", &[])
    )
    .unwrap();
    output.push_str("  )\n");

    let mut components: Vec<&Component> = design
        .components
        .iter()
        .filter(|component| component.physical.is_some())
        .collect();
    components.sort_by(|left, right| left.reference.cmp(&right.reference));
    for component in components {
        emit_footprint(&mut output, design, component);
    }

    let mut routes: Vec<_> = design.board.routes.iter().collect();
    routes.sort_by_key(|route| route.path.as_str());
    for route in routes {
        output.push_str("  (segment\n");
        writeln!(
            output,
            "    (start {} {})",
            millimeters(route.start.x),
            millimeters(route.start.y)
        )
        .unwrap();
        writeln!(
            output,
            "    (end {} {})",
            millimeters(route.end.x),
            millimeters(route.end.y)
        )
        .unwrap();
        writeln!(output, "    (width {})", millimeters(route.width_nm)).unwrap();
        writeln!(output, "    (layer \"{}\")", layer_name(route.layer)).unwrap();
        writeln!(output, "    (net {})", quoted(&route.net)).unwrap();
        writeln!(
            output,
            "    (uuid \"{}\")",
            stable_uuid(&design.name, "route-segment", &[&route.path])
        )
        .unwrap();
        output.push_str("  )\n");
    }

    output.push_str("  (embedded_fonts no)\n)\n");
    output
}

fn emit_footprint(output: &mut String, design: &Design, component: &Component) {
    let physical = component
        .physical
        .as_ref()
        .expect("filtered physical component must have an implementation");
    let layer = layer_name(physical.placement.layer);
    writeln!(
        output,
        "  (footprint {}",
        quoted(&physical.footprint.library_id)
    )
    .unwrap();
    writeln!(output, "    (layer \"{layer}\")").unwrap();
    writeln!(
        output,
        "    (uuid \"{}\")",
        stable_uuid(&design.name, "footprint", &[&component.path])
    )
    .unwrap();
    writeln!(
        output,
        "    (at {} {} {})",
        millimeters(physical.placement.position.x),
        millimeters(physical.placement.position.y),
        physical.placement.rotation_degrees.rem_euclid(360)
    )
    .unwrap();
    emit_property(
        output,
        design,
        component,
        "Reference",
        &component.reference,
        PointNm::new(0, -1_500_000),
        silk_layer(physical.placement.layer),
        false,
    );
    emit_property(
        output,
        design,
        component,
        "Value",
        &component.value_label(),
        PointNm::new(0, 1_500_000),
        fab_layer(physical.placement.layer),
        false,
    );
    emit_property(
        output,
        design,
        component,
        "Datasheet",
        "",
        PointNm::new(0, 0),
        fab_layer(physical.placement.layer),
        true,
    );
    emit_property(
        output,
        design,
        component,
        "Manufacturer",
        component
            .part
            .manufacturer
            .as_deref()
            .expect("validated physical part must have a manufacturer"),
        PointNm::new(0, 0),
        fab_layer(physical.placement.layer),
        true,
    );
    emit_property(
        output,
        design,
        component,
        "MPN",
        component
            .part
            .manufacturer_part_number
            .as_deref()
            .expect("validated physical part must have a manufacturer part number"),
        PointNm::new(0, 0),
        fab_layer(physical.placement.layer),
        true,
    );
    emit_property(
        output,
        design,
        component,
        "Description",
        &component.part.logical_device,
        PointNm::new(0, 0),
        fab_layer(physical.placement.layer),
        true,
    );
    writeln!(
        output,
        "    (path \"/{}\")",
        stable_uuid(&design.name, "schematic-symbol", &[&component.path])
    )
    .unwrap();
    output.push_str("    (sheetname \"/\")\n");
    writeln!(
        output,
        "    (sheetfile {})",
        quoted(&format!("{}.kicad_sch", design.name))
    )
    .unwrap();
    output.push_str("    (attr smd)\n");
    output.push_str("    (duplicate_pad_numbers_are_jumpers no)\n");
    emit_footprint_graphics(output, design, component);

    let mut pads: Vec<_> = physical.footprint.pads.iter().collect();
    pads.sort_by(|left, right| left.number.cmp(&right.number));
    for pad in pads {
        let shape = match pad.shape {
            PadShape::Rect => "rect",
            PadShape::RoundRect => "roundrect",
        };
        writeln!(output, "    (pad {} smd {shape}", quoted(&pad.number)).unwrap();
        writeln!(
            output,
            "      (at {} {})",
            millimeters(pad.offset.x),
            millimeters(pad.offset.y)
        )
        .unwrap();
        writeln!(
            output,
            "      (size {} {})",
            millimeters(pad.size.width),
            millimeters(pad.size.height)
        )
        .unwrap();
        writeln!(
            output,
            "      (layers \"{}\" \"{}\" \"{}\")",
            copper_layer(physical.placement.layer),
            paste_layer(physical.placement.layer),
            mask_layer(physical.placement.layer)
        )
        .unwrap();
        if pad.shape == PadShape::RoundRect {
            output.push_str("      (roundrect_rratio 0.2)\n");
        }
        if let Some(net) = kicad_net_for_pad(component, &pad.number) {
            writeln!(output, "      (net {})", quoted(&net)).unwrap();
        }
        writeln!(
            output,
            "      (uuid \"{}\")",
            stable_uuid(
                &design.name,
                "footprint-pad",
                &[&component.path, &pad.number]
            )
        )
        .unwrap();
        output.push_str("    )\n");
    }
    output.push_str("    (embedded_fonts no)\n  )\n");
}

fn emit_footprint_graphics(output: &mut String, design: &Design, component: &Component) {
    let physical = component
        .physical
        .as_ref()
        .expect("filtered physical component must have an implementation");
    let graphics = crate::library::footprint_graphics(&physical.footprint.library_id)
        .expect("validated catalog footprint must have drawing geometry");
    for line in graphics.silkscreen_lines {
        output.push_str("    (fp_line\n");
        writeln!(
            output,
            "      (start {} {})",
            millimeters(line.start.x),
            millimeters(line.start.y)
        )
        .unwrap();
        writeln!(
            output,
            "      (end {} {})",
            millimeters(line.end.x),
            millimeters(line.end.y)
        )
        .unwrap();
        writeln!(
            output,
            "      (stroke (width {}) (type default))",
            millimeters(line.width_nm)
        )
        .unwrap();
        writeln!(
            output,
            "      (layer \"{}\")",
            silk_layer(physical.placement.layer)
        )
        .unwrap();
        writeln!(
            output,
            "      (uuid \"{}\")",
            stable_uuid(
                &design.name,
                "footprint-silkscreen-line",
                &[&component.path, line.semantic_name]
            )
        )
        .unwrap();
        output.push_str("    )\n");
    }

    output.push_str("    (fp_rect\n");
    writeln!(
        output,
        "      (start {} {})",
        millimeters(graphics.courtyard_start.x),
        millimeters(graphics.courtyard_start.y)
    )
    .unwrap();
    writeln!(
        output,
        "      (end {} {})",
        millimeters(graphics.courtyard_end.x),
        millimeters(graphics.courtyard_end.y)
    )
    .unwrap();
    writeln!(
        output,
        "      (stroke (width {}) (type default))",
        millimeters(graphics.courtyard_width_nm)
    )
    .unwrap();
    output.push_str("      (fill none)\n");
    writeln!(
        output,
        "      (layer \"{}\")",
        courtyard_layer(physical.placement.layer)
    )
    .unwrap();
    writeln!(
        output,
        "      (uuid \"{}\")",
        stable_uuid(&design.name, "footprint-courtyard", &[&component.path])
    )
    .unwrap();
    output.push_str("    )\n");
}

fn kicad_net_for_pad<'a>(component: &'a Component, pad: &str) -> Option<Cow<'a, str>> {
    let pin = component.pin_for_pad(pad)?;
    match component.connection_for_pin(pin)? {
        ConnectionState::Connected(net) => Some(Cow::Borrowed(net)),
        ConnectionState::NoConnect => Some(Cow::Owned(format!(
            "unconnected-({}-Pad{pad})",
            component.reference
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_property(
    output: &mut String,
    design: &Design,
    component: &Component,
    name: &str,
    value: &str,
    position: PointNm,
    layer: &str,
    hidden: bool,
) {
    writeln!(output, "    (property {} {}", quoted(name), quoted(value)).unwrap();
    writeln!(
        output,
        "      (at {} {} 0)",
        millimeters(position.x),
        millimeters(position.y)
    )
    .unwrap();
    writeln!(output, "      (layer \"{layer}\")").unwrap();
    if hidden {
        output.push_str("      (hide yes)\n");
    }
    writeln!(
        output,
        "      (uuid \"{}\")",
        stable_uuid(&design.name, "footprint-property", &[&component.path, name])
    )
    .unwrap();
    output.push_str(
        "      (effects\n        (font\n          (size 1 1)\n          (thickness 0.15)\n        )\n      )\n    )\n",
    );
}

fn layer_name(layer: CopperLayer) -> &'static str {
    match layer {
        CopperLayer::Front => "F.Cu",
        CopperLayer::Back => "B.Cu",
    }
}

fn copper_layer(layer: CopperLayer) -> &'static str {
    layer_name(layer)
}

fn paste_layer(layer: CopperLayer) -> &'static str {
    match layer {
        CopperLayer::Front => "F.Paste",
        CopperLayer::Back => "B.Paste",
    }
}

fn mask_layer(layer: CopperLayer) -> &'static str {
    match layer {
        CopperLayer::Front => "F.Mask",
        CopperLayer::Back => "B.Mask",
    }
}

fn silk_layer(layer: CopperLayer) -> &'static str {
    match layer {
        CopperLayer::Front => "F.SilkS",
        CopperLayer::Back => "B.SilkS",
    }
}

fn courtyard_layer(layer: CopperLayer) -> &'static str {
    match layer {
        CopperLayer::Front => "F.CrtYd",
        CopperLayer::Back => "B.CrtYd",
    }
}

fn fab_layer(layer: CopperLayer) -> &'static str {
    match layer {
        CopperLayer::Front => "F.Fab",
        CopperLayer::Back => "B.Fab",
    }
}

fn quoted(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn millimeters(nanometers: i64) -> String {
    let negative = nanometers.is_negative();
    let magnitude = nanometers.unsigned_abs();
    let integer = magnitude / 1_000_000;
    let remainder = magnitude % 1_000_000;
    let sign = if negative { "-" } else { "" };
    if remainder == 0 {
        return format!("{sign}{integer}");
    }

    let mut fraction = format!("{remainder:06}");
    while fraction.ends_with('0') {
        fraction.pop();
    }
    format!("{sign}{integer}.{fraction}")
}

fn stable_uuid(namespace: &str, entity_kind: &str, identity_fields: &[&str]) -> String {
    let mut identity = Vec::new();
    append_identity_field(&mut identity, "circuitc-kicad-identity-v1");
    append_identity_field(&mut identity, namespace);
    append_identity_field(&mut identity, entity_kind);
    for field in identity_fields {
        append_identity_field(&mut identity, field);
    }

    let first = fnv1a64(0xcbf2_9ce4_8422_2325, &identity);
    let second = fnv1a64(0x8422_2325_cbf2_9ce4 ^ first, &identity);
    let mut bytes = ((u128::from(first) << 64) | u128::from(second)).to_be_bytes();

    // RFC 9562 version 8 reserves the payload for application-defined stable IDs.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn append_identity_field(identity: &mut Vec<u8>, value: &str) {
    let length = u64::try_from(value.len()).expect("Rust strings fit in u64 on supported targets");
    identity.extend_from_slice(&length.to_be_bytes());
    identity.extend_from_slice(value.as_bytes());
}

fn fnv1a64(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = seed;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::{millimeters, stable_uuid};

    #[test]
    fn converts_nanometers_without_floating_point() {
        assert_eq!(millimeters(1_000_000), "1");
        assert_eq!(millimeters(1), "0.000001");
        assert_eq!(millimeters(-1_250_000), "-1.25");
    }

    #[test]
    fn stable_uuid_is_repeatable_and_version_eight() {
        let first = stable_uuid("divider", "footprint-pad", &["r1", "1"]);
        let second = stable_uuid("divider", "footprint-pad", &["r1", "1"]);
        assert_eq!(first, second);
        assert_eq!(&first[14..15], "8");
        assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn stable_uuid_input_is_typed_and_length_delimited() {
        assert_ne!(
            stable_uuid("divider", "footprint", &["a.footprint.pad"]),
            stable_uuid("divider", "footprint-pad", &["a", "footprint"])
        );
        assert_ne!(
            stable_uuid("divider", "test", &["a", "bc"]),
            stable_uuid("divider", "test", &["ab", "c"])
        );
    }
}

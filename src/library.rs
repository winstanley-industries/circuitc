use crate::KicadLibraryFileKind;
use crate::design::{ElectricalPinType, Footprint, Pad, PadShape, PartIdentity, PointNm, SizeNm};

pub(crate) const SYMBOL_LIBRARY: &str = include_str!("../libraries/CircuitC.kicad_sym");
pub(crate) const RESISTOR_FOOTPRINT_LIBRARY: &str =
    include_str!("../libraries/CircuitC.pretty/R_0603_1608Metric.kicad_mod");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LibraryFileDefinition {
    pub kind: KicadLibraryFileKind,
    pub nickname: &'static str,
    pub relative_path: &'static str,
    pub table_relative_path: &'static str,
    pub contents: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SymbolPinDefinition {
    pub number: &'static str,
    pub electrical_type: ElectricalPinType,
    pub offset: PointNm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SymbolDefinition {
    pub library_id: &'static str,
    pub name: &'static str,
    pub pins: &'static [SymbolPinDefinition],
    pub on_board: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PartDefinition {
    pub logical_function: &'static str,
    pub manufacturer: Option<&'static str>,
    pub manufacturer_part_number: Option<&'static str>,
    pub package: Option<&'static str>,
    pub symbol_library_id: &'static str,
    pub footprint_library_id: Option<&'static str>,
}

const PART_DEFINITIONS: &[PartDefinition] = &[
    PartDefinition {
        logical_function: "resistor",
        manufacturer: Some("Yageo"),
        manufacturer_part_number: Some("RC0603FR-0710KL"),
        package: Some("0603_1608Metric"),
        symbol_library_id: "CircuitC:R",
        footprint_library_id: Some("CircuitC:R_0603_1608Metric"),
    },
    PartDefinition {
        logical_function: "dc_voltage_source",
        manufacturer: None,
        manufacturer_part_number: None,
        package: None,
        symbol_library_id: "CircuitC:VDC",
        footprint_library_id: None,
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GraphicLineDefinition {
    pub semantic_name: &'static str,
    pub start: PointNm,
    pub end: PointNm,
    pub width_nm: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FootprintGraphicsDefinition {
    pub silkscreen_lines: &'static [GraphicLineDefinition],
    pub courtyard_start: PointNm,
    pub courtyard_end: PointNm,
    pub courtyard_width_nm: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FootprintPadDefinition {
    number: &'static str,
    offset: PointNm,
    size: SizeNm,
    shape: PadShape,
}

pub(crate) fn part(identity: &PartIdentity) -> Option<PartDefinition> {
    PART_DEFINITIONS.iter().copied().find(|definition| {
        definition.logical_function == identity.logical_function
            && definition.manufacturer == identity.manufacturer.as_deref()
            && definition.manufacturer_part_number == identity.manufacturer_part_number.as_deref()
            && definition.package == identity.package.as_deref()
    })
}

#[cfg(test)]
pub(crate) fn part_definitions() -> &'static [PartDefinition] {
    PART_DEFINITIONS
}

const TWO_PASSIVE_PINS: &[SymbolPinDefinition] = &[
    SymbolPinDefinition {
        number: "1",
        electrical_type: ElectricalPinType::Passive,
        offset: PointNm::new(0, -3_810_000),
    },
    SymbolPinDefinition {
        number: "2",
        electrical_type: ElectricalPinType::Passive,
        offset: PointNm::new(0, 3_810_000),
    },
];

pub(crate) fn symbol(library_id: &str) -> Option<SymbolDefinition> {
    match library_id {
        "CircuitC:R" => Some(SymbolDefinition {
            library_id: "CircuitC:R",
            name: "R",
            pins: TWO_PASSIVE_PINS,
            on_board: true,
        }),
        "CircuitC:VDC" => Some(SymbolDefinition {
            library_id: "CircuitC:VDC",
            name: "VDC",
            pins: TWO_PASSIVE_PINS,
            on_board: false,
        }),
        _ => None,
    }
}

pub(crate) fn symbol_library_file(library_id: &str) -> Option<LibraryFileDefinition> {
    symbol(library_id).map(|_| LibraryFileDefinition {
        kind: KicadLibraryFileKind::Symbol,
        nickname: "CircuitC",
        relative_path: "CircuitC.kicad_sym",
        table_relative_path: "CircuitC.kicad_sym",
        contents: SYMBOL_LIBRARY,
    })
}

pub(crate) fn footprint(library_id: &str) -> Option<Footprint> {
    let pads = footprint_pads(library_id)?;
    Some(Footprint {
        library_id: library_id.to_owned(),
        pads: pads
            .iter()
            .map(|pad| Pad {
                number: pad.number.to_owned(),
                offset: pad.offset,
                size: pad.size,
                shape: pad.shape,
            })
            .collect(),
    })
}

const RESISTOR_PADS: &[FootprintPadDefinition] = &[
    FootprintPadDefinition {
        number: "1",
        offset: PointNm::new(-1_000_000, 0),
        size: SizeNm::new(900_000, 950_000),
        shape: PadShape::RoundRect,
    },
    FootprintPadDefinition {
        number: "2",
        offset: PointNm::new(1_000_000, 0),
        size: SizeNm::new(900_000, 950_000),
        shape: PadShape::RoundRect,
    },
];

fn footprint_pads(library_id: &str) -> Option<&'static [FootprintPadDefinition]> {
    match library_id {
        "CircuitC:R_0603_1608Metric" => Some(RESISTOR_PADS),
        _ => None,
    }
}

pub(crate) fn footprint_geometry_matches_catalog(footprint: &Footprint) -> Option<bool> {
    let expected = footprint_pads(&footprint.library_id)?;
    Some(
        footprint.pads.len() == expected.len()
            && expected.iter().all(|expected_pad| {
                footprint.pads.iter().any(|actual_pad| {
                    actual_pad.number == expected_pad.number
                        && actual_pad.offset == expected_pad.offset
                        && actual_pad.size == expected_pad.size
                        && actual_pad.shape == expected_pad.shape
                })
            }),
    )
}

const RESISTOR_SILKSCREEN: &[GraphicLineDefinition] = &[
    GraphicLineDefinition {
        semantic_name: "top",
        start: PointNm::new(-450_000, -500_000),
        end: PointNm::new(450_000, -500_000),
        width_nm: 120_000,
    },
    GraphicLineDefinition {
        semantic_name: "bottom",
        start: PointNm::new(-450_000, 500_000),
        end: PointNm::new(450_000, 500_000),
        width_nm: 120_000,
    },
];

const RESISTOR_GRAPHICS: FootprintGraphicsDefinition = FootprintGraphicsDefinition {
    silkscreen_lines: RESISTOR_SILKSCREEN,
    courtyard_start: PointNm::new(-1_700_000, -750_000),
    courtyard_end: PointNm::new(1_700_000, 750_000),
    courtyard_width_nm: 50_000,
};

const RESISTOR_LIBRARY_FILE: LibraryFileDefinition = LibraryFileDefinition {
    kind: KicadLibraryFileKind::Footprint,
    nickname: "CircuitC",
    relative_path: "CircuitC.pretty/R_0603_1608Metric.kicad_mod",
    table_relative_path: "CircuitC.pretty",
    contents: RESISTOR_FOOTPRINT_LIBRARY,
};

pub(crate) fn footprint_graphics(library_id: &str) -> Option<FootprintGraphicsDefinition> {
    match library_id {
        "CircuitC:R_0603_1608Metric" => Some(RESISTOR_GRAPHICS),
        _ => None,
    }
}

pub(crate) fn footprint_library_file(library_id: &str) -> Option<LibraryFileDefinition> {
    match library_id {
        "CircuitC:R_0603_1608Metric" => Some(RESISTOR_LIBRARY_FILE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::design::{PadShape, PartIdentity, PointNm, SizeNm};

    use super::{
        RESISTOR_FOOTPRINT_LIBRARY, SYMBOL_LIBRARY, footprint, footprint_graphics,
        footprint_library_file, part, part_definitions, symbol, symbol_library_file,
    };

    fn balanced_block<'a>(text: &'a str, needle: &str) -> &'a str {
        let start = text.find(needle).expect("requested block exists");
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

    #[test]
    fn vendored_assets_match_the_catalog() {
        let resistor_identity = PartIdentity {
            logical_function: "resistor".to_owned(),
            manufacturer: Some("Yageo".to_owned()),
            manufacturer_part_number: Some("RC0603FR-0710KL".to_owned()),
            package: Some("0603_1608Metric".to_owned()),
            lifecycle: None,
            sourcing: None,
            approved_substitutions: Vec::new(),
        };
        let resistor = part(&resistor_identity).expect("resistor part binding must exist");
        assert_eq!(resistor.logical_function, "resistor");
        assert_eq!(resistor.manufacturer, Some("Yageo"));
        assert_eq!(resistor.manufacturer_part_number, Some("RC0603FR-0710KL"));
        assert_eq!(resistor.package, Some("0603_1608Metric"));
        assert_eq!(resistor.symbol_library_id, "CircuitC:R");
        assert_eq!(
            resistor.footprint_library_id,
            Some("CircuitC:R_0603_1608Metric")
        );
        assert_eq!(
            part(&PartIdentity {
                logical_function: "dc_voltage_source".to_owned(),
                manufacturer: None,
                manufacturer_part_number: None,
                package: None,
                lifecycle: None,
                sourcing: None,
                approved_substitutions: Vec::new(),
            })
            .expect("voltage source part binding must exist")
            .footprint_library_id,
            None
        );
        for id in ["CircuitC:R", "CircuitC:VDC"] {
            let definition = symbol(id).expect("catalog symbol must exist");
            assert_eq!(definition.library_id, id);
            assert!(
                SYMBOL_LIBRARY.contains(&format!("(symbol \"{}\"", definition.name)),
                "vendored symbol library must contain {}",
                definition.name
            );
        }
        let footprint =
            footprint("CircuitC:R_0603_1608Metric").expect("catalog footprint must exist");
        assert!(RESISTOR_FOOTPRINT_LIBRARY.contains("(footprint \"R_0603_1608Metric\""));
        assert_eq!(footprint.pads.len(), 2);
        assert_eq!(footprint.pads[0].number, "1");
        assert_eq!(footprint.pads[0].offset, PointNm::new(-1_000_000, 0));
        assert_eq!(footprint.pads[0].size, SizeNm::new(900_000, 950_000));
        assert_eq!(footprint.pads[0].shape, PadShape::RoundRect);
        assert_eq!(footprint.pads[1].number, "2");
        assert_eq!(footprint.pads[1].offset, PointNm::new(1_000_000, 0));
        assert!(
            RESISTOR_FOOTPRINT_LIBRARY
                .contains("(pad \"1\" smd roundrect\n    (at -1 0)\n    (size 0.9 0.95)")
        );
        assert!(
            RESISTOR_FOOTPRINT_LIBRARY
                .contains("(pad \"2\" smd roundrect\n    (at 1 0)\n    (size 0.9 0.95)")
        );
        let graphics = footprint_graphics("CircuitC:R_0603_1608Metric")
            .expect("catalog footprint graphics must exist");
        assert_eq!(graphics.silkscreen_lines.len(), 2);
        assert_eq!(graphics.silkscreen_lines[0].semantic_name, "top");
        assert_eq!(
            graphics.silkscreen_lines[0].start,
            PointNm::new(-450_000, -500_000)
        );
        assert_eq!(
            graphics.silkscreen_lines[0].end,
            PointNm::new(450_000, -500_000)
        );
        assert_eq!(graphics.silkscreen_lines[0].width_nm, 120_000);
        assert_eq!(graphics.silkscreen_lines[1].semantic_name, "bottom");
        assert_eq!(
            graphics.silkscreen_lines[1].start,
            PointNm::new(-450_000, 500_000)
        );
        assert_eq!(
            graphics.silkscreen_lines[1].end,
            PointNm::new(450_000, 500_000)
        );
        assert_eq!(graphics.silkscreen_lines[1].width_nm, 120_000);
        assert_eq!(graphics.courtyard_start, PointNm::new(-1_700_000, -750_000));
        assert_eq!(graphics.courtyard_end, PointNm::new(1_700_000, 750_000));
        assert_eq!(graphics.courtyard_width_nm, 50_000);
        for expected in [
            concat!(
                "(fp_line\n",
                "    (start -0.45 -0.5)\n",
                "    (end 0.45 -0.5)\n",
                "    (stroke (width 0.12) (type default))\n",
                "    (layer \"F.SilkS\")\n",
                "  )",
            ),
            concat!(
                "(fp_line\n",
                "    (start -0.45 0.5)\n",
                "    (end 0.45 0.5)\n",
                "    (stroke (width 0.12) (type default))\n",
                "    (layer \"F.SilkS\")\n",
                "  )",
            ),
            concat!(
                "(fp_rect\n",
                "    (start -1.7 -0.75)\n",
                "    (end 1.7 0.75)\n",
                "    (stroke (width 0.05) (type default))\n",
                "    (fill none)\n",
                "    (layer \"F.CrtYd\")\n",
                "  )",
            ),
        ] {
            assert!(
                RESISTOR_FOOTPRINT_LIBRARY.contains(expected),
                "vendored footprint geometry does not match catalog field set: {expected}"
            );
        }
        for library_id in ["CircuitC:R", "CircuitC:VDC"] {
            let definition = symbol(library_id).expect("catalog symbol exists");
            let symbol_asset = balanced_block(
                SYMBOL_LIBRARY,
                &format!("  (symbol \"{}\"", definition.name),
            );
            for pin in definition.pins {
                let number_marker = format!("(number \"{}\"", pin.number);
                let number_offset = symbol_asset.find(&number_marker).unwrap_or_else(|| {
                    panic!("vendored {library_id} is missing pin {}", pin.number)
                });
                let pin_start = symbol_asset[..number_offset]
                    .rfind("(pin passive line")
                    .expect("pin number belongs to a passive pin stanza");
                let pin_asset = balanced_block(&symbol_asset[pin_start..], "(pin passive line");
                let expected_at = match pin.offset {
                    PointNm {
                        x: 0,
                        y: -3_810_000,
                    } => "(at 0 3.81 270)",
                    PointNm { x: 0, y: 3_810_000 } => "(at 0 -3.81 90)",
                    offset => panic!("unexpected {library_id} catalog pin offset: {offset:?}"),
                };
                assert!(
                    pin_asset.contains(expected_at),
                    "vendored {library_id} pin {} does not match catalog offset {:?}: {pin_asset}",
                    pin.number,
                    pin.offset
                );
                assert!(pin_asset.contains(&number_marker));
            }
        }
    }

    #[test]
    fn every_catalog_entry_has_a_publishable_library_file_and_footprint_graphics() {
        let mut library_bindings = std::collections::BTreeMap::new();
        let symbol_ids: std::collections::BTreeSet<_> = part_definitions()
            .iter()
            .map(|definition| definition.symbol_library_id)
            .collect();
        for library_id in symbol_ids {
            assert!(symbol(library_id).is_some());
            let file = symbol_library_file(library_id)
                .expect("every catalog symbol must have a publishable library file");
            assert_eq!(file.kind, crate::KicadLibraryFileKind::Symbol);
            assert_eq!(file.nickname, "CircuitC");
            assert_eq!(file.relative_path, "CircuitC.kicad_sym");
            assert_eq!(file.table_relative_path, file.relative_path);
            assert!(!file.contents.is_empty());
            if let Some(first_path) =
                library_bindings.insert((file.kind, file.nickname), file.table_relative_path)
            {
                assert_eq!(
                    first_path, file.table_relative_path,
                    "each symbol-library nickname must identify exactly one table path"
                );
            }
        }
        let footprint_ids: std::collections::BTreeSet<_> = part_definitions()
            .iter()
            .filter_map(|definition| definition.footprint_library_id)
            .collect();
        for library_id in footprint_ids {
            assert!(footprint(library_id).is_some());
            assert!(footprint_graphics(library_id).is_some());
            let file = footprint_library_file(library_id)
                .expect("every catalog footprint must have a publishable library file");
            assert_eq!(file.kind, crate::KicadLibraryFileKind::Footprint);
            assert_eq!(file.nickname, "CircuitC");
            assert!(file.relative_path.ends_with(".kicad_mod"));
            assert_eq!(file.table_relative_path, "CircuitC.pretty");
            assert!(
                file.relative_path
                    .strip_prefix(file.table_relative_path)
                    .is_some_and(|suffix| suffix.starts_with('/') && suffix.ends_with(".kicad_mod"))
            );
            assert!(!file.contents.is_empty());
            if let Some(first_path) =
                library_bindings.insert((file.kind, file.nickname), file.table_relative_path)
            {
                assert_eq!(
                    first_path, file.table_relative_path,
                    "each footprint-library nickname must identify exactly one table path"
                );
            }
        }
    }

    #[test]
    fn catalog_lookup_rejects_incoherent_exact_part_identities() {
        for identity in [
            PartIdentity {
                logical_function: "resistor".to_owned(),
                manufacturer: Some("Acme".to_owned()),
                manufacturer_part_number: Some("banana".to_owned()),
                package: Some("0603_1608Metric".to_owned()),
                lifecycle: None,
                sourcing: None,
                approved_substitutions: Vec::new(),
            },
            PartIdentity {
                logical_function: "dc_voltage_source".to_owned(),
                manufacturer: Some("Acme".to_owned()),
                manufacturer_part_number: Some("VIRTUAL".to_owned()),
                package: Some("virtual".to_owned()),
                lifecycle: None,
                sourcing: None,
                approved_substitutions: Vec::new(),
            },
        ] {
            assert_eq!(part(&identity), None);
        }
    }
}

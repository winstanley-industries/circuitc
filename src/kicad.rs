use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::design::{Component, CopperLayer, Design, Diagnostic, PadShape, PointNm};

const KICAD_BOARD_FORMAT_VERSION: u32 = 20_260_206;
const CIRCUITC_VERSION: &str = "0.1.0";

pub(crate) fn validate(design: &Design) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut uuids = BTreeMap::new();
    let mut register = |path: String, kind: &str, fields: &[&str]| {
        let uuid = stable_uuid(&design.name, kind, fields);
        if let Some(first_path) = uuids.insert(uuid.clone(), path.clone()) {
            diagnostics.push(Diagnostic {
                code: "CC-KICAD-ID-001",
                path,
                message: format!("generated KiCad UUID {uuid} collides with entity {first_path}"),
            });
        }
    };

    register("design.board.outline".to_owned(), "board-outline", &[]);
    for component in design
        .components
        .iter()
        .filter(|component| component.physical.is_some())
    {
        register(
            format!("{}.footprint", component.path),
            "footprint",
            &[&component.path],
        );
        for property in ["Reference", "Value", "Datasheet", "Description"] {
            register(
                format!("{}.footprint.property.{property}", component.path),
                "footprint-property",
                &[&component.path, property],
            );
        }
        for pad in &component
            .physical
            .as_ref()
            .expect("filtered physical component must have an implementation")
            .footprint
            .pads
        {
            register(
                format!("{}.footprint.pad.{}", component.path, pad.number),
                "footprint-pad",
                &[&component.path, &pad.number],
            );
        }
    }
    for route in &design.board.routes {
        register(
            format!("design.board.routes.{}", route.path),
            "route-segment",
            &[&route.path],
        );
    }
    diagnostics
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
        "Description",
        "Generated by CircuitC",
        PointNm::new(0, 0),
        fab_layer(physical.placement.layer),
        true,
    );
    output.push_str("    (attr smd)\n");
    output.push_str("    (duplicate_pad_numbers_are_jumpers no)\n");

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
        writeln!(
            output,
            "      (net {})",
            quoted(
                component
                    .net_for_pad(&pad.number)
                    .expect("validated physical pad must resolve")
            )
        )
        .unwrap();
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

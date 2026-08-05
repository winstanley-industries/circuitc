use sha2::{Digest, Sha256};

use crate::design::*;
use crate::quantity::{Quantity, Unit};

use super::contract::{ReleaseDiagnostic, diagnostic};

const DESIGN_IDENTITY_DOMAIN: &[u8] = b"CIRCUITC-DESIGN-IDENTITY-V1\0";

struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn tag(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn boolean(&mut self, value: bool) {
        self.tag(u8::from(value));
    }

    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn i8(&mut self, value: i8) {
        self.bytes.push(value as u8);
    }

    fn i16(&mut self, value: i16) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn string(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.bytes.extend_from_slice(value.as_bytes());
    }

    fn sequence<T>(&mut self, values: &[T], mut encode: impl FnMut(&mut Self, &T)) {
        self.u64(values.len() as u64);
        for value in values {
            encode(self, value);
        }
    }

    fn option<T>(&mut self, value: Option<&T>, encode: impl FnOnce(&mut Self, &T)) {
        match value {
            Some(value) => {
                self.tag(1);
                encode(self, value);
            }
            None => self.tag(0),
        }
    }

    fn point(&mut self, point: PointNm) {
        self.i64(point.x);
        self.i64(point.y);
    }

    fn size(&mut self, size: SizeNm) {
        self.i64(size.width);
        self.i64(size.height);
    }

    fn layer(&mut self, layer: CopperLayer) {
        self.tag(match layer {
            CopperLayer::Front => 0,
            CopperLayer::Back => 1,
        });
    }

    fn electrical_type(&mut self, kind: ElectricalPinType) {
        self.tag(match kind {
            ElectricalPinType::Input => 0,
            ElectricalPinType::Output => 1,
            ElectricalPinType::Bidirectional => 2,
            ElectricalPinType::Passive => 3,
            ElectricalPinType::PowerInput => 4,
            ElectricalPinType::PowerOutput => 5,
            ElectricalPinType::OpenCollector => 6,
            ElectricalPinType::OpenEmitter => 7,
        });
    }

    fn connection_state(&mut self, state: &ConnectionState) {
        match state {
            ConnectionState::Connected(net) => {
                self.tag(0);
                self.string(net);
            }
            ConnectionState::NoConnect => self.tag(1),
        }
    }

    fn quantity(&mut self, quantity: Quantity) {
        let quantity = quantity.canonicalized();
        self.i64(quantity.coefficient);
        self.i8(quantity.exponent);
        self.tag(match quantity.unit {
            Unit::Ohm => 0,
            Unit::Volt => 1,
            Unit::Ampere => 2,
            Unit::Farad => 3,
            Unit::Henry => 4,
            Unit::Hertz => 5,
            Unit::Second => 6,
            Unit::Degree => 7,
            Unit::Dimensionless => 8,
        });
    }

    fn substitution(&mut self, substitution: &ApprovedSubstitution) {
        self.string(&substitution.manufacturer);
        self.string(&substitution.manufacturer_part_number);
        self.string(&substitution.package);
    }

    fn design(&mut self, design: &Design) {
        self.u32(design.schema_version);
        self.string(&design.name);
        self.sequence(&design.nets, |this, net| {
            this.string(&net.name);
            this.boolean(net.is_ground);
        });
        self.sequence(&design.modules, |this, module| {
            this.string(&module.path);
            this.sequence(&module.ports, |this, port| {
                this.string(&port.name);
                this.tag(match port.direction {
                    PortDirection::Input => 0,
                    PortDirection::Output => 1,
                    PortDirection::InOut => 2,
                });
                this.electrical_type(port.electrical_type);
                this.connection_state(&port.state);
            });
        });
        self.sequence(&design.components, |this, component| {
            this.component(component);
        });
        self.sequence(&design.analyses, |this, analysis| {
            this.string(&analysis.path);
            match &analysis.kind {
                SimulationAnalysisKind::DcOperatingPoint => this.tag(0),
                SimulationAnalysisKind::AcLinearSweep {
                    source,
                    points,
                    start_frequency,
                    stop_frequency,
                    magnitude,
                    phase,
                } => {
                    this.tag(1);
                    this.string(source);
                    this.u32(*points);
                    this.quantity(*start_frequency);
                    this.quantity(*stop_frequency);
                    this.quantity(*magnitude);
                    this.quantity(*phase);
                }
                SimulationAnalysisKind::Transient {
                    step,
                    stop,
                    start,
                    uic,
                } => {
                    this.tag(2);
                    this.quantity(*step);
                    this.quantity(*stop);
                    this.quantity(*start);
                    this.boolean(*uic);
                }
            }
        });
        self.sequence(&design.assertions, |this, assertion| {
            this.string(&assertion.path);
            this.string(&assertion.analysis_path);
            this.string(&assertion.net);
            match assertion.sample {
                SimulationSample::Scalar => this.tag(0),
                SimulationSample::Frequency(quantity) => {
                    this.tag(1);
                    this.quantity(quantity);
                }
                SimulationSample::Time(quantity) => {
                    this.tag(2);
                    this.quantity(quantity);
                }
            }
            this.quantity(assertion.expected);
            this.quantity(assertion.absolute_tolerance);
            this.quantity(assertion.relative_tolerance);
        });
        self.point(design.board.outline.origin);
        self.size(design.board.outline.size);
        self.sequence(&design.board.routes, |this, route| {
            this.string(&route.path);
            this.string(&route.net);
            this.point(route.start);
            this.point(route.end);
            this.i64(route.width_nm);
            this.layer(route.layer);
        });
        self.sequence(&design.board.routing_requests, |this, request| {
            this.string(&request.path);
            this.string(&request.net);
            this.i64(request.width_nm);
            this.i64(request.clearance_nm);
            this.i64(request.grid_step_nm);
            this.layer(request.layer);
        });
        self.option(design.product.catalog.as_ref(), |this, catalog| {
            this.string(&catalog.snapshot_id);
            this.string(&catalog.sha256);
            this.string(&catalog.evaluated_on);
        });
        self.sequence(&design.product.variants, |this, variant| {
            this.string(&variant.path);
            this.u64(variant.build_quantity);
            this.sequence(&variant.components, |this, component| {
                this.string(&component.component_path);
                match &component.state {
                    PopulationState::Fitted => this.tag(0),
                    PopulationState::NotFitted => this.tag(1),
                    PopulationState::Alternate(substitution) => {
                        this.tag(2);
                        this.substitution(substitution);
                    }
                }
            });
            this.sequence(&variant.configurations, |this, configuration| {
                this.string(&configuration.key);
                this.string(&configuration.value);
            });
        });
        self.sequence(
            &design.product.manufacturability_analyses,
            |this, analysis| {
                this.string(&analysis.path);
                this.string(&analysis.adapter);
                this.string(&analysis.version);
                this.sequence(&analysis.assertions, |this, assertion| {
                    this.string(&assertion.path);
                    this.tag(match assertion.capability {
                        ManufacturabilityCapability::ErcClean => 0,
                        ManufacturabilityCapability::DrcClean => 1,
                        ManufacturabilityCapability::UnconnectedClean => 2,
                        ManufacturabilityCapability::SchematicParityClean => 3,
                        ManufacturabilityCapability::FabricationInventoryComplete => 4,
                    });
                });
            },
        );
    }

    fn component(&mut self, component: &Component) {
        self.string(&component.path);
        self.string(&component.reference);
        self.string(&component.part.logical_function);
        self.option(component.part.manufacturer.as_ref(), |this, value| {
            this.string(value)
        });
        self.option(
            component.part.manufacturer_part_number.as_ref(),
            |this, value| this.string(value),
        );
        self.option(component.part.package.as_ref(), |this, value| {
            this.string(value)
        });
        self.option(component.part.lifecycle.as_ref(), |this, lifecycle| {
            this.tag(match lifecycle {
                LifecycleStatus::Active => 0,
                LifecycleStatus::NotRecommendedForNewDesigns => 1,
                LifecycleStatus::Obsolete => 2,
            });
        });
        self.option(component.part.sourcing.as_ref(), |this, sourcing| {
            this.u64(sourcing.minimum_available_quantity);
            this.u32(sourcing.maximum_lead_time_days);
            this.string(&sourcing.required_region);
        });
        self.sequence(
            &component.part.approved_substitutions,
            |this, substitution| this.substitution(substitution),
        );
        self.string(&component.symbol.library_id);
        self.sequence(&component.symbol.pins, |this, pin| {
            this.string(&pin.pin);
            this.string(&pin.symbol_pin);
            this.electrical_type(pin.electrical_type);
        });
        self.point(component.schematic_placement.position);
        self.i16(component.schematic_placement.rotation_degrees);
        match component.value {
            ComponentValue::Resistance(quantity) => {
                self.tag(0);
                self.quantity(quantity);
            }
            ComponentValue::DcVoltage(quantity) => {
                self.tag(1);
                self.quantity(quantity);
            }
        }
        self.sequence(&component.connections, |this, connection| {
            this.string(&connection.pin);
            this.connection_state(&connection.state);
        });
        self.option(component.physical.as_ref(), |this, physical| {
            this.string(&physical.footprint.library_id);
            this.sequence(&physical.footprint.pads, |this, pad| {
                this.string(&pad.number);
                this.point(pad.offset);
                this.size(pad.size);
                this.tag(match pad.shape {
                    PadShape::Rect => 0,
                    PadShape::RoundRect => 1,
                });
            });
            this.point(physical.placement.position);
            this.i16(physical.placement.rotation_degrees);
            this.layer(physical.placement.layer);
            this.sequence(&physical.pin_pad_bindings, |this, binding| {
                this.string(&binding.pin);
                this.string(&binding.pad);
            });
        });
        self.option(component.simulation.as_ref(), |this, model| match model {
            SimulationModel::Resistor {
                model_id,
                positive_pin,
                negative_pin,
            } => {
                this.tag(0);
                this.string(model_id);
                this.string(positive_pin);
                this.string(negative_pin);
            }
            SimulationModel::DcVoltageSource {
                model_id,
                positive_pin,
                negative_pin,
            } => {
                this.tag(1);
                this.string(model_id);
                this.string(positive_pin);
                this.string(negative_pin);
            }
        });
    }
}

/// Compute the complete private Design-v1 identity used by release closure.
pub fn canonical_design_identity(design: &Design) -> Result<String, ReleaseDiagnostic> {
    design.validate().map_err(|diagnostics| {
        let first = diagnostics
            .first()
            .expect("Design validation failures are nonempty");
        diagnostic(
            "CC-RELEASE-DESIGN-001",
            &first.path,
            format!(
                "Design IR validation failed before identity encoding: {}",
                first.message
            ),
        )
    })?;
    let mut canonical = design.clone();
    canonical.canonicalize();
    let mut encoder = Encoder::new();
    encoder.design(&canonical);
    let mut hash = Sha256::new();
    hash.update(DESIGN_IDENTITY_DOMAIN);
    hash.update(&encoder.bytes);
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use crate::demo::voltage_divider;

    use super::*;

    fn unchecked_identity(design: &Design) -> String {
        let mut encoder = Encoder::new();
        encoder.design(design);
        let mut hash = Sha256::new();
        hash.update(DESIGN_IDENTITY_DOMAIN);
        hash.update(encoder.bytes);
        format!("{:x}", hash.finalize())
    }

    fn assert_changes(base: &Design, field: &str, mutate: impl FnOnce(&mut Design)) {
        let expected = unchecked_identity(base);
        let mut changed = base.clone();
        mutate(&mut changed);
        assert_ne!(
            unchecked_identity(&changed),
            expected,
            "identity encoder omitted or test mutation did not change {field}"
        );
    }

    #[test]
    fn identity_is_canonical_but_semantic() {
        let design = voltage_divider();
        let expected = canonical_design_identity(&design).unwrap();

        let mut permuted = design.clone();
        permuted.components.reverse();
        permuted.nets.reverse();
        assert_eq!(canonical_design_identity(&permuted).unwrap(), expected);

        let mut changed = design;
        changed.board.outline.size.width += 1;
        assert_ne!(canonical_design_identity(&changed).unwrap(), expected);
    }

    #[test]
    fn identity_encoder_covers_every_design_v1_field_family() {
        let mut design = voltage_divider();
        design.analyses = vec![
            SimulationAnalysis {
                path: "simulation.dc".to_owned(),
                kind: SimulationAnalysisKind::DcOperatingPoint,
            },
            SimulationAnalysis {
                path: "simulation.ac".to_owned(),
                kind: SimulationAnalysisKind::AcLinearSweep {
                    source: "divider.analysis.input".to_owned(),
                    points: 4,
                    start_frequency: Quantity::new(1, 0, Unit::Hertz),
                    stop_frequency: Quantity::new(4, 0, Unit::Hertz),
                    magnitude: Quantity::new(1, 0, Unit::Volt),
                    phase: Quantity::new(0, 0, Unit::Degree),
                },
            },
            SimulationAnalysis {
                path: "simulation.transient".to_owned(),
                kind: SimulationAnalysisKind::Transient {
                    step: Quantity::new(125, -3, Unit::Second),
                    stop: Quantity::new(500, -3, Unit::Second),
                    start: Quantity::new(0, 0, Unit::Second),
                    uic: false,
                },
            },
        ];
        design.assertions = vec![
            SimulationAssertion {
                path: "checks.scalar".to_owned(),
                analysis_path: "simulation.dc".to_owned(),
                net: "VOUT".to_owned(),
                sample: SimulationSample::Scalar,
                expected: Quantity::new(5, 0, Unit::Volt),
                absolute_tolerance: Quantity::new(1, -6, Unit::Volt),
                relative_tolerance: Quantity::new(0, 0, Unit::Dimensionless),
            },
            SimulationAssertion {
                path: "checks.frequency".to_owned(),
                analysis_path: "simulation.ac".to_owned(),
                net: "VOUT".to_owned(),
                sample: SimulationSample::Frequency(Quantity::new(3, 0, Unit::Hertz)),
                expected: Quantity::new(5, -1, Unit::Volt),
                absolute_tolerance: Quantity::new(1, -6, Unit::Volt),
                relative_tolerance: Quantity::new(0, 0, Unit::Dimensionless),
            },
            SimulationAssertion {
                path: "checks.time".to_owned(),
                analysis_path: "simulation.transient".to_owned(),
                net: "VOUT".to_owned(),
                sample: SimulationSample::Time(Quantity::new(250, -3, Unit::Second)),
                expected: Quantity::new(5, 0, Unit::Volt),
                absolute_tolerance: Quantity::new(1, -6, Unit::Volt),
                relative_tolerance: Quantity::new(0, 0, Unit::Dimensionless),
            },
        ];
        design.board.routing_requests = vec![RoutingRequest {
            path: "board.autoroute.vout".to_owned(),
            net: "VOUT".to_owned(),
            width_nm: 250_000,
            clearance_nm: 200_000,
            grid_step_nm: 1_000_000,
            layer: CopperLayer::Front,
        }];

        macro_rules! mutations {
            ($($mutation:expr),+ $(,)?) => {
                $(assert_changes(&design, stringify!($mutation), $mutation);)+
            };
        }

        mutations!(
            |d: &mut Design| d.schema_version += 1,
            |d: &mut Design| d.name.push('x'),
            |d: &mut Design| d.nets[0].name.push('x'),
            |d: &mut Design| d.nets[0].is_ground = !d.nets[0].is_ground,
            |d: &mut Design| d.modules[0].path.push('x'),
            |d: &mut Design| d.modules[0].ports[0].name.push('x'),
            |d: &mut Design| d.modules[0].ports[0].direction = PortDirection::InOut,
            |d: &mut Design| d.modules[0].ports[0].electrical_type = ElectricalPinType::OpenEmitter,
            |d: &mut Design| d.modules[0].ports[0].state = ConnectionState::NoConnect,
            |d: &mut Design| d.components[0].path.push('x'),
            |d: &mut Design| d.components[0].reference.push('x'),
            |d: &mut Design| d.components[0].part.logical_function.push('x'),
            |d: &mut Design| d.components[0].part.manufacturer = None,
            |d: &mut Design| d.components[0].part.manufacturer_part_number = None,
            |d: &mut Design| d.components[0].part.package = None,
            |d: &mut Design| d.components[0].part.lifecycle = Some(LifecycleStatus::Obsolete),
            |d: &mut Design| d.components[0]
                .part
                .sourcing
                .as_mut()
                .unwrap()
                .minimum_available_quantity += 1,
            |d: &mut Design| d.components[0]
                .part
                .sourcing
                .as_mut()
                .unwrap()
                .maximum_lead_time_days += 1,
            |d: &mut Design| d.components[0]
                .part
                .sourcing
                .as_mut()
                .unwrap()
                .required_region
                .push('x'),
            |d: &mut Design| d.components[0].part.approved_substitutions[0]
                .manufacturer
                .push('x'),
            |d: &mut Design| d.components[0].part.approved_substitutions[0]
                .manufacturer_part_number
                .push('x'),
            |d: &mut Design| d.components[0].part.approved_substitutions[0]
                .package
                .push('x'),
            |d: &mut Design| d.components[0].symbol.library_id.push('x'),
            |d: &mut Design| d.components[0].symbol.pins[0].pin.push('x'),
            |d: &mut Design| d.components[0].symbol.pins[0].symbol_pin.push('x'),
            |d: &mut Design| d.components[0].symbol.pins[0].electrical_type =
                ElectricalPinType::OpenCollector,
            |d: &mut Design| d.components[0].schematic_placement.position.x += 1,
            |d: &mut Design| d.components[0].schematic_placement.rotation_degrees += 90,
            |d: &mut Design| match &mut d.components[0].value {
                ComponentValue::Resistance(q) | ComponentValue::DcVoltage(q) => q.coefficient += 1,
            },
            |d: &mut Design| match &mut d.components[0].value {
                ComponentValue::Resistance(q) | ComponentValue::DcVoltage(q) => q.exponent += 1,
            },
            |d: &mut Design| d.components[0].value =
                ComponentValue::DcVoltage(Quantity::new(10, 3, Unit::Volt)),
            |d: &mut Design| d.components[0].connections[0].pin.push('x'),
            |d: &mut Design| d.components[0].connections[0].state = ConnectionState::NoConnect,
            |d: &mut Design| d.components[0]
                .physical
                .as_mut()
                .unwrap()
                .footprint
                .library_id
                .push('x'),
            |d: &mut Design| d.components[0].physical.as_mut().unwrap().footprint.pads[0]
                .number
                .push('x'),
            |d: &mut Design| d.components[0].physical.as_mut().unwrap().footprint.pads[0]
                .offset
                .x += 1,
            |d: &mut Design| d.components[0].physical.as_mut().unwrap().footprint.pads[0]
                .size
                .width += 1,
            |d: &mut Design| {
                let shape = &mut d.components[0].physical.as_mut().unwrap().footprint.pads[0].shape;
                *shape = match *shape {
                    PadShape::Rect => PadShape::RoundRect,
                    PadShape::RoundRect => PadShape::Rect,
                };
            },
            |d: &mut Design| d.components[0]
                .physical
                .as_mut()
                .unwrap()
                .placement
                .position
                .y += 1,
            |d: &mut Design| d.components[0]
                .physical
                .as_mut()
                .unwrap()
                .placement
                .rotation_degrees += 90,
            |d: &mut Design| d.components[0].physical.as_mut().unwrap().placement.layer =
                CopperLayer::Back,
            |d: &mut Design| d.components[0].physical.as_mut().unwrap().pin_pad_bindings[0]
                .pin
                .push('x'),
            |d: &mut Design| d.components[0].physical.as_mut().unwrap().pin_pad_bindings[0]
                .pad
                .push('x'),
            |d: &mut Design| d.components[0].simulation = None,
            |d: &mut Design| match d.components[0].simulation.as_mut().unwrap() {
                SimulationModel::Resistor { model_id, .. }
                | SimulationModel::DcVoltageSource { model_id, .. } => model_id.push('x'),
            },
            |d: &mut Design| match d.components[0].simulation.as_mut().unwrap() {
                SimulationModel::Resistor { positive_pin, .. }
                | SimulationModel::DcVoltageSource { positive_pin, .. } => positive_pin.push('x'),
            },
            |d: &mut Design| match d.components[0].simulation.as_mut().unwrap() {
                SimulationModel::Resistor { negative_pin, .. }
                | SimulationModel::DcVoltageSource { negative_pin, .. } => negative_pin.push('x'),
            },
            |d: &mut Design| d.analyses[0].path.push('x'),
            |d: &mut Design| d.analyses[0].kind = SimulationAnalysisKind::Transient {
                step: Quantity::new(1, 0, Unit::Second),
                stop: Quantity::new(2, 0, Unit::Second),
                start: Quantity::new(0, 0, Unit::Second),
                uic: false
            },
            |d: &mut Design| if let SimulationAnalysisKind::AcLinearSweep { source, .. } =
                &mut d.analyses[1].kind
            {
                source.push('x')
            },
            |d: &mut Design| if let SimulationAnalysisKind::AcLinearSweep { points, .. } =
                &mut d.analyses[1].kind
            {
                *points += 1
            },
            |d: &mut Design| if let SimulationAnalysisKind::AcLinearSweep {
                start_frequency, ..
            } = &mut d.analyses[1].kind
            {
                start_frequency.coefficient += 1
            },
            |d: &mut Design| if let SimulationAnalysisKind::AcLinearSweep {
                stop_frequency, ..
            } = &mut d.analyses[1].kind
            {
                stop_frequency.coefficient += 1
            },
            |d: &mut Design| if let SimulationAnalysisKind::AcLinearSweep { magnitude, .. } =
                &mut d.analyses[1].kind
            {
                magnitude.coefficient += 1
            },
            |d: &mut Design| if let SimulationAnalysisKind::AcLinearSweep { phase, .. } =
                &mut d.analyses[1].kind
            {
                phase.coefficient += 1
            },
            |d: &mut Design| if let SimulationAnalysisKind::Transient { step, .. } =
                &mut d.analyses[2].kind
            {
                step.coefficient += 1
            },
            |d: &mut Design| if let SimulationAnalysisKind::Transient { stop, .. } =
                &mut d.analyses[2].kind
            {
                stop.coefficient += 1
            },
            |d: &mut Design| if let SimulationAnalysisKind::Transient { start, .. } =
                &mut d.analyses[2].kind
            {
                start.coefficient += 1
            },
            |d: &mut Design| if let SimulationAnalysisKind::Transient { uic, .. } =
                &mut d.analyses[2].kind
            {
                *uic = !*uic
            },
            |d: &mut Design| d.assertions[0].path.push('x'),
            |d: &mut Design| d.assertions[0].analysis_path.push('x'),
            |d: &mut Design| d.assertions[0].net.push('x'),
            |d: &mut Design| d.assertions[0].sample =
                SimulationSample::Time(Quantity::new(1, 0, Unit::Second)),
            |d: &mut Design| if let SimulationSample::Frequency(q) = &mut d.assertions[1].sample {
                q.coefficient += 1
            },
            |d: &mut Design| if let SimulationSample::Time(q) = &mut d.assertions[2].sample {
                q.coefficient += 1
            },
            |d: &mut Design| d.assertions[0].expected.coefficient += 1,
            |d: &mut Design| d.assertions[0].absolute_tolerance.coefficient += 1,
            |d: &mut Design| d.assertions[0].relative_tolerance.coefficient += 1,
            |d: &mut Design| d.board.outline.origin.x += 1,
            |d: &mut Design| d.board.outline.size.height += 1,
            |d: &mut Design| d.board.routes[0].path.push('x'),
            |d: &mut Design| d.board.routes[0].net.push('x'),
            |d: &mut Design| d.board.routes[0].start.x += 1,
            |d: &mut Design| d.board.routes[0].end.y += 1,
            |d: &mut Design| d.board.routes[0].width_nm += 1,
            |d: &mut Design| d.board.routes[0].layer = CopperLayer::Back,
            |d: &mut Design| d.board.routing_requests[0].path.push('x'),
            |d: &mut Design| d.board.routing_requests[0].net.push('x'),
            |d: &mut Design| d.board.routing_requests[0].width_nm += 1,
            |d: &mut Design| d.board.routing_requests[0].clearance_nm += 1,
            |d: &mut Design| d.board.routing_requests[0].grid_step_nm += 1,
            |d: &mut Design| d.board.routing_requests[0].layer = CopperLayer::Back,
            |d: &mut Design| d.product.catalog.as_mut().unwrap().snapshot_id.push('x'),
            |d: &mut Design| d
                .product
                .catalog
                .as_mut()
                .unwrap()
                .sha256
                .replace_range(..1, "0"),
            |d: &mut Design| d.product.catalog.as_mut().unwrap().evaluated_on =
                "2026-08-03".to_owned(),
            |d: &mut Design| d.product.variants[0].path.push('x'),
            |d: &mut Design| d.product.variants[0].build_quantity += 1,
            |d: &mut Design| d.product.variants[0].components[0].component_path.push('x'),
            |d: &mut Design| d.product.variants[0].components[0].state = PopulationState::NotFitted,
            |d: &mut Design| {
                let alternate = d
                    .product
                    .variants
                    .iter_mut()
                    .flat_map(|variant| &mut variant.components)
                    .find_map(|component| match &mut component.state {
                        PopulationState::Alternate(value) => Some(value),
                        _ => None,
                    })
                    .expect("reference design has an alternate population");
                alternate.manufacturer.push('x');
            },
            |d: &mut Design| {
                let alternate = d
                    .product
                    .variants
                    .iter_mut()
                    .flat_map(|variant| &mut variant.components)
                    .find_map(|component| match &mut component.state {
                        PopulationState::Alternate(value) => Some(value),
                        _ => None,
                    })
                    .expect("reference design has an alternate population");
                alternate.manufacturer_part_number.push('x');
            },
            |d: &mut Design| {
                let alternate = d
                    .product
                    .variants
                    .iter_mut()
                    .flat_map(|variant| &mut variant.components)
                    .find_map(|component| match &mut component.state {
                        PopulationState::Alternate(value) => Some(value),
                        _ => None,
                    })
                    .expect("reference design has an alternate population");
                alternate.package.push('x');
            },
            |d: &mut Design| d.product.variants[0].configurations[0].key.push('x'),
            |d: &mut Design| d.product.variants[0].configurations[0].value.push('x'),
            |d: &mut Design| d.product.manufacturability_analyses[0].path.push('x'),
            |d: &mut Design| d.product.manufacturability_analyses[0].adapter.push('x'),
            |d: &mut Design| d.product.manufacturability_analyses[0].version.push('x'),
            |d: &mut Design| d.product.manufacturability_analyses[0].assertions[0]
                .path
                .push('x'),
            |d: &mut Design| {
                let capability =
                    &mut d.product.manufacturability_analyses[0].assertions[0].capability;
                *capability = match *capability {
                    ManufacturabilityCapability::ErcClean => ManufacturabilityCapability::DrcClean,
                    _ => ManufacturabilityCapability::ErcClean,
                };
            },
        );
    }
}

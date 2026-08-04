use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::quantity::{Quantity, Unit};

pub const DESIGN_SCHEMA_VERSION: u32 = 1;
pub const MAX_ABS_COORDINATE_NM: i64 = 1_000_000_000_000;
/// CircuitC resource ceiling for declared simulation analyses.
pub const MAX_SIMULATION_ANALYSES: usize = 256;
/// Design-level bound aligned with the versioned simulation wire contracts.
pub const MAX_SIMULATION_ASSERTIONS: usize = 10_000;
/// Maximum number of samples requested by one analysis, including both
/// endpoints of a transient time grid.
pub const MAX_SIMULATION_SAMPLES: u32 = 10_000;
/// Maximum aggregate declared nominal samples across all analyses.
pub const MAX_SIMULATION_TOTAL_SAMPLES: u32 = 10_000;
/// Maximum number of product variants in one Design.
pub const MAX_PRODUCT_VARIANTS: usize = 256;
/// Maximum aggregate variant-assignment and totality-validation work.
pub const MAX_PRODUCT_VARIANT_WORK: usize = 10_000;
/// Maximum number of product configuration entries in one variant.
pub const MAX_PRODUCT_CONFIGURATIONS: usize = 256;
/// Maximum UTF-8 byte length of one product configuration key.
pub const MAX_PRODUCT_CONFIGURATION_KEY_BYTES: usize = 128;
/// Maximum UTF-8 byte length of one product configuration value.
pub const MAX_PRODUCT_CONFIGURATION_VALUE_BYTES: usize = 4_096;
/// Maximum number of exact approved substitutions on one physical part.
pub const MAX_PRODUCT_SUBSTITUTIONS: usize = 256;
/// Maximum number of manufacturability analyses in one Design.
pub const MAX_MANUFACTURABILITY_ANALYSES: usize = 256;
/// Maximum aggregate number of manufacturability assertions in one Design.
pub const MAX_MANUFACTURABILITY_ASSERTIONS: usize = 10_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PointNm {
    pub x: i64,
    pub y: i64,
}

impl PointNm {
    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SizeNm {
    pub width: i64,
    pub height: i64,
}

impl SizeNm {
    pub const fn new(width: i64, height: i64) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RectNm {
    pub origin: PointNm,
    pub size: SizeNm,
}

impl RectNm {
    pub fn contains(self, point: PointNm) -> bool {
        let Some(max_x) = self.origin.x.checked_add(self.size.width) else {
            return false;
        };
        let Some(max_y) = self.origin.y.checked_add(self.size.height) else {
            return false;
        };
        point.x >= self.origin.x && point.y >= self.origin.y && point.x <= max_x && point.y <= max_y
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CopperLayer {
    Front,
    Back,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Net {
    pub name: String,
    pub is_ground: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ElectricalPinType {
    Input,
    Output,
    Bidirectional,
    Passive,
    PowerInput,
    PowerOutput,
    OpenCollector,
    OpenEmitter,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PortDirection {
    Input,
    Output,
    InOut,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConnectionState {
    Connected(String),
    NoConnect,
}

impl ConnectionState {
    pub fn net(&self) -> Option<&str> {
        match self {
            Self::Connected(net) => Some(net),
            Self::NoConnect => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Connection {
    pub pin: String,
    pub state: ConnectionState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModulePort {
    pub name: String,
    pub direction: PortDirection,
    pub electrical_type: ElectricalPinType,
    pub state: ConnectionState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleInstance {
    pub path: String,
    pub ports: Vec<ModulePort>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartIdentity {
    pub logical_function: String,
    pub manufacturer: Option<String>,
    pub manufacturer_part_number: Option<String>,
    pub package: Option<String>,
    pub lifecycle: Option<LifecycleStatus>,
    pub sourcing: Option<SourcingConstraints>,
    pub approved_substitutions: Vec<ApprovedSubstitution>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LifecycleStatus {
    Active,
    NotRecommendedForNewDesigns,
    Obsolete,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourcingConstraints {
    pub minimum_available_quantity: u64,
    pub maximum_lead_time_days: u32,
    pub required_region: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ApprovedSubstitution {
    pub manufacturer: String,
    pub manufacturer_part_number: String,
    pub package: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolPinBinding {
    pub pin: String,
    pub symbol_pin: String,
    pub electrical_type: ElectricalPinType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolBinding {
    pub library_id: String,
    pub pins: Vec<SymbolPinBinding>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PadShape {
    Rect,
    RoundRect,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pad {
    pub number: String,
    pub offset: PointNm,
    pub size: SizeNm,
    pub shape: PadShape,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Footprint {
    pub library_id: String,
    pub pads: Vec<Pad>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Placement {
    pub position: PointNm,
    pub rotation_degrees: i16,
    pub layer: CopperLayer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchematicPlacement {
    pub position: PointNm,
    pub rotation_degrees: i16,
}

impl Placement {
    pub fn transform(self, offset: PointNm) -> Option<PointNm> {
        let normalized = self.rotation_degrees.rem_euclid(360);
        let rotated = match normalized {
            0 => offset,
            90 => PointNm::new(offset.y, offset.x.checked_neg()?),
            180 => PointNm::new(offset.x.checked_neg()?, offset.y.checked_neg()?),
            270 => PointNm::new(offset.y.checked_neg()?, offset.x),
            _ => return None,
        };
        Some(PointNm::new(
            self.position.x.checked_add(rotated.x)?,
            self.position.y.checked_add(rotated.y)?,
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicalImplementation {
    pub footprint: Footprint,
    pub placement: Placement,
    pub pin_pad_bindings: Vec<PinPadBinding>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PinPadBinding {
    pub pin: String,
    pub pad: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComponentValue {
    Resistance(Quantity),
    DcVoltage(Quantity),
}

impl ComponentValue {
    pub fn quantity(&self) -> Quantity {
        match self {
            Self::Resistance(quantity) | Self::DcVoltage(quantity) => *quantity,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SimulationModel {
    Resistor {
        model_id: String,
        positive_pin: String,
        negative_pin: String,
    },
    DcVoltageSource {
        model_id: String,
        positive_pin: String,
        negative_pin: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulationAnalysis {
    pub path: String,
    pub kind: SimulationAnalysisKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SimulationAnalysisKind {
    DcOperatingPoint,
    AcLinearSweep {
        source: String,
        points: u32,
        start_frequency: Quantity,
        stop_frequency: Quantity,
        magnitude: Quantity,
        phase: Quantity,
    },
    Transient {
        step: Quantity,
        stop: Quantity,
        start: Quantity,
        uic: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulationAssertion {
    pub path: String,
    pub analysis_path: String,
    pub net: String,
    pub sample: SimulationSample,
    pub expected: Quantity,
    pub absolute_tolerance: Quantity,
    pub relative_tolerance: Quantity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SimulationSample {
    Scalar,
    Frequency(Quantity),
    Time(Quantity),
}

impl SimulationModel {
    fn pins(&self) -> (&str, &str) {
        match self {
            Self::Resistor {
                positive_pin,
                negative_pin,
                ..
            }
            | Self::DcVoltageSource {
                positive_pin,
                negative_pin,
                ..
            } => (positive_pin, negative_pin),
        }
    }

    pub fn model_id(&self) -> &str {
        match self {
            Self::Resistor { model_id, .. } | Self::DcVoltageSource { model_id, .. } => model_id,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Component {
    pub path: String,
    pub reference: String,
    pub part: PartIdentity,
    pub symbol: SymbolBinding,
    pub schematic_placement: SchematicPlacement,
    pub value: ComponentValue,
    pub connections: Vec<Connection>,
    pub physical: Option<PhysicalImplementation>,
    pub simulation: Option<SimulationModel>,
}

impl Component {
    pub fn module_path(&self) -> Option<&str> {
        self.path.rsplit_once('.').map(|(parent, _)| parent)
    }

    pub fn net_for_pin(&self, pin: &str) -> Option<&str> {
        self.connections
            .iter()
            .find(|connection| connection.pin == pin)
            .and_then(|connection| connection.state.net())
    }

    pub fn connection_for_pin(&self, pin: &str) -> Option<&ConnectionState> {
        self.connections
            .iter()
            .find(|connection| connection.pin == pin)
            .map(|connection| &connection.state)
    }

    pub fn value_label(&self) -> String {
        self.value.quantity().engineering_label()
    }

    pub fn pin_for_pad(&self, pad: &str) -> Option<&str> {
        self.physical
            .as_ref()?
            .pin_pad_bindings
            .iter()
            .find(|binding| binding.pad == pad)
            .map(|binding| binding.pin.as_str())
    }

    pub fn net_for_pad(&self, pad: &str) -> Option<&str> {
        self.net_for_pin(self.pin_for_pad(pad)?)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteSegment {
    pub path: String,
    pub net: String,
    pub start: PointNm,
    pub end: PointNm,
    pub width_nm: i64,
    pub layer: CopperLayer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutingRequest {
    pub path: String,
    pub net: String,
    pub width_nm: i64,
    pub clearance_nm: i64,
    pub grid_step_nm: i64,
    pub layer: CopperLayer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Board {
    pub outline: RectNm,
    pub routes: Vec<RouteSegment>,
    pub routing_requests: Vec<RoutingRequest>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductIntent {
    pub catalog: Option<CatalogEvidenceRef>,
    pub variants: Vec<ProductVariant>,
    pub manufacturability_analyses: Vec<ManufacturabilityAnalysis>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEvidenceRef {
    pub snapshot_id: String,
    pub sha256: String,
    pub evaluated_on: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductVariant {
    pub path: String,
    pub build_quantity: u64,
    pub components: Vec<VariantComponent>,
    pub configurations: Vec<ProductConfiguration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VariantComponent {
    pub component_path: String,
    pub state: PopulationState,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PopulationState {
    Fitted,
    NotFitted,
    Alternate(ApprovedSubstitution),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProductConfiguration {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManufacturabilityAnalysis {
    pub path: String,
    pub adapter: String,
    pub version: String,
    pub assertions: Vec<ManufacturabilityAssertion>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ManufacturabilityAssertion {
    pub path: String,
    pub capability: ManufacturabilityCapability,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ManufacturabilityCapability {
    ErcClean,
    DrcClean,
    UnconnectedClean,
    SchematicParityClean,
    FabricationInventoryComplete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Design {
    pub schema_version: u32,
    pub name: String,
    pub nets: Vec<Net>,
    pub modules: Vec<ModuleInstance>,
    pub components: Vec<Component>,
    pub analyses: Vec<SimulationAnalysis>,
    pub assertions: Vec<SimulationAssertion>,
    pub board: Board,
    pub product: ProductIntent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub path: String,
    pub related_path: Option<String>,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at {}: {}",
            self.code, self.path, self.message
        )
    }
}

impl Design {
    /// Canonicalize collections whose source declaration order has no semantic
    /// meaning. Frontends call this before exposing an elaborated Design so
    /// equality at the IR boundary is independent of source ordering.
    pub fn canonicalize(&mut self) {
        self.nets.sort_by(|left, right| {
            (left.is_ground, &left.name).cmp(&(right.is_ground, &right.name))
        });
        self.modules
            .sort_by(|left, right| left.path.cmp(&right.path));
        for module in &mut self.modules {
            module
                .ports
                .sort_by(|left, right| left.name.cmp(&right.name));
        }
        self.components.sort_by(|left, right| {
            (left.physical.is_none(), &left.reference, &left.path).cmp(&(
                right.physical.is_none(),
                &right.reference,
                &right.path,
            ))
        });
        for component in &mut self.components {
            if component.part.approved_substitutions.len() <= MAX_PRODUCT_SUBSTITUTIONS {
                component.part.approved_substitutions.sort();
            }
            match &mut component.value {
                ComponentValue::Resistance(resistance) => {
                    *resistance = resistance.canonicalized();
                }
                ComponentValue::DcVoltage(voltage) => {
                    *voltage = voltage.canonicalized();
                }
            }
            let terminal_rank =
                |pin: &str| match component.simulation.as_ref().map(|model| model.pins()) {
                    Some((positive, _)) if pin == positive => 0,
                    Some((_, negative)) if pin == negative => 1,
                    _ => 2,
                };
            component.connections.sort_by(|left, right| {
                (terminal_rank(&left.pin), &left.pin, &left.state).cmp(&(
                    terminal_rank(&right.pin),
                    &right.pin,
                    &right.state,
                ))
            });
            component
                .symbol
                .pins
                .sort_by(|left, right| left.pin.cmp(&right.pin));
            component.schematic_placement.rotation_degrees = component
                .schematic_placement
                .rotation_degrees
                .rem_euclid(360);
            if let Some(physical) = &mut component.physical {
                physical.placement.rotation_degrees =
                    physical.placement.rotation_degrees.rem_euclid(360);
                physical
                    .footprint
                    .pads
                    .sort_by(|left, right| left.number.cmp(&right.number));
                physical
                    .pin_pad_bindings
                    .sort_by(|left, right| (&left.pad, &left.pin).cmp(&(&right.pad, &right.pin)));
            }
        }
        for analysis in &mut self.analyses {
            canonicalize_analysis(analysis);
        }
        self.analyses.sort_by(compare_analyses);
        for assertion in &mut self.assertions {
            canonicalize_assertion(assertion);
        }
        self.assertions.sort_by(compare_assertions);
        self.board
            .routes
            .sort_by(|left, right| left.path.cmp(&right.path));
        self.board
            .routing_requests
            .sort_by(|left, right| left.path.cmp(&right.path));
        let physical_component_count = self
            .components
            .iter()
            .filter(|component| is_physical_part(component))
            .count();
        if product_variant_resources_are_bounded(&self.product.variants, physical_component_count) {
            for variant in &mut self.product.variants {
                variant.components.sort_by(|left, right| {
                    (&left.component_path, &left.state).cmp(&(&right.component_path, &right.state))
                });
                variant.configurations.sort();
            }
            self.product
                .variants
                .sort_by(|left, right| left.path.cmp(&right.path));
        }
        if self.product.manufacturability_analyses.len() <= MAX_MANUFACTURABILITY_ANALYSES {
            let assertion_count =
                manufacturability_assertion_count(&self.product.manufacturability_analyses);
            if assertion_count.is_some_and(|count| count <= MAX_MANUFACTURABILITY_ASSERTIONS) {
                for analysis in &mut self.product.manufacturability_analyses {
                    analysis.assertions.sort();
                }
                self.product
                    .manufacturability_analyses
                    .sort_by(|left, right| left.path.cmp(&right.path));
            }
        }
    }

    pub fn validate(&self) -> Result<(), Vec<Diagnostic>> {
        let mut diagnostics = Vec::new();

        if self.schema_version != DESIGN_SCHEMA_VERSION {
            push(
                &mut diagnostics,
                "CC-IR-001",
                "design",
                format!(
                    "unsupported Design IR schema {}; expected {}",
                    self.schema_version, DESIGN_SCHEMA_VERSION
                ),
            );
        }
        if !artifact_name_is_valid(&self.name) {
            push(
                &mut diagnostics,
                "CC-IR-002",
                "design.name",
                "design name must be a safe single-file artifact stem starting with an ASCII letter or underscore and containing only ASCII letters, digits, `_`, or `-`",
            );
        }
        validate_outline(self.board.outline, &mut diagnostics);

        let mut net_names = BTreeSet::new();
        let mut ground_count = 0_usize;
        for (index, net) in self.nets.iter().enumerate() {
            let path = format!("design.nets[{index}]");
            if !token_is_valid(&net.name) {
                push(
                    &mut diagnostics,
                    "CC-NET-001",
                    &path,
                    "net name must be a non-empty canonical token",
                );
            }
            if !net_names.insert(net.name.as_str()) {
                push(
                    &mut diagnostics,
                    "CC-NET-002",
                    &path,
                    format!("duplicate net name {}", net.name),
                );
            }
            if net.is_ground {
                ground_count += 1;
            }
        }

        let has_simulation = self
            .components
            .iter()
            .any(|component| component.simulation.is_some())
            || !self.analyses.is_empty();
        if ground_count > 1 {
            push(
                &mut diagnostics,
                "CC-NET-003",
                "design.nets",
                format!("a design permits at most one ground net; found {ground_count}"),
            );
        }
        if has_simulation && ground_count != 1 {
            push(
                &mut diagnostics,
                "CC-SIM-001",
                "design.nets",
                format!("a simulated design requires exactly one ground net; found {ground_count}"),
            );
        }

        let mut module_paths = BTreeSet::new();
        if self.modules.is_empty() {
            push(
                &mut diagnostics,
                "CC-MODULE-001",
                "design.modules",
                "design must contain at least one elaborated module instance",
            );
        }
        for (index, module) in self.modules.iter().enumerate() {
            let path = format!("design.modules[{index}]");
            if !semantic_path_is_valid(&module.path) {
                push(
                    &mut diagnostics,
                    "CC-MODULE-002",
                    &path,
                    "module semantic path is invalid",
                );
            }
            if !module_paths.insert(module.path.as_str()) {
                push(
                    &mut diagnostics,
                    "CC-MODULE-003",
                    &path,
                    format!("duplicate module path {}", module.path),
                );
            }
        }
        for (index, module) in self.modules.iter().enumerate() {
            let path = format!("design.modules[{index}]");
            if let Some((parent, _)) = module.path.rsplit_once('.')
                && !module_paths.contains(parent)
            {
                push(
                    &mut diagnostics,
                    "CC-MODULE-004",
                    &path,
                    format!("module {} requires parent module {parent}", module.path),
                );
            }
            let mut ports = BTreeSet::new();
            for port in &module.ports {
                if !token_is_valid(&port.name) {
                    push(
                        &mut diagnostics,
                        "CC-PORT-001",
                        &path,
                        "module port name must be a non-empty canonical token",
                    );
                }
                if !ports.insert(port.name.as_str()) {
                    push(
                        &mut diagnostics,
                        "CC-PORT-002",
                        &path,
                        format!("duplicate module port {}", port.name),
                    );
                }
                if let ConnectionState::Connected(net) = &port.state
                    && !net_names.contains(net.as_str())
                {
                    push(
                        &mut diagnostics,
                        "CC-PORT-003",
                        &path,
                        format!("port {} references unknown net {net}", port.name),
                    );
                }
            }
        }

        let mut paths = BTreeSet::new();
        let mut references = BTreeSet::new();
        for component in &self.components {
            validate_component(
                component,
                self.board.outline,
                &net_names,
                &module_paths,
                &mut paths,
                &mut references,
                &mut diagnostics,
            );
        }
        validate_simulation_intent(
            &self.analyses,
            &self.assertions,
            &self.components,
            &net_names,
            &mut diagnostics,
        );
        validate_product_intent(&self.product, &self.components, &mut diagnostics);
        let mut schematic_positions = BTreeMap::new();
        let mut schematic_components: Vec<_> = self.components.iter().collect();
        schematic_components.sort_by(|left, right| left.path.cmp(&right.path));
        for component in schematic_components {
            if let Some(first_path) = schematic_positions.insert(
                component.schematic_placement.position,
                component.path.as_str(),
            ) {
                push_related(
                    &mut diagnostics,
                    "CC-SCHEMATIC-003",
                    component.path.as_str(),
                    first_path,
                    format!("schematic placement shares its anchor with component {first_path}"),
                );
            }
        }

        let mut route_paths = BTreeSet::new();
        let mut route_keys = BTreeSet::new();
        for (index, route) in self.board.routes.iter().enumerate() {
            let path = format!("design.board.routes[{index}]");
            if !semantic_path_is_valid(&route.path) {
                push(
                    &mut diagnostics,
                    "CC-ROUTE-006",
                    &path,
                    "route semantic path is invalid",
                );
            }
            if !route_paths.insert(route.path.as_str()) {
                push(
                    &mut diagnostics,
                    "CC-ROUTE-007",
                    &path,
                    format!("duplicate route semantic path {}", route.path),
                );
            }
            if !net_names.contains(route.net.as_str()) {
                push(
                    &mut diagnostics,
                    "CC-ROUTE-001",
                    &path,
                    format!("route references unknown net {}", route.net),
                );
            }
            if route.width_nm <= 0 {
                push(
                    &mut diagnostics,
                    "CC-ROUTE-002",
                    &path,
                    "route width must be positive",
                );
            }
            validate_size_envelope(
                route.width_nm,
                "width_nm",
                &path,
                "CC-ROUTE-008",
                &mut diagnostics,
            );
            validate_point(
                route.start,
                "start",
                &path,
                "CC-ROUTE-009",
                &mut diagnostics,
            );
            validate_point(route.end, "end", &path, "CC-ROUTE-009", &mut diagnostics);
            if route.start == route.end {
                push(
                    &mut diagnostics,
                    "CC-ROUTE-003",
                    &path,
                    "route segment must have distinct endpoints",
                );
            }
            if !self.board.outline.contains(route.start) || !self.board.outline.contains(route.end)
            {
                push(
                    &mut diagnostics,
                    "CC-ROUTE-004",
                    &path,
                    "route endpoint lies outside the board outline",
                );
            }
            let (first, second) = if route.start <= route.end {
                (route.start, route.end)
            } else {
                (route.end, route.start)
            };
            let key = (
                route.net.as_str(),
                first,
                second,
                route.width_nm,
                route.layer,
            );
            if !route_keys.insert(key) {
                push(
                    &mut diagnostics,
                    "CC-ROUTE-005",
                    &path,
                    "duplicate route segment would produce a duplicate stable identity",
                );
            }
        }

        if self.board.routing_requests.len() > 1 {
            push(
                &mut diagnostics,
                "CC-AUTOROUTE-001",
                "design.board.routing_requests",
                "Design IR v1 supports at most one APGAR planar routing request",
            );
        }
        let mut request_paths = BTreeSet::new();
        for (index, request) in self.board.routing_requests.iter().enumerate() {
            let path = format!("design.board.routing_requests[{index}]");
            if !semantic_path_is_valid(&request.path) {
                push(
                    &mut diagnostics,
                    "CC-AUTOROUTE-002",
                    &path,
                    "routing request semantic path is invalid",
                );
            }
            if !request_paths.insert(request.path.as_str()) {
                push(
                    &mut diagnostics,
                    "CC-AUTOROUTE-003",
                    &path,
                    format!("duplicate routing request semantic path {}", request.path),
                );
            }
            if route_paths.contains(request.path.as_str()) {
                push_related(
                    &mut diagnostics,
                    "CC-AUTOROUTE-004",
                    &path,
                    request.path.as_str(),
                    format!(
                        "routing request semantic path {} is already used by authored copper",
                        request.path
                    ),
                );
            }
            if !net_names.contains(request.net.as_str()) {
                push(
                    &mut diagnostics,
                    "CC-AUTOROUTE-005",
                    &path,
                    format!("routing request references unknown net {}", request.net),
                );
            }
            for (value, field, positive_code, envelope_code) in [
                (
                    request.width_nm,
                    "width_nm",
                    "CC-AUTOROUTE-006",
                    "CC-AUTOROUTE-009",
                ),
                (
                    request.clearance_nm,
                    "clearance_nm",
                    "CC-AUTOROUTE-007",
                    "CC-AUTOROUTE-010",
                ),
                (
                    request.grid_step_nm,
                    "grid_step_nm",
                    "CC-AUTOROUTE-008",
                    "CC-AUTOROUTE-011",
                ),
            ] {
                if value <= 0 {
                    push(
                        &mut diagnostics,
                        positive_code,
                        format!("{path}.{field}"),
                        format!("routing request {field} must be positive"),
                    );
                }
                validate_size_envelope(value, field, &path, envelope_code, &mut diagnostics);
            }

            let mut terminals = Vec::new();
            for component in &self.components {
                let Some(physical) = &component.physical else {
                    continue;
                };
                for pad in &physical.footprint.pads {
                    if component.net_for_pad(&pad.number) != Some(request.net.as_str()) {
                        continue;
                    }
                    if let Some(center) = physical.placement.transform(pad.offset) {
                        terminals.push((
                            component.path.as_str(),
                            pad.number.as_str(),
                            center,
                            physical.placement.layer,
                        ));
                    }
                }
            }
            terminals.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
            if terminals.len() != 2 {
                push(
                    &mut diagnostics,
                    "CC-AUTOROUTE-012",
                    &path,
                    format!(
                        "APGAR planar routing requires exactly two physical terminal pads on net {}; found {}",
                        request.net,
                        terminals.len()
                    ),
                );
            }
            for (component, pad, center, layer) in terminals {
                if layer != request.layer {
                    push(
                        &mut diagnostics,
                        "CC-AUTOROUTE-013",
                        &path,
                        format!(
                            "terminal {component} pad {pad} is not on the requested copper layer"
                        ),
                    );
                }
                if request.grid_step_nm > 0 {
                    let on_grid = center
                        .x
                        .checked_sub(self.board.outline.origin.x)
                        .zip(center.y.checked_sub(self.board.outline.origin.y))
                        .is_some_and(|(delta_x, delta_y)| {
                            delta_x % request.grid_step_nm == 0
                                && delta_y % request.grid_step_nm == 0
                        });
                    if !on_grid {
                        push(
                            &mut diagnostics,
                            "CC-AUTOROUTE-014",
                            &path,
                            format!(
                                "terminal {component} pad {pad} center is not on the authored routing grid"
                            ),
                        );
                    }
                }
            }
        }

        if diagnostics.is_empty() {
            Ok(())
        } else {
            Err(diagnostics)
        }
    }

    pub fn net_map(&self) -> BTreeMap<&str, &Net> {
        self.nets
            .iter()
            .map(|net| (net.name.as_str(), net))
            .collect()
    }
}

fn canonicalize_analysis(analysis: &mut SimulationAnalysis) {
    match &mut analysis.kind {
        SimulationAnalysisKind::DcOperatingPoint => {}
        SimulationAnalysisKind::AcLinearSweep {
            start_frequency,
            stop_frequency,
            magnitude,
            phase,
            ..
        } => {
            *start_frequency = start_frequency.canonicalized();
            *stop_frequency = stop_frequency.canonicalized();
            *magnitude = magnitude.canonicalized();
            *phase = phase.canonicalized();
        }
        SimulationAnalysisKind::Transient {
            step, stop, start, ..
        } => {
            *step = step.canonicalized();
            *stop = stop.canonicalized();
            *start = start.canonicalized();
        }
    }
}

fn canonicalize_assertion(assertion: &mut SimulationAssertion) {
    match &mut assertion.sample {
        SimulationSample::Scalar => {}
        SimulationSample::Frequency(value) | SimulationSample::Time(value) => {
            *value = value.canonicalized();
        }
    }
    assertion.expected = assertion.expected.canonicalized();
    assertion.absolute_tolerance = assertion.absolute_tolerance.canonicalized();
    assertion.relative_tolerance = assertion.relative_tolerance.canonicalized();
}

fn compare_analyses(left: &SimulationAnalysis, right: &SimulationAnalysis) -> Ordering {
    left.path
        .cmp(&right.path)
        .then_with(|| compare_analysis_kinds(&left.kind, &right.kind))
}

fn compare_analysis_kinds(
    left: &SimulationAnalysisKind,
    right: &SimulationAnalysisKind,
) -> Ordering {
    let rank = |kind: &SimulationAnalysisKind| match kind {
        SimulationAnalysisKind::DcOperatingPoint => 0_u8,
        SimulationAnalysisKind::AcLinearSweep { .. } => 1,
        SimulationAnalysisKind::Transient { .. } => 2,
    };
    rank(left)
        .cmp(&rank(right))
        .then_with(|| match (left, right) {
            (
                SimulationAnalysisKind::AcLinearSweep {
                    source: left_source,
                    points: left_points,
                    start_frequency: left_start,
                    stop_frequency: left_stop,
                    magnitude: left_magnitude,
                    phase: left_phase,
                },
                SimulationAnalysisKind::AcLinearSweep {
                    source: right_source,
                    points: right_points,
                    start_frequency: right_start,
                    stop_frequency: right_stop,
                    magnitude: right_magnitude,
                    phase: right_phase,
                },
            ) => left_source
                .cmp(right_source)
                .then_with(|| left_points.cmp(right_points))
                .then_with(|| compare_quantity_fields(*left_start, *right_start))
                .then_with(|| compare_quantity_fields(*left_stop, *right_stop))
                .then_with(|| compare_quantity_fields(*left_magnitude, *right_magnitude))
                .then_with(|| compare_quantity_fields(*left_phase, *right_phase)),
            (
                SimulationAnalysisKind::Transient {
                    step: left_step,
                    stop: left_stop,
                    start: left_start,
                    uic: left_uic,
                },
                SimulationAnalysisKind::Transient {
                    step: right_step,
                    stop: right_stop,
                    start: right_start,
                    uic: right_uic,
                },
            ) => compare_quantity_fields(*left_step, *right_step)
                .then_with(|| compare_quantity_fields(*left_stop, *right_stop))
                .then_with(|| compare_quantity_fields(*left_start, *right_start))
                .then_with(|| left_uic.cmp(right_uic)),
            _ => Ordering::Equal,
        })
}

fn compare_assertions(left: &SimulationAssertion, right: &SimulationAssertion) -> Ordering {
    left.path
        .cmp(&right.path)
        .then_with(|| left.analysis_path.cmp(&right.analysis_path))
        .then_with(|| left.net.cmp(&right.net))
        .then_with(|| compare_samples(&left.sample, &right.sample))
        .then_with(|| compare_quantity_fields(left.expected, right.expected))
        .then_with(|| compare_quantity_fields(left.absolute_tolerance, right.absolute_tolerance))
        .then_with(|| compare_quantity_fields(left.relative_tolerance, right.relative_tolerance))
}

fn compare_samples(left: &SimulationSample, right: &SimulationSample) -> Ordering {
    let rank = |sample: &SimulationSample| match sample {
        SimulationSample::Scalar => 0_u8,
        SimulationSample::Frequency(_) => 1,
        SimulationSample::Time(_) => 2,
    };
    rank(left)
        .cmp(&rank(right))
        .then_with(|| match (left, right) {
            (SimulationSample::Frequency(left), SimulationSample::Frequency(right))
            | (SimulationSample::Time(left), SimulationSample::Time(right)) => {
                compare_quantity_fields(*left, *right)
            }
            _ => Ordering::Equal,
        })
}

fn compare_quantity_fields(left: Quantity, right: Quantity) -> Ordering {
    left.unit
        .cmp(&right.unit)
        .then_with(|| left.coefficient.cmp(&right.coefficient))
        .then_with(|| left.exponent.cmp(&right.exponent))
}

fn validate_simulation_intent(
    analyses: &[SimulationAnalysis],
    assertions: &[SimulationAssertion],
    components: &[Component],
    net_names: &BTreeSet<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if analyses.len() > MAX_SIMULATION_ANALYSES {
        push(
            diagnostics,
            "CC-SIM-ANALYSIS-011",
            "design.analyses",
            format!(
                "design declares {} analyses; the maximum is {MAX_SIMULATION_ANALYSES}",
                analyses.len()
            ),
        );
    }
    if assertions.len() > MAX_SIMULATION_ASSERTIONS {
        push(
            diagnostics,
            "CC-SIM-ASSERTION-011",
            "design.assertions",
            format!(
                "design declares {} assertions; the maximum is {MAX_SIMULATION_ASSERTIONS}",
                assertions.len()
            ),
        );
    }
    if analyses.len() > MAX_SIMULATION_ANALYSES || assertions.len() > MAX_SIMULATION_ASSERTIONS {
        return;
    }

    let saturation = MAX_SIMULATION_TOTAL_SAMPLES.saturating_add(1);
    let total_samples = analyses.iter().fold(0_u32, |total, analysis| {
        let samples = match &analysis.kind {
            SimulationAnalysisKind::DcOperatingPoint => 1,
            SimulationAnalysisKind::AcLinearSweep { points, .. } => (*points).min(saturation),
            SimulationAnalysisKind::Transient { step, stop, .. } => {
                transient_nominal_samples_saturating(*step, *stop, saturation)
            }
        };
        total
            .checked_add(samples)
            .unwrap_or(saturation)
            .min(saturation)
    });
    if total_samples > MAX_SIMULATION_TOTAL_SAMPLES {
        push(
            diagnostics,
            "CC-SIM-ANALYSIS-012",
            "design.analyses",
            format!(
                "aggregate declared simulation grid exceeds {MAX_SIMULATION_TOTAL_SAMPLES} nominal samples"
            ),
        );
    }

    let components_by_path: BTreeMap<_, _> = components
        .iter()
        .map(|component| (component.path.as_str(), component))
        .collect();
    if let Some(first_analysis) = analyses
        .iter()
        .min_by(|left, right| compare_analyses(left, right))
    {
        let related_analysis = simulation_intent_path("analyses", &first_analysis.path);
        for component in components.iter().filter(|component| {
            component
                .connections
                .iter()
                .any(|connection| matches!(connection.state, ConnectionState::Connected(_)))
                && component.simulation.is_none()
        }) {
            push_related(
                diagnostics,
                "CC-SIM-CAPABILITY-001",
                component.path.as_str(),
                &related_analysis,
                "electrically participating component has no supported explicit simulation model",
            );
        }
    }
    let mut analyses_by_path = BTreeMap::new();
    let mut analysis_paths = BTreeSet::new();

    for analysis in analyses {
        let base = simulation_intent_path("analyses", &analysis.path);
        if !semantic_path_is_valid(&analysis.path) {
            push(
                diagnostics,
                "CC-SIM-ANALYSIS-001",
                format!("{base}.path"),
                "analysis path must be a non-empty canonical semantic path",
            );
        }
        if !analysis_paths.insert(analysis.path.as_str()) {
            push(
                diagnostics,
                "CC-SIM-ANALYSIS-002",
                format!("{base}.path"),
                format!("duplicate analysis path {}", analysis.path),
            );
        }
        analyses_by_path
            .entry(analysis.path.as_str())
            .or_insert(analysis);

        match &analysis.kind {
            SimulationAnalysisKind::DcOperatingPoint => {}
            SimulationAnalysisKind::AcLinearSweep {
                source,
                points,
                start_frequency,
                stop_frequency,
                magnitude,
                phase,
            } => {
                if *points < 2 || *points > MAX_SIMULATION_SAMPLES {
                    push(
                        diagnostics,
                        "CC-SIM-ANALYSIS-003",
                        format!("{base}.points"),
                        format!(
                            "AC sweep points must be in 2..={MAX_SIMULATION_SAMPLES}; found {points}"
                        ),
                    );
                }

                match components_by_path.get(source.as_str()) {
                    Some(component)
                        if matches!(
                            component.simulation.as_ref(),
                            Some(SimulationModel::DcVoltageSource { .. })
                        ) => {}
                    Some(component) => push_related(
                        diagnostics,
                        "CC-SIM-ANALYSIS-004",
                        format!("{base}.source"),
                        component.path.as_str(),
                        format!(
                            "AC source {source} must select a component with a DC voltage-source simulation model"
                        ),
                    ),
                    None => push(
                        diagnostics,
                        "CC-SIM-ANALYSIS-004",
                        format!("{base}.source"),
                        format!("AC source references unknown component {source}"),
                    ),
                }

                let start_valid = validate_exact_quantity(
                    *start_frequency,
                    Unit::Hertz,
                    "CC-SIM-ANALYSIS-005",
                    format!("{base}.start_frequency"),
                    "AC start frequency",
                    diagnostics,
                );
                let stop_valid = validate_exact_quantity(
                    *stop_frequency,
                    Unit::Hertz,
                    "CC-SIM-ANALYSIS-005",
                    format!("{base}.stop_frequency"),
                    "AC stop frequency",
                    diagnostics,
                );
                if start_valid && start_frequency.coefficient <= 0 {
                    push(
                        diagnostics,
                        "CC-SIM-ANALYSIS-005",
                        format!("{base}.start_frequency"),
                        "AC start frequency must be positive",
                    );
                }
                if stop_valid && stop_frequency.coefficient <= 0 {
                    push(
                        diagnostics,
                        "CC-SIM-ANALYSIS-005",
                        format!("{base}.stop_frequency"),
                        "AC stop frequency must be positive",
                    );
                }
                if start_valid
                    && stop_valid
                    && start_frequency.exact_cmp(*stop_frequency) != Some(Ordering::Less)
                {
                    push(
                        diagnostics,
                        "CC-SIM-ANALYSIS-005",
                        format!("{base}.stop_frequency"),
                        "AC stop frequency must be greater than its start frequency",
                    );
                }

                if validate_exact_quantity(
                    *magnitude,
                    Unit::Volt,
                    "CC-SIM-ANALYSIS-006",
                    format!("{base}.magnitude"),
                    "AC source magnitude",
                    diagnostics,
                ) && magnitude.coefficient < 0
                {
                    push(
                        diagnostics,
                        "CC-SIM-ANALYSIS-006",
                        format!("{base}.magnitude"),
                        "AC source magnitude must be non-negative",
                    );
                }
                validate_exact_quantity(
                    *phase,
                    Unit::Degree,
                    "CC-SIM-ANALYSIS-007",
                    format!("{base}.phase"),
                    "AC source phase",
                    diagnostics,
                );
            }
            SimulationAnalysisKind::Transient {
                step, stop, start, ..
            } => {
                let step_valid = validate_exact_quantity(
                    *step,
                    Unit::Second,
                    "CC-SIM-ANALYSIS-008",
                    format!("{base}.step"),
                    "transient step",
                    diagnostics,
                );
                let stop_valid = validate_exact_quantity(
                    *stop,
                    Unit::Second,
                    "CC-SIM-ANALYSIS-008",
                    format!("{base}.stop"),
                    "transient stop",
                    diagnostics,
                );
                let start_valid = validate_exact_quantity(
                    *start,
                    Unit::Second,
                    "CC-SIM-ANALYSIS-008",
                    format!("{base}.start"),
                    "transient start",
                    diagnostics,
                );
                if step_valid && step.coefficient <= 0 {
                    push(
                        diagnostics,
                        "CC-SIM-ANALYSIS-009",
                        format!("{base}.step"),
                        "transient step must be positive",
                    );
                }
                if stop_valid && stop.coefficient <= 0 {
                    push(
                        diagnostics,
                        "CC-SIM-ANALYSIS-009",
                        format!("{base}.stop"),
                        "transient stop must be positive",
                    );
                }
                if start_valid && start.coefficient < 0 {
                    push(
                        diagnostics,
                        "CC-SIM-ANALYSIS-009",
                        format!("{base}.start"),
                        "transient start must be non-negative",
                    );
                }
                let interval_valid = step_valid
                    && stop_valid
                    && start_valid
                    && step.coefficient > 0
                    && stop.coefficient > 0
                    && start.coefficient >= 0
                    && start.exact_cmp(*stop) != Some(Ordering::Greater);
                if step_valid
                    && stop_valid
                    && start_valid
                    && start.exact_cmp(*stop) == Some(Ordering::Greater)
                {
                    push(
                        diagnostics,
                        "CC-SIM-ANALYSIS-009",
                        format!("{base}.start"),
                        "transient start must not be greater than its stop",
                    );
                }
                if interval_valid
                    && !transient_nominal_grid_is_bounded(*step, *stop, MAX_SIMULATION_SAMPLES)
                {
                    push(
                        diagnostics,
                        "CC-SIM-ANALYSIS-010",
                        format!("{base}.step"),
                        format!(
                            "inclusive transient grid exceeds {MAX_SIMULATION_SAMPLES} samples"
                        ),
                    );
                }
            }
        }
    }

    let mut assertion_paths = BTreeSet::new();
    for assertion in assertions {
        let base = simulation_intent_path("assertions", &assertion.path);
        if !semantic_path_is_valid(&assertion.path) {
            push(
                diagnostics,
                "CC-SIM-ASSERTION-001",
                format!("{base}.path"),
                "assertion path must be a non-empty canonical semantic path",
            );
        }
        if !assertion_paths.insert(assertion.path.as_str()) {
            push(
                diagnostics,
                "CC-SIM-ASSERTION-002",
                format!("{base}.path"),
                format!("duplicate assertion path {}", assertion.path),
            );
        }

        let analysis = if !semantic_path_is_valid(&assertion.analysis_path) {
            push(
                diagnostics,
                "CC-SIM-ASSERTION-003",
                format!("{base}.analysis_path"),
                "assertion analysis path must be a canonical semantic path",
            );
            None
        } else {
            match analyses_by_path.get(assertion.analysis_path.as_str()) {
                Some(analysis) => Some(*analysis),
                None => {
                    push(
                        diagnostics,
                        "CC-SIM-ASSERTION-003",
                        format!("{base}.analysis_path"),
                        format!(
                            "assertion references unknown analysis {}",
                            assertion.analysis_path
                        ),
                    );
                    None
                }
            }
        };

        if !token_is_valid(&assertion.net) || !net_names.contains(assertion.net.as_str()) {
            push(
                diagnostics,
                "CC-SIM-ASSERTION-004",
                format!("{base}.net"),
                format!(
                    "assertion references unknown or invalid net {}",
                    assertion.net
                ),
            );
        }

        if let Some(analysis) = analysis {
            validate_assertion_sample(assertion, analysis, &base, diagnostics);
        }
        validate_exact_quantity(
            assertion.expected,
            Unit::Volt,
            "CC-SIM-ASSERTION-008",
            format!("{base}.expected"),
            "assertion expected value",
            diagnostics,
        );
        if validate_exact_quantity(
            assertion.absolute_tolerance,
            Unit::Volt,
            "CC-SIM-ASSERTION-009",
            format!("{base}.absolute_tolerance"),
            "assertion absolute tolerance",
            diagnostics,
        ) && assertion.absolute_tolerance.coefficient < 0
        {
            push(
                diagnostics,
                "CC-SIM-ASSERTION-009",
                format!("{base}.absolute_tolerance"),
                "assertion absolute tolerance must be non-negative",
            );
        }
        if validate_exact_quantity(
            assertion.relative_tolerance,
            Unit::Dimensionless,
            "CC-SIM-ASSERTION-010",
            format!("{base}.relative_tolerance"),
            "assertion relative tolerance",
            diagnostics,
        ) && assertion.relative_tolerance.coefficient < 0
        {
            push(
                diagnostics,
                "CC-SIM-ASSERTION-010",
                format!("{base}.relative_tolerance"),
                "assertion relative tolerance must be non-negative",
            );
        }
    }
}

fn validate_assertion_sample(
    assertion: &SimulationAssertion,
    analysis: &SimulationAnalysis,
    base: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let analysis_base = simulation_intent_path("analyses", &analysis.path);
    match (&analysis.kind, &assertion.sample) {
        (SimulationAnalysisKind::DcOperatingPoint, SimulationSample::Scalar) => {}
        (
            SimulationAnalysisKind::AcLinearSweep {
                points,
                start_frequency,
                stop_frequency,
                ..
            },
            SimulationSample::Frequency(sample),
        ) => {
            let sample_valid = validate_exact_quantity(
                *sample,
                Unit::Hertz,
                "CC-SIM-ASSERTION-006",
                format!("{base}.sample"),
                "AC assertion sample",
                diagnostics,
            );
            let in_range = sample.exact_cmp(*start_frequency) != Some(Ordering::Less)
                && sample.exact_cmp(*stop_frequency) != Some(Ordering::Greater);
            if sample_valid && !in_range {
                push_related(
                    diagnostics,
                    "CC-SIM-ASSERTION-007",
                    format!("{base}.sample"),
                    analysis_base,
                    "AC assertion sample must lie inside the inclusive sweep range",
                );
            } else if sample_valid
                && in_range
                && *points >= 2
                && *points <= MAX_SIMULATION_SAMPLES
                && exact_quantity_is(*start_frequency, Unit::Hertz)
                && exact_quantity_is(*stop_frequency, Unit::Hertz)
                && start_frequency.coefficient > 0
                && start_frequency.exact_cmp(*stop_frequency) == Some(Ordering::Less)
                && ac_grid_index(*sample, *start_frequency, *stop_frequency, *points).is_none()
            {
                push_related(
                    diagnostics,
                    "CC-SIM-ASSERTION-007",
                    format!("{base}.sample"),
                    analysis_base,
                    "AC assertion sample must exactly equal a generated linear-grid sample",
                );
            }
        }
        (
            SimulationAnalysisKind::Transient {
                step, start, stop, ..
            },
            SimulationSample::Time(sample),
        ) => {
            let sample_valid = validate_exact_quantity(
                *sample,
                Unit::Second,
                "CC-SIM-ASSERTION-006",
                format!("{base}.sample"),
                "transient assertion sample",
                diagnostics,
            );
            let in_range = sample.exact_cmp(*start) != Some(Ordering::Less)
                && sample.exact_cmp(*stop) != Some(Ordering::Greater);
            if sample_valid && !in_range {
                push_related(
                    diagnostics,
                    "CC-SIM-ASSERTION-007",
                    format!("{base}.sample"),
                    analysis_base,
                    "transient assertion sample must lie inside the inclusive analysis interval",
                );
            } else if sample_valid
                && in_range
                && exact_quantity_is(*step, Unit::Second)
                && exact_quantity_is(*start, Unit::Second)
                && exact_quantity_is(*stop, Unit::Second)
                && step.coefficient > 0
                && start.coefficient >= 0
                && start.exact_cmp(*stop) != Some(Ordering::Greater)
                && transient_grid_location(*sample, *step, *stop).is_none()
            {
                push_related(
                    diagnostics,
                    "CC-SIM-ASSERTION-007",
                    format!("{base}.sample"),
                    analysis_base,
                    "transient assertion sample must exactly equal a zero-anchored step sample or the forced stop endpoint",
                );
            }
        }
        _ => push_related(
            diagnostics,
            "CC-SIM-ASSERTION-005",
            format!("{base}.sample"),
            analysis_base,
            "assertion sample kind does not match its analysis kind",
        ),
    }
}

fn validate_exact_quantity(
    quantity: Quantity,
    unit: Unit,
    code: &'static str,
    path: String,
    label: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let valid = exact_quantity_is(quantity, unit);
    if !valid {
        push(
            diagnostics,
            code,
            path,
            format!(
                "{label} must be a canonical exact {} quantity with exponent in [-18, 18]",
                unit.symbol()
            ),
        );
    }
    valid
}

fn exact_quantity_is(quantity: Quantity, unit: Unit) -> bool {
    quantity.unit == unit && quantity.exponent_is_valid() && quantity.is_canonical()
}

fn simulation_intent_path(collection: &str, path: &str) -> String {
    let path = if path.is_empty() { "<empty>" } else { path };
    format!("design.{collection}.{path}")
}

pub(crate) fn ac_grid_index(
    sample: Quantity,
    start: Quantity,
    stop: Quantity,
    points: u32,
) -> Option<u32> {
    if sample.unit != start.unit || start.unit != stop.unit {
        return None;
    }
    let intervals = points.checked_sub(1).filter(|intervals| *intervals != 0)?;
    let common_exponent = sample.exponent.min(start.exponent).min(stop.exponent);
    let offset = decimal_quantity_difference(sample, start, common_exponent)?;
    let span = decimal_quantity_difference(stop, start, common_exponent)?;
    let numerator = multiply_decimal_digits(&offset, intervals);
    let index = exact_decimal_digits_quotient_u32(&numerator, &span)?;
    (index < points).then_some(index)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransientGridLocation {
    Step(u32),
    StopEndpoint,
}

pub(crate) fn transient_grid_location(
    sample: Quantity,
    step: Quantity,
    stop: Quantity,
) -> Option<TransientGridLocation> {
    if sample.unit != step.unit || step.unit != stop.unit {
        return None;
    }
    if sample.exact_cmp(stop) == Some(Ordering::Equal) {
        return Some(TransientGridLocation::StopEndpoint);
    }
    if sample.exact_cmp(stop) != Some(Ordering::Less) {
        return None;
    }
    let common_exponent = sample.exponent.min(step.exponent);
    let sample = scaled_nonnegative_digits(sample, common_exponent, 1)?;
    let step = scaled_nonnegative_digits(step, common_exponent, 1)?;
    exact_decimal_digits_quotient_u32(&sample, &step).map(TransientGridLocation::Step)
}

fn decimal_quantity_difference(
    greater: Quantity,
    lesser: Quantity,
    common_exponent: i8,
) -> Option<String> {
    let greater = scaled_nonnegative_digits(greater, common_exponent, 1)?;
    let lesser = scaled_nonnegative_digits(lesser, common_exponent, 1)?;
    subtract_decimal_digits(&greater, &lesser)
}

/// Test whether `ceil(stop / step) + 1` fits the compute-step cap without
/// lowering exact decimals to floating point or scaling them into a fixed-width
/// integer. The pinned backend integrates from zero even when output filtering
/// starts later. The caller supplies validated, non-negative quantities.
fn transient_nominal_grid_is_bounded(step: Quantity, stop: Quantity, cap: u32) -> bool {
    transient_nominal_samples_saturating(step, stop, cap.saturating_add(1)) <= cap
}

/// Return the declared nominal-grid cardinality `ceil(stop / step) + 1`,
/// saturating at `saturation`, using exact decimal-string scaling. Binary
/// search bounds work to the public declaration envelope instead of iterating
/// over requested samples. The adaptive backend's actual step count is not
/// predicted here.
pub(crate) fn transient_nominal_samples_saturating(
    step: Quantity,
    stop: Quantity,
    saturation: u32,
) -> u32 {
    if saturation <= 1 || step.coefficient <= 0 || stop.coefficient < 0 {
        return saturation;
    }
    let common_exponent = step.exponent.min(stop.exponent);
    let Some(step_digits) = scaled_nonnegative_digits(step, common_exponent, 1) else {
        return saturation;
    };
    let Some(stop_digits) = scaled_nonnegative_digits(stop, common_exponent, 1) else {
        return saturation;
    };
    let max_intervals = saturation - 1;
    let max_time = multiply_decimal_digits(&step_digits, max_intervals);
    if compare_decimal_digits(&max_time, &stop_digits) == Ordering::Less {
        return saturation;
    }

    let mut lower = 0_u32;
    let mut upper = max_intervals;
    while lower < upper {
        let midpoint = lower + (upper - lower) / 2;
        let time = multiply_decimal_digits(&step_digits, midpoint);
        if compare_decimal_digits(&time, &stop_digits) == Ordering::Less {
            lower = midpoint + 1;
        } else {
            upper = midpoint;
        }
    }
    lower.saturating_add(1).min(saturation)
}

fn scaled_nonnegative_digits(
    quantity: Quantity,
    common_exponent: i8,
    multiplier: u32,
) -> Option<String> {
    let coefficient = u64::try_from(quantity.coefficient).ok()?;
    let shift = u8::try_from(quantity.exponent.checked_sub(common_exponent)?).ok()?;
    let mut digits = multiply_decimal_digits(&coefficient.to_string(), multiplier);
    digits.extend(std::iter::repeat_n('0', usize::from(shift)));
    Some(digits)
}

fn multiply_decimal_digits(value: &str, multiplier: u32) -> String {
    if multiplier == 0 || decimal_digits_without_leading_zeroes(value) == "0" {
        return "0".to_owned();
    }
    let mut carry = 0_u64;
    let mut result = Vec::new();
    for digit in value.bytes().rev() {
        let product = u64::from(digit - b'0') * u64::from(multiplier) + carry;
        result.push(b'0' + u8::try_from(product % 10).unwrap_or(0));
        carry = product / 10;
    }
    while carry > 0 {
        result.push(b'0' + u8::try_from(carry % 10).unwrap_or(0));
        carry /= 10;
    }
    result.reverse();
    result.into_iter().map(char::from).collect()
}

fn compare_decimal_digits(left: &str, right: &str) -> Ordering {
    let left = decimal_digits_without_leading_zeroes(left);
    let right = decimal_digits_without_leading_zeroes(right);
    left.len()
        .cmp(&right.len())
        .then_with(|| left.as_bytes().cmp(right.as_bytes()))
}

fn subtract_decimal_digits(left: &str, right: &str) -> Option<String> {
    if compare_decimal_digits(left, right) == Ordering::Less {
        return None;
    }
    let left = decimal_digits_without_leading_zeroes(left).as_bytes();
    let right = decimal_digits_without_leading_zeroes(right).as_bytes();
    let mut borrow = 0_i16;
    let mut result = Vec::with_capacity(left.len());
    for offset in 0..left.len() {
        let left_digit = i16::from(left[left.len() - 1 - offset] - b'0') - borrow;
        let right_digit = right
            .len()
            .checked_sub(offset + 1)
            .map(|index| i16::from(right[index] - b'0'))
            .unwrap_or(0);
        let (digit, next_borrow) = if left_digit < right_digit {
            (left_digit + 10 - right_digit, 1)
        } else {
            (left_digit - right_digit, 0)
        };
        result.push(b'0' + u8::try_from(digit).ok()?);
        borrow = next_borrow;
    }
    if borrow != 0 {
        return None;
    }
    result.reverse();
    let result: String = result.into_iter().map(char::from).collect();
    Some(decimal_digits_without_leading_zeroes(&result).to_owned())
}

fn exact_decimal_digits_quotient_u32(dividend: &str, divisor: &str) -> Option<u32> {
    let divisor = decimal_digits_without_leading_zeroes(divisor);
    if divisor == "0" {
        return None;
    }
    let mut remainder = "0".to_owned();
    let mut quotient = 0_u32;
    for digit in decimal_digits_without_leading_zeroes(dividend).bytes() {
        if remainder == "0" {
            remainder.clear();
        }
        remainder.push(char::from(digit));
        remainder = decimal_digits_without_leading_zeroes(&remainder).to_owned();
        let mut quotient_digit = 0_u32;
        while compare_decimal_digits(&remainder, divisor) != Ordering::Less {
            remainder = subtract_decimal_digits(&remainder, divisor)?;
            quotient_digit += 1;
        }
        quotient = quotient.checked_mul(10)?.checked_add(quotient_digit)?;
    }
    (decimal_digits_without_leading_zeroes(&remainder) == "0").then_some(quotient)
}

fn decimal_digits_without_leading_zeroes(value: &str) -> &str {
    let value = value.trim_start_matches('0');
    if value.is_empty() { "0" } else { value }
}

fn validate_outline(outline: RectNm, diagnostics: &mut Vec<Diagnostic>) {
    if outline.size.width <= 0 || outline.size.height <= 0 {
        push(
            diagnostics,
            "CC-BOARD-001",
            "design.board.outline",
            "board outline dimensions must be positive",
        );
    }
    validate_point(
        outline.origin,
        "origin",
        "design.board.outline",
        "CC-BOARD-002",
        diagnostics,
    );
    validate_size_envelope(
        outline.size.width,
        "size.width",
        "design.board.outline",
        "CC-BOARD-002",
        diagnostics,
    );
    validate_size_envelope(
        outline.size.height,
        "size.height",
        "design.board.outline",
        "CC-BOARD-002",
        diagnostics,
    );

    match (
        outline.origin.x.checked_add(outline.size.width),
        outline.origin.y.checked_add(outline.size.height),
    ) {
        (Some(max_x), Some(max_y)) => validate_point(
            PointNm::new(max_x, max_y),
            "far_corner",
            "design.board.outline",
            "CC-BOARD-004",
            diagnostics,
        ),
        _ => push(
            diagnostics,
            "CC-BOARD-003",
            "design.board.outline",
            "board outline arithmetic overflows signed 64-bit coordinates",
        ),
    }
}

fn validate_point(
    point: PointNm,
    name: &str,
    path: &str,
    code: &'static str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (axis, coordinate) in [("x", point.x), ("y", point.y)] {
        if coordinate.unsigned_abs() > MAX_ABS_COORDINATE_NM as u64 {
            push(
                diagnostics,
                code,
                path,
                format!("{name}.{axis} exceeds the Design IR coordinate envelope"),
            );
        }
    }
}

fn validate_size_envelope(
    size: i64,
    name: &str,
    path: &str,
    code: &'static str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if size.unsigned_abs() > MAX_ABS_COORDINATE_NM as u64 {
        push(
            diagnostics,
            code,
            path,
            format!("{name} exceeds the Design IR coordinate envelope"),
        );
    }
}

fn validate_component<'a>(
    component: &'a Component,
    outline: RectNm,
    net_names: &BTreeSet<&str>,
    module_paths: &BTreeSet<&str>,
    paths: &mut BTreeSet<&'a str>,
    references: &mut BTreeSet<&'a str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let path = if component.path.is_empty() {
        "design.components[unknown]"
    } else {
        component.path.as_str()
    };
    if !semantic_path_is_valid(&component.path) {
        push(
            diagnostics,
            "CC-COMP-001",
            path,
            "component semantic path is invalid",
        );
    }
    if !paths.insert(component.path.as_str()) {
        push(
            diagnostics,
            "CC-COMP-002",
            path,
            "duplicate component semantic path",
        );
    }
    let module_path = component.module_path();
    if module_path.is_none_or(|module_path| !module_paths.contains(module_path)) {
        push(
            diagnostics,
            "CC-COMP-007",
            path,
            match module_path {
                Some(module_path) => format!("component references unknown module {module_path}"),
                None => "component semantic path has no parent module".to_owned(),
            },
        );
    }
    if !token_is_valid(&component.reference) {
        push(
            diagnostics,
            "CC-COMP-003",
            path,
            "component reference must be a non-empty canonical token",
        );
    }
    if !references.insert(component.reference.as_str()) {
        push(
            diagnostics,
            "CC-COMP-004",
            path,
            format!("duplicate component reference {}", component.reference),
        );
    }
    if component.physical.is_none() && component.simulation.is_none() {
        push(
            diagnostics,
            "CC-COMP-005",
            path,
            "component must have a physical implementation, a simulation model, or both",
        );
    }
    validate_part_identity(component, path, diagnostics);

    if component.symbol.library_id.trim().is_empty()
        || component.symbol.library_id.chars().any(char::is_control)
    {
        push(
            diagnostics,
            "CC-SYMBOL-001",
            path,
            "symbol library identifier must be non-empty and contain no control characters",
        );
    }
    if component.symbol.pins.is_empty() {
        push(
            diagnostics,
            "CC-SYMBOL-002",
            path,
            "symbol must bind at least one logical pin",
        );
    }
    let mut symbol_pins = BTreeSet::new();
    let mut logical_pins = BTreeSet::new();
    for binding in &component.symbol.pins {
        if !token_is_valid(&binding.pin) || !token_is_valid(&binding.symbol_pin) {
            push(
                diagnostics,
                "CC-SYMBOL-003",
                path,
                "symbol pin bindings must use non-empty canonical tokens",
            );
        }
        if !logical_pins.insert(binding.pin.as_str()) {
            push(
                diagnostics,
                "CC-SYMBOL-004",
                path,
                format!("logical pin {} is bound more than once", binding.pin),
            );
        }
        if !symbol_pins.insert(binding.symbol_pin.as_str()) {
            push(
                diagnostics,
                "CC-SYMBOL-005",
                path,
                format!("symbol pin {} is bound more than once", binding.symbol_pin),
            );
        }
    }
    if !matches!(
        component
            .schematic_placement
            .rotation_degrees
            .rem_euclid(360),
        0 | 90 | 180 | 270
    ) {
        push(
            diagnostics,
            "CC-SCHEMATIC-001",
            path,
            "schematic rotation must be a multiple of 90 degrees",
        );
    }
    validate_point(
        component.schematic_placement.position,
        "schematic.position",
        path,
        "CC-SCHEMATIC-002",
        diagnostics,
    );

    let mut connections = BTreeMap::new();
    for connection in &component.connections {
        if !token_is_valid(&connection.pin) {
            push(
                diagnostics,
                "CC-PIN-001",
                path,
                "pin name must be a non-empty canonical token",
            );
        }
        if connections
            .insert(connection.pin.as_str(), connection)
            .is_some()
        {
            push(
                diagnostics,
                "CC-PIN-002",
                path,
                format!("pin {} is connected more than once", connection.pin),
            );
        }
        if !logical_pins.contains(connection.pin.as_str()) {
            push(
                diagnostics,
                "CC-PIN-003",
                path,
                format!(
                    "connection references unknown logical pin {}",
                    connection.pin
                ),
            );
        }
        if let ConnectionState::Connected(net) = &connection.state
            && !net_names.contains(net.as_str())
        {
            push(
                diagnostics,
                "CC-PIN-004",
                path,
                format!("pin {} references unknown net {net}", connection.pin),
            );
        }
    }
    for pin in logical_pins {
        if !connections.contains_key(pin) {
            push(
                diagnostics,
                "CC-PIN-005",
                path,
                format!("symbol logical pin {pin} has no explicit connection state"),
            );
        }
    }

    validate_component_value(component, path, diagnostics);

    if let Some(physical) = &component.physical {
        validate_physical(component, physical, outline, &connections, diagnostics);
    }
    if let Some(simulation) = &component.simulation {
        validate_simulation(component, simulation, &connections, path, diagnostics);
    }
}

fn is_physical_part(component: &Component) -> bool {
    component.part.manufacturer.is_some()
}

fn validate_part_identity(component: &Component, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let part = &component.part;
    if !token_is_valid(&part.logical_function) {
        push(
            diagnostics,
            "CC-PRODUCT-PART-001",
            path,
            "logical function must be a non-empty canonical token",
        );
    }

    let physical = is_physical_part(component);
    if physical
        && (part.manufacturer.is_none()
            || part.manufacturer_part_number.is_none()
            || part.package.is_none()
            || part.lifecycle.is_none()
            || part.sourcing.is_none())
    {
        push(
            diagnostics,
            "CC-PRODUCT-PART-002",
            path,
            "physical component requires manufacturer, manufacturer part number, package, lifecycle, and sourcing constraints",
        );
    }
    if !physical
        && (part.manufacturer.is_some()
            || part.manufacturer_part_number.is_some()
            || part.package.is_some()
            || part.lifecycle.is_some()
            || part.sourcing.is_some()
            || !part.approved_substitutions.is_empty())
    {
        push(
            diagnostics,
            "CC-PRODUCT-PART-003",
            path,
            "virtual component must not carry physical identity, lifecycle, sourcing, or substitution intent",
        );
    }

    for (field, value) in [
        ("manufacturer", part.manufacturer.as_deref()),
        (
            "manufacturer part number",
            part.manufacturer_part_number.as_deref(),
        ),
        ("package", part.package.as_deref()),
    ] {
        if value.is_some_and(|value| !canonical_text_is_valid(value)) {
            push(
                diagnostics,
                "CC-PRODUCT-PART-004",
                path,
                format!(
                    "{field} must be non-empty canonical text without surrounding whitespace or control characters"
                ),
            );
        }
    }

    if let Some(sourcing) = &part.sourcing {
        if sourcing.minimum_available_quantity == 0 || sourcing.maximum_lead_time_days == 0 {
            push(
                diagnostics,
                "CC-PRODUCT-PART-005",
                path,
                "sourcing minimum available quantity and maximum lead time days must be positive",
            );
        }
        if !token_is_valid(&sourcing.required_region) {
            push(
                diagnostics,
                "CC-PRODUCT-PART-006",
                path,
                "sourcing required region must be a non-empty canonical token",
            );
        }
    }

    if part.approved_substitutions.len() > MAX_PRODUCT_SUBSTITUTIONS {
        push(
            diagnostics,
            "CC-PRODUCT-PART-009",
            path,
            format!(
                "part declares {} approved substitutions; the maximum is {MAX_PRODUCT_SUBSTITUTIONS}",
                part.approved_substitutions.len()
            ),
        );
    } else {
        let mut substitutions = BTreeSet::new();
        for substitution in &part.approved_substitutions {
            if !approved_substitution_is_valid(substitution) {
                push(
                    diagnostics,
                    "CC-PRODUCT-PART-007",
                    path,
                    "approved substitution manufacturer, manufacturer part number, and package must be canonical text",
                );
            }
            if !substitutions.insert(substitution) {
                push(
                    diagnostics,
                    "CC-PRODUCT-PART-008",
                    path,
                    "approved substitutions must be unique exact manufacturer, part-number, and package tuples",
                );
            }
            if part
                .package
                .as_deref()
                .is_some_and(|package| substitution.package != package)
            {
                push(
                    diagnostics,
                    "CC-PRODUCT-PART-010",
                    path,
                    "approved substitution package must exactly match the base part package",
                );
            }
            if matches!(
                (
                    part.manufacturer.as_deref(),
                    part.manufacturer_part_number.as_deref(),
                    part.package.as_deref(),
                ),
                (Some(manufacturer), Some(number), Some(package))
                    if substitution.manufacturer == manufacturer
                        && substitution.manufacturer_part_number == number
                        && substitution.package == package
            ) {
                push(
                    diagnostics,
                    "CC-PRODUCT-PART-011",
                    path,
                    "approved substitution must not repeat the exact base part identity",
                );
            }
        }
    }
}

fn product_variant_work(
    variants: &[ProductVariant],
    physical_component_count: usize,
) -> Option<usize> {
    variants.iter().try_fold(0_usize, |total, variant| {
        total
            .checked_add(variant.components.len())?
            .checked_add(physical_component_count)
    })
}

fn product_variant_resources_are_bounded(
    variants: &[ProductVariant],
    physical_component_count: usize,
) -> bool {
    variants.len() <= MAX_PRODUCT_VARIANTS
        && product_variant_work(variants, physical_component_count)
            .is_some_and(|work| work <= MAX_PRODUCT_VARIANT_WORK)
        && variants.iter().all(|variant| {
            variant.configurations.len() <= MAX_PRODUCT_CONFIGURATIONS
                && variant.configurations.iter().all(|configuration| {
                    configuration.key.len() <= MAX_PRODUCT_CONFIGURATION_KEY_BYTES
                        && configuration.value.len() <= MAX_PRODUCT_CONFIGURATION_VALUE_BYTES
                })
        })
}

fn validate_product_variant_resources(
    variants: &[ProductVariant],
    physical_component_count: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let diagnostic_count = diagnostics.len();
    if variants.len() > MAX_PRODUCT_VARIANTS {
        push(
            diagnostics,
            "CC-PRODUCT-VARIANT-011",
            "design.product.variants",
            format!(
                "design declares {} product variants; the maximum is {MAX_PRODUCT_VARIANTS}",
                variants.len()
            ),
        );
        return false;
    }
    if product_variant_work(variants, physical_component_count)
        .is_none_or(|work| work > MAX_PRODUCT_VARIANT_WORK)
    {
        push(
            diagnostics,
            "CC-PRODUCT-VARIANT-012",
            "design.product.variants",
            format!(
                "aggregate product variant assignment and totality work exceeds {MAX_PRODUCT_VARIANT_WORK} entries"
            ),
        );
    }
    for variant in variants {
        let variant_path = if variant.path.is_empty() {
            "design.product.variants[unknown]"
        } else {
            variant.path.as_str()
        };
        if variant.configurations.len() > MAX_PRODUCT_CONFIGURATIONS {
            push(
                diagnostics,
                "CC-PRODUCT-CONFIG-004",
                variant_path,
                format!(
                    "product variant declares {} configurations; the maximum is {MAX_PRODUCT_CONFIGURATIONS}",
                    variant.configurations.len()
                ),
            );
            continue;
        }
        for configuration in &variant.configurations {
            if configuration.key.len() > MAX_PRODUCT_CONFIGURATION_KEY_BYTES {
                push(
                    diagnostics,
                    "CC-PRODUCT-CONFIG-005",
                    variant_path,
                    format!(
                        "product configuration key exceeds {MAX_PRODUCT_CONFIGURATION_KEY_BYTES} UTF-8 bytes"
                    ),
                );
            }
            if configuration.value.len() > MAX_PRODUCT_CONFIGURATION_VALUE_BYTES {
                push(
                    diagnostics,
                    "CC-PRODUCT-CONFIG-006",
                    variant_path,
                    format!(
                        "product configuration value exceeds {MAX_PRODUCT_CONFIGURATION_VALUE_BYTES} UTF-8 bytes"
                    ),
                );
            }
        }
    }
    diagnostics.len() == diagnostic_count
}

fn manufacturability_assertion_count(analyses: &[ManufacturabilityAnalysis]) -> Option<usize> {
    analyses.iter().try_fold(0_usize, |total, analysis| {
        total.checked_add(analysis.assertions.len())
    })
}

fn validate_product_intent(
    product: &ProductIntent,
    components: &[Component],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let component_by_path: BTreeMap<_, _> = components
        .iter()
        .map(|component| (component.path.as_str(), component))
        .collect();
    let physical_paths: BTreeSet<_> = components
        .iter()
        .filter(|component| is_physical_part(component))
        .map(|component| component.path.as_str())
        .collect();

    if !physical_paths.is_empty() && product.catalog.is_none() {
        push(
            diagnostics,
            "CC-PRODUCT-CATALOG-001",
            "design.product.catalog",
            "a design with physical components requires pinned catalog evidence",
        );
    }
    if let Some(catalog) = &product.catalog {
        if !token_is_valid(&catalog.snapshot_id) {
            push(
                diagnostics,
                "CC-PRODUCT-CATALOG-002",
                "design.product.catalog.snapshot_id",
                "catalog snapshot identity must be a non-empty canonical token",
            );
        }
        if !sha256_is_valid(&catalog.sha256) {
            push(
                diagnostics,
                "CC-PRODUCT-CATALOG-003",
                "design.product.catalog.sha256",
                "catalog digest must be exactly 64 lowercase hexadecimal SHA-256 characters",
            );
        }
        if !date_is_valid(&catalog.evaluated_on) {
            push(
                diagnostics,
                "CC-PRODUCT-CATALOG-004",
                "design.product.catalog.evaluated_on",
                "catalog evaluation date must be a real Gregorian date in YYYY-MM-DD form",
            );
        }
    }

    if !physical_paths.is_empty() && product.variants.is_empty() {
        push(
            diagnostics,
            "CC-PRODUCT-VARIANT-001",
            "design.product.variants",
            "a design with physical components requires at least one product variant",
        );
    }

    if validate_product_variant_resources(&product.variants, physical_paths.len(), diagnostics) {
        let mut variants: Vec<_> = product.variants.iter().collect();
        variants.sort_by(|left, right| left.path.cmp(&right.path));
        let mut variant_paths = BTreeSet::new();
        for variant in variants {
            let variant_path = if variant.path.is_empty() {
                "design.product.variants[unknown]"
            } else {
                variant.path.as_str()
            };
            if !semantic_path_is_valid(&variant.path) {
                push(
                    diagnostics,
                    "CC-PRODUCT-VARIANT-002",
                    variant_path,
                    "product variant path must be a canonical semantic path",
                );
            }
            if !variant_paths.insert(variant.path.as_str()) {
                push(
                    diagnostics,
                    "CC-PRODUCT-VARIANT-003",
                    variant_path,
                    "product variant paths must be unique",
                );
            }
            if variant.build_quantity == 0 {
                push(
                    diagnostics,
                    "CC-PRODUCT-VARIANT-004",
                    variant_path,
                    "product variant build quantity must be positive",
                );
            }

            let mut assignments: Vec<_> = variant.components.iter().collect();
            assignments.sort_by(|left, right| {
                (&left.component_path, &left.state).cmp(&(&right.component_path, &right.state))
            });
            let mut assigned_paths = BTreeSet::new();
            for assignment in assignments {
                if !semantic_path_is_valid(&assignment.component_path) {
                    push(
                        diagnostics,
                        "CC-PRODUCT-VARIANT-005",
                        variant_path,
                        format!(
                            "variant component path {} is not a canonical semantic path",
                            assignment.component_path
                        ),
                    );
                }
                if !assigned_paths.insert(assignment.component_path.as_str()) {
                    push(
                        diagnostics,
                        "CC-PRODUCT-VARIANT-006",
                        variant_path,
                        format!(
                            "physical component {} is assigned more than once",
                            assignment.component_path
                        ),
                    );
                }
                match component_by_path.get(assignment.component_path.as_str()) {
                    None => push(
                        diagnostics,
                        "CC-PRODUCT-VARIANT-007",
                        variant_path,
                        format!(
                            "variant assignment references unknown component {}",
                            assignment.component_path
                        ),
                    ),
                    Some(component) if !is_physical_part(component) => push(
                        diagnostics,
                        "CC-PRODUCT-VARIANT-008",
                        variant_path,
                        format!(
                            "variant assignment references virtual component {}",
                            assignment.component_path
                        ),
                    ),
                    Some(component) => {
                        if let PopulationState::Alternate(substitution) = &assignment.state
                            && !component.part.approved_substitutions.contains(substitution)
                        {
                            push(
                                diagnostics,
                                "CC-PRODUCT-VARIANT-010",
                                variant_path,
                                format!(
                                    "alternate for component {} is not an exact approved substitution",
                                    assignment.component_path
                                ),
                            );
                        }
                    }
                }
            }
            for physical_path in &physical_paths {
                if !assigned_paths.contains(physical_path) {
                    push(
                        diagnostics,
                        "CC-PRODUCT-VARIANT-009",
                        variant_path,
                        format!("physical component {physical_path} has no variant assignment"),
                    );
                }
            }

            let mut configurations: Vec<_> = variant.configurations.iter().collect();
            configurations.sort();
            let mut configuration_keys = BTreeSet::new();
            for configuration in configurations {
                if !token_is_valid(&configuration.key) {
                    push(
                        diagnostics,
                        "CC-PRODUCT-CONFIG-001",
                        variant_path,
                        "product configuration key must be a non-empty canonical token",
                    );
                }
                if !canonical_text_is_valid(&configuration.value) {
                    push(
                        diagnostics,
                        "CC-PRODUCT-CONFIG-002",
                        variant_path,
                        "product configuration value must be non-empty canonical text",
                    );
                }
                if !configuration_keys.insert(configuration.key.as_str()) {
                    push(
                        diagnostics,
                        "CC-PRODUCT-CONFIG-003",
                        variant_path,
                        format!("duplicate product configuration key {}", configuration.key),
                    );
                }
            }
        }
    }

    if product.manufacturability_analyses.len() > MAX_MANUFACTURABILITY_ANALYSES {
        push(
            diagnostics,
            "CC-PRODUCT-ANALYSIS-004",
            "design.product.manufacturability_analyses",
            format!(
                "design declares {} manufacturability analyses; the maximum is {MAX_MANUFACTURABILITY_ANALYSES}",
                product.manufacturability_analyses.len()
            ),
        );
    } else if manufacturability_assertion_count(&product.manufacturability_analyses)
        .is_none_or(|count| count > MAX_MANUFACTURABILITY_ASSERTIONS)
    {
        push(
            diagnostics,
            "CC-PRODUCT-ASSERTION-004",
            "design.product.manufacturability_analyses",
            format!(
                "aggregate manufacturability assertions exceed {MAX_MANUFACTURABILITY_ASSERTIONS} entries"
            ),
        );
    } else {
        let mut analyses: Vec<_> = product.manufacturability_analyses.iter().collect();
        analyses.sort_by(|left, right| left.path.cmp(&right.path));
        let mut analysis_paths = BTreeSet::new();
        let mut assertion_paths = BTreeSet::new();
        for analysis in analyses {
            let analysis_path = if analysis.path.is_empty() {
                "design.product.manufacturability_analyses[unknown]"
            } else {
                analysis.path.as_str()
            };
            if !semantic_path_is_valid(&analysis.path) {
                push(
                    diagnostics,
                    "CC-PRODUCT-ANALYSIS-001",
                    analysis_path,
                    "manufacturability analysis path must be a canonical semantic path",
                );
            }
            if !analysis_paths.insert(analysis.path.as_str()) {
                push(
                    diagnostics,
                    "CC-PRODUCT-ANALYSIS-002",
                    analysis_path,
                    "manufacturability analysis paths must be unique",
                );
            }
            if analysis.adapter != "kicad" || analysis.version != "10" {
                push(
                    diagnostics,
                    "CC-PRODUCT-ANALYSIS-003",
                    analysis_path,
                    "initial manufacturability analysis adapter must be exactly kicad version 10",
                );
            }

            let mut assertions: Vec<_> = analysis.assertions.iter().collect();
            assertions.sort();
            let mut capabilities = BTreeSet::new();
            for assertion in assertions {
                if !semantic_path_is_valid(&assertion.path) {
                    push(
                        diagnostics,
                        "CC-PRODUCT-ASSERTION-001",
                        analysis_path,
                        format!(
                            "manufacturability assertion path {} is not a canonical semantic path",
                            assertion.path
                        ),
                    );
                }
                if !assertion_paths.insert(assertion.path.as_str()) {
                    push(
                        diagnostics,
                        "CC-PRODUCT-ASSERTION-002",
                        analysis_path,
                        format!(
                            "duplicate manufacturability assertion path {}",
                            assertion.path
                        ),
                    );
                }
                if !capabilities.insert(assertion.capability) {
                    push(
                        diagnostics,
                        "CC-PRODUCT-ASSERTION-003",
                        analysis_path,
                        "manufacturability capabilities must be unique within one analysis",
                    );
                }
            }
        }
    }
}

fn validate_component_value(component: &Component, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let quantity = component.value.quantity();
    if !quantity.is_canonical() {
        push(
            diagnostics,
            "CC-VALUE-001",
            path,
            "component value must use its canonical exact decimal representation",
        );
    }
    if !quantity.exponent_is_valid() {
        push(
            diagnostics,
            "CC-VALUE-002",
            path,
            format!(
                "component value exponent {} is outside [-18, 18]",
                quantity.exponent
            ),
        );
    }

    match &component.value {
        ComponentValue::Resistance(resistance) => {
            if component.part.logical_function != "resistor" {
                push(
                    diagnostics,
                    "CC-VALUE-003",
                    path,
                    "resistance value requires logical device resistor",
                );
            }
            if resistance.unit != Unit::Ohm || resistance.coefficient <= 0 {
                push(
                    diagnostics,
                    "CC-VALUE-004",
                    path,
                    "resistor value must be a positive resistance",
                );
            }
        }
        ComponentValue::DcVoltage(voltage) => {
            if component.part.logical_function != "dc_voltage_source" {
                push(
                    diagnostics,
                    "CC-VALUE-003",
                    path,
                    "DC voltage value requires logical device dc_voltage_source",
                );
            }
            if voltage.unit != Unit::Volt {
                push(
                    diagnostics,
                    "CC-VALUE-005",
                    path,
                    "DC voltage value must have voltage dimension",
                );
            }
        }
    }
}

fn validate_physical(
    component: &Component,
    physical: &PhysicalImplementation,
    outline: RectNm,
    connections: &BTreeMap<&str, &Connection>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let path = component.path.as_str();
    if physical.footprint.library_id.trim().is_empty()
        || physical.footprint.library_id.chars().any(char::is_control)
    {
        push(
            diagnostics,
            "CC-PHYS-001",
            path,
            "footprint library identifier must be non-empty and contain no control characters",
        );
    }
    if physical.footprint.pads.is_empty() {
        push(
            diagnostics,
            "CC-PHYS-002",
            path,
            "physical footprint must contain at least one pad",
        );
    }
    if !matches!(
        physical.placement.rotation_degrees.rem_euclid(360),
        0 | 90 | 180 | 270
    ) {
        push(
            diagnostics,
            "CC-PHYS-003",
            path,
            "bootstrap placement rotation must be a multiple of 90 degrees",
        );
    }
    validate_point(
        physical.placement.position,
        "placement.position",
        path,
        "CC-PHYS-004",
        diagnostics,
    );
    if !outline.contains(physical.placement.position) {
        push(
            diagnostics,
            "CC-PHYS-005",
            path,
            "component placement lies outside the board outline",
        );
    }

    let mut bindings_by_pad = BTreeMap::new();
    let mut bound_pins = BTreeSet::new();
    for binding in &physical.pin_pad_bindings {
        if !token_is_valid(&binding.pin) || !token_is_valid(&binding.pad) {
            push(
                diagnostics,
                "CC-BIND-001",
                path,
                "pin-to-pad binding names must be non-empty canonical tokens",
            );
        }
        if !connections.contains_key(binding.pin.as_str()) {
            push(
                diagnostics,
                "CC-BIND-002",
                path,
                format!(
                    "pin-to-pad binding references unknown logical pin {}",
                    binding.pin
                ),
            );
        }
        if bindings_by_pad
            .insert(binding.pad.as_str(), binding.pin.as_str())
            .is_some()
        {
            push(
                diagnostics,
                "CC-BIND-003",
                path,
                format!("physical pad {} is bound more than once", binding.pad),
            );
        }
        bound_pins.insert(binding.pin.as_str());
    }

    let mut pad_numbers = BTreeSet::new();
    for pad in &physical.footprint.pads {
        if !token_is_valid(&pad.number) {
            push(
                diagnostics,
                "CC-PAD-001",
                path,
                "pad number must be a non-empty canonical token",
            );
        }
        if !pad_numbers.insert(pad.number.as_str()) {
            push(
                diagnostics,
                "CC-PAD-002",
                path,
                format!("duplicate pad number {}", pad.number),
            );
        }
        if pad.size.width <= 0 || pad.size.height <= 0 {
            push(
                diagnostics,
                "CC-PAD-003",
                path,
                format!("pad {} dimensions must be positive", pad.number),
            );
        }
        validate_point(pad.offset, "pad.offset", path, "CC-PAD-008", diagnostics);
        validate_size_envelope(
            pad.size.width,
            "pad.size.width",
            path,
            "CC-PAD-009",
            diagnostics,
        );
        validate_size_envelope(
            pad.size.height,
            "pad.size.height",
            path,
            "CC-PAD-009",
            diagnostics,
        );
        if !bindings_by_pad.contains_key(pad.number.as_str()) {
            push(
                diagnostics,
                "CC-PAD-004",
                path,
                format!("pad {} has no explicit logical-pin binding", pad.number),
            );
        }
        match physical.placement.transform(pad.offset) {
            Some(center) => {
                validate_point(center, "pad.center", path, "CC-PAD-010", diagnostics);
                if !outline.contains(center) {
                    push(
                        diagnostics,
                        "CC-PAD-005",
                        path,
                        format!("pad {} center lies outside the board outline", pad.number),
                    );
                }
            }
            None => push(
                diagnostics,
                "CC-PAD-006",
                path,
                format!("pad {} placement transform is invalid", pad.number),
            ),
        }
    }

    for binding in &physical.pin_pad_bindings {
        if !pad_numbers.contains(binding.pad.as_str()) {
            push(
                diagnostics,
                "CC-PAD-007",
                path,
                format!(
                    "pin-to-pad binding references unknown physical pad {}",
                    binding.pad
                ),
            );
        }
    }
    for (pin, connection) in connections {
        if matches!(connection.state, ConnectionState::Connected(_)) && !bound_pins.contains(pin) {
            push(
                diagnostics,
                "CC-BIND-004",
                path,
                format!("connected logical pin {pin} has no physical pad binding"),
            );
        }
    }
}

fn validate_simulation(
    component: &Component,
    simulation: &SimulationModel,
    connections: &BTreeMap<&str, &Connection>,
    path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let (positive_pin, negative_pin) = simulation.pins();
    if positive_pin == negative_pin {
        push(
            diagnostics,
            "CC-SIM-008",
            path,
            "simulation terminals must reference distinct logical pins",
        );
    }
    for pin in [positive_pin, negative_pin] {
        match connections.get(pin).map(|connection| &connection.state) {
            Some(ConnectionState::Connected(_)) => {}
            _ => push(
                diagnostics,
                "CC-SIM-003",
                path,
                format!("simulation terminal references unconnected pin {pin}"),
            ),
        }
    }

    match simulation {
        SimulationModel::Resistor { model_id, .. } => {
            if component.part.logical_function != "resistor" {
                push(
                    diagnostics,
                    "CC-SIM-011",
                    path,
                    "resistor simulation model requires logical device resistor",
                );
            }
            if model_id != "spice:R" {
                push(
                    diagnostics,
                    "CC-SIM-010",
                    path,
                    format!("unsupported resistor model identifier {model_id}"),
                );
            }
        }
        SimulationModel::DcVoltageSource { model_id, .. } => {
            if component.part.logical_function != "dc_voltage_source" {
                push(
                    diagnostics,
                    "CC-SIM-011",
                    path,
                    "DC voltage-source simulation model requires logical device dc_voltage_source",
                );
            }
            if model_id != "spice:Vdc" {
                push(
                    diagnostics,
                    "CC-SIM-010",
                    path,
                    format!("unsupported voltage-source model identifier {model_id}"),
                );
            }
        }
    }
}

fn canonical_text_is_valid(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && !value.chars().any(char::is_control)
}

fn approved_substitution_is_valid(substitution: &ApprovedSubstitution) -> bool {
    canonical_text_is_valid(&substitution.manufacturer)
        && canonical_text_is_valid(&substitution.manufacturer_part_number)
        && canonical_text_is_valid(&substitution.package)
}

fn sha256_is_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn date_is_valid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let decimal = |mut range: std::ops::Range<usize>| {
        range.try_fold(0_u32, |result, index| {
            bytes[index]
                .is_ascii_digit()
                .then(|| result * 10 + u32::from(bytes[index] - b'0'))
        })
    };
    let (Some(year), Some(month), Some(day)) = (decimal(0..4), decimal(5..7), decimal(8..10))
    else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap_year =
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let maximum_day = match month {
        2 if leap_year => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=maximum_day).contains(&day)
}

fn token_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_+-./".contains(character))
}

pub(crate) fn artifact_name_is_valid(value: &str) -> bool {
    value
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn semantic_path_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(token_is_valid)
        && !value.starts_with('.')
        && !value.ends_with('.')
}

fn push(
    diagnostics: &mut Vec<Diagnostic>,
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic {
        code,
        path: path.into(),
        related_path: None,
        message: message.into(),
    });
}

fn push_related(
    diagnostics: &mut Vec<Diagnostic>,
    code: &'static str,
    path: impl Into<String>,
    related_path: impl Into<String>,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic {
        code,
        path: path.into(),
        related_path: Some(related_path.into()),
        message: message.into(),
    });
}

#[cfg(test)]
mod tests {
    use std::panic::catch_unwind;

    use crate::demo::voltage_divider;
    use crate::quantity::{Quantity, Unit};

    use super::{
        ApprovedSubstitution, CatalogEvidenceRef, ComponentValue, ConnectionState, CopperLayer,
        LifecycleStatus, MAX_ABS_COORDINATE_NM, MAX_MANUFACTURABILITY_ANALYSES,
        MAX_MANUFACTURABILITY_ASSERTIONS, MAX_PRODUCT_CONFIGURATION_KEY_BYTES,
        MAX_PRODUCT_CONFIGURATION_VALUE_BYTES, MAX_PRODUCT_CONFIGURATIONS,
        MAX_PRODUCT_SUBSTITUTIONS, MAX_PRODUCT_VARIANT_WORK, MAX_PRODUCT_VARIANTS,
        MAX_SIMULATION_ANALYSES, MAX_SIMULATION_ASSERTIONS, MAX_SIMULATION_SAMPLES,
        MAX_SIMULATION_TOTAL_SAMPLES, ManufacturabilityAnalysis, ManufacturabilityAssertion,
        ManufacturabilityCapability, PinPadBinding, Placement, PointNm, PopulationState,
        ProductConfiguration, ProductIntent, ProductVariant, RoutingRequest, SimulationAnalysis,
        SimulationAnalysisKind, SimulationAssertion, SimulationSample, SourcingConstraints,
        TransientGridLocation, VariantComponent, ac_grid_index, transient_grid_location,
    };

    fn has_code(diagnostics: &[super::Diagnostic], code: &str) -> bool {
        diagnostics.iter().any(|diagnostic| diagnostic.code == code)
    }

    fn assert_rejected(design: super::Design, code: &str) {
        let diagnostics = design
            .validate()
            .expect_err("mutated Design IR must be rejected");
        assert!(
            has_code(&diagnostics, code),
            "missing diagnostic {code}: {diagnostics:#?}"
        );
    }

    fn rejected_diagnostics(design: super::Design) -> Vec<super::Diagnostic> {
        design
            .validate()
            .expect_err("mutated Design IR must be rejected")
    }

    fn approved_substitution() -> ApprovedSubstitution {
        ApprovedSubstitution {
            manufacturer: "Panasonic".to_owned(),
            manufacturer_part_number: "ERJ-3EKF1002V".to_owned(),
            package: "0603".to_owned(),
        }
    }

    fn product_design() -> super::Design {
        let mut design = voltage_divider();
        for component in &mut design.components {
            if component.physical.is_some() {
                component.part.package = Some("0603".to_owned());
                component.part.lifecycle = Some(LifecycleStatus::Active);
                component.part.sourcing = Some(SourcingConstraints {
                    minimum_available_quantity: 100,
                    maximum_lead_time_days: 30,
                    required_region: "US".to_owned(),
                });
                component.part.approved_substitutions = vec![approved_substitution()];
            } else {
                component.part.manufacturer = None;
                component.part.manufacturer_part_number = None;
                component.part.package = None;
                component.part.lifecycle = None;
                component.part.sourcing = None;
                component.part.approved_substitutions.clear();
            }
        }
        design.product = ProductIntent {
            catalog: Some(CatalogEvidenceRef {
                snapshot_id: "catalog-2026-08-04".to_owned(),
                sha256: "a".repeat(64),
                evaluated_on: "2026-08-04".to_owned(),
            }),
            variants: vec![ProductVariant {
                path: "product.variants.default".to_owned(),
                build_quantity: 100,
                components: vec![
                    VariantComponent {
                        component_path: "divider.r_bottom".to_owned(),
                        state: PopulationState::Fitted,
                    },
                    VariantComponent {
                        component_path: "divider.r_top".to_owned(),
                        state: PopulationState::Fitted,
                    },
                ],
                configurations: vec![
                    ProductConfiguration {
                        key: "finish".to_owned(),
                        value: "lead-free".to_owned(),
                    },
                    ProductConfiguration {
                        key: "assembly_line".to_owned(),
                        value: "line-a".to_owned(),
                    },
                ],
            }],
            manufacturability_analyses: vec![ManufacturabilityAnalysis {
                path: "product.manufacturability.kicad10".to_owned(),
                adapter: "kicad".to_owned(),
                version: "10".to_owned(),
                assertions: vec![
                    ManufacturabilityAssertion {
                        path: "product.assertions.fabrication_inventory".to_owned(),
                        capability: ManufacturabilityCapability::FabricationInventoryComplete,
                    },
                    ManufacturabilityAssertion {
                        path: "product.assertions.schematic_parity".to_owned(),
                        capability: ManufacturabilityCapability::SchematicParityClean,
                    },
                    ManufacturabilityAssertion {
                        path: "product.assertions.unconnected".to_owned(),
                        capability: ManufacturabilityCapability::UnconnectedClean,
                    },
                    ManufacturabilityAssertion {
                        path: "product.assertions.drc".to_owned(),
                        capability: ManufacturabilityCapability::DrcClean,
                    },
                    ManufacturabilityAssertion {
                        path: "product.assertions.erc".to_owned(),
                        capability: ManufacturabilityCapability::ErcClean,
                    },
                ],
            }],
        };
        design.canonicalize();
        design
    }

    fn physical_part_mut<'a>(
        design: &'a mut super::Design,
        reference: &str,
    ) -> &'a mut super::PartIdentity {
        &mut design
            .components
            .iter_mut()
            .find(|component| component.reference == reference)
            .expect("physical product fixture component must exist")
            .part
    }

    fn variant_mut(design: &mut super::Design) -> &mut ProductVariant {
        design
            .product
            .variants
            .iter_mut()
            .find(|variant| variant.path == "product.variants.default")
            .expect("default product variant must exist")
    }

    fn manufacturability_analysis_mut(
        design: &mut super::Design,
    ) -> &mut ManufacturabilityAnalysis {
        design
            .product
            .manufacturability_analyses
            .iter_mut()
            .find(|analysis| analysis.path == "product.manufacturability.kicad10")
            .expect("KiCad manufacturability analysis must exist")
    }

    fn vout_routing_request() -> RoutingRequest {
        RoutingRequest {
            path: "board.autoroute.vout".to_owned(),
            net: "VOUT".to_owned(),
            width_nm: 250_000,
            clearance_nm: 200_000,
            grid_step_nm: 250_000,
            layer: CopperLayer::Front,
        }
    }

    fn design_with_simulation_intent() -> super::Design {
        let mut design = voltage_divider();
        design.analyses = vec![
            SimulationAnalysis {
                path: "divider.simulation.dc".to_owned(),
                kind: SimulationAnalysisKind::DcOperatingPoint,
            },
            SimulationAnalysis {
                path: "divider.simulation.ac".to_owned(),
                kind: SimulationAnalysisKind::AcLinearSweep {
                    source: "divider.analysis.input".to_owned(),
                    points: 11,
                    start_frequency: Quantity::new(10, 0, Unit::Hertz),
                    stop_frequency: Quantity::new(100, 0, Unit::Hertz),
                    magnitude: Quantity::new(1, 0, Unit::Volt),
                    phase: Quantity::new(0, 0, Unit::Degree),
                },
            },
            SimulationAnalysis {
                path: "divider.simulation.transient".to_owned(),
                kind: SimulationAnalysisKind::Transient {
                    step: Quantity::new(1, -3, Unit::Second),
                    stop: Quantity::new(1, 0, Unit::Second),
                    start: Quantity::new(0, 0, Unit::Second),
                    uic: false,
                },
            },
        ];
        design.assertions = vec![
            simulation_assertion(
                "divider.assertions.dc_vout",
                "divider.simulation.dc",
                SimulationSample::Scalar,
            ),
            simulation_assertion(
                "divider.assertions.ac_vout",
                "divider.simulation.ac",
                SimulationSample::Frequency(Quantity::new(10, 0, Unit::Hertz)),
            ),
            simulation_assertion(
                "divider.assertions.transient_vout",
                "divider.simulation.transient",
                SimulationSample::Time(Quantity::new(500, -3, Unit::Second)),
            ),
        ];
        design.canonicalize();
        design
    }

    fn simulation_assertion(
        path: &str,
        analysis_path: &str,
        sample: SimulationSample,
    ) -> SimulationAssertion {
        SimulationAssertion {
            path: path.to_owned(),
            analysis_path: analysis_path.to_owned(),
            net: "VOUT".to_owned(),
            sample,
            expected: Quantity::new(5, 0, Unit::Volt),
            absolute_tolerance: Quantity::new(1, -6, Unit::Volt),
            relative_tolerance: Quantity::new(1, -3, Unit::Dimensionless),
        }
    }

    fn analysis_kind_mut<'a>(
        design: &'a mut super::Design,
        path: &str,
    ) -> &'a mut SimulationAnalysisKind {
        &mut design
            .analyses
            .iter_mut()
            .find(|analysis| analysis.path == path)
            .expect("simulation analysis fixture must exist")
            .kind
    }

    fn assertion_mut<'a>(design: &'a mut super::Design, path: &str) -> &'a mut SimulationAssertion {
        design
            .assertions
            .iter_mut()
            .find(|assertion| assertion.path == path)
            .expect("simulation assertion fixture must exist")
    }

    #[test]
    fn reference_design_is_valid() {
        let design = voltage_divider();
        assert!(design.analyses.is_empty());
        assert!(design.assertions.is_empty());
        assert_eq!(design.validate(), Ok(()));
    }

    #[test]
    fn validates_dc_ac_and_transient_intent_with_typed_assertions() {
        assert_eq!(design_with_simulation_intent().validate(), Ok(()));
    }

    #[test]
    fn declared_analyses_require_explicit_model_coverage_and_simulation_ground() {
        let mut uncovered = design_with_simulation_intent();
        uncovered
            .components
            .iter_mut()
            .find(|component| component.reference == "R1")
            .expect("reference resistor must exist")
            .simulation = None;
        let mut reversed = uncovered.clone();
        reversed.analyses.reverse();
        for candidate in [uncovered, reversed] {
            let diagnostics = candidate
                .validate()
                .expect_err("uncovered participating component must fail");
            let capability = diagnostics
                .iter()
                .find(|diagnostic| diagnostic.code == "CC-SIM-CAPABILITY-001")
                .expect("capability diagnostic must exist");
            assert_eq!(capability.path, "divider.r_top");
            assert_eq!(
                capability.related_path.as_deref(),
                Some("design.analyses.divider.simulation.ac")
            );
        }

        let mut no_models_or_ground = voltage_divider();
        for component in &mut no_models_or_ground.components {
            component.simulation = None;
        }
        for net in &mut no_models_or_ground.nets {
            net.is_ground = false;
        }
        no_models_or_ground.analyses.push(SimulationAnalysis {
            path: "divider.simulation.dc".to_owned(),
            kind: SimulationAnalysisKind::DcOperatingPoint,
        });
        assert_rejected(no_models_or_ground, "CC-SIM-001");
    }

    #[test]
    fn canonicalizes_simulation_intent_independent_of_order_and_decimal_spelling() {
        let expected = design_with_simulation_intent();
        let mut permuted = expected.clone();
        permuted.analyses.reverse();
        permuted.assertions.reverse();
        if let SimulationAnalysisKind::AcLinearSweep {
            start_frequency,
            stop_frequency,
            ..
        } = analysis_kind_mut(&mut permuted, "divider.simulation.ac")
        {
            *start_frequency = Quantity {
                coefficient: 10_000,
                exponent: -3,
                unit: Unit::Hertz,
            };
            *stop_frequency = Quantity {
                coefficient: 100_000,
                exponent: -3,
                unit: Unit::Hertz,
            };
        } else {
            unreachable!();
        }
        assertion_mut(&mut permuted, "divider.assertions.dc_vout").absolute_tolerance = Quantity {
            coefficient: 1_000,
            exponent: -9,
            unit: Unit::Volt,
        };

        permuted.canonicalize();

        assert_eq!(permuted, expected);
    }

    #[test]
    fn rejects_invalid_and_duplicate_simulation_semantic_paths() {
        let mut design = design_with_simulation_intent();
        design.analyses[0].path.clear();
        let diagnostics = design
            .validate()
            .expect_err("empty analysis path must be rejected");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-SIM-ANALYSIS-001")
            .expect("analysis path diagnostic must exist");
        assert_eq!(diagnostic.path, "design.analyses.<empty>.path");

        let mut design = design_with_simulation_intent();
        design.assertions[0].path.clear();
        let diagnostics = design
            .validate()
            .expect_err("empty assertion path must be rejected");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-SIM-ASSERTION-001")
            .expect("assertion path diagnostic must exist");
        assert_eq!(diagnostic.path, "design.assertions.<empty>.path");

        let mut design = design_with_simulation_intent();
        design.analyses.push(design.analyses[0].clone());
        assert_rejected(design, "CC-SIM-ANALYSIS-002");

        let mut design = design_with_simulation_intent();
        design.assertions.push(design.assertions[0].clone());
        assert_rejected(design, "CC-SIM-ASSERTION-002");
    }

    #[test]
    fn validates_ac_sweep_source_range_units_and_sample_cap() {
        let mut two_point = design_with_simulation_intent();
        two_point
            .analyses
            .retain(|analysis| analysis.path == "divider.simulation.ac");
        two_point.assertions.clear();
        if let SimulationAnalysisKind::AcLinearSweep { points, .. } =
            analysis_kind_mut(&mut two_point, "divider.simulation.ac")
        {
            *points = 2;
        } else {
            unreachable!();
        }
        assert_eq!(two_point.validate(), Ok(()));

        let mut design = design_with_simulation_intent();
        design
            .analyses
            .retain(|analysis| analysis.path == "divider.simulation.ac");
        design
            .assertions
            .retain(|assertion| assertion.path == "divider.assertions.ac_vout");
        if let SimulationAnalysisKind::AcLinearSweep { points, .. } =
            analysis_kind_mut(&mut design, "divider.simulation.ac")
        {
            *points = MAX_SIMULATION_SAMPLES;
        } else {
            unreachable!();
        }
        assert_eq!(design.validate(), Ok(()));

        let mut design = design_with_simulation_intent();
        if let SimulationAnalysisKind::AcLinearSweep { points, .. } =
            analysis_kind_mut(&mut design, "divider.simulation.ac")
        {
            *points = 1;
        } else {
            unreachable!();
        }
        assert_rejected(design, "CC-SIM-ANALYSIS-003");

        let mut design = design_with_simulation_intent();
        if let SimulationAnalysisKind::AcLinearSweep { points, .. } =
            analysis_kind_mut(&mut design, "divider.simulation.ac")
        {
            *points = MAX_SIMULATION_SAMPLES + 1;
        } else {
            unreachable!();
        }
        assertion_mut(&mut design, "divider.assertions.ac_vout").sample =
            SimulationSample::Frequency(Quantity::new(56, 0, Unit::Hertz));
        let diagnostics = design
            .validate()
            .expect_err("oversized AC sweep must be rejected");
        let points = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-SIM-ANALYSIS-003")
            .expect("AC points diagnostic must exist");
        assert_eq!(points.path, "design.analyses.divider.simulation.ac.points");
        assert!(
            !has_code(&diagnostics, "CC-SIM-ASSERTION-007"),
            "an invalid AC point count must not cascade into a grid-membership diagnostic"
        );

        let mut design = design_with_simulation_intent();
        if let SimulationAnalysisKind::AcLinearSweep { source, .. } =
            analysis_kind_mut(&mut design, "divider.simulation.ac")
        {
            *source = "divider.r_top".to_owned();
        } else {
            unreachable!();
        }
        assert_rejected(design, "CC-SIM-ANALYSIS-004");

        let mut design = design_with_simulation_intent();
        if let SimulationAnalysisKind::AcLinearSweep { source, .. } =
            analysis_kind_mut(&mut design, "divider.simulation.ac")
        {
            *source = "divider.missing_source".to_owned();
        } else {
            unreachable!();
        }
        assert_rejected(design, "CC-SIM-ANALYSIS-004");

        let mut design = design_with_simulation_intent();
        if let SimulationAnalysisKind::AcLinearSweep {
            start_frequency,
            stop_frequency,
            ..
        } = analysis_kind_mut(&mut design, "divider.simulation.ac")
        {
            *start_frequency = Quantity::new(100, 0, Unit::Hertz);
            *stop_frequency = Quantity::new(10, 0, Unit::Hertz);
        } else {
            unreachable!();
        }
        assert_rejected(design, "CC-SIM-ANALYSIS-005");

        let mut design = design_with_simulation_intent();
        if let SimulationAnalysisKind::AcLinearSweep {
            start_frequency, ..
        } = analysis_kind_mut(&mut design, "divider.simulation.ac")
        {
            *start_frequency = Quantity::new(0, 0, Unit::Hertz);
        } else {
            unreachable!();
        }
        assert_rejected(design, "CC-SIM-ANALYSIS-005");

        let mut design = design_with_simulation_intent();
        if let SimulationAnalysisKind::AcLinearSweep {
            start_frequency, ..
        } = analysis_kind_mut(&mut design, "divider.simulation.ac")
        {
            *start_frequency = Quantity::new(10, 0, Unit::Second);
        } else {
            unreachable!();
        }
        assert_rejected(design, "CC-SIM-ANALYSIS-005");

        let mut design = design_with_simulation_intent();
        if let SimulationAnalysisKind::AcLinearSweep { magnitude, .. } =
            analysis_kind_mut(&mut design, "divider.simulation.ac")
        {
            *magnitude = Quantity::new(0, 0, Unit::Volt);
        } else {
            unreachable!();
        }
        assert_eq!(design.validate(), Ok(()));

        let mut design = design_with_simulation_intent();
        if let SimulationAnalysisKind::AcLinearSweep { magnitude, .. } =
            analysis_kind_mut(&mut design, "divider.simulation.ac")
        {
            *magnitude = Quantity::new(-1, 0, Unit::Volt);
        } else {
            unreachable!();
        }
        assert_rejected(design, "CC-SIM-ANALYSIS-006");

        let mut design = design_with_simulation_intent();
        if let SimulationAnalysisKind::AcLinearSweep { phase, .. } =
            analysis_kind_mut(&mut design, "divider.simulation.ac")
        {
            *phase = Quantity::new(0, 0, Unit::Dimensionless);
        } else {
            unreachable!();
        }
        assert_rejected(design, "CC-SIM-ANALYSIS-007");
    }

    #[test]
    fn validates_transient_interval_and_exact_inclusive_grid_cap() {
        let transient_path = "divider.simulation.transient";
        let assertion_path = "divider.assertions.transient_vout";

        let mut single_sample = design_with_simulation_intent();
        if let SimulationAnalysisKind::Transient { start, stop, .. } =
            analysis_kind_mut(&mut single_sample, transient_path)
        {
            *start = Quantity::new(1, 0, Unit::Second);
            *stop = Quantity::new(1, 0, Unit::Second);
        } else {
            unreachable!();
        }
        assertion_mut(&mut single_sample, assertion_path).sample =
            SimulationSample::Time(Quantity::new(1, 0, Unit::Second));
        assert_eq!(single_sample.validate(), Ok(()));

        let mut boundary = design_with_simulation_intent();
        boundary
            .analyses
            .retain(|analysis| analysis.path == transient_path);
        boundary
            .assertions
            .retain(|assertion| assertion.path == assertion_path);
        if let SimulationAnalysisKind::Transient {
            step, start, stop, ..
        } = analysis_kind_mut(&mut boundary, transient_path)
        {
            *step = Quantity::new(1, 18, Unit::Second);
            *start = Quantity::new(0, 0, Unit::Second);
            *stop = Quantity::new(9_999, 18, Unit::Second);
        } else {
            unreachable!();
        }
        assertion_mut(&mut boundary, assertion_path).sample =
            SimulationSample::Time(Quantity::new(0, 0, Unit::Second));
        assert_eq!(boundary.validate(), Ok(()));

        let mut oversized = boundary;
        if let SimulationAnalysisKind::Transient { stop, .. } =
            analysis_kind_mut(&mut oversized, transient_path)
        {
            *stop = Quantity::new(10_000, 18, Unit::Second);
        } else {
            unreachable!();
        }
        assert_rejected(oversized, "CC-SIM-ANALYSIS-010");

        let mut exponent_extreme = design_with_simulation_intent();
        if let SimulationAnalysisKind::Transient { step, stop, .. } =
            analysis_kind_mut(&mut exponent_extreme, transient_path)
        {
            *step = Quantity::new(1, -18, Unit::Second);
            *stop = Quantity::new(1, 18, Unit::Second);
        } else {
            unreachable!();
        }
        let diagnostics = exponent_extreme
            .validate()
            .expect_err("exponent-extreme transient workload must be rejected");
        assert!(has_code(&diagnostics, "CC-SIM-ANALYSIS-010"));
        assert!(has_code(&diagnostics, "CC-SIM-ANALYSIS-012"));

        let mut large_filtered_start = design_with_simulation_intent();
        if let SimulationAnalysisKind::Transient {
            step, start, stop, ..
        } = analysis_kind_mut(&mut large_filtered_start, transient_path)
        {
            *step = Quantity::new(1, 0, Unit::Second);
            *start = Quantity::new(1, 18, Unit::Second);
            *stop = Quantity::new(1_000_000_000_000_000_001, 0, Unit::Second);
        } else {
            unreachable!();
        }
        assertion_mut(&mut large_filtered_start, assertion_path).sample =
            SimulationSample::Time(Quantity::new(1_000_000_000_000_000_001, 0, Unit::Second));
        assert_rejected(large_filtered_start, "CC-SIM-ANALYSIS-010");

        let mut reversed = design_with_simulation_intent();
        if let SimulationAnalysisKind::Transient { start, stop, .. } =
            analysis_kind_mut(&mut reversed, transient_path)
        {
            *start = Quantity::new(2, 0, Unit::Second);
            *stop = Quantity::new(1, 0, Unit::Second);
        } else {
            unreachable!();
        }
        assert_rejected(reversed, "CC-SIM-ANALYSIS-009");

        let mut zero_step = design_with_simulation_intent();
        if let SimulationAnalysisKind::Transient { step, .. } =
            analysis_kind_mut(&mut zero_step, transient_path)
        {
            *step = Quantity::new(0, 0, Unit::Second);
        } else {
            unreachable!();
        }
        assert_rejected(zero_step, "CC-SIM-ANALYSIS-009");

        let mut wrong_unit = design_with_simulation_intent();
        if let SimulationAnalysisKind::Transient { stop, .. } =
            analysis_kind_mut(&mut wrong_unit, transient_path)
        {
            *stop = Quantity::new(1, 0, Unit::Hertz);
        } else {
            unreachable!();
        }
        assert_rejected(wrong_unit, "CC-SIM-ANALYSIS-008");
    }

    #[test]
    fn validates_assertion_context_sample_units_ranges_and_tolerances() {
        let assertion_path = "divider.assertions.ac_vout";

        let mut design = design_with_simulation_intent();
        assertion_mut(&mut design, assertion_path).analysis_path = "missing.analysis".to_owned();
        assert_rejected(design, "CC-SIM-ASSERTION-003");

        let mut design = design_with_simulation_intent();
        assertion_mut(&mut design, assertion_path).net = "MISSING".to_owned();
        assert_rejected(design, "CC-SIM-ASSERTION-004");

        let mut design = design_with_simulation_intent();
        assertion_mut(&mut design, assertion_path).sample = SimulationSample::Scalar;
        assert_rejected(design, "CC-SIM-ASSERTION-005");

        let mut design = design_with_simulation_intent();
        assertion_mut(&mut design, assertion_path).sample =
            SimulationSample::Frequency(Quantity::new(1, 0, Unit::Second));
        assert_rejected(design, "CC-SIM-ASSERTION-006");

        let mut design = design_with_simulation_intent();
        assertion_mut(&mut design, assertion_path).sample =
            SimulationSample::Frequency(Quantity::new(1, 3, Unit::Hertz));
        assert_rejected(design, "CC-SIM-ASSERTION-007");

        let mut design = design_with_simulation_intent();
        assertion_mut(&mut design, assertion_path).expected = Quantity::new(5, 0, Unit::Ampere);
        assert_rejected(design, "CC-SIM-ASSERTION-008");

        let mut design = design_with_simulation_intent();
        assertion_mut(&mut design, assertion_path).absolute_tolerance =
            Quantity::new(-1, -6, Unit::Volt);
        assert_rejected(design, "CC-SIM-ASSERTION-009");

        let mut design = design_with_simulation_intent();
        assertion_mut(&mut design, assertion_path).absolute_tolerance =
            Quantity::new(1, -6, Unit::Ampere);
        assert_rejected(design, "CC-SIM-ASSERTION-009");

        let mut design = design_with_simulation_intent();
        assertion_mut(&mut design, assertion_path).relative_tolerance =
            Quantity::new(-1, -3, Unit::Dimensionless);
        assert_rejected(design, "CC-SIM-ASSERTION-010");

        let mut design = design_with_simulation_intent();
        assertion_mut(&mut design, assertion_path).relative_tolerance =
            Quantity::new(1, -3, Unit::Volt);
        assert_rejected(design, "CC-SIM-ASSERTION-010");

        let mut design = design_with_simulation_intent();
        assertion_mut(&mut design, assertion_path).expected = Quantity {
            coefficient: 5_000,
            exponent: -3,
            unit: Unit::Volt,
        };
        assert_rejected(design, "CC-SIM-ASSERTION-008");
    }

    #[test]
    fn exact_simulation_grid_ordinals_cover_boundaries_and_large_decimals() {
        let hz = |coefficient, exponent| Quantity::new(coefficient, exponent, Unit::Hertz);
        assert_eq!(ac_grid_index(hz(10, 0), hz(10, 0), hz(100, 0), 11), Some(0));
        assert_eq!(ac_grid_index(hz(55, 0), hz(10, 0), hz(100, 0), 11), Some(5));
        assert_eq!(
            ac_grid_index(hz(100, 0), hz(10, 0), hz(100, 0), 11),
            Some(10)
        );
        assert_eq!(ac_grid_index(hz(56, 0), hz(10, 0), hz(100, 0), 11), None);
        assert_eq!(ac_grid_index(hz(145, 0), hz(10, 0), hz(100, 0), 11), None);
        assert_eq!(ac_grid_index(hz(10, 0), hz(10, 0), hz(100, 0), 1), None);

        assert_eq!(
            ac_grid_index(hz(18, -2), hz(1, -1), hz(2, -1), 6),
            Some(4),
            "finite-decimal interior point must return its exact ordinal"
        );
        assert_eq!(ac_grid_index(hz(19, -2), hz(1, -1), hz(2, -1), 6), None);

        let huge_start = hz(1, -18);
        let huge_stop = hz(i64::MAX, 18);
        assert_eq!(
            ac_grid_index(huge_stop, huge_start, huge_stop, MAX_SIMULATION_SAMPLES),
            Some(MAX_SIMULATION_SAMPLES - 1),
            "endpoint division must remain exact across the full decimal exponent envelope"
        );
        assert_eq!(
            ac_grid_index(
                hz(u32::MAX.into(), 0),
                hz(0, 0),
                hz(u32::MAX.into(), 0),
                u32::MAX
            ),
            Some(u32::MAX - 1),
            "the largest representable AC ordinal must not overflow"
        );
        assert_eq!(
            ac_grid_index(hz(i64::MAX, 18), hz(0, 0), hz(1, 0), 2),
            None,
            "an exact quotient outside u32 and the declared point range must be rejected"
        );

        let seconds = |coefficient, exponent| Quantity::new(coefficient, exponent, Unit::Second);
        let step = seconds(3, -1);
        let stop = seconds(1, 0);
        assert_eq!(
            transient_grid_location(seconds(0, 0), step, stop),
            Some(TransientGridLocation::Step(0))
        );
        assert_eq!(
            transient_grid_location(seconds(3, -1), step, stop),
            Some(TransientGridLocation::Step(1))
        );
        assert_eq!(
            transient_grid_location(seconds(9, -1), step, stop),
            Some(TransientGridLocation::Step(3))
        );
        assert_eq!(
            transient_grid_location(stop, seconds(0, 0), stop),
            Some(TransientGridLocation::StopEndpoint),
            "the forced stop endpoint is classified before step division"
        );
        assert_eq!(transient_grid_location(seconds(4, -1), step, stop), None);
        assert_eq!(transient_grid_location(seconds(12, -1), step, stop), None);
        assert_eq!(transient_grid_location(seconds(-3, -1), step, stop), None);

        assert_eq!(
            transient_grid_location(
                seconds(9, 18),
                seconds(3, 18),
                seconds(9_000_000_000_000_000_001, 0),
            ),
            Some(TransientGridLocation::Step(3)),
            "large exact decimal coefficients must divide without fixed-width scaling"
        );
        assert_eq!(
            transient_grid_location(
                seconds(i64::from(u32::MAX), -18),
                seconds(1, -18),
                seconds(i64::from(u32::MAX) + 2, -18),
            ),
            Some(TransientGridLocation::Step(u32::MAX))
        );
        assert_eq!(
            transient_grid_location(
                seconds(i64::from(u32::MAX) + 1, -18),
                seconds(1, -18),
                seconds(i64::from(u32::MAX) + 2, -18),
            ),
            None,
            "a transient step quotient above u32 must be rejected"
        );
    }

    #[test]
    fn assertion_samples_must_be_members_of_exact_backend_grids() {
        let ac_path = "divider.assertions.ac_vout";
        for frequency in [10, 55, 100] {
            let mut design = design_with_simulation_intent();
            assertion_mut(&mut design, ac_path).sample =
                SimulationSample::Frequency(Quantity::new(frequency, 0, Unit::Hertz));
            assert_eq!(design.validate(), Ok(()), "{frequency} Hz must be on-grid");
        }
        let mut off_grid_ac = design_with_simulation_intent();
        assertion_mut(&mut off_grid_ac, ac_path).sample =
            SimulationSample::Frequency(Quantity::new(56, 0, Unit::Hertz));
        assert_rejected(off_grid_ac, "CC-SIM-ASSERTION-007");

        let transient_path = "divider.simulation.transient";
        let assertion_path = "divider.assertions.transient_vout";
        let transient_fixture = || {
            let mut design = design_with_simulation_intent();
            if let SimulationAnalysisKind::Transient {
                step, start, stop, ..
            } = analysis_kind_mut(&mut design, transient_path)
            {
                *step = Quantity::new(3, -1, Unit::Second);
                *start = Quantity::new(1, -1, Unit::Second);
                *stop = Quantity::new(1, 0, Unit::Second);
            } else {
                unreachable!();
            }
            design
        };
        for sample in [
            Quantity::new(3, -1, Unit::Second),
            Quantity::new(6, -1, Unit::Second),
            Quantity::new(1, 0, Unit::Second),
        ] {
            let mut design = transient_fixture();
            assertion_mut(&mut design, assertion_path).sample = SimulationSample::Time(sample);
            assert_eq!(design.validate(), Ok(()), "{sample} must be on-grid");
        }

        let mut start_anchored_but_not_zero_anchored = transient_fixture();
        assertion_mut(&mut start_anchored_but_not_zero_anchored, assertion_path).sample =
            SimulationSample::Time(Quantity::new(4, -1, Unit::Second));
        assert_rejected(start_anchored_but_not_zero_anchored, "CC-SIM-ASSERTION-007");

        let mut outside_but_zero_anchored = transient_fixture();
        assertion_mut(&mut outside_but_zero_anchored, assertion_path).sample =
            SimulationSample::Time(Quantity::new(12, -1, Unit::Second));
        assert_rejected(outside_but_zero_anchored, "CC-SIM-ASSERTION-007");
    }

    #[test]
    fn rejects_oversized_simulation_intent_before_walking_entries() {
        let mut analyses = voltage_divider();
        analyses.analyses = vec![
            SimulationAnalysis {
                path: "bad path".to_owned(),
                kind: SimulationAnalysisKind::DcOperatingPoint,
            };
            MAX_SIMULATION_ANALYSES + 1
        ];
        let diagnostics = analyses
            .validate()
            .expect_err("oversized analysis collection must be rejected");
        assert!(has_code(&diagnostics, "CC-SIM-ANALYSIS-011"));
        assert!(!has_code(&diagnostics, "CC-SIM-ANALYSIS-001"));

        let mut assertions = design_with_simulation_intent();
        assertions.assertions =
            vec![
                simulation_assertion("bad path", "missing.analysis", SimulationSample::Scalar);
                MAX_SIMULATION_ASSERTIONS + 1
            ];
        let diagnostics = assertions
            .validate()
            .expect_err("oversized assertion collection must be rejected");
        assert!(has_code(&diagnostics, "CC-SIM-ASSERTION-011"));
        assert!(!has_code(&diagnostics, "CC-SIM-ASSERTION-001"));
    }

    #[test]
    fn enforces_analysis_count_and_aggregate_workload_envelopes() {
        let dc_analysis = |index: usize| SimulationAnalysis {
            path: format!("divider.simulation.dc_{index:03}"),
            kind: SimulationAnalysisKind::DcOperatingPoint,
        };

        let mut count_boundary = voltage_divider();
        count_boundary.analyses = (0..MAX_SIMULATION_ANALYSES).map(dc_analysis).collect();
        assert_eq!(count_boundary.validate(), Ok(()));

        let mut count_overflow = count_boundary;
        count_overflow
            .analyses
            .push(dc_analysis(MAX_SIMULATION_ANALYSES));
        assert_rejected(count_overflow, "CC-SIM-ANALYSIS-011");

        let mut aggregate_boundary = design_with_simulation_intent();
        aggregate_boundary
            .analyses
            .retain(|analysis| analysis.path == "divider.simulation.ac");
        aggregate_boundary.assertions.clear();
        if let SimulationAnalysisKind::AcLinearSweep { points, .. } =
            analysis_kind_mut(&mut aggregate_boundary, "divider.simulation.ac")
        {
            *points = MAX_SIMULATION_TOTAL_SAMPLES - 1;
        } else {
            unreachable!();
        }
        aggregate_boundary.analyses.push(dc_analysis(0));
        aggregate_boundary.canonicalize();
        assert_eq!(aggregate_boundary.validate(), Ok(()));

        let mut aggregate_overflow = aggregate_boundary;
        aggregate_overflow.analyses.push(dc_analysis(1));
        let mut permuted = aggregate_overflow.clone();
        permuted.analyses.reverse();

        for design in [aggregate_overflow, permuted] {
            let diagnostics = design
                .validate()
                .expect_err("aggregate workload over 10,000 must be rejected");
            let total = diagnostics
                .iter()
                .find(|diagnostic| diagnostic.code == "CC-SIM-ANALYSIS-012")
                .expect("aggregate workload diagnostic must exist");
            assert_eq!(total.path, "design.analyses");
        }
    }

    #[test]
    fn aggregates_exact_transient_nominal_samples_across_analyses() {
        let transient_analysis = |path: &str, stop_seconds: i64| SimulationAnalysis {
            path: path.to_owned(),
            kind: SimulationAnalysisKind::Transient {
                step: Quantity::new(1, 0, Unit::Second),
                stop: Quantity::new(stop_seconds, 0, Unit::Second),
                start: Quantity::new(0, 0, Unit::Second),
                uic: false,
            },
        };

        let mut boundary = voltage_divider();
        boundary.analyses = vec![
            transient_analysis("divider.simulation.transient_a", 4_999),
            transient_analysis("divider.simulation.transient_b", 4_999),
        ];
        assert_eq!(boundary.validate(), Ok(()));

        if let SimulationAnalysisKind::Transient { stop, .. } = &mut boundary.analyses[0].kind {
            *stop = Quantity::new(5_000, 0, Unit::Second);
        } else {
            unreachable!();
        }
        let diagnostics = boundary
            .validate()
            .expect_err("aggregate transient workload of 10,001 must fail");
        let total = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-SIM-ANALYSIS-012")
            .expect("aggregate transient workload diagnostic must exist");
        assert_eq!(total.path, "design.analyses");
        assert!(!has_code(&diagnostics, "CC-SIM-ANALYSIS-010"));
    }

    #[test]
    fn physical_only_component_retains_and_validates_its_exact_value() {
        let mut design = voltage_divider();
        let component = &mut design.components[0];
        component.simulation = None;
        component.connections[0].state = ConnectionState::NoConnect;

        assert_eq!(
            component.value,
            ComponentValue::Resistance(crate::quantity::Quantity::new(
                10,
                3,
                crate::quantity::Unit::Ohm,
            ))
        );
        assert_eq!(component.value_label(), "10kΩ");
        assert_eq!(design.validate(), Ok(()));
    }

    #[test]
    fn rejects_no_connect_simulation_terminal() {
        let mut design = voltage_divider();
        let component = design
            .components
            .iter_mut()
            .find(|component| component.reference == "R1")
            .expect("reference resistor must exist");
        let positive_pin = match component
            .simulation
            .as_ref()
            .expect("reference resistor must retain its simulation model")
        {
            super::SimulationModel::Resistor { positive_pin, .. } => positive_pin.clone(),
            super::SimulationModel::DcVoltageSource { .. } => {
                panic!("reference resistor must use a resistor simulation model")
            }
        };
        component
            .connections
            .iter_mut()
            .find(|connection| connection.pin == positive_pin)
            .expect("simulation terminal must resolve to a connection")
            .state = ConnectionState::NoConnect;

        assert_rejected(design, "CC-SIM-003");
    }

    #[test]
    fn rejects_value_kind_that_disagrees_with_the_component_contract() {
        let mut design = voltage_divider();
        design.components[0].value = ComponentValue::DcVoltage(crate::quantity::Quantity::new(
            10,
            0,
            crate::quantity::Unit::Volt,
        ));

        let diagnostics = design
            .validate()
            .expect_err("resistor with voltage value must be rejected");
        assert!(has_code(&diagnostics, "CC-VALUE-003"));
    }

    #[test]
    fn invalid_public_component_values_return_diagnostics_without_panicking() {
        let mut design = voltage_divider();
        design.components[0].value = ComponentValue::Resistance(crate::quantity::Quantity {
            coefficient: 10,
            exponent: 19,
            unit: crate::quantity::Unit::Ohm,
        });

        let result = catch_unwind(|| design.validate());
        let diagnostics = result
            .expect("public component value validation must not panic")
            .expect_err("out-of-contract exact value must fail validation");
        assert!(has_code(&diagnostics, "CC-VALUE-001"));
        assert!(has_code(&diagnostics, "CC-VALUE-002"));
    }

    #[test]
    fn rejects_unknown_connection_net() {
        let mut design = voltage_divider();
        design.components[0].connections[0].state =
            ConnectionState::Connected("MISSING".to_owned());
        let diagnostics = design.validate().expect_err("invalid net must be rejected");
        assert!(has_code(&diagnostics, "CC-PIN-004"));
    }

    #[test]
    fn design_name_is_a_safe_single_file_artifact_stem() {
        for name in ["", "+divider", "divider/rev_a", "divider.rev_a"] {
            let mut design = voltage_divider();
            design.name = name.to_owned();
            assert_rejected(design, "CC-IR-002");
        }
    }

    #[test]
    fn validates_module_and_port_contracts_for_public_ir_consumers() {
        let mut design = voltage_divider();
        design.modules.clear();
        assert_rejected(design, "CC-MODULE-001");

        let mut design = voltage_divider();
        design.modules[0].path = ".invalid".to_owned();
        assert_rejected(design, "CC-MODULE-002");

        let mut design = voltage_divider();
        design.modules.push(design.modules[0].clone());
        assert_rejected(design, "CC-MODULE-003");

        let mut design = voltage_divider();
        design.modules.push(super::ModuleInstance {
            path: "orphan.child".to_owned(),
            ports: Vec::new(),
        });
        assert_rejected(design, "CC-MODULE-004");

        let mut design = voltage_divider();
        design.modules[0].ports[0].name = "bad name".to_owned();
        assert_rejected(design, "CC-PORT-001");

        let mut design = voltage_divider();
        let duplicate = design.modules[0].ports[0].clone();
        design.modules[0].ports.push(duplicate);
        assert_rejected(design, "CC-PORT-002");

        let mut design = voltage_divider();
        design.modules[0].ports[0].state = ConnectionState::Connected("MISSING".to_owned());
        assert_rejected(design, "CC-PORT-003");
    }

    #[test]
    fn validates_component_part_and_symbol_contracts_for_public_ir_consumers() {
        let mut design = voltage_divider();
        design.components[0].path = "missing.r_top".to_owned();
        assert_rejected(design, "CC-COMP-007");

        let mut design = voltage_divider();
        design.components[0].path = "orphan".to_owned();
        assert_rejected(design, "CC-COMP-007");

        let mut design = voltage_divider();
        design.components[0].part.logical_function.clear();
        assert_rejected(design, "CC-PRODUCT-PART-001");

        let mut design = voltage_divider();
        design.components[0].part.manufacturer = None;
        design.components[0].part.manufacturer_part_number = None;
        assert_rejected(design, "CC-PRODUCT-PART-003");

        let mut design = voltage_divider();
        design.components[0].part.manufacturer_part_number = None;
        assert_rejected(design, "CC-PRODUCT-PART-002");

        let mut design = voltage_divider();
        design.components[0].symbol.library_id.clear();
        assert_rejected(design, "CC-SYMBOL-001");

        let mut design = voltage_divider();
        design.components[0].symbol.pins.clear();
        assert_rejected(design, "CC-SYMBOL-002");

        let mut design = voltage_divider();
        design.components[0].symbol.pins[0].pin = "bad pin".to_owned();
        assert_rejected(design, "CC-SYMBOL-003");

        let mut design = voltage_divider();
        let duplicate = design.components[0].symbol.pins[0].pin.clone();
        design.components[0].symbol.pins[1].pin = duplicate;
        assert_rejected(design, "CC-SYMBOL-004");

        let mut design = voltage_divider();
        let duplicate = design.components[0].symbol.pins[0].symbol_pin.clone();
        design.components[0].symbol.pins[1].symbol_pin = duplicate;
        assert_rejected(design, "CC-SYMBOL-005");

        let mut design = voltage_divider();
        design.components[0].connections.push(super::Connection {
            pin: "3".to_owned(),
            state: ConnectionState::Connected("VIN".to_owned()),
        });
        assert_rejected(design, "CC-PIN-003");
    }

    #[test]
    fn validates_schematic_connection_value_and_model_contracts_for_public_ir_consumers() {
        let mut design = voltage_divider();
        design.components[0].schematic_placement.rotation_degrees = 45;
        assert_rejected(design, "CC-SCHEMATIC-001");

        let mut design = voltage_divider();
        design.components[0].schematic_placement.position.x = MAX_ABS_COORDINATE_NM + 1;
        assert_rejected(design, "CC-SCHEMATIC-002");

        let mut design = voltage_divider();
        design.components[1].schematic_placement.position =
            design.components[0].schematic_placement.position;
        let diagnostics = design
            .validate()
            .expect_err("duplicate schematic anchors must be rejected");
        let collision = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-SCHEMATIC-003")
            .expect("schematic anchor collision diagnostic must exist");
        assert_eq!(collision.path, "divider.r_top");
        assert_eq!(collision.related_path.as_deref(), Some("divider.r_bottom"));

        let mut design = voltage_divider();
        design.components[0].connections.pop();
        assert_rejected(design, "CC-PIN-005");

        let mut design = voltage_divider();
        design.components[0].value = ComponentValue::Resistance(crate::quantity::Quantity::new(
            -1,
            0,
            crate::quantity::Unit::Ohm,
        ));
        assert_rejected(design, "CC-VALUE-004");

        let mut design = voltage_divider();
        let voltage_source = design
            .components
            .iter_mut()
            .find(|component| component.reference == "V1")
            .expect("reference voltage source exists");
        voltage_source.value = ComponentValue::DcVoltage(crate::quantity::Quantity::new(
            10,
            0,
            crate::quantity::Unit::Ohm,
        ));
        assert_rejected(design, "CC-VALUE-005");

        let mut design = voltage_divider();
        match design.components[0]
            .simulation
            .as_mut()
            .expect("reference resistor is simulated")
        {
            super::SimulationModel::Resistor { model_id, .. } => {
                *model_id = "spice:unknown".to_owned();
            }
            super::SimulationModel::DcVoltageSource { .. } => unreachable!(),
        }
        assert_rejected(design, "CC-SIM-010");

        let mut design = voltage_divider();
        design.components[0].reference = "V_load".to_owned();
        design
            .components
            .iter_mut()
            .find(|component| {
                matches!(
                    component.simulation.as_ref(),
                    Some(super::SimulationModel::DcVoltageSource { .. })
                )
            })
            .expect("reference voltage source is simulated")
            .reference = "R_supply".to_owned();
        assert_eq!(
            design.validate(),
            Ok(()),
            "canonical references must not inherit backend R/V prefix requirements"
        );
    }

    #[test]
    fn transform_handles_minimum_coordinates_without_panicking() {
        let design = voltage_divider();
        let mut placement = design.components[0]
            .physical
            .as_ref()
            .expect("reference resistor is physical")
            .placement;
        for (rotation, offset) in [
            (90, PointNm::new(i64::MIN, 0)),
            (180, PointNm::new(i64::MIN, 0)),
            (270, PointNm::new(0, i64::MIN)),
        ] {
            placement.rotation_degrees = rotation;
            assert_eq!(placement.transform(offset), None);
        }
    }

    #[test]
    fn transform_applies_each_supported_orthogonal_rotation() {
        let offset = PointNm::new(1_000_000, 2_000_000);
        for (rotation_degrees, expected) in [
            (0, PointNm::new(11_000_000, 22_000_000)),
            (90, PointNm::new(12_000_000, 19_000_000)),
            (180, PointNm::new(9_000_000, 18_000_000)),
            (270, PointNm::new(8_000_000, 21_000_000)),
        ] {
            let placement = Placement {
                position: PointNm::new(10_000_000, 20_000_000),
                rotation_degrees,
                layer: CopperLayer::Front,
            };
            assert_eq!(placement.transform(offset), Some(expected));
        }
    }

    #[test]
    fn rejects_outline_far_corner_beyond_envelope() {
        let mut design = voltage_divider();
        design.board.outline.origin.x = MAX_ABS_COORDINATE_NM;
        design.board.outline.size.width = MAX_ABS_COORDINATE_NM;
        let diagnostics = design
            .validate()
            .expect_err("far corner outside envelope must be rejected");
        assert!(has_code(&diagnostics, "CC-BOARD-004"));
    }

    #[test]
    fn rejects_coordinate_bearing_values_outside_envelope() {
        let mut design = voltage_divider();
        let physical = design.components[0]
            .physical
            .as_mut()
            .expect("reference resistor is physical");
        physical.placement.position.x = i64::MIN;
        physical.footprint.pads[0].offset.y = i64::MIN;
        physical.footprint.pads[0].size.width = i64::MAX;
        design.board.routes[0].width_nm = i64::MAX;
        design.board.routes[0].start.x = i64::MIN;

        let diagnostics = design
            .validate()
            .expect_err("out-of-envelope values must be rejected");
        for code in [
            "CC-PHYS-004",
            "CC-PAD-008",
            "CC-PAD-009",
            "CC-ROUTE-008",
            "CC-ROUTE-009",
        ] {
            assert!(has_code(&diagnostics, code), "missing diagnostic {code}");
        }
    }

    #[test]
    fn accepts_explicit_logical_pin_to_physical_pad_mapping() {
        let mut design = voltage_divider();
        let component = &mut design.components[0];
        component.connections[0].pin = "POSITIVE".to_owned();
        component.connections[1].pin = "NEGATIVE".to_owned();
        component.symbol.pins[0].pin = "POSITIVE".to_owned();
        component.symbol.pins[1].pin = "NEGATIVE".to_owned();
        let physical = component
            .physical
            .as_mut()
            .expect("reference resistor is physical");
        physical.pin_pad_bindings = vec![
            PinPadBinding {
                pin: "POSITIVE".to_owned(),
                pad: "1".to_owned(),
            },
            PinPadBinding {
                pin: "NEGATIVE".to_owned(),
                pad: "2".to_owned(),
            },
        ];
        let simulation = component
            .simulation
            .as_mut()
            .expect("reference resistor is simulated");
        match simulation {
            super::SimulationModel::Resistor {
                positive_pin,
                negative_pin,
                ..
            } => {
                *positive_pin = "POSITIVE".to_owned();
                *negative_pin = "NEGATIVE".to_owned();
            }
            super::SimulationModel::DcVoltageSource { .. } => unreachable!(),
        }
        assert_eq!(component.net_for_pad("1"), Some("VIN"));
        assert_eq!(component.net_for_pad("2"), Some("VOUT"));
        assert_eq!(design.validate(), Ok(()));
    }

    #[test]
    fn rejects_missing_and_duplicate_pin_pad_bindings() {
        let mut design = voltage_divider();
        let physical = design.components[0]
            .physical
            .as_mut()
            .expect("reference resistor is physical");
        physical.pin_pad_bindings.pop();
        physical.pin_pad_bindings.push(PinPadBinding {
            pin: "1".to_owned(),
            pad: "1".to_owned(),
        });
        let diagnostics = design
            .validate()
            .expect_err("bad pin-to-pad bindings must be rejected");
        assert!(has_code(&diagnostics, "CC-BIND-003"));
        assert!(has_code(&diagnostics, "CC-BIND-004"));
        assert!(has_code(&diagnostics, "CC-PAD-004"));
    }

    #[test]
    fn rejects_multiple_ground_nets_without_simulation() {
        let mut design = voltage_divider();
        design
            .components
            .retain(|component| component.physical.is_some());
        for component in &mut design.components {
            component.simulation = None;
        }
        design.nets[0].is_ground = true;
        let diagnostics = design
            .validate()
            .expect_err("all designs must have at most one ground");
        assert!(has_code(&diagnostics, "CC-NET-003"));
    }

    #[test]
    fn rejects_same_simulation_terminal_pin() {
        let mut design = voltage_divider();
        match design.components[0]
            .simulation
            .as_mut()
            .expect("reference resistor is simulated")
        {
            super::SimulationModel::Resistor { negative_pin, .. } => {
                *negative_pin = "1".to_owned();
            }
            super::SimulationModel::DcVoltageSource { .. } => unreachable!(),
        }
        let diagnostics = design
            .validate()
            .expect_err("simulation terminals must be distinct");
        assert!(has_code(&diagnostics, "CC-SIM-008"));
    }

    #[test]
    fn rejects_simulator_model_that_disagrees_with_logical_function() {
        let mut design = voltage_divider();
        design.components[0].part.logical_function = "dc_voltage_source".to_owned();
        let diagnostics = design
            .validate()
            .expect_err("logical device and simulation primitive must agree");
        assert!(has_code(&diagnostics, "CC-SIM-011"));
    }

    #[test]
    fn rejects_reversed_duplicate_route_geometry() {
        let mut design = voltage_divider();
        let mut reverse = design.board.routes[0].clone();
        reverse.path = "board.routes.vout_bridge_reverse".to_owned();
        std::mem::swap(&mut reverse.start, &mut reverse.end);
        design.board.routes.push(reverse);
        let diagnostics = design
            .validate()
            .expect_err("reversed duplicate copper must be rejected");
        assert!(has_code(&diagnostics, "CC-ROUTE-005"));
    }

    #[test]
    fn rejects_duplicate_route_semantic_identity() {
        let mut design = voltage_divider();
        let mut second = design.board.routes[0].clone();
        second.start.y += 1;
        design.board.routes.push(second);
        let diagnostics = design
            .validate()
            .expect_err("duplicate route paths must be rejected");
        assert!(has_code(&diagnostics, "CC-ROUTE-007"));
    }

    #[test]
    fn accepts_explicit_two_terminal_planar_routing_intent() {
        let mut design = voltage_divider();
        design.board.routing_requests = vec![vout_routing_request()];
        assert_eq!(design.validate(), Ok(()));
        assert_eq!(
            design.board.routes.len(),
            1,
            "authored copper remains distinct"
        );
    }

    #[test]
    fn rejects_invalid_or_ambiguous_routing_request_identity() {
        let mut design = voltage_divider();
        let mut invalid = vout_routing_request();
        invalid.path = ".invalid".to_owned();
        design.board.routing_requests.push(invalid);
        assert_rejected(design, "CC-AUTOROUTE-002");

        let mut design = voltage_divider();
        let mut colliding = vout_routing_request();
        colliding.path = design.board.routes[0].path.clone();
        design.board.routing_requests.push(colliding);
        assert_rejected(design, "CC-AUTOROUTE-004");

        let mut design = voltage_divider();
        let first = vout_routing_request();
        let mut second = first.clone();
        second.path = "board.autoroute.second".to_owned();
        design.board.routing_requests = vec![first, second];
        assert_rejected(design, "CC-AUTOROUTE-001");
    }

    #[test]
    fn rejects_invalid_routing_request_dimensions_and_net() {
        let mut design = voltage_divider();
        let mut request = vout_routing_request();
        request.net = "UNKNOWN".to_owned();
        request.width_nm = 0;
        request.clearance_nm = -1;
        request.grid_step_nm = i64::MAX;
        design.board.routing_requests.push(request);
        let diagnostics = design
            .validate()
            .expect_err("invalid routing dimensions and net must fail");
        for code in [
            "CC-AUTOROUTE-005",
            "CC-AUTOROUTE-006",
            "CC-AUTOROUTE-007",
            "CC-AUTOROUTE-011",
        ] {
            assert!(has_code(&diagnostics, code), "missing diagnostic {code}");
        }
    }

    #[test]
    fn rejects_unsupported_terminal_count_layer_and_grid() {
        let mut design = voltage_divider();
        let mut request = vout_routing_request();
        request.net = "VIN".to_owned();
        design.board.routing_requests.push(request);
        assert_rejected(design, "CC-AUTOROUTE-012");

        let mut design = voltage_divider();
        let mut request = vout_routing_request();
        request.layer = CopperLayer::Back;
        design.board.routing_requests.push(request);
        assert_rejected(design, "CC-AUTOROUTE-013");

        let mut design = voltage_divider();
        let mut request = vout_routing_request();
        request.grid_step_nm = 300_000;
        design.board.routing_requests.push(request);
        assert_rejected(design, "CC-AUTOROUTE-014");

        let mut design = voltage_divider();
        design.board.outline.origin.x = i64::MIN;
        design.board.routing_requests.push(vout_routing_request());
        assert_rejected(design, "CC-AUTOROUTE-014");
    }

    #[test]
    fn canonicalization_normalizes_full_turn_placements() {
        let expected = voltage_divider();
        let mut rotated = expected.clone();
        for component in &mut rotated.components {
            if let Some(physical) = &mut component.physical {
                physical.placement.rotation_degrees = 360;
            }
        }
        rotated.canonicalize();
        assert_eq!(rotated, expected);
    }

    #[test]
    fn canonicalization_orders_modules_and_normalizes_schematic_rotation() {
        let expected = voltage_divider();
        let mut permuted = expected.clone();
        permuted.modules.reverse();
        for module in &mut permuted.modules {
            module.ports.reverse();
        }
        for component in &mut permuted.components {
            component.schematic_placement.rotation_degrees += 360;
        }

        permuted.canonicalize();

        assert_eq!(permuted, expected);
    }

    #[test]
    fn accepts_complete_canonical_product_intent() {
        assert_eq!(product_design().validate(), Ok(()));
    }

    #[test]
    fn manufacturer_identified_part_remains_physical_without_board_placement() {
        let mut design = product_design();
        design
            .components
            .iter_mut()
            .find(|component| component.reference == "R1")
            .expect("manufacturer-identified fixture component exists")
            .physical = None;

        assert_eq!(design.validate(), Ok(()));
    }

    #[test]
    fn product_part_identity_fails_closed_for_missing_or_virtual_physical_fields() {
        let mut logical_function = product_design();
        physical_part_mut(&mut logical_function, "R1").logical_function = "bad function".to_owned();
        assert_rejected(logical_function, "CC-PRODUCT-PART-001");

        let mut manufacturer = product_design();
        physical_part_mut(&mut manufacturer, "R1").manufacturer = None;
        let diagnostics = rejected_diagnostics(manufacturer);
        assert!(has_code(&diagnostics, "CC-PRODUCT-PART-003"));
        assert!(has_code(&diagnostics, "CC-PRODUCT-VARIANT-008"));
        assert!(!has_code(&diagnostics, "CC-PRODUCT-PART-002"));

        let mut package = product_design();
        physical_part_mut(&mut package, "R1").package = None;
        assert_rejected(package, "CC-PRODUCT-PART-002");

        let mut lifecycle = product_design();
        physical_part_mut(&mut lifecycle, "R1").lifecycle = None;
        assert_rejected(lifecycle, "CC-PRODUCT-PART-002");

        let mut sourcing = product_design();
        physical_part_mut(&mut sourcing, "R1").sourcing = None;
        assert_rejected(sourcing, "CC-PRODUCT-PART-002");

        let mut virtual_fields = product_design();
        let virtual_part = &mut virtual_fields
            .components
            .iter_mut()
            .find(|component| component.reference == "V1")
            .expect("virtual product fixture component must exist")
            .part;
        virtual_part.package = Some("virtual-package".to_owned());
        virtual_part.approved_substitutions = vec![approved_substitution()];
        assert_rejected(virtual_fields, "CC-PRODUCT-PART-003");
    }

    #[test]
    fn product_part_text_sourcing_and_substitution_guards_are_discriminating() {
        let mut text = product_design();
        physical_part_mut(&mut text, "R1").manufacturer = Some(" Yageo".to_owned());
        assert_rejected(text, "CC-PRODUCT-PART-004");

        let mut sourcing_quantity = product_design();
        let sourcing = physical_part_mut(&mut sourcing_quantity, "R1")
            .sourcing
            .as_mut()
            .expect("physical product fixture has sourcing constraints");
        sourcing.minimum_available_quantity = 0;
        sourcing.maximum_lead_time_days = 0;
        assert_rejected(sourcing_quantity, "CC-PRODUCT-PART-005");

        let mut region = product_design();
        physical_part_mut(&mut region, "R1")
            .sourcing
            .as_mut()
            .expect("physical product fixture has sourcing constraints")
            .required_region = "North America".to_owned();
        assert_rejected(region, "CC-PRODUCT-PART-006");

        let mut malformed_substitution = product_design();
        physical_part_mut(&mut malformed_substitution, "R1").approved_substitutions[0].package =
            " 0603".to_owned();
        assert_rejected(malformed_substitution, "CC-PRODUCT-PART-007");

        let mut duplicate_substitution = product_design();
        let duplicate =
            physical_part_mut(&mut duplicate_substitution, "R1").approved_substitutions[0].clone();
        physical_part_mut(&mut duplicate_substitution, "R1")
            .approved_substitutions
            .push(duplicate);
        assert_rejected(duplicate_substitution, "CC-PRODUCT-PART-008");
    }

    #[test]
    fn substitutions_reject_package_drift_and_the_exact_base_identity() {
        let mut package_drift = product_design();
        let mut incompatible = approved_substitution();
        incompatible.package = "0805".to_owned();
        physical_part_mut(&mut package_drift, "R1").approved_substitutions =
            vec![incompatible.clone()];
        variant_mut(&mut package_drift)
            .components
            .iter_mut()
            .find(|component| component.component_path == "divider.r_top")
            .expect("R1 variant assignment exists")
            .state = PopulationState::Alternate(incompatible);
        assert_rejected(package_drift, "CC-PRODUCT-PART-010");

        let mut self_substitution = product_design();
        let part = physical_part_mut(&mut self_substitution, "R1");
        let repeated_base = ApprovedSubstitution {
            manufacturer: part
                .manufacturer
                .clone()
                .expect("physical fixture manufacturer exists"),
            manufacturer_part_number: part
                .manufacturer_part_number
                .clone()
                .expect("physical fixture part number exists"),
            package: part
                .package
                .clone()
                .expect("physical fixture package exists"),
        };
        part.approved_substitutions = vec![repeated_base.clone()];
        variant_mut(&mut self_substitution)
            .components
            .iter_mut()
            .find(|component| component.component_path == "divider.r_top")
            .expect("R1 variant assignment exists")
            .state = PopulationState::Alternate(repeated_base);
        assert_rejected(self_substitution, "CC-PRODUCT-PART-011");
    }

    #[test]
    fn approved_substitution_count_accepts_the_boundary_and_skips_one_over() {
        let substitutions = (0..MAX_PRODUCT_SUBSTITUTIONS)
            .map(|index| ApprovedSubstitution {
                manufacturer: format!("Manufacturer{index}"),
                manufacturer_part_number: format!("Part{index}"),
                package: "0603".to_owned(),
            })
            .collect::<Vec<_>>();
        let mut boundary = product_design();
        physical_part_mut(&mut boundary, "R1").approved_substitutions = substitutions;
        assert_eq!(boundary.validate(), Ok(()));

        let mut one_over = product_design();
        physical_part_mut(&mut one_over, "R1").approved_substitutions =
            vec![approved_substitution(); MAX_PRODUCT_SUBSTITUTIONS + 1];
        let diagnostics = rejected_diagnostics(one_over);
        assert!(has_code(&diagnostics, "CC-PRODUCT-PART-009"));
        assert!(!has_code(&diagnostics, "CC-PRODUCT-PART-008"));
    }

    #[test]
    fn catalog_evidence_requires_canonical_identity_digest_and_real_date() {
        let mut missing = product_design();
        missing.product.catalog = None;
        assert_rejected(missing, "CC-PRODUCT-CATALOG-001");

        let mut snapshot = product_design();
        snapshot
            .product
            .catalog
            .as_mut()
            .expect("product catalog fixture exists")
            .snapshot_id = "bad snapshot".to_owned();
        assert_rejected(snapshot, "CC-PRODUCT-CATALOG-002");

        let mut digest = product_design();
        digest
            .product
            .catalog
            .as_mut()
            .expect("product catalog fixture exists")
            .sha256 = "A".repeat(64);
        assert_rejected(digest, "CC-PRODUCT-CATALOG-003");

        for invalid in ["2026-02-29", "0000-01-01", "2026-13-01", "2026-04-31"] {
            let mut date = product_design();
            date.product
                .catalog
                .as_mut()
                .expect("product catalog fixture exists")
                .evaluated_on = invalid.to_owned();
            assert_rejected(date, "CC-PRODUCT-CATALOG-004");
        }

        let mut leap_day = product_design();
        leap_day
            .product
            .catalog
            .as_mut()
            .expect("product catalog fixture exists")
            .evaluated_on = "2024-02-29".to_owned();
        assert_eq!(leap_day.validate(), Ok(()));
    }

    #[test]
    fn variants_require_positive_unique_paths_and_total_physical_assignment() {
        let mut missing_variant = product_design();
        missing_variant.product.variants.clear();
        assert_rejected(missing_variant, "CC-PRODUCT-VARIANT-001");

        let mut invalid_path = product_design();
        variant_mut(&mut invalid_path).path = "bad path".to_owned();
        assert_rejected(invalid_path, "CC-PRODUCT-VARIANT-002");

        let mut duplicate_path = product_design();
        let duplicate = variant_mut(&mut duplicate_path).clone();
        duplicate_path.product.variants.push(duplicate);
        assert_rejected(duplicate_path, "CC-PRODUCT-VARIANT-003");

        let mut zero_quantity = product_design();
        variant_mut(&mut zero_quantity).build_quantity = 0;
        assert_rejected(zero_quantity, "CC-PRODUCT-VARIANT-004");

        let mut invalid_component_path = product_design();
        variant_mut(&mut invalid_component_path).components[0].component_path =
            "bad path".to_owned();
        assert_rejected(invalid_component_path, "CC-PRODUCT-VARIANT-005");

        let mut duplicate_assignment = product_design();
        let duplicate = variant_mut(&mut duplicate_assignment).components[0].clone();
        variant_mut(&mut duplicate_assignment)
            .components
            .push(duplicate);
        assert_rejected(duplicate_assignment, "CC-PRODUCT-VARIANT-006");

        let mut unknown = product_design();
        variant_mut(&mut unknown).components.push(VariantComponent {
            component_path: "divider.unknown".to_owned(),
            state: PopulationState::NotFitted,
        });
        assert_rejected(unknown, "CC-PRODUCT-VARIANT-007");

        let mut virtual_component = product_design();
        variant_mut(&mut virtual_component)
            .components
            .push(VariantComponent {
                component_path: "divider.analysis.input".to_owned(),
                state: PopulationState::Fitted,
            });
        assert_rejected(virtual_component, "CC-PRODUCT-VARIANT-008");

        let mut missing_component = product_design();
        variant_mut(&mut missing_component).components.pop();
        assert_rejected(missing_component, "CC-PRODUCT-VARIANT-009");
    }

    #[test]
    fn product_variant_count_accepts_the_boundary_and_skips_one_over() {
        let template = product_design().product.variants[0].clone();
        let variants = (0..MAX_PRODUCT_VARIANTS)
            .map(|index| {
                let mut variant = template.clone();
                variant.path = format!("product.variants.boundary.{index}");
                variant
            })
            .collect::<Vec<_>>();
        let mut boundary = product_design();
        boundary.product.variants = variants;
        assert_eq!(boundary.validate(), Ok(()));

        let mut one_over = product_design();
        one_over.product.variants = vec![template; MAX_PRODUCT_VARIANTS + 1];
        let diagnostics = rejected_diagnostics(one_over);
        assert!(has_code(&diagnostics, "CC-PRODUCT-VARIANT-011"));
        assert!(!has_code(&diagnostics, "CC-PRODUCT-VARIANT-003"));
    }

    #[test]
    fn aggregate_variant_work_accepts_the_boundary_and_skips_one_over() {
        let repeated_assignment = VariantComponent {
            component_path: "divider.r_bottom".to_owned(),
            state: PopulationState::Fitted,
        };
        let mut boundary = product_design();
        boundary
            .components
            .iter_mut()
            .find(|component| component.reference == "R1")
            .expect("manufacturer-identified fixture component exists")
            .physical = None;
        variant_mut(&mut boundary).components =
            vec![repeated_assignment.clone(); MAX_PRODUCT_VARIANT_WORK - 2];
        let diagnostics = rejected_diagnostics(boundary);
        assert!(!has_code(&diagnostics, "CC-PRODUCT-VARIANT-012"));
        assert!(has_code(&diagnostics, "CC-PRODUCT-VARIANT-006"));

        let mut one_over = product_design();
        one_over
            .components
            .iter_mut()
            .find(|component| component.reference == "R1")
            .expect("manufacturer-identified fixture component exists")
            .physical = None;
        variant_mut(&mut one_over).components =
            vec![repeated_assignment; MAX_PRODUCT_VARIANT_WORK - 1];
        let diagnostics = rejected_diagnostics(one_over);
        assert!(has_code(&diagnostics, "CC-PRODUCT-VARIANT-012"));
        assert!(!has_code(&diagnostics, "CC-PRODUCT-VARIANT-006"));
    }

    #[test]
    fn alternate_population_requires_an_exact_approved_substitution() {
        let mut approved = product_design();
        variant_mut(&mut approved)
            .components
            .iter_mut()
            .find(|component| component.component_path == "divider.r_top")
            .expect("R1 variant assignment exists")
            .state = PopulationState::Alternate(approved_substitution());
        assert_eq!(approved.validate(), Ok(()));

        let mut unapproved = product_design();
        let mut substitution = approved_substitution();
        substitution
            .manufacturer_part_number
            .push_str("-unapproved");
        variant_mut(&mut unapproved)
            .components
            .iter_mut()
            .find(|component| component.component_path == "divider.r_top")
            .expect("R1 variant assignment exists")
            .state = PopulationState::Alternate(substitution);
        assert_rejected(unapproved, "CC-PRODUCT-VARIANT-010");
    }

    #[test]
    fn configurations_require_canonical_unique_keys_and_text_values() {
        let mut key = product_design();
        variant_mut(&mut key).configurations[0].key = "bad key".to_owned();
        assert_rejected(key, "CC-PRODUCT-CONFIG-001");

        let mut value = product_design();
        variant_mut(&mut value).configurations[0].value = "".to_owned();
        assert_rejected(value, "CC-PRODUCT-CONFIG-002");

        let mut duplicate = product_design();
        let duplicate_configuration = variant_mut(&mut duplicate).configurations[0].clone();
        variant_mut(&mut duplicate)
            .configurations
            .push(duplicate_configuration);
        assert_rejected(duplicate, "CC-PRODUCT-CONFIG-003");
    }

    #[test]
    fn product_configuration_count_accepts_the_boundary_and_skips_one_over() {
        let configurations = (0..MAX_PRODUCT_CONFIGURATIONS)
            .map(|index| ProductConfiguration {
                key: format!("key{index}"),
                value: format!("value-{index}"),
            })
            .collect::<Vec<_>>();
        let mut boundary = product_design();
        variant_mut(&mut boundary).configurations = configurations;
        assert_eq!(boundary.validate(), Ok(()));

        let mut one_over = product_design();
        let variant = variant_mut(&mut one_over);
        variant.configurations = vec![
            ProductConfiguration {
                key: "bad key".to_owned(),
                value: "".to_owned(),
            };
            MAX_PRODUCT_CONFIGURATIONS + 1
        ];
        variant.components = vec![
            VariantComponent {
                component_path: "divider.unknown".to_owned(),
                state: PopulationState::Fitted,
            },
            VariantComponent {
                component_path: "divider.unknown".to_owned(),
                state: PopulationState::Fitted,
            },
        ];
        let diagnostics = rejected_diagnostics(one_over);
        assert!(has_code(&diagnostics, "CC-PRODUCT-CONFIG-004"));
        for suppressed in [
            "CC-PRODUCT-VARIANT-006",
            "CC-PRODUCT-VARIANT-007",
            "CC-PRODUCT-VARIANT-009",
            "CC-PRODUCT-CONFIG-001",
            "CC-PRODUCT-CONFIG-002",
            "CC-PRODUCT-CONFIG-003",
        ] {
            assert!(
                !has_code(&diagnostics, suppressed),
                "resource preflight must suppress expanded diagnostic {suppressed}: {diagnostics:#?}"
            );
        }
    }

    #[test]
    fn product_configuration_utf8_byte_lengths_are_bounded() {
        let mut boundary = product_design();
        variant_mut(&mut boundary).configurations = vec![ProductConfiguration {
            key: "k".repeat(MAX_PRODUCT_CONFIGURATION_KEY_BYTES),
            value: "é".repeat(MAX_PRODUCT_CONFIGURATION_VALUE_BYTES / 2),
        }];
        assert_eq!(boundary.validate(), Ok(()));

        let mut key_one_over = product_design();
        let variant = variant_mut(&mut key_one_over);
        variant.configurations[0].key = "k".repeat(MAX_PRODUCT_CONFIGURATION_KEY_BYTES + 1);
        variant.configurations.push(ProductConfiguration {
            key: "bad key".to_owned(),
            value: "".to_owned(),
        });
        variant.components.clear();
        let diagnostics = rejected_diagnostics(key_one_over);
        assert!(has_code(&diagnostics, "CC-PRODUCT-CONFIG-005"));
        for suppressed in [
            "CC-PRODUCT-VARIANT-009",
            "CC-PRODUCT-CONFIG-001",
            "CC-PRODUCT-CONFIG-002",
        ] {
            assert!(
                !has_code(&diagnostics, suppressed),
                "byte preflight must suppress expanded diagnostic {suppressed}: {diagnostics:#?}"
            );
        }

        let mut value_one_over = product_design();
        variant_mut(&mut value_one_over).configurations[0].value =
            format!("{}x", "é".repeat(MAX_PRODUCT_CONFIGURATION_VALUE_BYTES / 2));
        assert_rejected(value_one_over, "CC-PRODUCT-CONFIG-006");
    }

    #[test]
    fn manufacturability_analysis_and_assertion_identity_is_fail_closed() {
        let mut invalid_path = product_design();
        manufacturability_analysis_mut(&mut invalid_path).path = "bad path".to_owned();
        assert_rejected(invalid_path, "CC-PRODUCT-ANALYSIS-001");

        let mut duplicate_analysis = product_design();
        let duplicate = manufacturability_analysis_mut(&mut duplicate_analysis).clone();
        duplicate_analysis
            .product
            .manufacturability_analyses
            .push(duplicate);
        assert_rejected(duplicate_analysis, "CC-PRODUCT-ANALYSIS-002");

        for (adapter, version) in [("other", "10"), ("kicad", "11")] {
            let mut unsupported = product_design();
            let analysis = manufacturability_analysis_mut(&mut unsupported);
            analysis.adapter = adapter.to_owned();
            analysis.version = version.to_owned();
            assert_rejected(unsupported, "CC-PRODUCT-ANALYSIS-003");
        }

        let mut invalid_assertion = product_design();
        manufacturability_analysis_mut(&mut invalid_assertion).assertions[0].path =
            "bad path".to_owned();
        assert_rejected(invalid_assertion, "CC-PRODUCT-ASSERTION-001");

        let mut duplicate_assertion = product_design();
        let duplicate_path = manufacturability_analysis_mut(&mut duplicate_assertion).assertions[0]
            .path
            .clone();
        manufacturability_analysis_mut(&mut duplicate_assertion).assertions[1].path =
            duplicate_path;
        assert_rejected(duplicate_assertion, "CC-PRODUCT-ASSERTION-002");

        let mut duplicate_capability = product_design();
        let duplicate_capability_value =
            manufacturability_analysis_mut(&mut duplicate_capability).assertions[0].capability;
        manufacturability_analysis_mut(&mut duplicate_capability).assertions[1].capability =
            duplicate_capability_value;
        assert_rejected(duplicate_capability, "CC-PRODUCT-ASSERTION-003");
    }

    #[test]
    fn manufacturability_analysis_count_accepts_the_boundary_and_skips_one_over() {
        let template = product_design().product.manufacturability_analyses[0].clone();
        let analyses = (0..MAX_MANUFACTURABILITY_ANALYSES)
            .map(|analysis_index| {
                let mut analysis = template.clone();
                analysis.path = format!("product.manufacturability.boundary.{analysis_index}");
                for (assertion_index, assertion) in analysis.assertions.iter_mut().enumerate() {
                    assertion.path =
                        format!("product.assertions.boundary.{analysis_index}.{assertion_index}");
                }
                analysis
            })
            .collect::<Vec<_>>();
        let mut boundary = product_design();
        boundary.product.manufacturability_analyses = analyses;
        assert_eq!(boundary.validate(), Ok(()));

        let mut one_over = product_design();
        one_over.product.manufacturability_analyses =
            vec![template; MAX_MANUFACTURABILITY_ANALYSES + 1];
        let diagnostics = rejected_diagnostics(one_over);
        assert!(has_code(&diagnostics, "CC-PRODUCT-ANALYSIS-004"));
        assert!(!has_code(&diagnostics, "CC-PRODUCT-ANALYSIS-002"));
    }

    #[test]
    fn aggregate_manufacturability_assertions_accept_the_boundary_and_skip_one_over() {
        let assertions = (0..MAX_MANUFACTURABILITY_ASSERTIONS)
            .map(|index| ManufacturabilityAssertion {
                path: format!("product.assertions.boundary.{index}"),
                capability: match index % 5 {
                    0 => ManufacturabilityCapability::ErcClean,
                    1 => ManufacturabilityCapability::DrcClean,
                    2 => ManufacturabilityCapability::UnconnectedClean,
                    3 => ManufacturabilityCapability::SchematicParityClean,
                    _ => ManufacturabilityCapability::FabricationInventoryComplete,
                },
            })
            .collect::<Vec<_>>();
        let mut boundary = product_design();
        manufacturability_analysis_mut(&mut boundary).assertions = assertions;
        let diagnostics = rejected_diagnostics(boundary.clone());
        assert!(!has_code(&diagnostics, "CC-PRODUCT-ASSERTION-004"));
        assert!(has_code(&diagnostics, "CC-PRODUCT-ASSERTION-003"));

        manufacturability_analysis_mut(&mut boundary)
            .assertions
            .push(ManufacturabilityAssertion {
                path: "product.assertions.one_over".to_owned(),
                capability: ManufacturabilityCapability::ErcClean,
            });
        let diagnostics = rejected_diagnostics(boundary);
        assert!(has_code(&diagnostics, "CC-PRODUCT-ASSERTION-004"));
        assert!(!has_code(&diagnostics, "CC-PRODUCT-ASSERTION-003"));
    }

    #[test]
    fn canonicalizes_every_product_collection_by_stable_semantic_keys() {
        let mut expected = product_design();
        let second_substitution = ApprovedSubstitution {
            manufacturer: "Vishay".to_owned(),
            manufacturer_part_number: "CRCW060310K0FKEA".to_owned(),
            package: "0603".to_owned(),
        };
        for component in &mut expected.components {
            if component.physical.is_some() {
                component
                    .part
                    .approved_substitutions
                    .push(second_substitution.clone());
            }
        }
        let mut second_variant = expected.product.variants[0].clone();
        second_variant.path = "product.variants.secondary".to_owned();
        second_variant.build_quantity = 25;
        expected.product.variants.push(second_variant);

        let mut second_analysis = expected.product.manufacturability_analyses[0].clone();
        second_analysis.path = "product.manufacturability.kicad10_secondary".to_owned();
        for assertion in &mut second_analysis.assertions {
            assertion.path =
                assertion
                    .path
                    .replacen("product.assertions.", "product.assertions.secondary.", 1);
        }
        expected
            .product
            .manufacturability_analyses
            .push(second_analysis);
        expected.canonicalize();
        assert_eq!(expected.validate(), Ok(()));

        let mut permuted = expected.clone();
        for component in &mut permuted.components {
            component.part.approved_substitutions.reverse();
        }
        permuted.product.variants.reverse();
        for variant in &mut permuted.product.variants {
            variant.components.reverse();
            variant.configurations.reverse();
        }
        permuted.product.manufacturability_analyses.reverse();
        for analysis in &mut permuted.product.manufacturability_analyses {
            analysis.assertions.reverse();
        }
        permuted.canonicalize();

        assert_eq!(permuted, expected);
    }
}

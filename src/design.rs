use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::quantity::{Quantity, Unit};

pub const DESIGN_SCHEMA_VERSION: u32 = 1;
pub const MAX_ABS_COORDINATE_NM: i64 = 1_000_000_000_000;

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
    pub logical_device: String,
    pub manufacturer: Option<String>,
    pub manufacturer_part_number: Option<String>,
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
pub struct Board {
    pub outline: RectNm,
    pub routes: Vec<RouteSegment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Design {
    pub schema_version: u32,
    pub name: String,
    pub nets: Vec<Net>,
    pub modules: Vec<ModuleInstance>,
    pub components: Vec<Component>,
    pub board: Board,
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
        self.board
            .routes
            .sort_by(|left, right| left.path.cmp(&right.path));
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
            .any(|component| component.simulation.is_some());
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
        let mut schematic_positions = BTreeMap::new();
        let mut schematic_components: Vec<_> = self.components.iter().collect();
        schematic_components.sort_by(|left, right| left.path.cmp(&right.path));
        for component in schematic_components {
            if let Some(first_path) = schematic_positions.insert(
                component.schematic_placement.position,
                component.path.as_str(),
            ) {
                push(
                    &mut diagnostics,
                    "CC-SCHEMATIC-003",
                    component.path.as_str(),
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
    if component.part.logical_device.trim().is_empty()
        || component.part.logical_device.chars().any(char::is_control)
    {
        push(
            diagnostics,
            "CC-PART-001",
            path,
            "logical device identity must be non-empty and contain no control characters",
        );
    }
    match (
        &component.part.manufacturer,
        &component.part.manufacturer_part_number,
    ) {
        (Some(manufacturer), Some(number))
            if !manufacturer.trim().is_empty()
                && !number.trim().is_empty()
                && !manufacturer.chars().any(char::is_control)
                && !number.chars().any(char::is_control) => {}
        (None, None) if component.physical.is_none() => {}
        (None, None) => push(
            diagnostics,
            "CC-PART-002",
            path,
            "physical component requires manufacturer and manufacturer part number",
        ),
        _ => push(
            diagnostics,
            "CC-PART-003",
            path,
            "manufacturer and manufacturer part number must be supplied together",
        ),
    }

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
            if component.part.logical_device != "resistor" {
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
            if component.part.logical_device != "dc_voltage_source" {
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
            if component.part.logical_device != "resistor" {
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
            if !component.reference.starts_with('R') {
                push(
                    diagnostics,
                    "CC-SIM-005",
                    path,
                    "SPICE resistor reference must begin with R",
                );
            }
        }
        SimulationModel::DcVoltageSource { model_id, .. } => {
            if component.part.logical_device != "dc_voltage_source" {
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
            if !component.reference.starts_with('V') {
                push(
                    diagnostics,
                    "CC-SIM-007",
                    path,
                    "SPICE voltage-source reference must begin with V",
                );
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use std::panic::catch_unwind;

    use crate::demo::voltage_divider;

    use super::{
        ComponentValue, ConnectionState, CopperLayer, MAX_ABS_COORDINATE_NM, PinPadBinding,
        Placement, PointNm,
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

    #[test]
    fn reference_design_is_valid() {
        assert_eq!(voltage_divider().validate(), Ok(()));
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
        design.components[0].part.logical_device.clear();
        assert_rejected(design, "CC-PART-001");

        let mut design = voltage_divider();
        design.components[0].part.manufacturer = None;
        design.components[0].part.manufacturer_part_number = None;
        assert_rejected(design, "CC-PART-002");

        let mut design = voltage_divider();
        design.components[0].part.manufacturer_part_number = None;
        assert_rejected(design, "CC-PART-003");

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
        assert_rejected(design, "CC-SCHEMATIC-003");

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
        design.components[0].reference = "X1".to_owned();
        assert_rejected(design, "CC-SIM-005");

        let mut design = voltage_divider();
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
            .reference = "X1".to_owned();
        assert_rejected(design, "CC-SIM-007");
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
    fn rejects_simulator_model_that_disagrees_with_logical_device() {
        let mut design = voltage_divider();
        design.components[0].part.logical_device = "dc_voltage_source".to_owned();
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
}

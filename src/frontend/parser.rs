use super::diagnostic::{SourceDiagnostic, sort_diagnostics};
use super::lexer::{Token, TokenKind, lex, token_word};
use super::syntax::{
    AutorouteSyntax, BindingSyntax, BoardItemSyntax, BoardSyntax, ComponentItemSyntax,
    ComponentKindSyntax, ComponentSyntax, ConnectionStateSyntax, DeclarationSyntax, DesignSyntax,
    FootprintItemSyntax, FootprintSyntax, ModuleSyntax, NetSyntax, PartSyntax, PlacementSyntax,
    PointSyntax, PortSyntax, QuantitySyntax, RectangleSyntax, RouteSyntax,
    SchematicPlacementSyntax, SimulationAnalysisKindSyntax, SimulationAnalysisSyntax,
    SimulationAssertionSyntax, SimulationSampleSyntax, SourceFile, Span, Spanned, SymbolPinSyntax,
    SymbolSyntax, SyntaxTree,
};

pub(crate) fn parse(source: SourceFile) -> Result<SyntaxTree, Vec<SourceDiagnostic>> {
    let (tokens, mut diagnostics) = lex(&source);
    let mut parser = Parser {
        source: &source,
        tokens,
        cursor: 0,
        diagnostics: Vec::new(),
    };
    let design = parser.parse_design();
    diagnostics.extend(parser.diagnostics);
    sort_diagnostics(&mut diagnostics);
    match (design, diagnostics.is_empty()) {
        (Some(design), true) => Ok(SyntaxTree {
            source,
            span: design.span,
            design_name: design.name.value.clone(),
            design,
        }),
        _ => Err(diagnostics),
    }
}

struct Parser<'a> {
    source: &'a SourceFile,
    tokens: Vec<Token>,
    cursor: usize,
    diagnostics: Vec<SourceDiagnostic>,
}

impl Parser<'_> {
    fn parse_design(&mut self) -> Option<DesignSyntax> {
        let start = self.current().span;
        if !self.expect_keyword("design") {
            self.recover_to_end();
            return None;
        }
        let name = self.expect_name("design name")?;
        if !self.expect_kind(TokenKind::LeftBrace, "`{` after the design name") {
            self.recover_to_end();
            return None;
        }

        let mut declarations = Vec::new();
        while !self.at_kind(&TokenKind::RightBrace) && !self.at_end() {
            let before = self.cursor;
            if let Some(declaration) = self.parse_declaration() {
                declarations.push(declaration);
            }
            if self.cursor == before {
                self.advance();
            }
        }
        let end = if self.at_kind(&TokenKind::RightBrace) {
            let span = self.current().span;
            self.advance();
            span
        } else {
            self.error_expected("`}` to close the design");
            self.current().span
        };
        if !self.at_end() {
            self.diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-PARSE-003",
                self.source,
                self.current().span,
                None,
                "unexpected tokens after the design declaration",
            ));
            self.recover_to_end();
        }
        Some(DesignSyntax {
            name,
            declarations,
            span: start.through(end),
        })
    }

    fn parse_declaration(&mut self) -> Option<DeclarationSyntax> {
        if self.at_keyword("net") {
            return self.parse_net(false).map(DeclarationSyntax::Net);
        }
        if self.at_keyword("ground") {
            return self.parse_net(true).map(DeclarationSyntax::Net);
        }
        if self.at_keyword("module") {
            return self.parse_module().map(DeclarationSyntax::Module);
        }
        if self.at_keyword("resistor") {
            return self
                .parse_component(ComponentKindSyntax::Resistor)
                .map(DeclarationSyntax::Component);
        }
        if self.at_keyword("dc_source") {
            return self
                .parse_component(ComponentKindSyntax::DcSource)
                .map(DeclarationSyntax::Component);
        }
        if self.at_keyword("analysis") {
            return self
                .parse_simulation_analysis()
                .map(DeclarationSyntax::SimulationAnalysis);
        }
        if self.at_keyword("assert") {
            return self
                .parse_simulation_assertion()
                .map(DeclarationSyntax::SimulationAssertion);
        }
        if self.at_keyword("board") {
            return self.parse_board().map(DeclarationSyntax::Board);
        }
        self.unsupported("design declaration");
        self.recover_item();
        None
    }

    fn parse_simulation_analysis(&mut self) -> Option<SimulationAnalysisSyntax> {
        let start = self.take().span;
        let kind = self.expect_name("simulation analysis kind")?;
        let path = self.expect_name("simulation analysis semantic path")?;
        let kind = match kind.value.as_str() {
            "dc_operating_point" => SimulationAnalysisKindSyntax::DcOperatingPoint,
            "ac_linear_sweep" => {
                self.require_keyword("source")?;
                let source = self.expect_name("AC source component path")?;
                self.require_keyword("points")?;
                let points = self.expect_number("AC linear sweep point count")?;
                self.require_keyword("start_frequency")?;
                let start_frequency = self.parse_quantity()?;
                self.require_keyword("stop_frequency")?;
                let stop_frequency = self.parse_quantity()?;
                self.require_keyword("magnitude")?;
                let magnitude = self.parse_quantity()?;
                self.require_keyword("phase")?;
                let phase = self.parse_quantity()?;
                SimulationAnalysisKindSyntax::AcLinearSweep {
                    source,
                    points,
                    start_frequency,
                    stop_frequency,
                    magnitude,
                    phase,
                }
            }
            "transient" => {
                self.require_keyword("step")?;
                let step = self.parse_quantity()?;
                self.require_keyword("stop")?;
                let stop = self.parse_quantity()?;
                self.require_keyword("start")?;
                let start = self.parse_quantity()?;
                self.require_keyword("uic")?;
                let uic = self.expect_name("`true` or `false` for transient `uic`")?;
                SimulationAnalysisKindSyntax::Transient {
                    step,
                    stop,
                    start,
                    uic,
                }
            }
            _ => {
                self.diagnostics.push(SourceDiagnostic::new(
                    "CC-LANG-PARSE-002",
                    self.source,
                    kind.span,
                    Some(path.value.clone()),
                    format!("unsupported simulation analysis kind `{}`", kind.value),
                ));
                self.recover_item();
                return None;
            }
        };
        let end = self.expect_semicolon()?;
        Some(SimulationAnalysisSyntax {
            path,
            kind,
            span: start.through(end),
        })
    }

    fn parse_simulation_assertion(&mut self) -> Option<SimulationAssertionSyntax> {
        let start = self.take().span;
        self.require_keyword("net_voltage")?;
        let path = self.expect_name("simulation assertion semantic path")?;
        self.require_keyword("analysis")?;
        let analysis_path = self.expect_name("referenced simulation analysis path")?;
        self.require_keyword("net")?;
        let net = self.expect_name("asserted net")?;
        self.require_keyword("sample")?;
        let sample_kind = self.expect_name("simulation assertion sample kind")?;
        let sample = match sample_kind.value.as_str() {
            "scalar" => SimulationSampleSyntax::Scalar(sample_kind.span),
            "frequency" => {
                let quantity = self.parse_quantity()?;
                let span = sample_kind.span.through(quantity.span);
                SimulationSampleSyntax::Frequency { quantity, span }
            }
            "time" => {
                let quantity = self.parse_quantity()?;
                let span = sample_kind.span.through(quantity.span);
                SimulationSampleSyntax::Time { quantity, span }
            }
            _ => {
                self.diagnostics.push(SourceDiagnostic::new(
                    "CC-LANG-PARSE-002",
                    self.source,
                    sample_kind.span,
                    Some(path.value.clone()),
                    format!(
                        "unsupported simulation assertion sample kind `{}`",
                        sample_kind.value
                    ),
                ));
                self.recover_item();
                return None;
            }
        };
        self.require_keyword("expected")?;
        let expected = self.parse_quantity()?;
        self.require_keyword("absolute_tolerance")?;
        let absolute_tolerance = self.parse_quantity()?;
        self.require_keyword("relative_tolerance")?;
        let relative_tolerance = self.parse_quantity()?;
        let end = self.expect_semicolon()?;
        Some(SimulationAssertionSyntax {
            path,
            analysis_path,
            net,
            sample,
            expected,
            absolute_tolerance,
            relative_tolerance,
            span: start.through(end),
        })
    }

    fn parse_net(&mut self, is_ground: bool) -> Option<NetSyntax> {
        let start = self.take().span;
        let name = self.expect_name("net name")?;
        let end = self.expect_semicolon()?;
        Some(NetSyntax {
            name,
            is_ground,
            span: start.through(end),
        })
    }

    fn parse_module(&mut self) -> Option<ModuleSyntax> {
        let start = self.take().span;
        let path = self.expect_name("module instance path")?;
        if !self.expect_kind(TokenKind::LeftBrace, "`{` to open the module") {
            self.recover_item();
            return None;
        }
        let mut ports = Vec::new();
        while !self.at_kind(&TokenKind::RightBrace) && !self.at_end() {
            let before = self.cursor;
            if self.at_keyword("port") {
                if let Some(port) = self.parse_port() {
                    ports.push(port);
                }
            } else {
                self.unsupported("module declaration");
                self.recover_item();
            }
            if self.cursor == before {
                self.advance();
            }
        }
        let end = self.expect_closing_brace("module")?;
        Some(ModuleSyntax {
            path,
            ports,
            span: start.through(end),
        })
    }

    fn parse_port(&mut self) -> Option<PortSyntax> {
        let start = self.take().span;
        let direction = self.expect_name("module port direction")?;
        let name = self.expect_name("module port name")?;
        let electrical_type = self.expect_name("module port electrical type")?;
        let state = if self.at_keyword("connect") {
            self.advance();
            ConnectionStateSyntax::Connected(self.expect_name("connected net")?)
        } else if self.at_keyword("no_connect") {
            let span = self.take().span;
            ConnectionStateSyntax::NoConnect(span)
        } else {
            self.error_expected("`connect NET` or `no_connect`");
            self.recover_item();
            return None;
        };
        let end = self.expect_semicolon()?;
        Some(PortSyntax {
            direction,
            name,
            electrical_type,
            state,
            span: start.through(end),
        })
    }

    fn parse_component(&mut self, kind: ComponentKindSyntax) -> Option<ComponentSyntax> {
        let start = self.take().span;
        let path = self.expect_name("component semantic path")?;
        let reference = self.expect_name("component reference")?;
        if !self.expect_kind(TokenKind::LeftBrace, "`{` to open the component") {
            self.recover_item();
            return None;
        }
        let mut items = Vec::new();
        while !self.at_kind(&TokenKind::RightBrace) && !self.at_end() {
            let before = self.cursor;
            let item = if self.at_keyword("part") {
                self.parse_part().map(ComponentItemSyntax::Part)
            } else if self.at_keyword("symbol") {
                self.parse_symbol().map(ComponentItemSyntax::Symbol)
            } else if self.at_keyword("model") {
                self.parse_model()
            } else if self.at_keyword("schematic") {
                self.parse_schematic_placement()
                    .map(ComponentItemSyntax::SchematicPlacement)
            } else if self.at_keyword("resistance") || self.at_keyword("voltage") {
                self.parse_value()
            } else if self.at_keyword("terminals") {
                self.parse_terminals()
            } else if self.at_keyword("connect") {
                self.parse_connection()
            } else if self.at_keyword("no_connect") {
                self.parse_no_connect()
            } else if self.at_keyword("footprint") {
                self.parse_footprint().map(ComponentItemSyntax::Footprint)
            } else {
                self.unsupported("component declaration");
                self.recover_item();
                None
            };
            if let Some(item) = item {
                items.push(item);
            }
            if self.cursor == before {
                self.advance();
            }
        }
        let end = self.expect_closing_brace("component")?;
        Some(ComponentSyntax {
            kind,
            path,
            reference,
            items,
            span: start.through(end),
        })
    }

    fn parse_part(&mut self) -> Option<PartSyntax> {
        let start = self.take().span;
        let logical_device = self.expect_string("quoted logical device identity")?;
        let (manufacturer, manufacturer_part_number) = if self.at_keyword("virtual") {
            self.advance();
            (None, None)
        } else {
            self.require_keyword("manufacturer")?;
            let manufacturer = self.expect_string("quoted manufacturer identity")?;
            self.require_keyword("number")?;
            let number = self.expect_string("quoted manufacturer part number")?;
            (Some(manufacturer), Some(number))
        };
        let end = self.expect_semicolon()?;
        Some(PartSyntax {
            logical_device,
            manufacturer,
            manufacturer_part_number,
            span: start.through(end),
        })
    }

    fn parse_symbol(&mut self) -> Option<SymbolSyntax> {
        let start = self.take().span;
        let library_id = self.expect_string("quoted symbol library identifier")?;
        if !self.expect_kind(TokenKind::LeftBrace, "`{` to open the symbol binding") {
            self.recover_item();
            return None;
        }
        let mut pins = Vec::new();
        while !self.at_kind(&TokenKind::RightBrace) && !self.at_end() {
            let before = self.cursor;
            if self.at_keyword("bind") {
                if let Some(pin) = self.parse_symbol_pin() {
                    pins.push(pin);
                }
            } else {
                self.unsupported("symbol binding declaration");
                self.recover_item();
            }
            if self.cursor == before {
                self.advance();
            }
        }
        let end = self.expect_closing_brace("symbol binding")?;
        Some(SymbolSyntax {
            library_id,
            pins,
            span: start.through(end),
        })
    }

    fn parse_symbol_pin(&mut self) -> Option<SymbolPinSyntax> {
        let start = self.take().span;
        let pin = self.expect_name("logical pin in symbol binding")?;
        let symbol_pin = self.expect_name("library symbol pin number")?;
        let electrical_type = self.expect_name("symbol pin electrical type")?;
        let end = self.expect_semicolon()?;
        Some(SymbolPinSyntax {
            pin,
            symbol_pin,
            electrical_type,
            span: start.through(end),
        })
    }

    fn parse_model(&mut self) -> Option<ComponentItemSyntax> {
        let start = self.take().span;
        let library_id = self.expect_string("quoted simulation model identifier")?;
        let end = self.expect_semicolon()?;
        Some(ComponentItemSyntax::Model {
            library_id,
            span: start.through(end),
        })
    }

    fn parse_schematic_placement(&mut self) -> Option<SchematicPlacementSyntax> {
        let start = self.take().span;
        self.require_keyword("at")?;
        let position = self.parse_point()?;
        self.require_keyword("rotation")?;
        let rotation = self.expect_number("schematic rotation in degrees")?;
        self.require_keyword("deg")?;
        let end = self.expect_semicolon()?;
        Some(SchematicPlacementSyntax {
            position,
            rotation,
            span: start.through(end),
        })
    }

    fn parse_value(&mut self) -> Option<ComponentItemSyntax> {
        let token = self.take();
        let keyword = token_word(&token).expect("value keyword is a word token");
        let mut quantity = self.parse_quantity()?;
        let end = self.expect_semicolon()?;
        quantity.span = token.span.through(end);
        Some(ComponentItemSyntax::Value { keyword, quantity })
    }

    fn parse_terminals(&mut self) -> Option<ComponentItemSyntax> {
        let start = self.take().span;
        let positive = self.expect_name("positive terminal pin")?;
        let negative = self.expect_name("negative terminal pin")?;
        let end = self.expect_semicolon()?;
        Some(ComponentItemSyntax::Terminals {
            positive,
            negative,
            span: start.through(end),
        })
    }

    fn parse_connection(&mut self) -> Option<ComponentItemSyntax> {
        let start = self.take().span;
        let pin = self.expect_name("logical pin")?;
        let net = self.expect_name("connected net")?;
        let end = self.expect_semicolon()?;
        Some(ComponentItemSyntax::Connection {
            pin,
            net,
            span: start.through(end),
        })
    }

    fn parse_no_connect(&mut self) -> Option<ComponentItemSyntax> {
        let start = self.take().span;
        let pin = self.expect_name("logical no-connect pin")?;
        let end = self.expect_semicolon()?;
        Some(ComponentItemSyntax::NoConnect {
            pin,
            span: start.through(end),
        })
    }

    fn parse_footprint(&mut self) -> Option<FootprintSyntax> {
        let start = self.take().span;
        let library_id = self.expect_string("quoted footprint library identifier")?;
        if !self.expect_kind(TokenKind::LeftBrace, "`{` to open the footprint") {
            self.recover_item();
            return None;
        }
        let mut items = Vec::new();
        while !self.at_kind(&TokenKind::RightBrace) && !self.at_end() {
            let before = self.cursor;
            let item = if self.at_keyword("bind") {
                self.parse_binding().map(FootprintItemSyntax::Binding)
            } else {
                self.unsupported("footprint declaration");
                self.recover_item();
                None
            };
            if let Some(item) = item {
                items.push(item);
            }
            if self.cursor == before {
                self.advance();
            }
        }
        let end = self.expect_closing_brace("footprint")?;
        Some(FootprintSyntax {
            library_id,
            items,
            span: start.through(end),
        })
    }

    fn parse_binding(&mut self) -> Option<BindingSyntax> {
        let start = self.take().span;
        let pin = self.expect_name("logical pin in binding")?;
        let pad = self.expect_name("physical pad in binding")?;
        let end = self.expect_semicolon()?;
        Some(BindingSyntax {
            pin,
            pad,
            span: start.through(end),
        })
    }

    fn parse_board(&mut self) -> Option<BoardSyntax> {
        let start = self.take().span;
        if !self.expect_kind(TokenKind::LeftBrace, "`{` to open the board") {
            self.recover_item();
            return None;
        }
        let mut items = Vec::new();
        while !self.at_kind(&TokenKind::RightBrace) && !self.at_end() {
            let before = self.cursor;
            let item = if self.at_keyword("rectangle") {
                self.parse_rectangle().map(BoardItemSyntax::Rectangle)
            } else if self.at_keyword("place") {
                self.parse_placement().map(BoardItemSyntax::Placement)
            } else if self.at_keyword("route") {
                self.parse_route()
                    .map(|route| BoardItemSyntax::Route(Box::new(route)))
            } else if self.at_keyword("autoroute") {
                self.parse_autoroute()
                    .map(|request| BoardItemSyntax::Autoroute(Box::new(request)))
            } else {
                self.unsupported("board declaration");
                self.recover_item();
                None
            };
            if let Some(item) = item {
                items.push(item);
            }
            if self.cursor == before {
                self.advance();
            }
        }
        let end = self.expect_closing_brace("board")?;
        Some(BoardSyntax {
            items,
            span: start.through(end),
        })
    }

    fn parse_rectangle(&mut self) -> Option<RectangleSyntax> {
        let start = self.take().span;
        self.require_keyword("at")?;
        let origin = self.parse_point()?;
        self.require_keyword("size")?;
        let size = self.parse_point()?;
        let end = self.expect_semicolon()?;
        Some(RectangleSyntax {
            origin,
            size,
            span: start.through(end),
        })
    }

    fn parse_placement(&mut self) -> Option<PlacementSyntax> {
        let start = self.take().span;
        let reference = self.expect_name("component reference in placement")?;
        self.require_keyword("at")?;
        let position = self.parse_point()?;
        self.require_keyword("rotation")?;
        let rotation = self.expect_number("rotation in degrees")?;
        self.require_keyword("deg")?;
        self.require_keyword("layer")?;
        let layer = self.expect_name("copper layer")?;
        let end = self.expect_semicolon()?;
        Some(PlacementSyntax {
            reference,
            position,
            rotation,
            layer,
            span: start.through(end),
        })
    }

    fn parse_route(&mut self) -> Option<RouteSyntax> {
        let start = self.take().span;
        let path = self.expect_name("route semantic path")?;
        self.require_keyword("net")?;
        let net = self.expect_name("route net")?;
        self.require_keyword("from")?;
        let route_start = self.parse_point()?;
        self.require_keyword("to")?;
        let end_point = self.parse_point()?;
        self.require_keyword("width")?;
        let width = self.parse_quantity()?;
        self.require_keyword("layer")?;
        let layer = self.expect_name("copper layer")?;
        let end = self.expect_semicolon()?;
        Some(RouteSyntax {
            path,
            net,
            start: route_start,
            end: end_point,
            width,
            layer,
            span: start.through(end),
        })
    }

    fn parse_autoroute(&mut self) -> Option<AutorouteSyntax> {
        let start = self.take().span;
        let path = self.expect_name("routing request semantic path")?;
        self.require_keyword("net")?;
        let net = self.expect_name("routing request net")?;
        self.require_keyword("width")?;
        let width = self.parse_quantity()?;
        self.require_keyword("clearance")?;
        let clearance = self.parse_quantity()?;
        self.require_keyword("grid")?;
        let grid_step = self.parse_quantity()?;
        self.require_keyword("layer")?;
        let layer = self.expect_name("copper layer")?;
        let end = self.expect_semicolon()?;
        Some(AutorouteSyntax {
            path,
            net,
            width,
            clearance,
            grid_step,
            layer,
            span: start.through(end),
        })
    }

    fn parse_point(&mut self) -> Option<PointSyntax> {
        let start = self.current().span;
        if !self.expect_kind(TokenKind::LeftParen, "`(` to start a coordinate pair") {
            return None;
        }
        let x = self.parse_quantity()?;
        if !self.expect_kind(TokenKind::Comma, "`,` between coordinates") {
            return None;
        }
        let y = self.parse_quantity()?;
        let end = self.current().span;
        if !self.expect_kind(TokenKind::RightParen, "`)` to end a coordinate pair") {
            return None;
        }
        Some(PointSyntax {
            x,
            y,
            span: start.through(end),
        })
    }

    fn parse_quantity(&mut self) -> Option<QuantitySyntax> {
        let number = self.expect_number("decimal quantity")?;
        let unit = self.expect_name("quantity unit")?;
        let span = number.span.through(unit.span);
        Some(QuantitySyntax { number, unit, span })
    }

    fn require_keyword(&mut self, keyword: &str) -> Option<()> {
        if self.expect_keyword(keyword) {
            Some(())
        } else {
            None
        }
    }

    fn expect_keyword(&mut self, keyword: &str) -> bool {
        if self.at_keyword(keyword) {
            self.advance();
            true
        } else {
            self.error_expected(&format!("keyword `{keyword}`"));
            false
        }
    }

    fn expect_name(&mut self, description: &str) -> Option<Spanned<String>> {
        let value = token_word(self.current());
        if value.is_some() {
            self.advance();
            value
        } else {
            self.error_expected(description);
            None
        }
    }

    fn expect_number(&mut self, description: &str) -> Option<Spanned<String>> {
        let TokenKind::Number(value) = &self.current().kind else {
            self.error_expected(description);
            return None;
        };
        let result = Spanned::new(value.clone(), self.current().span);
        self.advance();
        Some(result)
    }

    fn expect_string(&mut self, description: &str) -> Option<Spanned<String>> {
        let TokenKind::String(value) = &self.current().kind else {
            self.error_expected(description);
            return None;
        };
        let result = Spanned::new(value.clone(), self.current().span);
        self.advance();
        Some(result)
    }

    fn expect_semicolon(&mut self) -> Option<Span> {
        let span = self.current().span;
        if self.expect_kind(TokenKind::Semicolon, "`;` after the declaration") {
            Some(span)
        } else {
            self.recover_item();
            None
        }
    }

    fn expect_closing_brace(&mut self, description: &str) -> Option<Span> {
        if self.at_kind(&TokenKind::RightBrace) {
            let span = self.current().span;
            self.advance();
            Some(span)
        } else {
            self.error_expected(&format!("`}}` to close the {description}"));
            None
        }
    }

    fn expect_kind(&mut self, expected: TokenKind, description: &str) -> bool {
        if self.at_kind(&expected) {
            self.advance();
            true
        } else {
            self.error_expected(description);
            false
        }
    }

    fn error_expected(&mut self, description: &str) {
        self.diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-PARSE-001",
            self.source,
            self.current().span,
            None,
            format!(
                "expected {description}; found {}",
                self.current_description()
            ),
        ));
    }

    fn unsupported(&mut self, context: &str) {
        self.diagnostics.push(SourceDiagnostic::new(
            "CC-LANG-PARSE-002",
            self.source,
            self.current().span,
            None,
            format!(
                "unsupported {context} starting with {}",
                self.current_description()
            ),
        ));
    }

    fn recover_item(&mut self) {
        let mut brace_depth = 0_usize;
        while !self.at_end() {
            match self.current().kind {
                TokenKind::LeftBrace => {
                    brace_depth += 1;
                    self.advance();
                }
                TokenKind::RightBrace if brace_depth == 0 => break,
                TokenKind::RightBrace => {
                    brace_depth -= 1;
                    self.advance();
                    if brace_depth == 0 {
                        if self.at_kind(&TokenKind::Semicolon) {
                            self.advance();
                        }
                        break;
                    }
                }
                TokenKind::Semicolon if brace_depth == 0 => {
                    self.advance();
                    break;
                }
                _ => self.advance(),
            }
        }
    }

    fn recover_to_end(&mut self) {
        while !self.at_end() {
            self.advance();
        }
    }

    fn current_description(&self) -> String {
        match &self.current().kind {
            TokenKind::Word(value) | TokenKind::Number(value) => format!("`{value}`"),
            TokenKind::String(_) => "a string literal".to_owned(),
            TokenKind::LeftBrace => "`{`".to_owned(),
            TokenKind::RightBrace => "`}`".to_owned(),
            TokenKind::LeftParen => "`(`".to_owned(),
            TokenKind::RightParen => "`)`".to_owned(),
            TokenKind::Comma => "`,`".to_owned(),
            TokenKind::Semicolon => "`;`".to_owned(),
            TokenKind::End => "end of input".to_owned(),
        }
    }

    fn at_keyword(&self, keyword: &str) -> bool {
        matches!(&self.current().kind, TokenKind::Word(value) if value == keyword)
    }

    fn at_kind(&self, expected: &TokenKind) -> bool {
        std::mem::discriminant(&self.current().kind) == std::mem::discriminant(expected)
    }

    fn at_end(&self) -> bool {
        self.at_kind(&TokenKind::End)
    }

    fn current(&self) -> &Token {
        &self.tokens[self.cursor]
    }

    fn take(&mut self) -> Token {
        let token = self.current().clone();
        self.advance();
        token
    }

    fn advance(&mut self) {
        if !self.at_end() {
            self.cursor += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Parser, lex, parse};
    use crate::frontend::syntax::{DeclarationSyntax, SourceFile};

    #[test]
    fn parses_minimal_design() {
        let source = SourceFile::new(
            "minimal.circuitc",
            "design d { ground GND; board { rectangle at (0 mm, 0 mm) size (1 mm, 1 mm); } }",
        );
        let tree = parse(source).expect("minimal syntax must parse");
        assert_eq!(tree.design_name, "d");
        assert_eq!(tree.span.start, 0);
    }

    #[test]
    fn recovers_to_report_multiple_bad_declarations() {
        let source = SourceFile::new(
            "recovery.circuitc",
            "design d { widget x; gadget y; ground GND; }",
        );
        let diagnostics = parse(source).expect_err("unsupported declarations must fail");
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "CC-LANG-PARSE-002")
                .count(),
            2
        );
    }

    #[test]
    fn reports_incomplete_syntax() {
        let source = SourceFile::new("incomplete.circuitc", "design d { net VIN;");
        let diagnostics = parse(source).expect_err("incomplete design must fail");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "CC-LANG-PARSE-001"
                && diagnostic.message.contains("close the design")
        }));
    }

    #[test]
    fn braced_recovery_preserves_following_siblings() {
        let source = SourceFile::new(
            "recovery.circuitc",
            "design d { widget { inner; } gadget { nested; } ground GND; }",
        );
        let diagnostics = parse(source).expect_err("unsupported declarations must fail");
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "CC-LANG-PARSE-002")
                .count(),
            2
        );
    }

    #[test]
    fn module_recovery_reports_the_unsupported_span_and_preserves_following_ports() {
        let text = "design d { module m { widget x; port input A passive connect N; } }";
        let source = SourceFile::new("module-recovery.circuitc", text);
        let (tokens, mut diagnostics) = lex(&source);
        let mut parser = Parser {
            source: &source,
            tokens,
            cursor: 0,
            diagnostics: Vec::new(),
        };
        let design = parser
            .parse_design()
            .expect("parser recovery must preserve the enclosing design");
        diagnostics.extend(parser.diagnostics);

        let unsupported = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "CC-LANG-PARSE-002")
            .expect("unsupported module declaration must be diagnosed");
        let widget = text.find("widget").expect("fixture contains widget");
        assert_eq!((unsupported.start, unsupported.end), (widget, widget + 6));

        let module = design
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                DeclarationSyntax::Module(module) => Some(module),
                _ => None,
            })
            .expect("module syntax must survive recovery");
        assert_eq!(module.ports.len(), 1);
        assert_eq!(module.ports[0].name.value, "A");
    }

    #[test]
    fn new_declarations_report_expected_token_spans() {
        for (name, text, expected_message) in [
            (
                "missing-port-state.circuitc",
                "design d { module m { port input A passive; } }",
                "`connect NET` or `no_connect`",
            ),
            (
                "missing-part-number.circuitc",
                "design d { resistor m.r R1 { part \"resistor\" manufacturer \"Yageo\"; } }",
                "keyword `number`",
            ),
            (
                "missing-schematic-rotation.circuitc",
                "design d { resistor m.r R1 { schematic at (1 mm, 1 mm); } }",
                "keyword `rotation`",
            ),
        ] {
            let source = SourceFile::new(name, text);
            let diagnostics = parse(source).expect_err("incomplete declaration must fail");
            let diagnostic = diagnostics
                .iter()
                .find(|diagnostic| {
                    diagnostic.code == "CC-LANG-PARSE-001"
                        && diagnostic.message.contains(expected_message)
                })
                .unwrap_or_else(|| {
                    panic!(
                        "missing expected parser diagnostic {expected_message}: {diagnostics:#?}"
                    )
                });
            let semicolon = text.rfind(';').expect("fixture contains a semicolon");
            assert_eq!(
                (diagnostic.start, diagnostic.end),
                (semicolon, semicolon + 1),
                "unexpected diagnostic span for {name}"
            );
        }
    }

    #[test]
    fn parses_explicit_simulation_intent_declarations() {
        let text = r#"design d {
  analysis dc_operating_point sim.dc;
  analysis ac_linear_sweep sim.ac source root.input points 11 start_frequency 10 Hz stop_frequency 2.5 kHz magnitude 1 V phase -90 deg;
  analysis transient sim.tran step 1 us stop 10 ms start 0 s uic true;
  assert net_voltage checks.dc analysis sim.dc net OUT sample scalar expected -1 V absolute_tolerance 0.01 V relative_tolerance 0.001 ratio;
  assert net_voltage checks.ac analysis sim.ac net OUT sample frequency 1 kHz expected 1 V absolute_tolerance 0.01 V relative_tolerance 0 ratio;
  assert net_voltage checks.tran analysis sim.tran net OUT sample time 2 ms expected 1 V absolute_tolerance 0.01 V relative_tolerance 0 ratio;
}"#;
        let tree = parse(SourceFile::new("simulation.circuitc", text))
            .expect("all explicit simulation forms must parse");
        assert_eq!(tree.design.declarations.len(), 6);
        assert!(matches!(
            &tree.design.declarations[0],
            DeclarationSyntax::SimulationAnalysis(analysis)
                if analysis.path.value == "sim.dc"
        ));
        assert!(matches!(
            &tree.design.declarations[3],
            DeclarationSyntax::SimulationAssertion(assertion)
                if assertion.path.value == "checks.dc"
        ));

        let ac = text
            .find("analysis ac_linear_sweep")
            .expect("fixture contains AC");
        let DeclarationSyntax::SimulationAnalysis(analysis) = &tree.design.declarations[1] else {
            panic!("second declaration must be AC analysis syntax");
        };
        assert_eq!(analysis.span.start, ac);
        assert_eq!(
            &text[analysis.path.span.start..analysis.path.span.end],
            "sim.ac"
        );
    }

    #[test]
    fn unsupported_simulation_forms_have_stable_machine_readable_diagnostics() {
        for (text, needle, path) in [
            (
                "design d { analysis logarithmic_sweep sim.bad; }",
                "logarithmic_sweep",
                "sim.bad",
            ),
            (
                "design d { assert net_voltage checks.bad analysis sim.dc net OUT sample median expected 1 V absolute_tolerance 0 V relative_tolerance 0 ratio; }",
                "median",
                "checks.bad",
            ),
        ] {
            let diagnostics = parse(SourceFile::new("unsupported.circuitc", text))
                .expect_err("unsupported simulation syntax must fail");
            let diagnostic = diagnostics
                .iter()
                .find(|diagnostic| diagnostic.code == "CC-LANG-PARSE-002")
                .expect("unsupported-form diagnostic must exist");
            let start = text.find(needle).expect("fixture contains bad keyword");
            assert_eq!(
                (diagnostic.start, diagnostic.end),
                (start, start + needle.len())
            );
            assert_eq!(diagnostic.semantic_path.as_deref(), Some(path));
        }
    }
}

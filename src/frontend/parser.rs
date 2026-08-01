use super::diagnostic::{SourceDiagnostic, sort_diagnostics};
use super::lexer::{Token, TokenKind, lex, token_word};
use super::syntax::{
    BindingSyntax, BoardItemSyntax, BoardSyntax, ComponentItemSyntax, ComponentKindSyntax,
    ComponentSyntax, DeclarationSyntax, DesignSyntax, FootprintItemSyntax, FootprintSyntax,
    NetSyntax, PadSyntax, PlacementSyntax, PointSyntax, QuantitySyntax, RectangleSyntax,
    RouteSyntax, SourceFile, Span, Spanned, SyntaxTree,
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
        if self.at_keyword("board") {
            return self.parse_board().map(DeclarationSyntax::Board);
        }
        self.unsupported("design declaration");
        self.recover_item();
        None
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
            let item = if self.at_keyword("resistance") || self.at_keyword("voltage") {
                self.parse_value()
            } else if self.at_keyword("terminals") {
                self.parse_terminals()
            } else if self.at_keyword("connect") {
                self.parse_connection()
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
            let item = if self.at_keyword("pad") {
                self.parse_pad()
                    .map(|pad| FootprintItemSyntax::Pad(Box::new(pad)))
            } else if self.at_keyword("bind") {
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

    fn parse_pad(&mut self) -> Option<PadSyntax> {
        let start = self.take().span;
        let number = self.expect_name("pad number")?;
        self.require_keyword("at")?;
        let offset = self.parse_point()?;
        self.require_keyword("size")?;
        let size = self.parse_point()?;
        self.require_keyword("shape")?;
        let shape = self.expect_name("pad shape")?;
        let end = self.expect_semicolon()?;
        Some(PadSyntax {
            number,
            offset,
            size,
            shape,
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
    use super::parse;
    use crate::frontend::syntax::SourceFile;

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
}

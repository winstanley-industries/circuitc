#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub(crate) const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub(crate) fn through(self, other: Self) -> Self {
        Self::new(self.start.min(other.start), self.end.max(other.end))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub(crate) fn new(value: T, span: Span) -> Self {
        Self { value, span }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFile {
    pub(crate) name: String,
    pub(crate) text: String,
    line_starts: Vec<usize>,
    char_starts: Vec<usize>,
}

impl SourceFile {
    pub(crate) fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        let mut line_starts = vec![0];
        line_starts.extend(
            text.bytes()
                .enumerate()
                .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
        );
        let char_starts = text.char_indices().map(|(index, _)| index).collect();
        Self {
            name: name.into(),
            text,
            line_starts,
            char_starts,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn line_column(&self, byte_offset: usize) -> (usize, usize) {
        let mut offset = byte_offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line_index = self.line_starts.partition_point(|start| *start <= offset) - 1;
        let line = line_index + 1;
        let line_start = self.line_starts[line_index];
        let chars_before_offset = self.char_starts.partition_point(|start| *start < offset);
        let chars_before_line = self
            .char_starts
            .partition_point(|start| *start < line_start);
        let column = chars_before_offset - chars_before_line + 1;
        (line, column)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxTree {
    pub(crate) source: SourceFile,
    pub span: Span,
    pub design_name: String,
    pub(crate) design: DesignSyntax,
}

impl SyntaxTree {
    pub fn source(&self) -> &SourceFile {
        &self.source
    }
}

#[cfg(test)]
mod tests {
    use super::SourceFile;

    #[test]
    fn line_column_defensively_rounds_down_to_a_utf8_boundary() {
        let source = SourceFile::new("utf8.circuitc", "aé\n");
        assert_eq!(source.line_column(2), (1, 2));
    }

    #[test]
    fn indexed_line_starts_scale_across_large_sources() {
        let text: String = (0..4096).map(|index| format!("line{index}\n")).collect();
        let source = SourceFile::new("large.circuitc", &text);
        let mut offset = 0;
        for line in 1..=4096 {
            assert_eq!(source.line_column(offset), (line, 1));
            offset = text[offset..]
                .find('\n')
                .map(|relative| offset + relative + 1)
                .expect("generated line must terminate");
        }
    }

    #[test]
    fn indexed_character_starts_scale_across_large_single_line_sources() {
        let text = "é".repeat(4096);
        let source = SourceFile::new("large-single-line.circuitc", &text);
        for column in 1..=4097 {
            assert_eq!(source.line_column((column - 1) * "é".len()), (1, column));
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DesignSyntax {
    pub name: Spanned<String>,
    pub declarations: Vec<DeclarationSyntax>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DeclarationSyntax {
    Net(NetSyntax),
    Module(ModuleSyntax),
    Component(ComponentSyntax),
    Board(BoardSyntax),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NetSyntax {
    pub name: Spanned<String>,
    pub is_ground: bool,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModuleSyntax {
    pub path: Spanned<String>,
    pub ports: Vec<PortSyntax>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PortSyntax {
    pub direction: Spanned<String>,
    pub name: Spanned<String>,
    pub electrical_type: Spanned<String>,
    pub state: ConnectionStateSyntax,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConnectionStateSyntax {
    Connected(Spanned<String>),
    NoConnect(Span),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ComponentKindSyntax {
    Resistor,
    DcSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ComponentSyntax {
    pub kind: ComponentKindSyntax,
    pub path: Spanned<String>,
    pub reference: Spanned<String>,
    pub items: Vec<ComponentItemSyntax>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ComponentItemSyntax {
    Part(PartSyntax),
    Symbol(SymbolSyntax),
    Model {
        library_id: Spanned<String>,
        span: Span,
    },
    SchematicPlacement(SchematicPlacementSyntax),
    Value {
        keyword: Spanned<String>,
        quantity: QuantitySyntax,
    },
    Terminals {
        positive: Spanned<String>,
        negative: Spanned<String>,
        span: Span,
    },
    Connection {
        pin: Spanned<String>,
        net: Spanned<String>,
        span: Span,
    },
    NoConnect {
        pin: Spanned<String>,
        span: Span,
    },
    Footprint(FootprintSyntax),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PartSyntax {
    pub logical_device: Spanned<String>,
    pub manufacturer: Option<Spanned<String>>,
    pub manufacturer_part_number: Option<Spanned<String>>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolSyntax {
    pub library_id: Spanned<String>,
    pub pins: Vec<SymbolPinSyntax>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolPinSyntax {
    pub pin: Spanned<String>,
    pub symbol_pin: Spanned<String>,
    pub electrical_type: Spanned<String>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SchematicPlacementSyntax {
    pub position: PointSyntax,
    pub rotation: Spanned<String>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuantitySyntax {
    pub number: Spanned<String>,
    pub unit: Spanned<String>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PointSyntax {
    pub x: QuantitySyntax,
    pub y: QuantitySyntax,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FootprintSyntax {
    pub library_id: Spanned<String>,
    pub items: Vec<FootprintItemSyntax>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FootprintItemSyntax {
    Binding(BindingSyntax),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BindingSyntax {
    pub pin: Spanned<String>,
    pub pad: Spanned<String>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BoardSyntax {
    pub items: Vec<BoardItemSyntax>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BoardItemSyntax {
    Rectangle(RectangleSyntax),
    Placement(PlacementSyntax),
    Route(Box<RouteSyntax>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RectangleSyntax {
    pub origin: PointSyntax,
    pub size: PointSyntax,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlacementSyntax {
    pub reference: Spanned<String>,
    pub position: PointSyntax,
    pub rotation: Spanned<String>,
    pub layer: Spanned<String>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RouteSyntax {
    pub path: Spanned<String>,
    pub net: Spanned<String>,
    pub start: PointSyntax,
    pub end: PointSyntax,
    pub width: QuantitySyntax,
    pub layer: Spanned<String>,
    pub span: Span,
}

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
    pub name: String,
    pub text: String,
}

impl SourceFile {
    pub(crate) fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
        }
    }

    pub(crate) fn line_column(&self, byte_offset: usize) -> (usize, usize) {
        let offset = byte_offset.min(self.text.len());
        let prefix = &self.text[..offset];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let line_start = prefix.rfind('\n').map_or(0, |position| position + 1);
        let column = self.text[line_start..offset].chars().count() + 1;
        (line, column)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxTree {
    pub source: SourceFile,
    pub span: Span,
    pub design_name: String,
    pub(crate) design: DesignSyntax,
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
    Component(ComponentSyntax),
    Board(BoardSyntax),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NetSyntax {
    pub name: Spanned<String>,
    pub is_ground: bool,
    pub span: Span,
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
    Footprint(FootprintSyntax),
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
    Pad(Box<PadSyntax>),
    Binding(BindingSyntax),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PadSyntax {
    pub number: Spanned<String>,
    pub offset: PointSyntax,
    pub size: PointSyntax,
    pub shape: Spanned<String>,
    pub span: Span,
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

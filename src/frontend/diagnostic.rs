use std::cmp::Ordering;
use std::fmt::{self, Write as _};

use super::syntax::{SourceFile, Span};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticFormat {
    Human,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelatedLocation {
    pub filename: String,
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDiagnostic {
    pub code: &'static str,
    pub filename: String,
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
    pub semantic_path: Option<String>,
    pub message: String,
    pub related: Vec<RelatedLocation>,
}

impl SourceDiagnostic {
    pub(crate) fn new(
        code: &'static str,
        source: &SourceFile,
        span: Span,
        semantic_path: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        let (line, column) = source.line_column(span.start);
        Self {
            code,
            filename: source.name.clone(),
            start: span.start,
            end: span.end,
            line,
            column,
            semantic_path,
            message: message.into(),
            related: Vec::new(),
        }
    }

    pub(crate) fn with_related(
        mut self,
        source: &SourceFile,
        span: Span,
        message: impl Into<String>,
    ) -> Self {
        let (line, column) = source.line_column(span.start);
        self.related.push(RelatedLocation {
            filename: source.name.clone(),
            start: span.start,
            end: span.end,
            line,
            column,
            message: message.into(),
        });
        self
    }
}

impl fmt::Display for SourceDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}: {}",
            self.filename, self.line, self.column, self.code
        )?;
        if let Some(path) = &self.semantic_path {
            write!(formatter, " [{path}]")?;
        }
        write!(
            formatter,
            ": {} (bytes {}..{})",
            self.message, self.start, self.end
        )?;
        for related in &self.related {
            write!(
                formatter,
                "\n  related {}:{}:{}: {} (bytes {}..{})",
                related.filename,
                related.line,
                related.column,
                related.message,
                related.start,
                related.end
            )?;
        }
        Ok(())
    }
}

pub fn render_diagnostics(diagnostics: &[SourceDiagnostic], format: DiagnosticFormat) -> String {
    match format {
        DiagnosticFormat::Human => diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        DiagnosticFormat::Json => render_json(diagnostics),
    }
}

pub(crate) fn sort_diagnostics(diagnostics: &mut [SourceDiagnostic]) {
    diagnostics.sort_by(compare_diagnostics);
}

fn compare_diagnostics(left: &SourceDiagnostic, right: &SourceDiagnostic) -> Ordering {
    (
        &left.filename,
        left.start,
        left.end,
        left.code,
        left.semantic_path.as_deref(),
        &left.message,
    )
        .cmp(&(
            &right.filename,
            right.start,
            right.end,
            right.code,
            right.semantic_path.as_deref(),
            &right.message,
        ))
}

fn render_json(diagnostics: &[SourceDiagnostic]) -> String {
    let mut output = String::from("[\n");
    for (index, diagnostic) in diagnostics.iter().enumerate() {
        if index != 0 {
            output.push_str(",\n");
        }
        output.push_str("  {\n");
        json_string_field(&mut output, 4, "code", diagnostic.code, true);
        json_string_field(&mut output, 4, "filename", &diagnostic.filename, true);
        number_field(&mut output, 4, "start", diagnostic.start, true);
        number_field(&mut output, 4, "end", diagnostic.end, true);
        number_field(&mut output, 4, "line", diagnostic.line, true);
        number_field(&mut output, 4, "column", diagnostic.column, true);
        output.push_str("    \"semantic_path\": ");
        match &diagnostic.semantic_path {
            Some(path) => write_json_string(&mut output, path),
            None => output.push_str("null"),
        }
        output.push_str(",\n");
        json_string_field(&mut output, 4, "message", &diagnostic.message, true);
        output.push_str("    \"related\": [");
        if !diagnostic.related.is_empty() {
            output.push('\n');
        }
        for (related_index, related) in diagnostic.related.iter().enumerate() {
            if related_index != 0 {
                output.push_str(",\n");
            }
            output.push_str("      {\n");
            json_string_field(&mut output, 8, "filename", &related.filename, true);
            number_field(&mut output, 8, "start", related.start, true);
            number_field(&mut output, 8, "end", related.end, true);
            number_field(&mut output, 8, "line", related.line, true);
            number_field(&mut output, 8, "column", related.column, true);
            json_string_field(&mut output, 8, "message", &related.message, false);
            output.push_str("      }");
        }
        if !diagnostic.related.is_empty() {
            output.push_str("\n    ");
        }
        output.push_str("]\n  }");
    }
    output.push_str("\n]\n");
    output
}

fn json_string_field(
    output: &mut String,
    indentation: usize,
    name: &str,
    value: &str,
    comma: bool,
) {
    write!(output, "{}\"{name}\": ", " ".repeat(indentation)).unwrap();
    write_json_string(output, value);
    output.push_str(if comma { ",\n" } else { "\n" });
}

fn number_field(output: &mut String, indentation: usize, name: &str, value: usize, comma: bool) {
    writeln!(
        output,
        "{}\"{name}\": {value}{}",
        " ".repeat(indentation),
        if comma { "," } else { "" }
    )
    .unwrap();
}

fn write_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                write!(output, "\\u{:04x}", u32::from(character)).unwrap();
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

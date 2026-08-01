use super::diagnostic::{SourceDiagnostic, sort_diagnostics};
use super::syntax::{SourceFile, Span, Spanned};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TokenKind {
    Word(String),
    Number(String),
    String(String),
    LeftBrace,
    RightBrace,
    LeftParen,
    RightParen,
    Comma,
    Semicolon,
    End,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub(crate) fn lex(source: &SourceFile) -> (Vec<Token>, Vec<SourceDiagnostic>) {
    let mut lexer = Lexer {
        source,
        offset: 0,
        tokens: Vec::new(),
        diagnostics: Vec::new(),
    };
    lexer.run();
    sort_diagnostics(&mut lexer.diagnostics);
    (lexer.tokens, lexer.diagnostics)
}

struct Lexer<'a> {
    source: &'a SourceFile,
    offset: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<SourceDiagnostic>,
}

impl Lexer<'_> {
    fn run(&mut self) {
        while self.offset < self.source.text.len() {
            let remaining = &self.source.text[self.offset..];
            if remaining.starts_with("//") {
                self.skip_comment();
                continue;
            }
            let character = remaining
                .chars()
                .next()
                .expect("remaining source is non-empty");
            if character.is_whitespace() {
                self.offset += character.len_utf8();
                continue;
            }
            let start = self.offset;
            let punctuation = match character {
                '{' => Some(TokenKind::LeftBrace),
                '}' => Some(TokenKind::RightBrace),
                '(' => Some(TokenKind::LeftParen),
                ')' => Some(TokenKind::RightParen),
                ',' => Some(TokenKind::Comma),
                ';' => Some(TokenKind::Semicolon),
                _ => None,
            };
            if let Some(kind) = punctuation {
                self.offset += character.len_utf8();
                self.tokens.push(Token {
                    kind,
                    span: Span::new(start, self.offset),
                });
                continue;
            }
            if character == '"' {
                self.lex_string();
                continue;
            }
            if is_word_character(character) {
                self.lex_atom();
                continue;
            }
            self.offset += character.len_utf8();
            self.diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-LEX-001",
                self.source,
                Span::new(start, self.offset),
                None,
                format!("unexpected character {character:?}"),
            ));
        }
        self.tokens.push(Token {
            kind: TokenKind::End,
            span: Span::new(self.source.text.len(), self.source.text.len()),
        });
    }

    fn skip_comment(&mut self) {
        self.offset += 2;
        while self.offset < self.source.text.len() {
            let character = self.source.text[self.offset..]
                .chars()
                .next()
                .expect("remaining comment is non-empty");
            self.offset += character.len_utf8();
            if character == '\n' {
                break;
            }
        }
    }

    fn lex_atom(&mut self) {
        let start = self.offset;
        while self.peek().is_some_and(is_word_character)
            && !self.source.text[self.offset..].starts_with("//")
        {
            self.advance();
        }
        let value = &self.source.text[start..self.offset];
        self.tokens.push(Token {
            kind: if decimal_is_lexical(value) {
                TokenKind::Number(value.to_owned())
            } else {
                TokenKind::Word(value.to_owned())
            },
            span: Span::new(start, self.offset),
        });
    }

    fn lex_string(&mut self) {
        let start = self.offset;
        self.offset += 1;
        let mut value = String::new();
        let mut terminated = false;
        while self.offset < self.source.text.len() {
            let character = self.peek().expect("remaining string is non-empty");
            self.advance();
            match character {
                '"' => {
                    terminated = true;
                    break;
                }
                '\\' => {
                    let Some(escaped) = self.peek() else {
                        break;
                    };
                    self.advance();
                    match escaped {
                        '"' | '\\' => value.push(escaped),
                        _ => {
                            self.diagnostics.push(SourceDiagnostic::new(
                                "CC-LANG-LEX-003",
                                self.source,
                                Span::new(self.offset - escaped.len_utf8() - 1, self.offset),
                                None,
                                format!("unsupported string escape \\{escaped}"),
                            ));
                            value.push(escaped);
                        }
                    }
                }
                '\n' | '\r' => break,
                character => value.push(character),
            }
        }
        if !terminated {
            self.diagnostics.push(SourceDiagnostic::new(
                "CC-LANG-LEX-002",
                self.source,
                Span::new(start, self.offset),
                None,
                "unterminated string literal",
            ));
        }
        self.tokens.push(Token {
            kind: TokenKind::String(value),
            span: Span::new(start, self.offset),
        });
    }

    fn peek(&self) -> Option<char> {
        self.source.text[self.offset..].chars().next()
    }

    fn advance(&mut self) {
        self.offset += self
            .peek()
            .expect("advance requires a remaining character")
            .len_utf8();
    }
}

fn is_word_character(character: char) -> bool {
    character.is_ascii_alphabetic()
        || character.is_ascii_digit()
        || matches!(character, '_' | '+' | '-' | '.' | '/')
        || (!character.is_ascii() && !character.is_control() && !character.is_whitespace())
}

fn decimal_is_lexical(value: &str) -> bool {
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let (whole, fractional) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    !whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && (!value.contains('.') || !fractional.is_empty())
        && fractional.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn token_word(token: &Token) -> Option<Spanned<String>> {
    match &token.kind {
        TokenKind::Word(value) | TokenKind::Number(value) => {
            Some(Spanned::new(value.clone(), token.span))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{TokenKind, lex};
    use crate::frontend::syntax::SourceFile;

    #[test]
    fn retains_utf8_byte_spans() {
        let source = SourceFile::new("utf8.circuitc", "// µV\nnet VIN;");
        let (tokens, diagnostics) = lex(&source);
        assert!(diagnostics.is_empty());
        assert_eq!(tokens[0].kind, TokenKind::Word("net".to_owned()));
        assert_eq!(tokens[0].span.start, "// µV\n".len());
        assert_eq!(
            &source.text[tokens[1].span.start..tokens[1].span.end],
            "VIN"
        );
    }

    #[test]
    fn reports_invalid_characters_and_continues() {
        let source = SourceFile::new("bad.circuitc", "net VIN =; net VOUT;");
        let (tokens, diagnostics) = lex(&source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "CC-LANG-LEX-001");
        assert!(
            tokens
                .iter()
                .any(|token| token.kind == TokenKind::Word("VOUT".to_owned()))
        );
    }

    #[test]
    fn comments_start_without_preceding_whitespace() {
        let source = SourceFile::new("comment.circuitc", "ground GND// comment\n;");
        let (tokens, diagnostics) = lex(&source);
        assert!(diagnostics.is_empty());
        assert_eq!(tokens[1].kind, TokenKind::Word("GND".to_owned()));
        assert_eq!(tokens[2].kind, TokenKind::Semicolon);
    }

    #[test]
    fn digit_leading_identities_are_maximal_atoms() {
        let source = SourceFile::new("atoms.circuitc", "net 1V; net 1-2; pad 1;");
        let (tokens, diagnostics) = lex(&source);
        assert!(diagnostics.is_empty());
        assert_eq!(tokens[1].kind, TokenKind::Word("1V".to_owned()));
        assert_eq!(tokens[4].kind, TokenKind::Word("1-2".to_owned()));
        assert_eq!(tokens[7].kind, TokenKind::Number("1".to_owned()));
    }
}

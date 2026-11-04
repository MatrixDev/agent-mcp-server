//! Syntax-highlighted code as styled paragraphs, one per source line.
//!
//! A small tokenizer, no grammar: comments, strings, numbers, keywords, types (capitalised
//! identifiers and the language's primitive types), functions (identifiers before "(") and
//! macros (Rust identifiers before "!"). Good enough for slides.

use anyhow::Result;

use crate::tools::keynote::format::drawing::Color;
use crate::tools::keynote::format::rich_text::{Paragraph, RunStyle, TextRun};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Text,
    Keyword,
    Type,
    String,
    Number,
    Comment,
    Function,
    Macro,
}

/// Colours per token kind
#[derive(Debug, Clone)]
pub struct CodeTheme {
    pub text: Color,
    pub keyword: Color,
    pub type_: Color,
    pub string: Color,
    pub number: Color,
    pub comment: Color,
    pub function: Color,
    pub macro_: Color,
    pub background: Color,
    pub border: Option<Color>,
}

impl Default for CodeTheme {
    fn default() -> Self {
        let color = |e: &str| Color::parse(e).unwrap();
        Self {
            text: color("#F4EFE6"),
            keyword: color("#F2762E"),
            type_: color("#5DA9E9"),
            string: color("#6CC38B"),
            number: color("#F2C94C"),
            comment: color("#7E7A72"),
            function: color("#E8D8A8"),
            macro_: color("#C792EA"),
            background: color("#0F0E12"),
            border: Some(color("#2E2D35")),
        }
    }
}

impl CodeTheme {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn color(&self, token: Token) -> Color {
        match token {
            Token::Text => self.text,
            Token::Keyword => self.keyword,
            Token::Type => self.type_,
            Token::String => self.string,
            Token::Number => self.number,
            Token::Comment => self.comment,
            Token::Function => self.function,
            Token::Macro => self.macro_,
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
struct Language {
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    line_comment: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    macros: bool,
    /// a quote that isn't a string, Rust lifetimes
    lifetimes: bool,
}

const RUST: Language = Language {
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false",
        "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
        "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while",
        "yield",
    ],
    types: &[
        "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64",
        "bool", "char", "str",
    ],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    macros: true,
    lifetimes: true,
};

const C_FAMILY: Language = Language {
    keywords: &[
        "auto", "break", "case", "catch", "class", "const", "constexpr", "continue", "default", "delete", "do",
        "else", "enum", "explicit", "export", "extends", "false", "final", "finally", "for", "fun", "func", "function",
        "go", "if", "implements", "import", "in", "inline", "interface", "let", "namespace", "new", "nil", "null",
        "nullptr", "override", "package", "private", "protected", "public", "return", "sizeof", "static", "struct",
        "super", "switch", "template", "this", "throw", "throws", "true", "try", "typedef", "typename", "union",
        "using", "val", "var", "virtual", "void", "volatile", "while", "async", "await", "yield", "guard", "defer",
        "range", "map", "chan", "select", "type", "self", "def", "init", "deinit", "protocol", "extension",
    ],
    types: &[
        "int", "long", "short", "char", "float", "double", "bool", "boolean", "unsigned", "signed", "size_t",
        "string", "String", "byte", "Int", "Double", "Float", "Bool", "number", "any", "uint8_t", "int32_t",
        "int64_t", "uint32_t", "uint64_t",
    ],
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    macros: false,
    lifetimes: false,
};

const PYTHON: Language = Language {
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
        "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "None",
        "nonlocal", "not", "or", "pass", "raise", "return", "True", "try", "while", "with", "yield", "self",
    ],
    types: &["int", "float", "str", "bool", "list", "dict", "set", "tuple", "bytes"],
    line_comment: &["#"],
    block_comment: None,
    macros: false,
    lifetimes: false,
};

const SHELL: Language = Language {
    keywords: &[
        "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case", "esac", "function", "return",
        "export", "local", "in",
    ],
    types: &[],
    line_comment: &["#"],
    block_comment: None,
    macros: false,
    lifetimes: false,
};

const PLAIN: Language = Language {
    keywords: &[],
    types: &[],
    line_comment: &[],
    block_comment: None,
    macros: false,
    lifetimes: false,
};

fn language(name: &str) -> &'static Language {
    match name.trim().to_lowercase().as_str() {
        "rust" | "rs" => &RUST,
        "c" | "cpp" | "c++" | "h" | "hpp" | "java" | "kotlin" | "kt" | "swift" | "go" | "javascript" | "js"
        | "typescript" | "ts" | "csharp" | "cs" | "c#" | "dart" | "scala" => &C_FAMILY,
        "python" | "py" => &PYTHON,
        "sh" | "bash" | "zsh" | "shell" | "toml" | "yaml" | "yml" => &SHELL,
        _ => &PLAIN,
    }
}

////////////////////////////////////////////////////////////////////////////////
/// Tokens of every line, a block comment may span lines
pub fn tokenize(code: &str, language_name: &str) -> Vec<Vec<(Token, String)>> {
    let language = language(language_name);
    let mut lines = Vec::new();
    let mut in_block = false;

    for line in code.replace("\r\n", "\n").replace('\t', "    ").split('\n') {
        let chars: Vec<char> = line.chars().collect();
        let mut tokens: Vec<(Token, String)> = Vec::new();
        let push = |tokens: &mut Vec<(Token, String)>, token: Token, text: String| {
            if text.is_empty() {
                return;
            }
            match tokens.last_mut() {
                Some((last, existing)) if *last == token => existing.push_str(&text),
                _ => tokens.push((token, text)),
            }
        };
        let starts = |index: usize, pattern: &str| {
            let pattern: Vec<char> = pattern.chars().collect();
            chars.len() >= index + pattern.len() && chars[index..index + pattern.len()] == pattern[..]
        };

        let mut index = 0;
        while index < chars.len() {
            if in_block {
                let (_, end) = language.block_comment.unwrap();
                let close = (index..chars.len()).find(|e| starts(*e, end));
                let stop = close.map(|e| e + end.chars().count()).unwrap_or(chars.len());
                push(&mut tokens, Token::Comment, chars[index..stop].iter().collect());
                in_block = close.is_none();
                index = stop;
                continue;
            }
            if let Some((start, _)) = language.block_comment
                && starts(index, start)
            {
                in_block = true;
                continue;
            }
            if language.line_comment.iter().any(|e| starts(index, e)) {
                push(&mut tokens, Token::Comment, chars[index..].iter().collect());
                break;
            }

            let character = chars[index];
            if character == '"' || character == '`' || (character == '\'' && !language.lifetimes) {
                let mut end = index + 1;
                while end < chars.len() && chars[end] != character {
                    end += if chars[end] == '\\' { 2 } else { 1 };
                }
                let end = (end + 1).min(chars.len());
                push(&mut tokens, Token::String, chars[index..end].iter().collect());
                index = end;
                continue;
            }
            if character == '\'' && language.lifetimes {
                // a char literal 'x' or '\n', otherwise a lifetime
                let literal = match (chars.get(index + 1), chars.get(index + 2), chars.get(index + 3)) {
                    (Some('\\'), Some(_), Some('\'')) => Some(index + 4),
                    (Some(_), Some('\''), _) => Some(index + 3),
                    _ => None,
                };
                if let Some(end) = literal {
                    push(&mut tokens, Token::String, chars[index..end].iter().collect());
                    index = end;
                    continue;
                }
            }
            if character.is_ascii_digit() {
                let mut end = index;
                while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_' || chars[end] == '.') {
                    end += 1;
                }
                push(&mut tokens, Token::Number, chars[index..end].iter().collect());
                index = end;
                continue;
            }
            if character.is_alphabetic() || character == '_' {
                let mut end = index;
                while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
                    end += 1;
                }
                let word: String = chars[index..end].iter().collect();
                let next = chars[end..].iter().find(|e| **e != ' ');
                let token = if language.keywords.contains(&word.as_str()) {
                    Token::Keyword
                } else if language.macros && chars.get(end) == Some(&'!') && chars.get(end + 1) != Some(&'=') {
                    Token::Macro
                } else if language.types.contains(&word.as_str())
                    || (word.chars().next().is_some_and(char::is_uppercase) && !word.chars().all(|e| e.is_uppercase() || e == '_'))
                {
                    Token::Type
                } else if next == Some(&'(') {
                    Token::Function
                } else {
                    Token::Text
                };
                let end = if token == Token::Macro { end + 1 } else { end };
                push(&mut tokens, token, chars[index..end].iter().collect());
                index = end;
                continue;
            }
            push(&mut tokens, Token::Text, character.to_string());
            index += 1;
        }
        lines.push(tokens);
    }
    lines
}

////////////////////////////////////////////////////////////////////////////////
/// How a code block looks
#[derive(Debug, Clone)]
pub struct CodeOptions {
    pub language: String,
    pub theme: CodeTheme,
    pub font: String,
    pub size: f32,
    /// points from baseline to baseline
    pub line_height: Option<f32>,
    /// 1-based lines shown at full strength, the others dimmed
    pub highlight_lines: Vec<usize>,
    pub line_numbers: bool,
}

////////////////////////////////////////////////////////////////////////////////
/// Paragraphs of a code block, one per line
pub fn code_paragraphs(code: &str, options: &CodeOptions) -> Result<Vec<Paragraph>> {
    let code = code.trim_end_matches(['\n', '\r']);
    let lines = tokenize(code, &options.language);
    let width = lines.len().to_string().len();
    let dim = |color: Color, line: usize| {
        if options.highlight_lines.is_empty() || options.highlight_lines.contains(&line) {
            color
        } else {
            color.with_alpha(color.a * 0.35)
        }
    };
    let style = |color: Color| RunStyle {
        font: Some(options.font.clone()),
        size: Some(options.size),
        color: Some(color),
        ..Default::default()
    };

    let mut paragraphs = Vec::new();
    for (index, tokens) in lines.into_iter().enumerate() {
        let line = index + 1;
        let mut runs = Vec::new();
        if options.line_numbers {
            runs.push(TextRun {
                text: format!("{line:>width$}  "),
                style: style(dim(options.theme.comment, line)),
            });
        }
        for (token, text) in tokens {
            runs.push(TextRun {
                text,
                style: style(dim(options.theme.color(token), line)),
            });
        }
        if runs.is_empty() {
            runs.push(TextRun {
                text: String::new(),
                style: style(options.theme.text),
            });
        }
        paragraphs.push(Paragraph {
            runs,
            align: Some(crate::tools::keynote::format::rich_text::Align::Left),
            bullet: Some(crate::tools::keynote::format::rich_text::Bullet::None),
            line_spacing: options
                .line_height
                .map(crate::tools::keynote::format::rich_text::LineSpacing::Exact),
            space_before: Some(0.0),
            space_after: Some(0.0),
            ..Default::default()
        });
    }
    Ok(paragraphs)
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &[(Token, String)]) -> Vec<(Token, &str)> {
        line.iter().map(|(token, text)| (*token, text.as_str())).collect()
    }

    #[test]
    fn tokenizes_rust() {
        let lines = tokenize("fn main() {\n    let v = vec![1, 2]; // done\n    let s: &'a str = \"x\";\n}", "rust");
        assert_eq!(lines.len(), 4);
        assert_eq!(kinds(&lines[0])[..3], [(Token::Keyword, "fn"), (Token::Text, " "), (Token::Function, "main")]);
        let second = kinds(&lines[1]);
        assert!(second.contains(&(Token::Macro, "vec!")));
        assert!(second.contains(&(Token::Number, "1")));
        assert!(second.contains(&(Token::Comment, "// done")));
        let third = kinds(&lines[2]);
        assert!(third.contains(&(Token::Type, "str")));
        assert!(third.contains(&(Token::String, "\"x\"")));
    }

    #[test]
    fn tokenizes_block_comments_and_other_languages() {
        let lines = tokenize("int x = 1; /* a\nb */ return x;", "c");
        assert_eq!(kinds(&lines[1])[0], (Token::Comment, "b */"));
        assert!(kinds(&lines[1]).contains(&(Token::Keyword, "return")));
        let python = tokenize("def f(x): # hi\n    return 'a'", "python");
        assert!(kinds(&python[1]).contains(&(Token::String, "'a'")));
    }

    #[test]
    fn builds_paragraphs() {
        let options = CodeOptions {
            language: "rust".into(),
            theme: CodeTheme::default(),
            font: "Menlo".into(),
            size: 26.0,
            line_height: Some(38.0),
            highlight_lines: vec![2],
            line_numbers: true,
        };
        let paragraphs = code_paragraphs("let a = 1;\nlet b = 2;\n", &options).unwrap();
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].runs[0].text, "1  ");
        let dimmed = paragraphs[0].runs[1].style.color.unwrap();
        let full = paragraphs[1].runs[1].style.color.unwrap();
        assert!(dimmed.a < full.a);
    }
}

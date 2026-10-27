//! A light syntax highlighter for the diff: per language a few rules
//! (comments, strings, keywords), and words told apart by their shape —
//! a call, a type, a constant, a number. It reads a line at a time,
//! keeping only whether a block comment or a long string is still open,
//! so it is fast and never quite a parser.

use std::ops::Range;

/// What a run of a line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Keyword,
    String,
    Comment,
    /// Numbers, and `true`, `null`, `UPPER_CASE` and their like.
    Constant,
    Type,
    Function,
}

/// Runs of a line by char index, in order, never overlapping. What
/// isn't in one is plain.
pub type Spans = Vec<(Range<usize>, Kind)>;

/// How a language looks, as far as this highlighter knows it.
pub struct Lang {
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    /// Quote characters a string opens and closes with, on one line.
    quotes: &'static [char],
    /// A string that may run over lines: `"""` and its like.
    long_strings: &'static [&'static str],
    keywords: &'static [&'static str],
    constants: &'static [&'static str],
    /// Types by name, beside those told by a capital.
    types: &'static [&'static str],
    /// A word starting with a capital is a type.
    capital_types: bool,
    /// `name!(` is a call (Rust's macros).
    bang_calls: bool,
    /// `$name` and `@name` are one word.
    sigils: &'static [char],
    /// `-` may be inside a word (CSS, Lisp, shell options).
    dash_words: bool,
    /// Keywords match whatever their case (SQL).
    ignore_case: bool,
}

/// What is still open at the end of a line, for the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Code,
    /// Inside a block comment.
    Comment,
    /// Inside the long string opened by this delimiter.
    String(&'static str),
}

const C_TYPES: &[&str] = &[
    "void", "char", "short", "int", "long", "float", "double", "signed", "unsigned", "bool",
    "size_t", "ssize_t", "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t", "uint16_t",
    "uint32_t", "uint64_t", "auto",
];

static RUST: Lang = Lang {
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
        "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "type",
        "unsafe", "use", "where", "while", "yield",
    ],
    constants: &["true", "false"],
    types: &[
        "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32",
        "i64", "i128", "isize", "f32", "f64",
    ],
    bang_calls: true,
    ..C
};

static C: Lang = Lang {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    quotes: &['"', '\''],
    long_strings: &[],
    keywords: &[
        "break",
        "case",
        "const",
        "continue",
        "default",
        "do",
        "else",
        "enum",
        "extern",
        "for",
        "goto",
        "if",
        "inline",
        "register",
        "return",
        "sizeof",
        "static",
        "struct",
        "switch",
        "typedef",
        "union",
        "volatile",
        "while",
        "#include",
        "#define",
        "#if",
        "#ifdef",
        "#ifndef",
        "#else",
        "#elif",
        "#endif",
        "#pragma",
        "class",
        "namespace",
        "public",
        "private",
        "protected",
        "template",
        "typename",
        "virtual",
        "override",
        "new",
        "delete",
        "this",
        "using",
        "try",
        "catch",
        "throw",
        "operator",
        "constexpr",
        "noexcept",
        "final",
    ],
    constants: &["true", "false", "NULL", "nullptr"],
    types: C_TYPES,
    capital_types: true,
    bang_calls: false,
    sigils: &['#'],
    dash_words: false,
    ignore_case: false,
};

static GO: Lang = Lang {
    quotes: &['"', '\'', '`'],
    keywords: &[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "interface",
        "map",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "type",
        "var",
    ],
    constants: &["true", "false", "nil", "iota"],
    types: &[
        "bool",
        "byte",
        "rune",
        "string",
        "error",
        "any",
        "int",
        "int8",
        "int16",
        "int32",
        "int64",
        "uint",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "uintptr",
        "float32",
        "float64",
        "complex64",
        "complex128",
    ],
    sigils: &[],
    ..C
};

static JS: Lang = Lang {
    quotes: &['"', '\'', '`'],
    keywords: &[
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "export",
        "extends",
        "finally",
        "for",
        "from",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "let",
        "new",
        "of",
        "return",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "try",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        "yield",
        "type",
        "interface",
        "enum",
        "implements",
        "private",
        "public",
        "protected",
        "readonly",
        "declare",
        "namespace",
        "abstract",
        "as",
        "satisfies",
        "keyof",
    ],
    constants: &["true", "false", "null", "undefined", "NaN", "Infinity"],
    types: &[
        "string", "number", "boolean", "any", "unknown", "never", "object", "bigint", "symbol",
    ],
    sigils: &['$', '@'],
    ..C
};

static JAVA: Lang = Lang {
    keywords: &[
        "abstract",
        "break",
        "case",
        "catch",
        "class",
        "continue",
        "default",
        "do",
        "else",
        "enum",
        "extends",
        "final",
        "finally",
        "for",
        "if",
        "implements",
        "import",
        "instanceof",
        "interface",
        "new",
        "package",
        "private",
        "protected",
        "public",
        "return",
        "static",
        "super",
        "switch",
        "synchronized",
        "this",
        "throw",
        "throws",
        "try",
        "while",
        "var",
        "val",
        "fun",
        "object",
        "when",
        "is",
        "in",
        "override",
        "open",
        "data",
        "sealed",
        "companion",
        "namespace",
        "using",
        "async",
        "await",
        "record",
        "struct",
        "get",
        "set",
    ],
    constants: &["true", "false", "null"],
    types: &[
        "void", "boolean", "byte", "char", "short", "int", "long", "float", "double", "string",
        "object", "var",
    ],
    sigils: &['@'],
    ..C
};

static ZIG: Lang = Lang {
    block_comment: None,
    long_strings: &[],
    keywords: &[
        "const",
        "var",
        "fn",
        "pub",
        "return",
        "if",
        "else",
        "while",
        "for",
        "switch",
        "break",
        "continue",
        "struct",
        "enum",
        "union",
        "error",
        "defer",
        "errdefer",
        "try",
        "catch",
        "comptime",
        "inline",
        "orelse",
        "and",
        "or",
        "test",
        "usingnamespace",
        "extern",
        "export",
    ],
    constants: &["true", "false", "null", "undefined"],
    types: &[
        "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize", "f32", "f64",
        "bool", "void", "anyerror", "anytype", "type",
    ],
    sigils: &['@'],
    ..C
};

static PYTHON: Lang = Lang {
    line_comments: &["#"],
    block_comment: None,
    quotes: &['"', '\''],
    long_strings: &["\"\"\"", "'''"],
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
        "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is",
        "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with",
        "yield", "match", "case", "self",
    ],
    constants: &["True", "False", "None"],
    types: &[
        "int", "float", "str", "bytes", "bool", "list", "dict", "set", "tuple", "object",
    ],
    capital_types: true,
    bang_calls: false,
    sigils: &['@'],
    dash_words: false,
    ignore_case: false,
};

static RUBY: Lang = Lang {
    long_strings: &[],
    keywords: &[
        "alias",
        "and",
        "begin",
        "break",
        "case",
        "class",
        "def",
        "do",
        "else",
        "elsif",
        "end",
        "ensure",
        "for",
        "if",
        "in",
        "module",
        "next",
        "not",
        "or",
        "redo",
        "rescue",
        "retry",
        "return",
        "self",
        "super",
        "then",
        "unless",
        "until",
        "when",
        "while",
        "yield",
        "require",
        "attr_reader",
        "attr_accessor",
    ],
    constants: &["true", "false", "nil"],
    types: &[],
    sigils: &['@', '$', ':'],
    ..PYTHON
};

static SHELL: Lang = Lang {
    line_comments: &["#"],
    block_comment: None,
    quotes: &['"', '\''],
    long_strings: &[],
    keywords: &[
        "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
        "in", "function", "return", "local", "export", "readonly", "set", "unset", "source",
        "exit", "break", "continue", "eval", "exec", "shift", "trap",
    ],
    constants: &["true", "false"],
    types: &[],
    capital_types: false,
    bang_calls: false,
    sigils: &['$'],
    dash_words: true,
    ignore_case: false,
};

static LUA: Lang = Lang {
    line_comments: &["--"],
    block_comment: Some(("--[[", "]]")),
    quotes: &['"', '\''],
    long_strings: &["[["],
    keywords: &[
        "and", "break", "do", "else", "elseif", "end", "for", "function", "goto", "if", "in",
        "local", "not", "or", "repeat", "return", "then", "until", "while",
    ],
    constants: &["true", "false", "nil"],
    types: &[],
    capital_types: false,
    bang_calls: false,
    sigils: &[],
    dash_words: false,
    ignore_case: false,
};

static SQL: Lang = Lang {
    line_comments: &["--"],
    block_comment: Some(("/*", "*/")),
    quotes: &['\''],
    long_strings: &[],
    keywords: &[
        "select",
        "from",
        "where",
        "and",
        "or",
        "not",
        "insert",
        "into",
        "values",
        "update",
        "set",
        "delete",
        "create",
        "table",
        "drop",
        "alter",
        "add",
        "index",
        "join",
        "left",
        "right",
        "inner",
        "outer",
        "on",
        "as",
        "group",
        "by",
        "order",
        "having",
        "limit",
        "primary",
        "key",
        "foreign",
        "references",
        "default",
        "unique",
        "is",
        "in",
        "like",
        "distinct",
        "union",
        "case",
        "when",
        "then",
        "else",
        "end",
        "begin",
        "commit",
    ],
    constants: &["null", "true", "false"],
    types: &[
        "int",
        "integer",
        "bigint",
        "text",
        "varchar",
        "boolean",
        "real",
        "blob",
        "timestamp",
        "date",
    ],
    capital_types: false,
    bang_calls: false,
    sigils: &[],
    dash_words: false,
    ignore_case: true,
};

static NIX: Lang = Lang {
    line_comments: &["#"],
    block_comment: Some(("/*", "*/")),
    quotes: &['"'],
    long_strings: &["''"],
    keywords: &[
        "let", "in", "with", "rec", "inherit", "if", "then", "else", "assert", "import",
    ],
    constants: &["true", "false", "null"],
    types: &[],
    capital_types: false,
    bang_calls: false,
    sigils: &[],
    dash_words: true,
    ignore_case: false,
};

static CSS: Lang = Lang {
    line_comments: &[],
    block_comment: Some(("/*", "*/")),
    quotes: &['"', '\''],
    long_strings: &[],
    keywords: &[
        "@media",
        "@import",
        "@keyframes",
        "@font-face",
        "!important",
    ],
    constants: &[],
    types: &[],
    capital_types: false,
    bang_calls: false,
    sigils: &['@', '#', '.', '!'],
    dash_words: true,
    ignore_case: false,
};

/// TOML, YAML, INI and their like: comments, strings, numbers.
static CONFIG: Lang = Lang {
    line_comments: &["#"],
    block_comment: None,
    quotes: &['"', '\''],
    long_strings: &["\"\"\"", "'''"],
    keywords: &[],
    constants: &["true", "false", "null", "yes", "no", "on", "off"],
    types: &[],
    capital_types: false,
    bang_calls: false,
    sigils: &[],
    dash_words: true,
    ignore_case: false,
};

static JSON: Lang = Lang {
    line_comments: &["//"],
    block_comment: Some(("/*", "*/")),
    quotes: &['"'],
    long_strings: &[],
    keywords: &[],
    constants: &["true", "false", "null"],
    types: &[],
    capital_types: false,
    bang_calls: false,
    sigils: &[],
    dash_words: false,
    ignore_case: false,
};

static HTML: Lang = Lang {
    line_comments: &[],
    block_comment: Some(("<!--", "-->")),
    quotes: &['"', '\''],
    long_strings: &[],
    keywords: &[],
    constants: &[],
    types: &[],
    capital_types: false,
    bang_calls: false,
    sigils: &[],
    dash_words: true,
    ignore_case: false,
};

/// The language of the file at `path`, by its name; `None` when it's
/// not one this knows (it shows plain).
pub fn lang(path: &str) -> Option<&'static Lang> {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name {
        "Makefile" | "makefile" | "Dockerfile" | "PKGBUILD" | ".bashrc" | ".zshrc" | ".profile"
        | "Containerfile" | "justfile" | "Justfile" => return Some(&SHELL),
        "Cargo.lock" | ".gitconfig" | ".editorconfig" => return Some(&CONFIG),
        _ => {}
    }
    let (_, ext) = name.rsplit_once('.')?;
    Some(match ext.to_ascii_lowercase().as_str() {
        "rs" => &RUST,
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "m" | "mm" | "glsl" | "frag" | "vert"
        | "wgsl" | "hlsl" | "proto" => &C,
        "go" => &GO,
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "vue" | "svelte" => &JS,
        "java" | "kt" | "kts" | "scala" | "cs" | "swift" | "dart" | "groovy" | "gradle" => &JAVA,
        "zig" => &ZIG,
        "py" | "pyi" => &PYTHON,
        "rb" | "rake" => &RUBY,
        "sh" | "bash" | "zsh" | "fish" | "mk" => &SHELL,
        "lua" => &LUA,
        "sql" => &SQL,
        "nix" => &NIX,
        "css" | "scss" | "sass" | "less" => &CSS,
        "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "desktop" | "service" | "env" => &CONFIG,
        "json" | "jsonc" | "json5" => &JSON,
        "html" | "htm" | "xml" | "svg" | "xhtml" => &HTML,
        _ => return None,
    })
}

/// The runs of `line` in `lang`, starting in `state` and leaving it as
/// the line ends.
pub fn highlight(lang: &Lang, line: &str, state: &mut State) -> Spans {
    let chars: Vec<char> = line.chars().collect();
    let mut spans: Spans = Vec::new();
    let mut push = |range: Range<usize>, kind: Kind| {
        if range.is_empty() {
            return;
        }
        match spans.last_mut() {
            Some((last, k)) if *k == kind && last.end == range.start => last.end = range.end,
            _ => spans.push((range, kind)),
        }
    };
    let at = |i: usize, s: &str| {
        s.chars()
            .enumerate()
            .all(|(k, c)| chars.get(i + k) == Some(&c))
    };
    let len = |s: &str| s.chars().count();
    let word_char = |c: char| c.is_alphanumeric() || c == '_' || (lang.dash_words && c == '-');
    let mut i = 0;
    // A comment or long string left open by the line before.
    match *state {
        State::Code => {}
        State::Comment => {
            let (_, close) = lang.block_comment.unwrap_or(("", ""));
            let end = (0..chars.len()).find(|&k| !close.is_empty() && at(k, close));
            i = end.map_or(chars.len(), |k| k + len(close));
            push(0..i, Kind::Comment);
            if end.is_some() {
                *state = State::Code;
            }
        }
        State::String(delim) => {
            let close = closing(delim);
            let end = (0..chars.len()).find(|&k| at(k, close));
            i = end.map_or(chars.len(), |k| k + len(close));
            push(0..i, Kind::String);
            if end.is_some() {
                *state = State::Code;
            }
        }
    }
    let mut prev_word: Option<&str> = None;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if let Some((open, close)) = lang.block_comment
            && at(i, open)
        {
            let from = i + len(open);
            match (from..chars.len()).find(|&k| at(k, close)) {
                Some(k) => {
                    push(i..k + len(close), Kind::Comment);
                    i = k + len(close);
                }
                None => {
                    push(i..chars.len(), Kind::Comment);
                    *state = State::Comment;
                    return spans;
                }
            }
            continue;
        }
        // `#` right after a word (`a#b`) isn't a comment.
        if lang
            .line_comments
            .iter()
            .any(|m| at(i, m) && (*m != "#" || i == 0 || !word_char(chars[i - 1])))
        {
            push(i..chars.len(), Kind::Comment);
            return spans;
        }
        if let Some(&open) = lang.long_strings.iter().find(|s| at(i, s)) {
            let close = closing(open);
            let from = i + len(open);
            match (from..chars.len()).find(|&k| at(k, close)) {
                Some(k) => {
                    push(i..k + len(close), Kind::String);
                    i = k + len(close);
                }
                None => {
                    push(i..chars.len(), Kind::String);
                    *state = State::String(open);
                    return spans;
                }
            }
            continue;
        }
        if lang.quotes.contains(&c) && !lifetime(lang, &chars, i) {
            let mut k = i + 1;
            while k < chars.len() && chars[k] != c {
                k += if chars[k] == '\\' { 2 } else { 1 };
            }
            let end = (k + 1).min(chars.len());
            push(i..end, Kind::String);
            i = end;
            continue;
        }
        let sigil = lang.sigils.contains(&c) && chars.get(i + 1).is_some_and(|&n| word_char(n));
        if word_char(c) || sigil {
            let start = i;
            i += 1;
            while i < chars.len()
                && (word_char(chars[i]) || (c.is_ascii_digit() && chars[i] == '.'))
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let kind = word_kind(lang, &word, &chars, i, prev_word);
            if let Some(kind) = kind {
                push(start..i, kind);
            }
            prev_word = lang.keywords.iter().copied().find(|k| *k == word);
            continue;
        }
        prev_word = None;
        i += 1;
    }
    spans
}

/// What closes a long string opened by `open`.
fn closing(open: &'static str) -> &'static str {
    match open {
        "[[" => "]]",
        other => other,
    }
}

/// A Rust lifetime or label (`'a`), not a char: a quote then a word with
/// no quote closing it right after.
fn lifetime(lang: &Lang, chars: &[char], i: usize) -> bool {
    if !lang.bang_calls || chars[i] != '\'' {
        return false;
    }
    let mut k = i + 1;
    while k < chars.len() && (chars[k].is_alphanumeric() || chars[k] == '_') {
        k += 1;
    }
    k > i + 1 && chars.get(k) != Some(&'\'')
}

/// What `word`, ending before `chars[end]`, is; `None` when plain.
fn word_kind(
    lang: &Lang,
    word: &str,
    chars: &[char],
    end: usize,
    prev: Option<&str>,
) -> Option<Kind> {
    let first = word.chars().next()?;
    if first.is_ascii_digit() {
        return Some(Kind::Constant);
    }
    let w = if lang.ignore_case {
        word.to_ascii_lowercase()
    } else {
        word.to_string()
    };
    if lang.keywords.contains(&w.as_str()) {
        return Some(Kind::Keyword);
    }
    if lang.constants.contains(&w.as_str()) {
        return Some(Kind::Constant);
    }
    if lang.types.contains(&w.as_str()) {
        return Some(Kind::Type);
    }
    // `$name`, `@name`, `:name`, `.class`.
    if lang.sigils.contains(&first) {
        return Some(Kind::Constant);
    }
    let called = match chars.get(end) {
        Some('(') => true,
        Some('!') => lang.bang_calls && matches!(chars.get(end + 1), Some('(' | '[' | '{')),
        _ => false,
    };
    // `fn name`, `def name`: the name of what is being defined.
    let defined = matches!(prev, Some("fn" | "def" | "func" | "function" | "fun"));
    if called || defined {
        return Some(Kind::Function);
    }
    if lang.capital_types && first.is_uppercase() {
        let upper = word.len() > 1
            && word
                .chars()
                .all(|c| c.is_uppercase() || c.is_ascii_digit() || c == '_');
        return Some(if upper { Kind::Constant } else { Kind::Type });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `line`'s runs as their text and kind.
    fn runs(lang: &Lang, line: &str, state: &mut State) -> Vec<(String, Kind)> {
        let chars: Vec<char> = line.chars().collect();
        highlight(lang, line, state)
            .into_iter()
            .map(|(r, k)| (chars[r].iter().collect(), k))
            .collect()
    }

    fn of(kind: Kind, words: &[&str]) -> Vec<(String, Kind)> {
        words.iter().map(|w| (w.to_string(), kind)).collect()
    }

    #[test]
    fn languages_by_name() {
        assert!(std::ptr::eq(lang("src/main.rs").unwrap(), &RUST));
        assert!(std::ptr::eq(lang("a/b/Makefile").unwrap(), &SHELL));
        assert!(std::ptr::eq(lang("x.TSX").unwrap(), &JS));
        assert!(lang("README").is_none());
        assert!(lang("notes.md").is_none());
    }

    #[test]
    fn rust_reads_as_rust() {
        let mut state = State::Code;
        let got = runs(
            &RUST,
            r#"pub fn go<'a>(x: &'a str) -> Option<u8> { println!("hi {x}"); None } // done"#,
            &mut state,
        );
        assert_eq!(
            got,
            [
                ("pub".into(), Kind::Keyword),
                ("fn".into(), Kind::Keyword),
                ("go".into(), Kind::Function),
                ("str".into(), Kind::Type),
                ("Option".into(), Kind::Type),
                ("u8".into(), Kind::Type),
                ("println".into(), Kind::Function),
                ("\"hi {x}\"".into(), Kind::String),
                ("None".into(), Kind::Type),
                ("// done".into(), Kind::Comment),
            ]
        );
        assert_eq!(state, State::Code);
        let got = runs(&RUST, "let c = 'x'; const MAX: usize = 0x1f;", &mut state);
        assert_eq!(
            got,
            [
                ("let".into(), Kind::Keyword),
                ("'x'".into(), Kind::String),
                ("const".into(), Kind::Keyword),
                ("MAX".into(), Kind::Constant),
                ("usize".into(), Kind::Type),
                ("0x1f".into(), Kind::Constant),
            ]
        );
    }

    #[test]
    fn comments_and_strings_run_over_lines() {
        let mut state = State::Code;
        assert_eq!(
            runs(&C, "int x; /* one", &mut state),
            [("int".into(), Kind::Type), ("/* one".into(), Kind::Comment)]
        );
        assert_eq!(state, State::Comment);
        assert_eq!(runs(&C, "two", &mut state), of(Kind::Comment, &["two"]));
        assert_eq!(
            runs(&C, "three */ return 1;", &mut state),
            [
                ("three */".into(), Kind::Comment),
                ("return".into(), Kind::Keyword),
                ("1".into(), Kind::Constant),
            ]
        );
        assert_eq!(state, State::Code);

        let mut state = State::Code;
        runs(&PYTHON, "doc = \"\"\"Some", &mut state);
        assert_eq!(state, State::String("\"\"\""));
        assert_eq!(
            runs(&PYTHON, "text.\"\"\" if True:", &mut state),
            [
                ("text.\"\"\"".into(), Kind::String),
                ("if".into(), Kind::Keyword),
                ("True".into(), Kind::Constant),
            ]
        );
    }

    #[test]
    fn shell_and_config() {
        let mut state = State::Code;
        assert_eq!(
            runs(&SHELL, "if [ -n \"$X\" ]; then echo $HOME # hi", &mut state),
            [
                ("if".into(), Kind::Keyword),
                ("\"$X\"".into(), Kind::String),
                ("then".into(), Kind::Keyword),
                ("$HOME".into(), Kind::Constant),
                ("# hi".into(), Kind::Comment),
            ]
        );
        assert_eq!(
            runs(&CONFIG, "name = \"gerdr\" # crate", &mut state),
            [
                ("\"gerdr\"".into(), Kind::String),
                ("# crate".into(), Kind::Comment),
            ]
        );
        // `#` inside a word isn't a comment.
        assert!(runs(&SHELL, "echo a#b", &mut state).is_empty());
    }
}

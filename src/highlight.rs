//! Code coloured by syntect's grammars (Sublime's, built in): lines read
//! in order, so a string or comment open on one goes on in the next, and
//! each run syntect names told as one of `syntax::Kind`s, which the
//! palette colours for each look (`Palette::code`). A language syntect
//! doesn't know (TypeScript, TOML, Nix, …) is read by the light
//! highlighter in `syntax.rs` instead, as are the lines past what a
//! read may spend on grammars (`GRAMMAR_LINES`).
//!
//! The grammars load once, on first use: by the threads that read diffs.

use std::{ops::Range, sync::LazyLock};

use syntect::{
    highlighting::ScopeSelectors,
    parsing::{ParseState, ScopeStack, ScopeStackOp, SyntaxReference, SyntaxSet},
};

use crate::syntax::{self, Kind, Spans};

/// How many lines one read (a diff's files, a chat's item) gives to
/// syntect: about a quarter of a second's work. Those after go to the
/// light highlighter, or show plain where it doesn't know the language.
/// shortcut: a diff's first lines get the grammars, not those in view;
/// read the rest by grammar on a thread when big diffs are found to need it.
pub const GRAMMAR_LINES: usize = 2000;
/// A line longer than this many bytes shows plain (minified code takes
/// syntect's regexes seconds); what it opens isn't followed either.
const LONG_LINE: usize = 2000;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// The kinds code is coloured as, by scope: the selector that matches a
/// run best names it, as a theme's do; `None` keeps it plain.
const SCOPES: &[(&str, Option<Kind>)] = &[
    ("comment", Some(Kind::Comment)),
    ("string, constant.character", Some(Kind::String)),
    (
        "constant, support.constant, variable.other.constant, entity.other.attribute-name",
        Some(Kind::Constant),
    ),
    // `let`, `fn`, `int` and `const` are all `storage` (One Dark draws
    // it as keywords): the grammars tell declaring words from types
    // apart too seldom for anything else.
    (
        "keyword, storage, variable.language, entity.name.tag",
        Some(Kind::Keyword),
    ),
    // `=`, `+`, `->` and their like stay plain, as One Dark draws them.
    ("keyword.operator", None),
    ("keyword.operator.word", Some(Kind::Keyword)),
    (
        "support.type, support.class, entity.name.type, entity.name.class, entity.name.struct, \
         entity.name.enum, entity.name.trait, entity.name.interface, entity.name.impl, \
         entity.other.inherited-class",
        Some(Kind::Type),
    ),
    (
        "entity.name.function, support.function, variable.function",
        Some(Kind::Function),
    ),
];

static SELECTORS: LazyLock<Vec<(ScopeSelectors, Option<Kind>)>> = LazyLock::new(|| {
    (SCOPES.iter())
        .filter_map(|(s, kind)| Some((s.parse().ok()?, *kind)))
        .collect()
});

/// The grammar for a code fence's language (`rust`, `py`, `sh`) or a
/// file's `name` (by itself, else its extension).
fn grammar(name: &str) -> Option<&'static SyntaxReference> {
    let set = &*SYNTAXES;
    let ext = name.rsplit_once('.').map(|(_, ext)| ext);
    (set.find_syntax_by_token(name))
        .or_else(|| ext.and_then(|ext| set.find_syntax_by_extension(ext)))
        .filter(|g| g.name != "Plain Text")
}

/// `lines` coloured as the language a code fence names (`rust`, `ts`) or
/// the file at a path is in, one `Spans` per line by char index, read as
/// one text from the first. Each line syntect reads takes one of
/// `budget`'s; with none left the light highlighter goes on from there.
/// `None` when neither knows the language.
pub fn lines<'a>(
    lang_or_path: &str,
    lines: impl IntoIterator<Item = &'a str>,
    budget: &mut usize,
) -> Option<Vec<Spans>> {
    let name = lang_or_path.rsplit('/').next().unwrap_or(lang_or_path);
    let light = syntax::lang(name).or_else(|| syntax::lang(&format!("f.{name}")));
    let mut read = grammar(name).map(|g| Reader {
        parse: ParseState::new(g),
        stack: ScopeStack::new(),
        failed: false,
        name: &g.name,
    });
    if read.is_none() && light.is_none() {
        return None;
    }
    let mut state = syntax::State::default();
    let mut line = |line: &str| {
        if let Some(read) = read.as_mut().filter(|r| *budget > 0 && !r.failed) {
            *budget -= 1;
            return read.line(line);
        }
        match light {
            Some(lang) => syntax::highlight(lang, line, &mut state),
            None => Spans::new(),
        }
    };
    Some(lines.into_iter().map(&mut line).collect())
}

/// One text read by a grammar, line after line.
struct Reader<'a> {
    parse: ParseState,
    stack: ScopeStack,
    /// The grammar failed on a line: the light highlighter reads the rest.
    failed: bool,
    name: &'a str,
}

impl Reader<'_> {
    fn line(&mut self, line: &str) -> Spans {
        if self.failed || line.len() > LONG_LINE {
            return Spans::new();
        }
        // The grammars match line ends: they are given theirs.
        let text = format!("{line}\n");
        let ops = match self.parse.parse_line(&text, &SYNTAXES) {
            Ok(ops) => ops,
            Err(e) => return self.fail(e),
        };
        let mut spans = Spans::new();
        // Where the last op was, by byte and by char.
        let (mut at, mut chars) = (0, 0);
        for (byte, op) in ops.iter().chain([&(line.len(), ScopeStackOp::Noop)]) {
            let byte = (*byte).min(line.len());
            if byte > at {
                let n = line[at..byte].chars().count();
                if let Some(kind) = kind(&self.stack) {
                    push(&mut spans, chars..chars + n, kind);
                }
                (at, chars) = (byte, chars + n);
            }
            if let Err(e) = self.stack.apply(op) {
                return self.fail(e);
            }
        }
        spans
    }

    fn fail(&mut self, e: impl std::fmt::Display) -> Spans {
        tracing::warn!("{} highlighting stopped: {e}", self.name);
        self.failed = true;
        Spans::new()
    }
}

/// What the run under `stack` is coloured as.
fn kind(stack: &ScopeStack) -> Option<Kind> {
    let scopes = stack.as_slice();
    (SELECTORS.iter())
        .filter_map(|(sel, kind)| Some((sel.does_match(scopes)?, *kind)))
        .max_by_key(|(power, _)| *power)
        .and_then(|(_, kind)| kind)
}

/// `range` as `kind` after `spans`, one run with the last when they meet.
fn push(spans: &mut Spans, range: Range<usize>, kind: Kind) {
    match spans.last_mut() {
        Some((last, k)) if *k == kind && last.end == range.start => last.end = range.end,
        _ => spans.push((range, kind)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` read with a whole budget.
    fn read<'a>(lang: &str, text: impl IntoIterator<Item = &'a str>) -> Option<Vec<Spans>> {
        lines(lang, text, &mut GRAMMAR_LINES.clone())
    }

    /// The kind of the run over `word` in `spans` of `line`.
    fn kind_of(line: &str, spans: &Spans, word: &str) -> Option<Kind> {
        let start = line[..line.find(word)?].chars().count();
        spans
            .iter()
            .find(|(r, _)| r.start <= start && start < r.end)
            .map(|(_, k)| *k)
    }

    #[test]
    fn every_scope_selector_parses() {
        assert_eq!(SELECTORS.len(), SCOPES.len());
    }

    #[test]
    fn rust_reads_by_its_grammar_and_a_comment_runs_over_lines() {
        let text = [
            "fn go(x: &str) -> Foo {",
            "    let n = \"hi\"; // done",
            "    /* open",
            "    still */ None",
        ];
        let spans = read("src/main.rs", text).unwrap();
        assert_eq!(kind_of(text[0], &spans[0], "fn"), Some(Kind::Keyword));
        assert_eq!(kind_of(text[0], &spans[0], "go"), Some(Kind::Function));
        assert_eq!(kind_of(text[0], &spans[0], "str"), Some(Kind::Keyword));
        assert_eq!(kind_of(text[1], &spans[1], "let"), Some(Kind::Keyword));
        assert_eq!(kind_of(text[1], &spans[1], "\"hi\""), Some(Kind::String));
        assert_eq!(kind_of(text[1], &spans[1], "// done"), Some(Kind::Comment));
        assert_eq!(kind_of(text[1], &spans[1], "="), None);
        assert_eq!(kind_of(text[3], &spans[3], "still"), Some(Kind::Comment));
        assert_ne!(kind_of(text[3], &spans[3], "None"), Some(Kind::Comment));
        let decl = "struct Foo;";
        let spans = read("rust", [decl]).unwrap();
        assert_eq!(kind_of(decl, &spans[0], "Foo"), Some(Kind::Type));
    }

    #[test]
    fn a_fence_name_finds_its_grammar_and_unknown_ones_fall_back() {
        let py = ["def f():", "    return 1  # one"];
        let spans = read("python", py).unwrap();
        assert_eq!(kind_of(py[1], &spans[1], "# one"), Some(Kind::Comment));
        // syntect has no TypeScript: the light highlighter reads it.
        let ts = ["const x: number = 1; // n"];
        let mut budget = 5;
        let spans = lines("web/app.ts", ts, &mut budget).unwrap();
        assert_eq!(kind_of(ts[0], &spans[0], "// n"), Some(Kind::Comment));
        assert_eq!(budget, 5);
        assert!(read("ts", ts).is_some());
        assert!(read("notes.unknownext", ts).is_none());
        assert!(read("notes.txt", ts).is_none());
    }

    #[test]
    fn ranges_are_by_char_and_a_spent_budget_goes_on_light() {
        let line = "let é = \"ü\";";
        let spans = read("x.rs", [line]).unwrap();
        let (range, _) = spans[0].iter().find(|(_, k)| *k == Kind::String).unwrap();
        assert_eq!(*range, 8..11);
        // One line by grammar, the next by the light highlighter.
        let mut budget = 1;
        let spans = lines("x.rs", ["/* a", "b */ let x;"], &mut budget).unwrap();
        assert_eq!(budget, 0);
        assert_eq!(spans[0], [(0..4, Kind::Comment)]);
        assert_eq!(spans[1][0].1, Kind::Keyword);
        // Haskell has a grammar only: past the budget it is plain.
        let spans = lines("x.hs", ["main = 1 -- c"], &mut 0).unwrap();
        assert!(spans[0].is_empty());
        let long = format!("let x = \"{}\";", "a".repeat(LONG_LINE));
        assert!(read("x.rs", [long.as_str()]).unwrap()[0].is_empty());
    }
}

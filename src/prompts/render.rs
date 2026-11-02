//! A small mustache: `{{var}}` (dotted names such as `feature.title`),
//! `{{> partial}}`, `{{#if var}}…{{else}}…{{/if}}` (true when set and not
//! empty; nests) and `{{ask:label}}` (an answer the user gives; `ask:label`
//! in the values). Errors name the line.
//!
//! A block tag (`{{#if}}`, `{{else}}`, `{{/if}}`) or a partial alone on its
//! line takes the whole line with it, so templates can put them on lines
//! of their own without leaving blank lines behind.

use std::collections::BTreeMap;

/// The values a template is rendered with, by dotted name.
pub type Vars = BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderError {
    /// 1-based line of the template where it went wrong.
    pub line: usize,
    pub what: String,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.what)
    }
}

/// How deep partials may include partials: past it, one includes itself.
const MAX_DEPTH: usize = 8;

/// `template` with `vars` filled in and partials from `partial`. An
/// unknown variable, partial or unanswered ask, or an unclosed `{{#if`,
/// is an error.
pub fn render(
    template: &str,
    vars: &Vars,
    partial: &dyn Fn(&str) -> Option<String>,
) -> Result<String, RenderError> {
    render_at(template, vars, partial, 0)
}

fn render_at(
    template: &str,
    vars: &Vars,
    partial: &dyn Fn(&str) -> Option<String>,
    depth: usize,
) -> Result<String, RenderError> {
    let nodes = parse(template)?;
    let mut out = String::new();
    emit(&nodes, vars, partial, depth, &mut out)?;
    Ok(out)
}

/// A tag, as written between `{{` and `}}`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tag {
    Var(String),
    Ask(String),
    Partial(String),
    If(String),
    Else,
    End,
}

impl Tag {
    /// Whether it may take its line with it.
    fn block(&self) -> bool {
        matches!(self, Tag::Partial(_) | Tag::If(_) | Tag::Else | Tag::End)
    }
}

/// A piece of the template, before blocks are nested.
#[derive(Clone, Debug)]
enum Token {
    Text(String),
    Tag {
        tag: Tag,
        line: usize,
        /// Alone on its line: the line went with it.
        alone: bool,
    },
}

/// The template as a tree.
#[derive(Clone, Debug)]
enum Node {
    Text(String),
    Var(String, usize),
    Ask(String, usize),
    Partial {
        name: String,
        line: usize,
        alone: bool,
    },
    If {
        name: String,
        then: Vec<Node>,
        other: Vec<Node>,
    },
}

/// Whether `name` is a variable's name: letters, digits, `_`, `-` and dots.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// What the inside of a tag on `line` says.
fn tag_of(inner: &str, line: usize) -> Result<Tag, RenderError> {
    let err = |what: String| RenderError { line, what };
    let inner = inner.trim();
    if let Some(name) = inner.strip_prefix('>') {
        let name = name.trim();
        return if is_name(name) {
            Ok(Tag::Partial(name.to_owned()))
        } else {
            Err(err(format!("bad partial name “{name}”")))
        };
    }
    if let Some(rest) = inner.strip_prefix('#') {
        let rest = rest.trim();
        let Some(name) = rest.strip_prefix("if ").map(str::trim) else {
            return Err(err(format!("unknown block “#{rest}”: only #if")));
        };
        return if is_name(name) {
            Ok(Tag::If(name.to_owned()))
        } else {
            Err(err(format!("bad variable name “{name}”")))
        };
    }
    if let Some(rest) = inner.strip_prefix('/') {
        return match rest.trim() {
            "if" => Ok(Tag::End),
            other => Err(err(format!("unknown end “/{other}”: only /if"))),
        };
    }
    if inner == "else" {
        return Ok(Tag::Else);
    }
    if let Some(label) = inner.strip_prefix("ask:") {
        let label = label.trim();
        return if label.is_empty() {
            Err(err("an ask without a label".into()))
        } else {
            Ok(Tag::Ask(label.to_owned()))
        };
    }
    if is_name(inner) {
        Ok(Tag::Var(inner.to_owned()))
    } else if inner.is_empty() {
        Err(err("an empty tag".into()))
    } else {
        Err(err(format!("bad variable name “{inner}”")))
    }
}

/// `template` cut into text and tags, each tag with its line.
fn tokenize(template: &str) -> Result<Vec<Token>, RenderError> {
    let mut tokens = Vec::new();
    let mut rest = template;
    let mut line = 1;
    while let Some(open) = rest.find("{{") {
        let (text, after) = rest.split_at(open);
        line += text.matches('\n').count();
        if !text.is_empty() {
            tokens.push(Token::Text(text.to_owned()));
        }
        let after = &after[2..];
        let Some(close) = after.find("}}") else {
            return Err(RenderError {
                line,
                what: "“{{” without “}}”".into(),
            });
        };
        let inner = &after[..close];
        if inner.contains('\n') {
            return Err(RenderError {
                line,
                what: "a tag over several lines".into(),
            });
        }
        tokens.push(Token::Tag {
            tag: tag_of(inner, line)?,
            line,
            alone: false,
        });
        rest = &after[close + 2..];
    }
    if !rest.is_empty() {
        tokens.push(Token::Text(rest.to_owned()));
    }
    Ok(tokens)
}

/// Let block tags alone on their line take the line with them: the blanks
/// before them on it, and the rest of it up to and with its newline.
fn strip_alone(tokens: &mut [Token]) {
    let n = tokens.len();
    // Decided on the text as written, then cut: two block lines in a row
    // share the newline between them.
    let mut alone = vec![false; n];
    for i in 0..n {
        let Token::Tag { tag, .. } = &tokens[i] else {
            continue;
        };
        if !tag.block() {
            continue;
        }
        let starts = match i.checked_sub(1).map(|j| &tokens[j]) {
            None => true,
            Some(Token::Text(t)) => match t.rfind('\n') {
                Some(at) => t[at + 1..].chars().all(|c| c == ' ' || c == '\t'),
                None => i == 1 && t.chars().all(|c| c == ' ' || c == '\t'),
            },
            Some(Token::Tag { .. }) => false,
        };
        let ends = match tokens.get(i + 1) {
            None => true,
            Some(Token::Text(t)) => {
                let head = t.split('\n').next().unwrap_or("");
                let head = head.strip_suffix('\r').unwrap_or(head);
                head.chars().all(|c| c == ' ' || c == '\t') && (t.contains('\n') || i + 2 == n)
            }
            Some(Token::Tag { .. }) => false,
        };
        alone[i] = starts && ends;
    }
    for i in 0..n {
        if !alone[i] {
            continue;
        }
        if let Token::Tag { alone: a, .. } = &mut tokens[i] {
            *a = true;
        }
        if i > 0
            && let Token::Text(t) = &mut tokens[i - 1]
        {
            let keep = t.rfind('\n').map_or(0, |at| at + 1);
            t.truncate(keep);
        }
        if let Some(Token::Text(t)) = tokens.get_mut(i + 1) {
            let cut = t.find('\n').map_or(t.len(), |at| at + 1);
            t.drain(..cut);
        }
    }
}

/// The template as a tree of blocks.
fn parse(template: &str) -> Result<Vec<Node>, RenderError> {
    let mut tokens = tokenize(template)?;
    strip_alone(&mut tokens);
    // The blocks open: the `#if`'s name and line, the nodes before it, and
    // its `then` once `else` is seen.
    struct Open {
        name: String,
        line: usize,
        outer: Vec<Node>,
        then: Option<Vec<Node>>,
    }
    let mut stack: Vec<Open> = Vec::new();
    let mut nodes = Vec::new();
    for token in tokens {
        match token {
            Token::Text(text) if text.is_empty() => {}
            Token::Text(text) => nodes.push(Node::Text(text)),
            Token::Tag { tag, line, alone } => match tag {
                Tag::Var(name) => nodes.push(Node::Var(name, line)),
                Tag::Ask(label) => nodes.push(Node::Ask(label, line)),
                Tag::Partial(name) => nodes.push(Node::Partial { name, line, alone }),
                Tag::If(name) => stack.push(Open {
                    name,
                    line,
                    outer: std::mem::take(&mut nodes),
                    then: None,
                }),
                Tag::Else => {
                    let Some(open) = stack.last_mut() else {
                        return Err(RenderError {
                            line,
                            what: "“{{else}}” outside an “{{#if}}”".into(),
                        });
                    };
                    if open.then.is_some() {
                        return Err(RenderError {
                            line,
                            what: "a second “{{else}}”".into(),
                        });
                    }
                    open.then = Some(std::mem::take(&mut nodes));
                }
                Tag::End => {
                    let Some(open) = stack.pop() else {
                        return Err(RenderError {
                            line,
                            what: "“{{/if}}” without its “{{#if}}”".into(),
                        });
                    };
                    let inner = std::mem::replace(&mut nodes, open.outer);
                    let (then, other) = match open.then {
                        Some(then) => (then, inner),
                        None => (inner, Vec::new()),
                    };
                    nodes.push(Node::If {
                        name: open.name,
                        then,
                        other,
                    });
                }
            },
        }
    }
    if let Some(open) = stack.pop() {
        return Err(RenderError {
            line: open.line,
            what: format!("“{{{{#if {}}}}}” is never closed", open.name),
        });
    }
    Ok(nodes)
}

/// Write `nodes` out into `out`.
fn emit(
    nodes: &[Node],
    vars: &Vars,
    partial: &dyn Fn(&str) -> Option<String>,
    depth: usize,
    out: &mut String,
) -> Result<(), RenderError> {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Var(name, line) => match vars.get(name) {
                Some(value) => out.push_str(value),
                None => {
                    return Err(RenderError {
                        line: *line,
                        what: format!("unknown variable “{name}”"),
                    });
                }
            },
            Node::Ask(label, line) => match vars.get(&format!("ask:{label}")) {
                Some(value) => out.push_str(value),
                None => {
                    return Err(RenderError {
                        line: *line,
                        what: format!("unanswered ask “{label}”"),
                    });
                }
            },
            Node::Partial { name, line, alone } => {
                let err = |what: String| RenderError { line: *line, what };
                if depth >= MAX_DEPTH {
                    return Err(err(format!("partial “{name}” includes itself")));
                }
                let Some(text) = partial(name) else {
                    return Err(err(format!("unknown partial “{name}”")));
                };
                let text = render_at(&text, vars, partial, depth + 1)
                    .map_err(|e| err(format!("in partial “{name}”, {e}")))?;
                out.push_str(&text);
                if *alone && !text.is_empty() && !text.ends_with('\n') {
                    out.push('\n');
                }
            }
            Node::If { name, then, other } => {
                let set = vars.get(name).is_some_and(|v| !v.trim().is_empty());
                emit(if set { then } else { other }, vars, partial, depth, out)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vars {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn none(_: &str) -> Option<String> {
        None
    }

    fn go(template: &str, pairs: &[(&str, &str)]) -> Result<String, RenderError> {
        render(template, &vars(pairs), &none)
    }

    #[test]
    fn variables_fill_in_by_dotted_name() {
        assert_eq!(
            go(
                "Hi {{ feature.title }}, {{x}}!",
                &[("feature.title", "Bulk"), ("x", "y")]
            ),
            Ok("Hi Bulk, y!".into())
        );
        assert_eq!(go("no tags\n", &[]), Ok("no tags\n".into()));
    }

    #[test]
    fn an_unknown_variable_names_its_line() {
        assert_eq!(
            go("a\nb\n  {{nope}}\n", &[]),
            Err(RenderError {
                line: 3,
                what: "unknown variable “nope”".into()
            })
        );
        assert_eq!(go("a {{", &[]).unwrap_err().line, 1);
        assert_eq!(go("\n{{#each x}}", &[]).unwrap_err().line, 2);
        assert_eq!(go("{{}}", &[]).unwrap_err().what, "an empty tag");
        assert_eq!(
            go("{{a b}}", &[]).unwrap_err().what,
            "bad variable name “a b”"
        );
    }

    #[test]
    fn ifs_nest_and_take_their_lines() {
        let t = "top\n{{#if a}}\nA\n  {{#if b}}\n  B\n  {{/if}}\n{{else}}\nnot A\n{{/if}}\nend\n";
        assert_eq!(
            go(t, &[("a", "1"), ("b", "1")]),
            Ok("top\nA\n  B\nend\n".into())
        );
        assert_eq!(go(t, &[("a", "1"), ("b", "")]), Ok("top\nA\nend\n".into()));
        assert_eq!(go(t, &[]), Ok("top\nnot A\nend\n".into()));
        // Inline, they keep the line.
        assert_eq!(
            go("x {{#if a}}yes{{else}}no{{/if}} y", &[("a", " ")]),
            Ok("x no y".into())
        );
        assert_eq!(go("{{#if a}}1{{/if}}", &[("a", "v")]), Ok("1".into()));
    }

    #[test]
    fn unclosed_and_stray_blocks_name_their_lines() {
        assert_eq!(
            go("a\n{{#if x}}\nb\n", &[]),
            Err(RenderError {
                line: 2,
                what: "“{{#if x}}” is never closed".into()
            })
        );
        assert_eq!(go("a\n\n{{/if}}", &[]).unwrap_err().line, 3);
        assert_eq!(go("{{else}}", &[]).unwrap_err().line, 1);
        assert_eq!(
            go("{{#if a}}\n{{else}}\n{{else}}\n{{/if}}", &[])
                .unwrap_err()
                .line,
            3
        );
    }

    #[test]
    fn partials_render_with_the_same_values() {
        let partial = |name: &str| match name {
            "rules" => Some("Rule for {{who}}.".to_owned()),
            "outer" => Some("[{{> rules}}]\n".to_owned()),
            "bad" => Some("ok\n{{missing}}".to_owned()),
            "me" => Some("{{> me}}".to_owned()),
            _ => None,
        };
        let v = vars(&[("who", "you")]);
        assert_eq!(
            render("a\n{{> rules}}\nb\n", &v, &partial),
            Ok("a\nRule for you.\nb\n".into())
        );
        assert_eq!(
            render("x {{> outer}}", &v, &partial),
            Ok("x [Rule for you.]\n".into())
        );
        let err = render("\n{{> bad}}", &v, &partial).unwrap_err();
        assert_eq!(err.line, 2);
        assert_eq!(
            err.what,
            "in partial “bad”, line 2: unknown variable “missing”"
        );
        assert_eq!(
            render("{{> nope}}", &v, &partial).unwrap_err().what,
            "unknown partial “nope”"
        );
        assert!(
            render("{{> me}}", &v, &partial)
                .unwrap_err()
                .what
                .ends_with("includes itself")
        );
    }

    #[test]
    fn asks_take_the_users_answers() {
        assert_eq!(
            go("Ticket: {{ask:Ticket id}}", &[("ask:Ticket id", "ABC-1")]),
            Ok("Ticket: ABC-1".into())
        );
        assert_eq!(
            go("\n{{ask:Ticket id}}", &[]),
            Err(RenderError {
                line: 2,
                what: "unanswered ask “Ticket id”".into()
            })
        );
    }
}

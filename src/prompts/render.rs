//! A small mustache: `{{var}}` (dotted names such as `feature.title`),
//! `{{> partial}}`, `{{#if var}}…{{/if}}` (true when set and not empty;
//! nests) and `{{ask:label}}` (an answer the user gives; `ask:label` in
//! the values). Errors name the line.

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

/// `template` with `vars` filled in and partials from `partial`. An
/// unknown variable, partial or unanswered ask, or an unclosed `{{#if`,
/// is an error.
pub fn render(
    template: &str,
    vars: &Vars,
    partial: &dyn Fn(&str) -> Option<String>,
) -> Result<String, RenderError> {
    todo!(
        "render {} {} {:?}",
        template.len(),
        vars.len(),
        partial("x")
    )
}

//! Prompts for the feature stages' agents: templates rendered with the
//! feature's values (see `render`). Only the built-in templates for now,
//! from `assets/prompts/` (see `builtin`); a library of the user's own
//! comes later.

#![allow(dead_code)]

pub mod render;

/// The built-in template or partial `name` ("plan", "rules"), and its
/// version: bumped when its text changes meaning.
pub fn builtin(name: &str) -> Option<(&'static str, u32)> {
    todo!("prompts: builtin {name}")
}

//! An agent gerdr doesn't know: nothing of the trait's.

use super::Integration;
use crate::ai_work::AgentKind;

pub struct Plain;

impl Integration for Plain {
    fn kind(&self) -> AgentKind {
        AgentKind::Unknown
    }

    fn label(&self) -> &'static str {
        "Unknown"
    }

    /// herdr has no integration for it to tell of.
    fn herdr_target(&self) -> Option<&'static str> {
        None
    }
}

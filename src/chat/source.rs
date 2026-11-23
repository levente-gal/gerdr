//! Which chat a pane's agent holds, and where its transcript is.

use super::Source;
use crate::{ai_work::locate::Homes, herdr::AgentInfo};

/// The chat of the agent in a pane, as herdr knows it; else why there is
/// none, said to the user.
pub fn find(agent: &AgentInfo, homes: &Homes) -> Result<Source, String> {
    let _ = (agent, homes);
    Err("Couldn't find this agent's chat".into())
}

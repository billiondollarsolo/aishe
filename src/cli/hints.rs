//! CLI surface for local discovery-hint state.

use anyhow::Result;

use crate::config::Config;

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Status { json: bool },
    Reset,
}

pub fn command(config: &Config, action: Action) -> Result<u8> {
    match action {
        Action::Status { json } => {
            let status = crate::hints::discovery_status(config)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!(
                    "discovery hints: {} · launch hint: {} · first-answer hint: {}",
                    if status.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    },
                    if status.launch_hint_seen {
                        "seen"
                    } else {
                        "not seen"
                    },
                    if status.first_answer_hint_seen {
                        "seen"
                    } else {
                        "not seen"
                    }
                );
                println!("Next: aishe hints reset");
            }
            Ok(0)
        }
        Action::Reset => {
            crate::hints::reset_discovery()?;
            println!("discovery hint seen-state reset");
            Ok(0)
        }
    }
}

//! Human-readable safety profiles and autonomous-mode readiness checks.

use serde::{Deserialize, Serialize};

use crate::capabilities::{self, State};
use crate::config::Config;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Conservative,
    Balanced,
    Autonomous,
    Custom,
}

impl Profile {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "conservative" => Some(Self::Conservative),
            "balanced" => Some(Self::Balanced),
            "autonomous" | "yolo" => Some(Self::Autonomous),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Conservative => "conservative",
            Self::Balanced => "balanced",
            Self::Autonomous => "autonomous",
            Self::Custom => "custom",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub field: &'static str,
    pub before: String,
    pub after: String,
}

pub fn apply(config: &mut Config, profile: Profile) -> Vec<Change> {
    if profile == Profile::Custom {
        let before = config.aishe.safety_profile.clone();
        config.aishe.safety_profile = "custom".into();
        return (before != "custom")
            .then_some(Change {
                field: "safety_profile",
                before,
                after: "custom".into(),
            })
            .into_iter()
            .collect();
    }
    let backend = if crate::sandbox::bwrap_available() {
        "bwrap"
    } else {
        "policy"
    };
    let (mode, confirm, plan, preview, iterations) = match profile {
        Profile::Conservative => ("suggest", "all", true, true, 10),
        Profile::Balanced => ("auto", "writes", true, true, 15),
        Profile::Autonomous => ("yolo", "dangerous", false, false, 25),
        Profile::Custom => unreachable!(),
    };
    let mut changes = Vec::new();
    set_string(
        &mut changes,
        "safety_profile",
        &mut config.aishe.safety_profile,
        profile.key(),
    );
    set_string(&mut changes, "mode", &mut config.aishe.mode, mode);
    set_string(
        &mut changes,
        "yolo_confirm",
        &mut config.aishe.yolo_confirm,
        confirm,
    );
    set_bool(&mut changes, "yolo_plan", &mut config.aishe.yolo_plan, plan);
    set_bool(
        &mut changes,
        "yolo_preview",
        &mut config.aishe.yolo_preview,
        preview,
    );
    set_bool(
        &mut changes,
        "yolo_sandbox",
        &mut config.aishe.yolo_sandbox,
        true,
    );
    set_string(
        &mut changes,
        "sandbox_backend",
        &mut config.aishe.sandbox_backend,
        backend,
    );
    if config.aishe.max_yolo_iterations != iterations {
        changes.push(Change {
            field: "max_yolo_iterations",
            before: config.aishe.max_yolo_iterations.to_string(),
            after: iterations.to_string(),
        });
        config.aishe.max_yolo_iterations = iterations;
    }
    changes
}

fn set_string(changes: &mut Vec<Change>, field: &'static str, target: &mut String, value: &str) {
    if target != value {
        changes.push(Change {
            field,
            before: target.clone(),
            after: value.to_string(),
        });
        *target = value.to_string();
    }
}

fn set_bool(changes: &mut Vec<Change>, field: &'static str, target: &mut bool, value: bool) {
    if *target != value {
        changes.push(Change {
            field,
            before: target.to_string(),
            after: value.to_string(),
        });
        *target = value;
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ReadinessCheck {
    pub id: &'static str,
    pub ready: bool,
    pub required: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Readiness {
    pub ready: bool,
    pub checks: Vec<ReadinessCheck>,
}

pub fn readiness(config: &Config) -> Readiness {
    let mut checks = Vec::new();
    let cached = capabilities::load(config);
    let tools_ready = cached
        .as_ref()
        .map(|report| report.tools.state == State::Pass)
        .unwrap_or(false);
    checks.push(ReadinessCheck {
        id: "provider_tools",
        ready: tools_ready,
        required: true,
        detail: if tools_ready {
            "tool-capable provider was validated".into()
        } else {
            "run `aishe provider test --live` to validate function tools".into()
        },
    });
    let bwrap = crate::sandbox::bwrap_available();
    checks.push(ReadinessCheck {
        id: "bubblewrap",
        ready: bwrap,
        required: true,
        detail: if bwrap {
            "bubblewrap OS isolation is available".into()
        } else {
            "install bubblewrap before treating Autonomous as fully isolated".into()
        },
    });
    checks.push(ReadinessCheck {
        id: "sandbox",
        ready: config.aishe.yolo_sandbox,
        required: true,
        detail: if config.aishe.yolo_sandbox {
            format!("sandbox enabled ({})", config.aishe.sandbox_backend)
        } else {
            "yolo sandbox is disabled".into()
        },
    });
    checks.push(ReadinessCheck {
        id: "redaction",
        ready: config.aishe.redact_secrets,
        required: true,
        detail: if config.aishe.redact_secrets {
            "secret redaction is enabled".into()
        } else {
            "secret redaction is disabled".into()
        },
    });
    checks.push(ReadinessCheck {
        id: "undo",
        ready: true,
        required: true,
        detail: "built-in file changes are journaled for `aishe undo`".into(),
    });
    checks.push(ReadinessCheck {
        id: "budget",
        ready: config.aishe.budget_usd > 0.0,
        required: false,
        detail: if config.aishe.budget_usd > 0.0 {
            format!("session budget is ${:.2}", config.aishe.budget_usd)
        } else {
            "no session budget is configured (optional)".into()
        },
    });
    let ready = checks.iter().all(|check| !check.required || check.ready);
    Readiness { ready, checks }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_apply_exact_mappings_without_touching_budget() {
        let mut config = Config::default();
        config.aishe.budget_usd = 2.5;
        apply(&mut config, Profile::Balanced);
        assert_eq!(config.aishe.mode, "auto");
        assert_eq!(config.aishe.yolo_confirm, "writes");
        assert!(config.aishe.yolo_plan);
        assert!(config.aishe.yolo_preview);
        assert!(config.aishe.yolo_sandbox);
        assert_eq!(config.aishe.max_yolo_iterations, 15);
        assert_eq!(config.aishe.budget_usd, 2.5);
    }

    #[test]
    fn custom_changes_only_profile_marker() {
        let mut config = Config::default();
        config.aishe.safety_profile = "balanced".into();
        config.aishe.mode = "auto".into();
        let changes = apply(&mut config, Profile::Custom);
        assert_eq!(changes.len(), 1);
        assert_eq!(config.aishe.mode, "auto");
    }

    #[test]
    fn readiness_requires_validated_tools() {
        let mut config = Config::default();
        config.providers.anthropic.model = format!("__aishe-readiness-test-{}", std::process::id());
        config.aishe.yolo_sandbox = true;
        config.aishe.redact_secrets = true;
        let report = readiness(&config);
        assert!(!report.ready);
        let tools = report
            .checks
            .iter()
            .find(|check| check.id == "provider_tools")
            .unwrap();
        assert!(tools.required);
        assert!(!tools.ready);
        assert!(tools.detail.contains("provider test --live"));
    }
}

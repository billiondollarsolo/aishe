use serde::{Deserialize, Serialize};

/// AIShe-owned interaction modes. Backends may request work but never elevate
/// this value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Suggest,
    Auto,
    Yolo,
}

impl Mode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "suggest" => Some(Self::Suggest),
            "auto" => Some(Self::Auto),
            "yolo" => Some(Self::Yolo),
            _ => None,
        }
    }
}

/// Authority accepted by the user for this live AIShe shell. Acceptance is
/// intentionally not serializable as a durable grant.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionScope {
    Workspace,
    Host,
}

impl ExecutionScope {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "workspace" => Some(Self::Workspace),
            "host" => Some(Self::Host),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    Deny,
    Allow,
}

impl NetworkPolicy {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "deny" => Some(Self::Deny),
            "allow" => Some(Self::Allow),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_values_parse_fail_closed() {
        assert_eq!(Mode::parse("YOLO"), Some(Mode::Yolo));
        assert_eq!(
            ExecutionScope::parse("workspace"),
            Some(ExecutionScope::Workspace)
        );
        assert_eq!(NetworkPolicy::parse("deny"), Some(NetworkPolicy::Deny));
        assert_eq!(ExecutionScope::parse("root"), None);
        assert_eq!(NetworkPolicy::parse("maybe"), None);
    }
}

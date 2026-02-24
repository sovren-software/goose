//! Hook configuration: deserialization and loading from Goose config.
//!
//! Hooks are configured under the `hooks` key in `config.yaml`:
//!
//! ```yaml
//! hooks:
//!   session_start:
//!     - command: "/path/to/hook.sh"
//!       timeout: 10
//!   pre_tool_use:
//!     - command: "/path/to/scanner.sh"
//!       timeout: 5
//!       tool_name: ".*"
//! ```

use serde::{Deserialize, Serialize};

use crate::config::Config;

/// Default timeout for hook execution in seconds.
const DEFAULT_TIMEOUT: u64 = 10;

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT
}

/// A single hook entry specifying a command and its configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntry {
    /// Shell command to execute. Parsed via `shlex` for proper quoting.
    pub command: String,

    /// Timeout in seconds before the hook process is killed.
    #[serde(default = "default_timeout")]
    pub timeout: u64,

    /// Optional regex filter for tool name (pre_tool_use and post_tool_use only).
    /// When set, the hook only fires for tool calls matching this pattern.
    #[serde(default)]
    pub tool_name: Option<String>,
}

/// Top-level hooks configuration mapping event types to hook entries.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HooksConfig {
    #[serde(default)]
    pub session_start: Vec<HookEntry>,

    #[serde(default)]
    pub prompt_submit: Vec<HookEntry>,

    #[serde(default)]
    pub pre_tool_use: Vec<HookEntry>,

    #[serde(default)]
    pub post_tool_use: Vec<HookEntry>,

    #[serde(default)]
    pub session_stop: Vec<HookEntry>,
}

impl HooksConfig {
    /// Returns true if any hooks are configured for any event.
    pub fn has_any_hooks(&self) -> bool {
        !self.session_start.is_empty()
            || !self.prompt_submit.is_empty()
            || !self.pre_tool_use.is_empty()
            || !self.post_tool_use.is_empty()
            || !self.session_stop.is_empty()
    }
}

/// Load hooks configuration from the global Goose config.
///
/// Falls back to an empty config if no hooks are configured or if
/// deserialization fails, ensuring hooks never break normal operation.
pub fn load_hooks() -> HooksConfig {
    Config::global()
        .get_param::<HooksConfig>("hooks")
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hooks_config_deserialize() {
        let yaml = r#"
session_start:
  - command: "/usr/local/bin/start-hook.sh"
    timeout: 15
pre_tool_use:
  - command: "/usr/local/bin/scanner.sh"
    timeout: 5
    tool_name: "write_file"
post_tool_use:
  - command: "/usr/local/bin/logger.sh"
"#;
        let config: HooksConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.session_start.len(), 1);
        assert_eq!(config.session_start[0].timeout, 15);
        assert_eq!(config.pre_tool_use.len(), 1);
        assert_eq!(
            config.pre_tool_use[0].tool_name.as_deref(),
            Some("write_file")
        );
        assert_eq!(config.post_tool_use.len(), 1);
        assert_eq!(config.post_tool_use[0].timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn test_hooks_config_empty() {
        let yaml = "{}";
        let config: HooksConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.has_any_hooks());
        assert!(config.session_start.is_empty());
        assert!(config.pre_tool_use.is_empty());
    }

    #[test]
    fn test_hooks_config_default_timeout() {
        let yaml = r#"
session_start:
  - command: "my-hook"
"#;
        let config: HooksConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.session_start[0].timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn test_hooks_config_has_any_hooks() {
        let mut config = HooksConfig::default();
        assert!(!config.has_any_hooks());

        config.session_start.push(HookEntry {
            command: "test".to_string(),
            timeout: 10,
            tool_name: None,
        });
        assert!(config.has_any_hooks());
    }
}

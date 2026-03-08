use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Merged hook settings from global + project config.
#[derive(Debug, Clone, Default)]
pub struct HooksConfig {
    pub hooks: HashMap<String, Vec<HookEventConfig>>,
    pub allow_project_hooks: bool,
}

impl<'de> serde::Deserialize<'de> for HooksConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            hooks: HashMap<String, Vec<HookEventConfig>>,
            #[serde(default)]
            allow_project_hooks: bool,
        }

        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            hooks: raw.hooks,
            allow_project_hooks: raw.allow_project_hooks,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookEventConfig {
    #[serde(default)]
    pub matcher: Option<String>,

    #[serde(deserialize_with = "deserialize_hooks_skip_unknown")]
    pub hooks: Vec<HookAction>,
}

fn deserialize_hooks_skip_unknown<'de, D>(deserializer: D) -> Result<Vec<HookAction>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Vec<serde_json::Value> = Vec::deserialize(deserializer)?;
    let mut actions = Vec::new();
    for value in raw {
        match value.get("type").and_then(|t| t.as_str()) {
            Some("command") => match serde_json::from_value(value) {
                Ok(action) => actions.push(action),
                Err(e) => {
                    tracing::warn!("Invalid hook action config: {}", e);
                }
            },
            Some(other) => {
                tracing::debug!("Unsupported hook action type '{}', skipping", other);
            }
            None => {
                tracing::warn!("Hook action missing 'type' field, skipping");
            }
        }
    }
    Ok(actions)
}

/// Only command actions are supported. MCP tool routing was removed
/// in the HookRuntime re-architecture.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HookAction {
    Command {
        command: String,

        #[serde(default = "default_timeout")]
        timeout: u64,
    },
}

fn default_timeout() -> u64 {
    600
}

impl HooksConfig {
    /// Load merged config from global (~/.config/goose/hooks.json) and
    /// project (.goose/settings.json or .claude/settings.json).
    pub fn load_merged(working_dir: &Path) -> Result<Self> {
        let global_path = crate::config::paths::Paths::in_config_dir("hooks.json");
        let goose_project_path = working_dir.join(".goose").join("settings.json");
        let claude_project_path = working_dir.join(".claude").join("settings.json");

        let global = Self::load_from_file(&global_path).unwrap_or_else(|e| {
            tracing::debug!("No global hooks config at {:?}: {}", global_path, e);
            Self::default()
        });

        let allow_project_hooks = global.allow_project_hooks;

        if !allow_project_hooks {
            let project_path = if goose_project_path.exists() {
                Some(&goose_project_path)
            } else if claude_project_path.exists() {
                Some(&claude_project_path)
            } else {
                None
            };

            if let Some(path) = project_path {
                tracing::info!(
                    "Project hooks found at {:?} but project hooks are not enabled. \
                     Set allow_project_hooks: true in ~/.config/goose/hooks.json to enable.",
                    path
                );
            }

            return Ok(global);
        }

        let project = if goose_project_path.exists() {
            if claude_project_path.exists() {
                tracing::warn!("Found hooks config in both .goose/ and .claude/; using .goose/");
            }
            Self::load_from_file(&goose_project_path).unwrap_or_else(|e| {
                tracing::warn!(
                    "Failed to parse hooks config {:?}: {}",
                    goose_project_path,
                    e
                );
                Self::default()
            })
        } else {
            Self::load_from_file(&claude_project_path).unwrap_or_else(|e| {
                if claude_project_path.exists() {
                    tracing::warn!(
                        "Failed to parse hooks config {:?}: {}",
                        claude_project_path,
                        e
                    );
                }
                Self::default()
            })
        };

        Ok(Self::merge(global, project))
    }

    fn load_from_file(path: &Path) -> Result<Self> {
        if !path.exists() {
            anyhow::bail!("Config file does not exist: {:?}", path);
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read hooks config from {:?}", path))?;

        let config: Self = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse hooks config from {:?}", path))?;

        Ok(config)
    }

    fn merge(global: Self, project: Self) -> Self {
        let mut merged_hooks = global.hooks;

        for (event, project_configs) in project.hooks {
            merged_hooks
                .entry(event)
                .or_default()
                .extend(project_configs);
        }

        Self {
            hooks: merged_hooks,
            allow_project_hooks: global.allow_project_hooks,
        }
    }

    pub fn get_hooks_for_event(&self, event_kind: &str) -> &[HookEventConfig] {
        self.hooks
            .get(event_kind)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

use phi_proto::{RateLimits, SpaceSettings, SummaryMode, Validate};
use serde::Deserialize;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub data_dir: PathBuf,
    pub listen: SocketAddr,
    pub allow_space_creation: Creation,
    pub admin_token: Option<String>,
    pub tls: Tls,
    pub limits: Limits,
    pub defaults: Defaults,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Creation {
    Anyone,
    Token,
}
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tls {
    pub mode: TlsMode,
    pub domain: Option<String>,
    pub email: Option<String>,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    Off,
    Acme,
}
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_blob_bytes: u64,
    pub space_quota_bytes: u64,
    pub messages_per_min: u32,
    pub messages_per_hour: u32,
}
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub retention_days: u32,
    pub summary_mode: SummaryMode,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: "data".into(),
            listen: SocketAddr::from(([127, 0, 0, 1], 8443)),
            allow_space_creation: Creation::Token,
            admin_token: None,
            tls: Tls::default(),
            limits: Limits::default(),
            defaults: Defaults::default(),
        }
    }
}
impl Default for Tls {
    fn default() -> Self {
        Self {
            mode: TlsMode::Off,
            domain: None,
            email: None,
        }
    }
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_blob_bytes: phi_proto::DEFAULT_MAX_ATTACHMENT_BYTES,
            space_quota_bytes: phi_proto::DEFAULT_SPACE_QUOTA_BYTES,
            messages_per_min: 30,
            messages_per_hour: 600,
        }
    }
}
impl Default for Defaults {
    fn default() -> Self {
        Self {
            retention_days: 14,
            summary_mode: SummaryMode::Required,
        }
    }
}
impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut value: toml::Value = toml::from_str(&std::fs::read_to_string(path)?)?;
        // Nested overrides use double underscores, matching TOML table boundaries.
        for (name, raw) in std::env::vars()
            .filter(|(k, _)| k.starts_with("PHI_SERVER_") && k != "PHI_SERVER_CONFIG")
        {
            let parts: Vec<_> = name
                .trim_start_matches("PHI_SERVER_")
                .split("__")
                .map(str::to_lowercase)
                .collect();
            let mut table = value
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("expected config table"))?;
            for part in &parts[..parts.len() - 1] {
                table = table
                    .entry(part)
                    .or_insert_with(|| toml::Value::Table(Default::default()))
                    .as_table_mut()
                    .ok_or_else(|| anyhow::anyhow!("invalid override {name}"))?;
            }
            let numeric = matches!(
                parts.last().map(String::as_str),
                Some(
                    "max_blob_bytes"
                        | "space_quota_bytes"
                        | "messages_per_min"
                        | "messages_per_hour"
                        | "retention_days"
                )
            );
            let parsed = if numeric {
                toml::Value::Integer(raw.parse()?)
            } else {
                toml::Value::String(raw)
            };
            table.insert(parts[parts.len() - 1].clone(), parsed);
        }
        let config: Self = value.try_into()?;
        config.validate()?;
        Ok(config)
    }
    pub fn settings(&self) -> SpaceSettings {
        SpaceSettings {
            retention_days: self.defaults.retention_days,
            summary_mode: self.defaults.summary_mode,
            summary_bytes: SpaceSettings::default().summary_bytes,
            rate_limits: RateLimits {
                messages_per_minute: self.limits.messages_per_min,
                messages_per_hour: self.limits.messages_per_hour,
            },
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        self.settings().validate()?;
        anyhow::ensure!(
            self.limits.max_blob_bytes > 0
                && self.limits.max_blob_bytes <= (usize::MAX as u64 / 2)
                && self.limits.space_quota_bytes > 0,
            "invalid blob limits"
        );
        anyhow::ensure!(
            self.allow_space_creation != Creation::Token
                || self
                    .admin_token
                    .as_ref()
                    .is_some_and(|t| !t.trim().is_empty()),
            "token creation requires admin_token"
        );
        if self.tls.mode == TlsMode::Acme {
            anyhow::ensure!(
                self.tls.domain.as_ref().is_some_and(|d| !d.is_empty())
                    && self.tls.email.as_ref().is_some_and(|e| !e.is_empty()),
                "acme requires domain and email"
            );
        }
        Ok(())
    }
}

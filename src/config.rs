use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct SiteConfig {
    pub organization: Option<String>,
    pub auth_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    /// Default site to use when -s is not specified
    #[serde(default)]
    pub default_site: Option<String>,
    /// Site-specific configurations
    #[serde(default)]
    pub sites: HashMap<String, SiteConfig>,
    /// Legacy fields for backward compatibility
    #[serde(default)]
    pub organization: Option<String>,
    #[serde(default)]
    pub auth_token: Option<String>,
}

impl Config {
    /// Get config for a specific site, or the default/legacy config
    pub fn get_site(&self, site: Option<&str>) -> SiteConfig {
        // If site specified, look it up
        if let Some(s) = site {
            if let Some(cfg) = self.sites.get(s) {
                return cfg.clone();
            }
        }

        // Try default site
        if let Some(default) = &self.default_site {
            if let Some(cfg) = self.sites.get(default) {
                return cfg.clone();
            }
        }

        // Fall back to legacy top-level config
        SiteConfig {
            organization: self.organization.clone(),
            auth_token: self.auth_token.clone(),
        }
    }

    /// Set config for a specific site
    pub fn set_site(&mut self, site: &str, org: Option<String>, token: Option<String>) {
        let entry = self.sites.entry(site.to_string()).or_default();
        if let Some(o) = org {
            entry.organization = Some(o);
        }
        if let Some(t) = token {
            entry.auth_token = Some(t);
        }
    }

    /// List all configured sites
    pub fn list_sites(&self) -> Vec<&str> {
        self.sites.keys().map(|s| s.as_str()).collect()
    }
}

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("sentry-cli-rs")
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

fn sentryclirc_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sentryclirc")
}

/// Load config from our config file, falling back to ~/.sentryclirc
pub fn load_config() -> Result<Config> {
    let path = config_path();
    if path.exists() {
        let content = fs::read_to_string(&path)?;
        return Ok(serde_json::from_str(&content)?);
    }

    // Fall back to ~/.sentryclirc (INI format with [auth] and [defaults] sections)
    let sentryclirc = sentryclirc_path();
    if sentryclirc.exists() {
        let content = fs::read_to_string(&sentryclirc)?;
        let mut config = Config::default();
        let mut in_auth_section = false;
        let mut in_defaults_section = false;

        for line in content.lines() {
            let line = line.trim();

            // Track sections
            if line.starts_with('[') && line.ends_with(']') {
                in_auth_section = line == "[auth]";
                in_defaults_section = line == "[defaults]";
                continue;
            }

            // Parse token from [auth] section
            if in_auth_section && (line.starts_with("token=") || line.starts_with("token =")) {
                let token = line.splitn(2, '=').nth(1).map(|s| s.trim().to_string());
                config.auth_token = token;
            }

            // Parse org from [defaults] section
            if in_defaults_section && (line.starts_with("org=") || line.starts_with("org =")) {
                let org = line.splitn(2, '=').nth(1).map(|s| s.trim().to_string());
                config.organization = org;
            }
        }

        return Ok(config);
    }

    Ok(Config::default())
}

pub fn save_config(config: &Config) -> Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    let path = config_path();
    fs::write(&path, serde_json::to_string_pretty(config)?)?;
    Ok(())
}

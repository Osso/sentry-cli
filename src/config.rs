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
        .join("sentry")
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

fn sentryclirc_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sentryclirc")
}

fn parse_ini_value(line: &str, key: &str) -> Option<String> {
    let (parsed_key, value) = line.split_once('=')?;
    (parsed_key.trim() == key).then(|| value.trim().to_string())
}

fn is_section_header(line: &str) -> bool {
    line.starts_with('[') && line.ends_with(']')
}

fn parse_sentryclirc(content: &str) -> Config {
    let mut config = Config::default();
    let mut in_auth_section = false;
    let mut in_defaults_section = false;

    for raw_line in content.lines() {
        let line = raw_line.trim();

        if is_section_header(line) {
            in_auth_section = line == "[auth]";
            in_defaults_section = line == "[defaults]";
            continue;
        }

        if in_auth_section {
            if let Some(token) = parse_ini_value(line, "token") {
                config.auth_token = Some(token);
            }
        }

        if in_defaults_section {
            if let Some(org) = parse_ini_value(line, "org") {
                config.organization = Some(org);
            }
        }
    }

    config
}

fn load_from_json_config(path: &PathBuf) -> Result<Config> {
    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

fn load_from_sentryclirc(path: &PathBuf) -> Result<Option<Config>> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)?;
    Ok(Some(parse_sentryclirc(&content)))
}

/// Load config from our config file, falling back to ~/.sentryclirc
pub fn load_config() -> Result<Config> {
    let path = config_path();
    if path.exists() {
        return load_from_json_config(&path);
    }

    let sentryclirc = sentryclirc_path();
    if let Some(config) = load_from_sentryclirc(&sentryclirc)? {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct HomeGuard {
        xdg_previous: Option<String>,
        home_previous: Option<String>,
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            restore_env("XDG_CONFIG_HOME", &self.xdg_previous);
            restore_env("HOME", &self.home_previous);
        }
    }

    fn restore_env(name: &str, previous: &Option<String>) {
        unsafe {
            if let Some(value) = previous {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }

    fn with_home(test: impl FnOnce(&std::path::Path)) {
        let _lock = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let path = std::env::temp_dir().join(format!(
            "sentry-config-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let guard = HomeGuard {
            xdg_previous: std::env::var("XDG_CONFIG_HOME").ok(),
            home_previous: std::env::var("HOME").ok(),
        };
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &path);
            std::env::set_var("HOME", &path);
        }

        test(&path);

        drop(guard);
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn site_config_prefers_explicit_then_default_then_legacy() {
        let mut config = Config {
            default_site: Some("gc".to_string()),
            organization: Some("legacy-org".to_string()),
            auth_token: Some("legacy-token".to_string()),
            ..Default::default()
        };
        config.set_site(
            "gc",
            Some("gc-org".to_string()),
            Some("gc-token".to_string()),
        );
        config.set_site("mh", Some("mh-org".to_string()), None);

        assert_eq!(
            config.get_site(Some("mh")).organization.as_deref(),
            Some("mh-org")
        );
        assert_eq!(
            config.get_site(None).auth_token.as_deref(),
            Some("gc-token")
        );
        assert_eq!(
            Config {
                organization: Some("legacy-org".to_string()),
                auth_token: Some("legacy-token".to_string()),
                ..Default::default()
            }
            .get_site(Some("missing"))
            .organization
            .as_deref(),
            Some("legacy-org")
        );
        let mut sites = config.list_sites();
        sites.sort_unstable();
        assert_eq!(sites, vec!["gc", "mh"]);
    }

    #[test]
    fn parses_sentryclirc_auth_and_defaults_sections() {
        let parsed = parse_sentryclirc(
            r#"
                [auth]
                token = abc123
                [defaults]
                org = globalcomix
                [other]
                token = ignored
            "#,
        );

        assert_eq!(parsed.auth_token.as_deref(), Some("abc123"));
        assert_eq!(parsed.organization.as_deref(), Some("globalcomix"));
        assert_eq!(
            parse_ini_value("token = abc", "token"),
            Some("abc".to_string())
        );
        assert_eq!(parse_ini_value("org = abc", "token"), None);
        assert!(is_section_header("[auth]"));
        assert!(!is_section_header("token = abc"));
    }

    #[test]
    fn load_prefers_json_config_then_sentryclirc_then_default() {
        with_home(|root| {
            let default_config = load_config().unwrap();
            assert!(default_config.auth_token.is_none());

            std::fs::write(
                root.join(".sentryclirc"),
                "[auth]\ntoken = from-rc\n[defaults]\norg = rc-org\n",
            )
            .unwrap();
            let rc_config = load_config().unwrap();
            assert_eq!(rc_config.auth_token.as_deref(), Some("from-rc"));
            assert_eq!(rc_config.organization.as_deref(), Some("rc-org"));

            let json_config = Config {
                organization: Some("json-org".to_string()),
                auth_token: Some("json-token".to_string()),
                ..Default::default()
            };
            save_config(&json_config).unwrap();
            let loaded_json = load_config().unwrap();
            assert_eq!(loaded_json.organization.as_deref(), Some("json-org"));
            assert!(
                load_from_sentryclirc(&root.join("missing"))
                    .unwrap()
                    .is_none()
            );
        });
    }
}

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_max_open_tenants")]
    pub max_open_tenants: usize,
    #[serde(default)]
    pub auth: AuthConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AuthConfig {
    #[serde(default = "default_auth_mode")]
    pub mode: String,
    #[serde(default)]
    pub dev_token: String,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("./data")
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_max_open_tenants() -> usize {
    256
}

fn default_auth_mode() -> String {
    "stub".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            data_dir: default_data_dir(),
            log_level: default_log_level(),
            max_open_tenants: default_max_open_tenants(),
            auth: AuthConfig::default(),
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: default_auth_mode(),
            dev_token: String::new(),
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        match path {
            Some(path) => {
                let content = std::fs::read_to_string(path)?;
                let config: Config = toml::from_str(&content)?;
                Ok(config)
            }
            None => Ok(Config::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.data_dir, PathBuf::from("./data"));
        assert_eq!(config.log_level, "info");
        assert_eq!(config.max_open_tenants, 256);
        assert_eq!(config.auth.mode, "stub");
    }

    #[test]
    fn test_load_partial_config() -> anyhow::Result<()> {
        let mut file = NamedTempFile::new()?;
        writeln!(file, "port = 9090")?;
        
        let config = Config::load(Some(file.path()))?;
        assert_eq!(config.port, 9090);
        assert_eq!(config.host, "127.0.0.1"); // remains default
        Ok(())
    }

    #[test]
    fn test_load_empty_config() -> anyhow::Result<()> {
        let file = NamedTempFile::new()?;
        let config = Config::load(Some(file.path()))?;
        assert_eq!(config.port, 8080);
        Ok(())
    }

    #[test]
    fn test_load_invalid_toml() -> anyhow::Result<()> {
        let mut file = NamedTempFile::new()?;
        writeln!(file, "port = \"not a number\"")?;
        
        let result = Config::load(Some(file.path()));
        assert!(result.is_err());
        Ok(())
    }
}

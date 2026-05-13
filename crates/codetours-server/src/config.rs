use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
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
    #[serde(default = "default_max_pack_size")]
    pub max_pack_size: usize,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub auth: AuthConfig,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    #[serde(default = "default_pool_size")]
    pub pool_size: u32,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    #[serde(default = "default_auth_mode")]
    pub mode: String,
    #[serde(default)]
    pub dev_token: String,
    #[serde(default)]
    pub github: GithubAuthConfig,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct GithubAuthConfig {
    /// GitHub OAuth client ID
    #[serde(default)]
    pub client_id: String,
    /// GitHub OAuth client secret
    #[serde(default)]
    pub client_secret: String,
    /// OAuth redirect URI (e.g., http://localhost:8080/auth/github/callback)
    #[serde(default = "default_callback_url")]
    pub callback_url: String,
    /// JWT secret for signing session tokens
    #[serde(default)]
    pub jwt_secret: String,
    /// Session token expiration in seconds (default: 7 days)
    #[serde(default = "default_session_expires")]
    pub session_expires_in: u64,
}

impl GithubAuthConfig {
    /// Get client_id, prioritizing environment variable
    pub fn get_client_id(&self) -> String {
        std::env::var("GITHUB_CLIENT_ID").unwrap_or_else(|_| self.client_id.clone())
    }

    /// Get client_secret, prioritizing environment variable
    pub fn get_client_secret(&self) -> String {
        std::env::var("GITHUB_CLIENT_SECRET").unwrap_or_else(|_| self.client_secret.clone())
    }

    /// Get jwt_secret, prioritizing environment variable
    pub fn get_jwt_secret(&self) -> String {
        std::env::var("JWT_SECRET").unwrap_or_else(|_| self.jwt_secret.clone())
    }
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

fn default_max_pack_size() -> usize {
    5 * 1024 * 1024 // 5MB
}

fn default_pool_size() -> u32 {
    8
}

fn default_auth_mode() -> String {
    "stub".to_string()
}

fn default_callback_url() -> String {
    "http://localhost:8080/auth/github/callback".to_string()
}

fn default_session_expires() -> u64 {
    7 * 24 * 60 * 60 // 7 days
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            data_dir: default_data_dir(),
            log_level: default_log_level(),
            max_open_tenants: default_max_open_tenants(),
            max_pack_size: default_max_pack_size(),
            storage: StorageConfig::default(),
            auth: AuthConfig::default(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self { pool_size: default_pool_size() }
    }
}

impl Default for GithubAuthConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            callback_url: default_callback_url(),
            jwt_secret: String::new(),
            session_expires_in: default_session_expires(),
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: default_auth_mode(),
            dev_token: String::new(),
            github: GithubAuthConfig::default(),
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
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.data_dir, PathBuf::from("./data"));
        assert_eq!(config.log_level, "info");
        assert_eq!(config.max_open_tenants, 256);
        assert_eq!(config.storage.pool_size, 8);
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

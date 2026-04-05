use std::collections::HashMap;
use std::time::Duration;

// Target: bookmark an enum with string-like variants
#[derive(Debug, Clone, Copy)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

// Target: bookmark a struct with derive macros
#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub base_url: String,
    pub timeout: Duration,
    pub headers: HashMap<String, String>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        ApiConfig {
            base_url: "https://api.example.com".to_string(),
            timeout: Duration::from_secs(30),
            headers: HashMap::new(),
        }
    }
}

#[derive(Debug)]
pub struct ApiRequest {
    pub method: HttpMethod,
    pub path: String,
    pub body: Option<String>,
}

#[derive(Debug)]
pub struct ApiResponse {
    pub status: u16,
    pub body: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("request failed: {0}")]
    RequestFailed(String),
    #[error("timeout after {0:?}")]
    Timeout(Duration),
    #[error("auth error: {0}")]
    Auth(String),
}

// Target: bookmark the main struct
pub struct ApiClient {
    config: ApiConfig,
    auth_token: Option<String>,
}

impl ApiClient {
    pub fn new(config: ApiConfig) -> Self {
        ApiClient {
            config,
            auth_token: None,
        }
    }

    pub fn with_auth(mut self, token: String) -> Self {
        self.auth_token = Some(token);
        self
    }

    // Target: bookmark the main request method
    pub fn send(&self, request: &ApiRequest) -> Result<ApiResponse, ApiError> {
        let url = format!("{}{}", self.config.base_url, request.path);
        // Simulate request
        if url.contains("error") {
            return Err(ApiError::RequestFailed(format!("failed: {url}")));
        }
        Ok(ApiResponse {
            status: 200,
            body: format!("{{\"url\": \"{url}\"}}"),
        })
    }

    // Target: bookmark a private retry method
    fn send_with_retry(
        &self,
        request: &ApiRequest,
        max_attempts: u32,
    ) -> Result<ApiResponse, ApiError> {
        let mut last_err = None;
        for _ in 0..max_attempts {
            match self.send(request) {
                Ok(resp) => return Ok(resp),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or(ApiError::RequestFailed("unknown".into())))
    }

    // Target: bookmark a generic method
    pub fn send_and_decode<T: serde::de::DeserializeOwned>(
        &self,
        request: &ApiRequest,
    ) -> Result<T, ApiError> {
        let response = self.send(request)?;
        serde_json::from_str(&response.body)
            .map_err(|e| ApiError::RequestFailed(format!("decode error: {e}")))
    }
}

// Target: bookmark a trait implementation for Display
impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpMethod::Get => write!(f, "GET"),
            HttpMethod::Post => write!(f, "POST"),
            HttpMethod::Put => write!(f, "PUT"),
            HttpMethod::Delete => write!(f, "DELETE"),
        }
    }
}

// Target: bookmark convenience functions
pub fn get(client: &ApiClient, path: &str) -> Result<ApiResponse, ApiError> {
    client.send(&ApiRequest {
        method: HttpMethod::Get,
        path: path.to_string(),
        body: None,
    })
}

pub fn post(client: &ApiClient, path: &str, body: &str) -> Result<ApiResponse, ApiError> {
    client.send(&ApiRequest {
        method: HttpMethod::Post,
        path: path.to_string(),
        body: Some(body.to_string()),
    })
}

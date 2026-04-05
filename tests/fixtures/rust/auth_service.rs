use std::collections::HashMap;
use std::time::{Duration, SystemTime};

// Target: bookmark a struct declaration
#[derive(Debug, Clone)]
pub struct Claims {
    pub subject: String,
    pub expiry: SystemTime,
    pub roles: Vec<String>,
}

// Target: bookmark an enum with variants
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid token")]
    InvalidToken,
    #[error("token expired")]
    Expired,
    #[error("insufficient permissions: requires {required}")]
    InsufficientPermissions { required: String },
}

// Target: bookmark a trait declaration
pub trait AuthProvider {
    fn validate_token(&self, token: &str) -> Result<Claims, AuthError>;
    fn refresh_token(&self, token: &str) -> Result<String, AuthError>;
}

// Target: bookmark a struct with fields
pub struct AuthService {
    secret: String,
    issuer: String,
    token_cache: HashMap<String, Claims>,
}

// Target: bookmark the impl block constructor
impl AuthService {
    pub fn new(secret: String, issuer: String) -> Self {
        AuthService {
            secret,
            issuer,
            token_cache: HashMap::new(),
        }
    }

    // Target: bookmark a private method inside impl
    fn decode(&self, token: &str) -> Result<Claims, AuthError> {
        if token.is_empty() {
            return Err(AuthError::InvalidToken);
        }
        // Simulated decode
        Ok(Claims {
            subject: "user-1".to_string(),
            expiry: SystemTime::now() + Duration::from_secs(3600),
            roles: vec!["admin".to_string()],
        })
    }

    fn encode(&self, claims: &Claims) -> Result<String, AuthError> {
        Ok(format!("{}:{}:{}", self.issuer, claims.subject, self.secret))
    }

    // Target: bookmark a method with complex parameters
    pub fn check_permission(
        &self,
        claims: &Claims,
        required: &str,
        resource: &str,
    ) -> Result<(), AuthError> {
        if !claims.roles.contains(&required.to_string()) {
            return Err(AuthError::InsufficientPermissions {
                required: format!("{required} on {resource}"),
            });
        }
        Ok(())
    }
}

// Target: bookmark a trait implementation
impl AuthProvider for AuthService {
    fn validate_token(&self, token: &str) -> Result<Claims, AuthError> {
        if let Some(cached) = self.token_cache.get(token) {
            if cached.expiry > SystemTime::now() {
                return Ok(cached.clone());
            }
        }
        let claims = self.decode(token)?;
        if claims.expiry <= SystemTime::now() {
            return Err(AuthError::Expired);
        }
        Ok(claims)
    }

    fn refresh_token(&self, token: &str) -> Result<String, AuthError> {
        let claims = self.decode(token)?;
        if claims.expiry
            <= SystemTime::now()
                .checked_sub(Duration::from_secs(3600))
                .unwrap_or(SystemTime::UNIX_EPOCH)
        {
            return Err(AuthError::Expired);
        }
        self.encode(&claims)
    }
}

// Target: bookmark a free function
pub fn create_default_auth_service() -> AuthService {
    AuthService::new("default-secret".to_string(), "codemark".to_string())
}

// Target: bookmark a generic function
pub fn validate_and_check<P: AuthProvider>(
    provider: &P,
    token: &str,
    required_role: &str,
) -> Result<Claims, AuthError> {
    let claims = provider.validate_token(token)?;
    if !claims.roles.contains(&required_role.to_string()) {
        return Err(AuthError::InsufficientPermissions {
            required: required_role.to_string(),
        });
    }
    Ok(claims)
}

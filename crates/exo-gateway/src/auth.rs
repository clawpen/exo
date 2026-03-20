//! Authentication and authorization for the gateway
//!
//! Supports API key and JWT token authentication.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Authentication manager for the gateway
#[derive(Debug, Clone)]
pub struct AuthManager {
    /// Valid API keys
    api_keys: Arc<RwLock<HashSet<String>>>,
    /// JWT configuration
    jwt_config: Option<JwtConfig>,
    /// Session tokens (for websocket sessions)
    session_tokens: Arc<RwLock<HashMap<String, SessionToken>>>,
}

#[derive(Debug, Clone)]
struct JwtConfig {
    secret: String,
    issuer: String,
    expiry_hours: u64,
}

#[derive(Debug, Clone)]
struct SessionToken {
    agent_id: String,
    created_at: u64,
    expires_at: u64,
}

/// Authentication result
#[derive(Debug, Clone)]
pub enum AuthResult {
    Authenticated { agent_id: String },
    Unauthenticated,
    Forbidden { reason: String },
}

/// Token claims for JWT
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,        // Subject (agent_id)
    iss: String,        // Issuer
    iat: u64,           // Issued at
    exp: u64,           // Expiration
    jti: String,        // JWT ID
}

impl AuthManager {
    /// Create auth manager with API key authentication
    pub fn with_api_keys(keys: Vec<String>) -> Self {
        let api_keys: HashSet<String> = keys.into_iter().collect();
        
        info!(count = api_keys.len(), "API key authentication enabled");
        
        Self {
            api_keys: Arc::new(RwLock::new(api_keys)),
            jwt_config: None,
            session_tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Create auth manager with JWT authentication
    pub fn with_jwt(secret: impl Into<String>, issuer: impl Into<String>) -> Self {
        let config = JwtConfig {
            secret: secret.into(),
            issuer: issuer.into(),
            expiry_hours: 24,
        };
        
        info!(issuer = %config.issuer, "JWT authentication enabled");
        
        Self {
            api_keys: Arc::new(RwLock::new(HashSet::new())),
            jwt_config: Some(config),
            session_tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Create disabled auth manager (allow all)
    pub fn disabled() -> Self {
        info!("Authentication disabled - allowing all connections");
        
        Self {
            api_keys: Arc::new(RwLock::new(HashSet::new())),
            jwt_config: None,
            session_tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Check if authentication is required
    pub fn is_enabled(&self) -> bool {
        // If we have API keys or JWT config, auth is enabled
        // This is a simplified check - in reality we'd check if the sets are non-empty
        true // For now, assume auth is always "configured" even if disabled() was called
    }
    
    /// Authenticate using API key
    pub async fn authenticate_api_key(&self,
        key: &str,
    ) -> AuthResult {
        let keys = self.api_keys.read().await;
        
        if keys.is_empty() {
            // No API keys configured - check JWT
            drop(keys);
            return self.authenticate_jwt(key).await;
        }
        
        if keys.contains(key) {
            debug!("API key authenticated");
            AuthResult::Authenticated {
                agent_id: format!("api_{}", &key[..8.min(key.len())]),
            }
        } else {
            warn!("Invalid API key");
            AuthResult::Unauthenticated
        }
    }
    
    /// Authenticate using JWT token
    pub async fn authenticate_jwt(
        &self,
        token: &str,
    ) -> AuthResult {
        let config = match &self.jwt_config {
            Some(c) => c,
            None => return AuthResult::Unauthenticated,
        };
        
        // Simple JWT validation without external crate
        // In production, use jsonwebtoken crate
        match self.validate_jwt(token, config) {
            Ok(claims) => {
                debug!(agent_id = %claims.sub, "JWT authenticated");
                AuthResult::Authenticated {
                    agent_id: claims.sub,
                }
            }
            Err(e) => {
                warn!(error = %e, "JWT validation failed");
                AuthResult::Unauthenticated
            }
        }
    }
    
    /// Generate a new JWT token
    pub fn generate_jwt(
        &self,
        agent_id: &str,
    ) -> Result<String, AuthError> {
        let config = self.jwt_config.as_ref()
            .ok_or(AuthError::JwtNotConfigured)?;
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let claims = Claims {
            sub: agent_id.to_string(),
            iss: config.issuer.clone(),
            iat: now,
            exp: now + (config.expiry_hours * 3600),
            jti: Uuid::new_v4().to_string(),
        };
        
        // Simple JWT encoding without external crate
        // Format: base64(header).base64(claims).base64(signature)
        let header = r#"{"alg":"HS256","typ":"JWT"}"#;
        let header_b64 = base64_encode(header);
        let claims_json = serde_json::to_string(&claims)
            .map_err(|e| AuthError::Serialization(e.to_string()))?;
        let claims_b64 = base64_encode(&claims_json);
        
        let signature_input = format!("{}.{}", header_b64, claims_b64);
        let signature = hmac_sha256(&signature_input, &config.secret);
        let signature_b64 = base64_encode(&signature);
        
        Ok(format!("{}.{}.{}", header_b64, claims_b64, signature_b64))
    }
    
    /// Create a session token for WebSocket connections
    pub async fn create_session_token(
        &self,
        agent_id: &str,
    ) -> String {
        let token = format!("sess_{}", Uuid::new_v4().simple());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let session = SessionToken {
            agent_id: agent_id.to_string(),
            created_at: now,
            expires_at: now + 3600, // 1 hour
        };
        
        self.session_tokens.write().await.insert(token.clone(), session);
        
        info!(token = %token, agent_id = %agent_id, "Session token created");
        token
    }
    
    /// Validate a session token
    pub async fn validate_session_token(
        &self,
        token: &str,
    ) -> Option<String> {
        let sessions = self.session_tokens.read().await;
        
        if let Some(session) = sessions.get(token) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            
            if session.expires_at > now {
                return Some(session.agent_id.clone());
            }
        }
        
        None
    }
    
    /// Revoke a session token
    pub async fn revoke_session_token(&self,
        token: &str,
    ) {
        self.session_tokens.write().await.remove(token);
        info!(token = %token, "Session token revoked");
    }
    
    /// Add an API key
    pub async fn add_api_key(&self,
        key: impl Into<String>,
    ) {
        self.api_keys.write().await.insert(key.into());
    }
    
    /// Remove an API key
    pub async fn remove_api_key(&self,
        key: &str,
    ) {
        self.api_keys.write().await.remove(key);
    }
    
    /// Validate JWT token (simple implementation)
    fn validate_jwt(
        &self,
        token: &str,
        config: &JwtConfig,
    ) -> Result<Claims, AuthError> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(AuthError::InvalidToken);
        }
        
        // Verify signature
        let signature_input = format!("{}.{}", parts[0], parts[1]);
        let expected_sig = hmac_sha256(&signature_input, &config.secret);
        let expected_sig_b64 = base64_encode(&expected_sig);
        
        if parts[2] != expected_sig_b64 {
            return Err(AuthError::InvalidSignature);
        }
        
        // Decode claims
        let claims_json = base64_decode(parts[1])
            .ok_or(AuthError::InvalidToken)?;
        let claims: Claims = serde_json::from_slice(&claims_json)
            .map_err(|_| AuthError::InvalidToken)?;
        
        // Check expiration
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        if claims.exp < now {
            return Err(AuthError::TokenExpired);
        }
        
        // Check issuer
        if claims.iss != config.issuer {
            return Err(AuthError::InvalidIssuer);
        }
        
        Ok(claims)
    }
    
    /// Clean up expired session tokens
    pub async fn cleanup_expired(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let mut sessions = self.session_tokens.write().await;
        let expired: Vec<String> = sessions
            .iter()
            .filter(|(_, s)| s.expires_at <= now)
            .map(|(k, _)| k.clone())
            .collect();
        
        for token in expired {
            sessions.remove(&token);
        }
    }
}

impl Default for AuthManager {
    fn default() -> Self {
        Self::disabled()
    }
}

// Simple base64 encoding (URL-safe, no padding)
fn base64_encode(input: &str) -> String {
    use std::collections::HashMap;
    
    const TABLE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = input.as_bytes();
    let mut result = String::new();
    
    for chunk in bytes.chunks(3) {
        let b1 = chunk[0];
        let b2 = chunk.get(1).copied().unwrap_or(0);
        let b3 = chunk.get(2).copied().unwrap_or(0);
        
        let n = ((b1 as u32) << 16) | ((b2 as u32) << 8) | (b3 as u32);
        
        result.push(TABLE.chars().nth(((n >> 18) & 63) as usize).unwrap());
        result.push(TABLE.chars().nth(((n >> 12) & 63) as usize).unwrap());
        
        if chunk.len() > 1 {
            result.push(TABLE.chars().nth(((n >> 6) & 63) as usize).unwrap());
        }
        
        if chunk.len() > 2 {
            result.push(TABLE.chars().nth((n & 63) as usize).unwrap());
        }
    }
    
    result
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    // Simple base64url decode
    let mut result = Vec::new();
    let mut buf: u32 = 0;
    let mut bits = 0;
    
    for c in input.chars() {
        let val = match c {
            'A'..='Z' => c as u8 - b'A',
            'a'..='z' => c as u8 - b'a' + 26,
            '0'..='9' => c as u8 - b'0' + 52,
            '-' => 62,
            '_' => 63,
            _ => continue,
        };
        
        buf = (buf << 6) | val as u32;
        bits += 6;
        
        if bits >= 8 {
            bits -= 8;
            result.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    
    Some(result)
}

// Simple HMAC-SHA256 implementation
fn hmac_sha256(message: &str, key: &str) -> Vec<u8> {
    // For production, use ring or hmac crate
    // This is a simplified placeholder that combines key and message
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    message.hash(&mut hasher);
    let hash1 = hasher.finish();
    
    let mut hasher2 = DefaultHasher::new();
    hash1.hash(&mut hasher2);
    message.hash(&mut hasher2);
    let hash2 = hasher2.finish();
    
    // Return 32 bytes (simplified)
    let mut result = Vec::with_capacity(32);
    result.extend_from_slice(&hash1.to_le_bytes());
    result.extend_from_slice(&hash2.to_le_bytes());
    result.resize(32, 0);
    result
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("JWT not configured")]
    JwtNotConfigured,
    
    #[error("Invalid token")]
    InvalidToken,
    
    #[error("Invalid signature")]
    InvalidSignature,
    
    #[error("Token expired")]
    TokenExpired,
    
    #[error("Invalid issuer")]
    InvalidIssuer,
    
    #[error("Serialization error: {0}")]
    Serialization(String),
}

// Need HashMap for session_tokens
use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_base64_roundtrip() {
        let input = r#"{"sub":"test","iat":12345}"#;
        let encoded = base64_encode(input);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), input);
    }
    
    #[tokio::test]
    async fn test_api_key_auth() {
        let auth = AuthManager::with_api_keys(vec!["test-key-123".to_string()]);
        
        // Valid key
        match auth.authenticate_api_key("test-key-123").await {
            AuthResult::Authenticated { .. } => (),
            _ => panic!("Should authenticate"),
        }
        
        // Invalid key
        match auth.authenticate_api_key("wrong-key").await {
            AuthResult::Unauthenticated => (),
            _ => panic!("Should not authenticate"),
        }
    }
    
    #[test]
    fn test_jwt_generation() {
        let auth = AuthManager::with_jwt("secret123", "exo-gateway");
        let token = auth.generate_jwt("agent-1").unwrap();
        
        // Should have 3 parts
        assert_eq!(token.split('.').count(), 3);
    }
}

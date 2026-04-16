//! JWT Authentication Middleware for K2K Protected Routes

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    middleware::Next,
    response::Response,
    Json,
};
use std::sync::Arc;
use tracing::warn;

use super::models::{K2KClaims, K2KError};
use super::server::K2KServerState;

/// Middleware that validates JWT Bearer tokens on protected routes.
///
/// 1. Extracts Bearer token from Authorization header
/// 2. Decodes JWT to get client_id from claims
/// 3. Looks up client in KeyManager
/// 4. Verifies client status == "approved"
/// 5. Verifies JWT signature using client's public key
/// 6. Checks expiration
/// 7. Stores validated claims in request extensions
pub async fn require_k2k_auth(
    State(state): State<Arc<K2KServerState>>,
    headers: HeaderMap,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, (StatusCode, Json<K2KError>)> {
    // 1. Extract Bearer token
    let jwt = extract_bearer_token(&headers).map_err(|e| {
        warn!("Missing or invalid Authorization header: {}", e);
        (
            StatusCode::UNAUTHORIZED,
            Json(K2KError::unauthorized(
                "Missing or invalid Authorization header",
            )),
        )
    })?;

    // 2. Decode JWT to get client_id (without verification first)
    let client_id = extract_client_id_from_jwt(&jwt).map_err(|e| {
        warn!("Failed to extract client_id from JWT: {}", e);
        (
            StatusCode::UNAUTHORIZED,
            Json(K2KError::unauthorized("Invalid JWT format")),
        )
    })?;

    // 3-6. Verify JWT and check client status
    let claims = {
        let key_manager = state.key_manager.lock().await;

        // Check client status
        let status = key_manager.get_client_status(&client_id).await;
        match status.as_deref() {
            Some("approved") => {} // OK
            Some("pending") => {
                warn!("Client '{}' is pending approval", client_id);
                return Err((
                    StatusCode::FORBIDDEN,
                    Json(K2KError::forbidden(
                        "Client registration is pending approval",
                    )),
                ));
            }
            Some("rejected") => {
                warn!("Client '{}' has been rejected", client_id);
                return Err((
                    StatusCode::FORBIDDEN,
                    Json(K2KError::forbidden("Client registration was rejected")),
                ));
            }
            Some(other) => {
                warn!("Client '{}' has unknown status: {}", client_id, other);
                return Err((
                    StatusCode::FORBIDDEN,
                    Json(K2KError::forbidden("Client has invalid status")),
                ));
            }
            None => {
                warn!("Unknown client: {}", client_id);
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(K2KError::unauthorized("Unknown client")),
                ));
            }
        }

        // Verify JWT signature
        key_manager.verify_jwt(&jwt, &client_id).map_err(|e| {
            warn!("JWT verification failed for client '{}': {}", client_id, e);
            (
                StatusCode::UNAUTHORIZED,
                Json(K2KError::unauthorized(format!(
                    "JWT verification failed: {}",
                    e
                ))),
            )
        })?
    };

    // Enforce allowlist when configured (applies to all protected routes).
    if !state.config.k2k.allowed_clients.is_empty()
        && !state.config.k2k.allowed_clients.contains(&claims.client_id)
    {
        warn!(
            "Client '{}' blocked by allowed_clients policy",
            claims.client_id
        );
        return Err((
            StatusCode::FORBIDDEN,
            Json(K2KError::forbidden(format!(
                "Client '{}' not allowed",
                claims.client_id
            ))),
        ));
    }

    // 7. Store validated claims in request extensions for downstream handlers
    request.extensions_mut().insert(claims);

    // 8. Continue to the actual handler
    Ok(next.run(request).await)
}

fn extract_bearer_token(headers: &HeaderMap) -> Result<String, &'static str> {
    let auth_header = headers
        .get("authorization")
        .ok_or("Missing Authorization header")?
        .to_str()
        .map_err(|_| "Invalid Authorization header")?;

    if !auth_header.starts_with("Bearer ") {
        return Err("Authorization header must start with 'Bearer '");
    }

    Ok(auth_header[7..].to_string())
}

fn extract_client_id_from_jwt(jwt: &str) -> anyhow::Result<String> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        anyhow::bail!("Invalid JWT format");
    }

    use base64::Engine;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let payload_bytes = engine.decode(parts[1])?;
    let claims: K2KClaims = serde_json::from_slice(&payload_bytes)?;

    Ok(claims.client_id)
}

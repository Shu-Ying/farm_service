use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State, rejection::JsonRejection},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use subtle::ConstantTimeEq;

/// Shared HTTP dependencies. The token must never be logged.
#[derive(Clone)]
struct AppState {
    /// Persistent profile storage.
    pool: SqlitePool,
    /// Expected full Authorization header.
    authorization: Arc<str>,
}

/// One explicitly supported account namespace.
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Platform {
    /// QQ number from a trusted OneBot-style adapter, not official QQ OpenID.
    Qq,
    /// Telegram numeric user ID.
    Telegram,
    /// Discord numeric user ID.
    Discord,
}

impl Platform {
    fn as_str(self) -> &'static str {
        match self {
            Self::Qq => "qq",
            Self::Telegram => "telegram",
            Self::Discord => "discord",
        }
    }

    fn max_id_len(self) -> usize {
        match self {
            Self::Qq => 20,
            Self::Telegram | Self::Discord => 32,
        }
    }
}

/// User profile supplied by an authenticated bot deployment.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserInput {
    /// Account namespace used together with the ID as the unique key.
    platform: Platform,
    /// Decimal string; for qq this is the QQ number.
    platform_user_id: String,
    /// Current display name (QQ nickname on the qq platform).
    nickname: String,
}

impl UserInput {
    fn validate(&mut self) -> Result<(), ApiError> {
        validate_id(self.platform, &self.platform_user_id)?;
        self.nickname = self.nickname.trim().to_owned();
        if self.nickname.is_empty()
            || self.nickname.chars().count() > 64
            || self.nickname.chars().any(char::is_control)
        {
            return Err(ApiError::bad_request(
                "nickname must contain 1..=64 characters without control characters",
            ));
        }
        Ok(())
    }
}

/// Persisted profile and UTC timestamps returned to the authenticated caller.
#[derive(Serialize, FromRow)]
struct UserProfile {
    /// Stored platform namespace.
    platform: String,
    /// Stored decimal identity, preserved as a string.
    platform_user_id: String,
    /// Most recently reported nickname.
    nickname: String,
    /// First successful registration time, in UTC.
    created_at: String,
    /// Last successful profile write time, in UTC.
    updated_at: String,
}

/// Stable machine-readable error without request data.
#[derive(Serialize)]
struct ErrorBody {
    /// Error category.
    code: &'static str,
    /// Safe explanation without user identifiers or database details.
    message: &'static str,
}

/// Internal error mapping to an HTTP response.
struct ApiError {
    /// HTTP status for this error.
    status: StatusCode,
    /// Safe public payload.
    body: ErrorBody,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            body: ErrorBody { code, message },
        }
    }

    fn bad_request(message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_input", message)
    }

    fn database(error: sqlx::Error) -> Self {
        // Do not log error text: constraint failures may contain user data.
        tracing::error!(
            kind = database_error_kind(&error),
            "database_operation_failed"
        );
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "storage temporarily unavailable",
        )
    }
}

fn database_error_kind(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::Database(_) => "database",
        sqlx::Error::PoolTimedOut => "pool_timeout",
        sqlx::Error::PoolClosed => "pool_closed",
        _ => "storage",
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

/// Build a router; only the readiness endpoint is public.
pub fn router(pool: SqlitePool, api_token: String) -> Router {
    let state = AppState {
        pool,
        authorization: Arc::from(format!("Bearer {api_token}")),
    };
    let protected = Router::new()
        .route("/v1/users", put(upsert_user))
        .route("/v1/users/{platform}/{platform_user_id}", get(get_user))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .layer(DefaultBodyLimit::max(4096))
        .with_state(state)
}

async fn authenticate(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let supplied = request.headers().get(header::AUTHORIZATION);
    let authorized = supplied
        .is_some_and(|value| bool::from(value.as_bytes().ct_eq(state.authorization.as_bytes())));
    if !authorized {
        tracing::warn!("authentication_rejected");
        let mut response = ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "valid bearer token required",
        )
        .into_response();
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static("Bearer"),
        );
        return response;
    }
    next.run(request).await
}

/// Database readiness result.
#[derive(Serialize)]
struct Health {
    /// Ready only after a successful database probe.
    status: &'static str,
}

async fn health(State(state): State<AppState>) -> Result<Json<Health>, ApiError> {
    sqlx::query("SELECT 1")
        .execute(&state.pool)
        .await
        .map_err(ApiError::database)?;
    Ok(Json(Health { status: "ok" }))
}

async fn upsert_user(
    State(state): State<AppState>,
    payload: Result<Json<UserInput>, JsonRejection>,
) -> Result<Json<UserProfile>, ApiError> {
    let Json(mut input) = payload.map_err(|rejection| {
        ApiError::new(
            rejection.status(),
            "invalid_json",
            "expected JSON profile; check field names, types and body size",
        )
    })?;
    input.validate()?;
    // One SQL statement makes create/update atomic under concurrent reports.
    let profile = sqlx::query_as::<_, UserProfile>(
        "INSERT INTO users (platform, platform_user_id, nickname)
         VALUES (?, ?, ?)
         ON CONFLICT(platform, platform_user_id) DO UPDATE SET
             nickname = excluded.nickname,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         RETURNING platform, platform_user_id, nickname, created_at, updated_at",
    )
    .bind(input.platform.as_str())
    .bind(&input.platform_user_id)
    .bind(&input.nickname)
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::database)?;
    tracing::info!(platform = input.platform.as_str(), "user_profile_saved");
    Ok(Json(profile))
}

async fn get_user(
    State(state): State<AppState>,
    Path((platform, platform_user_id)): Path<(Platform, String)>,
) -> Result<Json<UserProfile>, ApiError> {
    validate_id(platform, &platform_user_id)?;
    let profile = sqlx::query_as::<_, UserProfile>(
        "SELECT platform, platform_user_id, nickname, created_at, updated_at
         FROM users WHERE platform = ? AND platform_user_id = ?",
    )
    .bind(platform.as_str())
    .bind(platform_user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::database)?
    .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not_found", "user not found"))?;
    Ok(Json(profile))
}

fn validate_id(platform: Platform, id: &str) -> Result<(), ApiError> {
    if id.is_empty()
        || id.len() > platform.max_id_len()
        || !id.bytes().all(|byte| byte.is_ascii_digit())
        || id.starts_with('0')
    {
        return Err(ApiError::bad_request(
            "platform_user_id must be a nonzero decimal string without leading zeros",
        ));
    }
    Ok(())
}

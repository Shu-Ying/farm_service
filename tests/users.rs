use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use farm_service::{api, db};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

const TOKEN: &str = "integration-test-token-not-for-production";

async fn app() -> (TempDir, SqlitePool, Router) {
    let directory = tempfile::tempdir().unwrap();
    let pool = db::open_database(&directory.path().join("test.sqlite3"))
        .await
        .unwrap();
    let router = api::router(pool.clone(), TOKEN.into());
    (directory, pool, router)
}

fn profile(platform: &str, id: &str, nickname: &str) -> Value {
    json!({
        "platform": platform,
        "platform_user_id": id,
        "nickname": nickname
    })
}

async fn send(app: Router, method: &str, uri: &str, body: Value, authenticated: bool) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if authenticated {
        request = request.header("authorization", format!("Bearer {TOKEN}"));
    }
    app.oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn health_checks_database_readiness() {
    let (_directory, pool, router) = app().await;
    let response = send(router.clone(), "GET", "/health", Value::Null, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await, json!({"status": "ok"}));
    pool.close().await;
    let response = send(router, "GET", "/health", Value::Null, false).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn authentication_prevents_profile_reads_and_writes() {
    let (_directory, pool, router) = app().await;
    for (method, uri) in [("PUT", "/v1/users"), ("GET", "/v1/users/qq/12345678")] {
        let response = send(
            router.clone(),
            method,
            uri,
            profile("qq", "12345678", "测试昵称"),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let response = router
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/v1/users")
                .header("authorization", "Bearer incorrect-token")
                .header("content-type", "application/json")
                .body(Body::from(profile("qq", "12345678", "测试").to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    pool.close().await;
}

#[tokio::test]
async fn profile_updates_are_persistent_and_preserve_creation_time() {
    let (directory, pool, router) = app().await;
    let first = send(
        router.clone(),
        "PUT",
        "/v1/users",
        profile("qq", "12345678", "  初始昵称  "),
        true,
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first = json_body(first).await;
    assert_eq!(first["nickname"], "初始昵称");
    let updated = send(
        router.clone(),
        "PUT",
        "/v1/users",
        profile("qq", "12345678", "新昵称"),
        true,
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let updated = json_body(updated).await;
    assert_eq!(first["created_at"], updated["created_at"]);
    assert_eq!(updated["platform_user_id"], "12345678");
    drop(router);
    pool.close().await;

    let reopened = db::open_database(&directory.path().join("test.sqlite3"))
        .await
        .unwrap();
    let response = send(
        api::router(reopened.clone(), TOKEN.into()),
        "GET",
        "/v1/users/qq/12345678",
        Value::Null,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["nickname"], "新昵称");
    reopened.close().await;
}

#[tokio::test]
async fn platforms_do_not_merge_equal_numeric_ids() {
    let (_directory, pool, router) = app().await;
    for platform in ["qq", "telegram", "discord"] {
        let response = send(
            router.clone(),
            "PUT",
            "/v1/users",
            profile(platform, "12345678", platform),
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3);
    let missing = send(router, "GET", "/v1/users/qq/99999999", Value::Null, true).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    pool.close().await;
}

#[tokio::test]
async fn invalid_profiles_do_not_write_any_data() {
    let (_directory, pool, router) = app().await;
    let mut cases = vec![
        profile("qq", "0", "昵称"),
        profile("qq", "00123456", "昵称"),
        profile("qq", "-123", "昵称"),
        profile("qq", "１２３", "昵称"),
        profile("qq", "12345678", "   "),
        profile("qq", "12345678", "换行\n昵称"),
        profile("qq", "12345678", &"字".repeat(65)),
        profile("qq", &"1".repeat(21), "昵称"),
        profile("unknown", "12345678", "昵称"),
        json!({"platform": "qq", "platform_user_id": 12345678, "nickname": "昵称"}),
    ];
    let mut unknown = profile("qq", "12345678", "昵称");
    unknown["unused_field"] = json!(true);
    cases.push(unknown);
    for body in cases {
        let response = send(router.clone(), "PUT", "/v1/users", body, true).await;
        assert!(response.status().is_client_error());
    }
    let oversized = send(
        router,
        "PUT",
        "/v1/users",
        profile("qq", "12345678", &"a".repeat(5000)),
        true,
    )
    .await;
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    pool.close().await;
}

#[tokio::test]
async fn concurrent_reports_create_one_profile() {
    let (_directory, pool, router) = app().await;
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..12 {
        let router = router.clone();
        tasks.spawn(async move {
            send(
                router,
                "PUT",
                "/v1/users",
                profile("qq", "12345678", &format!("昵称{index}")),
                true,
            )
            .await
            .status()
        });
    }
    while let Some(status) = tasks.join_next().await {
        assert_eq!(status.unwrap(), StatusCode::OK);
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    pool.close().await;
}

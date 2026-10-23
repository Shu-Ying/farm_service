use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use farm_service::assets::{self, AssetBundle, Manifest, Sources, digest};
use tower::ServiceExt;

fn fixture() -> (tempfile::TempDir, Manifest, Vec<u8>) {
    let directory = tempfile::tempdir().unwrap();
    // A real ZIP with one member named x containing a; no external ZIP dependency.
    let content = vec![
        80, 75, 3, 4, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 67, 190, 183, 232, 1, 0, 0, 0, 1, 0, 0, 0, 1,
        0, 0, 0, 120, 97, 80, 75, 1, 2, 20, 3, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 67, 190, 183, 232, 1,
        0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 1, 0, 0, 0, 0, 120, 80, 75,
        5, 6, 0, 0, 0, 0, 1, 0, 1, 0, 47, 0, 0, 0, 32, 0, 0, 0, 0, 0,
    ];
    let sha256 = digest(&content);
    let manifest = Manifest {
        schema: 1,
        version: "assets-test-1".into(),
        size_bytes: content.len() as u64,
        sources: Sources {
            github: "https://raw.githubusercontent.com/owner/farm/main/assets.zip".into(),
            gitea: "https://gitea.example/owner/farm/raw/branch/main/assets.zip".into(),
            farm_service: format!("https://farm.example/v1/assets/{sha256}/assets.zip"),
        },
        sha256,
    };
    std::fs::write(directory.path().join("assets.zip"), &content).unwrap();
    save(&directory, &manifest);
    (directory, manifest, content)
}

fn save(directory: &tempfile::TempDir, manifest: &Manifest) {
    std::fs::write(
        directory.path().join("manifest.json"),
        serde_json::to_vec(manifest).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn public_manifest_download_cache_and_snapshot() {
    let (directory, manifest, content) = fixture();
    let bundle = AssetBundle::load(&directory.path().join("manifest.json")).unwrap();
    let app = assets::router(Some(bundle));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/assets/manifest.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 16384).await.unwrap();
    let result: Manifest = serde_json::from_slice(&body).unwrap();
    assert_eq!(result.sha256, manifest.sha256);
    assert_eq!(result.sources.github, manifest.sources.github);
    // Requests must keep serving exactly the already-verified bytes, even if disk changes.
    std::fs::write(directory.path().join("assets.zip"), b"changed").unwrap();
    let uri = format!("/v1/assets/{}/assets.zip", manifest.sha256);
    let response = app
        .clone()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/zip");
    let etag = response.headers()["etag"].clone();
    assert_eq!(to_bytes(response.into_body(), 4096).await.unwrap(), content);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("if-none-match", etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/assets/unknown/assets.zip")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn rejects_corruption_invalid_urls_and_missing_files() {
    let (directory, original, mut content) = fixture();
    let path = directory.path().join("manifest.json");
    for value in [
        "http://farm.example/a",
        "https://user:pass@farm.example/a",
        "https://farm.example/a?q=token",
    ] {
        let mut manifest = original.clone();
        manifest.sources.github = value.into();
        save(&directory, &manifest);
        assert!(AssetBundle::load(&path).is_err());
    }
    let mut manifest = original.clone();
    manifest.size_bytes += 1;
    save(&directory, &manifest);
    assert!(AssetBundle::load(&path).is_err());
    save(&directory, &original);
    content[31] ^= 1;
    std::fs::write(directory.path().join("assets.zip"), content).unwrap();
    assert!(
        AssetBundle::load(&path)
            .err()
            .unwrap()
            .to_string()
            .contains("SHA256")
    );
    std::fs::write(directory.path().join("assets.zip"), b"corrupt").unwrap();
    assert!(AssetBundle::load(&path).is_err());
    std::fs::remove_file(directory.path().join("assets.zip")).unwrap();
    assert!(AssetBundle::load(&path).is_err());
    std::fs::write(path.clone(), b"{}").unwrap();
    assert!(AssetBundle::load(&path).is_err());
}

#[tokio::test]
async fn disabled_resources_do_not_expose_files() {
    let response = assets::router(None)
        .oneshot(
            Request::builder()
                .uri("/v1/assets/manifest.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn public_assets_do_not_bypass_profile_authentication() {
    let (directory, _, _) = fixture();
    let pool = farm_service::db::open_database(&directory.path().join("users.sqlite3"))
        .await
        .unwrap();
    let bundle = AssetBundle::load(&directory.path().join("manifest.json")).unwrap();
    let app = farm_service::api::router(pool.clone(), "test-only-token".into())
        .merge(assets::router(Some(bundle)));
    for (uri, status) in [
        ("/v1/assets/manifest.json", StatusCode::OK),
        ("/v1/users/qq/123456", StatusCode::UNAUTHORIZED),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    pool.close().await;
}

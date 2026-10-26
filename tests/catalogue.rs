use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use farm_service::{
    assets::digest,
    catalogue::{self, CatalogueStore},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};
use tower::ServiceExt;

fn publish(root: &Path, version: &str, plants: &[u8]) -> Value {
    let content = BTreeMap::from([
        ("plants.json", plants),
        ("metadata.json", b"{\"schema\":1}".as_slice()),
        ("assets.zip", b"PK\x03\x04pictures".as_slice()),
        ("land-soils.zip", b"PK\x03\x04soils".as_slice()),
    ]);
    let files: BTreeMap<_, _> = content
        .iter()
        .map(|(name, bytes)| {
            (
                *name,
                json!({"sha256": digest(bytes), "size_bytes": bytes.len()}),
            )
        })
        .collect();
    let release = digest(&serde_json::to_vec(&files).unwrap());
    let manifest = json!({"schema": 1, "version": version, "release": release,
                          "files": files});
    let directory = root.join("releases").join(&release);
    std::fs::create_dir_all(&directory).unwrap();
    for (name, bytes) in content {
        std::fs::write(directory.join(name), bytes).unwrap();
    }
    let bytes = serde_json::to_vec(&manifest).unwrap();
    std::fs::write(directory.join("manifest.json"), &bytes).unwrap();
    std::fs::write(root.join("manifest.json"), bytes).unwrap();
    manifest
}

#[tokio::test]
async fn serves_latest_history_etags_and_verified_memory_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let old = publish(directory.path(), "one", b"{\"schema\":1,\"plants\":[1]}");
    let latest = publish(directory.path(), "two", b"{\"schema\":1,\"plants\":[2]}");
    let store = CatalogueStore::load(directory.path()).unwrap();
    let app = catalogue::router(Some(store));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/catalogue/manifest.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()["etag"].clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&to_bytes(response.into_body(), 16384).await.unwrap())
            .unwrap(),
        latest
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/catalogue/manifest.json")
                .header("if-none-match", etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    let release = old["release"].as_str().unwrap();
    std::fs::write(
        directory
            .path()
            .join("releases")
            .join(release)
            .join("plants.json"),
        b"changed",
    )
    .unwrap();
    let uri = format!("/v1/catalogue/releases/{release}/plants.json");
    let response = app
        .clone()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    let etag = response.headers()["etag"].clone();
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap(),
        b"{\"schema\":1,\"plants\":[1]}".as_slice()
    );
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
    for file in [
        "metadata.json",
        "assets.zip",
        "land-soils.zip",
        "manifest.json",
    ] {
        let uri = format!("/v1/catalogue/releases/{release}/{file}");
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn disabled_unknown_paths_and_profile_authentication_remain_separate() {
    let response = catalogue::router(None)
        .oneshot(
            Request::builder()
                .uri("/v1/catalogue/manifest.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let directory = tempfile::tempdir().unwrap();
    let manifest = publish(directory.path(), "test", b"{}");
    let store = CatalogueStore::load(directory.path()).unwrap();
    let pool = farm_service::db::open_database(&directory.path().join("test.sqlite3"))
        .await
        .unwrap();
    let app = farm_service::api::router(pool.clone(), "test-only-token".into())
        .merge(catalogue::router(Some(store)));
    let release = manifest["release"].as_str().unwrap();
    for (uri, expected) in [
        (
            format!("/v1/catalogue/releases/{release}/secret.txt"),
            StatusCode::NOT_FOUND,
        ),
        (
            "/v1/catalogue/releases/unknown/plants.json".into(),
            StatusCode::NOT_FOUND,
        ),
        ("/v1/catalogue/manifest.json".into(), StatusCode::OK),
        ("/v1/users/qq/123456".into(), StatusCode::UNAUTHORIZED),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    pool.close().await;
}

#[test]
fn rejects_missing_corrupt_oversized_or_unbound_release_files() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = publish(directory.path(), "test", b"{}");
    let release = manifest["release"].as_str().unwrap();
    let file = directory
        .path()
        .join("releases")
        .join(release)
        .join("plants.json");
    std::fs::write(&file, b"bad").unwrap();
    assert!(CatalogueStore::load(directory.path()).is_err());
    std::fs::write(&file, b"[]").unwrap();
    assert!(CatalogueStore::load(directory.path()).is_err());
    std::fs::remove_file(&file).unwrap();
    assert!(CatalogueStore::load(directory.path()).is_err());
    std::fs::write(&file, b"{}").unwrap();
    let mut invalid = manifest.clone();
    invalid["files"]["plants.json"]["size_bytes"] = json!(2 * 1024 * 1024 + 1);
    std::fs::write(
        directory.path().join("manifest.json"),
        serde_json::to_vec(&invalid).unwrap(),
    )
    .unwrap();
    assert!(CatalogueStore::load(directory.path()).is_err());
    invalid = manifest.clone();
    invalid["release"] = json!("a".repeat(64));
    std::fs::write(
        directory.path().join("manifest.json"),
        serde_json::to_vec(&invalid).unwrap(),
    )
    .unwrap();
    assert!(CatalogueStore::load(directory.path()).is_err());
}

#[test]
fn rejects_unknown_schema_fields_missing_latest_history_and_bad_historical_manifest() {
    for kind in ["schema", "extra", "missing", "label", "history"] {
        let directory = tempfile::tempdir().unwrap();
        let mut manifest = publish(directory.path(), "test", b"{}");
        let release = manifest["release"].as_str().unwrap().to_string();
        match kind {
            "schema" => manifest["schema"] = json!(2),
            "extra" => {
                manifest["files"]["private.txt"] =
                    json!({"sha256": "a".repeat(64), "size_bytes": 1})
            }
            "missing" => {
                std::fs::remove_dir_all(directory.path().join("releases").join(&release)).unwrap();
            }
            "label" => manifest["version"] = json!("different-label"),
            "history" => {
                let old = publish(directory.path(), "old", b"[]");
                std::fs::write(
                    directory
                        .path()
                        .join("releases")
                        .join(old["release"].as_str().unwrap())
                        .join("manifest.json"),
                    b"{}",
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        std::fs::write(
            directory.path().join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(
            CatalogueStore::load(directory.path()).is_err(),
            "accepted {kind}"
        );
    }
}

#[test]
fn independently_published_repository_release_is_accepted_by_rust() {
    CatalogueStore::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("catalogue")).unwrap();
}

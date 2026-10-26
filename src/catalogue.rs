//! Public crop releases verified once at startup, independent of profile tokens.

use std::{collections::BTreeMap, fs::File, io::Read, path::Path, sync::Arc};

use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path as RoutePath, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::assets::digest;

const MAX_MANIFEST: usize = 16 * 1024;
const FILES: [&str; 4] = [
    "plants.json",
    "metadata.json",
    "assets.zip",
    "land-soils.zip",
];

/// A bounded file whose bytes are bound into the release identifier.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileDescriptor {
    /// Exact lowercase SHA256 of the published file bytes.
    pub sha256: String,
    /// Positive byte length checked before serving the snapshot.
    pub size_bytes: u64,
}

/// Versioned discovery contract shared with the independent Farm client.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Contract schema, currently one.
    pub schema: u32,
    /// Human selected immutable label, limited to safe ASCII characters.
    pub version: String,
    /// SHA256 of the sorted compact JSON files object.
    pub release: String,
    /// Exactly the four required release files, without arbitrary paths or URLs.
    pub files: BTreeMap<String, FileDescriptor>,
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn file_limit(name: &str) -> Option<usize> {
    match name {
        "plants.json" => Some(2 * 1024 * 1024),
        "metadata.json" => Some(4 * 1024 * 1024),
        "assets.zip" | "land-soils.zip" => Some(64 * 1024 * 1024),
        "manifest.json" => Some(MAX_MANIFEST),
        _ => None,
    }
}

impl Manifest {
    fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "unsupported catalogue schema");
        ensure!(
            !self.version.is_empty()
                && self.version.len() <= 64
                && self
                    .version
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
            "invalid catalogue version"
        );
        ensure!(valid_digest(&self.release), "invalid catalogue release");
        ensure!(
            self.files.len() == FILES.len()
                && FILES.iter().all(|name| self.files.contains_key(*name)),
            "catalogue requires exactly four release files"
        );
        for (name, descriptor) in &self.files {
            ensure!(
                valid_digest(&descriptor.sha256),
                "invalid catalogue file digest"
            );
            ensure!(
                descriptor.size_bytes > 0
                    && descriptor.size_bytes
                        <= file_limit(name).expect("validated file name") as u64,
                "invalid catalogue file size"
            );
        }
        // Value uses sorted maps, including the fields inside each descriptor.
        let canonical = serde_json::to_vec(&serde_json::to_value(&self.files)?)?;
        ensure!(
            digest(&canonical) == self.release,
            "catalogue release digest mismatch"
        );
        Ok(())
    }
}

fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    ensure!(!path.is_symlink(), "catalogue files cannot be symlinks");
    let file = File::open(path).context("cannot open catalogue file")?;
    let mut content = Vec::new();
    file.take(maximum as u64 + 1).read_to_end(&mut content)?;
    ensure!(
        !content.is_empty() && content.len() <= maximum,
        "catalogue file exceeds size limit"
    );
    Ok(content)
}

fn read_manifest(path: &Path) -> Result<(Manifest, Bytes)> {
    let content = read_bounded(path, MAX_MANIFEST)?;
    let manifest: Manifest =
        serde_json::from_slice(&content).context("invalid catalogue manifest JSON")?;
    manifest.validate()?;
    Ok((manifest, Bytes::from(content)))
}

struct ReleaseSnapshot {
    files: BTreeMap<String, Bytes>,
    manifest: Manifest,
}

struct CatalogueSnapshot {
    latest: Bytes,
    latest_digest: String,
    releases: BTreeMap<String, ReleaseSnapshot>,
}

/// Shared immutable memory snapshot of latest and all published history.
#[derive(Clone)]
pub struct CatalogueStore(Arc<CatalogueSnapshot>);

impl CatalogueStore {
    /// Reject invalid manifests, missing history or corrupt bytes before startup.
    pub fn load(directory: &Path) -> Result<Self> {
        let (latest_manifest, latest) = read_manifest(&directory.join("manifest.json"))?;
        let release_root = directory.join("releases");
        ensure!(
            !release_root.is_symlink(),
            "catalogue releases cannot be a symlink"
        );
        let mut releases = BTreeMap::new();
        for entry in std::fs::read_dir(&release_root).context("cannot read catalogue releases")? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(release) = name.to_str() else {
                anyhow::bail!("invalid release directory name")
            };
            // Incomplete publication directories are never publicly discoverable.
            if release.starts_with(".prepare-") {
                continue;
            }
            ensure!(valid_digest(release), "invalid release directory name");
            ensure!(entry.file_type()?.is_dir(), "release must be a directory");
            let (manifest, manifest_bytes) = read_manifest(&entry.path().join("manifest.json"))?;
            ensure!(
                manifest.release == release,
                "release directory digest mismatch"
            );
            let mut files = BTreeMap::new();
            for (name, descriptor) in &manifest.files {
                let content = read_bounded(&entry.path().join(name), file_limit(name).unwrap())?;
                ensure!(
                    content.len() as u64 == descriptor.size_bytes,
                    "catalogue file size mismatch"
                );
                ensure!(
                    digest(&content) == descriptor.sha256,
                    "catalogue file SHA256 mismatch"
                );
                files.insert(name.clone(), Bytes::from(content));
            }
            files.insert("manifest.json".into(), manifest_bytes);
            releases.insert(release.to_string(), ReleaseSnapshot { files, manifest });
        }
        let selected = releases
            .get(&latest_manifest.release)
            .context("latest release is missing")?;
        ensure!(
            selected.manifest == latest_manifest,
            "latest manifest differs from release manifest"
        );
        tracing::info!(release = %latest_manifest.release, releases = releases.len(), "catalogue_verified");
        Ok(Self(Arc::new(CatalogueSnapshot {
            latest_digest: digest(&latest),
            latest,
            releases,
        })))
    }
}

/// The catalogue API is public and read-only; None exposes only unavailable status.
pub fn router(store: Option<CatalogueStore>) -> Router {
    Router::new()
        .route("/v1/catalogue/manifest.json", get(get_manifest))
        .route("/v1/catalogue/releases/{release}/{file}", get(download))
        .with_state(store)
}

async fn get_manifest(State(store): State<Option<CatalogueStore>>, headers: HeaderMap) -> Response {
    let Some(store) = store else {
        return unavailable();
    };
    respond(
        store.0.latest.clone(),
        &store.0.latest_digest,
        "application/json",
        false,
        &headers,
    )
}

async fn download(
    State(store): State<Option<CatalogueStore>>,
    RoutePath((release, file)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let Some(store) = store else {
        return unavailable();
    };
    if !valid_digest(&release) || file_limit(&file).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(snapshot) = store.0.releases.get(&release) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(content) = snapshot.files.get(&file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let checksum = snapshot
        .manifest
        .files
        .get(&file)
        .map(|f| f.sha256.clone())
        .unwrap_or_else(|| digest(content));
    let content_type = if file.ends_with(".zip") {
        "application/zip"
    } else {
        "application/json"
    };
    respond(content.clone(), &checksum, content_type, true, &headers)
}

fn respond(
    content: Bytes,
    checksum: &str,
    content_type: &'static str,
    immutable: bool,
    headers: &HeaderMap,
) -> Response {
    let etag = format!("\"{checksum}\"");
    let cached = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .map(str::trim)
                .any(|v| v == "*" || v.trim_start_matches("W/") == etag)
        });
    let mut response = if cached {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(header::CONTENT_TYPE, content_type)], content).into_response()
    };
    response
        .headers_mut()
        .insert(header::ETAG, etag.parse().expect("digest header"));
    let cache = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        cache.parse().expect("constant header"),
    );
    response
}

fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "code": "catalogue_unavailable", "message": "catalogue distribution is not configured"
        })),
    )
        .into_response()
}

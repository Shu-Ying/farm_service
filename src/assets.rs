//! Public, versioned asset manifest and an immutable in-memory ZIP snapshot.
//! Profiles remain behind their existing bearer authentication middleware.

use std::{fs::File, io::Read, path::Path};

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
use sha2::{Digest, Sha256};
use url::Url;

/// Maximum ZIP bytes held in memory for the active immutable snapshot.
pub const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
/// Maximum JSON manifest size accepted from disk.
const MAX_MANIFEST_BYTES: usize = 16 * 1024;

/// Absolute HTTPS mirrors for the same exact ZIP bytes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sources {
    /// Public GitHub raw or versioned release URL.
    pub github: String,
    /// Owner-controlled Gitea raw or versioned release URL.
    pub gitea: String,
    /// Public farm_service download endpoint; never a profile API token.
    pub farm_service: String,
}

/// Public JSON contract; schema and asset version are separate concepts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Contract version, currently one.
    pub schema: u32,
    /// Immutable release label selected by the operator.
    pub version: String,
    /// Lowercase SHA256 of the complete ZIP, not individual pictures.
    pub sha256: String,
    /// Compressed ZIP byte length for transfer limits and integrity checks.
    pub size_bytes: u64,
    /// Download mirrors containing byte-identical content.
    pub sources: Sources,
}

impl Manifest {
    pub fn validate(&self) -> Result<()> {
        self.validate_for("assets")
    }

    fn validate_for(&self, namespace: &str) -> Result<()> {
        ensure!(self.schema == 1, "unsupported asset manifest schema");
        ensure!(
            !self.version.is_empty()
                && self.version.len() <= 64
                && self
                    .version
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
            "invalid asset version"
        );
        ensure!(
            self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid asset SHA256"
        );
        ensure!(
            self.size_bytes > 0 && self.size_bytes <= MAX_BUNDLE_BYTES as u64,
            "invalid asset size"
        );
        for value in [
            &self.sources.github,
            &self.sources.gitea,
            &self.sources.farm_service,
        ] {
            validate_url(value)?;
        }
        let service_url = Url::parse(&self.sources.farm_service)?;
        ensure!(
            service_url
                .path()
                .ends_with(&format!("/v1/{namespace}/{}/assets.zip", self.sha256)),
            "farm_service URL must include the bundle SHA256"
        );
        Ok(())
    }
}

fn validate_url(value: &str) -> Result<()> {
    ensure!(value.len() <= 2048, "asset URL too long");
    let url = Url::parse(value).context("invalid asset URL")?;
    ensure!(
        url.scheme() == "https" && url.host_str().is_some(),
        "asset URL requires HTTPS"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "asset URL cannot contain credentials, queries or fragments"
    );
    Ok(())
}

fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>> {
    let file = File::open(path).context("cannot open asset file")?;
    let mut bytes = Vec::new();
    file.take(max as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= max, "asset file exceeds size limit");
    Ok(bytes)
}

/// A verified snapshot shared by HTTP requests without disk rereads or copies.
#[derive(Clone)]
pub struct AssetBundle {
    /// Validated public manifest.
    manifest: Manifest,
    /// Exactly the bytes used to compute the manifest digest.
    content: Bytes,
}

impl AssetBundle {
    /// Load manifest.json and its sibling assets.zip, verifying both at startup.
    pub fn load(manifest_path: &Path) -> Result<Self> {
        Self::load_for(manifest_path, "assets")
    }

    /// Load an independent font archive with the same integrity contract.
    pub fn load_font(manifest_path: &Path) -> Result<Self> {
        Self::load_for(manifest_path, "fonts")
    }

    fn load_for(manifest_path: &Path, namespace: &str) -> Result<Self> {
        let manifest: Manifest =
            serde_json::from_slice(&read_bounded(manifest_path, MAX_MANIFEST_BYTES)?)
                .context("invalid asset manifest JSON")?;
        manifest.validate_for(namespace)?;
        let content = read_bounded(
            &manifest_path.with_file_name("assets.zip"),
            MAX_BUNDLE_BYTES,
        )?;
        ensure!(
            content.len() as u64 == manifest.size_bytes,
            "asset size mismatch"
        );
        ensure!(digest(&content) == manifest.sha256, "asset SHA256 mismatch");
        ensure!(
            content.starts_with(b"PK\x03\x04"),
            "asset file is not a ZIP"
        );
        tracing::info!(asset_version = %manifest.version, bytes = content.len(), "assets_verified");
        Ok(Self {
            manifest,
            content: Bytes::from(content),
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}

pub fn digest(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

/// Build independent public read-only routes. None means assets are disabled.
pub fn router(bundle: Option<AssetBundle>) -> Router {
    Router::new()
        .route("/v1/assets/manifest.json", get(get_manifest))
        .route("/v1/assets/{sha256}/assets.zip", get(download))
        .with_state(bundle)
}

/// Font distribution is independent of the picture bundle and profile APIs.
pub fn font_router(bundle: Option<AssetBundle>) -> Router {
    Router::new()
        .route("/v1/fonts/manifest.json", get(get_manifest))
        .route("/v1/fonts/{sha256}/assets.zip", get(download))
        .with_state(bundle)
}

async fn get_manifest(State(bundle): State<Option<AssetBundle>>) -> Response {
    match bundle {
        Some(bundle) => {
            ([(header::CACHE_CONTROL, "no-cache")], Json(bundle.manifest)).into_response()
        }
        None => unavailable(),
    }
}

async fn download(
    State(bundle): State<Option<AssetBundle>>,
    RoutePath(sha256): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    let Some(bundle) = bundle else {
        return unavailable();
    };
    if sha256 != bundle.manifest.sha256 {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "code": "asset_not_found", "message": "requested asset version is unavailable"
            })),
        )
            .into_response();
    }
    let etag = format!("\"{}\"", bundle.manifest.sha256);
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
        (
            [
                (header::CONTENT_TYPE, "application/zip"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=assets.zip",
                ),
            ],
            bundle.content,
        )
            .into_response()
    };
    response
        .headers_mut()
        .insert(header::ETAG, etag.parse().expect("hex digest header"));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "public, max-age=31536000, immutable"
            .parse()
            .expect("constant header"),
    );
    response
}

fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "code": "assets_unavailable", "message": "asset distribution is not configured"
        })),
    )
        .into_response()
}

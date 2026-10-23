//! Prepare a local asset directory; never uploads files or fetches mirrors.
use std::{env, fs, path::PathBuf};

use anyhow::{Result, ensure};
use farm_service::assets::{AssetBundle, MAX_BUNDLE_BYTES, Manifest, Sources, digest};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    ensure!(
        args.len() == 6,
        "usage: prepare_assets ZIP OUTPUT_DIR VERSION GITHUB_URL GITEA_URL PUBLIC_BASE_URL"
    );
    let source = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    ensure!(!output.exists(), "use a new output directory");
    ensure!(
        fs::metadata(&source)?.len() <= MAX_BUNDLE_BYTES as u64,
        "asset ZIP too large"
    );
    let content = fs::read(source)?;
    ensure!(
        content.starts_with(b"PK\x03\x04"),
        "asset file is not a ZIP"
    );
    let sha256 = digest(&content);
    let public = args[5].trim_end_matches('/');
    let manifest = Manifest {
        schema: 1,
        version: args[2].clone(),
        sources: Sources {
            github: args[3].clone(),
            gitea: args[4].clone(),
            farm_service: format!("{public}/v1/assets/{sha256}/assets.zip"),
        },
        sha256,
        size_bytes: content.len() as u64,
    };
    manifest.validate()?;
    fs::create_dir_all(&output)?;
    fs::write(output.join("assets.zip"), content)?;
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    AssetBundle::load(&output.join("manifest.json"))?;
    println!(
        "prepared {} ({} bytes, SHA256 {})",
        manifest.version, manifest.size_bytes, manifest.sha256
    );
    Ok(())
}

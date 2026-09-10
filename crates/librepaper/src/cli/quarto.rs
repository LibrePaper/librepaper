//! Saved Quarto artifact operations. Import never starts a computation engine.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use clap::Subcommand;
use serde_json::{json, Value};

use crate::http::{get_as, post_json_as, text, Credentials};
use crate::quarto::{
    BlobUpload, ComputationEvidence, ProvenanceKind, PublishRequest, Verification,
};

#[derive(Subcommand)]
pub enum QuartoCommand {
    /// Inspect source cells and fingerprints without running code
    Inspect {
        file: PathBuf,
        /// Project-relative identity of this source file
        #[arg(long)]
        main: Option<String>,
    },
    /// Import an existing HTML/PDF/DOCX output; freshness is recorded as unknown
    Import {
        id: String,
        artifact: PathBuf,
        #[arg(long)]
        key: Option<String>,
    },
    /// Upload a complete exported manifest and its base64 blob payloads
    PublishBundle {
        id: String,
        bundle: PathBuf,
        #[arg(long)]
        key: Option<String>,
    },
    /// Inspect the selected saved render for a context without rendering
    Status {
        id: String,
        #[arg(long)]
        context: Option<String>,
        #[arg(long)]
        key: Option<String>,
    },
}

async fn connection(
    id: &str,
    server: Option<String>,
    key: Option<String>,
    token: Option<String>,
) -> (String, String, Credentials) {
    let server = super::server_or_die(server);
    let key = super::link_key(&key.unwrap_or_default());
    let credentials = Credentials::new(&super::stored_token_for(&server, token.as_deref()), &key);
    let slug = super::resolve_identifier(id, &server, &key, token.as_deref()).await;
    (server, slug, credentials)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() > crate::quarto::MAX_BUNDLE_BYTES as u64 {
        return Err(format!("{} is not a bounded regular file", path.display()));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .take(crate::quarto::MAX_BUNDLE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > crate::quarto::MAX_BUNDLE_BYTES {
        return Err("input grew beyond its size limit".into());
    }
    Ok(bytes)
}

fn blob(bytes: &[u8], mime: &str) -> BlobUpload {
    BlobUpload {
        sha256: crate::quarto::sha256(bytes),
        mime: mime.into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

fn default_context(format: &str) -> String {
    let parameters = crate::quarto::parameters_sha256(&BTreeMap::new());
    let material = format!("librepaper-quarto-selection-v1\0{format}\0\0{parameters}");
    format!("ctx-{}", &crate::quarto::sha256(material.as_bytes())[..16])
}

pub async fn run(
    command: QuartoCommand,
    server: Option<String>,
    token: Option<String>,
) -> Result<(), String> {
    match command {
        QuartoCommand::Inspect { file, main } => {
            let bytes = read_bounded(&file)?;
            let source = std::str::from_utf8(&bytes).map_err(|_| "Quarto source must be UTF-8")?;
            let main = main.unwrap_or_else(|| {
                file.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            });
            let document = crate::quarto::parse_qmd(source, &main);
            let cells: Vec<_> = document.cells.iter().map(|cell| json!({
                "id":cell.id,"label":cell.label,"language":cell.language,"options":cell.options,
                "source":cell.source,"source_sha256":cell.source_sha256,
                "start_line":cell.start_line,"end_line":cell.end_line
            })).collect();
            let context = crate::quarto::computation_fingerprint(&document, &main, &[], None);
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"schema":1,"main":main,"cells":cells,
                "computation_sha256":context,"diagnostics":document.diagnostics}))
                .map_err(|error| error.to_string())?
            );
        }
        QuartoCommand::Status { id, context, key } => {
            let context = context.unwrap_or_else(|| default_context("html"));
            if context.is_empty()
                || !context
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            {
                return Err("invalid context ID".into());
            }
            let (server, slug, who) = connection(&id, server, key, token).await;
            let (status, result) = get_as(
                &format!("{server}/api/documents/{slug}/quarto/bundles/selected/{context}"),
                &who,
                Duration::from_secs(30),
            )
            .await?;
            if status != 200 {
                return Err(format!(
                    "saved Quarto output ({status}): {}",
                    crate::http::detail_of(&result)
                ));
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
            );
        }
        QuartoCommand::PublishBundle { id, bundle, key } => {
            let request: PublishRequest = serde_json::from_slice(&read_bounded(&bundle)?)
                .map_err(|error| format!("invalid bundle: {error}"))?;
            let (server, slug, who) = connection(&id, server, key, token).await;
            if request.manifest.document_id != slug {
                return Err("bundle belongs to another document".into());
            }
            publish(&server, &slug, &who, request).await?;
        }
        QuartoCommand::Import { id, artifact, key } => {
            let artifact = artifact
                .canonicalize()
                .map_err(|error| format!("artifact: {error}"))?;
            let root = artifact
                .parent()
                .ok_or("artifact needs a parent directory")?;
            let name = artifact
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("artifact needs a UTF-8 filename")?;
            let format = match artifact
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "html" | "htm" => "html",
                "pdf" => "pdf",
                "docx" => "docx",
                _ => return Err("import supports existing HTML, PDF, or DOCX artifacts".into()),
            };
            let bytes = read_bounded(&artifact)?;
            let imported = crate::local::quarto::import_artifact(root, name, format)?;
            let (server, slug, who) = connection(&id, server, key, token).await;
            let (status, source) = get_as(
                &format!("{server}/api/documents/{slug}/snapshot"),
                &who,
                Duration::from_secs(30),
            )
            .await?;
            if status != 200 {
                return Err(format!(
                    "source ({status}): {}",
                    crate::http::detail_of(&source)
                ));
            }
            if text(&source, "format") != "quarto" {
                return Err("import target must be a Quarto source document".into());
            }
            let mut manifest = imported.to_storage_manifest(&slug, "");
            manifest.source.main = text(&source, "main");
            if manifest.source.main.is_empty() {
                manifest.source.main = "main.qmd".into();
            }
            manifest.source.tree_sha256 = None;
            manifest.source.verification = Verification::Imported;
            manifest.provenance.kind = ProvenanceKind::Imported;
            manifest.provenance.computation = ComputationEvidence::NoExecution;
            manifest.context.id = default_context(format);
            let mime = match format {
                "html" => "text/html",
                "pdf" => "application/pdf",
                _ => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            };
            if let Some(descriptor) = manifest.artifact.as_mut() {
                descriptor.mime = mime.into();
            }
            let mut blobs = vec![blob(&bytes, mime)];
            for asset in &manifest.assets {
                let path = root
                    .join(&asset.path)
                    .canonicalize()
                    .map_err(|error| format!("asset {}: {error}", asset.path))?;
                if !path.starts_with(root) {
                    return Err("imported dependency escapes the artifact directory".into());
                }
                let bytes = read_bounded(&path)?;
                if crate::quarto::sha256(&bytes) != asset.sha256 {
                    return Err("artifact dependency changed during import".into());
                }
                if !blobs.iter().any(|blob| blob.sha256 == asset.sha256) {
                    blobs.push(blob(&bytes, &asset.mime));
                }
            }
            let (status, selected) = get_as(
                &format!(
                    "{server}/api/documents/{slug}/quarto/bundles/selected/{}",
                    manifest.context.id
                ),
                &who,
                Duration::from_secs(30),
            )
            .await?;
            let generation = match status {
                404 => selected
                    .get("generation")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                200 => selected
                    .get("selection")
                    .and_then(|value| value.get("generation"))
                    .and_then(Value::as_u64)
                    .ok_or("invalid selected bundle response")?,
                _ => return Err(format!("cannot read bundle selection ({status})")),
            };
            if let Some(previous) = selected.get("manifest") {
                let mut known = std::collections::BTreeSet::new();
                if let Some(digest) = previous.pointer("/artifact/sha256").and_then(Value::as_str) {
                    known.insert(digest);
                }
                for asset in previous
                    .get("assets")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(digest) = asset.get("sha256").and_then(Value::as_str) {
                        known.insert(digest);
                    }
                }
                blobs.retain(|blob| !known.contains(blob.sha256.as_str()));
            }
            publish(
                &server,
                &slug,
                &who,
                PublishRequest {
                    manifest,
                    blobs,
                    select: true,
                    expected_generation: Some(generation),
                },
            )
            .await?;
        }
    }
    Ok(())
}

async fn publish(
    server: &str,
    slug: &str,
    who: &Credentials,
    request: PublishRequest,
) -> Result<(), String> {
    request
        .manifest
        .validate()
        .map_err(|error| error.to_string())?;
    let (status, result) = post_json_as(
        &format!("{server}/api/documents/{slug}/quarto/bundles"),
        &json!(request),
        who,
        Duration::from_secs(300),
    )
    .await?;
    if status == 409 {
        // Selection uses a separate generation fence. Losing it does not
        // undo a complete immutable bundle already committed by this upload.
        let (saved_status, saved) = get_as(
            &format!(
                "{server}/api/documents/{slug}/quarto/bundles/{}",
                request.manifest.render_id
            ),
            who,
            Duration::from_secs(30),
        )
        .await?;
        if saved_status == 200
            && serde_json::from_value::<crate::quarto::BundleManifest>(saved)
                .ok()
                .as_ref()
                == Some(&request.manifest)
        {
            println!(
                "saved Quarto render {}; a newer output remains selected",
                request.manifest.render_id
            );
            return Ok(());
        }
    }
    if status != 201 && status != 200 {
        return Err(format!(
            "publish Quarto output ({status}): {}",
            crate::http::detail_of(&result)
        ));
    }
    println!("saved Quarto render {}", text(&result, "render_id"));
    Ok(())
}

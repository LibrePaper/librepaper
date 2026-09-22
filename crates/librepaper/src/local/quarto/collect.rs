//! Turning what a render left on disk into a bundle.
//!
//! Deliberately separate from execution, and this module is that separation
//! made structural: a Quarto process that exits zero may still have produced
//! output with incomplete cell coverage, a missing artifact, or resources it
//! references and did not write. Deciding that is reading, not running, and
//! it is the half that changes when Quarto's output does.

use super::*;

/// The test-facing entry point: a collection with no declared
/// dependencies.
#[cfg(test)]
pub(crate) fn collect_bundle(
    project: &Path,
    options: &QuartoJobOptions,
    output: &Path,
    inventory: &SourceInventory,
    quarto_version: Option<String>,
    started: &str,
) -> Result<QuartoBundle, String> {
    collect_bundle_with_dependencies(
        project,
        options,
        output,
        inventory,
        quarto_version,
        started,
        &[],
    )
}

pub(crate) fn collect_bundle_with_dependencies(
    project: &Path,
    options: &QuartoJobOptions,
    output: &Path,
    inventory: &SourceInventory,
    quarto_version: Option<String>,
    // `started` is when the render began, RFC 3339, read by the caller that
    // spawned it. Not derived here: collection happens after the render, so
    // a clock read here is a completion time however it is named.
    started: &str,
    dependencies: &[String],
) -> Result<QuartoBundle, String> {
    let main_path = project.join(&options.main);
    let source =
        std::fs::read_to_string(&main_path).map_err(|e| format!("read {}: {e}", options.main))?;
    let parsed_source = crate::quarto::parse_qmd(&source, &options.main);
    let cells: Vec<QuartoCell> = parsed_source
        .cells
        .iter()
        .map(|cell| QuartoCell {
            id: cell.id.clone(),
            source_path: options.main.clone(),
            label: cell.label.clone(),
            source_sha256: cell.source_sha256.clone(),
            coverage: "unavailable".into(),
            outputs: Vec::new(),
        })
        .collect();
    let mut assets = Vec::new();
    let mut artifact = None;
    let mut files = Vec::new();
    walk_files(output, output, &mut files)?;
    if files.len() > MAX_QUARTO_OUTPUT_FILES {
        return Err("Quarto output contains too many files".into());
    }
    let mut total_bytes = 0usize;
    for relative in files {
        let metadata = std::fs::metadata(output.join(&relative))
            .map_err(|e| format!("stat output {relative}: {e}"))?;
        let size =
            usize::try_from(metadata.len()).map_err(|_| "Quarto output file is too large")?;
        total_bytes = total_bytes
            .checked_add(size)
            .ok_or("Quarto output is too large")?;
        if total_bytes > MAX_QUARTO_OUTPUT_BYTES {
            return Err("Quarto output exceeds its aggregate size limit".into());
        }
        let bytes = std::fs::read(output.join(&relative))
            .map_err(|e| format!("read output {relative}: {e}"))?;
        let digest = sha256(&bytes);
        if is_artifact(
            &relative,
            &options.main,
            &options.format,
            options.render_scope,
        ) {
            artifact = Some(QuartoArtifact {
                kind: options.format.clone(),
                entrypoint: relative.clone(),
                sha256: digest,
                size: bytes.len() as u64,
            });
        } else if relative != ".librepaper-quarto-cells.tsv" {
            assets.push(QuartoAsset {
                path: relative.clone(),
                sha256: digest,
                mime: crate::results::canonical_mime(&relative, mime_for(&relative)).into(),
                size: bytes.len() as u64,
            });
        }
    }
    if options.render_scope == protocol::QuartoRenderScope::Project && artifact.is_none() {
        if let Some(index) = assets.iter().position(|asset| asset.path == "index.html") {
            let index = assets.remove(index);
            artifact = Some(QuartoArtifact {
                kind: options.format.clone(),
                entrypoint: index.path,
                sha256: index.sha256,
                size: index.size,
            });
        }
    }
    if matches!(options.format.as_str(), "html" | "revealjs") {
        let Some(artifact_descriptor) = artifact.as_ref() else {
            return Err("Quarto HTML render did not produce an artifact".into());
        };
        let artifact_bytes = std::fs::read(output.join(&artifact_descriptor.entrypoint))
            .map_err(|error| format!("read Quarto artifact: {error}"))?;
        let references = referenced_resource_closure(
            output,
            Some(project),
            Some(&inventory.files),
            &artifact_descriptor.entrypoint,
            &artifact_bytes,
        )?;
        assets.retain(|asset| references.contains(&asset.path));
        for relative in references {
            if assets.iter().any(|asset| asset.path == relative) || output.join(&relative).is_file()
            {
                continue;
            }
            let path = project.join(&relative);
            let bytes = std::fs::read(&path)
                .map_err(|error| format!("read Quarto dependency {relative}: {error}"))?;
            assets.push(QuartoAsset {
                path: relative.clone(),
                sha256: sha256(&bytes),
                mime: crate::results::canonical_mime(&relative, mime_for(&relative)).into(),
                size: bytes.len() as u64,
            });
        }
    }
    let has_manifest = output.join(".librepaper-quarto-manifest.json").is_file()
        || output.join(".librepaper-quarto-cells.tsv").is_file();
    let mut cells = cells;
    let mut coverage = QuartoCoverage {
        full_artifact: artifact.is_some(),
        cell_outputs: if has_manifest {
            "captured".into()
        } else {
            "unavailable".into()
        },
        ..Default::default()
    };
    for cell in &mut cells {
        if has_manifest {
            cell.coverage = "unmapped".into();
            coverage.ambiguous += 1;
        } else {
            cell.coverage = "unavailable".into();
            coverage.unavailable += 1;
        }
    }
    if has_manifest {
        coverage.cell_outputs = "partial".into();
    }
    let parameters_sha256 = crate::results::parameters_sha256(&options.parameters);
    let profiles: Vec<String> = options.profile.iter().cloned().collect();
    let format_name = options.format.as_str();
    // Keep the durable computation identity sensitive to every shared input
    // supplied for this invocation. The path/hash records are public source
    // identity only; private linked-project files remain in provenance.
    let computation_sha256 = computation_fingerprint(
        &source,
        &options.main,
        format_name,
        &profiles,
        Some(&parameters_sha256),
        dependencies,
    );
    let context_material = match options.render_scope {
        protocol::QuartoRenderScope::Document => format!(
            "librepaper-quarto-selection-v1\0{format_name}\0{}\0{parameters_sha256}",
            profiles.join("\0")
        ),
        protocol::QuartoRenderScope::Project => format!(
            "librepaper-quarto-selection-v2\0project\0{format_name}\0{}\0{parameters_sha256}",
            profiles.join("\0")
        ),
    };
    let context_id = format!("ctx-{}", &sha256(context_material.as_bytes())[..16]);
    let now = timestamp();
    Ok(QuartoBundle {
        schema: crate::results::BUNDLE_SCHEMA.into(),
        render_id: opaque_id(),
        source: QuartoSource {
            revision: None,
            tree_sha256: options.shared_tree_sha256.clone(),
            main: options.main.clone(),
            verification: if options.execution_mode
                == protocol::QuartoExecutionMode::IsolatedSnapshot
            {
                "isolated-snapshot-verified"
            } else if options.shared_tree_sha256.is_some() {
                "working-tree-verified"
            } else {
                "working-tree-unverified"
            }
            .into(),
        },
        context: QuartoContext {
            id: context_id,
            fingerprint_version: 1,
            computation_sha256,
            format: options.format.clone(),
            profiles,
            parameters_sha256,
        },
        provenance: QuartoProvenance {
            kind: "managed-local-render".into(),
            quarto_version,
            collector_version: protocol::QUARTO_COLLECTOR_VERSION.into(),
            policy: policy_name(options.policy).into(),
            computation: match options.policy {
                QuartoRenderPolicy::RefreshComputations => "refresh-requested",
                QuartoRenderPolicy::Frozen => "frozen-results",
                QuartoRenderPolicy::ProjectDefaults => "cache-use-unknown",
            }
            .into(),
            external_inputs: if options.data_inputs.is_empty() {
                "not-fully-observed"
            } else {
                "unknown"
            }
            .into(),
            started_at: started.to_string(),
            completed_at: now,
        },
        artifact,
        cells,
        assets,
        inline_results: Vec::new(),
        diagnostics: parsed_source.diagnostics,
        coverage,
    })
}

/// Import an existing render without invoking Quarto. With no source bytes
/// available, the full artifact remains useful but cell association and
/// freshness are explicitly unknown.
#[cfg(test)]
pub(crate) fn import_artifact(
    artifact_root: &Path,
    entrypoint: &str,
    format: &str,
) -> Result<QuartoBundle, String> {
    if !protocol::safe_relative_path(entrypoint) {
        return Err("unsafe imported artifact path".into());
    }
    if !matches!(format, "html" | "pdf" | "docx" | "revealjs") {
        return Err(format!("unsupported imported format: {format}"));
    }
    let artifact = artifact_root.join(entrypoint);
    let bytes = read_file_bounded(&artifact, "imported artifact")?;
    if bytes.len() > MAX_QUARTO_OUTPUT_BYTES {
        return Err("imported artifact exceeds its size limit".into());
    }
    let mut assets = Vec::new();
    let mut files = Vec::new();
    walk_files(artifact_root, artifact_root, &mut files)?;
    if files.len() > MAX_QUARTO_OUTPUT_FILES {
        return Err("imported artifact directory contains too many files".into());
    }
    // An import is allowed to see only files explicitly referenced by the
    // artifact. A render directory can sit beside private data, source, or
    // credentials; inventorying every sibling would publish those by
    // accident. HTML references are normalized and traversal is rejected.
    let references = if format == "html" {
        referenced_resource_closure(artifact_root, None, None, entrypoint, &bytes)?
    } else {
        BTreeSet::new()
    };
    let mut total_bytes = bytes.len();
    for relative in files {
        if relative == entrypoint {
            continue;
        }
        if !references.contains(&relative) {
            continue;
        }
        let bytes = read_file_bounded(
            &artifact_root.join(&relative),
            &format!("imported asset {relative}"),
        )?;
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or("imported artifact exceeds its aggregate size limit")?;
        if total_bytes > MAX_QUARTO_OUTPUT_BYTES {
            return Err("imported artifact exceeds its aggregate size limit".into());
        }
        let mime = crate::results::canonical_mime(&relative, mime_for(&relative)).to_string();
        assets.push(QuartoAsset {
            path: relative,
            sha256: sha256(&bytes),
            mime,
            size: bytes.len() as u64,
        });
    }
    let digest = sha256(&bytes);
    let context_material = format!("librepaper-quarto-selection-v1\0{format}\0\0");
    let context = sha256(context_material.as_bytes());
    Ok(QuartoBundle {
        schema: crate::results::BUNDLE_SCHEMA.into(),
        render_id: opaque_id(),
        source: QuartoSource {
            revision: None,
            tree_sha256: None,
            main: entrypoint.into(),
            verification: "imported-unknown".into(),
        },
        context: QuartoContext {
            id: format!("ctx-{}", &context[..16]),
            fingerprint_version: 1,
            computation_sha256: context.clone(),
            format: format.into(),
            profiles: Vec::new(),
            parameters_sha256: crate::results::parameters_sha256(&BTreeMap::new()),
        },
        provenance: QuartoProvenance {
            kind: "imported-artifact".into(),
            quarto_version: None,
            collector_version: protocol::QUARTO_COLLECTOR_VERSION.into(),
            policy: "import-existing-output".into(),
            computation: "unknown".into(),
            external_inputs: "unknown".into(),
            started_at: timestamp(),
            completed_at: timestamp(),
        },
        artifact: Some(QuartoArtifact {
            kind: format.into(),
            entrypoint: entrypoint.into(),
            sha256: digest,
            size: bytes.len() as u64,
        }),
        cells: Vec::new(),
        assets,
        inline_results: Vec::new(),
        diagnostics: Vec::new(),
        coverage: QuartoCoverage {
            full_artifact: true,
            cell_outputs: "unknown".into(),
            unavailable: 1,
            ..Default::default()
        },
    })
}

pub(crate) fn referenced_resource_closure(
    root: &Path,
    fallback: Option<&Path>,
    fallback_allowed: Option<&[String]>,
    entrypoint: &str,
    artifact: &[u8],
) -> Result<BTreeSet<String>, String> {
    let mut references = BTreeSet::new();
    let mut seen = BTreeMap::new();
    let mut pending = vec![(entrypoint.to_string(), artifact.to_vec())];
    let mut total_bytes = artifact.len();
    while let Some((current, bytes)) = pending.pop() {
        for (reference, kind) in referenced_resources(&bytes, &current) {
            if seen
                .get(&reference)
                .is_some_and(|previous| *previous == ReferenceKind::Resource || *previous == kind)
            {
                continue;
            }
            seen.insert(reference.clone(), kind);
            let path = root.join(&reference);
            let path = if is_regular_file(&path) {
                path
            } else if let Some(fallback) = fallback {
                let fallback_path = fallback.join(&reference);
                let allowed = fallback_allowed
                    .is_none_or(|files| files.iter().any(|file| file == &reference));
                if allowed && is_regular_file(&fallback_path) {
                    fallback_path
                } else if kind == ReferenceKind::Navigation {
                    // A link to another page may point at a page outside the
                    // submitted render closure.  Keep it as navigation when
                    // present in the output, but never read an unshared local
                    // page merely because it was linked from HTML.
                    seen.remove(&reference);
                    continue;
                } else {
                    return Err(format!(
                        "required artifact dependency {reference} is missing"
                    ));
                }
            } else if kind == ReferenceKind::Navigation {
                seen.remove(&reference);
                continue;
            } else {
                return Err(format!(
                    "required artifact dependency {reference} is missing"
                ));
            };
            references.insert(reference.clone());
            let extension = Path::new(&reference)
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.to_ascii_lowercase());
            if matches!(extension.as_deref(), Some("css" | "html" | "htm")) {
                let dependency =
                    read_file_bounded(&path, &format!("read artifact dependency {reference}"))?;
                total_bytes = total_bytes
                    .checked_add(dependency.len())
                    .ok_or("imported artifact dependency closure is too large")?;
                if total_bytes > MAX_QUARTO_OUTPUT_BYTES {
                    return Err("imported artifact dependency closure is too large".into());
                }
                pending.push((reference, dependency));
            } else {
                let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                    format!("required artifact dependency {reference}: {error}")
                })?;
                total_bytes = total_bytes
                    .checked_add(metadata.len() as usize)
                    .ok_or("imported artifact dependency closure is too large")?;
                if total_bytes > MAX_QUARTO_OUTPUT_BYTES {
                    return Err("imported artifact dependency closure is too large".into());
                }
            }
        }
    }
    Ok(references)
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReferenceKind {
    Navigation,
    Resource,
}

fn referenced_resources(bytes: &[u8], current: &str) -> BTreeMap<String, ReferenceKind> {
    let text = String::from_utf8_lossy(bytes);
    let mut paths = BTreeMap::new();
    for marker in ["src=", "href=", "url("] {
        let mut rest = text.as_ref();
        while let Some(index) = rest.find(marker) {
            let marker_offset = text.len() - rest.len() + index;
            rest = &rest[index + marker.len()..];
            let rest_trimmed = rest.trim_start();
            let quote = rest_trimmed
                .as_bytes()
                .first()
                .copied()
                .filter(|byte| *byte == b'\'' || *byte == b'\"');
            let value = if let Some(quote) = quote {
                let body = &rest_trimmed[1..];
                let end = body.find(char::from(quote)).unwrap_or(body.len());
                &body[..end]
            } else {
                let end = rest_trimmed
                    .find(['\"', '\'', ')', ' ', '\t', '\r', '\n'])
                    .unwrap_or(rest_trimmed.len());
                &rest_trimmed[..end]
            };
            let ignored = value.starts_with('#')
                || value.contains("://")
                || value.starts_with("data:")
                || value.starts_with("mailto:");
            if !ignored {
                let clean = value
                    .split('?')
                    .next()
                    .unwrap_or(value)
                    .split('#')
                    .next()
                    .unwrap_or(value)
                    .trim();
                if let Some(path) = resolve_resource_path(current, clean) {
                    let kind = if marker == "href="
                        && html_tag_name(&text, marker_offset).is_some_and(|tag| {
                            tag.eq_ignore_ascii_case("a") || tag.eq_ignore_ascii_case("area")
                        }) {
                        ReferenceKind::Navigation
                    } else {
                        ReferenceKind::Resource
                    };
                    paths
                        .entry(path)
                        .and_modify(|existing| {
                            if kind == ReferenceKind::Resource {
                                *existing = kind;
                            }
                        })
                        .or_insert(kind);
                }
            }
            rest = rest_trimmed.get(value.len()..).unwrap_or_default();
        }
    }
    paths
}

fn html_tag_name(text: &str, marker_offset: usize) -> Option<&str> {
    let open = text[..marker_offset].rfind('<')?;
    if text[..marker_offset]
        .rfind('>')
        .is_some_and(|close| close > open)
    {
        return None;
    }
    let start = open + 1;
    let tag = text[start..marker_offset].trim_start();
    let end = tag
        .find(|character: char| character.is_whitespace() || character == '>')
        .unwrap_or(tag.len());
    let name = &tag[..end];
    (!name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphabetic()))
    .then_some(name)
}

pub(crate) fn resolve_resource_path(current: &str, reference: &str) -> Option<String> {
    if reference.is_empty() || reference.starts_with('/') {
        return None;
    }
    let mut components: Vec<String> = Path::new(current)
        .parent()
        .map(|parent| {
            parent
                .to_string_lossy()
                .split('/')
                .filter(|component| !component.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    for component in reference.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            value => components.push(value.to_owned()),
        }
    }
    let path = components.join("/");
    protocol::safe_relative_path(&path).then_some(path)
}

//! The bridge to the catalogue: reading a room's comments, manifest and
//! renderings out of it when the room loads, and writing them back.

use super::*;

/// Load the mutable annotation state from SQLite.  The JSON room object is
/// retained only for isolated legacy fixtures; a catalogue-backed room never
/// consults it, so a restart has one authoritative source for comments and
/// replies.
pub(super) fn load_catalog_comments(
    catalog: &crate::storage::catalog::Catalog,
    slug: &str,
) -> crate::storage::catalog::CatalogResult<(i64, Vec<Comment>)> {
    let rows = catalog.comments(slug, None, 500)?;
    let mut comments = Vec::with_capacity(rows.len());
    let mut seq = 0;
    for row in rows {
        seq = seq.max(row.seq);
        let region = row
            .region
            .map(|raw| {
                serde_json::from_str::<Region>(&raw).map_err(|err| {
                    crate::storage::catalog::CatalogError::Invalid(format!(
                        "comment region is invalid: {err}"
                    ))
                })
            })
            .transpose()?;
        let source = row.source_path.map(|path| SourceAnchor {
            path,
            exact: row.source_exact.unwrap_or_default(),
            prefix: row.source_prefix.unwrap_or_default(),
            suffix: row.source_suffix.unwrap_or_default(),
            position: row.source_position,
        });
        let replies = catalog.replies(slug, &row.id, 100)?;
        comments.push(Comment {
            id: row.id,
            seq: row.seq,
            motivation: row.motivation,
            body: row.body,
            creator: row.creator,
            author: row.author,
            via: row.via,
            created: row.created,
            exact: row.exact,
            prefix: row.prefix,
            suffix: row.suffix,
            position: row.position,
            region,
            source,
            proposed: row.proposed,
            outcome: row.outcome,
            accept_request: row.accept_request,
            revision: row.revision,
            resolved: row.resolved,
            resolved_at: row.resolved_at,
            resolved_in: row.resolved_in,
            replies: replies
                .into_iter()
                .map(|reply| Reply {
                    id: reply.id,
                    body: reply.body,
                    creator: reply.creator,
                    author: reply.author,
                    created: reply.created,
                })
                .collect(),
        });
    }
    Ok((seq, comments))
}

pub(super) fn catalog_comment_row(
    slug: &str,
    item: &Comment,
) -> Result<crate::storage::catalog::Comment, String> {
    let region = item
        .region
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| format!("comment region is not serializable: {err}"))?;
    let (source_path, source_exact, source_prefix, source_suffix, source_position) =
        match &item.source {
            Some(source) => (
                Some(source.path.clone()),
                Some(source.exact.clone()),
                Some(source.prefix.clone()),
                Some(source.suffix.clone()),
                source.position,
            ),
            None => (None, None, None, None, None),
        };
    Ok(crate::storage::catalog::Comment {
        slug: slug.to_string(),
        id: item.id.clone(),
        seq: item.seq,
        motivation: item.motivation.clone(),
        body: item.body.clone(),
        creator: item.creator.clone(),
        author: item.author.clone(),
        via: item.via.clone(),
        created: item.created.clone(),
        exact: item.exact.clone(),
        prefix: item.prefix.clone(),
        suffix: item.suffix.clone(),
        position: item.position,
        region,
        source_path,
        source_exact,
        source_prefix,
        source_suffix,
        source_position,
        proposed: item.proposed.clone(),
        outcome: item.outcome.clone(),
        accept_request: item.accept_request.clone(),
        revision: item.revision.clone(),
        resolved: item.resolved,
        resolved_at: item.resolved_at.clone(),
        resolved_in: item.resolved_in.clone(),
    })
}

pub(super) fn request_digest(value: &Value) -> String {
    fn canonical(value: &Value, out: &mut String) {
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Value::Number(value) => out.push_str(&value.to_string()),
            Value::String(value) => {
                out.push_str(&serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into()))
            }
            Value::Array(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    canonical(value, out);
                }
                out.push(']');
            }
            Value::Object(values) => {
                let mut keys: Vec<_> = values.keys().collect();
                keys.sort();
                out.push('{');
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()));
                    out.push(':');
                    canonical(&values[key], out);
                }
                out.push('}');
            }
        }
    }
    let mut bytes = String::new();
    canonical(value, &mut bytes);
    // The hashing step -- not the canonicalization above it -- is shared with
    // every other content digest in this codebase, so a stored receipt and a
    // stored document digest are hex(sha256(...)) by the same one function
    // rather than by two copies that could drift.
    crate::document::store::digest_of_bytes(bytes.as_bytes())
}

/// Reconciles the *entire* in-memory comment list against the catalogue, one
/// row (and one reply listing) at a time -- an upsert per comment plus a
/// bounded reply read, for every comment the room currently holds, on every
/// call. Every request-serving mutation (add/delete/resolve/reply/anchor/
/// accept/reject in `room/comments.rs` and `room/suggestions.rs`) instead
/// writes only the one row it changed and never reaches this function; the
/// only production caller is `seed::seed_annotations`, which uses it to
/// write a handful of demo annotations once per document when `komodoc seed`
/// populates an empty deployment. That caller's input is small and bounded
/// by the fixed example set, so the O(comments²) catalogue traffic (this
/// runs once per comment added, each time reconciling every comment added so
/// far) costs nothing worth batching. Do not add a new caller on a request
/// path: update the one row that changed instead, the way every handler
/// above already does.
pub(super) fn save_catalog_comments(
    catalog: &crate::storage::catalog::Catalog,
    slug: &str,
    seq: &mut i64,
    comments: &mut [Comment],
) -> Result<(), String> {
    for item in comments.iter_mut() {
        let current = match catalog.comment(slug, &item.id) {
            Ok(row) => Some(row),
            Err(crate::storage::catalog::CatalogError::NotFound) => None,
            Err(err) => return Err(err.to_string()),
        };
        let mut row = catalog_comment_row(slug, item)?;
        if let Some(current) = current.as_ref() {
            row.seq = current.seq;
            catalog
                .update_comment(&row)
                .map_err(|err| err.to_string())?;
            item.seq = current.seq;
        } else {
            row.seq = -1;
            let inserted = catalog
                .insert_comment(&row)
                .map_err(|err| err.to_string())?;
            item.seq = inserted.seq;
            *seq = (*seq).max(inserted.seq);
        }
        let current_replies = catalog
            .replies(slug, &item.id, 100)
            .map_err(|err| err.to_string())?
            .into_iter()
            .map(|reply| Reply {
                id: reply.id,
                body: reply.body,
                creator: reply.creator,
                author: reply.author,
                created: reply.created,
            })
            .collect::<Vec<_>>();
        let desired_reply_ids: std::collections::HashSet<String> =
            item.replies.iter().map(|reply| reply.id.clone()).collect();
        for reply in &item.replies {
            let row = crate::storage::catalog::Reply {
                slug: slug.to_string(),
                comment_id: item.id.clone(),
                id: reply.id.clone(),
                body: reply.body.clone(),
                creator: reply.creator.clone(),
                author: reply.author.clone(),
                created: reply.created.clone(),
            };
            if current_replies.iter().any(|old| old.id == reply.id) {
                catalog.update_reply(&row).map_err(|err| err.to_string())?;
            } else {
                catalog.insert_reply(&row).map_err(|err| err.to_string())?;
            }
        }
        for reply in current_replies {
            if !desired_reply_ids.contains(&reply.id) {
                catalog
                    .delete_reply(slug, &item.id, &reply.id)
                    .map_err(|err| err.to_string())?;
            }
        }
    }
    // Deletions are issued by the delete operation itself.  Never infer them
    // from a room snapshot: a cold/stale room may not contain a comment another
    // process inserted, and replacing all rows would erase that concurrent
    // write.
    // comment_seq is only ever advanced by insert_comment.  Never write the
    // room's possibly stale cached value back over the authoritative counter.
    let _ = seq;
    Ok(())
}

/// Number of history entries a live room retains for hot-path operations.
/// SQLite remains the source of truth for the complete timeline; keeping the
/// tail here bounds resident memory for documents with years of checkpoints.
pub(super) const RESIDENT_CATALOG_HISTORY: u32 = 64;

pub(super) fn load_catalog_manifest(
    catalog: &crate::storage::catalog::Catalog,
    slug: &str,
) -> crate::storage::catalog::CatalogResult<Manifest> {
    let rows = catalog.checkpoints_tail(slug, RESIDENT_CATALOG_HISTORY)?;
    Manifest::from_catalog_rows(rows).map_err(crate::storage::catalog::CatalogError::Invalid)
}

pub(super) fn load_catalog_history(
    catalog: &crate::storage::catalog::Catalog,
    slug: &str,
) -> crate::storage::catalog::CatalogResult<Vec<Checkpoint>> {
    let rows = load_catalog_checkpoint_rows(catalog, slug)?;
    Manifest::from_catalog_rows(rows)
        .map(|manifest| manifest.checkpoints)
        .map_err(crate::storage::catalog::CatalogError::Invalid)
}

pub(super) fn load_catalog_checkpoint_rows(
    catalog: &crate::storage::catalog::Catalog,
    slug: &str,
) -> crate::storage::catalog::CatalogResult<Vec<crate::storage::catalog::Checkpoint>> {
    // Catalog reads are deliberately bounded.  Never load only the first
    // page and then let a later metadata write treat that prefix as the whole
    // history: doing so would silently discard every newer checkpoint.
    let mut rows = Vec::new();
    let mut after = None;
    loop {
        let page = catalog.checkpoints(slug, after, 200)?;
        if page.is_empty() {
            break;
        }
        after = page.last().map(|row| row.seq);
        let complete = page.len() < 200;
        rows.extend(page);
        if complete {
            break;
        }
    }
    Ok(rows)
}

pub(super) fn save_catalog_manifest(
    catalog: &crate::storage::catalog::Catalog,
    slug: &str,
    previous: &Manifest,
    manifest: &Manifest,
    durable_seq: i64,
) -> Result<(), String> {
    let previous_by_sha: HashMap<_, _> = previous
        .checkpoints
        .iter()
        .map(|point| (point.sha.as_str(), point))
        .collect();
    let mut rows = Vec::new();
    for point in &manifest.checkpoints {
        let row = Manifest::catalog_row(slug, point, -1, durable_seq)
            .map_err(|error| error.to_string())?;
        // Keep labels from the staged resident manifest, but avoid requiring
        // a read transaction for each row. The catalogue method performs the
        // existence check and all inserts/updates in one write transaction.
        if !previous_by_sha.contains_key(point.sha.as_str())
            || previous_by_sha
                .get(point.sha.as_str())
                .is_some_and(|old| old.label != point.label)
        {
            rows.push(row);
        }
    }
    // A publication has a prepared receipt.  Keep its checkpoint descriptor
    // in that receipt until the final commit transaction; ordinary checkpoints
    // retain the direct atomic insert path.
    if catalog
        .document(slug)
        .map_err(|err| err.to_string())?
        .and_then(|document| document.pending_publication)
        .is_some()
    {
        if let Some(row) = rows.last() {
            catalog
                .stage_publication_checkpoint(slug, row)
                .map_err(|err| err.to_string())?;
        }
    } else {
        catalog
            .insert_checkpoints_atomic(&rows)
            .map_err(|err| err.to_string())?;
    }
    // A resident-tail snapshot intentionally omits older rows.  Absence from
    // `previous`/`manifest` therefore never means deletion; destructive
    // retention is an explicit catalogue operation with its own policy.
    Ok(())
}

#[allow(dead_code)]
pub(super) fn save_catalog_rendering(
    catalog: &crate::storage::catalog::Catalog,
    slug: &str,
    tree_sha: &str,
    synctex: bool,
    size: i64,
    actor: Option<(&str, &str, &str)>,
) -> Result<(), String> {
    save_catalog_rendering_with_authority(
        catalog,
        slug,
        tree_sha,
        synctex,
        size,
        actor.map(|actor| crate::storage::catalog::MutationAuthority {
            account_id: actor.0,
            owner_key: actor.1,
            generation: actor.2,
            link_hash: "",
            policy_editor: true,
            automation: false,
            unowned_publisher: false,
        }),
    )
}

pub(super) fn save_catalog_rendering_with_authority(
    catalog: &crate::storage::catalog::Catalog,
    slug: &str,
    tree_sha: &str,
    synctex: bool,
    size: i64,
    actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
) -> Result<(), String> {
    let previous = catalog
        .rendering(slug, tree_sha)
        .map_err(|err| err.to_string())?;
    let rendering = crate::storage::catalog::Rendering {
        slug: slug.to_string(),
        tree_sha: tree_sha.to_string(),
        at: timestamp(),
        backend: previous
            .as_ref()
            .map(|row| row.backend.clone())
            .unwrap_or_default(),
        engine: previous
            .as_ref()
            .map(|row| row.engine.clone())
            .unwrap_or_default(),
        release: previous
            .as_ref()
            .map(|row| row.release.clone())
            .unwrap_or_default(),
        tools: previous
            .as_ref()
            .map(|row| row.tools.clone())
            .unwrap_or_default(),
        bytes: if synctex {
            previous.as_ref().map(|row| row.bytes).unwrap_or_default()
        } else {
            size
        },
        synctex: synctex || previous.as_ref().is_some_and(|row| row.synctex),
        synctex_bytes: if synctex {
            size
        } else {
            previous
                .as_ref()
                .map(|row| row.synctex_bytes)
                .unwrap_or_default()
        },
    };
    if let Some(actor) = actor {
        catalog
            .publish_rendering_with_authority(&rendering, actor)
            .map(|_| ())
            .map_err(|err| err.to_string())
    } else {
        catalog
            .publish_rendering(&rendering)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod request_digest_tests {
    use super::request_digest;
    use serde_json::json;

    /// `request_digest` is what makes a stored comment/suggestion receipt
    /// idempotent: the same canonical bytes must hash the same way forever,
    /// or a retried request could be charged or applied twice after a
    /// canonicalization change nobody meant to make. These fix the digest of
    /// a handful of representative values so any future edit to `canonical`
    /// (key order, string escaping, or number formatting) that is not
    /// byte-for-byte compatible fails loudly here rather than silently
    /// breaking retry matching against already-stored receipts.
    #[test]
    fn nested_object_keys_are_sorted_before_hashing() {
        // Two JSON objects with the same keys inserted in a different order
        // must canonicalize (and therefore hash) identically.
        let forward = json!({"a": 1, "b": 2, "z": 3});
        let reversed = json!({"z": 3, "b": 2, "a": 1});
        assert_eq!(request_digest(&forward), request_digest(&reversed));
        assert_eq!(
            request_digest(&forward),
            "329d4b5a274b8081ef038bb735813dc3082cf6d95855f8029c9cd8432168c112"
        );
    }

    #[test]
    fn string_escapes_are_included_in_the_canonical_bytes() {
        let value = json!({"body": "quote \" and backslash \\ and newline \n"});
        assert_eq!(
            request_digest(&value),
            "dde0899efbe2c590bf2985f9ff257a896714a5907f558f811d0006ee7e998ee5"
        );
    }

    /// Distinguishes an integer from an equal-valued float: `1` and `1.0`
    /// must not collide, because a stored receipt's request payload keeps
    /// whatever numeric shape the client sent.
    #[test]
    fn integer_and_float_of_equal_value_hash_differently() {
        let integer = json!({"position": 1});
        let float = json!({"position": 1.0});
        assert_ne!(request_digest(&integer), request_digest(&float));
    }

    #[test]
    fn nested_arrays_and_objects_canonicalize_deterministically() {
        let value = json!({
            "outer": {"z": [1, 2, {"inner": "value"}], "a": null},
            "flag": true,
        });
        // Recomputing twice from equivalent but differently-ordered input
        // must agree; this is the property the receipt matching relies on.
        let same_value_reordered = json!({
            "flag": true,
            "outer": {"a": null, "z": [1, 2, {"inner": "value"}]},
        });
        assert_eq!(
            request_digest(&value),
            request_digest(&same_value_reordered)
        );
    }
}

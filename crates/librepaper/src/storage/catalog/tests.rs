use super::{
    Account, AnnotationAuthority, Catalog, CatalogError, Checkpoint, Comment, JournalPreparation,
    DocumentId, JournalSegment, Link, MutationAuthority, NewDocument, ObjectId, OperationKind,
    OperationScope, Reply, SourceHistoryObject, SourceHistoryRecord,
    UnixMillis, V2Operation, V2OperationInput,
};
use sha2::Digest;
use std::sync::Arc;
use crate::storage::blob::BlobStore;

pub(super) fn account() -> Account {
    Account {
        id: "acct-1".into(),
        provider: "github".into(),
        handle: "alice".into(),
        name: "Alice".into(),
        email: "alice@example.test".into(),
        first_seen: "2026-01-01T00:00:00.000Z".into(),
        last_seen: "2026-01-01T00:00:00.000Z".into(),
        plan: "free".into(),
        status: "active".into(),
        session_generation: "generation-1".into(),
        erasure_cursor: None,
    }
}

pub(super) fn annotation(id: &str, motivation: &str) -> Comment {
    Comment {
        slug: "doc".into(),
        id: id.into(),
        seq: 0,
        motivation: motivation.into(),
        body: "words".into(),
        creator: "Alice".into(),
        author: "acct-1".into(),
        via: String::new(),
        created: "2026-01-01T00:00:00.000Z".into(),
        publication_id: "publication-1".into(),
        exact: "text".into(),
        prefix: String::new(),
        suffix: String::new(),
        position: None,
        point: false,
        color: None,
        region: None,
        quarto_output: None,
        source_path: None,
        source_exact: None,
        source_prefix: None,
        source_suffix: None,
        source_position: None,
        proposed: (motivation == "editing").then(|| "better words".into()),
        pass: String::new(),
        outcome: String::new(),
        accept_request: String::new(),
        // Ordinary annotation fixtures are not anchored to a source
        // checkpoint. Tests that exercise retention protection install a
        // real checkpoint and set this field explicitly.
        revision: String::new(),
        resolved: false,
        resolved_at: None,
        resolved_in: String::new(),
    }
}

#[test]
fn annotation_writes_recheck_the_account_session_generation() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let current = AnnotationAuthority {
        account_id: "acct-1",
        generation: "generation-1",
        ..Default::default()
    };
    let comment_request = crate::util::new_request_key();
    let suggestion_request = crate::util::new_request_key();
    let accept_request = crate::util::new_request_key();
    let comment_digest = "1".repeat(64);
    let suggestion_digest = "2".repeat(64);
    let accept_digest = "3".repeat(64);
    let now = crate::util::now_millis();
    let comment = catalog
        .insert_comment_request_authorized(
            &annotation("comment-1", "commenting"),
            &comment_request,
            &comment_digest,
            now,
            current,
        )
        .unwrap();
    assert_eq!(comment.publication_id, "publication-1");
    assert_eq!(
        catalog.comment("doc", "comment-1").unwrap().publication_id,
        "publication-1"
    );
    catalog
        .insert_comment_request_authorized(
            &annotation("suggestion-1", "editing"),
            &suggestion_request,
            &suggestion_digest,
            now.saturating_add(1),
            current,
        )
        .unwrap();
    catalog
        .begin_suggestion_accept_authorized(
            "doc",
            "suggestion-1",
            &accept_request,
            &accept_digest,
            now.saturating_add(2),
            current,
        )
        .unwrap();
    catalog
        .stage_suggestion_accept_update_authorized(
            "doc",
            "suggestion-1",
            &accept_request,
            &accept_digest,
            &[1, 2, 3],
            current,
        )
        .unwrap();
    catalog
        .record_suggestion_accept_checkpoint(
            "doc",
            "suggestion-1",
            &accept_request,
            &accept_digest,
            "checkpoint-sha",
            "2026-01-01T00:00:03.000Z",
        )
        .unwrap();
    catalog.revoke_sessions("acct-1", "generation-2").unwrap();

    let stale = AnnotationAuthority {
        account_id: "acct-1",
        generation: "generation-1",
        ..Default::default()
    };
    let reply_request = crate::util::new_request_key();
    let reply_digest = "4".repeat(64);
    let stale_accept_request = crate::util::new_request_key();
    let mut changed = comment.clone();
    changed.body = "stale edit".into();
    assert!(catalog.update_comment_authorized(&changed, stale).is_err());
    assert!(catalog
        .insert_comment_request_authorized(
            &annotation("comment-without-receipt", "commenting"),
            "",
            "",
            3,
            stale,
        )
        .is_err());
    assert!(catalog
        .insert_reply_request_authorized(
            &Reply {
                slug: "doc".into(),
                comment_id: comment.id.clone(),
                id: "reply-1".into(),
                body: "stale reply".into(),
                creator: "Alice".into(),
                author: "acct-1".into(),
                created: "2026-01-01T00:00:01.000Z".into(),
            },
            &reply_request,
            &reply_digest,
            now.saturating_add(3),
            stale,
        )
        .is_err());
    assert!(catalog
        .insert_reply_request_authorized(
            &Reply {
                slug: "doc".into(),
                comment_id: comment.id.clone(),
                id: "reply-without-receipt".into(),
                body: "stale reply".into(),
                creator: "Alice".into(),
                author: "acct-1".into(),
                created: "2026-01-01T00:00:02.000Z".into(),
            },
            "",
            "",
            3,
            stale,
        )
        .is_err());
    assert!(catalog
        .delete_comment_authorized("doc", &comment.id, stale)
        .is_err());
    assert!(catalog
        .stage_suggestion_accept_update_authorized(
            "doc",
            "suggestion-1",
            &accept_request,
            &accept_digest,
            &[1, 2, 3],
            stale,
        )
        .is_err());
    assert!(catalog
        .begin_suggestion_accept_authorized(
            "doc",
            "suggestion-1",
            &stale_accept_request,
            &"5".repeat(64),
            now.saturating_add(4),
            stale,
        )
        .is_err());

    // Authorization belongs to preparing and staging the operation. Once its
    // document edit has landed, the service must be able to settle that
    // durable receipt even if the initiating session has since been revoked.
    let accepted = catalog
        .finish_suggestion_accept(
            "doc",
            "suggestion-1",
            &accept_request,
            &accept_digest,
            "checkpoint-sha",
            "2026-01-01T00:00:03.000Z",
        )
        .unwrap();
    assert_eq!(accepted.outcome, "accepted");

    assert_eq!(catalog.comment("doc", &comment.id).unwrap().body, "words");
    assert!(catalog.replies("doc", &comment.id, 100).unwrap().is_empty());
}

#[test]
fn reply_edit_and_delete_require_author_or_editor_and_keep_attribution() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut editor = account();
    editor.id = "acct-2".into();
    editor.handle = "editor".into();
    editor.name = "Editor".into();
    editor.session_generation = "generation-2".into();
    catalog.upsert_account(&editor).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .grant("doc", "commenter", "acct-2", "2026-01-01T00:00:00.000Z")
        .unwrap();

    let owner = AnnotationAuthority {
        account_id: "acct-1",
        generation: "generation-1",
        policy_comment: true,
        ..Default::default()
    };
    let comment = catalog
        .insert_comment_request_authorized(&annotation("reply-auth", "commenting"), "", "", crate::util::now_millis(), owner)
        .unwrap();
    let reply = Reply {
        slug: "doc".into(),
        comment_id: comment.id.clone(),
        id: "reply-auth".into(),
        body: "original body".into(),
        creator: "Opaque display".into(),
        author: "visitor:opaque-author".into(),
        created: "2026-01-01T00:00:01.000Z".into(),
    };
    let request = crate::util::new_request_key();
    let digest = "d".repeat(64);
    catalog
        .insert_reply_request_authorized(
            &reply,
            &request,
            &digest,
            crate::util::now_millis(),
            owner,
        )
        .unwrap();
    let commenter = AnnotationAuthority {
        account_id: "acct-2",
        generation: "generation-2",
        policy_comment: true,
        ..Default::default()
    };
    let mut forged = reply.clone();
    forged.body = "forged body".into();
    forged.creator = "Forged display".into();
    forged.author = "forged-author".into();
    assert!(catalog.update_reply_authorized(&forged, commenter).is_err());
    assert!(catalog
        .delete_reply_authorized("doc", &reply.comment_id, &reply.id, commenter)
        .is_err());

    let mut edited = reply.clone();
    edited.body = "edited body".into();
    edited.creator = "Changed display must not win".into();
    edited.author = "changed opaque key".into();
    catalog.update_reply_authorized(&edited, owner).unwrap();
    let stored = catalog.replies("doc", &comment.id, 10).unwrap().remove(0);
    assert_eq!(stored.body, "edited body");
    assert_eq!(stored.creator, "Opaque display");
    assert_eq!(stored.author, "visitor:opaque-author");
    catalog
        .delete_reply_authorized("doc", &reply.comment_id, &reply.id, owner)
        .unwrap();

    catalog
        .grant("doc", "editor", "acct-2", "2026-01-01T00:00:02.000Z")
        .unwrap();
    let mut forged_comment = comment.clone();
    forged_comment.body = "editor body".into();
    forged_comment.creator = "Erased display must stay erased".into();
    forged_comment.author = "erased-author-must-stay-erased".into();
    catalog
        .update_comment_authorized(&forged_comment, commenter)
        .unwrap();
    let stored_comment = catalog.comment("doc", &comment.id).unwrap();
    assert_eq!(stored_comment.body, "editor body");
    assert_eq!(stored_comment.creator, "Alice");
    assert_eq!(stored_comment.author, "acct-1");
}

#[test]
fn reopening_annotation_restores_protection_from_stored_source_revision() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let mut revision = attributed("revision", "Alice", Some("acct-1"));
    revision.tree_sha = fixture_tree_digest("revision");
    insert_fixture_checkpoint(&catalog, &revision);
    let owner = AnnotationAuthority {
        account_id: "acct-1",
        generation: "generation-1",
        policy_comment: true,
        ..Default::default()
    };
    let mut comment = annotation("protected", "commenting");
    comment.revision = "revision".into();
    let comment_request = crate::util::new_request_key();
    let comment_digest = "d".repeat(64);
    let stored_comment = catalog
        .insert_comment_request_authorized(
            &comment,
            &comment_request,
            &comment_digest,
            crate::util::now_millis(),
            owner,
        )
        .unwrap();
    catalog
        .with_connection(|connection| {
            connection.execute(
                "UPDATE annotations SET resolved_at=created_at+1,protected_checkpoint_id=NULL
                 WHERE document_id='storage-1' AND id='protected'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let mut reopened = stored_comment;
    reopened.body = "reopened body".into();
    catalog
        .update_comment_authorized(&reopened, owner)
        .unwrap();
    let protected: Option<String> = catalog
        .with_connection(|connection| {
            connection.query_row(
                "SELECT protected_checkpoint_id FROM annotations
                 WHERE document_id='storage-1' AND id='protected'",
                [],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(protected.as_deref(), Some("revision"));
}

#[test]
fn quota_preferences_use_optimistic_revisions_and_preserve_payload() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    assert_eq!(
        catalog
            .quota_preferences("acct-1")
            .unwrap()
            .unwrap()
            .revision,
        0
    );
    let first = catalog
        .save_quota_preferences("acct-1", 0, r#"{"version":2,"retentionProfile":"default","retentionPolicyVersion":2,"displayTimezone":"UTC","warningThresholds":[75,90],"futureField":true}"#, 10)
        .unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(first.payload, r#"{"version":2,"retentionProfile":"default","retentionPolicyVersion":2,"displayTimezone":"UTC","warningThresholds":[75,90],"futureField":true}"#);
    assert!(matches!(
        catalog.save_quota_preferences(
            "acct-1",
            0,
            r#"{"version":2,"retentionProfile":"default","retentionPolicyVersion":2,"displayTimezone":"UTC","warningThresholds":[75,90],"futureField":true}"#,
            11,
        ),
        Err(CatalogError::Conflict(_))
    ));
    let second = catalog
        .save_quota_preferences("acct-1", 1, r#"{"version":2,"retentionProfile":"default","retentionPolicyVersion":2,"displayTimezone":"UTC","warningThresholds":[75,90],"futureField":false}"#, 12)
        .unwrap();
    assert_eq!(second.revision, 2);
    let stored = catalog.quota_preferences("acct-1").unwrap().unwrap();
    assert_eq!(stored.account_id, second.account_id);
    assert_eq!(stored.revision, second.revision);
    assert_eq!(stored.payload, second.payload);
    assert_eq!(stored.updated_at, second.updated_at);
}

#[test]
fn quota_apply_marks_live_policy_for_advisory_worker() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let tree_digest = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO objects
                 (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,
                  created_at,live_root,gc_after)
                 VALUES('storage-1','quota-tree','objects/quota-tree','source_tree','available',
                        ?1,1,0,0,1,0)",
                [tree_digest],
            )?;
            Ok(())
        })
        .unwrap();
    for index in 0..3 {
        let mut point = attributed(&format!("milestone-{index}"), "alice", Some("acct-1"));
        point.seq = index + 1;
        point.tree_sha = tree_digest.into();
        point.at = format!("2026-01-01T00:00:0{index}.000Z");
        point.label = "important".into();
        point.parent = if index == 0 {
            String::new()
        } else {
            format!("milestone-{}", index - 1)
        };
        catalog.insert_checkpoint(&point).unwrap();
    }
    let payload =
        serde_json::to_string(&crate::document::quota::QuotaPreferences::default()).unwrap();
    catalog
        .save_quota_preferences_advisory("acct-1", 0, &payload, 1)
        .unwrap();
    let pass = catalog.run_retention_pass(2, 100).unwrap();
    assert!(pass.removed.is_empty());
    assert!(pass.blocked == 0);
    assert!(catalog.checkpoint("doc", "milestone-1").unwrap().is_some());
    assert!(catalog.checkpoint("doc", "milestone-2").unwrap().is_some());
    // V2 keeps protection on annotation/source rows.  Checkpoint metadata
    // reports the durable parent and does not fabricate the removed legacy
    // protection flag.
    assert_eq!(
        catalog
            .checkpoint_retention_metadata("doc", "milestone-1")
            .unwrap(),
        Some(("milestone-0".into(), false))
    );
}

#[test]
fn retention_wakes_time_only_age_policy_without_existing_candidates() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO objects
                 (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,
                  created_at,live_root,gc_after)
                 VALUES('storage-1','age-tree','objects/age-tree','source_tree','available',
                        ?1,1,0,0,1,0)",
                ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
            )?;
            Ok(())
        })
        .unwrap();
    let now = crate::util::now_millis();
    for index in 0..2 {
        let mut point = attributed(&format!("age-{index}"), "alice", Some("acct-1"));
        // Sequence zero is treated as "allocate the next sequence" by the
        // compatibility insert path.  Make both fixture rows explicit so
        // the first allocated value cannot collide with the second row.
        point.seq = index + 1;
        point.tree_sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into();
        point.at = (now - (2 - index as i64) * 1_000).to_string();
        point.parent = if index == 0 {
            String::new()
        } else {
            "age-0".into()
        };
        catalog.insert_checkpoint(&point).unwrap();
    }
    catalog
        .schedule_document_balanced(
            "doc",
            now,
            crate::document::quota::RetentionBounds::default(),
        )
        .unwrap();
    let due: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT retention_due_at FROM documents WHERE slug='doc'",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert!(due > now, "age policy was not persisted as a future wakeup");
    let eligible: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM checkpoints WHERE eligible_after IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(eligible, 0, "fresh points should not be candidates yet");
    catalog
        .schedule_due_documents(due, 64, crate::document::quota::RetentionBounds::default())
        .unwrap();
    let eligible_after: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM checkpoints WHERE eligible_after IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(eligible_after, 1);
}

#[test]
fn account_usage_charges_unique_physical_objects_and_not_tree_size() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut input = document();
    input.size = 1;
    input.counted_size = 2;
    catalog.create_document(&input).unwrap();

    // V2 accounting is held by the account/server counters and the typed
    // object closure.  Keep a tree object for the checkpoint and three
    // differently typed physical objects whose measured bytes sum to 34;
    // the logical document size remains deliberately unrelated.
    let tree_id = fixture_object_id("usage-tree");
    let tree_digest = fixture_tree_digest("usage-tree");
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO objects
                 (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at)
                 VALUES('storage-1',?1,?2,'source_tree','available',?3,3,0,0)",
                rusqlite::params![
                    tree_id,
                    format!("v2/documents/storage-1/objects/{tree_id}"),
                    tree_digest,
                ],
            )?;
            for (id, kind, bytes) in [
                (fixture_object_id("usage-chunk"), "source_chunk", 7),
                (fixture_object_id("usage-asset"), "asset", 11),
                (fixture_object_id("usage-publication"), "publication_html", 13),
            ] {
                connection.execute(
                    "INSERT INTO objects
                     (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at)
                     VALUES('storage-1',?1,?2,?3,'available',?4,?5,0,0)",
                    rusqlite::params![
                        id,
                        format!("v2/documents/storage-1/objects/{id}"),
                        kind,
                        "a".repeat(64),
                        bytes,
                    ],
                )?;
            }
            connection.execute(
                "UPDATE documents SET stored_bytes=34 WHERE id='storage-1'",
                [],
            )?;
            connection.execute(
                "UPDATE accounts SET stored_bytes=34 WHERE id='acct-1'",
                [],
            )?;
            connection.execute("UPDATE server_state SET stored_bytes=34 WHERE id=1", [])?;
            Ok(())
        })
        .unwrap();
    let point = attributed("checkpoint-a", "Alice", Some("acct-1"));
    insert_fixture_checkpoint(&catalog, &point);

    assert!(catalog.audit_v2_counters().unwrap());
    let usage = catalog.account_storage_usage("acct-1").unwrap();
    assert!(usage.physical_accounting);
    assert_eq!(usage.charged_bytes, 34);
    assert_eq!(usage.document_count, 1);
    assert_eq!(usage.checkpoint_count, 1);
    let mut physical_by_kind = catalog
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT kind,SUM(byte_length) FROM objects
                 WHERE document_id='storage-1' AND state='available'
                 GROUP BY kind ORDER BY kind",
            )?;
            let rows = statement
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap();
    physical_by_kind.sort();
    assert_eq!(
        physical_by_kind,
        vec![
            ("asset".into(), 11),
            ("publication_html".into(), 13),
            ("source_chunk".into(), 7),
            ("source_tree".into(), 3),
        ]
    );
    assert!(catalog.audit_v2_counters().unwrap());
}

#[test]
fn new_account_examples_resume_without_reenrolling_on_profile_refresh() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let pending = catalog.pending_account_examples("acct-1").unwrap();
    assert_eq!(pending.len(), 5);
    catalog.complete_account_example("acct-1", 0).unwrap();
    catalog.upsert_account(&account()).unwrap();
    assert_eq!(
        catalog.pending_account_examples("acct-1").unwrap(),
        pending[1..]
    );
    for position in 1..5 {
        catalog
            .complete_account_example("acct-1", position)
            .unwrap();
    }
    catalog.upsert_account(&account()).unwrap();
    assert!(catalog
        .pending_account_examples("acct-1")
        .unwrap()
        .is_empty());
}

pub(super) fn document() -> NewDocument {
    NewDocument {
        slug: "doc".into(),
        storage_id: "storage-1".into(),
        title: "Document".into(),
        sha: "sha".into(),
        created_at: "2026-01-01T00:00:00.000Z".into(),
        published_at: "2026-01-01T00:00:00.000Z".into(),
        updated_at: "2026-01-01T00:00:00.000Z".into(),
        example: false,
        owner_key: String::new(),
        owner_id: Some("acct-1".into()),
        status: "active".into(),
        size: 10,
        counted_size: 20,
        maintenance_reserved: 0,
        last_auto_checkpoint_at: 0,
        source_format: "markdown".into(),
        main: "README.md".into(),
    }
}

#[test]
fn source_history_gc_keeps_a_chunk_shared_by_two_retained_files() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let file_a = "a".repeat(64);
    let file_b = "b".repeat(64);
    let shared_id = fixture_object_id("source-shared");
    let shared_key = format!("v2/documents/storage-1/objects/{shared_id}");
    let recipe_key = |seed: &str| {
        let id = fixture_object_id(seed);
        (id.clone(), format!("v2/documents/storage-1/objects/{id}"))
    };
    let (recipe_a_id, recipe_a_key) = recipe_key("recipe-a");
    let (recipe_b_id, recipe_b_key) = recipe_key("recipe-b");
    let tree_a_id = fixture_object_id("tree-a");
    let tree_b_id = fixture_object_id("tree-b");
    catalog
        .with_connection(|connection| {
            for (id, key, kind, digest, bytes) in [
                (
                    shared_id.clone(),
                    shared_key.clone(),
                    "source_chunk",
                    fixture_tree_digest("shared"),
                    7_i64,
                ),
                (
                    recipe_a_id.clone(),
                    recipe_a_key.clone(),
                    "source_recipe",
                    fixture_tree_digest("recipe-a"),
                    3,
                ),
                (
                    recipe_b_id.clone(),
                    recipe_b_key.clone(),
                    "source_recipe",
                    fixture_tree_digest("recipe-b"),
                    3,
                ),
                (
                    tree_a_id.clone(),
                    format!("v2/documents/storage-1/objects/{tree_a_id}"),
                    "source_tree",
                    fixture_tree_digest("checkpoint-a"),
                    1,
                ),
                (
                    tree_b_id.clone(),
                    format!("v2/documents/storage-1/objects/{tree_b_id}"),
                    "source_tree",
                    fixture_tree_digest("checkpoint-b"),
                    1,
                ),
            ] {
                connection.execute(
                    "INSERT INTO objects
                     (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at)
                     VALUES('storage-1',?1,?2,?3,'available',?4,?5,0,0)",
                    rusqlite::params![id, key, kind, digest, bytes],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let object = SourceHistoryObject {
        object_key: shared_key,
        kind: "source_chunk".into(),
        bytes: 7,
    };
    let record = |digest: &str, recipe: &str, recipe_digest: &str| SourceHistoryRecord {
        file_digest: digest.into(),
        recipe_key: recipe.into(),
        recipe_digest: recipe_digest.into(),
        codec: 1,
        uncompressed_bytes: 7,
        recipe_bytes: 3,
        objects: vec![
            SourceHistoryObject {
                object_key: recipe.into(),
                kind: "source_recipe".into(),
                bytes: 3,
            },
            object.clone(),
        ],
    };
    let checkpoint = |sha: &str| Checkpoint {
        slug: "doc".into(),
        sha: sha.into(),
        seq: -1,
        durable_seq: 0,
        tree_sha: fixture_tree_digest(sha),
        parent: String::new(),
        at: "2026-01-01T00:00:00.000Z".into(),
        by: String::new(),
        why: "test".into(),
        source_format: "markdown".into(),
        size: 7,
        label: String::new(),
        git_commit: String::new(),
        dirty: false,
        changed: None,
        by_account: None,
    };
    let first = checkpoint("checkpoint-a");
    catalog
        .insert_checkpoints_atomic_with_sources(
            std::slice::from_ref(&first),
            None,
            &[record(&file_a, &recipe_a_key, &"c".repeat(64))],
        )
        .unwrap();
    let second = checkpoint("checkpoint-b");
    catalog
        .insert_checkpoints_atomic_with_sources(
            std::slice::from_ref(&second),
            None,
            &[record(&file_b, &recipe_b_key, &"d".repeat(64))],
        )
        .unwrap();

    // Supply the durable v2 retention evaluation that makes these historical
    // points eligible for the typed checkpoint-delete boundary.
    catalog
        .with_connection(|connection| {
            connection.execute(
                "UPDATE checkpoints SET eligible_after=0 WHERE document_id='storage-1'",
                [],
            )?;
            connection.execute(
                r#"UPDATE documents SET retention_due_at=1,
                 retention_json='{"version":1,"evaluation":{"accountRevision":0,"documentRevision":0}}'
                 WHERE id='storage-1'"#,
                [],
            )?;
            Ok(())
        })
        .unwrap();

    catalog.delete_checkpoint("doc", "checkpoint-a").unwrap();
    let pending = catalog.due_deletes(crate::util::now_millis(), 100).unwrap();
    assert!(pending
        .iter()
        .all(|entry| entry.object_key != object.object_key));

    catalog.delete_checkpoint("doc", "checkpoint-b").unwrap();
    let document_id = crate::storage::catalog::DocumentId::new("storage-1").unwrap();
    let shared_object_id = crate::storage::catalog::ObjectId::new(shared_id).unwrap();
    catalog
        .with_connection(|connection| {
            connection.execute(
                "UPDATE objects SET gc_after=0 WHERE document_id='storage-1' AND id=?1",
                [shared_object_id.as_str()],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(catalog
        .claim_v2_object_for_deletion(
            &document_id,
            &shared_object_id,
            crate::storage::catalog::UnixMillis::new(crate::util::now_millis()).unwrap(),
            crate::storage::catalog::UnixMillis::new(crate::util::now_millis()).unwrap(),
        )
        .unwrap());
    let pending = catalog.due_deletes(crate::util::now_millis(), 100).unwrap();
    assert!(pending
        .iter()
        .any(|entry| entry.object_key == object.object_key));
}

#[test]
fn source_history_writer_lease_rejects_an_object_already_queued_for_deletion() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let now = crate::util::now_millis();
    let request_key = crate::util::new_request_key();
    let operation = catalog
        .prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(DocumentId::new("storage-1").unwrap()),
                actor_key: "test-source-writer".into(),
                request_key,
                kind: OperationKind::SourcePublish,
                request_digest: "a".repeat(64),
                plan_json: r#"{"version":2,"effect":"source_publish"}"#.into(),
                expected_document_generation: None,
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: Some(UnixMillis::new(now + 120_000).unwrap()),
            },
            UnixMillis::new(now).unwrap(),
        )
        .unwrap();
    for (directory, kind) in [("chunks", "source_chunk"), ("assets", "asset")] {
        let object_id = fixture_object_id(directory);
        let object_key = format!("v2/documents/storage-1/objects/{object_id}");
        catalog
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO objects
                     (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at)
                     VALUES('storage-1',?1,?2,?3,'available',?4,7,0,?5)",
                    rusqlite::params![
                        object_id,
                        object_key,
                        kind,
                        "b".repeat(64),
                        now,
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        let object = SourceHistoryObject {
            object_key: object_key.clone(),
            kind: kind.into(),
            bytes: 7,
        };
        catalog
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE objects SET gc_after=0 WHERE document_id='storage-1' AND id=?1",
                    [&object_id],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(catalog
            .claim_v2_object_for_deletion(
                &DocumentId::new("storage-1").unwrap(),
                &ObjectId::new(object_id).unwrap(),
                UnixMillis::new(now).unwrap(),
                UnixMillis::new(now).unwrap(),
            )
            .unwrap());
        let result = catalog.begin_source_history_lease(
            "storage-1",
            operation.id.as_str(),
            &[object],
            now,
            now + 1_000,
        );
        assert!(matches!(result, Err(CatalogError::Conflict(_))));
    }
}

#[test]
fn source_history_gc_on_a_fresh_catalog_with_no_objects_is_scoped_and_idempotent() {
    let catalog = Catalog::open_in_memory().unwrap();
    let now = crate::util::now_unix();
    assert_eq!(catalog.expire_source_history_leases(now, 32).unwrap(), 0);
    assert!(catalog.due_deletes(now, 32).unwrap().is_empty());
    // The startup sweep may run again before the first lease is created.  It
    // must use its own durable cursor/state and remain harmless on an empty
    // source-history graph.
    assert_eq!(catalog.expire_source_history_leases(now, 32).unwrap(), 0);
    assert!(catalog.due_deletes(now, 32).unwrap().is_empty());
}

#[tokio::test]
async fn source_history_gc_pages_live_objects_before_reaching_orphans() {
    let catalog = Arc::new(Catalog::open_in_memory().unwrap());
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let mut objects = Vec::new();
    catalog
        .with_connection(|db| {
            for index in 1..=300 {
                let id = format!("{index:032x}");
                let key = format!("v2/documents/storage-1/objects/{id}");
                objects.push((id.clone(), key.clone()));
                db.execute(
                    "INSERT INTO objects
                     (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at)
                     VALUES('storage-1',?1,?2,'source_chunk','available',?3,7,0,0)",
                    rusqlite::params![id, key, "e".repeat(64)],
                )?;
            }
            for index in 0..5 {
                let id = format!("f{index:031x}");
                let key = format!("v2/documents/storage-1/objects/{id}");
                objects.push((id.clone(), key.clone()));
                db.execute(
                    "INSERT INTO objects
                     (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at)
                     VALUES('storage-1',?1,?2,'source_chunk','available',?3,7,0,0)",
                    rusqlite::params![id, key, "e".repeat(64)],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let checkpoint = attributed("source-page-checkpoint", "Alice", Some("acct-1"));
    insert_fixture_checkpoint(&catalog, &checkpoint);
    let now = crate::util::now_millis();
    let live_ids = objects[300..]
        .iter()
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    catalog
        .with_connection(|db| {
            for id in &live_ids {
                db.execute(
                    "INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id)
                     VALUES('storage-1','source-page-checkpoint',?1)",
                    [id],
                )?;
            }
            db.execute(
                "UPDATE documents SET checkpoint_ref_count=checkpoint_ref_count+5
                 WHERE id='storage-1'",
                [],
            )?;
            db.execute(
                "UPDATE server_state SET checkpoint_ref_count=checkpoint_ref_count+5
                 WHERE id=1",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let total_bytes = 305_i64 * 7;
    catalog
        .with_connection(|db| {
            db.execute(
                "UPDATE objects SET gc_after=0
                 WHERE document_id='storage-1' AND id NOT IN (SELECT object_id
                   FROM checkpoint_objects WHERE document_id='storage-1')",
                [],
            )?;
            db.execute("UPDATE documents SET stored_bytes=?1 WHERE id='storage-1'", [total_bytes])?;
            db.execute("UPDATE accounts SET stored_bytes=?1 WHERE id='acct-1'", [total_bytes])?;
            db.execute("UPDATE server_state SET stored_bytes=?1 WHERE id=1", [total_bytes])?;
            Ok(())
        })
        .unwrap();
    assert!(catalog.audit_v2_counters().unwrap());

    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn crate::storage::blob::BlobStore> =
        Arc::new(crate::storage::blob::FsStore::new(dir.path(), true));
    for (_, key) in &objects {
        blobs.put(key, vec![b'x'; 7], "application/octet-stream").await.unwrap();
    }
    let worker = crate::storage::maintenance::DeletionWorker::new(
        Arc::clone(&catalog),
        blobs,
        crate::storage::maintenance::DeletionLimits::default(),
    )
    .unwrap();
    let first = worker.run_v2_once(now).await.unwrap();
    assert_eq!(first.candidates_claimed, 256);
    assert_eq!(first.objects_deleted, 256);
    let second = worker.run_v2_once(now).await.unwrap();
    assert_eq!(second.objects_deleted, 44);
    assert!(catalog.audit_v2_counters().unwrap());
    let remaining: i64 = catalog
        .with_connection(|db| {
            db.query_row(
                "SELECT COUNT(*) FROM objects WHERE document_id='storage-1' AND kind='source_chunk'",
                [],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(remaining, 5);
}

#[test]
fn source_history_gc_drains_a_large_orphan_encoding_across_pages() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let now = crate::util::now_millis();
    let mut objects = Vec::new();
    catalog
        .with_connection(|db| {
            for index in 0..5 {
                let id = fixture_object_id(&format!("large-source-{index}"));
                let key = format!("v2/documents/storage-1/objects/{id}");
                objects.push((id.clone(), key.clone()));
                db.execute(
                    "INSERT INTO objects
                     (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at)
                     VALUES('storage-1',?1,?2,'source_chunk','available',?3,7,0,0)",
                    rusqlite::params![id, key, "f".repeat(64)],
                )?;
            }
            Ok(())
        })
        .unwrap();
    catalog
        .with_connection(|db| {
            db.execute("UPDATE documents SET stored_bytes=35 WHERE id='storage-1'", [])?;
            db.execute("UPDATE accounts SET stored_bytes=35 WHERE id='acct-1'", [])?;
            db.execute("UPDATE server_state SET stored_bytes=35 WHERE id=1", [])?;
            Ok(())
        })
        .unwrap();
    assert!(catalog.audit_v2_counters().unwrap());
    let document_id = DocumentId::new("storage-1").unwrap();
    for (_, key) in &objects {
        catalog
            .queue_delete(&super::PendingDelete {
                slug: "doc".into(),
                object_key: key.clone(),
                bytes: 7,
                queued_at: now,
                delete_after: now,
            })
            .unwrap();
    }
    let mut completed = 0;
    for page in objects.chunks(2) {
        for (id, _) in page {
            let object_id = ObjectId::new(id.clone()).unwrap();
            assert!(catalog
                .claim_v2_object_for_deletion(
                    &document_id,
                    &object_id,
                    UnixMillis::new(now).unwrap(),
                    UnixMillis::new(now).unwrap(),
                )
                .unwrap());
        }
        assert_eq!(catalog.due_deletes(now, 2).unwrap().len(), page.len());
        for (id, _) in page {
            assert!(catalog
                .confirm_v2_object_deleted(&document_id, &ObjectId::new(id.clone()).unwrap())
                .unwrap());
            completed += 1;
        }
    }
    assert_eq!(completed, 5);
    assert!(catalog.audit_v2_counters().unwrap());
    let remaining: i64 = catalog
        .with_connection(|db| {
            db.query_row(
                "SELECT COUNT(*) FROM objects WHERE document_id='storage-1' AND kind='source_chunk'",
                [],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn v2_settlement_releases_the_admitted_reservation() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut v2_document = document();
    v2_document.size = 0;
    v2_document.counted_size = 0;
    catalog.create_document(&v2_document).unwrap();
    let document_id = DocumentId::new("storage-1").unwrap();
    let now = UnixMillis::new(crate::util::now_millis()).unwrap();
    let operation = catalog
        .prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(document_id.clone()),
                actor_key: "account:acct-1".into(),
                request_key: crate::util::new_request_key(),
                kind: OperationKind::SourcePublish,
                request_digest: fixture_tree_digest("maintenance-borrow"),
                plan_json: r#"{"version":2,"effect":"source_publish"}"#.into(),
                expected_document_generation: None,
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: Some(UnixMillis::new(now.0 + 120_000).unwrap()),
            },
            now,
        )
        .unwrap();
    let object_id = ObjectId::new(fixture_object_id("maintenance-borrow")).unwrap();
    catalog
        .allocate_v2_object_with_limits(
            &crate::storage::catalog::V2ObjectAllocation {
                document_id: document_id.clone(),
                id: object_id.clone(),
                storage_key: format!("v2/documents/{document_id}/objects/{object_id}"),
                kind: crate::storage::catalog::ObjectKind::SourceChunk,
                digest: fixture_tree_digest("maintenance-borrow"),
                logical_digest: None,
                encoding_version: 1,
                reserved_bytes: 100,
                operation_id: operation.id,
                now,
            },
            crate::storage::catalog::V2AdmissionLimits {
                owner_bytes: 1_000,
                deployment_bytes: 1_000,
                owner_documents: 1,
            },
        )
        .unwrap();
    let reserved = catalog.document("doc").unwrap().unwrap();
    assert_eq!(reserved.size, 0);
    assert_eq!(reserved.maintenance_reserved, 100);
    assert_eq!(reserved.counted_size, 100);
    catalog
        .settle_v2_object(&document_id, &object_id, 10, now)
        .unwrap();
    let settled = catalog.document("doc").unwrap().unwrap();
    assert_eq!(settled.size, 10);
    assert_eq!(settled.counted_size, 10);
    assert_eq!(settled.maintenance_reserved, 0);
    assert!(catalog.audit_v2_counters().unwrap());
}
            connection.execute(
                "INSERT INTO objects
                 (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at)
                 VALUES('storage-1',?1,?2,'source_tree','available',?3,0,0,0)",
                rusqlite::params![
                    tree_object_id,
                    format!("v2/documents/storage-1/objects/{tree_object_id}"),
                    tree_digest,
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let actor = MutationAuthority {
        account_id: "acct-1",
        owner_key: "",
        generation: "generation-1",
        link_hash: "",
        policy_editor: false,
        automation: false,
        unowned_publisher: false,
        execution_epoch: "",
        agent_checkpoint: None,
    };
    let mut point = attributed("quarto-render", "Alice", Some("acct-1"));
    point.source_format = "quarto".into();
    point.tree_sha = tree_digest;
    point.why = "render".into();
    catalog.revoke_sessions("acct-1", "generation-2").unwrap();
    assert!(catalog
        .insert_checkpoints_atomic_with_authority(std::slice::from_ref(&point), Some(actor))
        .is_err());
    assert!(catalog
        .checkpoint("doc", "quarto-render")
        .unwrap()
        .is_none());
    catalog
        .insert_checkpoints_atomic_with_authority(
            &[point],
            Some(MutationAuthority {
                generation: "generation-2",
                ..actor
            }),
        )
        .unwrap();
    assert!(catalog
        .checkpoint("doc", "quarto-render")
        .unwrap()
        .is_some());
}

#[test]
fn system_seed_identity_cannot_forge_checkpoint_authority() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let point = attributed("forged-system-checkpoint", "Examples", None);
    let forged = MutationAuthority {
        account_id: "system:examples",
        owner_key: "",
        generation: "seed-session",
        link_hash: "",
        policy_editor: true,
        automation: false,
        unowned_publisher: false,
        execution_epoch: "",
        agent_checkpoint: None,
    };
    assert!(matches!(
        catalog.insert_checkpoints_atomic_with_authority(&[point], Some(forged)),
        Err(CatalogError::Refused(_, _))
    ));
    assert!(catalog
        .checkpoint("doc", "forged-system-checkpoint")
        .unwrap()
        .is_none());
}

#[test]
fn execution_epoch_fences_checkpoint_commit_inside_sql_transaction() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let first_epoch = catalog
        .issue_agent_execution_lease("doc", "sidebar")
        .unwrap();
    catalog
        .revoke_agent_execution_lease("doc", "sidebar", &first_epoch)
        .unwrap();
    let tree_digest = "e".repeat(64);
    let tree_object_id = fixture_object_id("execution");
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO objects
                 (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at)
                 VALUES('storage-1',?1,?2,'source_tree','available',?3,0,0,0)",
                rusqlite::params![
                    tree_object_id,
                    format!("v2/documents/storage-1/objects/{tree_object_id}"),
                    tree_digest,
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let point = Checkpoint {
        slug: "doc".into(),
        sha: "fenced-checkpoint".into(),
        seq: -1,
        durable_seq: 0,
        tree_sha: tree_digest,
        parent: String::new(),
        at: "2026-01-01T00:00:00.000Z".into(),
        by: "Alice".into(),
        why: "agent".into(),
        source_format: "markdown".into(),
        size: 1,
        label: String::new(),
        git_commit: String::new(),
        dirty: false,
        changed: None,
        by_account: Some("acct-1".into()),
    };
    let commit = super::AgentCheckpointCommit {
        request_id: crate::util::new_request_key(),
        digest: "a".repeat(64),
        operation: serde_json::json!({"epoch":"epoch","id":"checkpoint"}),
        source_revision: "tree".into(),
    };
    let actor = MutationAuthority {
        account_id: "acct-1",
        owner_key: "",
        generation: "generation-1",
        link_hash: "",
        policy_editor: false,
        automation: false,
        unowned_publisher: false,
        execution_epoch: &first_epoch,
        agent_checkpoint: Some(&commit),
    };
    assert!(catalog
        .insert_checkpoints_atomic_with_authority(std::slice::from_ref(&point), Some(actor))
        .is_err());
    assert!(catalog
        .checkpoint("doc", "fenced-checkpoint")
        .unwrap()
        .is_none());

    let second_epoch = catalog
        .issue_agent_execution_lease("doc", "sidebar")
        .unwrap();
    let actor = MutationAuthority {
        execution_epoch: &second_epoch,
        agent_checkpoint: Some(&commit),
        ..actor
    };
    let storage_id = catalog.document("doc").unwrap().unwrap().storage_id;
    catalog.with_connection(|db| {
        db.execute("INSERT INTO agent_cancellations VALUES(?1,'agent-checkpoint-operation','cancel','digest','operation','','cancel_requested','{}',0)", [&storage_id])?;
        Ok(())
    }).unwrap();
    assert!(catalog
        .insert_checkpoints_atomic_with_authority(std::slice::from_ref(&point), Some(actor))
        .is_err());
    assert!(catalog
        .checkpoint("doc", "fenced-checkpoint")
        .unwrap()
        .is_none());
    catalog.with_connection(|db| {
        db.execute("DELETE FROM agent_cancellations", [])?;
        db.execute_batch("CREATE TRIGGER fail_agent_receipt BEFORE INSERT ON catalog_operations BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;")?;
        Ok(())
    }).unwrap();
    assert!(catalog
        .insert_checkpoints_atomic_with_authority(std::slice::from_ref(&point), Some(actor))
        .is_err());
    assert!(
        catalog
            .checkpoint("doc", "fenced-checkpoint")
            .unwrap()
            .is_none(),
        "receipt failure must roll back checkpoint insertion"
    );
    catalog
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_agent_receipt")?;
            Ok(())
        })
        .unwrap();
    catalog
        .insert_checkpoints_atomic_with_authority(std::slice::from_ref(&point), Some(actor))
        .unwrap();
    let receipt = catalog
        .operation(&storage_id, &commit.request_id)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.status, "committed");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&receipt.result).unwrap()["checkpoint_id"],
        point.sha
    );
    catalog
        .insert_checkpoints_atomic_with_authority(&[point], Some(actor))
        .unwrap();
    assert!(catalog
        .checkpoint("doc", "fenced-checkpoint")
        .unwrap()
        .is_some());
}

#[test]
fn deletion_resolves_prepared_publication_without_refunding_live_bytes() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    // `begin_delete` reports the durable v2 physical closure, which is kept
    // charged until the worker has settled its object.  The old fixture only
    // populated NewDocument::counted_size, a compatibility field that v2
    // deliberately ignores.
    let object_id = fixture_object_id("deletion-live-object");
    let object_digest = fixture_tree_digest("deletion-live-object");
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO objects
                 (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at)
                 VALUES('storage-1',?1,?2,'source_tree','available',?3,20,0,0)",
                rusqlite::params![
                    object_id,
                    format!("v2/documents/storage-1/objects/{object_id}"),
                    object_digest,
                ],
            )?;
            connection.execute(
                "UPDATE documents SET stored_bytes=20 WHERE id='storage-1'",
                [],
            )?;
            connection.execute(
                "UPDATE accounts SET stored_bytes=20 WHERE id='acct-1'",
                [],
            )?;
            connection.execute("UPDATE server_state SET stored_bytes=20 WHERE id=1", [])?;
            Ok(())
        })
        .unwrap();
    let now = UnixMillis::new(crate::util::now_millis()).unwrap();
    let operation = catalog
        .prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(DocumentId::new("storage-1").unwrap()),
                actor_key: "account:acct-1".into(),
                request_key: crate::util::new_request_key(),
                kind: OperationKind::SourcePublish,
                request_digest: fixture_tree_digest("deletion-publication"),
                plan_json: r#"{"version":2,"effect":"source_publish"}"#.into(),
                expected_document_generation: None,
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: Some(UnixMillis::new(now.0 + 120_000).unwrap()),
            },
            now,
        )
        .unwrap();
    let deleting = catalog.begin_delete("doc").unwrap();
    assert_eq!(deleting.counted_size, 20);
    assert!(deleting.pending_publication.is_none());
    let state: String = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT state FROM operations WHERE id=?1",
                    [operation.id.as_str()],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(state, "aborted");
    assert!(catalog
        .finish_v2_operation(&operation.id, "{}", true, now)
        .is_err());
    drain_document_delete(&catalog, "doc");
    // The bounded document worker only marks roots for physical retirement;
    // finalization must remain blocked while the physical object is present.
    assert!(matches!(
        catalog.finish_delete("doc"),
        Err(CatalogError::Conflict(_))
    ));
    assert_eq!(catalog.totals().unwrap(), (20, 1));
    // Model the separate blob-GC acknowledgement through the typed claim and
    // confirmation boundary. This is where the counters are actually
    // settled, after physical absence has been observed.
    catalog
        .with_connection(|connection| {
            connection.execute(
                "UPDATE objects SET gc_after=0 WHERE document_id='storage-1' AND id=?1",
                [&object_id],
            )?;
            Ok(())
        })
        .unwrap();
    let document_id = crate::storage::catalog::DocumentId::new("storage-1").unwrap();
    let object_id = crate::storage::catalog::ObjectId::new(object_id).unwrap();
    let now = crate::storage::catalog::UnixMillis::new(crate::util::now_millis()).unwrap();
    assert!(catalog
        .claim_v2_object_for_deletion(&document_id, &object_id, now, now)
        .unwrap());
    assert!(catalog
        .confirm_v2_object_deleted(&document_id, &object_id)
        .unwrap());
    catalog.finish_delete("doc").unwrap();
    assert_eq!(catalog.totals().unwrap(), (0, 0));
}

#[test]
fn fresh_schema_enables_foreign_keys_and_creates_all_tables() {
    let catalog = Catalog::open_in_memory().unwrap();
    assert_eq!(catalog.schema_version().unwrap(), 2);
    assert_eq!(catalog.totals().unwrap(), (0, 0));
    let tables = catalog
        .with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT name,sql FROM sqlite_master
                     WHERE type='table' AND name NOT LIKE 'sqlite_%'
                     ORDER BY name",
                )
                .unwrap();
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap();
            Ok(rows.map(Result::unwrap).collect::<Vec<_>>())
        })
        .unwrap();
    let mut expected = [
        "accounts",
        "documents",
        "grants",
        "links",
        "annotations",
        "replies",
        "checkpoints",
        "objects",
        "checkpoint_objects",
        "object_leases",
        "operations",
        "server_state",
    ];
    expected.sort_unstable();
    let actual = tables
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert_eq!(tables.len(), 12);
    assert!(tables.iter().all(|(_, sql)| sql.contains("STRICT")));
    catalog
        .with_connection(|connection| {
            assert_eq!(
                connection
                    .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                connection
                    .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                    .unwrap(),
                "ok"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn document_pages_cross_the_limit_without_skips_or_duplicates() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    for index in 0..401 {
        let mut row = document();
        row.slug = format!("doc-{index:04}");
        row.storage_id = format!("storage-{index:04}");
        row.title = format!("Document {index}");
        row.sha = format!("sha-{index:04}");
        // Equal timestamps exercise the slug half of the keyset cursor.
        row.updated_at = "2026-01-01T00:00:00.000Z".into();
        catalog.create_document(&row).unwrap();
    }

    let mut cursor: Option<(String, String)> = None;
    let mut slugs = Vec::new();
    loop {
        let page = catalog
            .documents_page(
                cursor
                    .as_ref()
                    .map(|(updated, slug)| (updated.as_str(), slug.as_str())),
                200,
            )
            .unwrap();
        if page.is_empty() {
            break;
        }
        cursor = page
            .last()
            .map(|row| (row.updated_at.clone(), row.slug.clone()));
        slugs.extend(page.into_iter().map(|row| row.slug));
    }

    assert_eq!(slugs.len(), 401);
    assert_eq!(slugs.first().map(String::as_str), Some("doc-0400"));
    assert_eq!(slugs.last().map(String::as_str), Some("doc-0000"));
    let unique = slugs.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(unique.len(), slugs.len());
}

#[test]
fn visible_documents_is_keyset_bounded_and_respects_listing_switch() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    for index in 0..1000 {
        catalog
            .create_document(&NewDocument {
                slug: format!("owned-{index:04}"),
                storage_id: format!("storage-{index:04}"),
                title: format!("Owned {index}"),
                sha: format!("sha-{index:04}"),
                created_at: format!("2026-01-01T00:{:02}:00Z", index % 60),
                published_at: format!("2026-01-01T00:{:02}:00Z", index % 60),
                updated_at: format!("2026-01-01T00:{:02}:00Z", index % 60),
                example: false,
        // Account-owned v2 rows do not carry the legacy opaque owner key.
        owner_key: String::new(),
                owner_id: Some("acct-1".into()),
                status: "active".into(),
                size: 1,
                counted_size: 1,
                maintenance_reserved: 0,
                last_auto_checkpoint_at: 0,
                source_format: "markdown".into(),
                main: "main.md".into(),
            })
            .unwrap();
    }
    catalog
        .create_document(&NewDocument {
            slug: "example-0000".into(),
            storage_id: "example-storage".into(),
            title: "Example project".into(),
            sha: "example-sha".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            published_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            example: true,
            owner_key: String::new(),
            owner_id: Some("acct-1".into()),
            status: "active".into(),
            size: 1,
            counted_size: 1,
            maintenance_reserved: 0,
            last_auto_checkpoint_at: 0,
            source_format: "markdown".into(),
            main: "main.md".into(),
        })
        .unwrap();
    let hidden = catalog
        .visible_documents_with_examples(None, None, None, 20, false)
        .unwrap();
    assert!(hidden.is_empty());
    let first = catalog
        .visible_documents_with_examples(Some("acct-1"), Some("owner"), None, 20, false)
        .unwrap();
    assert_eq!(first.len(), 20);
    let cursor = first
        .last()
        .map(|row| (row.updated_at.as_str(), row.slug.as_str()));
    let second = catalog
        .visible_documents_with_examples(Some("acct-1"), Some("owner"), cursor, 20, false)
        .unwrap();
    assert_eq!(second.len(), 20);
    assert!(first
        .iter()
        .all(|left| second.iter().all(|right| left.slug != right.slug)));
    let plan = catalog
        .with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "EXPLAIN QUERY PLAN SELECT d.slug FROM documents d
                     JOIN accounts a ON a.id=d.owner_id AND a.status='active'
                     WHERE d.status='active' AND d.owner_id='acct-1'
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT 20",
                )
                .unwrap();
            let rows = statement
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap();
            Ok(rows.collect::<Result<Vec<_>, _>>().unwrap())
        })
        .unwrap();
    assert!(plan.iter().all(|detail| !detail.contains("SCAN documents")));
    assert!(plan.iter().all(|detail| !detail.contains("TEMP B-TREE")));

    // The other authorization sources must keep the same bounded, ordered
    // shape.  These plans are deliberately checked separately: combining
    // grants/guests/examples with OR or EXISTS makes SQLite sort or scan the
    // whole document table before it can honor LIMIT.
    let plans = catalog
        .with_connection(|connection| {
            let queries = [
                "EXPLAIN QUERY PLAN SELECT d.slug FROM grants g JOIN documents d ON d.id=g.document_id JOIN accounts a ON a.id=d.owner_id AND a.status='active' WHERE d.status='active' AND g.account_id='acct-1' ORDER BY d.updated_at DESC,d.slug DESC LIMIT 20",
                "EXPLAIN QUERY PLAN SELECT d.slug FROM links l JOIN documents d ON d.id=l.document_id JOIN accounts a ON a.id=d.owner_id AND a.status='active' WHERE d.status='active' AND l.credential_generation>0 ORDER BY d.updated_at DESC,d.slug DESC LIMIT 20",
                "EXPLAIN QUERY PLAN SELECT d.slug FROM documents d INDEXED BY documents_examples WHERE d.status='active' AND d.ownership_mode='example' ORDER BY d.updated_at DESC,d.slug DESC LIMIT 20",
            ];
            Ok(queries
                .iter()
                .map(|query| {
                    let mut statement = connection.prepare(query).unwrap();
                    let rows = statement
                        .query_map([], |row| row.get::<_, String>(3))
                        .unwrap();
                    rows.collect::<Result<Vec<_>, _>>().unwrap()
                })
                .collect::<Vec<_>>())
        })
        .unwrap();
    for plan in plans {
        assert!(
            plan.iter().all(|detail| !detail.contains("TEMP B-TREE")),
            "{plan:?}"
        );
        assert!(
            plan.iter().all(|detail| !detail.contains("SCAN documents")),
            "{plan:?}"
        );
    }
}

#[test]
fn publication_receipts_are_atomic_and_idempotent() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let document_id = DocumentId::new("storage-1").unwrap();
    let now = UnixMillis::new(crate::util::now_millis()).unwrap();
    let request_id = crate::util::new_request_key();
    let input = V2OperationInput {
        scope: OperationScope::Document(document_id),
        actor_key: "account:acct-1".into(),
        request_key: request_id.clone(),
        kind: OperationKind::SourcePublish,
        request_digest: fixture_tree_digest("publication-receipt"),
        plan_json: r#"{"version":2,"effect":"source_publish"}"#.into(),
        expected_document_generation: None,
        conversation_id: None,
        execution_epoch: None,
        work_expires_at: Some(UnixMillis::new(now.0 + 120_000).unwrap()),
    };
    let prepared = catalog.prepare_v2_operation(&input, now).unwrap();
    assert_eq!(prepared.state, "prepared");
    catalog
        .finish_v2_operation(&prepared.id, r#"{"head":2}"#, true, now)
        .unwrap();
    let retry = catalog.prepare_v2_operation(&input, now).unwrap();
    assert_eq!(retry, V2Operation {
        state: "committed".into(),
        ..prepared.clone()
    });
    let (state, result): (String, String) = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT state,result_json FROM operations WHERE request_key=?1",
                    [&request_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(state, "committed");
    assert_eq!(result, r#"{"head":2}"#);
    assert!(catalog.document("doc").unwrap().unwrap().pending_publication.is_none());
}
#[test]
fn operation_capacity_refusal_does_not_leave_an_orphan_receipt() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    for index in 0..113 {
        let mut document = document();
        document.slug = format!("doc-{index}");
        document.storage_id = format!("storage-{index}");
        document.title = format!("Document {index}");
        catalog.create_document(&document).unwrap();
    }
    let now = UnixMillis::new(crate::util::now_millis()).unwrap();
    for index in 0..112 {
        let request_id = crate::util::new_request_key();
        let digest = format!("{index:064x}");
        catalog
            .prepare_v2_operation(
                &V2OperationInput {
                    scope: OperationScope::Document(
                        DocumentId::new(format!("storage-{index}")).unwrap(),
                    ),
                    actor_key: "account:acct-1".into(),
                    request_key: request_id,
                    kind: OperationKind::SourcePublish,
                    request_digest: digest,
                    plan_json: r#"{"version":2,"effect":"source_publish"}"#.into(),
                    expected_document_generation: None,
                    conversation_id: None,
                    execution_epoch: None,
                    work_expires_at: Some(UnixMillis::new(now.0 + 120_000).unwrap()),
                },
                now,
            )
            .unwrap();
    }
    let before: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM operations",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(before, 112);
    let request_id = crate::util::new_request_key();
    let digest = "e".repeat(64);
    assert!(matches!(
        catalog.prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(DocumentId::new("storage-112").unwrap()),
                actor_key: "account:acct-1".into(),
                request_key: request_id.clone(),
                kind: OperationKind::SourcePublish,
                request_digest: digest,
                plan_json: r#"{"version":2,"effect":"source_publish"}"#.into(),
                expected_document_generation: None,
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: Some(UnixMillis::new(now.0 + 120_000).unwrap()),
            },
            now,
        ),
        Err(CatalogError::Busy)
    ));
    let after: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM operations",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(after, before);
    let orphan: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT count(*) FROM operations WHERE request_key=?1",
                    [&request_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(orphan, 0);
}

#[test]
fn admission_and_reconciliation_keep_totals_exact() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut v2_document = document();
    v2_document.size = 0;
    v2_document.counted_size = 0;
    catalog.create_document(&v2_document).unwrap();
    let now = UnixMillis::new(crate::util::now_millis()).unwrap();
    let operation = catalog
        .prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(DocumentId::new("storage-1").unwrap()),
                actor_key: "account:acct-1".into(),
                request_key: crate::util::new_request_key(),
                kind: OperationKind::SourcePublish,
                request_digest: "a".repeat(64),
                plan_json: r#"{"version":2,"effect":"source_publish"}"#.into(),
                expected_document_generation: None,
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: Some(UnixMillis::new(now.0 + 120_000).unwrap()),
            },
            now,
        )
        .unwrap();
    let object_id = ObjectId::new(fixture_object_id("admission-source")).unwrap();
    catalog
        .allocate_v2_object_with_limits(
            &crate::storage::catalog::V2ObjectAllocation {
                document_id: DocumentId::new("storage-1").unwrap(),
                id: object_id.clone(),
                storage_key: format!("v2/documents/storage-1/objects/{object_id}"),
                kind: crate::storage::catalog::ObjectKind::SourceChunk,
                digest: "b".repeat(64),
                logical_digest: None,
                encoding_version: 1,
                reserved_bytes: 25,
                operation_id: operation.id,
                now,
            },
            crate::storage::catalog::V2AdmissionLimits {
                owner_bytes: 1_000,
                deployment_bytes: 1_000,
                owner_documents: 1,
            },
        )
        .unwrap();
    let reserved = catalog.document("doc").unwrap().unwrap();
    assert_eq!(reserved.counted_size, 25);
    assert_eq!(reserved.maintenance_reserved, 25);
    catalog
        .settle_v2_object(
            &DocumentId::new("storage-1").unwrap(),
            &object_id,
            12,
            now,
        )
        .unwrap();
    let settled = catalog.document("doc").unwrap().unwrap();
    assert_eq!(settled.size, 12);
    assert_eq!(settled.counted_size, 12);
    assert_eq!(settled.maintenance_reserved, 0);
    assert!(catalog.audit_v2_counters().unwrap());
}

#[test]
fn deleting_keeps_slug_reserved_until_finish() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog.begin_delete("doc").unwrap();
    assert_eq!(catalog.document("doc").unwrap().unwrap().status, "deleting");
    assert!(catalog.create_document(&document()).is_err());
    drain_document_delete(&catalog, "doc");
    catalog.finish_delete("doc").unwrap();
    assert!(catalog.document("doc").unwrap().is_none());
}

#[test]
fn deleting_document_cannot_commit_a_prepared_publication() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let now = UnixMillis::new(crate::util::now_millis()).unwrap();
    let operation = catalog
        .prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(DocumentId::new("storage-1").unwrap()),
                actor_key: "account:acct-1".into(),
                request_key: crate::util::new_request_key(),
                kind: OperationKind::SourcePublish,
                request_digest: fixture_tree_digest("deleting-publication"),
                plan_json: r#"{"version":2,"effect":"source_publish"}"#.into(),
                expected_document_generation: None,
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: Some(UnixMillis::new(now.0 + 120_000).unwrap()),
            },
            now,
        )
        .unwrap();
    catalog.begin_delete("doc").unwrap();
    assert!(catalog
        .finish_v2_operation(&operation.id, "{}", true, now)
        .is_err());
    let state: String = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT state FROM operations WHERE id=?1",
                    [operation.id.as_str()],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(state, "aborted");
}

#[test]
fn deleting_document_keeps_capacity_until_bounded_teardown_finishes() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog.begin_delete("doc").unwrap();
    assert!(catalog.finish_delete("doc").is_err());
    drain_document_delete(&catalog, "doc");
    catalog.finish_delete("doc").unwrap();
}

#[test]
fn object_accounting_settles_repeated_v2_closures_without_counter_drift() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut v2_document = document();
    v2_document.size = 0;
    v2_document.counted_size = 0;
    catalog.create_document(&v2_document).unwrap();

    let document_id = DocumentId::new("storage-1").unwrap();
    let now = UnixMillis::new(crate::util::now_millis()).unwrap();
    let limits = crate::storage::catalog::V2AdmissionLimits {
        owner_bytes: 16,
        deployment_bytes: 16,
        owner_documents: 1,
    };
    let mut allocate = |label: &str, reserved_bytes: i64, measured_bytes: i64| {
        let operation = catalog
            .prepare_v2_operation(
                &V2OperationInput {
                    scope: OperationScope::Document(document_id.clone()),
                    actor_key: "account:acct-1".into(),
                    request_key: crate::util::new_request_key(),
                    kind: OperationKind::SourcePublish,
                    request_digest: fixture_tree_digest(label),
                    plan_json: r#"{"version":2,"effect":"source_publish"}"#.into(),
                    expected_document_generation: None,
                    conversation_id: None,
                    execution_epoch: None,
                    work_expires_at: Some(UnixMillis::new(now.0 + 120_000).unwrap()),
                },
                now,
            )
            .unwrap();
        let id = ObjectId::new(fixture_object_id(label)).unwrap();
        catalog
            .allocate_v2_object_with_limits(
                &crate::storage::catalog::V2ObjectAllocation {
                    document_id: document_id.clone(),
                    id: id.clone(),
                    storage_key: format!("v2/documents/{document_id}/objects/{id}"),
                    kind: crate::storage::catalog::ObjectKind::SourceChunk,
                    digest: fixture_tree_digest(label),
                    logical_digest: None,
                    encoding_version: 1,
                    reserved_bytes,
                    operation_id: operation.id,
                    now,
                },
                limits,
            )
            .unwrap();
        catalog
            .settle_v2_object(&document_id, &id, measured_bytes, now)
            .unwrap();
    };

    allocate("accounting-first", 5, 5);
    allocate("accounting-second", 3, 3);
    let settled = catalog.document("doc").unwrap().unwrap();
    assert_eq!(settled.size, 8);
    assert_eq!(settled.counted_size, 8);
    assert_eq!(settled.maintenance_reserved, 0);
    assert!(catalog.audit_v2_counters().unwrap());

    // A refused closure must leave neither an object row nor a reservation.
    let before = catalog.document("doc").unwrap().unwrap();
    let operation = catalog
        .prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(document_id.clone()),
                actor_key: "account:acct-1".into(),
                request_key: crate::util::new_request_key(),
                kind: OperationKind::SourcePublish,
                request_digest: fixture_tree_digest("accounting-refused"),
                plan_json: r#"{"version":2,"effect":"source_publish"}"#.into(),
                expected_document_generation: None,
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: Some(UnixMillis::new(now.0 + 120_000).unwrap()),
            },
            now,
        )
        .unwrap();
    let refused_id = ObjectId::new(fixture_object_id("accounting-refused")).unwrap();
    assert!(catalog
        .allocate_v2_object_with_limits(
            &crate::storage::catalog::V2ObjectAllocation {
                document_id: document_id.clone(),
                id: refused_id.clone(),
                storage_key: format!("v2/documents/{document_id}/objects/{refused_id}"),
                kind: crate::storage::catalog::ObjectKind::SourceChunk,
                digest: fixture_tree_digest("accounting-refused"),
                logical_digest: None,
                encoding_version: 1,
                reserved_bytes: 9,
                operation_id: operation.id,
                now,
            },
            limits,
        )
        .is_err());
    let after = catalog.document("doc").unwrap().unwrap();
    assert_eq!(after.size, before.size);
    assert_eq!(after.counted_size, before.counted_size);
    assert_eq!(after.maintenance_reserved, before.maintenance_reserved);
    let object_count: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT count(*) FROM objects
                     WHERE document_id='storage-1' AND id=?1",
                    [refused_id.as_str()],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(object_count, 0);
    assert!(catalog.audit_v2_counters().unwrap());
}
fn file_catalog_reopens_with_wal_and_journal_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.db");
    {
        let catalog = Catalog::open(&path).unwrap();
        catalog
            .configure_journal("deployment-1", "writer-1")
            .unwrap();
        catalog
            .prepare_journal(&JournalPreparation {
                operation_id: "flush-1".into(),
                kind: "flush".into(),
                expected_revision: 0,
                expected_generation: "writer-1".into(),
                created_at: 1,
                plan: "{}".into(),
                resolved_at: None,
            })
            .unwrap();
        catalog
            .commit_journal(
                "flush-1",
                &JournalSegment {
                    segment_id: "segment-1".into(),
                    segment_seq: 0,
                    operation_id: "flush-1".into(),
                    object_key: "journal/segment-1".into(),
                    digest: "digest".into(),
                    encoded_bytes: 10,
                    committed_at: 2,
                },
                "journal/manifest-1",
                "manifest-digest",
                12,
                1,
                2,
            )
            .unwrap();
    }
    let reopened = Catalog::open(&path).unwrap();
    let state = reopened.journal_state().unwrap();
    assert_eq!(state.revision, 1);
    assert_eq!(reopened.journal_segments(-1, 10).unwrap().len(), 1);
}

#[test]
fn journal_commit_rejects_non_head_sequence_and_negative_time() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog
        .configure_journal("deployment-1", "writer-1")
        .unwrap();
    catalog
        .prepare_journal(&JournalPreparation {
            operation_id: "flush-1".into(),
            kind: "flush".into(),
            expected_revision: 0,
            expected_generation: "writer-1".into(),
            created_at: 1,
            plan: "{}".into(),
            resolved_at: None,
        })
        .unwrap();
    let segment = JournalSegment {
        segment_id: "segment-1".into(),
        segment_seq: 1,
        operation_id: "flush-1".into(),
        object_key: "journal/segment-1".into(),
        digest: "digest".into(),
        encoded_bytes: 10,
        committed_at: 2,
    };
    assert!(matches!(
        catalog.commit_journal(
            "flush-1",
            &segment,
            "journal/manifest-1",
            "manifest-digest",
            12,
            1,
            2,
        ),
        Err(CatalogError::Conflict(_))
    ));
    let mut segment = segment;
    segment.segment_seq = 0;
    segment.committed_at = -1;
    assert!(matches!(
        catalog.commit_journal(
            "flush-1",
            &segment,
            "journal/manifest-1",
            "manifest-digest",
            12,
            1,
            -1,
        ),
        Err(CatalogError::Invalid(_))
    ));
}

#[test]
fn journal_preparation_rejects_negative_creation_time() {
    let catalog = Catalog::open_in_memory().unwrap();
    let error = catalog
        .prepare_journal(&JournalPreparation {
            operation_id: "flush-1".into(),
            kind: "flush".into(),
            expected_revision: 0,
            expected_generation: "writer-1".into(),
            created_at: -1,
            plan: "{}".into(),
            resolved_at: None,
        })
        .unwrap_err();
    assert!(matches!(error, CatalogError::Invalid(_)));
}

#[test]
fn sql_children_are_bounded_and_expiry_filtered() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let tree_digest = fixture_tree_digest("sql-children");
    let tree_object_id = fixture_object_id("sql-children");
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO objects
                 (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at)
                 VALUES('storage-1',?1,?2,'source_tree','available',?3,0,0,0)",
                rusqlite::params![
                    tree_object_id,
                    format!("v2/documents/storage-1/objects/{tree_object_id}"),
                    tree_digest,
                ],
            )?;
            Ok(())
        })
        .unwrap();
    catalog.set_link_sealing_key(&[1_u8; 32]).unwrap();
    let link_secret = "bounded-reader-key";
    let link_hash = hex::encode(sha2::Sha256::digest(link_secret.as_bytes()));
    let link_sealed = catalog
        .seal_link_key("storage-1", "reader", &link_hash, link_secret)
        .unwrap();
    let link = Link {
        slug: "doc".into(),
        role: "reader".into(),
        // v2 token hashes are persisted as canonical SHA-256 hex.  Keep the
        // envelope deliberately tiny: this test exercises the SQL child
        // bounds, while the production lookup test below covers malformed
        // sealed credentials separately.
        hash: link_hash,
        sealed: link_sealed,
        label: "reader".into(),
        budget: None,
        since: "2026-01-01T00:00:00.000Z".into(),
        until: "".into(),
    };
    catalog.put_link(&link).unwrap();
    let checkpoint = catalog
        .insert_checkpoint(&Checkpoint {
            slug: "doc".into(),
            sha: "checkpoint".into(),
            seq: -1,
            durable_seq: 0,
            tree_sha: tree_digest,
            parent: String::new(),
            at: "2026-01-01T00:00:00.000Z".into(),
            by: "acct-1".into(),
            why: "test".into(),
            source_format: "markdown".into(),
            size: 10,
            label: String::new(),
            git_commit: String::new(),
            dirty: false,
            changed: None,
            by_account: None,
        })
        .unwrap();
    // v2 sequences are one-based; zero is reserved for the absent parent.
    assert_eq!(checkpoint.seq, 1);
}

#[test]
fn rotating_link_key_reseals_every_credential_atomically() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let old = [7_u8; 32];
    let new = [9_u8; 32];
    catalog.set_link_sealing_key(&old).unwrap();
    let plaintext = "secret-reader-key";
    let digest = hex::encode(sha2::Sha256::digest(plaintext.as_bytes()));
    let sealed = catalog
        .seal_link_key("storage-1", "reader", &digest, plaintext)
        .unwrap();
    catalog
        .put_link(&Link {
            slug: "doc".into(),
            role: "reader".into(),
            hash: digest.clone(),
            sealed,
            label: String::new(),
            budget: None,
            since: "2026-01-01T00:00:00Z".into(),
            until: String::new(),
        })
        .unwrap();

    let original_envelope = catalog.links("doc").unwrap().remove(0).sealed;
    assert_eq!(catalog.rotate_link_sealing_key(&new).unwrap(), 1);
    let rotated = catalog.links("doc").unwrap().remove(0);
    assert_eq!(
        catalog
            .open_link_key("storage-1", "reader", &digest, &rotated.sealed)
            .unwrap(),
        plaintext
    );
    assert_ne!(rotated.sealed, original_envelope);
}

#[test]
fn link_key_rotation_resumes_after_a_bounded_batch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.db");
    let old = [17_u8; 32];
    let new = [19_u8; 32];
    let operation_id;
    {
        let catalog = Catalog::open(&path).unwrap();
        catalog.upsert_account(&account()).unwrap();
        catalog.set_link_sealing_key(&old).unwrap();
        for index in 0..401 {
            let storage_id = format!("storage-{index:03}");
            let slug = format!("doc-{index:03}");
            catalog.with_connection(|db| {
                db.execute("INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path)
                    VALUES(?1,?2,'acct-1','owned',?2,?2,'active',1,1,'markdown','main.md')",rusqlite::params![storage_id,slug])?;
                Ok(())
            }).unwrap();
            let plaintext = format!("reader-key-{index}");
            let digest = hex::encode(sha2::Sha256::digest(plaintext.as_bytes()));
            let sealed = catalog.seal_link_key(&storage_id,"reader",&digest,&plaintext).unwrap();
            catalog.put_link(&Link { slug,role:"reader".into(),hash:digest,sealed,label:String::new(),budget:None,
                since:"2026-01-01T00:00:00Z".into(),until:String::new() }).unwrap();
        }
        let first = catalog.rotate_link_sealing_key_batch(&new).unwrap();
        assert_eq!(first.status,"running");
        assert_eq!(first.processed,200);
        assert_eq!(first.cursor_document_id.as_deref(),Some("storage-199"));
        operation_id = first.id;
        let running: String = catalog.with_connection(|db| Ok(db.query_row("SELECT state FROM operations WHERE id=?1",[&operation_id],|row|row.get(0))?)).unwrap();
        assert_eq!(running,"prepared");
    }
    // SQL selects the new primary even while old envelopes remain. Restoring
    // all persisted secrets makes the durable cursor resumable before traffic.
    let reopened = Catalog::open(&path).unwrap();
    assert_eq!(Catalog::persisted_primary_link_key_id(&path).unwrap(),link_key_id_for_test(&new));
    reopened.set_link_sealing_key(&new).unwrap();
    assert!(reopened.resume_link_key_rotation().is_err(), "missing source secret must not silently finish a rotation");
    reopened.add_link_decryption_key(&old).unwrap();
    assert_eq!(reopened.resume_link_key_rotation().unwrap(),Some(201));
    reopened.with_connection(|db| {
        let (state,completed,expiry):(String,i64,i64)=db.query_row("SELECT state,completed_at,receipt_expires_at FROM operations WHERE id=?1",[&operation_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
        assert_eq!(state,"committed");
        assert_eq!(expiry-completed,604800000);
        assert_eq!(db.query_row("SELECT count(*) FROM links WHERE sealing_key_id<>?1",[link_key_id_for_test(&new)],|row|row.get::<_,i64>(0))?,0);
        Ok(())
    }).unwrap();
    for index in [0,199,200,400] {
        let link = reopened.links(&format!("doc-{index:03}")).unwrap().remove(0);
        assert_eq!(reopened.open_link_key(&format!("storage-{index:03}"),"reader",&link.hash,&link.sealed).unwrap(),format!("reader-key-{index}"));
    }
    assert_eq!(reopened.rotate_link_sealing_key(&new).unwrap(),0);
}

#[test]
fn link_key_rotation_without_links_persists_primary_and_receipt() {
    let catalog=Catalog::open_in_memory().unwrap();
    let old=[23_u8;32]; let new=[29_u8;32];
    catalog.set_link_sealing_key(&old).unwrap();
    let result=catalog.rotate_link_sealing_key_batch(&new).unwrap();
    assert_eq!(result.status,"committed");
    assert_eq!(result.processed,0);
    assert_eq!(catalog.link_keyring_primary_id().unwrap(),Some(link_key_id_for_test(&new)));
    assert_eq!(catalog.resume_link_key_rotation().unwrap(),None);
}

fn link_key_id_for_test(key: &[u8; 32]) -> String {
    hex::encode(sha2::Sha256::digest(key))[..16].to_string()
}

#[test]
fn access_rotation_is_cas_protected_and_keeps_link_identity() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog.set_link_sealing_key(&[41_u8; 32]).unwrap();
    let first_key = "first-reader-key";
    let first_hash = hex::encode(sha2::Sha256::digest(first_key.as_bytes()));
    let first = Link {
        slug: "doc".into(),
        role: "reader".into(),
        hash: first_hash.clone(),
        sealed: catalog
            .seal_link_key("storage-1", "reader", &first_hash, first_key)
            .unwrap(),
        label: "first".into(),
        budget: None,
        since: "2026-01-01T00:00:00Z".into(),
        until: String::new(),
    };
    let snapshot = catalog.document("doc").unwrap().unwrap();
    catalog
        .update_document_access(
            &snapshot,
            &[],
            std::slice::from_ref(&first),
            &[],
            Some(("acct-1", "", "generation-1")),
        )
        .unwrap();
    let first_id: String = catalog
        .with_connection(|db| {
            db.query_row(
                "SELECT id FROM links WHERE document_id='storage-1' AND role='reader'",
                [],
                |row| row.get(0),
            )
            .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    let second_key = "second-reader-key";
    let second_hash = hex::encode(sha2::Sha256::digest(second_key.as_bytes()));
    let second = Link {
        hash: second_hash.clone(),
        sealed: catalog
            .seal_link_key("storage-1", "reader", &second_hash, second_key)
            .unwrap(),
        label: "rotated".into(),
        ..first
    };
    let current = catalog.document("doc").unwrap().unwrap();
    catalog
        .update_document_access(
            &current,
            &[],
            std::slice::from_ref(&second),
            &[],
            Some(("acct-1", "", "generation-1")),
        )
        .unwrap();
    let (second_id, generation): (String, i64) = catalog
        .with_connection(|db| {
            db.query_row(
                "SELECT id,credential_generation FROM links
                 WHERE document_id='storage-1' AND role='reader'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(second_id, first_id);
    assert_eq!(generation, 2);
    assert!(catalog
        .update_document_access(
            &snapshot,
            &[],
            std::slice::from_ref(&second),
            &[],
            Some(("acct-1", "", "generation-1")),
        )
        .is_err());
}

#[test]
fn visibility_reads_live_bookmarks_and_removes_rotated_ones() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut guest = account();
    guest.id = "acct-guest".into();
    guest.handle = "guest".into();
    guest.session_generation = "guest-generation".into();
    catalog.upsert_account(&guest).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog.set_link_sealing_key(&[43_u8; 32]).unwrap();
    let key = "bookmark-reader-key";
    let hash = hex::encode(sha2::Sha256::digest(key.as_bytes()));
    let sealed = catalog
        .seal_link_key("storage-1", "reader", &hash, key)
        .unwrap();
    catalog
        .put_link(&Link {
            slug: "doc".into(),
            role: "reader".into(),
            hash: hash.clone(),
            sealed,
            label: String::new(),
            budget: None,
            since: "2026-01-01T00:00:00Z".into(),
            until: String::new(),
        })
        .unwrap();
    catalog
        .pin_guest(&super::Guest {
            slug: "doc".into(),
            account_id: "acct-guest".into(),
            since: "2026-01-01T00:00:00Z".into(),
            link_hash: hash,
        })
        .unwrap();
    assert_eq!(
        catalog
            .visible_documents(Some("acct-guest"), None, None, 20)
            .unwrap()
            .iter()
            .filter(|document| document.slug == "doc")
            .count(),
        1
    );
    catalog.drop_link("doc", "reader").unwrap();
    assert!(catalog
        .visible_documents(Some("acct-guest"), None, None, 20)
        .unwrap()
        .is_empty());
}

#[test]
fn transfer_rechecks_owner_inside_the_write_transaction() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let mut bob = account();
    bob.id = "acct-2".into();
    bob.handle = "bob".into();
    bob.session_generation = "generation-2".into();
    catalog.upsert_account(&bob).unwrap();

    assert!(catalog
        .transfer_ownership_authorized_with_generation(
            "doc",
            Some("acct-attacker"),
            "",
            Some("generation-1"),
            "acct-2",
            100,
        )
        .is_err());
    let moved = catalog
        .transfer_ownership_authorized_with_generation(
            "doc",
            Some("acct-1"),
            "",
            Some("generation-1"),
            "acct-2",
            100,
        )
        .unwrap();
    assert_eq!(moved.owner_id.as_deref(), Some("acct-2"));
    assert!(catalog
        .transfer_ownership_authorized_with_generation(
            "doc",
            Some("acct-1"),
            "",
            Some("generation-1"),
            "acct-1",
            100,
        )
        .is_err());
}

#[test]
fn transfer_rechecks_account_generation_inside_the_write_transaction() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut bob = account();
    bob.id = "acct-2".into();
    bob.handle = "bob".into();
    catalog.upsert_account(&bob).unwrap();
    catalog.create_document(&document()).unwrap();

    assert!(catalog
        .transfer_ownership_authorized_with_generation(
            "doc",
            Some("acct-1"),
            "",
            Some("revoked-generation"),
            "acct-2",
            100,
        )
        .is_err());
    let moved = catalog
        .transfer_ownership_authorized_with_generation(
            "doc",
            Some("acct-1"),
            "",
            Some("generation-1"),
            "acct-2",
            100,
        )
        .unwrap();
    assert_eq!(moved.owner_id.as_deref(), Some("acct-2"));
}

#[test]
fn mutation_authority_rechecks_live_editor_links_and_automation_bounds() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut coauthor = account();
    coauthor.id = "acct-2".into();
    coauthor.handle = "bob".into();
    coauthor.session_generation = "generation-2".into();
    catalog.upsert_account(&coauthor).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog.set_link_sealing_key(&[53_u8; 32]).unwrap();
    let editor_key = "editor-link-secret";
    let link_hash = hex::encode(sha2::Sha256::digest(editor_key.as_bytes()));
    let sealed = catalog
        .seal_link_key("storage-1", "editor", &link_hash, editor_key)
        .unwrap();
    catalog
        .put_link(&Link {
            slug: "doc".into(),
            role: "editor".into(),
            hash: link_hash.clone(),
            sealed,
            label: String::new(),
            budget: None,
            since: "2026-01-01T00:00:00Z".into(),
            until: "".into(),
        })
        .unwrap();

    let authority = crate::document::store::MutationActor {
        account_id: "acct-2".into(),
        owner_key: "bob".into(),
        session_generation: "generation-2".into(),
        link_hash: link_hash.clone(),
        policy_editor: true,
        automation: false,
        unowned_publisher: false,
    };
    let admit_publication = |label: &str,
                             actor: &crate::document::store::MutationActor|
     -> Result<(), CatalogError> {
        let now = UnixMillis::now();
        let operation_id = crate::storage::catalog::OperationId::new(
            fixture_object_id(&format!("{label}-operation")),
        )
        .map_err(|error| CatalogError::Invalid(error.to_string()))?;
        let document_id = DocumentId::new("storage-1").unwrap();
        let operation = V2OperationInput {
            scope: OperationScope::Document(document_id.clone()),
            actor_key: format!("account:{}", actor.account_id),
            request_key: crate::util::new_request_key(),
            kind: OperationKind::DisplayPublish,
            request_digest: fixture_tree_digest(label),
            plan_json: r#"{"version":1,"expected_publication_id":""}"#.into(),
            expected_document_generation: Some(0),
            conversation_id: None,
            execution_epoch: None,
            work_expires_at: Some(UnixMillis::new(now.0 + 120_000).unwrap()),
        };
        let allocations = [
            ("manifest", crate::storage::catalog::ObjectKind::PublicationManifest, 32_i64),
            ("html", crate::storage::catalog::ObjectKind::PublicationHtml, 16_i64),
        ]
        .into_iter()
        .map(|(kind_label, kind, bytes)| {
            let id = ObjectId::new(fixture_object_id(&format!("{label}-{kind_label}"))).unwrap();
            crate::storage::catalog::V2ObjectAllocation {
                document_id: document_id.clone(),
                id: id.clone(),
                storage_key: format!("v2/documents/{document_id}/objects/{id}"),
                kind,
                digest: fixture_tree_digest(&format!("{label}-{kind_label}-digest")),
                logical_digest: None,
                encoding_version: 1,
                reserved_bytes: bytes,
                operation_id: operation_id.clone(),
                now,
            }
        })
        .collect::<Vec<_>>();
        catalog.prepare_publication_bundle(
            &operation,
            &allocations,
            actor,
            crate::storage::catalog::V2AdmissionLimits {
                owner_bytes: 1_000,
                deployment_bytes: 1_000,
                owner_documents: 1,
            },
        )
    };
    admit_publication("editor-before-revoke", &authority).unwrap();
    let rotated_sealed = catalog
        .seal_link_key("storage-1", "editor", &link_hash, editor_key)
        .unwrap();
    catalog
        .put_link(&Link {
            slug: "doc".into(),
            role: "editor".into(),
            hash: link_hash.clone(),
            sealed: rotated_sealed,
            label: String::new(),
            budget: None,
            since: "2026-01-01T00:00:00Z".into(),
            until: "2020-01-01T00:00:00Z".into(),
        })
        .unwrap();
    assert!(admit_publication("editor-after-revoke", &authority).is_err());
    let automation_without_link = crate::document::store::MutationActor {
        automation: true,
        link_hash: String::new(),
        ..authority.clone()
    };
    assert!(admit_publication("automation-without-link", &automation_without_link).is_err());
}

#[tokio::test]
async fn checked_production_lookup_propagates_corrupt_authorization_rows() {
    let catalog = std::sync::Arc::new(Catalog::open_in_memory().unwrap());
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog.set_link_sealing_key(&[1_u8; 32]).unwrap();
    let key_id = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT active_link_key_id FROM server_state WHERE id=1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO links
                 (document_id,id,role,token_hash,sealed_token,sealing_key_id,label,
                  created_at,expires_at)
                 VALUES('storage-1','reader','reader',?1,?2,?3,'',?4,NULL)",
                rusqlite::params![
                    "b".repeat(64),
                    vec![1_u8, 2, 3],
                    key_id,
                    crate::util::now_millis(),
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let blobs: std::sync::Arc<dyn crate::storage::blob::BlobStore> =
        std::sync::Arc::new(crate::storage::blob::FsStore::new(dir.path(), true));
    let store = crate::document::store::Store::open_with_catalog(
        blobs,
        std::sync::Arc::new(crate::config::Configuration::default()),
        catalog,
    )
    .await
    .unwrap();
    assert!(
        store.get_checked("doc").await.is_err(),
        "lookup must not collapse corrupt authorization rows into not-found"
    );
}

#[tokio::test]
async fn bounded_listing_propagates_catalog_failure() {
    let catalog = std::sync::Arc::new(Catalog::open_in_memory().unwrap());
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let blobs: std::sync::Arc<dyn crate::storage::blob::BlobStore> =
        std::sync::Arc::new(crate::storage::blob::FsStore::new(dir.path(), true));
    let store = crate::document::store::Store::open_with_catalog(
        blobs,
        std::sync::Arc::new(crate::config::Configuration::default()),
        catalog.clone(),
    )
    .await
    .unwrap();
    catalog
        .with_connection(|connection| {
            connection
                // v2 stores guest visits in each account's bookmarks; links
                // remain the authoritative authorization rows used while
                // materializing a listing entry.
                .execute("DROP TABLE links", [])
                .map(|_| ())
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert!(
        store
            .visible_page_with_options(Some("acct-1"), None, None, 20, true)
            .await
            .is_err(),
        "listing must fail closed when catalogue authorization data is unreadable"
    );
}

#[test]
fn v2_admission_keeps_inflight_reservation_until_settlement() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut v2_document = document();
    v2_document.size = 0;
    v2_document.counted_size = 0;
    catalog.create_document(&v2_document).unwrap();
    let document_id = DocumentId::new("storage-1").unwrap();
    let now = UnixMillis::new(crate::util::now_millis()).unwrap();
    let operation = catalog
        .prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(document_id.clone()),
                actor_key: "account:acct-1".into(),
                request_key: crate::util::new_request_key(),
                kind: OperationKind::SourcePublish,
                request_digest: fixture_tree_digest("inflight-reservation"),
                plan_json: r#"{"version":2,"effect":"source_publish"}"#.into(),
                expected_document_generation: None,
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: Some(UnixMillis::new(now.0 + 120_000).unwrap()),
            },
            now,
        )
        .unwrap();
    let object_id = ObjectId::new(fixture_object_id("inflight-reservation")).unwrap();
    catalog
        .allocate_v2_object_with_limits(
            &crate::storage::catalog::V2ObjectAllocation {
                document_id: document_id.clone(),
                id: object_id.clone(),
                storage_key: format!("v2/documents/{document_id}/objects/{object_id}"),
                kind: crate::storage::catalog::ObjectKind::SourceChunk,
                digest: fixture_tree_digest("inflight-reservation"),
                logical_digest: None,
                encoding_version: 1,
                reserved_bytes: 25,
                operation_id: operation.id,
                now,
            },
            crate::storage::catalog::V2AdmissionLimits {
                owner_bytes: 1_000,
                deployment_bytes: 1_000,
                owner_documents: 1,
            },
        )
        .unwrap();
    let reserved = catalog.document("doc").unwrap().unwrap();
    assert_eq!(reserved.size, 0);
    assert_eq!(reserved.counted_size, 25);
    assert_eq!(reserved.maintenance_reserved, 25);
    assert_eq!(catalog.catalog_allocated_bytes().unwrap(), 25);
    catalog
        .settle_v2_object(&document_id, &object_id, 20, now)
        .unwrap();
    let settled = catalog.document("doc").unwrap().unwrap();
    assert_eq!(settled.size, 20);
    assert_eq!(settled.counted_size, 20);
    assert_eq!(settled.maintenance_reserved, 0);
    assert_eq!(catalog.catalog_allocated_bytes().unwrap(), 0);
    assert!(catalog.audit_v2_counters().unwrap());
}

#[test]
fn v2_checkpoint_operation_receipt_is_bounded_and_terminal() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let request_id = crate::util::new_request_key();
    let now = UnixMillis::new(crate::util::now_millis()).unwrap();
    let prepared = catalog
        .prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(DocumentId::new("storage-1").unwrap()),
                actor_key: "account:acct-1".into(),
                request_key: request_id.clone(),
                kind: OperationKind::Checkpoint,
                request_digest: "c".repeat(64),
                plan_json: r#"{"version":2,"effect":"checkpoint"}"#.into(),
                expected_document_generation: None,
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: Some(UnixMillis::new(now.0 + 120_000).unwrap()),
            },
            now,
        )
        .unwrap();
    assert_eq!(prepared.state, "prepared");
    let expires: Option<i64> = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT work_expires_at FROM operations WHERE request_key=?1",
                    [&prepared.request_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert!(expires.is_some(), "prepared v2 work must have a bounded lease");
    catalog
        .finish_v2_operation(&prepared.id, "{}", true, now)
        .unwrap();
    let (state, result, receipt_expires_at): (String, String, i64) = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT state,result_json,receipt_expires_at
                     FROM operations WHERE request_key=?1",
                    [&request_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(state, "committed");
    assert_eq!(result, "{}");
    assert!(receipt_expires_at > crate::util::now_millis());
}

#[test]
fn account_erasure_uses_a_stable_primary_key_cursor() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut owner = account();
    owner.id = "acct-2".into();
    owner.handle = "bob".into();
    catalog.upsert_account(&owner).unwrap();
    for index in 0..3 {
        let mut document = document();
        document.slug = format!("doc-{index}");
        document.title = format!("Document {index}");
        document.storage_id = format!("storage-{index}");
        document.owner_id = Some(owner.id.clone());
        catalog.create_document(&document).unwrap();
        catalog
            .grant(
                &document.slug,
                "reader",
                &account().id,
                "2026-01-01T00:00:00Z",
            )
            .unwrap();
    }
    catalog.begin_erasure("acct-1", "generation-2").unwrap();
    let mut cursor = None;
    loop {
        let removed = catalog
            .erase_account_batch("acct-1", "grants", cursor.as_deref(), 0, 1)
            .unwrap();
        if removed == 0 {
            break;
        }
        cursor = catalog
            .erasure_progress("acct-1")
            .unwrap()
            .and_then(|(_, cursor)| cursor);
    }
    let remaining: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM grants WHERE account_id='acct-1'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(remaining, 0);
}

/* ------------------------------------------------ stable checkpoint identity */

fn contributor(id: &str, handle: &str) -> Account {
    Account {
        id: id.into(),
        provider: "github".into(),
        handle: handle.into(),
        name: handle.into(),
        email: format!("{handle}@example.test"),
        first_seen: "2026-01-01T00:00:00.000Z".into(),
        last_seen: "2026-01-01T00:00:00.000Z".into(),
        plan: "free".into(),
        status: "active".into(),
        session_generation: "generation-1".into(),
        erasure_cursor: None,
    }
}

/// A checkpoint row with explicit attribution. `sha` doubles as the
/// content identity, so the assertions below can show that erasure moved the
/// attribution and nothing else.
fn attributed(sha: &str, by: &str, by_account: Option<&str>) -> Checkpoint {
    Checkpoint {
        slug: "doc".into(),
        sha: sha.into(),
        seq: -1,
        durable_seq: 0,
        tree_sha: fixture_tree_digest(sha),
        parent: String::new(),
        at: "2026-02-02T00:00:00.000Z".into(),
        by: by.into(),
        by_account: by_account.map(str::to_string),
        why: "cli".into(),
        source_format: "markdown".into(),
        size: 7,
        label: String::new(),
        git_commit: String::new(),
        dirty: false,
        changed: Some("[]".into()),
    }
}

fn fixture_tree_digest(seed: &str) -> String {
    hex::encode(sha2::Sha256::digest(seed.as_bytes()))
}

/// v2 checkpoint admission requires a settled source-tree object.  The old
/// catalog fixtures carried that object implicitly, so install a minimal
/// available row before exercising attribution and erasure behavior.
fn fixture_object_id(seed: &str) -> String {
    fixture_tree_digest(seed)[..32].to_owned()
}

fn insert_fixture_checkpoints(catalog: &Catalog, checkpoints: &[Checkpoint]) {
    let document_id = catalog
        .document(&checkpoints[0].slug)
        .unwrap()
        .expect("fixture document")
        .storage_id;
    catalog
        .with_connection(|connection| {
            for checkpoint in checkpoints {
                let object_id = fixture_object_id(&checkpoint.sha);
                connection.execute(
                    "INSERT OR IGNORE INTO objects
                     (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at)
                    VALUES(?1,?2,?3,'source_tree','available',?4,0,0,0)",
                    rusqlite::params![
                        document_id.as_str(),
                        object_id,
                        format!(
                            "v2/documents/{}/objects/{}",
                            document_id,
                            fixture_object_id(&checkpoint.sha)
                        ),
                        checkpoint.tree_sha.as_str(),
                    ],
                )?;
            }
            Ok(())
        })
        .unwrap();
    catalog.insert_checkpoints_atomic(checkpoints).unwrap();
}

fn insert_fixture_checkpoint(catalog: &Catalog, checkpoint: &Checkpoint) {
    insert_fixture_checkpoints(catalog, std::slice::from_ref(checkpoint));
}

fn insert_attributed(catalog: &Catalog, sha: &str, by: &str, by_account: Option<&str>) {
    let checkpoint = attributed(sha, by, by_account);
    insert_fixture_checkpoint(catalog, &checkpoint);
}

fn drain_erasure(catalog: &Catalog, id: &str, limit: u32) {
    let stages = [
        "grants",
        "bookmarks",
        "annotation_replies",
        "annotations",
        "replies",
        "checkpoints",
    ];
    for stage in stages {
        let mut cursor: Option<String> = None;
        loop {
            let touched = catalog
                .erase_account_batch(id, stage, cursor.as_deref(), 0, limit)
                .unwrap();
            if touched == 0 {
                break;
            }
            cursor = catalog
                .erasure_progress(id)
                .unwrap()
                .and_then(|(_, cursor)| cursor);
        }
    }
}

fn drain_document_delete(catalog: &Catalog, slug: &str) {
    for _ in 0..64 {
        let _ = catalog
            .erase_document_batch(slug, 250, crate::util::now_millis())
            .unwrap();
        let done = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT json_extract(plan_json,'$.stage')='done'
                         FROM operations o JOIN documents d ON d.id=o.document_id
                         WHERE d.slug=?1 AND o.kind='erase_document' AND o.state='prepared'",
                        [slug],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(CatalogError::from)
            })
            .unwrap();
        if done {
            return;
        }
    }
    panic!("document teardown did not reach its durable done stage");
}

fn attribution_of(catalog: &Catalog, sha: &str) -> (String, Option<String>) {
    let row = catalog.checkpoint("doc", sha).unwrap().unwrap();
    (row.by, row.by_account)
}

/// A handle is not an identity. Renaming one account and giving its old
/// handle to another must not move either account's historical checkpoints,
/// and two accounts that show the same display name stay distinct.
#[test]
fn erasure_follows_the_account_not_the_handle() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .upsert_account(&contributor("acct-writer", "alice"))
        .unwrap();
    catalog
        .upsert_account(&contributor("acct-other", "alice"))
        .unwrap();
    insert_attributed(&catalog, "mine", "alice", Some("acct-writer"));
    insert_attributed(&catalog, "theirs", "alice", Some("acct-other"));
    // The handle moves on: the writer is renamed and its old name is taken
    // by the other account. Neither rename may reach a checkpoint.
    let mut renamed = contributor("acct-writer", "carol");
    renamed.last_seen = "2026-03-01T00:00:00.000Z".into();
    catalog.upsert_account(&renamed).unwrap();

    catalog
        .begin_erasure("acct-writer", "generation-2")
        .unwrap();
    drain_erasure(&catalog, "acct-writer", 1000);

    assert_eq!(
        attribution_of(&catalog, "mine"),
        ("Deleted user".to_string(), None),
        "the renamed account's own checkpoint loses its attribution"
    );
    assert_eq!(
        attribution_of(&catalog, "theirs"),
        ("alice".to_string(), Some("acct-other".to_string())),
        "an equal display name on another account is untouched"
    );
    let kept = catalog.checkpoint("doc", "mine").unwrap().unwrap();
    assert_eq!(kept.tree_sha, fixture_tree_digest("mine"));
    assert_eq!(kept.at, "2026-02-02T00:00:00.000Z");
    assert_eq!(kept.size, 7);
    assert_eq!(kept.why, "cli");
    catalog.finish_erasure("acct-writer").unwrap();
    assert!(catalog.account("acct-writer").unwrap().is_none());
    assert!(catalog.account("acct-other").unwrap().is_some());
}

/// Anonymous, imported and system checkpoints have no account to erase, and a
/// legacy row whose `by` literally holds an account id is still reachable.
#[test]
fn erasure_covers_legacy_rows_and_leaves_unattributed_ones_alone() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .upsert_account(&contributor("acct-writer", "alice"))
        .unwrap();
    insert_attributed(&catalog, "stable", "alice", Some("acct-writer"));
    // A v2 row without an account attribution is immutable provenance. The
    // opaque legacy value is retained; erasure must not guess that it names
    // the account being erased.
    insert_attributed(&catalog, "legacy", "acct-writer", None);
    insert_attributed(&catalog, "anonymous", "Reviewer two", None);
    insert_attributed(&catalog, "imported", "", None);

    catalog
        .begin_erasure("acct-writer", "generation-2")
        .unwrap();
    drain_erasure(&catalog, "acct-writer", 1000);

    assert_eq!(
        attribution_of(&catalog, "stable"),
        ("Deleted user".to_string(), None)
    );
    assert_eq!(
        attribution_of(&catalog, "legacy"),
        ("acct-writer".to_string(), None)
    );
    assert_eq!(
        attribution_of(&catalog, "anonymous"),
        ("Reviewer two".to_string(), None),
        "an unattributed checkpoint is nobody's to erase"
    );
    assert_eq!(attribution_of(&catalog, "imported"), (String::new(), None));
    catalog.finish_erasure("acct-writer").unwrap();
}

/// The worker may crash between batches and resume from the stored cursor, or
/// lose the cursor entirely and repeat from the start. Neither may skip a row.
#[test]
fn erasure_batches_restart_without_skipping_checkpoints() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .upsert_account(&contributor("acct-writer", "alice"))
        .unwrap();
    for index in 0..25 {
        insert_attributed(
            &catalog,
            &format!("point-{index:03}"),
            "alice",
            Some("acct-writer"),
        );
    }
    catalog
        .begin_erasure("acct-writer", "generation-2")
        .unwrap();
    // One row per batch, and every third batch forgets its cursor the way a
    // restart that lost the in-memory position would.
    let mut cursor: Option<String> = None;
    let mut batches = 0;
    loop {
        let touched = catalog
            .erase_account_batch("acct-writer", "checkpoints", cursor.as_deref(), 0, 1)
            .unwrap();
        if touched == 0 {
            break;
        }
        batches += 1;
        cursor = if batches % 3 == 0 {
            None
        } else {
            catalog
                .erasure_progress("acct-writer")
                .unwrap()
                .and_then(|(_, cursor)| cursor)
        };
    }
    for index in 0..25 {
        assert_eq!(
            attribution_of(&catalog, &format!("point-{index:03}")),
            ("Deleted user".to_string(), None),
            "a restarted batch must not skip a record"
        );
    }
}

/// A checkpoint is admitted, its objects written, and only then inserted. An
/// erasure that starts inside that window must win: the insert commits the
/// content and drops the identity.
#[test]
fn a_queued_checkpoint_cannot_reintroduce_erased_attribution() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .upsert_account(&contributor("acct-writer", "alice"))
        .unwrap();
    // The row the caller prepared before the erasure began.
    let queued = attributed("queued", "alice", Some("acct-writer"));
    catalog
        .begin_erasure("acct-writer", "generation-2")
        .unwrap();
    drain_erasure(&catalog, "acct-writer", 1000);
    insert_fixture_checkpoint(&catalog, &queued);
    let stored = catalog.checkpoint("doc", "queued").unwrap().unwrap();
    assert_eq!(stored.by_account, None);
    assert_eq!(stored.by, "Deleted user");
    assert_eq!(
        stored.tree_sha,
        fixture_tree_digest("queued"),
        "the content is retained"
    );
    // The batched insert path and the erasure gate agree.
    let mut second = attributed("staged", "alice", Some("acct-writer"));
    second.seq = -1;
    insert_fixture_checkpoints(&catalog, &[second]);
    assert_eq!(
        attribution_of(&catalog, "staged"),
        ("Deleted user".to_string(), None)
    );
    catalog.finish_erasure("acct-writer").unwrap();
}

/// An account with checkpoint attribution left is not finished being erased.
#[test]
fn finish_erasure_waits_for_checkpoint_attribution() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .upsert_account(&contributor("acct-writer", "alice"))
        .unwrap();
    insert_attributed(&catalog, "stable", "alice", Some("acct-writer"));
    insert_attributed(&catalog, "legacy", "acct-writer", None);
    catalog
        .begin_erasure("acct-writer", "generation-2")
        .unwrap();
    assert!(matches!(
        catalog.finish_erasure("acct-writer"),
        Err(crate::storage::catalog::CatalogError::Conflict(_))
    ));
    catalog
        .erase_account_batch("acct-writer", "checkpoints", None, 0, 1000)
        .unwrap();
    catalog.finish_erasure("acct-writer").unwrap();
}

/// A local backup is a `VACUUM INTO` image, so the identity distinction has to
/// survive it: attributed, unattributed and already-erased rows all come back
/// as they were, and an erasure done after the backup was taken is not undone
/// in the live catalogue by restoring one elsewhere.
#[test]
fn vacuum_backup_preserves_the_identity_distinction() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(dir.path().join("catalog.db")).unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .upsert_account(&contributor("acct-writer", "alice"))
        .unwrap();
    catalog
        .upsert_account(&contributor("acct-gone", "bob"))
        .unwrap();
    insert_attributed(&catalog, "stable", "alice", Some("acct-writer"));
    insert_attributed(&catalog, "anonymous", "Reviewer two", None);
    catalog.begin_erasure("acct-gone", "generation-2").unwrap();
    insert_attributed(&catalog, "erased", "bob", Some("acct-gone"));
    let snapshot = dir.path().join("backup.db");
    catalog
        .with_connection(|connection| {
            connection
                .execute("VACUUM INTO ?1", [&snapshot.to_string_lossy().to_string()])
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    let restored = Catalog::open(&snapshot).unwrap();
    assert_eq!(restored.schema_version().unwrap(), 2);
    assert_eq!(
        attribution_of(&restored, "stable"),
        ("alice".to_string(), Some("acct-writer".to_string()))
    );
    assert_eq!(
        attribution_of(&restored, "anonymous"),
        ("Reviewer two".to_string(), None)
    );
    assert_eq!(
        attribution_of(&restored, "erased"),
        ("Deleted user".to_string(), None),
        "attribution refused at the write boundary is refused in the image too"
    );
}

#[test]
fn project_names_are_unique_per_owner_across_creation_and_rename() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let mut second = document();
    second.slug = "second".into();
    second.storage_id = "storage-2".into();
    second.title = "  DOCUMENT  ".into();
    // v2 admitted document rows carry bytes only after object admission. The
    // title uniqueness assertion is independent of that physical closure.
    second.size = 0;
    second.counted_size = 0;
    assert!(
        matches!(catalog.create_document_admitted(&second, 1000, 1000, 10, 10),
        Err(CatalogError::Conflict(message)) if message.contains("project with this name"))
    );
    assert!(catalog.document("second").unwrap().is_none());
    second.title = "Another project".into();
    let mut saved = catalog
        .create_document_admitted(&second, 1000, 1000, 10, 10)
        .unwrap();
    saved.title = "document".into();
    assert!(matches!(
        catalog.update_document(&saved),
        Err(CatalogError::Conflict(_))
    ));
    assert_eq!(
        catalog.document("second").unwrap().unwrap().title,
        "Another project"
    );
    // Keeping one's own name is valid, and a different owner has a separate namespace.
    catalog
        .update_document(&catalog.document("doc").unwrap().unwrap())
        .unwrap();
    let mut other = account();
    other.id = "acct-2".into();
    other.handle = "bob".into();
    catalog.upsert_account(&other).unwrap();
    second.slug = "third".into();
    second.storage_id = "storage-3".into();
    second.owner_id = Some(other.id);
    second.title = "Document".into();
    catalog.create_document(&second).unwrap();
    assert!(matches!(
        catalog.transfer_ownership("third", "acct-1", 1000),
        Err(CatalogError::Conflict(_))
    ));
}

#[test]
fn v2_erasure_reopens_with_revocation_and_250_row_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let catalog = Catalog::open(&path).unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog
        .with_connection(|connection| {
            for index in 0..251 {
                let document_id = format!("document-{index:03}");
                connection.execute(
                    "INSERT INTO documents
                     (id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path)
                     VALUES(?1,?2,'acct-1','owned',?2,?2,'active',0,0,'markdown','README.md')",
                    rusqlite::params![document_id, format!("slug-{index:03}")],
                )?;
                connection.execute(
                    "INSERT INTO grants(document_id,account_id,role,created_at)
                     VALUES(?1,'acct-1','reader',0)",
                    [&document_id],
                )?;
            }
            Ok(())
        })
        .unwrap();
    catalog
        .begin_erasure("acct-1", "generation-erasing")
        .unwrap();
    let (status, generation, operation_id): (String, String, String) = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT a.status,a.session_generation,o.id
                     FROM accounts a JOIN operations o ON o.account_id=a.id
                      AND o.kind='erase_account' AND o.state='prepared'
                     WHERE a.id='acct-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(status, "erasing");
    assert_eq!(generation, "generation-erasing");
    assert_eq!(operation_id.len(), 32);
    drop(catalog);

    let reopened = Catalog::open(&path).unwrap();
    assert_eq!(
        reopened.erasure_stage("acct-1").unwrap().as_deref(),
        Some("owned_documents")
    );
    assert_eq!(
        reopened
            .erase_account_batch("acct-1", "owned_documents", None, 1, 1000)
            .unwrap(),
        250
    );
    let deleting: i64 = reopened
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM documents WHERE owner_id='acct-1' AND status='deleting'",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(deleting, 250);
    assert_eq!(
        reopened
            .erase_account_batch("acct-1", "grants", None, 2, 1000)
            .unwrap(),
        250
    );
    let remaining: i64 = reopened
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM grants WHERE account_id='acct-1'",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(remaining, 1);
}

#[test]
fn v2_erasure_finalization_removes_account_operation_after_fk_cleanup() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog
        .begin_erasure("acct-1", "generation-erasing")
        .unwrap();
    catalog.finish_erasure("acct-1").unwrap();
    assert!(catalog.account("acct-1").unwrap().is_none());
    let operations: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM operations WHERE account_id='acct-1'",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(operations, 0);
}

#[test]
fn v2_erasure_drains_annotation_children_within_250_row_budget() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog
        .begin_erasure("acct-1", "generation-erasing")
        .unwrap();
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO documents
                 (id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,
                  source_format,main_path)
                 VALUES('doc-annotations','annotations','acct-1','owned','Annotations',
                        'annotations','active',0,0,'markdown','README.md')",
                [],
            )?;
            connection.execute(
                r#"INSERT INTO annotations
                 (document_id,id,seq,kind,body,author_account_id,author_key,author_label,
                  via,created_at,updated_at,selector_json,context_json)
                 VALUES('doc-annotations','annotation-1',1,'comment','body','acct-1',
                        'account:acct-1','Account','web',0,0,'{}','{"version":1}')"#,
                [],
            )?;
            for index in 0..251 {
                connection.execute(
                    "INSERT INTO replies
                     (document_id,annotation_id,id,body,author_account_id,author_key,
                      author_label,created_at,updated_at)
                     VALUES('doc-annotations','annotation-1',?1,'reply',NULL,'account:other',
                            'Other',?2,?2)",
                    rusqlite::params![format!("reply-{index:03}"), index as i64],
                )?;
            }
            Ok(())
        })
        .unwrap();

    assert_eq!(
        catalog
            .erase_account_batch("acct-1", "annotation_replies", None, 1, 1000)
            .unwrap(),
        250
    );
    let remaining: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM replies
                     WHERE document_id='doc-annotations' AND annotation_id='annotation-1'",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(remaining, 1);
    assert_eq!(
        catalog
            .erase_account_batch("acct-1", "annotation_replies", None, 2, 1000)
            .unwrap(),
        1
    );
    assert_eq!(
        catalog
            .erase_account_batch("acct-1", "annotations", None, 3, 1000)
            .unwrap(),
        1
    );
}

#[test]
fn v2_erasure_worker_resumes_pinned_operation_and_refuses_live_child_cascade() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog
        .begin_erasure("acct-1", "generation-erasing")
        .unwrap();
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO documents
                 (id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,
                  source_format,main_path)
                 VALUES('doc-worker','worker','acct-1','owned','Worker','worker','active',0,0,
                        'markdown','README.md')",
                [],
            )?;
            connection.execute(
                r#"INSERT INTO annotations
                 (document_id,id,seq,kind,body,author_account_id,author_key,author_label,
                  via,created_at,updated_at,selector_json,context_json)
                 VALUES('doc-worker','annotation-worker',1,'comment','body','acct-1',
                        'account:acct-1','Account','web',0,0,'{}','{"version":1}')"#,
                [],
            )?;
            let generation: String = connection.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1",
                [],
                |row| row.get(0),
            )?;
            connection.execute(
                r#"INSERT INTO operations
                 (id,document_id,actor_key,request_key,kind,request_digest,state,
                  writer_generation,result_json,created_at,updated_at,completed_at,receipt_expires_at)
                 VALUES('operation-pinned','doc-worker','acct-1','worker-request','source_publish',
                        'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                        'aborted',?1,'{"version":1}',0,0,0,0)"#,
                [&generation],
            )?;
            connection.execute(
                r#"INSERT INTO objects
                 (document_id,id,storage_key,kind,state,digest,reserved_bytes,
                  allocation_operation_id,created_at)
                 VALUES('doc-worker','object-pinned','objects/pinned','source_chunk','allocated',
                        'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                        1,'operation-pinned',0)"#,
                [],
            )?;
            Ok(())
        })
        .unwrap();

    crate::storage::maintenance::run_erasure_pass(&catalog, 1, 1, 1).unwrap();
    crate::storage::maintenance::run_erasure_pass(&catalog, 2, 1, 1).unwrap();
    assert_eq!(
        catalog.erasure_stage("acct-1").unwrap().as_deref(),
        Some("operations")
    );

    catalog
        .with_connection(|connection| {
            connection.execute(
                "DELETE FROM objects WHERE document_id='doc-worker' AND id='object-pinned'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    // Persist the child-drain stage while the parent still exists. This
    // models a worker crash between stage transition and its next pass and
    // keeps the live-reply fixture valid under the parent foreign key.
    catalog
        .erasure_batch("acct-1", "annotation_replies", None, 3, 1)
        .unwrap();
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO replies
                 (document_id,annotation_id,id,body,author_account_id,author_key,author_label,
                  created_at,updated_at)
                 VALUES('doc-worker','annotation-worker','live-reply','reply',NULL,
                        'account:other','Other',0,0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    crate::storage::maintenance::run_erasure_pass(&catalog, 3, 1, 1).unwrap();
    let still_there: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM annotations
                     WHERE document_id='doc-worker' AND id='annotation-worker'",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(still_there, 1);
}

#[test]
fn annotation_receipt_rechecks_live_link_and_preserves_seven_day_window() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let link = "b".repeat(64);
    catalog.with_connection(|db| {
        db.execute_batch("INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path)
            VALUES('doc','doc','acct-1','owned','Title','title','active',0,0,'markdown','main.md');
            UPDATE accounts SET document_count=1 WHERE id='acct-1';
            UPDATE server_state SET document_count=1 WHERE id=1;")?;
        db.execute("INSERT INTO links(document_id,id,role,token_hash,sealed_token,sealing_key_id,label,created_at)
            VALUES('doc','comment-link','commenter',?1,X'00','test','Reviewer',0)",[&link])?;
        Ok(())
    }).unwrap();
    let authority = AnnotationAuthority {
        link_hash: &link,
        policy_comment: true,
        ..Default::default()
    };
    let key = crate::util::new_request_key();
    let digest = "c".repeat(64);
    let comment = annotation("comment-link-retry", "commenting");
    let now = crate::util::now_millis();
    let inserted = catalog
        .insert_comment_request_authorized(&comment, &key, &digest, now, authority)
        .unwrap();
    let replay = catalog
        .insert_comment_request_authorized(&comment, &key, &digest, now, authority)
        .unwrap();
    assert_eq!(inserted, replay);
    let mut changed = inserted.clone();
    changed.body = "Updated words".into();
    let own_authority = AnnotationAuthority {
        author_key: "acct-1",
        ..authority
    };
    let foreign_authority = AnnotationAuthority {
        author_key: "another-visitor",
        ..authority
    };
    assert!(matches!(
        catalog.update_comment_authorized(&changed, foreign_authority),
        Err(CatalogError::Refused(super::CatalogRefusal::ActorRights, _))
    ));
    catalog
        .update_comment_authorized(&changed, own_authority)
        .unwrap();
    let suggestion = catalog
        .insert_comment_request_authorized(
            &annotation("suggestion-wire", "editing"),
            &crate::util::new_request_key(),
            &digest,
            now,
            authority,
        )
        .unwrap();
    assert_eq!(suggestion.motivation, "editing");
    assert_eq!(suggestion.outcome, "");
    let highlight = catalog
        .insert_comment_request_authorized(
            &annotation("highlight-wire", "highlighting"),
            &crate::util::new_request_key(),
            &digest,
            now,
            authority,
        )
        .unwrap();
    assert_eq!(highlight.motivation, "highlighting");
    catalog.with_connection(|db| {
        let (created,completed,expiry):(i64,i64,i64) = db.query_row("SELECT created_at,completed_at,receipt_expires_at FROM operations WHERE request_key=?1",[&key],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
        assert_eq!(created,now);
        assert_eq!(expiry-completed,7*24*60*60*1000);
        Ok(())
    }).unwrap();
    // Expiry is effective even if the maintenance worker has not removed the receipt.
    catalog.with_connection(|db| {
        db.execute("UPDATE operations SET created_at=1,updated_at=1,completed_at=1,receipt_expires_at=2 WHERE request_key=?1",[&key])?;
        Ok(())
    }).unwrap();
    assert_eq!(catalog.insert_comment_request_authorized(&comment, &key, &digest, now, authority).unwrap_err().refusal(), super::CatalogRefusal::RequestExpired);
    let forgotten_key = format!("v2.1.{}", "a".repeat(32));
    assert_eq!(catalog.insert_comment_request_authorized(&annotation("never-inserted", "commenting"), &forgotten_key, &digest, now, authority).unwrap_err().refusal(), super::CatalogRefusal::RequestExpired);
    assert!(matches!(catalog.comment("doc", "never-inserted"), Err(CatalogError::NotFound)));
    catalog.with_connection(|db| {
        db.execute("DELETE FROM links WHERE token_hash=?1",[&link])?;
        Ok(())
    }).unwrap();
    assert!(matches!(
        catalog.insert_comment_request_authorized(&comment, &key, &digest, now, authority),
        Err(CatalogError::Refused(super::CatalogRefusal::ActorRights, _))
    ));
}

#[test]
fn v2_document_worker_bounds_checkpoint_edges_and_repeated_begin() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .with_connection(|connection| {
            let generation: String = connection.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1",
                [],
                |row| row.get(0),
            )?;
            connection.execute(
                r#"INSERT INTO objects
                 (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,
                  created_at,live_root,gc_after)
                 VALUES('storage-1','tree-object','objects/tree','source_tree','available',
                        'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                        1,0,0,1,100)"#,
                [],
            )?;
            for edge in 0..1024 {
                connection.execute(
                    r#"INSERT INTO objects
                     (document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,
                      created_at)
                     VALUES('storage-1',?1,?2,'source_chunk','available',
                            'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                            1,0,0)"#,
                    rusqlite::params![
                        format!("edge-{edge:04}"),
                        format!("objects/edge-{edge:04}"),
                    ],
                )?;
            }
            for checkpoint in 0..33 {
                let checkpoint_id = format!("checkpoint-{checkpoint:02}");
                connection.execute(
                    r#"INSERT INTO checkpoints
                     (document_id,id,seq,tree_object_id,tree_digest,parent_id,created_at,
                      author_account_id,author_label,reason,source_format,logical_bytes,
                      label,journal_epoch,journal_sequence,metadata_json)
                     VALUES('storage-1',?1,?2,'tree-object',
                            'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
                            NULL,0,'acct-1','Alice','manual','markdown',1,NULL,0,0,
                            '{"version":1}')"#,
                    rusqlite::params![checkpoint_id, i64::from(checkpoint + 1)],
                )?;
                for edge in 0..1024 {
                    connection.execute(
                        "INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id)
                         VALUES('storage-1',?1,?2)",
                        rusqlite::params![
                            checkpoint_id,
                            format!("edge-{edge:04}"),
                        ],
                    )?;
                }
            }
            connection.execute(
                "UPDATE documents SET checkpoint_ref_count=33792 WHERE id='storage-1'",
                [],
            )?;
            connection.execute(
                "UPDATE server_state SET checkpoint_ref_count=33792 WHERE id=1",
                [],
            )?;
            connection.execute(
                r#"INSERT INTO operations
                 (id,document_id,actor_key,request_key,kind,request_digest,state,
                  writer_generation,result_json,created_at,updated_at,completed_at,receipt_expires_at)
                 VALUES('operation-pinned','storage-1','acct-1','worker-request','source_publish',
                        'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',
                        'aborted',?1,'{"version":1}',0,0,0,0)"#,
                [&generation],
            )?;
            connection.execute(
                r#"INSERT INTO objects
                 (document_id,id,storage_key,kind,state,digest,reserved_bytes,
                  allocation_operation_id,created_at)
                 VALUES('storage-1','allocated-object','objects/allocated','source_chunk','allocated',
                        'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
                        1,'operation-pinned',0)"#,
                [],
            )?;
            Ok(())
        })
        .unwrap();

    catalog.begin_delete("doc").unwrap();
    catalog.begin_delete("doc").unwrap();
    let gc_after: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT gc_after FROM objects WHERE id='tree-object'",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(gc_after, 100);

    catalog.erase_document_batch("doc", 250, 1_000).unwrap();
    catalog.erase_document_batch("doc", 250, 1_000).unwrap();
    catalog.erase_document_batch("doc", 250, 1_000).unwrap();
    assert_eq!(catalog.erase_document_batch("doc", 250, 1_000).unwrap(), 1);
    let mut pinned_batch = None;
    for _ in 0..8 {
        let changed = catalog.erase_document_batch("doc", 250, 1_000).unwrap();
        if changed == 0 {
            pinned_batch = Some(changed);
            break;
        }
    }
    assert_eq!(pinned_batch, Some(0));
    let remaining_edges: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM checkpoint_objects WHERE document_id='storage-1'",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(remaining_edges, 0);
    let counters: (i64, i64) = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT d.checkpoint_ref_count,
                            (SELECT checkpoint_ref_count FROM server_state WHERE id=1)
                     FROM documents d WHERE d.id='storage-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(counters, (0, 0));
    let pinned_rows: (i64, i64) = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM operations WHERE id='operation-pinned'),
                            (SELECT COUNT(*) FROM objects WHERE id='allocated-object')",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    assert_eq!(pinned_rows, (1, 1));

}

#[test]
fn obsolete_replacement_alias_is_not_a_display_publication() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let operation = catalog
        .prepare_operation(&OperationRequest {
            storage_id: "storage-1",
            request_id: &crate::util::new_request_key(),
            kind: "replace",
            request_digest: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            intent: "{}",
            created_at: crate::util::now_millis(),
            actor: None,
        });
    assert!(matches!(operation, Err(CatalogError::Invalid(_))));
}

#[test]
fn internal_checkpoint_fence_requires_a_live_document_owner() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .require_internal_checkpoint_authority("doc")
        .unwrap();
    catalog
        .with_connection(|connection| {
            connection.execute("UPDATE accounts SET status='erasing' WHERE id='acct-1'", [])?;
            Ok(())
        })
        .unwrap();
    assert!(catalog
        .require_internal_checkpoint_authority("doc")
        .is_err());
}

#[test]
fn agent_operation_plan_is_versioned_without_persisting_bearer_credentials() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let request_id = crate::util::new_request_key();
    let operation = catalog
        .prepare_operation(&OperationRequest {
            storage_id: "storage-1",
            request_id: &request_id,
            kind: "agent_apply",
            request_digest: &"a".repeat(64),
            intent: r#"{"actor":{"account_id":"acct-1","generation":"generation-1","owner_key":"transient-secret"},"before_tree":"before","after_tree":"after"}"#,
            created_at: crate::util::now_millis(),
            actor: None,
        })
        .unwrap();
    let plan: serde_json::Value = serde_json::from_str(&operation.intent).unwrap();
    assert_eq!(plan.get("version").and_then(serde_json::Value::as_i64), Some(2));
    assert!(plan
        .get("actor")
        .and_then(|actor| actor.get("owner_key"))
        .is_none());
    assert_eq!(
        plan.get("actor")
            .and_then(|actor| actor.get("generation"))
            .and_then(serde_json::Value::as_str),
        Some("generation-1")
    );
}

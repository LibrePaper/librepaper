use serde_json::json;
use std::sync::Arc;

use crate::config::Configuration;
use crate::room::{Command, Message, RoomSet};
use crate::storage::blob::FsStore;

#[test]
fn wire_adapter_rejects_unknown_kinds_without_losing_correlation() {
    let error = Message {
        kind: "future-comment-operation".into(),
        temp_id: "optimistic-17".into(),
        request_id: "request-17".into(),
        ..Message::default()
    }
    .into_command()
    .expect_err("unknown command must be rejected");

    assert_eq!(error.message(), "unknown message type");
    let response = error.response();
    assert_eq!(response["temp_id"], "optimistic-17");
    assert_eq!(response["request_id"], "request-17");
}

#[test]
fn required_fields_report_the_operation_and_field() {
    let error = Message {
        kind: "reply".into(),
        request_id: "reply-1".into(),
        ..Message::default()
    }
    .into_command()
    .expect_err("reply without its target must be rejected");

    assert_eq!(error.message(), "reply requires comment_id");

    let error = Message {
        kind: "accept".into(),
        ..Message::default()
    }
    .into_command()
    .expect_err("the test must use an operation with a missing field");
    assert_eq!(error.message(), "accept requires comment_id");
}

#[test]
fn adapter_preserves_fields_used_by_retry_identity() {
    let message = Message {
        kind: "comment".into(),
        motivation: "editing".into(),
        body: " note ".into(),
        exact: "passage".into(),
        prefix: "before".into(),
        suffix: "after".into(),
        proposed: Some(String::new()),
        temp_id: "temp-1".into(),
        request_id: "request-1".into(),
        ..Message::default()
    };
    let command = message.into_command().unwrap();
    let Command::Comment {
        body,
        exact,
        proposed,
        temp_id,
        request_id,
        ..
    } = command
    else {
        panic!("comment wire frame did not produce a Comment command");
    };
    assert_eq!(body, " note ");
    assert_eq!(exact, "passage");
    assert_eq!(proposed, Some(String::new()));
    assert_eq!(temp_id, "temp-1");
    assert_eq!(request_id, "request-1");
}

#[test]
fn error_response_is_a_protocol_error() {
    let error = Message {
        kind: "resolve".into(),
        request_id: "r".into(),
        ..Message::default()
    }
    .into_command()
    .unwrap_err();
    assert_eq!(
        error.response(),
        json!({
            "type": "error",
            "message": "resolve requires comment_id",
            "temp_id": "",
            "request_id": "r",
            "version": 1,
            "protocol": "komodoc.room.v1",
        })
    );
}

#[test]
fn malformed_anchor_error_keeps_the_target_for_optimistic_rollback() {
    let error = Message {
        kind: "anchor".into(),
        comment_id: "comment-7".into(),
        request_id: "anchor-7".into(),
        ..Message::default()
    }
    .into_command()
    .unwrap_err();
    assert_eq!(error.response()["comment_id"], "comment-7");
}

#[tokio::test]
async fn unknown_command_does_not_consume_comment_allowance() {
    let dir = tempfile::tempdir().unwrap();
    let rooms = RoomSet::new(
        Arc::new(FsStore::new(dir.path())),
        Arc::new(Configuration::default()),
    );
    let room = rooms.get("command-rate").await;

    let (error, ok) = room
        .apply(
            Message {
                kind: "unknown".into(),
                temp_id: "123e4567-e89b-12d3-a456-426614174000".into(),
                ..Message::default()
            },
            "198.51.100.7",
            "visitor:test",
            "",
            Some(1),
            false,
        )
        .await;
    assert!(!ok);
    assert_eq!(error["message"], "unknown message type");

    let (result, ok) = room
        .apply(
            Message {
                kind: "comment".into(),
                exact: "text".into(),
                body: "a valid comment".into(),
                ..Message::default()
            },
            "198.51.100.7",
            "visitor:test",
            "",
            Some(1),
            false,
        )
        .await;
    assert!(ok, "valid command was charged by unknown command: {result}");

    let comment_id = result["comment"]["id"].as_str().unwrap().to_owned();
    let reply_id = "123e4567-e89b-12d3-a456-426614174001";
    let (first_reply, ok) = room
        .apply(
            Message {
                kind: "reply".into(),
                comment_id: comment_id.clone(),
                body: "a reply".into(),
                temp_id: reply_id.into(),
                ..Message::default()
            },
            "198.51.100.7",
            "visitor:test",
            "",
            Some(3),
            false,
        )
        .await;
    assert!(ok, "reply was refused: {first_reply}");
    let (retry, ok) = room
        .apply(
            Message {
                kind: "reply".into(),
                comment_id,
                temp_id: reply_id.into(),
                ..Message::default()
            },
            "198.51.100.7",
            "visitor:test",
            "",
            Some(3),
            false,
        )
        .await;
    assert!(ok, "retry with an empty body was not idempotent: {retry}");
    assert_eq!(retry["reply"]["id"], reply_id);
}

use serde_json::json;
use std::sync::Arc;

use crate::config::Configuration;
use crate::room::{Command, Message, RoomSet, SourceAnchor};
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
    let round_trip = message.clone().into_command().unwrap().into_message();
    assert_eq!(
        serde_json::to_value(message).unwrap(),
        serde_json::to_value(round_trip).unwrap()
    );
}

#[test]
fn every_comment_command_is_a_real_internal_variant() {
    let cases = [
        (
            "comment",
            Command::Comment {
                motivation: String::new(),
                body: "body".into(),
                creator: String::new(),
                exact: "text".into(),
                prefix: String::new(),
                suffix: String::new(),
                position: None,
                region: None,
                source: None,
                proposed: None,
                temp_id: String::new(),
                request_id: String::new(),
            },
        ),
        (
            "reply",
            Command::Reply {
                comment_id: "c".into(),
                body: "body".into(),
                creator: String::new(),
                temp_id: String::new(),
                request_id: String::new(),
            },
        ),
        (
            "resolve",
            Command::Resolve {
                comment_id: "c".into(),
                resolved: true,
                temp_id: String::new(),
                request_id: String::new(),
            },
        ),
        (
            "delete",
            Command::Delete {
                comment_id: "c".into(),
                temp_id: String::new(),
                request_id: String::new(),
            },
        ),
        (
            "anchor",
            Command::Anchor {
                comment_id: "c".into(),
                source: SourceAnchor::default(),
                temp_id: String::new(),
                request_id: String::new(),
            },
        ),
        (
            "accept",
            Command::Accept {
                comment_id: "c".into(),
                temp_id: String::new(),
                request_id: String::new(),
            },
        ),
        (
            "reject",
            Command::Reject {
                comment_id: "c".into(),
                temp_id: String::new(),
                request_id: String::new(),
            },
        ),
    ];
    for (kind, command) in cases {
        assert_eq!(command.kind(), kind);
    }
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
}

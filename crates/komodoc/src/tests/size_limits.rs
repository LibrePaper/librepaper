//! Track 10: admission and persistence size limits.
//!
//! The invariant every test here defends is that nothing is accepted,
//! acknowledged, or relayed that persistence cannot carry, and that a
//! deployment which is merely busy is never told its document can never fit.

use crate::config::{
    CapacityRefusal, Configuration, PersistenceLimits, SizeRefusal, WriteRefusal,
    DEFAULT_MAX_ENCODED_SNAPSHOT_BYTES, SUPPORTED_MAX_SOURCE_BYTES,
};

const MIB: usize = 1024 * 1024;

#[test]
fn the_default_policy_can_process_one_maximum_snapshot() {
    PersistenceLimits::default()
        .validate()
        .expect("the shipped defaults must be a supported configuration");
}

#[test]
fn the_supported_maximum_source_ceiling_is_accepted() {
    let mut config = Configuration::default();
    config
        .set_max_document(SUPPORTED_MAX_SOURCE_BYTES / MIB)
        .expect("the supported maximum must be configurable");
    assert_eq!(config.max_document, SUPPORTED_MAX_SOURCE_BYTES);
    config.persistence().validate().expect("and must validate");
}

#[test]
fn the_formerly_accepted_hundred_megabyte_configuration_is_refused() {
    let mut config = Configuration::default();
    let error = config
        .set_max_document(100)
        .expect_err("100 MB used to be accepted and could never be durably saved");
    assert!(error.contains("--max-size"), "{error}");
    assert!(error.contains("not supported"), "{error}");
    // Refused, not silently clamped: the operator asked for something this
    // deployment cannot do and has to be told so.
    assert_eq!(config.max_document, Configuration::default().max_document);
}

#[test]
fn one_megabyte_over_the_supported_maximum_is_refused() {
    let mut config = Configuration::default();
    assert!(config
        .set_max_document(SUPPORTED_MAX_SOURCE_BYTES / MIB + 1)
        .is_err());
}

#[test]
fn a_source_ceiling_above_the_encoded_ceiling_is_refused() {
    let limits = PersistenceLimits {
        max_source_bytes: 32 * MIB,
        max_encoded_snapshot_bytes: 16 * MIB,
        ..PersistenceLimits::default()
    };
    assert!(limits.validate().is_err());
}

#[test]
fn an_encoded_ceiling_past_recovery_decoding_is_refused() {
    let limits = PersistenceLimits {
        max_encoded_snapshot_bytes: 128 * MIB,
        max_queued_payload_bytes: 256 * MIB,
        max_staging_bytes: 4096 * MIB,
        ..PersistenceLimits::default()
    };
    let error = limits
        .validate()
        .expect_err("a snapshot that cannot be decoded back must not be writable");
    assert!(error.contains("recovery base"), "{error}");
}

#[test]
fn a_queue_that_cannot_hold_one_snapshot_is_refused() {
    let limits = PersistenceLimits {
        max_queued_payload_bytes: 8 * MIB,
        ..PersistenceLimits::default()
    };
    let error = limits.validate().expect_err("Q must fit one E");
    assert!(error.contains("payload budget"), "{error}");
}

#[test]
fn a_memory_budget_that_cannot_hold_one_snapshot_is_refused() {
    let limits = PersistenceLimits {
        max_staging_bytes: 32 * MIB,
        ..PersistenceLimits::default()
    };
    let error = limits.validate().expect_err("M must fit one snapshot peak");
    assert!(error.contains("memory budget"), "{error}");
}

#[test]
fn derived_bounds_use_checked_arithmetic() {
    // A ceiling near the top of the address space must produce an error, not
    // a wrapped product that silently admits everything.
    let limits = PersistenceLimits {
        max_source_bytes: 1,
        max_encoded_snapshot_bytes: usize::MAX,
        max_queued_payload_bytes: usize::MAX,
        max_staging_bytes: usize::MAX,
    };
    assert!(limits.validate().is_err());
}

#[test]
fn a_temporary_refusal_never_reads_as_a_permanent_one() {
    let temporary = WriteRefusal::Temporary(CapacityRefusal::JournalQueue);
    assert!(!temporary.is_permanent());
    assert!(temporary.message().contains("try again"));
    let permanent = WriteRefusal::Permanent(SizeRefusal::Encoded {
        bytes: 32 * MIB,
        ceiling: DEFAULT_MAX_ENCODED_SNAPSHOT_BYTES,
    });
    assert!(permanent.is_permanent());
    assert!(!permanent.message().contains("try again"));
    assert!(permanent.message().contains("history"));
}

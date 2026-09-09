//! Durable local edit-journal primitives.
//!
//! The journal deliberately has no knowledge of rooms or Yjs.  It owns the
//! bounded record/segment format and the small SQL state transition used by a
//! coordinator.  Object writes are performed before `commit_segment`; callers
//! must therefore treat a failed commit as an unknown outcome and reconcile by
//! operation id before retrying.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::storage::blob::{
    journal_base_key, journal_manifest_key, journal_segment_key, BlobError, BlobStore,
};
use crate::storage::catalog::{Catalog, CatalogError, CatalogResult};

mod budget;
mod coordinator;
mod recovery;
mod runtime;
mod segment;
mod store;

pub use budget::*;
pub use coordinator::*;
pub use recovery::*;
pub use runtime::*;
pub use segment::*;
pub use store::*;

#[derive(Debug)]
pub enum JournalError {
    Invalid(String),
    Corrupt(String),
    /// A permanent size refusal: the work is past a ceiling this journal
    /// format supports, and no retry of the same work can succeed.
    Limit(String),
    /// A temporary capacity refusal: a shared budget is momentarily full and
    /// the same work may succeed once another operation settles. It is a
    /// separate variant because reporting saturation as a permanent limit
    /// tells a person their document can never be saved, which is false.
    Busy(String),
    Storage(String),
    Catalog(CatalogError),
}

impl JournalError {
    /// Whether retrying the identical request is pointless.
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::Limit(_) | Self::Invalid(_) | Self::Corrupt(_))
    }

    /// Whether the same request may succeed once capacity frees up.
    pub fn is_temporary(&self) -> bool {
        matches!(self, Self::Busy(_))
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid journal request: {message}"),
            Self::Corrupt(message) => write!(f, "corrupt journal segment: {message}"),
            Self::Limit(message) => write!(f, "journal limit exceeded: {message}"),
            Self::Busy(message) => write!(f, "journal capacity is full: {message}"),
            Self::Storage(message) => write!(f, "journal storage error: {message}"),
            Self::Catalog(error) => write!(f, "journal catalogue error: {error}"),
        }
    }
}

impl std::error::Error for JournalError {}

impl From<CatalogError> for JournalError {
    fn from(error: CatalogError) -> Self {
        Self::Catalog(error)
    }
}

/// Admission saturation is capacity, not a permanent refusal: reporting it as
/// a catalogue error would tell a caller its work can never succeed, when the
/// same request will be admitted once another catalogue job settles.
impl From<crate::storage::catalog::CatalogExecError> for JournalError {
    fn from(error: crate::storage::catalog::CatalogExecError) -> Self {
        match error {
            crate::storage::catalog::CatalogExecError::Saturated => {
                Self::Busy("the catalogue execution boundary is saturated".into())
            }
            other => Self::Catalog(CatalogError::from(other)),
        }
    }
}

impl From<BlobError> for JournalError {
    fn from(error: BlobError) -> Self {
        Self::Storage(error.to_string())
    }
}

pub type JournalResult<T> = Result<T, JournalError>;

impl JournalRuntime {}

impl JournalStore {}

#[cfg(test)]
mod tests;

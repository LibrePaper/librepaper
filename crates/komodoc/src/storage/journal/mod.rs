//! Durable local edit-journal primitives.
//!
//! The journal deliberately has no knowledge of rooms or Yjs.  It owns the
//! bounded record/segment format and the small SQL state transition used by a
//! coordinator.  Object writes are performed before `commit_segment`; callers
//! must therefore treat a failed commit as an unknown outcome and reconcile by
//! operation id before retrying.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::storage::blob::{
    journal_base_key, journal_manifest_key, journal_segment_key, BlobError, BlobStore,
};
use crate::storage::catalog::{Catalog, CatalogError};

mod coordinator;
mod recovery;
mod runtime;
mod segment;
mod store;

pub use coordinator::*;
pub use recovery::*;
pub use runtime::*;
pub use segment::*;
pub use store::*;

#[derive(Debug)]
pub enum JournalError {
    Invalid(String),
    Corrupt(String),
    Limit(String),
    Storage(String),
    Catalog(CatalogError),
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid journal request: {message}"),
            Self::Corrupt(message) => write!(f, "corrupt journal segment: {message}"),
            Self::Limit(message) => write!(f, "journal limit exceeded: {message}"),
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

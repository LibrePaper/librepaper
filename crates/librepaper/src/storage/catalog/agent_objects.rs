//! Agent payload objects are ordinary v2 `agent_payload` objects.
//!
//! The catalogue stores their digest, canonical storage key, allocation
//! operation and accounting.  Payload bytes are owned by the object store and
//! are never copied into SQL or hidden in an operation JSON document.
use super::*;

impl Catalog {
    pub fn put_agent_object(
        &self,
        _slug: &str,
        _actor: &str,
        _id: &str,
        _kind: &str,
        _payload: &[u8],
        _expires_at: i64,
    ) -> CatalogResult<()> {
        Err(CatalogError::Invalid(
            "put_agent_object was removed; allocate and settle an agent_payload object through the v2 operation API".into(),
        ))
    }

    pub fn agent_object(
        &self,
        _slug: &str,
        _actor: &str,
        _id: &str,
        _kind: &str,
    ) -> CatalogResult<Option<Vec<u8>>> {
        Err(CatalogError::Invalid(
            "agent_object payload reads use the object store and v2 read lease API".into(),
        ))
    }
}

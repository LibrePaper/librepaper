//! Binary recovery-base framing for the native document journal.

use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RecoveryBaseBody {
    pub(crate) format_version: u16,
    pub(crate) storage_id: String,
    pub(crate) epoch: u64,
    pub(crate) sequence: u64,
    pub(crate) payload: Vec<u8>,
    pub(crate) digest: String,
}

const RECOVERY_BASE_MAGIC: &[u8; 4] = b"KJBS";
const RECOVERY_BASE_CODEC_VERSION: u16 = 1;
/// Compaction receives a complete snapshot, which may be larger than one
/// journal record. Keep its aggregate bounded independently of record framing.
pub const MAX_RECOVERY_BASE_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Bytes surrounding a recovery-base payload for a document identity of the
/// supplied length. Keep this derived from the fixed binary fields so the
/// storage adapter can distinguish logical snapshot budget from envelope
/// overhead without a guessed constant.
pub const fn recovery_base_framing_bytes(identity_len: usize) -> usize {
    96 + identity_len
}

/// Encode recovery bases as bounded binary objects.
pub(crate) fn encode_recovery_base(body: &RecoveryBaseBody) -> JournalResult<Vec<u8>> {
    validate_recovery_base(body)?;
    let mut encoded = Vec::with_capacity(64 + body.storage_id.len() + body.payload.len());
    encoded.extend_from_slice(RECOVERY_BASE_MAGIC);
    put_u16(&mut encoded, RECOVERY_BASE_CODEC_VERSION);
    put_u16(&mut encoded, body.format_version);
    put_bytes_u16(&mut encoded, body.storage_id.as_bytes())?;
    put_u64(&mut encoded, body.epoch);
    put_u64(&mut encoded, body.sequence);
    put_u32(
        &mut encoded,
        u32::try_from(body.payload.len())
            .map_err(|_| JournalError::Limit("recovery base payload is too large".into()))?,
    );
    put_bytes_u16(&mut encoded, body.digest.as_bytes())?;
    encoded.extend_from_slice(&body.payload);
    if encoded.len()
        > MAX_RECOVERY_BASE_PAYLOAD_BYTES
            .saturating_add(recovery_base_framing_bytes(body.storage_id.len()))
    {
        return Err(JournalError::Limit("recovery base is too large".into()));
    }
    Ok(encoded)
}

pub(crate) fn decode_recovery_base(bytes: &[u8]) -> JournalResult<RecoveryBaseBody> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(4)? != RECOVERY_BASE_MAGIC {
        return Err(JournalError::Corrupt("bad recovery base magic".into()));
    }
    if cursor.u16()? != RECOVERY_BASE_CODEC_VERSION {
        return Err(JournalError::Corrupt(
            "unsupported recovery base version".into(),
        ));
    }
    let format_version = cursor.u16()?;
    let storage_id = String::from_utf8(cursor.bytes_u16()?)
        .map_err(|_| JournalError::Corrupt("recovery base storage id is not utf-8".into()))?;
    let epoch = cursor.u64()?;
    let sequence = cursor.u64()?;
    let payload_len = cursor.u32()? as usize;
    if payload_len > MAX_RECOVERY_BASE_PAYLOAD_BYTES {
        return Err(JournalError::Limit("recovery base is too large".into()));
    }
    let digest = String::from_utf8(cursor.bytes_u16()?)
        .map_err(|_| JournalError::Corrupt("recovery base digest is not utf-8".into()))?;
    let payload = cursor.take(payload_len)?.to_vec();
    if !cursor.is_empty() {
        return Err(JournalError::Corrupt("trailing recovery base bytes".into()));
    }
    let body = RecoveryBaseBody {
        format_version,
        storage_id,
        epoch,
        sequence,
        payload,
        digest,
    };
    if bytes.len()
        > MAX_RECOVERY_BASE_PAYLOAD_BYTES
            .saturating_add(recovery_base_framing_bytes(body.storage_id.len()))
    {
        return Err(JournalError::Limit("recovery base is too large".into()));
    }
    validate_recovery_base(&body)?;
    Ok(body)
}

fn validate_recovery_base(body: &RecoveryBaseBody) -> JournalResult<()> {
    if body.format_version != SEGMENT_FORMAT
        || body.storage_id.is_empty()
        || body.sequence == 0
        || body.payload.is_empty()
        || body.payload.len() > MAX_RECOVERY_BASE_PAYLOAD_BYTES
        || body.digest != hex::encode(Sha256::digest(&body.payload))
    {
        return Err(JournalError::Corrupt(
            "invalid recovery base identity or digest".into(),
        ));
    }
    if body.storage_id.len() > u16::MAX as usize || body.digest.len() > u16::MAX as usize {
        return Err(JournalError::Limit(
            "recovery base metadata is too large".into(),
        ));
    }
    Ok(())
}

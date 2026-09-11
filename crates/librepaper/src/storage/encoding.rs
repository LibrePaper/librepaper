//! Verified, content-defined encoding for source-history text.
//!
//! The logical identity of a source file is always the SHA-256 digest of its
//! uncompressed bytes.  This module only describes the physical representation
//! beneath that identity: small files are one zstd object and larger files are
//! a versioned recipe referring to independently compressed FastCDC chunks.
//! Recipes contain raw digests and lengths rather than JSON or hexadecimal
//! strings so their charged metadata is stable and bounded.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Read;
use std::sync::Arc;

use fastcdc::v2020::{FastCDC, Normalization};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

static RECONSTRUCTION_POOL: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();

fn reconstruction_pool() -> &'static Arc<Semaphore> {
    RECONSTRUCTION_POOL.get_or_init(|| Arc::new(Semaphore::new(if cfg!(test) { 32 } else { 2 })))
}

/// The durable recipe wire format.  A wire-format change requires a new
/// version; changing the boundary profile only changes `profile_id`.
pub const RECIPE_VERSION: u16 = 1;
pub const PROFILE_ID: u16 = 1;
pub const ZSTD_LEVEL: i32 = 3;
/// Text at or below this size uses one whole-file object.  This includes the
/// recipe/object overhead in the decision rather than creating tiny chunks.
pub const SMALL_FILE_THRESHOLD: usize = 4 * 1024;
pub const MAX_RECIPE_CHUNKS: usize = 1_000_000;
pub const MAX_RECIPE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_OBJECT_BYTES: u64 = 64 * 1024 * 1024;

/// Initial writer profile: FastCDC v2020, Level1 normalization, seed
/// zero, with a four-KiB target and conservative bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodingProfile {
    pub id: u16,
    pub min_size: usize,
    pub average_size: usize,
    pub max_size: usize,
    pub small_file_threshold: usize,
}

impl Default for EncodingProfile {
    fn default() -> Self {
        Self {
            id: PROFILE_ID,
            min_size: 1024,
            average_size: 4096,
            max_size: 32 * 1024,
            small_file_threshold: SMALL_FILE_THRESHOLD,
        }
    }
}

impl EncodingProfile {
    pub fn validate(self) -> Result<Self, EncodingError> {
        if self.id == 0
            || self.min_size == 0
            || self.min_size > self.average_size
            || self.average_size > self.max_size
            || !self.min_size.is_multiple_of(2)
            || !self.average_size.is_multiple_of(2)
            || !self.max_size.is_multiple_of(2)
            || self.small_file_threshold == 0
        {
            return Err(EncodingError::InvalidProfile);
        }
        Ok(self)
    }
}

/// Physical codec named by a recipe.  Both values use zstd; whole-file and
/// chunked representation have distinct semantics for reuse and GC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Codec {
    WholeZstd = 1,
    ChunkedZstd = 2,
}

impl Codec {
    fn from_wire(value: u8) -> Result<Self, EncodingError> {
        match value {
            1 => Ok(Self::WholeZstd),
            2 => Ok(Self::ChunkedZstd),
            _ => Err(EncodingError::UnsupportedCodec(value)),
        }
    }
}

/// A raw SHA-256 object digest and its uncompressed length.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ChunkRef {
    pub digest: [u8; 32],
    pub length: u32,
}

/// A complete independently readable recipe for one source file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recipe {
    pub version: u16,
    pub profile_id: u16,
    pub codec: Codec,
    pub uncompressed_len: u64,
    pub file_digest: [u8; 32],
    pub chunks: Vec<ChunkRef>,
}

impl Recipe {
    pub fn encoded_len(&self) -> usize {
        RECIPE_HEADER_LEN + self.chunks.len() * CHUNK_REF_LEN
    }

    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(self.to_bytes()).into()
    }

    /// Serialize the compact little-endian recipe. The complete-file digest
    /// and raw lengths make a recipe independently verifiable on read.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.encoded_len());
        bytes.extend_from_slice(RECIPE_MAGIC);
        bytes.extend_from_slice(&self.version.to_le_bytes());
        bytes.extend_from_slice(&self.profile_id.to_le_bytes());
        bytes.push(self.codec as u8);
        bytes.push(0); // reserved flags, must remain zero in version 1
        bytes.extend_from_slice(&self.uncompressed_len.to_le_bytes());
        bytes.extend_from_slice(&self.file_digest);
        bytes.extend_from_slice(&(self.chunks.len() as u32).to_le_bytes());
        for chunk in &self.chunks {
            bytes.extend_from_slice(&chunk.digest);
            bytes.extend_from_slice(&chunk.length.to_le_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EncodingError> {
        if bytes.len() < RECIPE_HEADER_LEN || bytes.len() > MAX_RECIPE_BYTES {
            return Err(EncodingError::InvalidRecipe("invalid recipe length".into()));
        }
        if &bytes[..RECIPE_MAGIC.len()] != RECIPE_MAGIC {
            return Err(EncodingError::InvalidRecipe("recipe magic mismatch".into()));
        }
        let mut cursor = RECIPE_MAGIC.len();
        let version = read_u16(bytes, &mut cursor)?;
        let profile_id = read_u16(bytes, &mut cursor)?;
        let codec = Codec::from_wire(read_u8(bytes, &mut cursor)?)?;
        if read_u8(bytes, &mut cursor)? != 0 {
            return Err(EncodingError::InvalidRecipe(
                "reserved recipe flags set".into(),
            ));
        }
        let uncompressed_len = read_u64(bytes, &mut cursor)?;
        if uncompressed_len > MAX_SOURCE_BYTES {
            return Err(EncodingError::InvalidRecipe(
                "recipe file exceeds decode limit".into(),
            ));
        }
        let file_digest = read_array(bytes, &mut cursor)?;
        let count = read_u32(bytes, &mut cursor)? as usize;
        if version != RECIPE_VERSION || count > MAX_RECIPE_CHUNKS {
            return Err(EncodingError::InvalidRecipe(
                "unsupported recipe version or chunk count".into(),
            ));
        }
        let expected = RECIPE_HEADER_LEN
            .checked_add(
                count
                    .checked_mul(CHUNK_REF_LEN)
                    .ok_or_else(|| EncodingError::InvalidRecipe("recipe size overflow".into()))?,
            )
            .ok_or_else(|| EncodingError::InvalidRecipe("recipe size overflow".into()))?;
        if expected != bytes.len() {
            return Err(EncodingError::InvalidRecipe(
                "recipe has trailing or missing bytes".into(),
            ));
        }
        let mut chunks = Vec::with_capacity(count);
        let mut total = 0u64;
        for _ in 0..count {
            let digest = read_array(bytes, &mut cursor)?;
            let length = read_u32(bytes, &mut cursor)?;
            total = total
                .checked_add(length as u64)
                .ok_or_else(|| EncodingError::InvalidRecipe("recipe length overflow".into()))?;
            chunks.push(ChunkRef { digest, length });
        }
        if total != uncompressed_len || (count == 0 && uncompressed_len != 0) {
            return Err(EncodingError::InvalidRecipe(
                "recipe lengths do not match file length".into(),
            ));
        }
        if codec == Codec::WholeZstd && count != 1 {
            return Err(EncodingError::InvalidRecipe(
                "whole-file recipe must contain one object".into(),
            ));
        }
        Ok(Self {
            version,
            profile_id,
            codec,
            uncompressed_len,
            file_digest,
            chunks,
        })
    }
}

const RECIPE_MAGIC: &[u8; 8] = b"LPREC001";
const RECIPE_HEADER_LEN: usize = 8 + 2 + 2 + 1 + 1 + 8 + 32 + 4;
const CHUNK_REF_LEN: usize = 32 + 4;

/// A compressed object staged for publication. The key is derived from its
/// digest and scoped by the document storage identity by the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedObject {
    /// Digest of the uncompressed object. This permits a catalogue/object
    /// lookup before compression and lets the reader verify the decoded bytes.
    pub digest: [u8; 32],
    pub encoded: Vec<u8>,
    pub uncompressed_len: u32,
}

/// The complete output of one bounded encoding job. Objects are deduplicated
/// within this result; the catalogue decides which already-published objects
/// can be reused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedSource {
    pub file_digest: [u8; 32],
    pub uncompressed_len: u64,
    pub recipe: Recipe,
    pub recipe_bytes: Vec<u8>,
    pub objects: Vec<EncodedObject>,
}

/// The digest/cut pass of encoding, separated from compression so a caller
/// can query the catalogue for already durable chunks first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodingPlan {
    pub file_digest: [u8; 32],
    pub uncompressed_len: u64,
    pub recipe: Recipe,
}

impl EncodedSource {
    pub fn codec(&self) -> Codec {
        self.recipe.codec
    }

    pub fn recipe_digest(&self) -> [u8; 32] {
        self.recipe.digest()
    }

    /// Stored bytes introduced by this encoding before catalogue metadata.
    pub fn encoded_bytes(&self) -> usize {
        self.recipe_bytes.len()
            + self
                .objects
                .iter()
                .map(|object| object.encoded.len())
                .sum::<usize>()
    }
}

/// Errors deliberately distinguish overload from corrupt durable data so a
/// caller can defer history work without acknowledging a false checkpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EncodingError {
    InvalidProfile,
    InvalidInput(String),
    InvalidRecipe(String),
    UnsupportedCodec(u8),
    Integrity(String),
    Overloaded,
    Worker(String),
}

impl fmt::Display for EncodingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProfile => f.write_str("invalid source encoding profile"),
            Self::InvalidInput(message) => write!(f, "invalid source input: {message}"),
            Self::InvalidRecipe(message) => write!(f, "invalid source recipe: {message}"),
            Self::UnsupportedCodec(codec) => write!(f, "unsupported source codec {codec}"),
            Self::Integrity(message) => write!(f, "source encoding integrity failure: {message}"),
            Self::Overloaded => f.write_str("source encoding capacity is temporarily full"),
            Self::Worker(message) => write!(f, "source encoding worker failed: {message}"),
        }
    }
}

impl std::error::Error for EncodingError {}

/// Encode one complete file. This function performs no object-store I/O and
/// can therefore run in a bounded native worker outside room locks.
#[cfg(test)]
pub fn encode_source(source: &[u8]) -> Result<EncodedSource, EncodingError> {
    encode_source_with_profile(source, EncodingProfile::default())
}

#[cfg(test)]
pub fn encode_source_with_profile(
    source: &[u8],
    profile: EncodingProfile,
) -> Result<EncodedSource, EncodingError> {
    let plan = plan_source_with_profile(source, profile)?;
    encode_source_from_plan(source, plan, &HashSet::new())
}

pub fn plan_source_with_profile(
    source: &[u8],
    profile: EncodingProfile,
) -> Result<EncodingPlan, EncodingError> {
    profile.validate()?;
    if source.len() as u64 > MAX_SOURCE_BYTES {
        return Err(EncodingError::InvalidInput("source is too large".into()));
    }
    let file_digest: [u8; 32] = Sha256::digest(source).into();
    let codec = if source.len() <= profile.small_file_threshold {
        Codec::WholeZstd
    } else {
        Codec::ChunkedZstd
    };
    let chunks = match codec {
        Codec::WholeZstd => vec![ChunkRef {
            digest: file_digest,
            length: u32::try_from(source.len())
                .map_err(|_| EncodingError::InvalidInput("file is too large".into()))?,
        }],
        Codec::ChunkedZstd => {
            let mut refs = Vec::new();
            for chunk in FastCDC::with_level_and_seed(
                source,
                profile.min_size,
                profile.average_size,
                profile.max_size,
                Normalization::Level1,
                0,
            ) {
                let raw = &source[chunk.offset..chunk.offset + chunk.length];
                refs.push(ChunkRef {
                    digest: Sha256::digest(raw).into(),
                    length: u32::try_from(raw.len())
                        .map_err(|_| EncodingError::InvalidInput("chunk is too large".into()))?,
                });
                if refs.len() > MAX_RECIPE_CHUNKS {
                    return Err(EncodingError::InvalidInput("too many source chunks".into()));
                }
            }
            refs
        }
    };
    Ok(EncodingPlan {
        file_digest,
        uncompressed_len: source.len() as u64,
        recipe: Recipe {
            version: RECIPE_VERSION,
            profile_id: profile.id,
            codec,
            uncompressed_len: source.len() as u64,
            file_digest,
            chunks,
        },
    })
}

pub fn encode_source_from_plan(
    source: &[u8],
    plan: EncodingPlan,
    existing: &HashSet<[u8; 32]>,
) -> Result<EncodedSource, EncodingError> {
    let file_digest: [u8; 32] = Sha256::digest(source).into();
    if source.len() as u64 != plan.uncompressed_len || file_digest != plan.file_digest {
        return Err(EncodingError::Integrity(
            "source changed while encoding plan was staged".into(),
        ));
    }
    let mut objects = Vec::new();
    let mut offset = 0usize;
    let mut emitted = HashSet::new();
    for reference in &plan.recipe.chunks {
        let end = offset
            .checked_add(reference.length as usize)
            .ok_or_else(|| EncodingError::InvalidInput("source length overflow".into()))?;
        let raw = source
            .get(offset..end)
            .ok_or_else(|| EncodingError::Integrity("encoding plan exceeds source".into()))?;
        if !existing.contains(&reference.digest) && emitted.insert(reference.digest) {
            objects.push(EncodedObject {
                digest: reference.digest,
                encoded: compress(raw)?,
                uncompressed_len: reference.length,
            });
        }
        offset = end;
    }
    if offset != source.len() {
        return Err(EncodingError::Integrity(
            "encoding plan does not cover source".into(),
        ));
    }
    let recipe_bytes = plan.recipe.to_bytes();
    if recipe_bytes.len() > MAX_RECIPE_BYTES {
        return Err(EncodingError::InvalidInput(
            "source recipe is too large".into(),
        ));
    }
    Ok(EncodedSource {
        file_digest: plan.file_digest,
        uncompressed_len: plan.uncompressed_len,
        recipe: plan.recipe,
        recipe_bytes,
        objects,
    })
}

/// Variant used by a publisher that has already batched object-existence
/// lookups. The callback receives an uncompressed object digest; returning
/// `true` omits that object from the staged output while keeping its reference
/// in the complete recipe.
#[cfg(test)]
pub fn encode_source_with_lookup<F>(
    source: &[u8],
    profile: EncodingProfile,
    mut object_exists: F,
) -> Result<EncodedSource, EncodingError>
where
    F: FnMut(&[u8; 32]) -> Result<bool, EncodingError>,
{
    let plan = plan_source_with_profile(source, profile)?;
    let mut existing = HashSet::new();
    for chunk in &plan.recipe.chunks {
        if object_exists(&chunk.digest)? {
            existing.insert(chunk.digest);
        }
    }
    encode_source_from_plan(source, plan, &existing)
}

fn compress(source: &[u8]) -> Result<Vec<u8>, EncodingError> {
    zstd::stream::encode_all(source, ZSTD_LEVEL)
        .map_err(|error| EncodingError::Worker(format!("zstd compression failed: {error}")))
}

/// Reconstruct and verify a file from a decoded recipe. The lookup callback
/// must return the compressed bytes for the requested object digest.
pub fn reconstruct<F>(recipe: &Recipe, mut lookup: F) -> Result<Vec<u8>, EncodingError>
where
    F: FnMut(&[u8; 32]) -> Result<Vec<u8>, EncodingError>,
{
    if recipe.version != RECIPE_VERSION {
        return Err(EncodingError::InvalidRecipe(
            "unsupported recipe version".into(),
        ));
    }
    if recipe.uncompressed_len > MAX_SOURCE_BYTES {
        return Err(EncodingError::InvalidRecipe(
            "recipe file exceeds decode limit".into(),
        ));
    }
    let mut output = Vec::with_capacity(
        usize::try_from(recipe.uncompressed_len)
            .map_err(|_| EncodingError::InvalidRecipe("file is too large".into()))?,
    );
    let mut seen = HashMap::<[u8; 32], Vec<u8>>::new();
    for reference in &recipe.chunks {
        let encoded = if let Some(encoded) = seen.get(&reference.digest) {
            encoded.clone()
        } else {
            let encoded = lookup(&reference.digest)?;
            seen.insert(reference.digest, encoded.clone());
            encoded
        };
        if reference.length as u64 > MAX_OBJECT_BYTES {
            return Err(EncodingError::Integrity(
                "object exceeds decode limit".into(),
            ));
        }
        let decoder = zstd::stream::read::Decoder::new(encoded.as_slice()).map_err(|error| {
            EncodingError::Integrity(format!("zstd decompression failed: {error}"))
        })?;
        let mut limited = decoder.take(reference.length as u64 + 1);
        let mut decoded = Vec::with_capacity(reference.length as usize);
        limited.read_to_end(&mut decoded).map_err(|error| {
            EncodingError::Integrity(format!("zstd decompression failed: {error}"))
        })?;
        if decoded.len() != reference.length as usize {
            return Err(EncodingError::Integrity(
                "decoded object length does not match recipe".into(),
            ));
        }
        let actual: [u8; 32] = Sha256::digest(&decoded).into();
        if actual != reference.digest {
            return Err(EncodingError::Integrity(format!(
                "decoded object digest {} does not match recipe",
                hex::encode(reference.digest)
            )));
        }
        output.extend_from_slice(&decoded);
    }
    if output.len() as u64 != recipe.uncompressed_len {
        return Err(EncodingError::Integrity(
            "reconstructed file length does not match recipe".into(),
        ));
    }
    let actual: [u8; 32] = Sha256::digest(&output).into();
    if actual != recipe.file_digest {
        return Err(EncodingError::Integrity(
            "reconstructed file digest does not match recipe".into(),
        ));
    }
    Ok(output)
}

/// Read the recipe/chunk representation. Missing or corrupt recipes and chunks
/// are errors and are never substituted with unrelated bytes.
pub async fn read_file(
    blobs: &dyn crate::storage::blob::BlobStore,
    storage_id: &str,
    file_digest: &str,
) -> Result<Vec<u8>, EncodingError> {
    // Admit before object I/O, not after materializing every chunk. Otherwise
    // queued readers retain unbounded compressed inputs while awaiting CPU.
    let permit = reconstruction_pool()
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| EncodingError::Worker("reconstruction pool is closed".into()))?;
    let expected = decode_digest(file_digest)?;
    let recipe_key = crate::storage::blob::content_recipe_key(storage_id, file_digest);
    let recipe_bytes = blobs
        .get(&recipe_key)
        .await
        .map_err(|error| EncodingError::Integrity(error.to_string()))?;
    let recipe = Recipe::from_bytes(&recipe_bytes)?;
    if recipe.file_digest != expected {
        return Err(EncodingError::Integrity(
            "recipe key and complete-file digest disagree".into(),
        ));
    }
    let mut objects = HashMap::with_capacity(recipe.chunks.len());
    let mut encoded_bytes = 0u64;
    let encoded_limit = recipe
        .uncompressed_len
        .saturating_add((recipe.chunks.len() as u64).saturating_mul(256));
    for reference in &recipe.chunks {
        if objects.contains_key(&reference.digest) {
            continue;
        }
        let digest = hex::encode(reference.digest);
        let bytes = blobs
            .get(&crate::storage::blob::content_chunk_key(
                storage_id, &digest,
            ))
            .await
            .map_err(|error| EncodingError::Integrity(error.to_string()))?;
        encoded_bytes = encoded_bytes.saturating_add(bytes.len() as u64);
        if bytes.len() as u64 > MAX_OBJECT_BYTES || encoded_bytes > encoded_limit {
            return Err(EncodingError::Integrity(
                "encoded source exceeds read budget".into(),
            ));
        }
        objects.insert(reference.digest, bytes);
    }
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        reconstruct(&recipe, |digest| {
            objects
                .get(digest)
                .cloned()
                .ok_or_else(|| EncodingError::Integrity("recipe object is missing".into()))
        })
    })
    .await
    .map_err(|error| EncodingError::Worker(error.to_string()))?
}

fn decode_digest(value: &str) -> Result<[u8; 32], EncodingError> {
    let bytes = hex::decode(value)
        .map_err(|_| EncodingError::InvalidInput("source digest is not hexadecimal".into()))?;
    bytes
        .try_into()
        .map_err(|_| EncodingError::InvalidInput("source digest must be SHA-256".into()))
}

/// Explicitly bounded native encoding admission. `try_encode` returns
/// `Overloaded` when either worker slots or queued input bytes are exhausted;
/// callers should report a pending/retry checkpoint state and preserve live
/// session durability.
pub struct EncodingPool {
    workers: Arc<Semaphore>,
    input_bytes: Arc<Semaphore>,
    profile: EncodingProfile,
    max_input_bytes: usize,
}

impl fmt::Debug for EncodingPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncodingPool")
            .field("profile", &self.profile)
            .field("max_input_bytes", &self.max_input_bytes)
            .finish_non_exhaustive()
    }
}

impl EncodingPool {
    pub fn new(
        workers: usize,
        max_input_bytes: usize,
        profile: EncodingProfile,
    ) -> Result<Self, EncodingError> {
        if workers == 0 || max_input_bytes == 0 || max_input_bytes > u32::MAX as usize {
            return Err(EncodingError::InvalidInput(
                "invalid encoding pool limits".into(),
            ));
        }
        profile.validate()?;
        Ok(Self {
            workers: Arc::new(Semaphore::new(workers)),
            input_bytes: Arc::new(Semaphore::new(max_input_bytes)),
            profile,
            max_input_bytes,
        })
    }

    /// Run only the bounded digest/cut pass.  Publishers use the resulting
    /// digests to query durable object accounting before paying compression
    /// CPU for chunks that are already present.
    pub async fn try_plan(&self, source: Vec<u8>) -> Result<EncodingPlan, EncodingError> {
        if source.len() > self.max_input_bytes {
            return Err(EncodingError::Overloaded);
        }
        let input = self
            .input_bytes
            .clone()
            .try_acquire_many_owned(source.len() as u32)
            .map_err(|_| EncodingError::Overloaded)?;
        let worker = self
            .workers
            .clone()
            .try_acquire_owned()
            .map_err(|_| EncodingError::Overloaded)?;
        let profile = self.profile;
        tokio::task::spawn_blocking(move || {
            let _input = input;
            let _worker = worker;
            plan_source_with_profile(&source, profile)
        })
        .await
        .map_err(|error| EncodingError::Worker(error.to_string()))?
    }

    /// Compress the missing objects from a previously admitted plan. The
    /// catalogue lookup happens between [`try_plan`] and this method, so
    /// existing chunks never enter zstd merely to be discarded.
    pub async fn try_encode_planned(
        &self,
        source: Vec<u8>,
        plan: EncodingPlan,
        existing: HashSet<[u8; 32]>,
    ) -> Result<EncodedSource, EncodingError> {
        if source.len() > self.max_input_bytes {
            return Err(EncodingError::Overloaded);
        }
        let input = self
            .input_bytes
            .clone()
            .try_acquire_many_owned(source.len() as u32)
            .map_err(|_| EncodingError::Overloaded)?;
        let worker = self
            .workers
            .clone()
            .try_acquire_owned()
            .map_err(|_| EncodingError::Overloaded)?;
        tokio::task::spawn_blocking(move || {
            let _input = input;
            let _worker = worker;
            encode_source_from_plan(&source, plan, &existing)
        })
        .await
        .map_err(|error| EncodingError::Worker(error.to_string()))?
    }
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, EncodingError> {
    let byte = *bytes
        .get(*cursor)
        .ok_or_else(|| EncodingError::InvalidRecipe("truncated recipe".into()))?;
    *cursor += 1;
    Ok(byte)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, EncodingError> {
    let end = cursor
        .checked_add(2)
        .ok_or_else(|| EncodingError::InvalidRecipe("truncated recipe".into()))?;
    let value = u16::from_le_bytes(
        bytes
            .get(*cursor..end)
            .ok_or_else(|| EncodingError::InvalidRecipe("truncated recipe".into()))?
            .try_into()
            .map_err(|_| EncodingError::InvalidRecipe("truncated recipe".into()))?,
    );
    *cursor = end;
    Ok(value)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, EncodingError> {
    let end = cursor
        .checked_add(4)
        .ok_or_else(|| EncodingError::InvalidRecipe("truncated recipe".into()))?;
    let value = u32::from_le_bytes(
        bytes
            .get(*cursor..end)
            .ok_or_else(|| EncodingError::InvalidRecipe("truncated recipe".into()))?
            .try_into()
            .map_err(|_| EncodingError::InvalidRecipe("truncated recipe".into()))?,
    );
    *cursor = end;
    Ok(value)
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, EncodingError> {
    let end = cursor
        .checked_add(8)
        .ok_or_else(|| EncodingError::InvalidRecipe("truncated recipe".into()))?;
    let value = u64::from_le_bytes(
        bytes
            .get(*cursor..end)
            .ok_or_else(|| EncodingError::InvalidRecipe("truncated recipe".into()))?
            .try_into()
            .map_err(|_| EncodingError::InvalidRecipe("truncated recipe".into()))?,
    );
    *cursor = end;
    Ok(value)
}

fn read_array(bytes: &[u8], cursor: &mut usize) -> Result<[u8; 32], EncodingError> {
    let end = cursor
        .checked_add(32)
        .ok_or_else(|| EncodingError::InvalidRecipe("truncated recipe".into()))?;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| EncodingError::InvalidRecipe("truncated recipe".into()))?
        .try_into()
        .map_err(|_| EncodingError::InvalidRecipe("truncated recipe".into()))?;
    *cursor = end;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn restore(encoded: &EncodedSource) -> Vec<u8> {
        let objects: HashMap<_, _> = encoded
            .objects
            .iter()
            .map(|object| (object.digest, object.encoded.clone()))
            .collect();
        reconstruct(&encoded.recipe, |digest| {
            objects
                .get(digest)
                .cloned()
                .ok_or_else(|| EncodingError::Integrity("test object missing".into()))
        })
        .expect("encoded source reconstructs")
    }

    #[test]
    fn small_source_uses_whole_zstd_and_round_trips() {
        let source = b"a short source";
        let encoded = encode_source(source).expect("encode");
        assert_eq!(encoded.codec(), Codec::WholeZstd);
        assert_eq!(restore(&encoded), source);
        assert_eq!(
            Recipe::from_bytes(&encoded.recipe_bytes).unwrap(),
            encoded.recipe
        );
    }

    #[test]
    fn large_source_has_content_defined_chunks_and_round_trips() {
        let mut source = Vec::new();
        for _ in 0..2048 {
            source.extend_from_slice(b"A paragraph with stable source boundaries.\n");
        }
        let encoded = encode_source(&source).expect("encode");
        assert_eq!(encoded.codec(), Codec::ChunkedZstd);
        assert!(encoded.recipe.chunks.len() > 1);
        assert_eq!(restore(&encoded), source);
    }

    #[test]
    fn lookup_omits_existing_objects_without_changing_recipe() {
        let source = vec![b'x'; 8192];
        let full = encode_source(&source).expect("encode");
        let reused = encode_source_with_lookup(&source, EncodingProfile::default(), |_| Ok(true))
            .expect("encode with lookup");
        assert!(reused.objects.is_empty());
        assert_eq!(reused.recipe, full.recipe);
    }

    #[test]
    fn tampering_with_recipe_or_object_is_rejected() {
        let encoded = encode_source(b"verify me").expect("encode");
        let mut recipe = encoded.recipe_bytes.clone();
        recipe[0] ^= 1;
        assert!(Recipe::from_bytes(&recipe).is_err());
        let mut objects: HashMap<_, _> = encoded
            .objects
            .iter()
            .map(|object| (object.digest, object.encoded.clone()))
            .collect();
        objects.values_mut().next().unwrap().push(1);
        assert!(reconstruct(&encoded.recipe, |digest| {
            objects
                .get(digest)
                .cloned()
                .ok_or_else(|| EncodingError::Integrity("missing".into()))
        })
        .is_err());
    }

    /// A deliberately ignored, in-tree measurement harness for choosing the
    /// first writer profile. It models one document-local object namespace,
    /// so a chunk or whole-file object emitted by one file/revision can be
    /// reused by every later file/revision in that history. The test prints
    /// source bytes, references reused, unique physical objects, serialized
    /// recipe bytes, encoded physical bytes, and encode/reconstruction time;
    /// every encoded checkpoint is reconstructed before the next one begins.
    ///
    /// Run explicitly with `cargo test measure_source_history_profiles
    /// -- --ignored --nocapture` when a local measurement is wanted. The
    /// timing is diagnostic only and is intentionally not an assertion.
    #[test]
    #[ignore]
    fn measure_source_history_profiles() {
        let cases = measurement_cases();
        let profiles = [
            (
                "whole-zstd",
                EncodingProfile {
                    id: 10,
                    min_size: 1024,
                    average_size: 4096,
                    max_size: 32768,
                    small_file_threshold: usize::MAX,
                },
            ),
            (
                "fastcdc-1k",
                EncodingProfile {
                    id: 11,
                    min_size: 256,
                    average_size: 1024,
                    max_size: 8192,
                    small_file_threshold: 4096,
                },
            ),
            ("fastcdc-4k", EncodingProfile::default()),
        ];

        println!("native source-history profile measurement");
        println!(
            "profile | history | checkpoints | input | unique objects | reused refs | recipe | physical | encode | reconstruct"
        );
        for (profile_name, profile) in profiles {
            for case in &cases {
                let result = measure_history(case, profile);
                println!(
                    "{profile_name:>11} | {name:<20} | {:>11} | {:>8} B | {:>14} | {:>11} | {:>8} B | {:>8} B | {:>8.3} ms | {:>12.3} ms",
                    result.checkpoints,
                    result.input_bytes,
                    result.unique_objects,
                    result.reused_references,
                    result.recipe_bytes,
                    result.physical_bytes,
                    result.encode_nanos as f64 / 1_000_000.0,
                    result.reconstruct_nanos as f64 / 1_000_000.0,
                    name = case.name,
                );
            }
        }
    }

    #[derive(Debug)]
    struct MeasurementCase {
        name: &'static str,
        /// Each outer item is one checkpoint; each inner item is one text file
        /// in that checkpoint's directory tree.
        revisions: Vec<Vec<Vec<u8>>>,
    }

    #[derive(Default)]
    struct Measurement {
        checkpoints: usize,
        input_bytes: usize,
        unique_objects: usize,
        reused_references: usize,
        recipe_bytes: usize,
        physical_bytes: usize,
        encode_nanos: u128,
        reconstruct_nanos: u128,
    }

    fn measure_history(case: &MeasurementCase, profile: EncodingProfile) -> Measurement {
        let mut result = Measurement {
            checkpoints: case.revisions.len(),
            ..Measurement::default()
        };
        let mut known_objects = HashSet::<[u8; 32]>::new();
        let mut encoded_objects = HashMap::<[u8; 32], Vec<u8>>::new();
        let mut known_recipes = HashSet::<[u8; 32]>::new();

        for files in &case.revisions {
            let encode_started = Instant::now();
            let mut checkpoint = Vec::with_capacity(files.len());
            for source in files {
                result.input_bytes += source.len();
                let existing = known_objects.clone();
                let plan = plan_source_with_profile(source, profile).expect("measurement plan");
                let encoded =
                    encode_source_from_plan(source, plan, &existing).expect("measurement encode");

                let mut new_references = 0usize;
                for reference in &encoded.recipe.chunks {
                    if known_objects.insert(reference.digest) {
                        new_references += 1;
                    } else {
                        result.reused_references += 1;
                    }
                }
                assert_eq!(
                    encoded.objects.len(),
                    new_references,
                    "the emitted object set must match newly referenced chunks"
                );
                for object in &encoded.objects {
                    encoded_objects.insert(object.digest, object.encoded.clone());
                    result.physical_bytes += object.encoded.len();
                }
                if known_recipes.insert(encoded.file_digest) {
                    result.recipe_bytes += encoded.recipe_bytes.len();
                    result.physical_bytes += encoded.recipe_bytes.len();
                }
                checkpoint.push(encoded);
            }
            result.encode_nanos += encode_started.elapsed().as_nanos();

            let reconstruct_started = Instant::now();
            for (source, encoded) in files.iter().zip(checkpoint.iter()) {
                let restored = reconstruct(&encoded.recipe, |digest| {
                    encoded_objects.get(digest).cloned().ok_or_else(|| {
                        EncodingError::Integrity("measurement object missing".into())
                    })
                })
                .expect("measurement reconstruction");
                assert_eq!(
                    restored, *source,
                    "every checkpoint must reconstruct exactly"
                );
            }
            result.reconstruct_nanos += reconstruct_started.elapsed().as_nanos();
        }
        result.unique_objects = encoded_objects.len();
        result
    }

    fn measurement_cases() -> Vec<MeasurementCase> {
        vec![
            MeasurementCase {
                name: "body changes",
                revisions: body_change_history(),
            },
            MeasurementCase {
                name: "many tiny files",
                revisions: many_tiny_file_history(),
            },
            MeasurementCase {
                name: "punctuation/structural",
                revisions: punctuation_history(),
            },
            MeasurementCase {
                name: "large boilerplate",
                revisions: large_boilerplate_history(),
            },
        ]
    }

    fn body_change_history() -> Vec<Vec<Vec<u8>>> {
        (0..12)
            .map(|revision| {
                let mut body = Vec::new();
                for line in 0..320 {
                    if line == (revision * 23) % 320 {
                        body.extend_from_slice(
                            format!("A localized revision marker {revision:02} appears here.\n")
                                .as_bytes(),
                        );
                    } else {
                        body.extend_from_slice(
                            format!("Stable paragraph {line:03}: the body remains reusable.\n")
                                .as_bytes(),
                        );
                    }
                }
                vec![body]
            })
            .collect()
    }

    fn many_tiny_file_history() -> Vec<Vec<Vec<u8>>> {
        (0..10)
            .map(|revision| {
                (0..48)
                    .map(|file| {
                        let edit = if file == (revision * 7) % 48 {
                            format!("revision {revision:02} changed this tiny file")
                        } else {
                            "unchanged tiny file contents".to_string()
                        };
                        format!("file-{file:02}\n{edit}\nline two\n").into_bytes()
                    })
                    .collect()
            })
            .collect()
    }

    fn punctuation_history() -> Vec<Vec<Vec<u8>>> {
        (0..10)
            .map(|revision| {
                let mut body = String::from("# Structured source\n\n");
                for section in 0..180 {
                    let punctuation = match (revision + section) % 4 {
                        0 => "{}",
                        1 => "[]",
                        2 => "()",
                        _ => "<>/",
                    };
                    body.push_str(&format!(
                        "## Section {section:03} {punctuation}\n- item {section:03}: **stable** _text_\n\n"
                    ));
                }
                vec![body.into_bytes()]
            })
            .collect()
    }

    fn large_boilerplate_history() -> Vec<Vec<Vec<u8>>> {
        (0..9)
            .map(|revision| {
                let mut body = Vec::new();
                for line in 0..2400 {
                    if line % 257 == revision % 257 {
                        body.extend_from_slice(
                            format!(
                                "boilerplate exception {revision:02} at generated line {line:04}\n"
                            )
                            .as_bytes(),
                        );
                    } else {
                        body.extend_from_slice(
                            format!(
                                "The same long boilerplate paragraph repeats at line {line:04}; "
                            )
                            .as_bytes(),
                        );
                        body.extend_from_slice(b"it is intentionally stable across revisions.\n");
                    }
                }
                vec![body]
            })
            .collect()
    }
}

//! Every rule the deployment enforces lives here, and only here. The shell
//! gets these values injected into its source in place of `__CONFIG__`; the
//! server reads them directly. A limit changed here changes everywhere on the
//! next build.

use serde::Serialize;
use std::path::PathBuf;

/// The public static compiler distribution used when an operator does not
/// host a mirror copy. Browsers fetch it directly; the origin never proxies
/// these bytes.
pub const DEFAULT_LATEX_MIRROR: &str = "https://latex.librepaper.workers.dev/";

/// Private disposable state used by the server process, and the fixed
/// filesystem layout of a local deployment directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentPaths {
    pub deployment: PathBuf,
    pub objects: PathBuf,
    pub state: PathBuf,
    pub secrets: PathBuf,
}

impl DeploymentPaths {
    pub fn local(deployment: impl Into<PathBuf>) -> Self {
        let deployment = deployment.into();
        let state = deployment.join("state");
        Self {
            objects: deployment.join("objects"),
            secrets: deployment.join("secrets"),
            state,
            deployment,
        }
    }

    /// Validate the private-state boundary before any driver opens a file.
    pub fn validate(&self) -> Result<(), String> {
        if !self.state.is_absolute() {
            return Err(format!(
                "server state path must be absolute: {}",
                self.state.display()
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Configuration {
    /// How many files one document may hold, across its texts and its assets.
    /// A paper has a dozen; a directory of two hundred is somebody using a
    /// document as a filesystem.
    pub max_files: usize,
    /// The longest one path may be, in bytes.
    pub max_path: usize,
    /// What one document's log may weigh: its compaction base plus every row
    /// since. Past this, new updates are refused with a retryable reason and
    /// semantic commands still work against the log as it stands
    /// (SPEC-server-is-a-log §9.1). It is the real bound on how long a build
    /// can occupy a thread, which is why it is a per-document ceiling and not
    /// a deployment-wide one. The default is what a 512 MiB memory budget can
    /// build, because a cold build reserves the resident expansion plus the
    /// transient one, fourteen times the log at the measured factors, and
    /// raising the quota means raising the budget with it.
    pub log_quota_bytes: usize,
    /// One process-wide budget for everything holding a decoded document:
    /// resident cache entries, in-flight builds, temporary forks and
    /// projection output (§9.2).
    pub memory_budget_bytes: u64,
    /// How much larger a decoded document is than the bytes it was loaded
    /// from. A measurement rather than a constant of nature, so a deployment
    /// can correct it without a build.
    pub cache_expansion: u64,
    /// One deployment-wide ceiling on unsaved source bytes: everything every
    /// document's buffer is holding while it waits for PostgreSQL, framing
    /// included (SPEC-frugal §2). Separate from `memory_budget_bytes` because
    /// pending updates are somebody's typing and cannot be evicted the way a
    /// decoded document can.
    pub pending_bytes: u64,
    /// The other half of that bound: what writing those bytes costs while the
    /// write is in flight -- the encoded row and both driver buffers, including capacity growth.
    /// Its own pool, so a deployment whose buffers are full can still drain them.
    /// The default derives from the log quota via `default_pending_scratch_bytes`.
    pub pending_scratch_bytes: u64,
    /// Which extensions name a file a person edits, which name bytes nobody
    /// edits in place, and which name what a compiler wrote -- and a document
    /// keeps what a person wrote. Rules rather than constants, so a deployment
    /// can widen or narrow them without a build.
    pub text_extensions: Vec<String>,
    pub asset_extensions: Vec<String>,
    pub derived_extensions: Vec<String>,
    pub caps: CapLimit,

    /// Storage is what keeps a deployment's bill bounded no matter who shows
    /// up: a ceiling on everything stored, a ceiling per publisher, and a cap
    /// on how many uploads an hour one publisher gets. Sizes are bytes; retained
    /// storage counts label archives, figures, and each document's editing log
    /// (compaction base plus rows).
    pub storage: StorageLimit,

    /// The deployment-wide cost envelope: implementation guardrails with
    /// documented defaults.
    #[serde(skip)]
    pub cost: CostPolicy,
    #[serde(skip)]
    pub sockets: crate::server::socket_budget::SocketPolicy,
    /// Optional reporting metadata for operator-managed backups.
    pub backup: BackupPolicy,

    /// The only file types the reader can frame and anchor comments into. The
    /// upload page checks them before sending, and the server checks them
    /// again.
    pub extensions: Vec<String>,

    /// The markups a document may be kept as, beside the HTML it was rendered
    /// to, so it can be reopened and edited. A source in anything else is
    /// dropped rather than refused: the document is fine, it simply cannot be
    /// reopened.
    pub source_formats: Vec<String>,

    /// The W3C Web Annotation motivations an annotation may carry. Using the
    /// standard vocabulary rather than an invented one means an exported
    /// annotation says the same thing to any tool that reads the spec.
    ///
    /// Three of them: a remark about a passage, a passage marked as worth
    /// returning to, and a suggestion -- a proposed rewording, inert until an
    /// editor decides it. The shades of saying something -- a question, a
    /// judgement -- are otherwise the comment's own words, not a taxonomy to
    /// pick from before writing one.
    pub motivations: Vec<String>,
    pub default_motivation: String,

    /// Caps a document title, in characters. Titles live in the index, which is
    /// read on nearly every request, so an unbounded title is a way to sink the
    /// whole deployment.
    pub max_title: usize,
    /// The live document, and what it costs to keep. Every one of these is a
    /// bound on the bill or on the memory of a server nobody is watching.
    pub session: SessionLimit,

    /// The shape of a valid slug, as a RegExp source string.
    pub slug_pattern: String,
    pub slug_max: usize,

    /// Documents are unlisted, so the URL is the only way in and the slug has
    /// to be unguessable. 10 characters from a 32-symbol alphabet is 50 bits,
    /// drawn from a CSPRNG; look-alike characters are left out so a link
    /// survives being read aloud or retyped.
    pub suffix_alphabet: String,
    pub suffix_length: usize,
}

/// Bounds what a deployment will hold. `total` and `per_owner` are bytes;
/// `uploads_per_hour` is a count.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct StorageLimit {
    pub total: i64,
    pub per_owner: i64,
    pub uploads_per_hour: usize,
}

/// Optional operator-declared backup policy.  LibrePaper currently creates
/// complete local copies; these values describe the operator's intended
/// destination and retention so status can report it without managing the
/// destination.  `None` means the operator made no declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupPolicy {
    pub destination_class: Option<String>,
    /// Declared backup frequency in seconds.
    pub frequency: Option<u64>,
    pub retained_count: Option<usize>,
    pub encrypted: Option<bool>,
    /// Warn after this many completed backups for the deployment.
    pub warning_count: usize,
}

impl Default for BackupPolicy {
    fn default() -> Self {
        Self {
            destination_class: None,
            frequency: None,
            retained_count: None,
            encrypted: None,
            warning_count: 10,
        }
    }
}

/// Optional values accepted from the advanced YAML file.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackupPolicyOverrides {
    pub destination_class: Option<String>,
    pub frequency: Option<u64>,
    pub retained_count: Option<usize>,
    pub encrypted: Option<bool>,
    pub warning_count: Option<usize>,
}

const MAX_BACKUP_POLICY_STRING: usize = 64;
const MAX_BACKUP_POLICY_FREQUENCY_SECONDS: u64 = 10 * 365 * 24 * 60 * 60;
const MAX_BACKUP_POLICY_COUNT: usize = 1_000_000;

impl BackupPolicy {
    pub fn apply(&mut self, overrides: BackupPolicyOverrides) -> Result<(), String> {
        if let Some(value) = overrides.destination_class {
            let value = value.trim().to_owned();
            if value.is_empty()
                || value.len() > MAX_BACKUP_POLICY_STRING
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._- /".contains(&byte))
            {
                return Err(
                    "backup.destination_class must be 1-64 ASCII letters, digits, spaces, or ._- /"
                        .into(),
                );
            }
            self.destination_class = Some(value);
        }
        if let Some(value) = overrides.frequency {
            if !(1..=MAX_BACKUP_POLICY_FREQUENCY_SECONDS).contains(&value) {
                return Err("backup.frequency must be between 1 second and 10 years".into());
            }
            self.frequency = Some(value);
        }
        if let Some(value) = overrides.retained_count {
            if value == 0 || value > MAX_BACKUP_POLICY_COUNT {
                return Err("backup.retained_count must be between 1 and 1000000".into());
            }
            self.retained_count = Some(value);
        }
        if let Some(value) = overrides.encrypted {
            self.encrypted = Some(value);
        }
        if let Some(value) = overrides.warning_count {
            if value > MAX_BACKUP_POLICY_COUNT {
                return Err("backup.warning_count must be at most 1000000".into());
            }
            self.warning_count = value;
        }
        Ok(())
    }
}

/// Deployment cost policy. Values are deliberately expressed in bytes,
/// counts, and rolling windows rather than currency so the policy is
/// portable between hosts and providers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CostPolicy {
    /// Deployment request guardrail, measured over one rolling minute.
    pub requests_per_principal_minute: usize,
    /// Maximum concurrent compiler/font/artifact transfers admitted by the
    /// origin.
    pub artifact_transfers: usize,
    /// Concurrent HTTP handlers performing origin work.
    pub work_concurrency: usize,
    /// TCP peers whose X-Forwarded-For header may be used for client identity.
    /// An empty list means the TCP peer address is authoritative.
    pub trusted_proxies: Vec<String>,
}

pub const DEFAULT_REQUESTS_PER_PRINCIPAL_MINUTE: usize = 6_000;
pub const DEFAULT_ARTIFACT_TRANSFERS: usize = 64;

impl Default for CostPolicy {
    fn default() -> Self {
        Self {
            requests_per_principal_minute: DEFAULT_REQUESTS_PER_PRINCIPAL_MINUTE,
            artifact_transfers: DEFAULT_ARTIFACT_TRANSFERS,
            work_concurrency: 64,
            trusted_proxies: Vec::new(),
        }
    }
}

impl CostPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if self.work_concurrency == 0 {
            return Err("cost.work_concurrency must be positive".into());
        }
        if self.requests_per_principal_minute == 0 || self.artifact_transfers == 0 {
            return Err("cost request and concurrency guardrails must be positive".into());
        }
        if self.trusted_proxies.len() > 128 {
            return Err("trusted_proxies may contain at most 128 networks".into());
        }
        for network in &self.trusted_proxies {
            validate_proxy_network(network)?;
        }
        Ok(())
    }
}

fn validate_proxy_network(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 {
        return Err(format!(
            "trusted_proxies entry {value:?} is empty or too long"
        ));
    }
    let (address, prefix) = value.split_once('/').unwrap_or((value, ""));
    let address = address
        .parse::<std::net::IpAddr>()
        .map_err(|_| format!("trusted_proxies entry {value:?} is not an IP address or CIDR"))?;
    let bits = match address {
        std::net::IpAddr::V4(_) => 32,
        std::net::IpAddr::V6(_) => 128,
    };
    if prefix.is_empty() {
        return if value.contains('/') {
            Err("trusted_proxies CIDR requires a prefix".into())
        } else {
            Ok(())
        };
    }
    let prefix = prefix
        .parse::<u8>()
        .map_err(|_| format!("trusted_proxies entry {value:?} has an invalid prefix"))?;
    if matches!(address, std::net::IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some()) && prefix < 96 {
        return Err("mapped IPv4 proxy CIDR requires a prefix between 96 and 128".into());
    }
    if prefix > bits {
        return Err(format!(
            "trusted_proxies entry {value:?} has prefix /{prefix}, but this address has {bits} bits"
        ));
    }
    Ok(())
}

/// What the server-held document may cost: how big a state may travel inline,
/// how far behind a socket may fall, and how fast one may write.
///
/// These bound the memory and the traffic, which storage quotas say nothing about.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct SessionLimit {
    /// A state larger than this is fetched over HTTP instead of being sent
    /// down the socket, so one cold join of a large document does not sit in a
    /// text frame.
    pub inline_state_max: usize,
    /// How many frames may be queued for one socket before it is disconnected.
    /// A peer that cannot keep up is resynchronised on reconnect, which costs
    /// one state transfer and bounds what a slow reader can make the server
    /// hold.
    pub peer_queue: usize,
    /// How many document updates one socket may send in a minute.
    pub updates_per_minute: i64,
}

/// The maximum length of each free-text field on an annotation.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct CapLimit {
    pub body: usize,
    pub creator: usize,
    pub exact: usize,
    pub context: usize,
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration {
            max_files: 200,
            max_path: 200,
            log_quota_bytes: DEFAULT_LOG_QUOTA_BYTES,
            memory_budget_bytes: 512 * 1024 * 1024,
            cache_expansion: crate::log::budget::DEFAULT_EXPANSION,
            pending_bytes: DEFAULT_PENDING_BYTES,
            pending_scratch_bytes: default_pending_scratch_bytes(DEFAULT_LOG_QUOTA_BYTES),
            text_extensions: [
                ".tex",
                ".typ",
                ".md",
                ".markdown",
                ".qmd",
                ".csl",
                ".r",
                ".py",
                ".jl",
                ".lua",
                ".bib",
                ".sty",
                ".cls",
                ".bst",
                ".txt",
                ".csv",
                ".json",
                ".yml",
                ".yaml",
                ".html",
                ".css",
            ]
            .map(String::from)
            .to_vec(),
            asset_extensions: [
                ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".pdf", ".otf", ".ttf", ".woff",
                ".woff2",
            ]
            .map(String::from)
            .to_vec(),
            derived_extensions: [
                ".aux",
                ".bbl",
                ".blg",
                ".log",
                ".out",
                ".toc",
                ".fls",
                ".fdb_latexmk",
                ".synctex.gz",
                ".lof",
                ".lot",
                ".nav",
                ".snm",
            ]
            .map(String::from)
            .to_vec(),
            storage: StorageLimit {
                total: 5 * 1024 * 1024 * 1024,
                per_owner: 100 * 1024 * 1024,
                uploads_per_hour: 30,
            },
            cost: CostPolicy::default(),
            sockets: crate::server::socket_budget::SocketPolicy::default(),
            backup: BackupPolicy::default(),
            // What the upload form takes. Every one of these is a source
            // format `document_format` names and `storable_source` allows, so
            // the list a person is shown and the list the publish route
            // accepts are the same list. A `.typ` or a `.tex` dropped on its
            // own is a document like any other -- one that reaches its
            // figures and its bibliography only if it came with them, which
            // is what `publish <directory>` is for.
            extensions: [".html", ".htm", ".md", ".markdown", ".qmd", ".typ", ".tex"]
                .map(String::from)
                .to_vec(),
            // HTML is a source format like the other two, and its renderer is
            // the identity: there is no longer a document without a source.
            // LaTeX is one with no renderer on this side at all -- the store
            // keeps the source, and a browser that has fetched a distribution
            // is the only thing anywhere that can make pages of it. Which is
            // why `storable_source` and `renderers` are two questions.
            source_formats: ["markdown", "quarto", "typst", "html", "latex"]
                .map(String::from)
                .to_vec(),
            caps: CapLimit {
                body: 5000,
                creator: 80,
                exact: 1000,
                context: 64,
            },
            motivations: ["commenting", "highlighting", "editing"]
                .map(String::from)
                .to_vec(),
            default_motivation: "commenting".to_string(),
            max_title: 200,
            session: SessionLimit {
                inline_state_max: 256 * 1024,
                peer_queue: 256,
                updates_per_minute: 3000,
            },
            slug_pattern: r"^[a-z0-9]+(?:-[a-z0-9]+)*$".to_string(),
            slug_max: 80,
            suffix_alphabet: "abcdefghijkmnpqrstuvwxyz23456789".to_string(),
            suffix_length: 10,
        }
    }
}

impl Configuration {
    /// Keeps an unknown motivation out of storage, falling back to the default
    /// rather than rejecting the annotation.
    pub fn allowed_motivation(&self, value: &str) -> String {
        if self.motivations.iter().any(|known| known == value) {
            value.to_string()
        } else {
            self.default_motivation.clone()
        }
    }

    /// Says whether a source in this format is worth keeping beside the
    /// document it rendered to. Whether it can be rendered again *here* is a
    /// separate question, answered by `renderers`.
    pub fn storable_source(&self, format: &str) -> bool {
        self.source_formats.iter().any(|known| known == format)
    }

    /// Apply the optional operator-declared backup policy.
    pub fn apply_backup_overrides(
        &mut self,
        overrides: BackupPolicyOverrides,
    ) -> Result<(), String> {
        self.backup.apply(overrides)
    }

    /// Overrides the storage ceilings, in megabytes: how much one publisher may
    /// hold across all their documents, and how much the whole deployment will
    /// hold. `None` leaves a default alone.
    pub fn set_storage(
        &mut self,
        quota_mb: Option<usize>,
        total_mb: Option<usize>,
    ) -> Result<(), String> {
        if let Some(quota_mb) = quota_mb {
            self.storage.per_owner = (quota_mb * 1024 * 1024) as i64;
        }
        if let Some(total_mb) = total_mb {
            self.storage.total = (total_mb * 1024 * 1024) as i64;
        }
        if self.storage.per_owner > self.storage.total {
            return Err(format!(
                "--publisher-storage-limit ({} MB) cannot exceed --deployment-storage-limit ({} MB)",
                self.storage.per_owner >> 20,
                self.storage.total >> 20
            ));
        }
        Ok(())
    }

    /// Overrides how many frames one socket's outbound queue may hold before
    /// the subscriber is closed and asked to reconnect. The byte budget is
    /// this limit times 64 KiB. Advanced configuration only; tests lower it
    /// to reach overflow once a stalled socket stops draining the queue.
    pub fn set_peer_queue(&mut self, frames: Option<usize>) -> Result<(), String> {
        let Some(frames) = frames else {
            return Ok(());
        };
        if frames == 0 {
            return Err("session.peer_queue must be at least one frame".into());
        }
        self.session.peer_queue = frames;
        Ok(())
    }

    /// Overrides the deployment-wide pending-source ceilings, in megabytes.
    /// `None` leaves a default alone. Advanced configuration only; an
    /// integration test lowers them to reach pressure without buffering
    /// sixty-four megabytes first.
    pub fn set_pending(
        &mut self,
        pending_mb: Option<u64>,
        scratch_mb: Option<u64>,
    ) -> Result<(), String> {
        // Checked on a candidate and committed only if it holds, so a refused
        // override leaves the configuration as it was rather than half
        // applied.
        let mut candidate = self.clone();
        if let Some(megabytes) = pending_mb {
            candidate.pending_bytes = megabytes
                .checked_mul(1024 * 1024)
                .ok_or_else(|| "pending_bytes is too large".to_string())?;
        }
        if let Some(megabytes) = scratch_mb {
            candidate.pending_scratch_bytes = megabytes
                .checked_mul(1024 * 1024)
                .ok_or_else(|| "pending_scratch_bytes is too large".to_string())?;
        }
        candidate.validate_pending()?;
        self.pending_bytes = candidate.pending_bytes;
        self.pending_scratch_bytes = candidate.pending_scratch_bytes;
        Ok(())
    }

    /// Refuses a pending configuration that could not save the work it
    /// admits.
    ///
    /// Two impossibilities, and they are different. A `pending_bytes` below
    /// one document's buffer ceiling would refuse an update this deployment
    /// still advertises it accepts, and would do so from the very first
    /// document. A `pending_scratch_bytes` below one maximum row's scratch
    /// would let work be accepted that no flush could ever write -- the
    /// deadlock the two-pool design exists to prevent, reintroduced through
    /// configuration. Both are checked arithmetic: an operator who writes a
    /// number near `u64::MAX` gets a configuration error rather than a
    /// comparison that wrapped.
    pub fn validate_pending(&self) -> Result<(), String> {
        use crate::log::sequencer::{max_pending_charge, max_row_bytes, BUFFER_CEILING_BYTES};

        if self.pending_bytes == 0 || self.pending_scratch_bytes == 0 {
            return Err("the pending-source ceilings must be positive".into());
        }
        if self.pending_bytes < max_pending_charge(self.log_quota_bytes) as u64 {
            return Err(format!(
                "the deployment pending-source ceiling ({} MB) cannot hold one document's \
                 buffer, which this deployment allows to reach {} MB",
                self.pending_bytes >> 20,
                BUFFER_CEILING_BYTES >> 20,
            ));
        }
        // A row past the log quota is refused, so the quota is the largest row
        // any path writes.
        let largest_row = max_row_bytes(self.log_quota_bytes) as u64;
        let needed = crate::log::pending::scratch_for(largest_row);
        if self.pending_scratch_bytes < needed {
            return Err(format!(
                "the persistence scratch ceiling ({} MB) cannot write one maximum-size row, \
                 which needs {} MB; accepted work would then have no way to reach storage",
                self.pending_scratch_bytes >> 20,
                needed >> 20,
            ));
        }
        Ok(())
    }

    /// Overrides how many uploads one publisher may make in an hour. `None`
    /// leaves the default alone.
    pub fn set_uploads_per_hour(&mut self, uploads_per_hour: Option<usize>) -> Result<(), String> {
        if let Some(uploads_per_hour) = uploads_per_hour {
            self.storage.uploads_per_hour = uploads_per_hour;
        }
        Ok(())
    }

    /// Overrides the per-document log ceiling, in megabytes. `None` leaves the
    /// default alone. Refuses zero or a quota smaller than the buffer ceiling.
    /// Does not check the memory budget, because the two are checked together
    /// by `validate_budgets` once every override is applied.
    pub fn set_log_quota(&mut self, megabytes: Option<u64>) -> Result<(), String> {
        use crate::log::sequencer::BUFFER_CEILING_BYTES;

        let Some(megabytes) = megabytes else {
            return Ok(());
        };
        if megabytes == 0 {
            return Err("log quota must be positive".to_string());
        }
        let bytes = megabytes
            .checked_mul(1024 * 1024)
            .ok_or_else(|| "log quota is too large".to_string())?;
        if (bytes as usize) < BUFFER_CEILING_BYTES {
            return Err(format!(
                "the log quota cannot be below the buffer ceiling of {} bytes",
                BUFFER_CEILING_BYTES
            ));
        }

        // If scratch is at the old default, move it to the new default.
        let old_default_scratch = default_pending_scratch_bytes(self.log_quota_bytes);
        let new_quota = bytes as usize;
        if self.pending_scratch_bytes == old_default_scratch {
            self.pending_scratch_bytes = default_pending_scratch_bytes(new_quota);
        }

        self.log_quota_bytes = new_quota;
        Ok(())
    }

    /// Overrides the process-wide budget for decoded documents, in megabytes.
    /// `None` leaves the default alone. Refuses zero. Does not check the log
    /// quota, because the two are checked together by `validate_budgets` once
    /// every override is applied.
    pub fn set_memory_budget(&mut self, megabytes: Option<u64>) -> Result<(), String> {
        let Some(megabytes) = megabytes else {
            return Ok(());
        };
        if megabytes == 0 {
            return Err("memory budget must be positive".to_string());
        }
        let bytes = megabytes
            .checked_mul(1024 * 1024)
            .ok_or_else(|| "memory budget is too large".to_string())?;
        self.memory_budget_bytes = bytes;
        Ok(())
    }

    /// Validates that the log quota and memory budget are compatible:
    /// the quota can hold one maximum-size row, and the budget can build and
    /// read one document at the quota this deployment allows. A cold build
    /// reserves the resident expansion plus the transient one, and Budget::reserve
    /// refuses a request larger than the whole limit, so the budget must cover both.
    pub fn validate_budgets(&self) -> Result<(), String> {
        use crate::log::sequencer::BUFFER_CEILING_BYTES;

        if self.log_quota_bytes < BUFFER_CEILING_BYTES {
            return Err(format!(
                "the log quota ({} MB) cannot be below the buffer ceiling of {} bytes",
                self.log_quota_bytes >> 20,
                BUFFER_CEILING_BYTES,
            ));
        }

        let quota = self.log_quota_bytes as u64;
        let resident = crate::log::budget::estimate(quota, self.cache_expansion);
        let transient =
            crate::log::budget::estimate(quota, crate::log::budget::BUILD_TRANSIENT_EXPANSION);
        let needed = resident
            .checked_add(transient)
            .ok_or_else(|| "the memory budget requirement overflows".to_string())?;

        if self.memory_budget_bytes < needed {
            return Err(format!(
                "the memory budget ({} MB) cannot build one document at the log quota this \
                 deployment allows ({} MB): a cold build reserves {} MB, resident plus \
                 transient, and a reservation past the budget is refused outright, so every \
                 document at the quota would be unreadable",
                self.memory_budget_bytes >> 20,
                self.log_quota_bytes >> 20,
                needed >> 20,
            ));
        }
        Ok(())
    }
}

/// The per-document log ceiling, in bytes.
///
/// The default is what a 512 MiB memory budget can build, because a cold build
/// reserves the resident expansion plus the transient one, fourteen times the
/// log at the measured factors, and raising the quota means raising the budget
/// with it.
pub const DEFAULT_LOG_QUOTA_BYTES: usize = 32 * 1024 * 1024;

/// How much unsaved source the whole deployment will hold at once.
///
/// Conservative on purpose, for the small VPS this targets. One document may
/// buffer 4 MiB (`sequencer::BUFFER_CEILING_BYTES`), so this is sixteen
/// documents entirely backed up, or -- far more likely -- several hundred
/// documents each holding a few seconds of typing. It is an eighth of the
/// default decoded-document budget (`memory_budget_bytes`, 512 MiB), which is
/// the right proportion for a process whose expensive half is decoded CRDTs:
/// the two together add at most 128 MiB above that budget rather than the
/// several gigabytes a per-document cap alone permits across a thousand
/// documents (SPEC-frugal §2).
pub const DEFAULT_PENDING_BYTES: u64 = 64 * 1024 * 1024;

/// Compute the default pending-scratch ceiling for a given log quota.
///
/// What writing one maximum row costs while the write is in flight: the encoded
/// row and both driver buffers, including capacity growth. The pool is a ceiling
/// on reservations, not memory held.
pub fn default_pending_scratch_bytes(log_quota_bytes: usize) -> u64 {
    crate::log::pending::scratch_for(crate::log::sequencer::max_row_bytes(log_quota_bytes) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configuration that cannot persist what it admits has to be refused
    /// at startup rather than discovered when the first document fills up.
    #[test]
    fn pending_ceilings_that_could_not_save_what_they_admit_are_refused() {
        let config = Configuration::default();
        assert!(
            config.validate_pending().is_ok(),
            "the defaults must be sane"
        );

        // Scratch that cannot hold one maximum row: work would be accepted
        // that no flush could ever write. This is the deadlock the two-pool
        // design exists to prevent, and configuration must not reintroduce it.
        let mut starved = config.clone();
        starved.pending_scratch_bytes = 1024;
        let error = starved
            .validate_pending()
            .expect_err("a scratch ceiling below one row is not a valid configuration");
        assert!(error.contains("maximum-size row"), "{error}");

        // A retained ceiling below one document's own allowance would refuse
        // updates this deployment still says it accepts.
        let mut cramped = config.clone();
        cramped.pending_bytes = 1024;
        assert!(cramped.validate_pending().is_err());

        // Zero is not a budget.
        let mut nothing = config.clone();
        nothing.pending_bytes = 0;
        assert!(nothing.validate_pending().is_err());
    }

    /// The megabyte setters an operator reaches through the advanced
    /// configuration file validate what they were given rather than storing
    /// it and failing later.
    #[test]
    fn the_pending_overrides_validate_what_they_are_given() {
        let mut config = Configuration::default();
        assert!(config.set_pending(Some(128), Some(256)).is_ok());
        assert_eq!(config.pending_bytes, 128 * 1024 * 1024);
        assert_eq!(config.pending_scratch_bytes, 256 * 1024 * 1024);
        assert!(
            config.set_pending(None, Some(1)).is_err(),
            "one megabyte of scratch is too small for the largest row",
        );
        assert_eq!(
            config.pending_scratch_bytes,
            256 * 1024 * 1024,
            "a refused override left the configuration half applied",
        );
        assert!(config.set_pending(Some(u64::MAX), None).is_err());
        assert_eq!(config.pending_bytes, 128 * 1024 * 1024);
    }

    #[test]
    fn log_quota_change_updates_scratch_default() {
        let mut config = Configuration::default();
        let old_scratch = config.pending_scratch_bytes;

        // When scratch is at the old default and log quota changes,
        // scratch should move to the new default.
        assert!(config.set_log_quota(Some(64)).is_ok());
        let new_scratch = default_pending_scratch_bytes(64 * 1024 * 1024);
        assert_eq!(config.pending_scratch_bytes, new_scratch);
        assert_ne!(old_scratch, new_scratch);

        // But if scratch was set explicitly, it should stay unchanged.
        let mut config2 = Configuration {
            pending_scratch_bytes: 100 * 1024 * 1024,
            ..Default::default()
        };
        let explicit_scratch = config2.pending_scratch_bytes;
        assert!(config2.set_log_quota(Some(128)).is_ok());
        assert_eq!(config2.pending_scratch_bytes, explicit_scratch);
    }

    #[test]
    fn backup_policy_overrides_are_bounded_and_reported() {
        let overrides: BackupPolicyOverrides = serde_yaml::from_str(
            "destination_class: object-store\nfrequency: 86400\nretained_count: 30\nencrypted: true\nwarning_count: 20\n",
        )
        .unwrap();
        let mut config = Configuration::default();
        config.apply_backup_overrides(overrides).unwrap();
        assert_eq!(config.backup.warning_count, 20);
    }

    /// The megabyte setters an operator reaches through the advanced
    /// configuration file validate what they were given rather than storing
    /// it and failing later. The log quota and memory budget are checked
    /// together by validate_budgets once every override is applied.
    #[test]
    fn the_budget_overrides_validate_what_they_are_given() {
        let mut config = Configuration::default();

        // Configuration::default().validate_budgets() is ok
        assert!(
            config.validate_budgets().is_ok(),
            "the defaults must be sane"
        );

        // set_log_quota(Some(128)) is ok and stores 128 MiB
        assert!(config.set_log_quota(Some(128)).is_ok());
        assert_eq!(config.log_quota_bytes, 128 * 1024 * 1024);

        // validate_budgets() now errors (128 MiB times fourteen exceeds 512 MiB)
        assert!(
            config.validate_budgets().is_err(),
            "128 MiB at expansion 6+8 exceeds 512 MiB budget"
        );

        // set_memory_budget(Some(2048)) is ok and stores 2 GiB
        assert!(config.set_memory_budget(Some(2048)).is_ok());
        assert_eq!(config.memory_budget_bytes, 2048 * 1024 * 1024);

        // validate_budgets() is ok
        assert!(
            config.validate_budgets().is_ok(),
            "2 GiB budget can build 128 MiB quota"
        );

        // set_log_quota(Some(0)) errors and leaves 128 MiB
        assert!(config.set_log_quota(Some(0)).is_err());
        assert_eq!(config.log_quota_bytes, 128 * 1024 * 1024);

        // set_log_quota(Some(1)) errors because 1 MiB is below BUFFER_CEILING_BYTES (4 MiB), and leaves 128 MiB
        assert!(
            config.set_log_quota(Some(1)).is_err(),
            "1 MiB is below BUFFER_CEILING_BYTES"
        );
        assert_eq!(config.log_quota_bytes, 128 * 1024 * 1024);

        // set_memory_budget(Some(0)) errors and leaves 2 GiB
        assert!(config.set_memory_budget(Some(0)).is_err());
        assert_eq!(config.memory_budget_bytes, 2048 * 1024 * 1024);

        // set_log_quota(Some(u64::MAX)) errors and leaves 128 MiB
        assert!(config.set_log_quota(Some(u64::MAX)).is_err());
        assert_eq!(config.log_quota_bytes, 128 * 1024 * 1024);
    }
}

//! Every rule the deployment enforces lives here, and only here. The shell
//! gets these values injected into its source in place of `__CONFIG__`; the
//! server reads them directly. A limit changed here changes everywhere on the
//! next build.

use serde::Serialize;
use std::path::{Path, PathBuf};

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

    /// Apply the private state-directory boundary before opening a catalogue
    /// or replica. Existing directories are tightened as well; relying on the
    /// process umask alone leaves an unsafe deployment after a permissions
    /// change or restore.
    pub fn prepare_state(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.state)
            .map_err(|err| format!("could not create {}: {err}", self.state.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.state, std::fs::Permissions::from_mode(0o700))
                .map_err(|err| format!("could not protect {}: {err}", self.state.display()))?;
        }
        Ok(())
    }

    pub fn protect_file(path: &Path) -> Result<(), String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|err| format!("could not protect {}: {err}", path.display()))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Configuration {
    /// The sum of every text in a document and of every key naming one. A
    /// paper split into thirty files is allowed exactly what a paper in one
    /// file is allowed, which is why this bounds the sum rather than each
    /// text. It bounds a rendering too, now that a document may store one, so
    /// it is named for the document rather than for the HTML it once was.
    pub max_document: usize,
    /// How many files one document may hold, across its texts and its assets.
    /// A paper has a dozen; a directory of two hundred is somebody using a
    /// document as a filesystem.
    pub max_files: usize,
    /// The longest one path may be, in bytes.
    pub max_path: usize,
    /// What the figures of one document may come to, and what one of them may
    /// be. Assets are where the bytes of a paper actually go -- a directory of
    /// figures is an order of magnitude larger than its text -- so they get
    /// ceilings of their own beside `max_document` rather than sharing it, and
    /// they count against the owner's quota like everything else stored.
    pub max_assets: i64,
    pub max_asset: i64,
    /// Which extensions name a file a person edits, which name bytes nobody
    /// edits in place, and which name what a compiler wrote -- and a document
    /// keeps what a person wrote. Rules rather than constants, so a deployment
    /// can widen or narrow them without a build.
    pub text_extensions: Vec<String>,
    pub asset_extensions: Vec<String>,
    pub derived_extensions: Vec<String>,
    pub max_comments: usize,
    pub rate_per_hour: i64,
    pub caps: CapLimit,

    /// Storage is what keeps a deployment's bill bounded no matter who shows
    /// up: a ceiling on everything stored, a ceiling per publisher, and a cap
    /// on how many documents and how many uploads an hour one publisher gets.
    /// Sizes are bytes of stored HTML; every index entry records its own.
    pub storage: StorageLimit,

    /// The deployment-wide cost envelope. Most of these are implementation
    /// guardrails with documented defaults; the daily transfer allowance is
    /// the one everyday cost control exposed by the CLI. Keeping this policy
    /// beside the document and storage limits makes the effective policy a
    /// single value for startup reporting and the server.
    #[serde(skip)]
    pub cost: CostPolicy,
    #[serde(skip)]
    pub sockets: crate::server::socket_budget::SocketPolicy,
    /// Optional reporting metadata for operator-managed backups.
    pub backup: BackupPolicy,
    #[serde(skip)]
    pub policy_origins: std::collections::BTreeMap<String, String>,

    /// Caps the serialized seed annotations a reserved example carries, in
    /// bytes.
    pub max_annotations: usize,

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
    /// Caps replies on one comment, so a thread cannot grow without bound and a
    /// room stays small enough to load and rewrite whole.
    pub max_replies: usize,

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

    /// The persistence policy: what a source may be, what its encoded CRDT
    /// snapshot may be, and the transient memory budget that must carry one.
    /// Not part of the shell configuration -- a
    /// browser has no use for the server's storage ceilings -- so it is
    /// skipped rather than injected. `max_source_bytes` follows
    /// `max_document`; read the pair through [`Configuration::persistence`].
    #[serde(skip)]
    pub persistence: PersistenceLimits,
}

/// Bounds what a deployment will hold. `total` and `per_owner` are bytes;
/// `documents_per_owner` and `uploads_per_hour` are counts.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct StorageLimit {
    pub total: i64,
    pub per_owner: i64,
    pub documents_per_owner: usize,
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

/// Versioned deployment cost policy. Values are deliberately expressed in
/// bytes, counts, and rolling windows rather than currency so the policy is
/// portable between hosts and providers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CostPolicy {
    /// Schema version for status output and future advanced configuration.
    pub version: u32,
    /// Origin response bytes allowed in a rolling 24-hour window. `None`
    /// retains the historical unlimited behavior; `Some(0)` refuses ordinary
    /// transfer while leaving the emergency allowance available.
    pub transfer_bytes: Option<u64>,
    /// Internal reserve for health, authentication, deletion, export-control,
    /// quota-status, and durability acknowledgements.
    pub emergency_bytes: u64,
    /// Deployment request guardrails, each measured over one rolling minute.
    pub requests_per_minute: usize,
    pub requests_per_network_minute: usize,
    pub requests_per_principal_minute: usize,
    pub requests_per_document_minute: usize,
    /// Maximum concurrent compiler/font/artifact transfers admitted by the
    /// origin.
    pub artifact_transfers: usize,
    /// Concurrent HTTP handlers performing origin work.
    pub work_concurrency: usize,
    /// Deployment-wide request body parsing/decoded-payload memory ceiling.
    pub request_body_memory_bytes: usize,
    /// TCP peers whose X-Forwarded-For header may be used for client identity.
    /// An empty list means the TCP peer address is authoritative.
    pub trusted_proxies: Vec<String>,
}

pub const COST_POLICY_VERSION: u32 = 1;
pub const DEFAULT_EMERGENCY_BYTES: u64 = 1 << 20;
pub const DEFAULT_REQUESTS_PER_MINUTE: usize = 60_000;
pub const DEFAULT_REQUESTS_PER_NETWORK_MINUTE: usize = 6_000;
pub const DEFAULT_REQUESTS_PER_PRINCIPAL_MINUTE: usize = 6_000;
pub const DEFAULT_REQUESTS_PER_DOCUMENT_MINUTE: usize = 12_000;
pub const DEFAULT_ARTIFACT_TRANSFERS: usize = 64;
/// Maximum decoded/request parsing memory reserved across concurrent HTTP
/// handlers. Four times the incoming body estimate is admitted before reads.
pub const DEFAULT_REQUEST_BODY_MEMORY_BYTES: usize = 256 * 1024 * 1024;

/// Optional advanced configuration-file values. Every member is optional so
/// operators can override one guardrail without copying the policy defaults.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CostPolicyOverrides {
    pub transfer_bytes: Option<u64>,
    pub requests_per_minute: Option<usize>,
    pub requests_per_network_minute: Option<usize>,
    pub requests_per_principal_minute: Option<usize>,
    pub requests_per_document_minute: Option<usize>,
    pub artifact_transfers: Option<usize>,
    pub work_concurrency: Option<usize>,
    pub request_body_memory_bytes: Option<usize>,
}

impl Default for CostPolicy {
    fn default() -> Self {
        Self {
            version: COST_POLICY_VERSION,
            transfer_bytes: None,
            emergency_bytes: DEFAULT_EMERGENCY_BYTES,
            requests_per_minute: DEFAULT_REQUESTS_PER_MINUTE,
            requests_per_network_minute: DEFAULT_REQUESTS_PER_NETWORK_MINUTE,
            requests_per_principal_minute: DEFAULT_REQUESTS_PER_PRINCIPAL_MINUTE,
            requests_per_document_minute: DEFAULT_REQUESTS_PER_DOCUMENT_MINUTE,
            artifact_transfers: DEFAULT_ARTIFACT_TRANSFERS,
            work_concurrency: 64,
            request_body_memory_bytes: DEFAULT_REQUEST_BODY_MEMORY_BYTES,
            trusted_proxies: Vec::new(),
        }
    }
}

impl CostPolicy {
    /// The machine-readable policy used by operator status output. The
    /// explicit field names keep an omitted advanced setting distinguishable
    /// from an unlimited transfer budget.
    pub fn effective_policy(&self) -> serde_json::Value {
        serde_json::json!({
            "version": self.version,
            "transfer_bytes": self.transfer_bytes,
            "emergency_bytes": self.emergency_bytes,
            "requests_per_minute": self.requests_per_minute,
            "requests_per_network_minute": self.requests_per_network_minute,
            "requests_per_principal_minute": self.requests_per_principal_minute,
            "requests_per_document_minute": self.requests_per_document_minute,
            "artifact_transfers": self.artifact_transfers,
            "work_concurrency": self.work_concurrency,
            "request_body_memory_bytes": self.request_body_memory_bytes,
            "trusted_proxies": self.trusted_proxies,
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.work_concurrency == 0 {
            return Err("cost.work_concurrency must be positive".into());
        }
        if self.request_body_memory_bytes == 0 || self.request_body_memory_bytes > u32::MAX as usize
        {
            return Err(
                "cost.request_body_memory_bytes must be between 1 and 4294967295 bytes".into(),
            );
        }
        if self.version != COST_POLICY_VERSION {
            return Err(format!(
                "unsupported cost policy version {}; this build supports {}",
                self.version, COST_POLICY_VERSION
            ));
        }
        if self.emergency_bytes == 0 {
            return Err("the emergency transfer allowance must be positive".into());
        }
        if self.requests_per_minute == 0
            || self.requests_per_network_minute == 0
            || self.requests_per_principal_minute == 0
            || self.requests_per_document_minute == 0
            || self.artifact_transfers == 0
        {
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

/// Parse a daily transfer allowance. Binary units avoid ambiguity when an
/// operator is budgeting storage and network together. A bare integer is
/// bytes, as required by the CLI contract.
pub fn parse_budget_transfer(value: &str) -> Result<u64, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("--transfer-budget needs a byte count (for example 10GiB)".into());
    }
    if value.starts_with('-') {
        return Err(format!("--transfer-budget {value:?} cannot be negative"));
    }
    let split = value
        .bytes()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(value.len());
    let (digits, unit) = value.split_at(split);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "--transfer-budget {value:?} is not a byte count; use an integer with an optional B, KiB, MiB, GiB, or TiB suffix"
        ));
    }
    let number = digits
        .parse::<u64>()
        .map_err(|_| format!("--transfer-budget {value:?} is too large"))?;
    let multiplier = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kib" => 1 << 10,
        "mib" => 1 << 20,
        "gib" => 1 << 30,
        "tib" => 1 << 40,
        _ => {
            return Err(format!(
                "--transfer-budget {value:?} has an unknown unit; use B, KiB, MiB, GiB, or TiB"
            ));
        }
    };
    number
        .checked_mul(multiplier)
        .ok_or_else(|| format!("--transfer-budget {value:?} is too large"))
}

/// What the server-held document may cost: how often it is written, how big a
/// state may travel inline, how many rooms may be hot at once, how far behind
/// a socket may fall, and how fast one may write.
///
/// Storage quotas bound the bill; these bound the memory and the traffic,
/// which storage quotas say nothing about.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct SessionLimit {
    /// Seconds of quiet before the room writes `sessions/<slug>`. Every update
    /// is relayed at once whatever this is; what waits is the durability
    /// acknowledgment, and nothing tells a browser its work is safe until the
    /// write has landed.
    pub write_after_seconds: i64,
    /// Seconds of quiet before a checkpoint is taken.
    pub checkpoint_seconds: i64,
    /// Maximum interval between automatic checkpoints for changed rooms,
    /// independent of quiet-time debounce and room eviction.
    pub history_interval_seconds: i64,
    /// Rolling-hour exact/automatic checkpoint budget for one owner.
    pub checkpoint_owner_per_hour: i64,
    /// Rolling-hour checkpoint budget shared by the deployment.
    pub checkpoint_deployment_per_hour: i64,
    /// The most checkpoints one document keeps. Zero is no cap.
    pub history_max: usize,
    /// A state larger than this is fetched over HTTP instead of being sent
    /// down the socket, so one cold join of a large document does not sit in a
    /// text frame.
    pub inline_state_max: usize,
    /// How many documents may be held in memory at once. The least recently
    /// used idle room is persisted and evicted past this.
    pub rooms_max: usize,
    /// Conservative resident-memory budget for all open and loading rooms.
    pub rooms_bytes_max: usize,
    /// How many frames may be queued for one socket before it is disconnected.
    /// A peer that cannot keep up is resynchronised on reconnect, which costs
    /// one state transfer and bounds what a slow reader can make the server
    /// hold.
    pub peer_queue: usize,
    /// How many document updates one socket may send in a minute.
    pub updates_per_minute: i64,
}

/// The quiet period before an edited document receives an automatic checkpoint.
/// Keep this separate from the configurable session limit so deployments can
/// tune policy without changing the scheduling algorithm.
pub const CHECKPOINT_QUIET_SECONDS: i64 = 30;

/// The maximum age of a pending edit before an automatic checkpoint is due.
/// This is measured from the first uncheckpointed edit, not from the previous
/// checkpoint, so continuous editing cannot postpone it indefinitely.
pub const CHECKPOINT_MAX_INTERVAL_SECONDS: i64 = 5 * 60;

/// A checkpoint asked for within this many seconds of the last one waits until
/// they have passed. A burst of saves is one mark in the timeline, and a sync
/// client writing all day is two marks a minute at most. A constant rather
/// than a flag, on purpose.
pub const CHECKPOINT_DEFER_SECONDS: i64 = 30;

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
            max_document: DEFAULT_MAX_SOURCE_BYTES,
            max_files: 200,
            max_path: 200,
            max_assets: 32 * 1024 * 1024,
            max_asset: 8 * 1024 * 1024,
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
            max_comments: 500,
            rate_per_hour: 20,
            storage: StorageLimit {
                total: 5 * 1024 * 1024 * 1024,
                per_owner: 100 * 1024 * 1024,
                documents_per_owner: 50,
                uploads_per_hour: 30,
            },
            cost: CostPolicy::default(),
            sockets: crate::server::socket_budget::SocketPolicy::default(),
            backup: BackupPolicy::default(),
            policy_origins: std::collections::BTreeMap::new(),
            max_annotations: 256 * 1024,
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
            max_replies: 100,
            session: SessionLimit {
                write_after_seconds: 2,
                checkpoint_seconds: CHECKPOINT_QUIET_SECONDS,
                history_interval_seconds: CHECKPOINT_MAX_INTERVAL_SECONDS,
                checkpoint_owner_per_hour: 300,
                checkpoint_deployment_per_hour: 10_000,
                history_max: 0,
                inline_state_max: 256 * 1024,
                rooms_max: 200,
                rooms_bytes_max: 512 * 1024 * 1024,
                peer_queue: 256,
                updates_per_minute: 3000,
            },
            slug_pattern: r"^[a-z0-9]+(?:-[a-z0-9]+)*$".to_string(),
            slug_max: 80,
            suffix_alphabet: "abcdefghijkmnpqrstuvwxyz23456789".to_string(),
            suffix_length: 10,
            persistence: PersistenceLimits::default(),
        }
    }
}

impl Configuration {
    /// Return the complete effective operator policy for startup and the
    /// loopback status surface. Callers should redact `trusted_proxies` when
    /// exposing a public configuration response.
    pub fn effective_policy(&self) -> serde_json::Value {
        serde_json::json!({
            "version": COST_POLICY_VERSION,
            "cost": self.cost.effective_policy(),
            "limits": self.policy_limits(),
            "storage": self.storage,
            "backup": self.backup,
            "document": {
                "max_source_bytes": self.max_document,
                "max_input_assets_bytes": self.max_assets,
                "max_asset_bytes": self.max_asset,
            },
            "session": self.session,
        })
    }

    pub fn policy_limits(&self) -> Vec<serde_json::Value> {
        use serde_json::json;
        let mut limits = Vec::new();
        let mut add = |name: &str,
                       value: serde_json::Value,
                       units: &str,
                       scope: &str,
                       window: Option<u64>| {
            limits.push(json!({"name":name,"value":value,"units":units,"scope":scope,"window_seconds":window,"origin":self.policy_origins.get(name).map(String::as_str).unwrap_or("built-in default")}));
        };
        add(
            "cost.transfer_bytes",
            self.cost
                .transfer_bytes
                .map_or(json!("unlimited"), |n| json!(n)),
            "bytes",
            "deployment",
            Some(86400),
        );
        add(
            "cost.emergency_bytes",
            json!(self.cost.emergency_bytes),
            "bytes",
            "deployment",
            Some(86400),
        );
        for (name, value, scope) in [
            (
                "cost.requests_per_minute",
                self.cost.requests_per_minute,
                "deployment",
            ),
            (
                "cost.requests_per_network_minute",
                self.cost.requests_per_network_minute,
                "network",
            ),
            (
                "cost.requests_per_principal_minute",
                self.cost.requests_per_principal_minute,
                "principal",
            ),
            (
                "cost.requests_per_document_minute",
                self.cost.requests_per_document_minute,
                "document",
            ),
        ] {
            add(name, json!(value), "requests", scope, Some(60));
        }
        add(
            "cost.work_concurrency",
            json!(self.cost.work_concurrency),
            "handlers",
            "deployment",
            None,
        );
        add(
            "cost.request_body_memory_bytes",
            json!(self.cost.request_body_memory_bytes),
            "bytes",
            "deployment",
            None,
        );
        add(
            "work.emergency_concurrency",
            json!(16),
            "handlers",
            "deployment",
            None,
        );
        add(
            "cost.artifact_transfers",
            json!(self.cost.artifact_transfers),
            "concurrent transfers",
            "deployment",
            None,
        );
        add(
            "storage.total",
            json!(self.storage.total),
            "bytes",
            "deployment",
            None,
        );
        add(
            "storage.per_owner",
            json!(self.storage.per_owner),
            "bytes",
            "owner",
            None,
        );
        add(
            "storage.documents_per_owner",
            json!(self.storage.documents_per_owner),
            "documents",
            "owner",
            None,
        );
        add(
            "storage.uploads_per_hour",
            json!(self.storage.uploads_per_hour),
            "uploads",
            "owner",
            Some(3600),
        );
        add(
            "max_document",
            json!(self.max_document),
            "bytes",
            "document source",
            None,
        );
        add(
            "max_assets",
            json!(self.max_assets),
            "bytes",
            "document inputs",
            None,
        );
        add(
            "max_asset",
            json!(self.max_asset),
            "bytes",
            "input file",
            None,
        );
        add(
            "session.rooms_max",
            json!(self.session.rooms_max),
            "rooms",
            "deployment",
            None,
        );
        add(
            "session.rooms_bytes_max",
            json!(self.session.rooms_bytes_max),
            "bytes",
            "resident rooms",
            None,
        );
        add(
            "session.peer_queue",
            json!(self.session.peer_queue),
            "frames",
            "peer",
            None,
        );
        add(
            "session.updates_per_minute",
            json!(self.session.updates_per_minute),
            "updates",
            "peer",
            Some(60),
        );
        add(
            "session.checkpoint_owner_per_hour",
            json!(self.session.checkpoint_owner_per_hour),
            "checkpoints",
            "owner",
            Some(3600),
        );
        add(
            "session.checkpoint_deployment_per_hour",
            json!(self.session.checkpoint_deployment_per_hour),
            "checkpoints",
            "deployment",
            Some(3600),
        );
        add(
            "session.checkpoint_seconds",
            json!(self.session.checkpoint_seconds),
            "seconds",
            "document quiet period",
            None,
        );
        add(
            "session.history_max",
            if self.session.history_max == 0 {
                json!("unlimited")
            } else {
                json!(self.session.history_max)
            },
            "checkpoints",
            "document",
            None,
        );
        add(
            "persistence.max_encoded_snapshot_bytes",
            json!(self.persistence.max_encoded_snapshot_bytes),
            "bytes",
            "snapshot",
            None,
        );
        add(
            "persistence.max_staging_bytes",
            json!(self.persistence.max_staging_bytes),
            "bytes",
            "persistence memory",
            None,
        );
        add("oauth.requests", json!(16), "requests", "deployment", None);
        add(
            "oauth.lookup_requests",
            json!(8),
            "requests",
            "deployment",
            None,
        );
        add(
            "oauth.connect_timeout",
            json!(5),
            "seconds",
            "provider request",
            None,
        );
        add(
            "oauth.total_timeout",
            json!(15),
            "seconds",
            "provider request",
            None,
        );
        add("fonts.files", json!(4096), "files", "font library", None);
        add(
            "requests.emergency",
            json!(300),
            "requests",
            "deployment",
            Some(60),
        );
        add(
            "requests.emergency_network",
            json!(60),
            "requests",
            "network",
            Some(60),
        );
        add(
            "requests.identity_keys",
            json!(4096),
            "identities",
            "deployment",
            Some(60),
        );
        add(
            "proxy.header_bytes",
            json!(2048),
            "bytes",
            "forwarding chain",
            None,
        );
        add(
            "proxy.hops",
            json!(32),
            "addresses",
            "forwarding chain",
            None,
        );
        for (name, value) in serde_json::to_value(self.sockets)
            .unwrap()
            .as_object()
            .unwrap()
        {
            let units = if name.contains("bytes") {
                "bytes"
            } else if name.ends_with("seconds") {
                "seconds"
            } else {
                "connections"
            };
            add(
                &format!("sockets.{name}"),
                value.clone(),
                units,
                "live collaboration",
                name.starts_with("state_").then_some(3600),
            );
        }
        add(
            "backup.warning_count",
            json!(self.backup.warning_count),
            "backups",
            "deployment",
            None,
        );
        if let Some(frequency) = self.backup.frequency {
            add(
                "backup.frequency",
                json!(frequency),
                "seconds",
                "deployment",
                None,
            );
        }
        if let Some(retained_count) = self.backup.retained_count {
            add(
                "backup.retained_count",
                json!(retained_count),
                "backups",
                "deployment",
                None,
            );
        }
        if let Some(destination_class) = &self.backup.destination_class {
            add(
                "backup.destination_class",
                json!(destination_class),
                "class",
                "backup",
                None,
            );
        }
        if let Some(encrypted) = self.backup.encrypted {
            add(
                "backup.encrypted",
                json!(encrypted),
                "boolean",
                "backup",
                None,
            );
        }
        // Include the remaining existing numeric guardrails without making
        // operators copy defaults into their optional configuration file.
        fn numeric_limits(
            value: &serde_json::Value,
            prefix: &str,
            origins: &std::collections::BTreeMap<String, String>,
            limits: &mut Vec<serde_json::Value>,
        ) {
            if let Some(fields) = value.as_object() {
                for (name, value) in fields {
                    let name = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}.{name}")
                    };
                    numeric_limits(value, &name, origins, limits);
                }
            } else if value.is_number() && !limits.iter().any(|limit| limit["name"] == prefix) {
                let units = if prefix.contains("bytes") {
                    "bytes"
                } else if prefix.ends_with("seconds") {
                    "seconds"
                } else {
                    "count"
                };
                limits.push(serde_json::json!({"name":prefix,"value":value,"units":units,"scope":"deployment policy","window_seconds":null,"origin":origins.get(prefix).map(String::as_str).unwrap_or("built-in default")}));
            }
        }
        numeric_limits(
            &serde_json::to_value(self).unwrap_or_default(),
            "",
            &self.policy_origins,
            &mut limits,
        );
        limits
    }

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

    /// The persistence policy this configuration implies: the source ceiling
    /// an operator set, and the encoded and memory ceilings the collaboration
    /// storage format supports. One value, so admission, publication, append
    /// and compaction cannot drift apart.
    pub fn persistence(&self) -> PersistenceLimits {
        PersistenceLimits {
            max_source_bytes: self.max_document,
            ..self.persistence
        }
    }

    /// Overrides the document size ceiling, in megabytes. `None` leaves the
    /// default alone.
    ///
    /// The supported maximum is whatever [`PersistenceLimits::validate`]
    /// accepts for collaboration recovery and bounded transient memory.
    pub fn set_max_document(&mut self, megabytes: Option<usize>) -> Result<(), String> {
        let Some(megabytes) = megabytes else {
            return Ok(());
        };
        let bytes = megabytes
            .checked_mul(1024 * 1024)
            .ok_or_else(|| "--document-size-limit is too large".to_string())?;
        let candidate = PersistenceLimits {
            max_source_bytes: bytes,
            ..self.persistence
        };
        candidate
            .validate()
            .map_err(|why| format!("--document-size-limit: {why}"))?;
        self.max_document = bytes;
        Ok(())
    }

    /// Overrides what the figures of one document may come to, in megabytes.
    /// `None` leaves the default alone.
    ///
    /// Worth setting low on a deployment anybody may publish to. Figures are
    /// where the bytes of a paper actually go, and while they count against
    /// `--publisher-storage-limit` like everything else, this is what stops one document from
    /// spending a publisher's whole allowance on images.
    pub fn set_budget_document_assets(&mut self, megabytes: Option<usize>) -> Result<(), String> {
        let Some(megabytes) = megabytes else {
            return Ok(());
        };
        if !(1..=1024).contains(&megabytes) {
            return Err("--document-assets-limit must be between 1 and 1024 MiB".into());
        }
        self.max_assets = (megabytes * 1024 * 1024) as i64;
        // One figure may never be more than all of them.
        self.max_asset = self.max_asset.min(self.max_assets);
        Ok(())
    }

    /// Parse and apply the one daily transfer budget exposed by the ordinary
    /// command line. Bare integers are bytes; binary units are accepted for
    /// readable operator values such as `10GiB`. Omission is represented by
    /// `None` and remains unlimited, while an explicit zero is meaningful.
    pub fn set_budget_transfer(&mut self, value: Option<&str>) -> Result<(), String> {
        self.cost.transfer_bytes = value.map(parse_budget_transfer).transpose()?;
        Ok(())
    }

    /// Apply advanced, file-backed policy overrides without requiring callers
    /// to restate the complete policy. This is intentionally separate from the
    /// everyday CLI flags.
    pub fn apply_cost_overrides(&mut self, overrides: CostPolicyOverrides) {
        if let Ok(serde_json::Value::Object(values)) = serde_json::to_value(&overrides) {
            for (name, value) in values {
                if !value.is_null() {
                    self.policy_origins
                        .insert(format!("cost.{name}"), "configuration file".into());
                }
            }
        }
        if let Some(value) = overrides.transfer_bytes {
            self.cost.transfer_bytes = Some(value);
        }
        if let Some(value) = overrides.requests_per_minute {
            self.cost.requests_per_minute = value;
        }
        if let Some(value) = overrides.requests_per_network_minute {
            self.cost.requests_per_network_minute = value;
        }
        if let Some(value) = overrides.requests_per_principal_minute {
            self.cost.requests_per_principal_minute = value;
        }
        if let Some(value) = overrides.requests_per_document_minute {
            self.cost.requests_per_document_minute = value;
        }
        if let Some(value) = overrides.work_concurrency {
            self.cost.work_concurrency = value;
        }
        if let Some(value) = overrides.artifact_transfers {
            self.cost.artifact_transfers = value;
        }
        if let Some(value) = overrides.request_body_memory_bytes {
            self.cost.request_body_memory_bytes = value;
        }
    }

    /// Apply the optional operator-declared backup policy and record the
    /// configuration origin for every declared member.
    pub fn apply_backup_overrides(
        &mut self,
        overrides: BackupPolicyOverrides,
    ) -> Result<(), String> {
        if let Ok(serde_json::Value::Object(values)) = serde_json::to_value(&overrides) {
            for (name, value) in values {
                if !value.is_null() {
                    self.policy_origins
                        .insert(format!("backup.{name}"), "configuration file".into());
                }
            }
        }
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

    /// Overrides the history settings an operator has a reason to change: how
    /// long a document has to be quiet before a checkpoint is taken, in
    /// minutes, and how many checkpoints one document keeps. `None` leaves a
    /// default alone; `--history-limit 0` is "no history beyond the session state",
    /// which is expressed as a cap of one, since the newest checkpoint is
    /// never shed.
    pub fn set_history(
        &mut self,
        checkpoint_minutes: Option<usize>,
        keep: Option<usize>,
    ) -> Result<(), String> {
        if let Some(checkpoint_minutes) = checkpoint_minutes {
            self.session.checkpoint_seconds = (checkpoint_minutes * 60) as i64;
        }
        if let Some(keep) = keep {
            self.session.history_max = keep.max(1);
        }
        Ok(())
    }

    /// Overrides the per-publisher counts: how many documents one publisher may
    /// hold, and how many uploads they may make in an hour. `None` leaves a
    /// default alone.
    pub fn set_counts(
        &mut self,
        documents: Option<usize>,
        uploads_per_hour: Option<usize>,
    ) -> Result<(), String> {
        if let Some(documents) = documents {
            self.storage.documents_per_owner = documents;
        }
        if let Some(uploads_per_hour) = uploads_per_hour {
            self.storage.uploads_per_hour = uploads_per_hour;
        }
        Ok(())
    }
}

/// The source ceiling a deployment gets without saying otherwise.
pub const DEFAULT_MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;

/// The supported source ceiling, in bytes. `--document-size-limit` may not exceed it.
///
/// It is a policy, not a theorem: nothing proves that eight megabytes of text
/// can never encode past [`PersistenceLimits::max_encoded_snapshot_bytes`].
/// It is the largest source ceiling for which the snapshot copies counted by
/// [`SNAPSHOT_COPY_FACTOR`] still leave room for the CRDT history and
/// metadata that grow beside the visible text.
pub const SUPPORTED_MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;

/// `E`: the largest encoded CRDT snapshot this deployment will write.
pub const DEFAULT_MAX_ENCODED_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

/// `M`: the memory admission for the transient copies persistence makes
/// around one snapshot -- construction, staging, encoded segment bodies,
/// record fragments, and the overlap a compaction adds. It is not a storage
/// quota: nothing here is billed to an owner, and nothing here survives the
/// operation.
pub const DEFAULT_MAX_STAGING_BYTES: usize = 512 * 1024 * 1024;

/// How many live copies of one snapshot persistence may hold at its peak.
/// The inventory behind this number is the set of transient copies listed
/// under [`DEFAULT_MAX_STAGING_BYTES`]; changing the factor without
/// recounting them makes the memory admission a decoration.
pub const SNAPSHOT_COPY_FACTOR: usize = 8;

/// The relationship between visible source, encoded collaboration state, and
/// the transient memory needed to validate and compact that state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersistenceLimits {
    /// `S`: the configured source-byte ceiling, which is `max_document`.
    pub max_source_bytes: usize,
    /// `E`: the supported encoded CRDT snapshot ceiling.
    pub max_encoded_snapshot_bytes: usize,
    /// `M`: the memory admission for transient persistence copies.
    pub max_staging_bytes: usize,
}

impl Default for PersistenceLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: DEFAULT_MAX_SOURCE_BYTES,
            max_encoded_snapshot_bytes: DEFAULT_MAX_ENCODED_SNAPSHOT_BYTES,
            max_staging_bytes: DEFAULT_MAX_STAGING_BYTES,
        }
    }
}

impl PersistenceLimits {
    /// The peak transient memory one snapshot of `bytes` is admitted for.
    /// Saturating rather than checked: an estimate that overflows `usize` is
    /// past every budget anyway, and the caller compares it against one.
    pub fn staging_cost(bytes: usize) -> usize {
        bytes.saturating_mul(SNAPSHOT_COPY_FACTOR)
    }

    /// Refuse a configuration that cannot durably save the work it advertises.
    ///
    /// Every derived bound is checked arithmetic: an operator who sets a
    /// ceiling near `usize::MAX` gets a configuration error rather than a
    /// wrapped comparison that silently admits everything.
    pub fn validate(&self) -> Result<(), String> {
        if self.max_source_bytes == 0 {
            return Err("the document size ceiling must be positive".into());
        }
        if self.max_source_bytes > SUPPORTED_MAX_SOURCE_BYTES {
            return Err(format!(
                "a document source ceiling of {} MB is not supported; the maximum is {} MB, \
                 because a larger source cannot be guaranteed to encode inside the {} MB \
                 collaboration snapshot ceiling this deployment can save",
                self.max_source_bytes >> 20,
                SUPPORTED_MAX_SOURCE_BYTES >> 20,
                self.max_encoded_snapshot_bytes >> 20,
            ));
        }
        if self.max_source_bytes > self.max_encoded_snapshot_bytes {
            return Err(format!(
                "the document source ceiling ({} MB) cannot exceed the encoded snapshot \
                 ceiling ({} MB)",
                self.max_source_bytes >> 20,
                self.max_encoded_snapshot_bytes >> 20,
            ));
        }
        if self.max_encoded_snapshot_bytes == 0 {
            return Err("the encoded snapshot ceiling must be positive".into());
        }
        let peak = self
            .max_encoded_snapshot_bytes
            .checked_mul(SNAPSHOT_COPY_FACTOR)
            .ok_or_else(|| "the snapshot memory estimate overflows".to_string())?;
        if peak > self.max_staging_bytes {
            return Err(format!(
                "the persistence memory budget ({} MB) cannot process one maximum snapshot, \
                 which peaks at {} MB",
                self.max_staging_bytes >> 20,
                peak >> 20,
            ));
        }
        Ok(())
    }
}

/// Why a proposed write can never be saved as this deployment is configured.
/// No retry helps; the work itself is past a ceiling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SizeRefusal {
    /// Past `S`: the visible source of the document.
    Source { bytes: usize, ceiling: usize },
    /// Past `E`: the encoded CRDT snapshot, which includes the history and
    /// metadata that grow independently of the visible source.
    Encoded { bytes: usize, ceiling: usize },
}

impl SizeRefusal {
    pub fn message(&self) -> String {
        match self {
            Self::Source { bytes, ceiling } => format!(
                "this document is {} MB of text, past the {} MB this deployment accepts",
                bytes >> 20,
                ceiling >> 20
            ),
            Self::Encoded { bytes, ceiling } => format!(
                "this document's saved state would be {} MB, past the {} MB this deployment \
                 can durably save; its edit history and metadata count towards that as well \
                 as its text",
                bytes >> 20,
                ceiling >> 20
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_body_memory_default_and_bounds_are_validated() {
        let mut config = Configuration::default();
        assert_eq!(
            config.cost.request_body_memory_bytes,
            DEFAULT_REQUEST_BODY_MEMORY_BYTES
        );
        assert!(config.cost.validate().is_ok());
        config.cost.request_body_memory_bytes = 0;
        assert!(config.cost.validate().is_err());
        if let Ok(value) = usize::try_from(u64::from(u32::MAX) + 1) {
            config.cost.request_body_memory_bytes = value;
            assert!(config.cost.validate().is_err());
        }
    }

    #[test]
    fn emergency_bytes_is_not_an_advanced_override() {
        let parsed = serde_yaml::from_str::<CostPolicyOverrides>("emergency_bytes: 1");
        assert!(parsed.is_err());
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
        assert_eq!(
            config.policy_origins["backup.warning_count"],
            "configuration file"
        );
        assert!(config
            .policy_limits()
            .iter()
            .any(|limit| limit["name"] == "backup.warning_count"
                && limit["origin"] == "configuration file"));
    }
}

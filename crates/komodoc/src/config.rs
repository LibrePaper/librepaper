//! Every rule the deployment enforces lives here, and only here. The shell
//! gets these values injected into its source in place of `__CONFIG__`; the
//! server reads them directly. A limit changed here changes everywhere on the
//! next build.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// Which catalogue driver backs a deployment.
///
/// The product code should not branch on this value; it is configuration for
/// selecting the driver at startup. Keeping it typed prevents a partially
/// filled hosted configuration from being mistaken for a local deployment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeploymentProfile {
    Local,
    Hosted,
}

/// Where pure catalogue reads are served from in a hosted deployment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogReads {
    Replica,
    Primary,
}

impl Default for CatalogReads {
    fn default() -> Self {
        Self::Replica
    }
}

impl std::str::FromStr for CatalogReads {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "replica" => Ok(Self::Replica),
            "primary" => Ok(Self::Primary),
            _ => Err(format!(
                "--catalog-reads must be either replica or primary, not {value:?}"
            )),
        }
    }
}

impl std::fmt::Display for CatalogReads {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Replica => "replica",
            Self::Primary => "primary",
        })
    }
}

/// Private disposable state used by the server process. Hosted deployments
/// must supply this explicitly and with an absolute path; local deployments
/// derive it from their deployment directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentPaths {
    pub deployment: Option<PathBuf>,
    pub catalog: Option<PathBuf>,
    pub objects: Option<PathBuf>,
    pub state: PathBuf,
    pub secrets: Option<PathBuf>,
    pub writer_lock: PathBuf,
    pub replica: Option<PathBuf>,
}

impl DeploymentPaths {
    pub fn local(deployment: impl Into<PathBuf>) -> Self {
        let deployment = deployment.into();
        let state = deployment.join("state");
        Self {
            catalog: Some(deployment.join("catalog.db")),
            objects: Some(deployment.join("objects")),
            secrets: Some(deployment.join("secrets")),
            writer_lock: state.join("writer.lock"),
            state,
            deployment: Some(deployment),
            replica: None,
        }
    }

    pub fn hosted(state: impl Into<PathBuf>) -> Self {
        let state = state.into();
        Self {
            replica: Some(state.join("catalog-replica.db")),
            writer_lock: state.join("writer.lock"),
            state,
            deployment: None,
            catalog: None,
            objects: None,
            secrets: None,
        }
    }

    /// Validate the private-state boundary before any driver opens a file.
    pub fn validate(&self, profile: DeploymentProfile) -> Result<(), String> {
        if !self.state.is_absolute() {
            return Err(format!(
                "server state path must be absolute: {}",
                self.state.display()
            ));
        }
        if profile == DeploymentProfile::Local && self.deployment.is_none() {
            return Err("local deployments need a deployment directory".into());
        }
        if profile == DeploymentProfile::Hosted && self.deployment.is_some() {
            return Err("hosted deployments cannot use a local deployment directory".into());
        }
        Ok(())
    }

    /// The hosted replica may be overridden only inside the private state
    /// directory. This prevents selecting a second path to evade the writer
    /// lock or accidentally putting catalogue data in a shared location.
    pub fn replica_path(&self, override_path: Option<&Path>) -> Result<PathBuf, String> {
        let Some(default) = self.replica.as_ref() else {
            return Err("catalogue replicas are only available in hosted mode".into());
        };
        let Some(path) = override_path else {
            return Ok(default.clone());
        };
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            return Err("catalogue replica path must be absolute".into());
        };
        if !path.starts_with(&self.state) {
            return Err("catalogue replica must be below --server-state".into());
        }
        Ok(path)
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
    /// How long an asset nothing refers to is kept before it is pruned. It
    /// exists because uploading and naming are two requests: the bytes are
    /// stored, and the digest is written into the shared document a moment
    /// later. An asset pruned in that moment would be one somebody had just
    /// successfully uploaded.
    pub asset_grace: i64,
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
    /// The most checkpoints one document keeps. Zero is no cap.
    pub history_max: usize,
    /// A state larger than this is fetched over HTTP instead of being sent
    /// down the socket, so one cold join of a large document does not sit in a
    /// text frame.
    pub inline_state_max: usize,
    /// How many documents may be held in memory at once. The least recently
    /// used idle room is persisted and evicted past this.
    pub rooms_max: usize,
    /// How many frames may be queued for one socket before it is disconnected.
    /// A peer that cannot keep up is resynchronised on reconnect, which costs
    /// one state transfer and bounds what a slow reader can make the server
    /// hold.
    pub peer_queue: usize,
    /// How many document updates one socket may send in a minute.
    pub updates_per_minute: i64,
}

/// A checkpoint asked for within this many seconds of the last one waits until
/// they have passed. A burst of saves is one mark in the timeline, and a sync
/// client writing all day is two marks a minute at most. A constant rather
/// than a flag, as `docs/specs/history.md` says.
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
            max_document: 4 * 1024 * 1024,
            max_files: 200,
            max_path: 200,
            max_assets: 32 * 1024 * 1024,
            max_asset: 8 * 1024 * 1024,
            asset_grace: 3600,
            text_extensions: [
                ".tex",
                ".typ",
                ".md",
                ".markdown",
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
            max_annotations: 256 * 1024,
            // What the upload form takes. Every one of these is a source
            // format `document_format` names and `storable_source` allows, so
            // the list a person is shown and the list the publish route
            // accepts are the same list. A `.typ` or a `.tex` dropped on its
            // own is a document like any other -- one that reaches its
            // figures and its bibliography only if it came with them, which
            // is what `publish <directory>` is for.
            extensions: [".html", ".htm", ".md", ".markdown", ".typ", ".tex"]
                .map(String::from)
                .to_vec(),
            // HTML is a source format like the other two, and its renderer is
            // the identity: there is no longer a document without a source.
            // LaTeX is one with no renderer on this side at all -- the store
            // keeps the source, and a browser that has fetched a distribution
            // is the only thing anywhere that can make pages of it. Which is
            // why `storable_source` and `renderers` are two questions.
            source_formats: ["markdown", "typst", "html", "latex"]
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
                checkpoint_seconds: 5 * 60,
                history_max: 0,
                inline_state_max: 256 * 1024,
                rooms_max: 200,
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

    /// Overrides the document size ceiling, in megabytes. Zero leaves the
    /// default alone.
    pub fn set_max_document(&mut self, megabytes: usize) -> Result<(), String> {
        if megabytes == 0 {
            return Ok(());
        }
        if !(1..=100).contains(&megabytes) {
            return Err("--max-size must be between 1 and 100 MB".into());
        }
        self.max_document = megabytes * 1024 * 1024;
        Ok(())
    }

    /// Overrides what the figures of one document may come to, in megabytes.
    /// Zero leaves the default alone.
    ///
    /// Worth setting low on a deployment anybody may publish to. Figures are
    /// where the bytes of a paper actually go, and while they count against
    /// `--quota` like everything else, this is what stops one document from
    /// spending a publisher's whole allowance on images.
    pub fn set_max_assets(&mut self, megabytes: i64) -> Result<(), String> {
        if megabytes == 0 {
            return Ok(());
        }
        if !(1..=1024).contains(&megabytes) {
            return Err("--max-assets must be between 1 and 1024 MB".into());
        }
        self.max_assets = megabytes * 1024 * 1024;
        // One figure may never be more than all of them.
        self.max_asset = self.max_asset.min(self.max_assets);
        Ok(())
    }

    /// Overrides the storage ceilings, in megabytes: how much one publisher may
    /// hold across all their documents, and how much the whole deployment will
    /// hold. Zero leaves a default alone.
    pub fn set_storage(&mut self, quota_mb: i64, total_mb: i64) -> Result<(), String> {
        if quota_mb < 0 || total_mb < 0 {
            return Err("--quota and --storage must be positive".into());
        }
        if quota_mb > 0 {
            self.storage.per_owner = quota_mb * 1024 * 1024;
        }
        if total_mb > 0 {
            self.storage.total = total_mb * 1024 * 1024;
        }
        if self.storage.per_owner > self.storage.total {
            return Err(format!(
                "--quota ({} MB) cannot exceed --storage ({} MB)",
                self.storage.per_owner >> 20,
                self.storage.total >> 20
            ));
        }
        Ok(())
    }

    /// Overrides the history settings an operator has a reason to change: how
    /// long a document has to be quiet before a checkpoint is taken, in
    /// minutes, and how many checkpoints one document keeps. Zero leaves a
    /// default alone; `--history 0` is "no history beyond the session state",
    /// which is expressed as a cap of one, since the newest checkpoint is
    /// never shed.
    pub fn set_history(
        &mut self,
        checkpoint_minutes: i64,
        keep: Option<usize>,
    ) -> Result<(), String> {
        if checkpoint_minutes < 0 {
            return Err("--checkpoint must be positive".into());
        }
        if checkpoint_minutes > 0 {
            self.session.checkpoint_seconds = checkpoint_minutes * 60;
        }
        if let Some(keep) = keep {
            self.session.history_max = keep.max(1);
        }
        Ok(())
    }

    /// Overrides the per-publisher counts: how many documents one publisher may
    /// hold, and how many uploads they may make in an hour. Zero leaves a
    /// default alone.
    pub fn set_counts(&mut self, documents: i64, uploads_per_hour: i64) -> Result<(), String> {
        if documents < 0 || uploads_per_hour < 0 {
            return Err("--max-documents and --uploads-per-hour must be positive".into());
        }
        if documents > 0 {
            self.storage.documents_per_owner = documents as usize;
        }
        if uploads_per_hour > 0 {
            self.storage.uploads_per_hour = uploads_per_hour as usize;
        }
        Ok(())
    }
}

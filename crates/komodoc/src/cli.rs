//! The command line: publishing, listing, opening, signing in. Every command
//! here talks to a deployment over HTTP, the way a browser does.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Configuration;
use crate::http::{
    detail_of, get_as, get_json, get_with_token, post_directory, post_json, post_json_as,
    put_current_bytes, text, Credentials,
};
use crate::render::{
    counted, is_html, is_markdown, is_typst, pdf_of, read_and_note, read_and_note_from_files,
    report, title_from_html, title_from_markdown, title_from_typst,
};
use crate::util::{die, is_terminal_stdin, is_terminal_stdout, read_line};

pub fn server_from(flag: &str) -> String {
    let mut server = flag.to_string();
    if server.is_empty() {
        server = std::env::var("KOMODOC_SERVER").unwrap_or_default();
    }
    if server.is_empty() {
        die("set --server or $KOMODOC_SERVER");
    }
    server.trim_end_matches('/').to_string()
}

/* --------------------------------------------------------------- login */

// The CLI signs in through the deployment, not through a provider: it asks the
// server for a code, you open the URL it prints and approve there with
// whichever provider that deployment offers, and the token lands here. No
// callback URL and no local web server, so it works over SSH and on a machine
// with no browser of its own -- and adding a provider to a deployment adds it
// to `login` with no new flag and no new release of this binary.

/// The config directory to read and write under, following XDG. This is the
/// only place any of the token helpers below touches the environment, so
/// everything else can be pure and tested by handing it a base directory
/// directly rather than mutating `$HOME` or `$XDG_CONFIG_HOME` for the whole
/// process.
pub(crate) fn config_home() -> PathBuf {
    match std::env::var("XDG_CONFIG_HOME") {
        Ok(base) if !base.is_empty() => PathBuf::from(base),
        _ => {
            let home = std::env::var("HOME")
                .ok()
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| die("no home directory to store the token in"));
            Path::new(&home).join(".config")
        }
    }
}

/// Where every komodoc file lives under the config directory.
fn komodoc_dir(base: &Path) -> PathBuf {
    base.join("komodoc")
}

/// The single unscoped file `login` used to write before tokens were cached
/// per deployment. It is honoured only for the default server -- see
/// `stored_token_at` -- and removed once its token has been migrated into the
/// scoped cache, so it either holds the default server's token or does not
/// exist.
pub(crate) fn legacy_token_path(base: &Path) -> PathBuf {
    komodoc_dir(base).join("token")
}

/// One JSON object mapping a normalized server origin to the bearer token
/// `login` received from it. Scoped by origin, not by the literal `--server`
/// string, so `https://x.example` and `https://x.example/` share a cache
/// entry and a request never carries one deployment's token to another.
pub(crate) fn tokens_path(base: &Path) -> PathBuf {
    komodoc_dir(base).join("tokens.json")
}

/// Normalizes a server into the origin its cached token is filed under:
/// scheme, host and port, with the scheme's default port made explicit so an
/// address with and without an explicit `:443` resolve to the same entry. A
/// string that does not parse as a URL is lowercased and trimmed instead of
/// failing -- every value handed to `--server` needs a cache key, valid URL
/// or not.
fn origin_of(server: &str) -> String {
    match url::Url::parse(server) {
        Ok(url) if url.host_str().is_some() => format!(
            "{}://{}:{}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            url.port_or_known_default().unwrap_or(0)
        ),
        _ => server.trim().trim_end_matches('/').to_lowercase(),
    }
}

/// The default server: the one `server_from("")` resolves to. Used only to
/// decide whether the legacy unscoped token could belong to `server` --
/// never to choose a server for a command that was given one explicitly.
fn default_server() -> String {
    std::env::var("KOMODOC_SERVER")
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string()
}

/// All cached tokens, keyed by origin. A missing or unreadable file is the
/// same as no tokens cached yet, which is not worth failing a command over.
fn load_tokens(base: &Path) -> std::collections::HashMap<String, String> {
    std::fs::read_to_string(tokens_path(base))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_tokens(
    base: &Path,
    tokens: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    let path = tokens_path(base);
    let body = serde_json::to_string_pretty(tokens)
        .map_err(|err| format!("could not encode {}: {err}", path.display()))?;
    // `write_token` is the general "write this text where nobody else can
    // read it" primitive, not only the one `login` used to use for a lone
    // bearer string; a trailing newline on a JSON file is harmless.
    write_token(&path, &body)
}

/// The token cached for one server's origin, or "" if there is none. The
/// legacy unscoped file is consulted only when `server`'s origin is the
/// default server it predates -- it is never forwarded to a different
/// deployment, which is the bug this replaces.
pub(crate) fn stored_token_at(base: &Path, server: &str, default_server: &str) -> String {
    let origin = origin_of(server);
    if let Some(token) = load_tokens(base).get(&origin) {
        let trimmed = token.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if origin == origin_of(default_server) {
        if let Ok(raw) = std::fs::read_to_string(legacy_token_path(base)) {
            let trimmed = raw.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }
    String::new()
}

/// Caches `token` under `server`'s origin. When that origin is the default
/// server, the legacy unscoped file -- the only thing that could have been
/// caching a token for it before -- is migrated away: its content, if any, is
/// now superseded by this scoped entry, and leaving it in place would let a
/// stale copy resurface if the scoped cache were ever cleared.
pub(crate) fn store_token_at(
    base: &Path,
    server: &str,
    default_server: &str,
    token: &str,
) -> Result<(), String> {
    let origin = origin_of(server);
    let mut tokens = load_tokens(base);
    tokens.insert(origin.clone(), token.to_string());
    save_tokens(base, &tokens)?;
    if origin == origin_of(default_server) {
        let legacy = legacy_token_path(base);
        if legacy.exists() {
            std::fs::remove_file(&legacy)
                .map_err(|err| format!("could not remove {}: {err}", legacy.display()))?;
        }
    }
    Ok(())
}

/// The token to send to `server`: `$KOMODOC_TOKEN` if it is set, or whatever
/// `komodoc login` cached for that server's own origin.
///
/// `KOMODOC_TOKEN` is explicit and is sent to whichever server was selected,
/// wherever that is -- it is a bearer a shell script hands the CLI on
/// purpose, and there is nothing here to scope it against. The cache, by
/// contrast, is scoped to the server's origin precisely so that signing in to
/// one deployment never sends its token to another. A `KOMODOC_TOKEN` holding
/// a GitHub token still works: the server tells the two apart by the `kmd_`
/// prefix and verifies each its own way.
pub fn stored_token_for(server: &str) -> String {
    let env_token = std::env::var("KOMODOC_TOKEN").ok();
    stored_token_with(
        &config_home(),
        server,
        &default_server(),
        env_token.as_deref(),
    )
}

/// The pure core of `stored_token_for`: everything above it does is read the
/// environment and the config directory, which is factored out here so the
/// precedence between an explicit `KOMODOC_TOKEN` and the scoped cache can be
/// tested by passing values in, rather than by mutating the process
/// environment a test binary's threads share.
pub(crate) fn stored_token_with(
    base: &Path,
    server: &str,
    default_server: &str,
    env_token: Option<&str>,
) -> String {
    if let Some(token) = env_token {
        if !token.trim().is_empty() {
            return token.trim().to_string();
        }
    }
    stored_token_at(base, server, default_server)
}

/// What every command that writes needs.
pub fn require_token_for(server: &str) -> String {
    let token = stored_token_for(server);
    if token.is_empty() {
        die("not signed in. Run:\n    komodoc login");
    }
    token
}

pub async fn login(server_flag: String) {
    let server = server_from(&server_flag);
    let code = request_device_code(&server)
        .await
        .unwrap_or_else(|err| die(format!("could not start the sign-in: {err}")));
    eprintln!(
        "\n  Open {}\n  and enter the code:  {}\n",
        code.verification_url, code.user_code
    );
    eprint!("  waiting for you to approve it");

    let token = poll_for_token(&server, &code).await;
    eprintln!();
    let token = token.unwrap_or_else(|err| die(err));

    // Who the token says you are. It is this deployment's own token, so the
    // deployment is the only thing that can answer, and it costs one call.
    let who =
        match get_with_token(&format!("{server}/api/me"), &token, Duration::from_secs(30)).await {
            Ok((200, payload)) => text(&payload, "name"),
            _ => String::new(),
        };

    let base = config_home();
    store_token_at(&base, &server, &default_server(), &token).unwrap_or_else(|err| die(err));
    println!("signed in as {who}");
    eprintln!("  token stored in {}", tokens_path(&base).display());
}

/// Writes bytes to a path only its owner can read, from the moment the file
/// is created -- there is no window where a broader mode briefly applies --
/// and fixes the permissions of a file that already existed under a looser
/// one. A bearer token's file permissions are the whole of its protection at
/// rest, so a failure to set them is reported rather than swallowed.
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|err| format!("could not create {}: {err}", path.display()))?;
    use std::io::Write;
    file.write_all(bytes)
        .map_err(|err| format!("could not write {}: {err}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("could not set permissions on {}: {err}", path.display()))?;
    }
    Ok(())
}

/// Writes the token where the next command will look for it, readable by
/// nobody else. Kept as a thin wrapper over `write_private_file` because
/// tests write a token to an arbitrary path directly, without going through
/// `login`'s scoped cache.
pub fn write_token(path: &Path, token: &str) -> Result<(), String> {
    write_private_file(path, format!("{token}\n").as_bytes())
}

/// Signs out of every cached deployment at once: `logout` takes no `--server`
/// of its own, so there is no single origin to clear selectively.
pub fn logout() {
    let base = config_home();
    let cleared_scoped = std::fs::remove_file(tokens_path(&base)).is_ok();
    let legacy = legacy_token_path(&base);
    let cleared_legacy = match std::fs::remove_file(&legacy) {
        Ok(()) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => die(format!("could not remove {}: {err}", legacy.display())),
    };
    if cleared_scoped || cleared_legacy {
        println!("signed out");
    } else {
        println!("not signed in");
    }
}

pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

pub async fn request_device_code(server: &str) -> Result<DeviceCode, String> {
    // No bearer: the terminal has no token yet, which is the whole reason it
    // is asking.
    let (status, payload) = post_json(
        &format!("{server}/api/auth/device"),
        &json!({}),
        "",
        Duration::from_secs(30),
    )
    .await?;
    let device_code = text(&payload, "device_code");
    if device_code.is_empty() {
        return Err(format!(
            "{server} returned {status}: {}",
            detail_of(&payload)
        ));
    }
    Ok(DeviceCode {
        device_code,
        user_code: text(&payload, "user_code"),
        verification_url: text(&payload, "verification_url"),
        expires_in: payload
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(600),
        interval: payload
            .get("interval")
            .and_then(Value::as_u64)
            .filter(|i| *i > 0)
            .unwrap_or(5),
    })
}

/// Waits for the code to be approved, at the interval the server asks for and
/// no faster. The deadline is the server's own expiry, so a code the server
/// has already forgotten is not polled for after it says so.
pub async fn poll_for_token(server: &str, code: &DeviceCode) -> Result<String, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(code.expires_in.max(60));
    let interval = Duration::from_secs(code.interval);
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(interval).await;
        eprint!(".");
        let Ok((_, reply)) = post_json(
            &format!("{server}/api/auth/device/token"),
            &json!({"device_code": code.device_code}),
            "",
            Duration::from_secs(30),
        )
        .await
        else {
            continue;
        };
        let token = text(&reply, "token");
        if !token.is_empty() {
            return Ok(token);
        }
        match text(&reply, "error").as_str() {
            "authorization_pending" | "" => {}
            "expired_token" => return Err("the code expired before it was approved".into()),
            other => return Err(format!("the server said: {other}")),
        }
    }
    Err("the code expired before it was approved".into())
}

/* ------------------------------------------------------------- publish */

/// The files of a directory, as the document will know them: relative paths,
/// `/`-separated, with what does not belong left behind.
///
/// Three things are skipped, and each for its own reason. A name beginning
/// with a dot is not part of a document -- `.git`, `.DS_Store`, an editor's
/// swap file. The main file's own output is what a compiler wrote, and a
/// document keeps what a person wrote. And whatever git ignores is the
/// author's own statement of what is derived, which is a better list than any
/// this could invent.
///
/// A subdirectory is followed even when it is a symlink -- it is the
/// author's own directory, and refusing to follow it would be a stranger
/// surprise than following it. But each is followed at most once by its
/// resolved (canonicalized) location, so a symlink cycle terminates instead
/// of being walked forever, and one that resolves outside `root` is left out
/// and named on stderr instead of silently published: uploading what an
/// author expected to be private is worse than uploading it and saying so.
pub fn files_under(root: &Path, main_stem: &str, ignored: &dyn Fn(&Path) -> bool) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(canonical_root) = root.canonicalize() else {
        // A root that cannot be resolved (does not exist, a broken symlink)
        // has nothing under it worth walking.
        return found;
    };
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    visited.insert(canonical_root.clone());
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                match path.canonicalize() {
                    Ok(resolved) if resolved.starts_with(&canonical_root) => {
                        // `insert` returns false for a location already
                        // visited -- an ancestor symlink cycle, or two links
                        // to the same place -- which is exactly when this
                        // must not be walked again.
                        if visited.insert(resolved) {
                            stack.push(path);
                        }
                    }
                    Ok(_) => {
                        eprintln!(
                            "note: skipping {} (a symlink resolving outside {})",
                            path.display(),
                            root.display()
                        );
                    }
                    // A broken symlink resolves to nothing worth walking.
                    Err(_) => {}
                }
                continue;
            }
            // The output of the document itself. A `paper.pdf` beside
            // `paper.typ` is what the last compile produced, and uploading it
            // would put a derived file in a store that keeps sources.
            if !main_stem.is_empty() && name == format!("{main_stem}.pdf") {
                continue;
            }
            if ignored(&path) {
                continue;
            }
            if let Ok(relative) = path.strip_prefix(root) {
                let mut at = String::new();
                for part in relative.components() {
                    if let std::path::Component::Normal(piece) = part {
                        if !at.is_empty() {
                            at.push('/');
                        }
                        at.push_str(&piece.to_string_lossy());
                    }
                }
                if !at.is_empty() {
                    found.push(at);
                }
            }
        }
    }
    found.sort();
    found
}

/// What git ignores under this directory, asked of git rather than worked out
/// from `.gitignore` -- which has precedence rules, nested files and a global
/// config, and reimplementing them would be a way to disagree with git rather
/// than to agree with it.
///
/// A directory that is not in a working tree, or a machine with no git,
/// ignores nothing and says so: silently uploading what an author expected to
/// be skipped is worse than uploading it and telling them.
pub fn git_ignores(root: &Path) -> Box<dyn Fn(&Path) -> bool> {
    let inside = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output();
    let usable = matches!(&inside, Ok(out) if out.status.success());
    if !usable {
        if inside.is_err() {
            eprintln!("note: git is not on the path, so nothing is skipped as ignored");
        }
        return Box::new(|_| false);
    }
    let root = root.to_path_buf();
    Box::new(move |path: &Path| {
        // `git -C root` runs with `root` as its working directory, so the
        // path handed to `check-ignore` must be relative to `root` too --
        // otherwise, with a relative root such as `paper`, a root-prefixed
        // path like `paper/private.txt` is resolved against that working
        // directory as `paper/paper/private.txt`, and a root-anchored
        // pattern such as `/private.txt` never matches. `--` ends option
        // parsing first, so a filename beginning with `-` is not read as a
        // flag.
        let relative = path.strip_prefix(&root).unwrap_or(path);
        std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["check-ignore", "-q", "--"])
            .arg(relative)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

/// Which file is the document. Named with `--main`, or worked out: the one
/// text at the top level whose extension is a format this renders, or
/// `main.*`. Anything else is a refusal that lists what it was choosing
/// between, because guessing wrong here publishes the wrong document.
pub fn main_file(files: &[String], asked: &str) -> Result<String, String> {
    if !asked.is_empty() {
        let wanted = crate::paths::normalise(asked);
        if !files.contains(&wanted) {
            return Err(format!("--main {asked} is not a file in that directory"));
        }
        return Ok(wanted);
    }
    let top: Vec<&String> = files
        .iter()
        .filter(|path| !path.contains('/'))
        .filter(|path| crate::render::document_format(path).is_some())
        .collect();
    if top.len() == 1 {
        return Ok(top[0].clone());
    }
    if let Some(named) = top.iter().find(|path| {
        Path::new(path)
            .file_stem()
            .is_some_and(|stem| stem.eq_ignore_ascii_case("main"))
    }) {
        return Ok((*named).clone());
    }
    if top.is_empty() {
        return Err("that directory holds no document to publish".into());
    }
    Err(format!(
        "several files could be the document; name one with --main:\n  {}",
        top.iter()
            .map(|path| path.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    ))
}

pub async fn publish(file: &str, title: String, slug: String, server_flag: String, main: String) {
    let path = Path::new(file);
    let Ok(info) = std::fs::metadata(path) else {
        die(format!("file not found: {file}"))
    };
    if info.is_dir() {
        return publish_directory(path, title, slug, server_flag, main).await;
    }
    if !main.is_empty() {
        die("--main names a file inside a directory; publish the directory to use it");
    }
    publish_file(file, title, slug, server_flag).await
}

/// A directory, as one document: every file in it that belongs, with one of
/// them named as the document itself.
async fn publish_directory(
    root: &Path,
    mut title: String,
    slug: String,
    server_flag: String,
    main: String,
) {
    let config = Configuration::default();
    let rules = config.paths();
    // The main file first, since what it is called decides what is skipped as
    // its output. Worked out from the whole listing, so `--main` can name a
    // file the rules would otherwise have to be asked about twice.
    let listed = files_under(root, "", &git_ignores(root));
    let main = main_file(&listed, &main).unwrap_or_else(|why| die(why));
    let stem = Path::new(&main)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let paths = files_under(root, &stem, &git_ignores(root));

    // Every path checked before anything is read, so a refusal names the file
    // rather than arriving after a megabyte has been sent.
    let mut texts = 0usize;
    let mut figures = 0i64;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for at in &paths {
        let kind = match crate::paths::check(&rules, at) {
            Ok(kind) => kind,
            // A file the rules refuse is said and skipped rather than fatal: a
            // directory usually has something in it that is not part of the
            // document, and refusing the whole publish over a stray file would
            // be unhelpful.
            Err(why) => {
                eprintln!("skipping {why}");
                continue;
            }
        };
        let bytes = std::fs::read(root.join(at))
            .unwrap_or_else(|err| die(format!("could not read {at}: {err}")));
        match kind {
            crate::paths::Kind::Text => {
                if std::str::from_utf8(&bytes).is_err() {
                    eprintln!("skipping {at}: it is not valid UTF-8 text");
                    continue;
                }
                texts += bytes.len();
            }
            crate::paths::Kind::Asset => {
                if bytes.len() as i64 > config.max_asset {
                    die(format!(
                        "{at} is larger than the {} MB one figure may be",
                        config.max_asset >> 20
                    ));
                }
                figures += bytes.len() as i64;
            }
        }
        files.push((at.clone(), bytes));
    }
    if texts > config.max_document {
        die(format!(
            "the texts of that directory come to more than the {} MB a document may be",
            config.max_document / (1024 * 1024)
        ));
    }
    if figures > config.max_assets {
        die(format!(
            "the figures of that directory come to more than the {} MB a document may keep",
            config.max_assets >> 20
        ));
    }
    if files.len() > config.max_files {
        die(format!(
            "that directory holds {} files; a document may hold {}",
            files.len(),
            config.max_files
        ));
    }

    // The document compiles, or nothing is sent: `publish` exists to make a
    // document readable, and one that does not compile is not one. It is
    // compiled against the directory it will be stored as, so an import that
    // works here works there.
    let source = files
        .iter()
        .find(|(at, _)| *at == main)
        .map(|(_, bytes)| String::from_utf8_lossy(bytes).to_string())
        .unwrap_or_default();
    // What the document is written in follows from the main file's name, and
    // the server works it out again from the same name -- so this is only for
    // the title and, for typst, for the compile that says whether the document
    // is one a reader will be able to render.
    let mut typst_pdf = None;
    let mut typst_dependencies = Vec::new();
    let mut typst_inputs = String::new();
    if is_typst(&main) {
        if title.is_empty() {
            title = title_from_typst(&source);
        }
        let (compiled, dependencies) =
            read_and_note_from_files(&main, &source, &title_or(&title, &main), &files);
        typst_dependencies = dependencies;
        typst_inputs = input_digest_for_typst(&main, &files, &rules);
        report(&compiled.diagnostics, &main);
        if compiled.output.is_none() {
            die(format!(
                "{main} did not compile ({})",
                counted(compiled.errors().count().max(1), "error")
            ))
        }
        typst_pdf = pdf_of(&compiled);
    } else if title.is_empty() {
        // A format with no heading scan of its own is named by its file, which
        // is what `title_or` below does anyway. Better that than running an
        // HTML title scan over something that is not HTML.
        title = match crate::render::document_format(&main) {
            Some("markdown") => title_from_markdown(&source),
            Some("html") => title_from_html(&source),
            Some("latex") => crate::render::title_from_latex(&source),
            _ => String::new(),
        };
    }
    if title.is_empty() {
        title = title_or("", &main);
    }

    eprintln!(
        "publishing {} ({}, {} KiB of text{})",
        main,
        counted(files.len(), "file"),
        texts / 1024,
        if figures == 0 {
            String::new()
        } else {
            format!(", {} KiB of figures", figures / 1024)
        }
    );

    let server = server_from(&server_flag);
    let uploaded_paths: std::collections::HashSet<String> = files
        .iter()
        .map(|(path, _)| crate::paths::normalise(path))
        .collect();
    let (status, document) = post_directory(
        &format!("{server}/api/documents"),
        &title,
        &slug,
        &main,
        files,
        &stored_token_for(&server),
        Duration::from_secs(600),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 201 {
        die(format!(
            "upload failed ({status}): {}",
            detail_of(&document)
        ));
    }
    report_published(&server, &document, &format!("{}", root.display()));
    if let Some(pdf) = typst_pdf {
        let missing: Vec<String> = typst_dependencies
            .iter()
            .filter(|path| !uploaded_paths.contains(&crate::paths::normalise(path)))
            .cloned()
            .collect();
        if missing.is_empty() {
            upload_typst_pdf(&server, &document, pdf, &typst_inputs).await;
        } else {
            eprintln!(
                "warning: source published, but no PDF artifact was uploaded; the local compile read files not in the published tree: {}",
                missing.join(", ")
            );
        }
    }
}

/// What `publish` prints: the read link when the document has one, since
/// that is the thing to send, and the bare URL otherwise -- which opens for
/// the owner alone, and the note says so. The note goes to stderr so the
/// link stays alone on stdout for a pipe.
fn report_published(server: &str, document: &Value, path: &str) {
    let share = text(document, "share_url");
    let slug = text(document, "slug");
    if share.is_empty() {
        println!("{server}{}", text(document, "url"));
    } else {
        println!("{server}{share}");
    }
    if !is_terminal_stdout() {
        return;
    }
    if share.is_empty() {
        eprintln!(
            "\nThis link opens for you alone; the document has no read link.\n\
             To mint one:\n  komodoc share {slug} --link read"
        );
    } else {
        eprintln!(
            "\nShare this link; anyone with it can read, no account needed.\n\
             For a link that also lets them comment:\n  komodoc share {slug} --link comment"
        );
    }
    eprintln!(
        "To publish a revision to the same document:\n  komodoc publish {path} --slug {slug}"
    );
}

async fn publish_file(file: &str, mut title: String, slug: String, server_flag: String) {
    let path = Path::new(file);
    let base_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    // What is stored is always HTML: the reader frames the document and
    // anchors comments into its text nodes. Markdown and typst are rendered
    // here, before they are uploaded.
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if extension != "html" && extension != "htm" && crate::render::document_format(file).is_none() {
        die(format!(
            "{base_name} is not a document Komodoc can serve.\n\n  \
             It takes HTML, markdown, typst or LaTeX.\n  \
             From Quarto:\n    quarto render paper.qmd --to html -M embed-resources:true"
        ));
    }
    let raw =
        std::fs::read(path).unwrap_or_else(|err| die(format!("could not read {file}: {err}")));
    let config = Configuration::default();
    if raw.len() > config.max_document {
        die(format!(
            "document exceeds the {} MB limit",
            config.max_document / (1024 * 1024)
        ));
    }
    let Ok(html) = String::from_utf8(raw.clone()) else {
        die(format!("{base_name} is not valid UTF-8 text"))
    };
    // Kept beside the rendered document so it can be reopened in the editor.
    // Every format komodoc renders has one, HTML included: its renderer is the
    // identity, so an HTML document's source is the HTML it was published as.
    let (mut source, mut source_format) = (String::new(), String::new());
    let mut typst_pdf = None;
    let mut typst_inputs = String::new();

    if is_typst(file) {
        if title.is_empty() {
            // The first heading names the document, before the filename does.
            title = title_from_typst(&html);
        }
        // A one-file publish is stored under the canonical `main.typ` path.
        // Compile that exact tree so a source that refers to its own filename
        // cannot produce a PDF for a different input than the server stores.
        let canonical_main = crate::room::main_path_for("", "typst");
        let (original, discovered) = read_and_note(path, &html, &title_or(&title, file));
        let (compiled, siblings) = if discovered.is_empty() {
            (
                read_and_note_from_files(
                    &canonical_main,
                    &html,
                    &title_or(&title, file),
                    &[(canonical_main.clone(), raw.clone())],
                )
                .0,
                discovered,
            )
        } else {
            (original, discovered)
        };
        // Every diagnostic, where it is, and nothing uploaded if any of them
        // is an error: `publish` exists to make a document readable, and a
        // document that does not compile is not one.
        report(&compiled.diagnostics, &base_name);
        // A document that read its siblings compiles here and nowhere else.
        // Published as one file it arrives without them, and the reader --
        // who renders it themselves -- gets the error the author never saw.
        // Said before the upload rather than discovered afterwards.
        if !siblings.is_empty() {
            let directory = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            eprintln!(
                "\n{base_name} reads {}; publish the directory to send them along:\n  \
                 komodoc publish {}",
                siblings.join(" and "),
                directory.display()
            );
        }
        let errors = compiled.errors().count();
        let warnings = compiled.warnings().count();
        if compiled.output.is_none() {
            die(format!(
                "{base_name} did not compile ({})",
                counted(errors.max(1), "error")
            ))
        }
        if siblings.is_empty() {
            typst_pdf = pdf_of(&compiled);
            let config = Configuration::default();
            typst_inputs = input_digest_for_typst(
                &canonical_main,
                &[(canonical_main.clone(), raw.clone())],
                &config.paths(),
            );
        } else {
            eprintln!(
                "warning: source will be published without a PDF artifact because the local compile read files not included in this one-file publish: {}",
                siblings.join(", ")
            );
        }
        eprintln!(
            "rendered {base_name} ({} KiB of typst{})",
            raw.len() / 1024,
            if warnings == 0 {
                String::new()
            } else {
                format!(", {}", counted(warnings, "warning"))
            }
        );
        source = html;
        source_format = "typst".to_string();
    } else if is_html(file) {
        if title.is_empty() {
            // What the document calls itself, which is what the landing page
            // has always shown and what the command line used to ignore.
            title = title_from_html(&html);
        }
        source = html.clone();
        source_format = "html".to_string();
    } else if is_markdown(file) {
        if title.is_empty() {
            title = title_from_markdown(&html);
        }
        eprintln!("read {base_name} ({} KiB of markdown)", raw.len() / 1024);
        source = html;
        source_format = "markdown".to_string();
    } else if crate::render::is_latex(file) {
        // Nothing is rendered here, and nothing can be: Komodoc carries no TeX
        // and no build embeds one. So this is the one format `publish` uploads
        // without having compiled it first, and the check that a document
        // compiles -- which is the reason `publish` compiles typst at all --
        // happens in the first browser that opens it instead. Said plainly,
        // because an author used to `publish` refusing a broken paper should
        // not have to infer that this one is different.
        if title.is_empty() {
            title = crate::render::title_from_latex(&html);
        }
        eprintln!(
            "read {base_name} ({} KiB of LaTeX; not compiled here -- \
             the first browser to open it compiles it)",
            raw.len() / 1024
        );
        source = html;
        source_format = "latex".to_string();
    } else if !html.contains('<') {
        die(format!("{base_name} contains no HTML tags"));
    }

    let server = server_from(&server_flag);
    if title.is_empty() && !slug.is_empty() {
        // Publishing a revision: keep the title the document already has
        // rather than silently renaming it after the file on disk.
        if let Ok((200, existing)) = get_json(
            &format!("{server}/api/documents/{slug}"),
            Duration::from_secs(30),
        )
        .await
        {
            title = text(&existing, "title");
        }
    }
    if title.is_empty() {
        title = title_or("", file);
    }

    // stored_token, not require_token: a deployment whose publishers are
    // "anyone" takes documents with no sign-in, and one that does need an
    // account answers with its own message.
    let (status, document) = post_json(
        &format!("{server}/api/documents"),
        // The source, and nothing rendered from it: the server stores the
        // document and every browser that shows it renders it. The compile
        // above still happens, because `publish` exists to make a document
        // readable and a document that does not compile is not one -- but what
        // it produces is a check, not a payload.
        &json!({"title": title, "slug": slug, "source": source, "source_format": source_format}),
        &stored_token_for(&server),
        Duration::from_secs(300),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 201 {
        die(format!(
            "upload failed ({status}): {}",
            detail_of(&document)
        ));
    }

    report_published(&server, &document, file);
    if let Some(pdf) = typst_pdf {
        // A one-file publish is allowed to remain source-only when the local
        // compile had imports beside it. The source is still useful, but the
        // PDF must never be attached to a server tree that does not contain
        // those inputs.
        if source_format == "typst" {
            upload_typst_pdf(&server, &document, pdf, &typst_inputs).await;
        }
    }
}

/// Uploads the native Typst PDF after the source tree has been accepted. The
/// server's `live` digest is authoritative: a revision may be checkpointed
/// lazily, and the response's historical `sha` can therefore lag the exact
/// tree the upload just installed.
async fn upload_typst_pdf(server: &str, document: &Value, pdf: Vec<u8>, expected_inputs: &str) {
    let slug = text(document, "slug");
    if slug.is_empty() {
        eprintln!("warning: source published, but its PDF artifact has no document slug");
        return;
    }
    let token = crate::cli::stored_token_for(server);
    let (status, latest) = match get_with_token(
        &format!("{server}/api/documents/{slug}/renderings/latest"),
        &token,
        Duration::from_secs(60),
    )
    .await
    {
        Ok(result) => result,
        Err(err) => {
            eprintln!(
                "warning: source published, but could not find its live tree for the PDF: {err}"
            );
            return;
        }
    };
    if status != 200 {
        eprintln!(
            "warning: source published, but could not find its live tree for the PDF: {}",
            detail_of(&latest)
        );
        return;
    }
    let sha = text(&latest, "live");
    if sha.is_empty() {
        eprintln!(
            "warning: source published, but the server returned no live tree digest for the PDF"
        );
        return;
    }
    if text(&latest, "inputs") != expected_inputs {
        eprintln!(
            "warning: source published, but its canonical file tree differs from the local Typst inputs; no PDF artifact was uploaded"
        );
        return;
    }
    let (status, reply) = match put_current_bytes(
        &format!("{server}/api/documents/{slug}/renderings/{sha}"),
        pdf,
        &token,
        "application/pdf",
        expected_inputs,
        Duration::from_secs(300),
    )
    .await
    {
        Ok(result) => result,
        Err(err) => {
            eprintln!("warning: source published, but PDF artifact upload failed: {err}");
            return;
        }
    };
    if status != 200 {
        eprintln!(
            "warning: source published, but PDF artifact upload failed ({}): {}\n  Retry by compiling the Typst source again and PUTting it to /api/documents/{slug}/renderings/{sha}",
            status,
            detail_of(&reply)
        );
    } else {
        eprintln!("uploaded Typst PDF artifact for tree {sha}");
    }
}

/// Builds the same source-input identity the server reports for a live room.
/// Yjs item ids are intentionally omitted: they identify an editing history,
/// while this digest identifies the exact main file and dependency bytes used
/// by the native compiler.
fn input_digest_for_typst(
    main: &str,
    files: &[(String, Vec<u8>)],
    rules: &crate::paths::Rules<'_>,
) -> String {
    let mut tree = crate::history::Tree {
        main: main.to_string(),
        files: std::collections::BTreeMap::new(),
        settings: None,
    };
    for (path, bytes) in files {
        let kind = match crate::paths::check(rules, path) {
            Ok(kind) => kind,
            Err(_) => continue,
        };
        let kind = match kind {
            crate::paths::Kind::Text => "text",
            crate::paths::Kind::Asset => "asset",
        };
        tree.files.insert(
            crate::paths::normalise(path),
            crate::history::TreeEntry {
                kind: kind.to_string(),
                id: String::new(),
                sha: crate::store::digest_of_bytes(bytes),
                size: bytes.len() as i64,
            },
        );
    }
    tree.input_digest()
}

/// Falls back to the filename, the way an untitled document is named.
pub fn title_or(title: &str, file: &str) -> String {
    if !title.trim().is_empty() {
        return title.to_string();
    }
    let stem = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    stem.replace(['_', '-'], " ").trim().to_string()
}

/* ------------------------------------------------------------ listing */

pub async fn list_documents(server_flag: String) {
    let server = server_from(&server_flag);
    let (status, payload) = post_json(
        &format!("{server}/api/list"),
        &json!({}),
        &require_token_for(&server),
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "listing failed ({status}): {}",
            detail_of(&payload)
        ));
    }
    let documents = payload
        .get("documents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if documents.is_empty() {
        println!("no documents yet");
        return;
    }
    let ids = short_ids(&documents, &Configuration::default());
    let width = ids.values().map(String::len).max().unwrap_or(0);
    for document in &documents {
        let mut updated = text(document, "updated_at");
        updated.truncate(10);
        let slug = text(document, "slug");
        // A document shared with you by name is in your list, and says what
        // you hold on it. Your own say nothing: everything here without a mark
        // is yours, which is what this listing has always meant.
        let role = text(document, "role");
        let held = if role == "owner" || role.is_empty() {
            String::new()
        } else {
            format!("  ({role})")
        };
        println!(
            "{:<width$}  {}  {}{held}",
            ids.get(&slug).cloned().unwrap_or_default(),
            updated,
            text(document, "title")
        );
    }
}

/// The shortest handle `list` will print. One character is unique today and
/// ambiguous after the next publish, and it reads as a typo rather than a
/// name; three is short enough to type and stable enough to keep in a note.
const SHORT_ID_MINIMUM: usize = 3;

/// Gives each listed document a short handle: a prefix of its generated random
/// suffix (or explicit slug). Every handle is cut to the same width -- ragged
/// ids are hard to read down a column and hard to remember -- which is the
/// longest prefix any one document needs to be unambiguous, and never fewer
/// than three characters.
pub fn short_ids(
    documents: &[Value],
    config: &Configuration,
) -> std::collections::HashMap<String, String> {
    let items: Vec<(String, String)> = documents
        .iter()
        .filter_map(|document| {
            let slug = document.get("slug")?.as_str()?.to_string();
            let last = slug.rsplit('-').next().unwrap_or(&slug).to_string();
            let key = if last.len() == config.suffix_length
                && last.chars().all(|c| config.suffix_alphabet.contains(c))
            {
                last
            } else {
                slug.clone()
            };
            Some((slug, key))
        })
        .collect();

    let mut width = SHORT_ID_MINIMUM;
    for (_, key) in &items {
        let mut needed = key.len();
        for length in 1..=key.len() {
            let prefix = &key[..length];
            if items
                .iter()
                .filter(|(_, other)| other.starts_with(prefix))
                .count()
                == 1
            {
                needed = length;
                break;
            }
        }
        width = width.max(needed);
    }

    items
        .into_iter()
        .map(|(slug, key)| {
            // A key shorter than the common width is used whole; it is already
            // as distinct as it will ever be.
            let id = if width < key.len() {
                key[..width].to_string()
            } else {
                key
            };
            (slug, id)
        })
        .collect()
}

/// Turns what the user typed -- a full slug, or one of the short handles
/// `list` prints -- into the slug the API knows.
pub async fn resolve_identifier(identifier: &str, server: &str, key: &str) -> String {
    // A full slug needs no listing: this is the path a command run from a
    // link someone sent takes, with the link's key as its credential, and
    // the path an owner's own slug takes with their sign-in.
    if let Ok((200, _)) = get_as(
        &format!("{server}/api/documents/{identifier}"),
        &Credentials::new(&stored_token_for(server), key),
        Duration::from_secs(30),
    )
    .await
    {
        return identifier.to_string();
    }
    let (status, payload) = post_json(
        &format!("{server}/api/list"),
        &json!({}),
        &require_token_for(server),
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "listing failed ({status}): {}",
            detail_of(&payload)
        ));
    }
    let documents = payload
        .get("documents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let ids = short_ids(&documents, &Configuration::default());
    let mut found = String::new();
    for document in &documents {
        let slug = text(document, "slug");
        if identifier == slug || ids.get(&slug).is_some_and(|id| id == identifier) {
            if !found.is_empty() {
                die(format!("{identifier:?} matches more than one document"));
            }
            found = slug;
        }
    }
    if found.is_empty() {
        die(format!("no visible document matches {identifier:?}"));
    }
    found
}

pub async fn comment_document(identifier: &str, server_flag: String, key: String) {
    let server = server_from(&server_flag);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key).await;
    // The key goes back where a browser expects it, in the fragment, so the
    // page opened is the link exactly as it was shared.
    if key.is_empty() {
        open_url(&format!("{server}/docs/{slug}"));
    } else {
        open_url(&format!("{server}/docs/{slug}#k={key}"));
    }
}

/// What `--key` takes: the key itself, or the whole link it came in, since a
/// link is what a person actually has in their clipboard. A URL with no key
/// in its fragment is an empty key, which is what a plain document URL
/// carries.
pub fn link_key(flag: &str) -> String {
    let flag = flag.trim();
    let Some((_, fragment)) = flag.split_once('#') else {
        return if flag.contains("://") {
            String::new()
        } else {
            flag.to_string()
        };
    };
    fragment
        .split('&')
        .find_map(|part| part.strip_prefix("k="))
        .map(|key| {
            url::form_urlencoded::parse(format!("k={key}").as_bytes())
                .next()
                .map(|(_, value)| value.to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// `komodoc edit` opens a document in the reader, with its source beside it.
/// The editor is part of the reader rather than a program of its own, so this
/// is what it should be: a way to get to the right page from a short id.
pub async fn edit_document(identifier: &str, server_flag: String, key: String) {
    comment_document(identifier, server_flag, key).await;
}

pub fn open_url(target: &str) {
    let (command, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![target])
    } else if cfg!(target_os = "windows") {
        ("rundll32", vec!["url.dll,FileProtocolHandler", target])
    } else {
        ("xdg-open", vec![target])
    };
    if let Err(err) = std::process::Command::new(command).args(args).spawn() {
        die(format!("could not open {target}: {err}"));
    }
}

/* --------------------------------------------------------------- sharing */

// Rights live on the document rather than in the server's flags: a document
// names its coauthors and its reviewers, and the flags are the ceiling it may
// not open past. These commands are that, on the command line.

/// The word a `--link` flag may spell a role with. The wire already accepts
/// both spellings (see `parse_role_word` on the server), so this exists only
/// to die locally with a clear message rather than spending a round trip to
/// learn that "editorr" is not a role. It returns the canonical, long-form
/// word, which is what a person reading the response back would expect to
/// see echoed.
pub(crate) fn parse_link_role(word: &str) -> Result<&'static str, String> {
    match word {
        "read" | "reader" => Ok("reader"),
        "comment" | "commenter" => Ok("commenter"),
        "edit" | "editor" => Ok("editor"),
        _ => Err(format!(
            "{word:?} is not a role: --link takes read, comment, or edit"
        )),
    }
}

/// `komodoc share c9k` with nothing else prints what the document says; with a
/// flag, changes it and prints the result.
pub async fn share_document(
    identifier: &str,
    server_flag: String,
    link: String,
    until: String,
    label: Option<String>,
    budget: Option<i64>,
    revoke: String,
) {
    let server = server_from(&server_flag);
    let slug = resolve_identifier(identifier, &server, "").await;
    let target = format!("{server}/api/documents/{slug}/share");

    let mut change = json!({});
    let mut minted_role: Option<&'static str> = None;
    if !link.is_empty() {
        let role = parse_link_role(&link).unwrap_or_else(|err| die(err));
        minted_role = Some(role);
        change["link"] = json!({"role": role, "until": until, "budget": budget});
        if let Some(label) = label {
            change["link"]["label"] = json!(label);
        }
    } else if !until.is_empty() || label.is_some() || budget.is_some() {
        die("--until, --label and --budget describe a link; pass --link read, --link comment, or --link edit");
    }
    if !revoke.is_empty() {
        change["revoke"] = json!(revoke);
    }

    // Nothing to change is a request to see what is there, which needs no
    // write and no confirmation.
    let asking = change.as_object().is_some_and(|fields| fields.is_empty());
    let (status, payload) = if asking {
        get_with_token(
            &target,
            &require_token_for(&server),
            Duration::from_secs(60),
        )
        .await
        .unwrap_or_else(|err| die(err))
    } else {
        post_json(
            &target,
            &change,
            &require_token_for(&server),
            Duration::from_secs(60),
        )
        .await
        .unwrap_or_else(|err| die(err))
    };
    if status != 200 {
        die(format!("share failed ({status}): {}", detail_of(&payload)));
    }

    // A mint prints just the link it made and its expiry: the document keeps
    // the key now (see `LinkGrant::key`), so there is nothing left to warn
    // about, and a full listing would only bury the one line somebody ran
    // this command to see.
    if let Some(role) = minted_role {
        let empty = Value::Null;
        let link = payload
            .get("links")
            .and_then(|links| links.get(role))
            .unwrap_or(&empty);
        println!("{}", format_role_row(role, link, &server));
        return;
    }
    print_sharing(&payload, &server, &slug);
}

/// One row of `komodoc share`'s listing: the role, the link if it has one,
/// and what state that link is in. `server` and the row's own `url` (a path
/// on that origin) are joined here because that is the whole point of the
/// row -- a link nobody can paste anywhere is not much of a share.
pub(crate) fn format_role_row(role: &str, link: &Value, server: &str) -> String {
    if link.is_null() {
        return format!("  {role:<8} off");
    }
    let key = text(link, "key");
    if key.is_empty() {
        // A link written before the key was kept: the document still knows
        // it existed, but cannot show it, so the only way forward is a new
        // one.
        return format!("  {role:<8} legacy link, reset to get a new one");
    }
    let url = text(link, "url");
    let expired = link.get("expired") == Some(&Value::Bool(true));
    let mut until = text(link, "until");
    until.truncate(10);
    let state = if expired {
        "expired".to_string()
    } else if until.is_empty() {
        "no expiry".to_string()
    } else {
        format!("expires {until}")
    };
    let label = text(link, "label");
    let label = if label.is_empty() {
        String::new()
    } else {
        format!(" [{label}]")
    };
    let budget = link
        .get("budget")
        .and_then(Value::as_i64)
        .map(|budget| format!("   {budget} comments/hour"))
        .unwrap_or_default();
    format!("  {role:<8}{label} {server}{url}   {state}{budget}")
}

/// The full listing `komodoc share` with no flags prints, one line per entry:
/// the slug, the owner's own link, then one row per role in the fixed order a
/// reader, a commenter, and an editor matter to somebody deciding what to
/// change, then whatever legacy people are still named on the document. Built as a plain
/// `Vec<String>` rather than printed straight away, so the order and the
/// content of the report can be checked without capturing stdout.
pub(crate) fn sharing_report_lines(payload: &Value, server: &str, slug: &str) -> Vec<String> {
    let mut lines = vec![
        slug.to_string(),
        // The owner's way in is their sign-in, and the row says so where the
        // others say when they expire: there is nothing to mint or revoke.
        format!("  {:<8} {server}/docs/{slug}   yours, signed in", "owner"),
    ];
    let links = payload.get("links").cloned().unwrap_or(Value::Null);
    let empty = Value::Null;
    for role in ["reader", "commenter", "editor"] {
        let link = links.get(role).unwrap_or(&empty);
        lines.push(format_role_row(role, link, server));
    }
    if let Some(legacy) = payload.get("legacy") {
        lines.push("  people (legacy)".to_string());
        for (field, role) in [("editors", "editor"), ("commenters", "commenter")] {
            for grant in legacy
                .get(field)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let mut since = text(&grant, "since");
                since.truncate(10);
                lines.push(format!("    {role:<9} @{}  {since}", text(&grant, "login")));
            }
        }
    }
    lines
}

fn print_sharing(payload: &Value, server: &str, slug: &str) {
    for line in sharing_report_lines(payload, server, slug) {
        println!("{line}");
    }
}

/// Hands a document to somebody else: its history, its comments and its quota
/// go with it. Confirmed like `destroy`, because it is the one other change
/// that leaves the caller with nothing.
pub async fn transfer_document(identifier: &str, to: &str, server_flag: String, yes: bool) {
    let server = server_from(&server_flag);
    let slug = resolve_identifier(identifier, &server, "").await;
    println!("About to transfer on {server}:");
    println!("  {slug}");
    println!("  to @{to}, with its history, its comments and its storage quota");
    println!("\nYou stop being its owner. Only @{to} can share or delete it after this.");
    if !yes {
        if !is_terminal_stdin() {
            die("refusing to transfer without a terminal to confirm at; pass --yes if you are certain");
        }
        eprint!("\nType '{slug}' to confirm: ");
        if read_line() != slug {
            println!("aborted, nothing was transferred");
            return;
        }
    }
    let (status, payload) = post_json(
        &format!("{server}/api/documents/{slug}/transfer"),
        &json!({"to": to}),
        &require_token_for(&server),
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "transfer failed ({status}): {}",
            detail_of(&payload)
        ));
    }
    println!("{slug} now belongs to @{}", text(&payload, "owner"));
}

/* ------------------------------------------------------------- destroy */

pub async fn destroy_document(identifier: &str, server_flag: String, yes: bool) {
    let server = server_from(&server_flag);
    // The same identifier `comment` and `export` take. The confirmation
    // below still asks for the whole slug: this is the one irreversible
    // command, and a three-character answer is too easy to give.
    let slug = resolve_identifier(identifier, &server, "").await;
    // Authenticated, not `get_json`: an owner's own document can be private,
    // and an unauthenticated read of it gets the same 404 a stranger would --
    // which used to stop `destroy` here before it ever reached the delete
    // route it already carries a token to. A document with no owner still
    // answers to an empty token the same as before.
    let (status, document) = get_with_token(
        &format!("{server}/api/documents/{slug}"),
        &stored_token_for(&server),
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!("no document with the slug {slug:?} at {server}"));
    }
    let count = document
        .get("comment_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    println!("About to permanently delete from {server}:");
    println!("  {slug}  {}", text(&document, "title"));
    println!("  {count} comment(s), and every reply to them");
    println!("\nThe document, its history and its comments all go. The link stops");
    println!("working. Nothing else on this deployment is touched.");

    if !yes {
        if !is_terminal_stdin() {
            die("refusing to delete without a terminal to confirm at; pass --yes if you are certain");
        }
        eprint!("\nType '{slug}' to confirm: ");
        if read_line() != slug {
            println!("aborted, nothing was deleted");
            return;
        }
    }

    let (status, payload) = post_json(
        &format!("{server}/api/documents/{slug}/delete"),
        &json!({}),
        &require_token_for(&server),
        Duration::from_secs(120),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!("delete failed ({status}): {}", detail_of(&payload)));
    }
    println!("deleted {slug}");
}

/* --------------------------------------------------------------- the timeline */

/// What this document used to say, and when.
///
/// One line per checkpoint, oldest first, which is the order the manifest is
/// in and the order a history reads in. The newest is marked, because "where
/// am I" is the first question anybody asks of a list like this, and a label
/// is printed as it was given: it is somebody's own words about a moment.
pub async fn history_document(identifier: &str, server_flag: String, key: String) {
    let server = server_from(&server_flag);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key).await;
    let (status, payload) = get_as(
        &format!("{server}/api/documents/{slug}/history"),
        &Credentials::new(&stored_token_for(&server), &key),
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "history failed ({status}): {}",
            detail_of(&payload)
        ));
    }
    let checkpoints = payload
        .get("checkpoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if checkpoints.is_empty() {
        println!("no checkpoints yet");
        return;
    }
    println!("sha      at                    by                  why      label");
    let last = checkpoints.len() - 1;
    for (n, point) in checkpoints.iter().enumerate() {
        let sha = text(point, "sha");
        // Seven characters, which is what git prints and what the panel and
        // `komodoc label` both accept.
        let short = sha.chars().take(7).collect::<String>();
        // The stored time is ISO 8601 in UTC; a table reads better with the
        // T and the Z taken out and nothing else changed.
        let at = text(point, "at")
            .replace('T', " ")
            .trim_end_matches('Z')
            .to_string();
        let label = text(point, "label");
        // The newest carries a star, because "where am I" is the first
        // question anybody asks of a list like this.
        let mark = match (n == last, label.is_empty()) {
            (false, _) => "",
            (true, true) => "*",
            (true, false) => "  *",
        };
        println!(
            "{short:<7}  {at:<19}  {:<18}  {:<7}  {label}{mark}",
            text(point, "by"),
            text(point, "why"),
        );
    }
}

/// Fetches a checkpoint after resolving the short digest printed by
/// `history`. The server still checks read permission on every request.
async fn checkpoint_for(
    server: &str,
    slug: &str,
    requested: &str,
    credentials: &Credentials,
) -> Value {
    let (status, history) = get_as(
        &format!("{server}/api/documents/{slug}/history"),
        credentials,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "history failed ({status}): {}",
            detail_of(&history)
        ));
    }
    let matching: Vec<String> = history
        .get("checkpoints")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|point| text(point, "sha"))
        .filter(|sha| sha.starts_with(requested))
        .collect();
    let sha = match matching.as_slice() {
        [sha] => sha,
        [] => die(format!("no checkpoint of {slug} starts with {requested:?}")),
        many => die(format!(
            "{requested:?} names {} checkpoints of {slug}; give more of the digest",
            many.len()
        )),
    };
    let (status, checkpoint) = get_as(
        &format!("{server}/api/documents/{slug}/history/{sha}"),
        credentials,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "checkpoint failed ({status}): {}",
            detail_of(&checkpoint)
        ));
    }
    checkpoint
}

/// Prints a source diff between two checkpoints. It uses the same checkpoint
/// objects as the browser, and a compact unified representation suitable for
/// a terminal or a pipe.
pub async fn diff_document(
    identifier: &str,
    from: &str,
    to: &str,
    server_flag: String,
    key: String,
) {
    let server = server_from(&server_flag);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key).await;
    let credentials = Credentials::new(&stored_token_for(&server), &key);
    let old = checkpoint_for(&server, &slug, from, &credentials).await;
    let new = checkpoint_for(&server, &slug, to, &credentials).await;
    let old_files = old.get("texts").and_then(Value::as_object);
    let new_files = new.get("texts").and_then(Value::as_object);
    let old_entries = old.get("files").and_then(Value::as_object);
    let new_entries = new.get("files").and_then(Value::as_object);
    let mut paths: Vec<String> = old_files
        .into_iter()
        .flat_map(|files| files.keys().cloned())
        .chain(
            new_files
                .into_iter()
                .flat_map(|files| files.keys().cloned()),
        )
        .chain(
            old_entries
                .into_iter()
                .flat_map(|files| files.keys().cloned()),
        )
        .chain(
            new_entries
                .into_iter()
                .flat_map(|files| files.keys().cloned()),
        )
        .collect();
    paths.sort();
    paths.dedup();
    let old_sha = text(&old, "sha");
    let new_sha = text(&new, "sha");
    let old_main = text(&old, "main");
    let new_main = text(&new, "main");
    if old_main != new_main {
        println!("# main file changed: {old_main} -> {new_main}");
    }
    for path in paths {
        let before = old_files
            .and_then(|files| files.get(&path))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let after = new_files
            .and_then(|files| files.get(&path))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let old_entry = old_entries.and_then(|files| files.get(&path));
        let new_entry = new_entries.and_then(|files| files.get(&path));
        let old_kind = old_entry
            .and_then(|entry| entry.get("kind"))
            .and_then(Value::as_str);
        let new_kind = new_entry
            .and_then(|entry| entry.get("kind"))
            .and_then(Value::as_str);
        let old_present =
            old_entry.is_some() || old_files.is_some_and(|files| files.contains_key(&path));
        let new_present =
            new_entry.is_some() || new_files.is_some_and(|files| files.contains_key(&path));
        if old_kind == Some("asset") || new_kind == Some("asset") {
            if old_kind != new_kind
                || old_entry.and_then(|entry| entry.get("sha"))
                    != new_entry.and_then(|entry| entry.get("sha"))
            {
                println!("Binary files a/{path}@{old_sha} and b/{path}@{new_sha} differ");
            }
            continue;
        }
        if old_kind != new_kind && old_kind.is_some() && new_kind.is_some() {
            println!("# file kind changed: {path} ({old_kind:?} -> {new_kind:?})");
            continue;
        }
        if before == after && old_present == new_present {
            continue;
        }
        print!(
            "{}",
            unified_source_diff(
                &path,
                before,
                after,
                &old_sha,
                &new_sha,
                old_present,
                new_present,
            )
        );
        if before.is_empty() && after.is_empty() && old_present != new_present {
            println!(
                "# empty file {}: {}",
                path,
                if new_present { "added" } else { "deleted" }
            );
        }
    }
}

fn unified_source_diff(
    path: &str,
    before: &str,
    after: &str,
    old_sha: &str,
    new_sha: &str,
    old_present: bool,
    new_present: bool,
) -> String {
    let old = diff_lines(before);
    let new = diff_lines(after);
    let ops = line_ops(&old, &new);
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(index, op)| (!matches!(op, LineOp::Equal(_))).then_some(index))
        .collect();
    let old_label = if !old_present {
        "/dev/null".to_string()
    } else {
        format!("a/{path}@{old_sha}")
    };
    let new_label = if !new_present {
        "/dev/null".to_string()
    } else {
        format!("b/{path}@{new_sha}")
    };
    let mut out = format!("--- {old_label}\n+++ {new_label}\n");
    let mut regions: Vec<(usize, usize)> = Vec::new();
    for index in changed {
        let start = index.saturating_sub(3);
        let end = (index + 4).min(ops.len());
        match regions.last_mut() {
            Some((_, previous_end)) if start <= *previous_end => {
                *previous_end = end.max(*previous_end)
            }
            _ => regions.push((start, end)),
        }
    }
    for (start, end) in regions {
        let old_before = ops[..start]
            .iter()
            .filter(|op| !matches!(op, LineOp::Insert(_)))
            .count();
        let new_before = ops[..start]
            .iter()
            .filter(|op| !matches!(op, LineOp::Delete(_)))
            .count();
        let old_count = ops[start..end]
            .iter()
            .filter(|op| !matches!(op, LineOp::Insert(_)))
            .count();
        let new_count = ops[start..end]
            .iter()
            .filter(|op| !matches!(op, LineOp::Delete(_)))
            .count();
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            unified_range(old_before, old_count),
            unified_range(new_before, new_count)
        ));
        for op in &ops[start..end] {
            let (prefix, line) = match op {
                LineOp::Equal(line) => (' ', line),
                LineOp::Delete(line) => ('-', line),
                LineOp::Insert(line) => ('+', line),
            };
            out.push(prefix);
            out.push_str(&line.text);
            out.push('\n');
            if !line.newline {
                out.push_str("\\ No newline at end of file\n");
            }
        }
    }
    out
}

#[derive(Clone)]
struct DiffLine {
    text: String,
    newline: bool,
}

enum LineOp {
    Equal(DiffLine),
    Delete(DiffLine),
    Insert(DiffLine),
}

fn diff_lines(source: &str) -> Vec<DiffLine> {
    if source.is_empty() {
        return Vec::new();
    }
    source
        .split_inclusive('\n')
        .map(|line| {
            let newline = line.ends_with('\n');
            DiffLine {
                text: line.trim_end_matches('\n').to_string(),
                newline,
            }
        })
        .collect()
}

fn line_ops(old: &[DiffLine], new: &[DiffLine]) -> Vec<LineOp> {
    let mut prefix = 0;
    while prefix < old.len() && prefix < new.len() && same_line(&old[prefix], &new[prefix]) {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old.len() - prefix
        && suffix < new.len() - prefix
        && same_line(&old[old.len() - 1 - suffix], &new[new.len() - 1 - suffix])
    {
        suffix += 1;
    }
    let old_mid = &old[prefix..old.len() - suffix];
    let new_mid = &new[prefix..new.len() - suffix];
    let mut ops = old[..prefix]
        .iter()
        .cloned()
        .map(LineOp::Equal)
        .collect::<Vec<_>>();
    // Generated sources can have many thousands of lines. A bounded fallback
    // remains a valid unified diff and avoids allocating a quadratic matrix
    // when the changed region itself is too large for an exact LCS.
    if old_mid.len().saturating_mul(new_mid.len()) > 4_000_000 {
        ops.extend(old_mid.iter().cloned().map(LineOp::Delete));
        ops.extend(new_mid.iter().cloned().map(LineOp::Insert));
    } else {
        ops.extend(line_ops_middle(old_mid, new_mid));
    }
    ops.extend(new[new.len() - suffix..].iter().cloned().map(LineOp::Equal));
    ops
}

fn same_line(old: &DiffLine, new: &DiffLine) -> bool {
    old.text == new.text && old.newline == new.newline
}

fn line_ops_middle(old: &[DiffLine], new: &[DiffLine]) -> Vec<LineOp> {
    let columns = new.len() + 1;
    let mut common = vec![0usize; (old.len() + 1) * columns];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            common[i * columns + j] =
                if old[i].text == new[j].text && old[i].newline == new[j].newline {
                    common[(i + 1) * columns + j + 1] + 1
                } else {
                    common[(i + 1) * columns + j].max(common[i * columns + j + 1])
                };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < old.len() || j < new.len() {
        if i < old.len()
            && j < new.len()
            && old[i].text == new[j].text
            && old[i].newline == new[j].newline
        {
            ops.push(LineOp::Equal(old[i].clone()));
            i += 1;
            j += 1;
        } else if j == new.len()
            || (i < old.len() && common[(i + 1) * columns + j] >= common[i * columns + j + 1])
        {
            ops.push(LineOp::Delete(old[i].clone()));
            i += 1;
        } else {
            ops.push(LineOp::Insert(new[j].clone()));
            j += 1;
        }
    }
    ops
}

fn unified_range(start: usize, count: usize) -> String {
    if count == 0 {
        format!("{start},0")
    } else if count == 1 {
        (start + 1).to_string()
    } else {
        format!("{},{}", start + 1, count)
    }
}

/// Restores a checkpoint through the editor-only API. The key may be either a
/// raw share key or the complete link copied from a browser.
pub async fn restore_document(identifier: &str, sha: &str, server_flag: String, key: String) {
    let server = server_from(&server_flag);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key).await;
    let credentials = Credentials::new(&stored_token_for(&server), &key);
    let (status, document) = get_as(
        &format!("{server}/api/documents/{slug}"),
        &credentials,
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!("cannot restore {slug}: {}", detail_of(&document)));
    }
    let role = text(&document, "role");
    if document.get("can_edit") != Some(&Value::Bool(true))
        && !matches!(role.as_str(), "editor" | "owner")
    {
        die(format!(
            "you may read {slug} but not edit it; restore requires an editor"
        ));
    }
    let (status, payload) = post_json_as(
        &format!("{server}/api/documents/{slug}/restore"),
        &json!({"sha": sha}),
        &credentials,
        Duration::from_secs(120),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "restore failed ({status}): {}",
            detail_of(&payload)
        ));
    }
    let restored = text(&payload, "sha");
    println!(
        "restored {slug} to {}",
        restored.chars().take(7).collect::<String>()
    );
}

/// Names a checkpoint, or takes its name away with an empty one.
///
/// The SHA may be the short form the table prints, which is resolved against
/// the manifest here rather than on the server: the server takes one name for
/// a checkpoint, its whole digest, and a prefix that matched two of them would
/// be a thing for a person to disambiguate rather than for a route to guess.
pub async fn label_checkpoint(identifier: &str, sha: &str, label: String, server_flag: String) {
    let server = server_from(&server_flag);
    let slug = resolve_identifier(identifier, &server, "").await;
    let token = require_token_for(&server);
    let (status, payload) = get_with_token(
        &format!("{server}/api/documents/{slug}/history"),
        &token,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "history failed ({status}): {}",
            detail_of(&payload)
        ));
    }
    let checkpoints = payload
        .get("checkpoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let matching: Vec<String> = checkpoints
        .iter()
        .map(|point| text(point, "sha"))
        .filter(|known| known.starts_with(sha))
        .collect();
    let full = match matching.len() {
        0 => die(format!("no checkpoint of {slug} starts with {sha:?}")),
        1 => matching[0].clone(),
        many => die(format!(
            "{sha:?} names {many} checkpoints of {slug}; give more of the digest"
        )),
    };

    let (status, payload) = crate::http::patch_json(
        &format!("{server}/api/documents/{slug}/history/{full}"),
        &json!({"label": label}),
        &token,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!("label failed ({status}): {}", detail_of(&payload)));
    }
    let given = text(&payload, "label");
    let short = full.chars().take(7).collect::<String>();
    if given.is_empty() {
        println!("{short}  unnamed");
    } else {
        println!("{short}  {given}");
    }
}

#[cfg(test)]
mod diff_tests {
    use super::{unified_range, unified_source_diff};

    #[test]
    fn zero_line_ranges_keep_the_zero_count() {
        assert_eq!(unified_range(0, 0), "0,0");
        assert_eq!(unified_range(4, 0), "4,0");
    }

    #[test]
    fn added_and_deleted_files_have_valid_unified_ranges() {
        let added = unified_source_diff("new.md", "", "added\n", "old", "new", false, true);
        assert!(added.contains("--- /dev/null\n+++ b/new.md@new\n"));
        assert!(added.contains("@@ -0,0 +1 @@"), "{added}");

        let deleted = unified_source_diff("gone.md", "removed\n", "", "old", "new", true, false);
        assert!(deleted.contains("--- a/gone.md@old\n+++ /dev/null\n"));
        assert!(deleted.contains("@@ -1 +0,0 @@"), "{deleted}");
    }

    #[test]
    fn empty_file_membership_has_headers_without_a_fake_hunk() {
        let added = unified_source_diff("empty.md", "", "", "old", "new", false, true);
        assert!(added.contains("--- /dev/null\n+++ b/empty.md@new\n"));
        assert!(!added.contains("@@"));
    }
}

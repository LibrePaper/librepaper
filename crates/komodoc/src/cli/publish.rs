//! `komodoc publish`: a file or a directory, rendered here when it is markdown
//! or typst, sent to the deployment as one upload.

use super::*;

/// The subset of deployment configuration that controls a publish preflight.
/// This is decoded from `/api/config` so a CLI pointed at a deployment with
/// operator-selected limits does not reject a document using its own defaults.
#[derive(serde::Deserialize)]
pub(crate) struct PublishLimits {
    pub(crate) max_document: usize,
    pub(crate) max_files: usize,
    pub(crate) max_path: usize,
    pub(crate) max_assets: i64,
    pub(crate) max_asset: i64,
}

impl PublishLimits {
    fn validate(self) -> Result<Self, String> {
        if self.max_document == 0
            || self.max_files == 0
            || self.max_path == 0
            || self.max_assets < 0
            || self.max_asset < 0
            || self.max_asset > self.max_assets
        {
            return Err("deployment returned invalid publishing limits".into());
        }
        Ok(self)
    }
}

pub(crate) async fn publish_limits(server: &str) -> Result<PublishLimits, String> {
    let (status, payload) =
        get_json(&format!("{server}/api/config"), Duration::from_secs(30)).await?;
    if status != 200 {
        return Err(format!(
            "could not read publishing limits ({status}): {}",
            detail_of(&payload)
        ));
    }
    serde_json::from_value(payload)
        .map_err(|err| format!("deployment returned invalid publishing limits: {err}"))
        .and_then(PublishLimits::validate)
}

fn config_with_publish_limits(limits: PublishLimits) -> Configuration {
    Configuration {
        max_document: limits.max_document,
        max_files: limits.max_files,
        max_path: limits.max_path,
        max_assets: limits.max_assets,
        max_asset: limits.max_asset,
        ..Configuration::default()
    }
}

pub(crate) async fn preserve_revision_title_with_token(
    server: &str,
    slug: &str,
    title: &mut String,
    token: &str,
) -> Result<(), String> {
    if slug.is_empty() {
        return Ok(());
    }
    let (status, existing) = get_with_token(
        &format!("{server}/api/documents/{slug}"),
        token,
        Duration::from_secs(30),
    )
    .await
    .map_err(|err| format!("could not read existing document {slug}: {err}"))?;
    if status != 200 {
        return Err(format!(
            "could not read existing document {slug} ({status}): {}",
            detail_of(&existing)
        ));
    }
    let existing_title = text(&existing, "title");
    if existing_title.is_empty() {
        return Err(format!(
            "could not read existing document {slug}: response had no title"
        ));
    }
    *title = existing_title;
    Ok(())
}

async fn preserve_revision_title(
    server: &str,
    slug: &str,
    title: &mut String,
) -> Result<(), String> {
    preserve_revision_title_with_token(server, slug, title, &stored_token_for(server)).await
}

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
            eprintln!("note: could not read directory {}", directory.display());
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                eprintln!("note: could not read an entry in {}", directory.display());
                continue;
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.ends_with(".komodoc-lock") {
                continue;
            }
            // Ask git about directories as well as files. An ignored
            // directory is pruned here, avoiding a process per descendant and
            // preserving the author's intent for generated trees.
            if ignored(&path) {
                continue;
            }
            let resolved = match path.canonicalize() {
                Ok(resolved) => resolved,
                Err(_) => {
                    eprintln!("note: skipping broken or unreadable {}", path.display());
                    continue;
                }
            };
            if !resolved.starts_with(&canonical_root) {
                eprintln!(
                    "note: skipping {} (a symlink resolving outside {})",
                    path.display(),
                    root.display()
                );
                continue;
            }
            if path.is_dir() {
                // `insert` returns false for a location already visited -- an
                // ancestor symlink cycle, or two links to the same place --
                // which is exactly when this must not be walked again.
                if visited.insert(resolved) {
                    stack.push(path);
                }
                continue;
            }
            let Ok(metadata) = std::fs::metadata(&path) else {
                eprintln!("note: skipping unreadable {}", path.display());
                continue;
            };
            if !metadata.is_file() {
                eprintln!("note: skipping non-regular file {}", path.display());
                continue;
            }
            // The output of the document itself. A `paper.pdf` beside
            // `paper.typ` is what the last compile produced, and uploading it
            // would put a derived file in a store that keeps sources.
            let output = if main_stem.is_empty() {
                None
            } else {
                let main = Path::new(main_stem);
                let stem = main
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_else(|| main_stem.to_string());
                Some(main.with_file_name(format!("{stem}.pdf")))
            };
            let relative = path.strip_prefix(root).ok();
            if output.as_deref() == relative {
                continue;
            }
            if let Some(relative) = relative {
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
    let cache = std::sync::Mutex::new(std::collections::HashMap::<PathBuf, bool>::new());
    Box::new(move |path: &Path| {
        let relative = path.strip_prefix(&root).unwrap_or(path).to_path_buf();
        if let Ok(cache) = cache.lock() {
            if let Some(&ignored) = cache.get(&relative) {
                return ignored;
            }
        }
        // `git -C root` runs with `root` as its working directory, so the
        // path handed to `check-ignore` must be relative to `root` too --
        // otherwise, with a relative root such as `paper`, a root-prefixed
        // path like `paper/private.txt` is resolved against that working
        // directory as `paper/paper/private.txt`, and a root-anchored
        // pattern such as `/private.txt` never matches. `--` ends option
        // parsing first, so a filename beginning with `-` is not read as a
        // flag.
        let ignored = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["check-ignore", "-q", "--"])
            .arg(relative)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if let Ok(mut cache) = cache.lock() {
            cache.insert(
                path.strip_prefix(&root).unwrap_or(path).to_path_buf(),
                ignored,
            );
        }
        ignored
    })
}

/// Which file is the document. Named with `--main`, or worked out: the one
/// text at the top level whose extension is a format this renders, or
/// `main.*`. Anything else is a refusal that lists what it was choosing
/// between, because guessing wrong here publishes the wrong document.
pub fn main_file(files: &[String], asked: &str) -> Result<String, String> {
    if !asked.is_empty() {
        let wanted = crate::document::paths::normalise(asked);
        if !files.contains(&wanted) {
            return Err(format!("--main {asked} is not a file in that directory"));
        }
        return Ok(wanted);
    }
    let top: Vec<&String> = files
        .iter()
        .filter(|path| !path.contains('/'))
        .filter(|path| crate::document::render::document_format(path).is_some())
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
pub(super) async fn publish_directory(
    root: &Path,
    mut title: String,
    slug: String,
    server_flag: String,
    main: String,
) {
    let title_explicit = !title.is_empty();
    let server = server_from(&server_flag);
    let config = publish_limits(&server)
        .await
        .map(config_with_publish_limits)
        .unwrap_or_else(|err| die(err));
    let rules = config.paths();
    // The main file first, since what it is called decides what is skipped as
    // its output. Worked out from the whole listing, so `--main` can name a
    // file the rules would otherwise have to be asked about twice.
    let ignored = git_ignores(root);
    let listed = files_under(root, "", &ignored);
    let main = main_file(&listed, &main).unwrap_or_else(|why| die(why));
    // The first walk is also the upload walk. Filter only the output beside
    // the selected main file; a `fig/main.pdf` is an ordinary asset when the
    // main file is `main.typ`.
    let output = Path::new(&main).with_file_name(
        Path::new(&main)
            .file_stem()
            .map(|stem| format!("{}.pdf", stem.to_string_lossy()))
            .unwrap_or_default(),
    );
    let paths: Vec<String> = listed
        .into_iter()
        .filter(|path| Path::new(path) != output.as_path())
        .collect();

    // Every path checked before anything is read, so a refusal names the file
    // rather than arriving after a megabyte has been sent.
    let mut texts = 0usize;
    let mut figures = 0i64;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for at in &paths {
        let kind = match crate::document::paths::check(&rules, at) {
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
            crate::document::paths::Kind::Text => {
                if std::str::from_utf8(&bytes).is_err() {
                    eprintln!("skipping {at}: it is not valid UTF-8 text");
                    continue;
                }
                texts += bytes.len();
            }
            crate::document::paths::Kind::Asset => {
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
        title = match crate::document::render::document_format(&main) {
            Some("markdown") => title_from_markdown(&source),
            Some("html") => title_from_html(&source),
            Some("latex") => crate::document::render::title_from_latex(&source),
            _ => String::new(),
        };
    }
    if !title_explicit {
        preserve_revision_title(&server, &slug, &mut title)
            .await
            .unwrap_or_else(|err| die(err));
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

    let uploaded_paths: std::collections::HashSet<String> = files
        .iter()
        .map(|(path, _)| crate::document::paths::normalise(path))
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
            .filter(|path| !uploaded_paths.contains(&crate::document::paths::normalise(path)))
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
pub(super) fn report_published(server: &str, document: &Value, path: &str) {
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

pub(super) async fn publish_file(file: &str, mut title: String, slug: String, server_flag: String) {
    let title_explicit = !title.is_empty();
    let server = server_from(&server_flag);
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
    if extension != "html"
        && extension != "htm"
        && crate::document::render::document_format(file).is_none()
    {
        die(format!(
            "{base_name} is not a document Komodoc can serve.\n\n  \
             It takes HTML, markdown, typst or LaTeX.\n  \
             From Quarto:\n    quarto render paper.qmd --to html -M embed-resources:true"
        ));
    }
    let raw =
        std::fs::read(path).unwrap_or_else(|err| die(format!("could not read {file}: {err}")));
    let config = publish_limits(&server)
        .await
        .map(config_with_publish_limits)
        .unwrap_or_else(|err| die(err));
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
    } else if crate::document::render::is_latex(file) {
        // Nothing is rendered here, and nothing can be: Komodoc carries no TeX
        // and no build embeds one. So this is the one format `publish` uploads
        // without having compiled it first, and the check that a document
        // compiles -- which is the reason `publish` compiles typst at all --
        // happens in the first browser that opens it instead. Said plainly,
        // because an author used to `publish` refusing a broken paper should
        // not have to infer that this one is different.
        if title.is_empty() {
            title = crate::document::render::title_from_latex(&html);
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

    if !title_explicit {
        // Publishing a revision keeps the existing title even when the new
        // source has a heading of its own. The authenticated lookup matters
        // for private documents, whose metadata is invisible anonymously.
        preserve_revision_title(&server, &slug, &mut title)
            .await
            .unwrap_or_else(|err| die(err));
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
pub(super) async fn upload_typst_pdf(
    server: &str,
    document: &Value,
    pdf: Vec<u8>,
    expected_inputs: &str,
) {
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
pub(super) fn input_digest_for_typst(
    main: &str,
    files: &[(String, Vec<u8>)],
    rules: &crate::document::paths::Rules<'_>,
) -> String {
    let mut tree = crate::document::history::Tree {
        main: main.to_string(),
        files: std::collections::BTreeMap::new(),
        settings: None,
    };
    for (path, bytes) in files {
        let kind = match crate::document::paths::check(rules, path) {
            Ok(kind) => kind,
            Err(_) => continue,
        };
        let kind = match kind {
            crate::document::paths::Kind::Text => "text",
            crate::document::paths::Kind::Asset => "asset",
        };
        tree.files.insert(
            crate::document::paths::normalise(path),
            crate::document::history::TreeEntry {
                kind: kind.to_string(),
                id: String::new(),
                sha: crate::document::store::digest_of_bytes(bytes),
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

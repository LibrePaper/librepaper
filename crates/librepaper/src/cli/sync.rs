//! `pull` and `push`: move files between a working directory and a web
//! project. The work is `librepaper-sync`'s; this reads the command line,
//! finds the sign-in and says what happened.

use super::*;

use librepaper_sync::{
    commit_summary, publish, pull as pull_files, push as push_files, store_for, Client, Link,
    MergeOutcome, PullOptions, PullOutcome, PushOptions, PushOutcome, State, StateStore,
};

/// The sign-in to use, when it is not the one `login` stored.
#[derive(Args, Clone, Debug)]
pub(crate) struct Sign {
    /// A login token or a project token to use instead of the stored sign-in
    #[arg(
        long,
        env = "LIBREPAPER_TOKEN",
        value_name = "TOKEN",
        hide_env_values = true,
        help_heading = "Deployment"
    )]
    token: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct PullArgs {
    /// The project page, https://<site>/docs/<slug>; needed the first time
    #[arg(value_name = "URL")]
    url: Option<String>,
    /// The directory to bring the project into
    #[arg(long, value_name = "DIR", default_value = ".")]
    dir: PathBuf,
    /// In a git repository, fetch the versions and stop before the merge
    #[arg(long)]
    no_merge: bool,
    /// Outside a repository, keep this file as it is when both sides changed it
    #[arg(long, value_name = "PATH")]
    mine: Vec<String>,
    /// Outside a repository, take the web version of this file when both
    /// sides changed it
    #[arg(long, value_name = "PATH")]
    theirs: Vec<String>,
    #[command(flatten)]
    sign: Sign,
}

#[derive(Args, Debug)]
pub(crate) struct PushArgs {
    /// The project page, https://<site>/docs/<slug>; needed the first time
    #[arg(value_name = "URL")]
    url: Option<String>,
    /// The directory to send
    #[arg(long, value_name = "DIR", default_value = ".")]
    dir: PathBuf,
    /// What to call the version; in a git repository, the latest commit's
    /// subject
    #[arg(long, value_name = "TEXT")]
    message: Option<String>,
    /// Who made the change; in a git repository, the latest commit's author
    #[arg(long, value_name = "NAME")]
    author: Option<String>,
    /// Publish the directory as a new project with this title
    #[arg(long, value_name = "TEXT")]
    title: Option<String>,
    #[command(flatten)]
    deployment: Deployment,
}

/// A directory and what it remembers.
struct Workspace {
    dir: PathBuf,
    store: Box<dyn StateStore>,
    state: Option<State>,
}

fn open(dir: &Path) -> Result<Workspace, String> {
    let dir = std::fs::canonicalize(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let store = store_for(&dir, &state_home());
    let state = store.load().map_err(|error| error.to_string())?;
    Ok(Workspace { dir, store, state })
}

/// The project the directory is linked to and a client for it. A first call
/// with a URL records the link, so that the URL is not needed again.
fn connect(
    workspace: &Workspace,
    url: Option<&str>,
    token: Option<&str>,
) -> Result<(Client, State), String> {
    let (link, state) = match (&workspace.state, url) {
        (Some(state), None) => (state.link.clone(), state.clone()),
        (Some(state), Some(url)) => {
            let given = Link::parse(url).map_err(|error| error.to_string())?;
            if given != state.link {
                return Err(format!(
                    "this directory is linked to {}",
                    state.link.project_url()
                ));
            }
            (given, state.clone())
        }
        (None, Some(url)) => {
            let link = Link::parse(url).map_err(|error| error.to_string())?;
            let state = State::new(link.clone());
            (link, state)
        }
        (None, None) => {
            return Err(
                "this directory is not linked to a project; give its page once, such as https://app.librepaper.org/docs/<slug>"
                    .into(),
            )
        }
    };
    let token = require_token_for(&link.origin(), token);
    if workspace.state.is_none() {
        workspace
            .store
            .save(&state)
            .map_err(|error| error.to_string())?;
    }
    Ok((
        Client::new(link.site.clone(), Credentials::token(&token)),
        state,
    ))
}

pub(crate) async fn pull(args: PullArgs) {
    match run_pull(args).await {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => die(error),
    }
}

async fn run_pull(args: PullArgs) -> Result<bool, String> {
    let workspace = open(&args.dir)?;
    let (client, _) = connect(&workspace, args.url.as_deref(), args.sign.token.as_deref())?;
    let options = PullOptions {
        merge: !args.no_merge,
        mine: args.mine,
        theirs: args.theirs,
    };
    let outcome = pull_files(&client, &workspace.dir, workspace.store.as_ref(), &options)
        .await
        .map_err(|error| error.to_string())?;
    Ok(show_pull(&outcome))
}

pub(crate) async fn push(args: PushArgs) {
    match run_push(args).await {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => die(error),
    }
}

async fn run_push(args: PushArgs) -> Result<bool, String> {
    let workspace = open(&args.dir)?;
    if let Some(title) = &args.title {
        if workspace.state.is_some() {
            return Err(
                "this directory is already linked to a project; --title publishes a new one".into(),
            );
        }
        if args.url.is_some() {
            return Err("give a project page or --title, not both".into());
        }
        let server = server_or_die(args.deployment.server.clone());
        let site = url::Url::parse(&server).map_err(|error| format!("{server}: {error}"))?;
        let token = require_token_for(&server, args.deployment.token.as_deref());
        let client = Client::new(site, Credentials::token(&token));
        let link = publish(&client, &workspace.dir, workspace.store.as_ref(), title)
            .await
            .map_err(|error| error.to_string())?;
        println!("Published {}.", link.project_url());
        return Ok(true);
    }

    let (client, state) = connect(
        &workspace,
        args.url.as_deref(),
        args.deployment.token.as_deref(),
    )?;
    let summary = commit_summary(&workspace.dir, &state);
    let message = args
        .message
        .or_else(|| summary.as_ref().map(|summary| summary.message.clone()))
        .ok_or("give --message: this directory is not a git repository")?;
    let author = args
        .author
        .or_else(|| summary.map(|summary| summary.author));
    let options = PushOptions { message, author };
    let outcome = push_files(&client, &workspace.dir, workspace.store.as_ref(), &options)
        .await
        .map_err(|error| error.to_string())?;
    Ok(show_push(&outcome, &options.message))
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

fn list(heading: &str, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    println!("{heading}");
    for path in paths {
        println!("  {path}");
    }
}

/// Prints what a pull did. False when the person has something to settle.
fn show_pull(outcome: &PullOutcome) -> bool {
    match outcome {
        PullOutcome::UpToDate => {
            println!("Up to date.");
            true
        }
        PullOutcome::Repository {
            versions,
            merge,
            trimmed,
        } => {
            if *trimmed {
                println!("The web history was trimmed; started again from its newest version.");
            }
            if !versions.is_empty() {
                println!("Took {}:", plural(versions.len(), "version"));
                for version in versions {
                    println!("  {} ({})", version.label, version.by);
                }
            }
            show_merge(merge)
        }
        PullOutcome::Folder {
            written,
            trashed,
            kept,
            conflicts,
            skipped,
            trimmed,
        } => {
            if *trimmed {
                println!("The web history was trimmed; started again from its newest version.");
            }
            if written.is_empty()
                && trashed.is_empty()
                && kept.is_empty()
                && conflicts.is_empty()
                && skipped.is_empty()
            {
                println!("Up to date.");
                return true;
            }
            list("Wrote:", written);
            list("Moved to the trash:", trashed);
            list("Kept, changed only here:", kept);
            list(
                "Excluded here (ignored, hidden or not text), so left alone though the web has them:",
                skipped,
            );
            if conflicts.is_empty() {
                return true;
            }
            eprintln!("Conflicts, changed here and on the web, so not written:");
            for path in conflicts {
                eprintln!("  {path}");
            }
            eprintln!("Run again with --mine PATH to keep yours or --theirs PATH to take the web version.");
            false
        }
    }
}

fn show_merge(merge: &MergeOutcome) -> bool {
    match merge {
        MergeOutcome::Skipped => {
            println!(
                "Fetched into refs/librepaper/main. To merge it: git merge refs/librepaper/main"
            );
            true
        }
        MergeOutcome::FastForward => {
            println!("Fast-forwarded.");
            true
        }
        MergeOutcome::Merged => {
            println!("Merged.");
            true
        }
        MergeOutcome::AlreadyMerged => {
            println!("Already merged.");
            true
        }
        MergeOutcome::Conflicts(paths) => {
            eprintln!("The merge stopped on conflicts in:");
            for path in paths {
                eprintln!("  {path}");
            }
            eprintln!("Resolve them and commit, as after any git merge.");
            false
        }
        MergeOutcome::DirtyTree(paths) => {
            eprintln!("These files have uncommitted changes:");
            for path in paths {
                eprintln!("  {path}");
            }
            eprintln!("Commit them first, then pull again.");
            false
        }
    }
}

/// Prints what a push did. False when the person has something to settle.
fn show_push(outcome: &PushOutcome, message: &str) -> bool {
    match outcome {
        PushOutcome::Nothing => {
            println!("Nothing to push.");
            true
        }
        PushOutcome::Pushed { files, .. } => {
            println!("Pushed {}: {message}", plural(*files, "file"));
            true
        }
        PushOutcome::Refused { paths } => {
            eprintln!("These files changed on the web:");
            for path in paths {
                eprintln!("  {path}");
            }
            eprintln!("Pull first.");
            false
        }
        PushOutcome::Suggestion { path } => {
            eprintln!(
                "{path} has a pending suggestion. Accept or reject it on the web, then push again."
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_worded_in_the_singular_and_the_plural() {
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(0, "file"), "0 files");
        assert_eq!(plural(3, "version"), "3 versions");
    }

    #[test]
    fn only_settled_outcomes_exit_with_success() {
        assert!(show_push(&PushOutcome::Nothing, "m"));
        assert!(show_push(
            &PushOutcome::Pushed {
                label_sha: "s".into(),
                sequence: 1,
                files: 2
            },
            "m"
        ));
        assert!(!show_push(
            &PushOutcome::Refused {
                paths: vec!["a.tex".into()]
            },
            "m"
        ));
        assert!(!show_push(
            &PushOutcome::Suggestion {
                path: "a.tex".into()
            },
            "m"
        ));

        assert!(show_pull(&PullOutcome::UpToDate));
        for merge in [
            MergeOutcome::Skipped,
            MergeOutcome::FastForward,
            MergeOutcome::Merged,
            MergeOutcome::AlreadyMerged,
        ] {
            assert!(show_merge(&merge));
        }
        assert!(!show_merge(&MergeOutcome::Conflicts(vec!["a.tex".into()])));
        assert!(!show_merge(&MergeOutcome::DirtyTree(vec!["a.tex".into()])));
        assert!(show_pull(&PullOutcome::Folder {
            written: vec!["a.tex".into()],
            trashed: Vec::new(),
            kept: Vec::new(),
            conflicts: Vec::new(),
            skipped: Vec::new(),
            trimmed: false,
        }));
        assert!(!show_pull(&PullOutcome::Folder {
            written: Vec::new(),
            trashed: Vec::new(),
            kept: Vec::new(),
            conflicts: vec!["a.tex".into()],
            skipped: Vec::new(),
            trimmed: false,
        }));
        // Files left alone are told, and are not something to settle.
        assert!(show_pull(&PullOutcome::Folder {
            written: Vec::new(),
            trashed: Vec::new(),
            kept: Vec::new(),
            conflicts: Vec::new(),
            skipped: vec!["build/out.tex".into()],
            trimmed: false,
        }));
    }

    #[test]
    fn a_directory_needs_its_link_once() {
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = store_for(dir.path(), home.path());
        let unlinked = Workspace {
            dir: dir.path().to_path_buf(),
            store,
            state: None,
        };
        // No stored state and no URL: nothing to connect to.
        let error = connect(&unlinked, None, Some("lp_token")).unwrap_err();
        assert!(error.contains("not linked"), "{error}");
        let error = connect(&unlinked, Some("not a url"), Some("lp_token")).unwrap_err();
        assert!(error.contains("not a project URL"), "{error}");

        // A URL with a token records the link.
        let (_, state) = connect(
            &unlinked,
            Some("https://app.librepaper.org/docs/paper-abc"),
            Some("lp_token"),
        )
        .unwrap();
        assert_eq!(state.link.slug, "paper-abc");
        let remembered = unlinked.store.load().unwrap().unwrap();
        assert_eq!(remembered.link, state.link);

        // Later calls need no URL, and a different one is refused.
        let linked = Workspace {
            state: Some(remembered),
            ..unlinked
        };
        assert!(connect(&linked, None, Some("lp_token")).is_ok());
        let error = connect(
            &linked,
            Some("https://app.librepaper.org/docs/other"),
            Some("lp_token"),
        )
        .unwrap_err();
        assert!(error.contains("paper-abc"), "{error}");
    }
}

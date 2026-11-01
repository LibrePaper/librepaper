# Syncing a folder or a Git repository

`librepaper pull` brings a web project's current version into a folder. `librepaper push` sends the folder's changes back. A GitHub Action runs both for a repository. The full guide is `docs/git.md`. Read its sections for anything this file leaves out.

LibrePaper never merges text and never holds a Git credential. Git is the user's, so conflicts are resolved with the user's own tools.

## Before the first command

- The `librepaper` command must be installed (see [agent-setup.md](agent-setup.md), step 2).
- A credential, one of:
  - A project token: an `lpt_` agent token with the Edit role, up to one year. Give it with `--token` or `LIBREPAPER_TOKEN` (not `LIBREPAPER_AGENT_TOKEN`, which is the MCP server's variable). It works only for that existing project.
  - `librepaper login`, which signs this computer in for 90 days. Publishing a new project with `push --title` needs this.
- Treat the token as in [agent.md](agent.md): never print it whole or write it into the repository or the workflow file.

```sh
# Paste the token once for this shell; nothing is echoed or kept in history
read -rs LIBREPAPER_TOKEN && export LIBREPAPER_TOKEN
```

## Commands

```sh
librepaper pull https://<app origin>/docs/<slug> --preview       # before the first pull: what it would change; writes nothing
librepaper pull https://<app origin>/docs/<slug> --branch main   # first real pull: binds the repository to main and to the project
librepaper pull                                                  # later: fetch and merge into the bound branch
librepaper pull --no-merge                                       # fetch only, as commits on librepaper/main
librepaper pull --mine intro.tex                                 # plain folder: keep the local copy of a file changed on both sides
librepaper pull --theirs intro.tex                               # plain folder: take the web copy
librepaper push                                                  # send changes; the message defaults to the latest commit subject
librepaper push --message "Tighten the abstract"                 # required outside a repository
librepaper push --title "Thesis draft"                           # publish a folder with no project yet; needs librepaper login
```

- Run `--preview` first when the folder and the project both already hold files. It lists files only on one side, different on both, excluded, and blocked.
- A push is all or nothing. Each push becomes one named version in the project's History.
- Each named version becomes one commit on pull, authored `handle@site`, with a `LibrePaper-Version` trailer. Unnamed edits arrive as one "Current version" commit.

## What syncs

- Text, figures and fonts with an extension the project accepts. Text must be UTF-8.
- Never: `.gitignore`d files, files with the Git attribute `-librepaper` in `.gitattributes`, names starting with a dot, paths deeper than eight segments, and build outputs.
- Comments, suggestions and chat stay on the web. Pull leaves pending suggestions out of the text it writes.
- Limits: 200 files per project and one push no larger than the log quota (32 MiB) by default.

## When a command refuses

| Message or symptom | Cause | Fix |
|---|---|---|
| Pull refuses: uncommitted changes | The repository is dirty | Commit, then pull |
| Pull refuses: run `librepaper push` first | A push is pending from an interrupted run | `librepaper push`, then pull |
| Stops and names the bound branch | Another branch, or a detached HEAD, is checked out | `git switch <bound branch>`. LibrePaper never switches for you |
| No bound branch | First real transfer without `--branch` | Run pull once with `--branch <branch>` |
| Refused for another project | The repository is bound to a different project | Use a separate clone; nothing was overwritten |
| Push says "Pull first." | A file changed on the web too | Pull, resolve any conflict, push again |
| Push says a suggestion is pending | A changed file has a pending tracked change | Ask an editor to accept or reject it on the web |
| Push interrupted | The command stopped before the server confirmed | Run `librepaper push` again; it sends the pending request first |
| Cannot sign in, 401 | The token expired or was revoked | Ask for a new token; replace the secret or the exported value |

- Recovery of a pending push works only with the same account on the bound branch. It is not exactly once if history was trimmed in between.
- Never delete `refs/librepaper/state` or the folder's sync state to get past an error. That loses the pending request and the binding.

## GitHub Action

`LibrePaper/sync-action@v1` runs pull on a schedule and push on every push to the bound branch. `docs/git.md` (GitHub Action) has the workflow file to copy.

- The project token goes in the repository secret `LIBREPAPER_TOKEN`. The workflow needs `contents: write` and `pull-requests: write`, `fetch-depth: 0`, and `concurrency: librepaper`.
- `branch` must equal the `ref` that `actions/checkout` checks out.
- When a push is refused or a scheduled pull conflicts, the Action commits the web's files to `librepaper/sync`, opens a pull request titled "LibrePaper changed files you also changed", and fails on purpose. Merge that pull request in Git. The next push sends the merged files.
- "run librepaper push": a send from an earlier run is pending. Re-run the push workflow for that commit, or run `librepaper push` in a clone.
- A schedule more often than every 15 minutes buys nothing. Hourly fits a private repository's free GitHub minutes. Every 15 minutes does not.
- The Action never force-pushes or bypasses branch protection.

## The desktop app's Git panel

The desktop app can connect a web project to a repository from its Git panel, without the CLI. It uses the user's own SSH key or credential helper. On GitHub, use the SSH address (`git@github.com:owner/repo.git`), because the `https://` address does not sign in through the GitHub CLI there.

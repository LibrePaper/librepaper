# LibrePaper sync

A composite action that keeps a Git repository and a LibrePaper web project in step. It downloads a pinned `librepaper` release and runs its `push` and `pull` commands. Nothing else needs installing.

## What it does

- On a push to the branch, it sends the pushed commit to the project as one named version. The commit subject and author name the version. This is the repository-to-web direction.
- On a schedule, or a manual run, it takes the web project's current version and merges it into the branch. It pushes the merge when there is one. This is the web-to-repository direction. If the branch moved during the run, the push fails, the state is not published, and the next run repeats the pull from the same point.
- When a file changed on both sides, it does not guess. It opens a pull request from `librepaper/sync` and the run fails. A person merges the pull request in git.
- When a send to the web project is pending from an earlier run, a scheduled or manual run stops with an error. Re-run the push workflow, or run `librepaper push` in a clone, to send it.
- It keeps the sync state under `refs/librepaper/*` on origin, so the state survives between runners.

Every event finishes its own transfer, or reports what is left. The Action's own pushes do not start another workflow run.

## Inputs

| Input | Required | Default | Meaning |
| --- | --- | --- | --- |
| `project` | yes | | The project page, `https://<site>/docs/<slug>`. |
| `token` | yes | | A project token with the Edit role. Store it as a secret. |
| `version` | no | `v0.0.52` | The `librepaper` release tag to download. |
| `branch` | no | `main` | The branch to sync. The workflow checks it out with `ref` set to this branch. The script stops when the checkout is on another branch or a detached HEAD. |

The default `version` follows the workspace version in `Cargo.toml`. Bump it in `action.yml` when a release ships `push` and `pull`.

## Create the token

1. Open the project on LibrePaper and choose Share.
2. Create a project token with the Edit role and an expiry of 1 year.
3. In the repository, add the token as a secret named `LIBREPAPER_TOKEN`.

Revoke the token in Share to stop the sync.

In the repository settings, enable "Allow GitHub Actions to create and approve pull requests". Otherwise a refused push cannot open its pull request.

## The first run

The first run links the repository to the project. It pulls the project's versions into the branch, pushes the branch, and pushes the repository's own files that the project lacks. Nothing needs seeding by hand.

If a file differs on both sides, the first run stops with an error. A person then merges it once on a computer: open the project in the LibrePaper desktop app and use Connect a repository in its Git panel, or run `librepaper pull <project>` in a clone. Then push the branch and `refs/librepaper/*`.

## Workflow

Copy this into `.github/workflows/librepaper.yml`. Replace `<slug>` with the project's slug. Keep `ref` and `branch` equal.

```yaml
on:
  push:
    branches: [main]
  schedule:
    - cron: "0 * * * *"          # hourly
  workflow_dispatch:
concurrency: librepaper
jobs:
  sync:
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
    steps:
      - uses: actions/checkout@v4
        with:
          ref: main
          fetch-depth: 0
      - uses: LibrePaper/sync-action@v1
        with:
          project: https://app.librepaper.org/docs/<slug>
          token: ${{ secrets.LIBREPAPER_TOKEN }}
          branch: main
```

Each scheduled run spends GitHub Actions minutes, so hourly is the default. GitHub also delays frequent scheduled runs. A shorter interval costs more minutes, and it should not be shorter than 15 minutes.

## When a push is refused

- The push run fails with a red mark. Its log says the web project changed files that this push also changed.
- A pull request titled "LibrePaper changed files you also changed" opens from `librepaper/sync`. Later refusals add a comment to it.
- Merge the pull request in git, or merge `librepaper/sync` into the branch and resolve any conflicts. The next push sends the merged result to the web project.

A scheduled run that hits a conflict aborts its merge and opens the same pull request.

## When a send is pending

A push can stop after it saves its request and before the web project confirms it. The next push run sends the pending request first. A scheduled or manual run does not, so it stops with this error:

- Re-run the push workflow for the commit that failed. A re-run sends the pending request only while that commit is still the tip of the branch.
- Or run `librepaper push` in a clone of the repository, with the same token and branch.

The next push run recovers the request only when the run that stopped published its state. A run that stopped before it published the state, or a runner that disappeared before publishing, leaves nothing to recover.

## Repository and web results

Each run reports the two sides separately. A repository push that fails does not undo a web write that already succeeded. The Action never force-pushes the branch or bypasses branch protection. If a protected branch rejects the Action's push, the web side is reported as it is, and the next run repeats the transfer.

## Security

- Anyone with push rights on the repository can change the project through this action. Each push is a named version with the git author. It appears in the project's History and can be restored there.
- The GitHub token only touches this repository. The LibrePaper token only touches the project.

## For maintainers

This directory is published as the repository `LibrePaper/sync-action`. Publish it from a clone of this repository, and move the `v1` tag on each release.

```sh
# Create the public repository once
gh repo create LibrePaper/sync-action --public
# Split tools/sync-action into its own history and push it as main
git subtree split --prefix tools/sync-action --branch sync-action-publish
git push git@github.com:LibrePaper/sync-action.git sync-action-publish:main
# Point the v1 tag at that history and push it
git tag -f v1 sync-action-publish
git push -f git@github.com:LibrePaper/sync-action.git v1
```

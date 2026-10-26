# LibrePaper sync

A composite action that keeps a Git repository and a LibrePaper web project in step. It downloads a pinned `librepaper` release and runs its `push` and `pull` commands. Nothing else needs installing.

## What it does

- On a push to the branch, it sends the pushed commit to the project as one named version. The commit subject and author name the version.
- On a schedule, or a manual run, it takes the web project's newer versions and merges them into the branch. It pushes the merge when there is one. If the branch moved during the run, the push fails, the state is not published, and the next run repeats the pull from the same point.
- When a file changed on both sides, it does not guess. It opens a pull request from `librepaper/sync` and the run fails. A person merges the pull request in git.
- It keeps the sync state under `refs/librepaper/*` on origin, so the state survives between runners.

## Inputs

| Input | Required | Default | Meaning |
| --- | --- | --- | --- |
| `project` | yes | | The project page, `https://<site>/docs/<slug>`. |
| `token` | yes | | A project token with the Edit role. Store it as a secret. |
| `version` | no | `v0.0.52` | The `librepaper` release tag to download. |
| `branch` | no | `main` | The branch to sync. The workflow must check it out. |

The default `version` follows the workspace version in `Cargo.toml`. Bump it in `action.yml` when a release ships `push` and `pull`.

## Create the token

1. Open the project on LibrePaper and choose Share.
2. Create a project token with the Edit role and an expiry of 1 year.
3. In the repository, add the token as a secret named `LIBREPAPER_TOKEN`.

Revoke the token in Share to stop the sync.

In the repository settings, enable "Allow GitHub Actions to create and approve pull requests". Otherwise a refused push cannot open its pull request.

## Before the first run

Seed the LibrePaper state once, from a clone of the repository on your computer:

```sh
# Once, on your computer, in a clone of the repository
librepaper pull https://app.librepaper.org/docs/<slug>   # links the clone and fetches the project as commits
git push origin main 'refs/librepaper/*'                 # the Action starts from this state
```

The first pull merges the project's versions into your branch itself, even when the repository and the project share no history. A file that differs on both sides is left as a git conflict to resolve before you push.

The Action refuses to run until `refs/librepaper/state` exists on GitHub, and after that it keeps that state current itself.

## Workflow

Copy this into `.github/workflows/librepaper.yml`. Replace `<slug>` with the project's slug.

```yaml
on:
  push:
    branches: [main]
  schedule:
    - cron: "*/15 * * * *"
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
          fetch-depth: 0
      - uses: LibrePaper/sync-action@v1
        with:
          project: https://app.librepaper.org/docs/<slug>
          token: ${{ secrets.LIBREPAPER_TOKEN }}
```

## When a push is refused

- The push run fails with a red mark. Its log says the web project changed files that this push also changed.
- A pull request titled "LibrePaper changed files you also changed" opens from `librepaper/sync`. Later refusals add a comment to it.
- Merge the pull request in git, or merge `librepaper/sync` into the branch and resolve any conflicts. The next push sends the merged result to the web project.

A scheduled run that hits a conflict aborts its merge and opens the same pull request.

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

---
title: "Storage and configuration"
---

## Storage

`librepaper admin serve` keeps everything in the directory named by `--data-directory` or
`LIBREPAPER_DATA`, `librepaper-data` in the working directory by default: the
catalogue (`catalog.db`), the objects it names, private server state, and the
secrets that keep sessions and share links valid. Back it up if the instance
holds real work; `librepaper admin backup create` writes a verified recovery point of
all of it, and `librepaper admin backup restore` restores one into a fresh
directory.
See the [operator cost policy](https://github.com/LibrePaper/librepaper/blob/main/docs/cost-policy.md) for the complete defaults,
advanced YAML schema, `admin status` command, capacity accounting, and backup
reservations.

The storage flags bound what a deployment will store:

| Flag | Caps | Default |
| --- | --- | --- |
| `--document-size-limit` | combined source text of one document | 4 MB (maximum 8) |
| `--document-assets-limit` | combined input assets of one document | 32 MiB |
| `--publisher-storage-limit` | everything one publisher holds | 100 MB |
| `--deployment-storage-limit` | the whole deployment | 5120 MB |
| `--publisher-document-limit` | documents one publisher may hold | 50 |
| `--publisher-upload-limit` | uploads one publisher may make in an hour | 30 |

```sh
librepaper admin serve --document-size-limit 8 --document-assets-limit 16 --publisher-storage-limit 500 --deployment-storage-limit 10240
```

`--document-size-limit` may not be set above 8 MB. It bounds the text a person can see;
what has to be durably saved is the CRDT snapshot behind that text, which
carries the document's edit history and metadata as well, and this deployment
supports snapshots up to 16 MB. A document can therefore reach that second
ceiling without its visible text ever approaching the first -- an edit refused
for that reason says so, and says that the history counts too. A configuration
whose ceilings could accept work the journal could not durably save is refused
at startup rather than at the first save.

A document is a directory, so `--document-size-limit` bounds the sum of its texts and
`--document-assets-limit` bounds the combined input assets. Both count against `--publisher-storage-limit`; a figure is
an upload and counts against `--publisher-upload-limit` like any other. A Typst or
LaTeX document keeps source and input assets only. PDF and HTML output created
by a browser or companion is transient and never counts toward storage,
quotas, or uploads. On a deployment with many publishers,
`--document-assets-limit` is the one worth lowering:
figures are where a paper's bytes actually are, and it is what stops a single
document spending a publisher's whole allowance on images.

Origin transfer has its own rolling 24-hour budget:

```sh
librepaper admin serve --transfer-budget 10GiB
```

The value is bytes (binary suffixes such as `KiB`, `MiB`, `GiB`, and `TiB` are
accepted). An explicit `0` refuses ordinary transfer while retaining the small
emergency allowance for control and durability responses. Omitting the flag
keeps transfer unlimited and produces a startup warning. Compiler files come
from the direct mirror and do not count against this origin budget.

Publishing is always attributed to an authenticated Google or GitHub account
and charged against that account's quota. Anonymous readers and commenters do
not receive a publishing quota.

## Environment variables

Service settings that support environment variables follow the same name:
`--foo-bar` is `LIBREPAPER_FOO_BAR`, and the flag wins when both are set.
`librepaper admin serve --help` (and every other subcommand's `--help`) is the
reference for the full list. Advanced guardrails may be overridden in an
optional YAML file selected with `--config PATH` (or `LIBREPAPER_CONFIG`). The
proxy list is the top-level `trusted_proxies` key; `cost.trusted_proxies` is
not accepted. An optional `backup` map is reporting metadata for
operator-managed backups (`destination_class`, `frequency` in seconds,
`retained_count`, `encrypted`, and `warning_count`); it does not schedule or
delete backups. Omitted keys retain their documented defaults.

These variables are useful in deployment files. Secrets are environment-only;
service settings have corresponding CLI flags, and installer variables control
the installation script:

| Variable | Purpose |
| --- | --- |
| `LIBREPAPER_GITHUB_CLIENT_SECRET` | GitHub OAuth app client secret |
| `LIBREPAPER_GOOGLE_CLIENT_SECRET` | Google OAuth client secret |
| `LIBREPAPER_BUDGET_TRANSFER` | rolling 24-hour origin response budget; bare values are bytes |
| `LIBREPAPER_BUDGET_DOCUMENT_ASSETS` | combined input assets per document, in MiB |
| `LIBREPAPER_LATEX_MIRROR` | HTTPS static mirror URL fetched directly by browsers |
| `LIBREPAPER_TYPST_FONTS` | optional local directory of additional Typst fonts |
| `LIBREPAPER_CONFIG` | optional advanced YAML policy overrides |
| `LIBREPAPER_VERSION` | Version the installer fetches |
| `LIBREPAPER_BIN_DIR` | Installation directory the installer uses |

[^github-data]: LibrePaper requests no GitHub scopes through OAuth. It uses the
GitHub API only to obtain your public login name and account id; it does not
collect your email, repositories, or other profile data. The bar shows you your
own public avatar, fetched from GitHub by your browser.

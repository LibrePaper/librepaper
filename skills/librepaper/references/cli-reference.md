<!-- Generated from the CLI's clap definitions by `make cli-reference`. Do not edit. -->

# librepaper command reference

This document contains the help content for the `librepaper` command-line program.

**Command Overview:**

* [`librepaper`](#librepaper)
* [`librepaper login`](#librepaper-login)
* [`librepaper logout`](#librepaper-logout)
* [`librepaper admin`](#librepaper-admin)
* [`librepaper admin serve`](#librepaper-admin-serve)
* [`librepaper admin migrate`](#librepaper-admin-migrate)
* [`librepaper admin config`](#librepaper-admin-config)
* [`librepaper admin config check`](#librepaper-admin-config-check)
* [`librepaper admin config show`](#librepaper-admin-config-show)
* [`librepaper admin backup`](#librepaper-admin-backup)
* [`librepaper admin restore`](#librepaper-admin-restore)
* [`librepaper admin moderate`](#librepaper-admin-moderate)
* [`librepaper admin moderate block-account`](#librepaper-admin-moderate-block-account)
* [`librepaper admin moderate unblock-account`](#librepaper-admin-moderate-unblock-account)
* [`librepaper admin moderate erase-account`](#librepaper-admin-moderate-erase-account)
* [`librepaper admin moderate hide-project`](#librepaper-admin-moderate-hide-project)
* [`librepaper admin moderate unhide-project`](#librepaper-admin-moderate-unhide-project)
* [`librepaper list`](#librepaper-list)
* [`librepaper export`](#librepaper-export)
* [`librepaper pull`](#librepaper-pull)
* [`librepaper push`](#librepaper-push)
* [`librepaper mcp`](#librepaper-mcp)

## `librepaper`

sign in, list, export, pull and push documents, and administer deployments

**Usage:** `librepaper <COMMAND>`

To sign in from this terminal, set the deployment and run `librepaper login`:

    export LIBREPAPER_SERVER=https://librepaper.example.org
    librepaper login

###### **Subcommands:**

* `login`: Sign in through a deployment, in a browser
* `logout`: Forget the stored sign-in
* `admin`: Deployment administration and operator commands
* `list`: List your documents
* `export`: Export a complete independent copy of the project
* `pull`: Bring a project's named versions into a directory
* `push`: Send a directory's changes to a project as one named version
* `mcp`: Serve a project's document tools to an MCP client over stdio



## `librepaper login`

Sign in through a deployment, in a browser

**Usage:** `librepaper login [OPTIONS]`

###### **Options:**

* `--server <URL>`: Deployment to talk to
* `--token <TOKEN>`: A credential to use instead of the one `login` stored



## `librepaper logout`

Forget the stored sign-in

**Usage:** `librepaper logout`



## `librepaper admin`

Deployment administration and operator commands

**Usage:** `librepaper admin <COMMAND>`

###### **Subcommands:**

* `serve`: Run the service on this machine
* `migrate`: Apply pending PostgreSQL schema migrations, then exit
* `config`: Inspect or validate the effective deployment configuration
* `backup`: Create a snapshot-consistent PostgreSQL and immutable-object recovery point
* `restore`: Restore a verified backup into a new deployment directory
* `moderate`: Block accounts or hide and restore projects. This command connects directly to the deployment database and requires database access



## `librepaper admin serve`

Run the service on this machine

**Usage:** `librepaper admin serve [OPTIONS]`

###### **Options:**

* `--config <PATH>`

  Default value: `/etc/librepaper/librepaper.toml`



## `librepaper admin migrate`

Apply pending PostgreSQL schema migrations, then exit

**Usage:** `librepaper admin migrate [OPTIONS]`

###### **Options:**

* `--config <PATH>`

  Default value: `/etc/librepaper/librepaper.toml`



## `librepaper admin config`

Inspect or validate the effective deployment configuration

**Usage:** `librepaper admin config <COMMAND>`

###### **Subcommands:**

* `check`: Validate the configuration without opening storage or binding ports
* `show`: Print effective configuration with secrets redacted



## `librepaper admin config check`

Validate the configuration without opening storage or binding ports

**Usage:** `librepaper admin config check [OPTIONS]`

###### **Options:**

* `--config <PATH>`

  Default value: `/etc/librepaper/librepaper.toml`



## `librepaper admin config show`

Print effective configuration with secrets redacted

**Usage:** `librepaper admin config show [OPTIONS]`

###### **Options:**

* `--config <PATH>`

  Default value: `/etc/librepaper/librepaper.toml`



## `librepaper admin backup`

Create a snapshot-consistent PostgreSQL and immutable-object recovery point

**Usage:** `librepaper admin backup [OPTIONS] <DIRECTORY>`

###### **Arguments:**

* `<DIRECTORY>`: New directory to create, or - to write a tar archive to stdout

###### **Options:**

* `--config <PATH>`

  Default value: `/etc/librepaper/librepaper.toml`
* `--id <ID>`: Identifier to record for this backup; generated when omitted
* `--prefix <PREFIX>`: Path inside the archive that holds the recovery point (with -)

  Default value: `backup`
* `--include <PATH>`: Absolute file or directory to add to the archive at its own path (with -)



## `librepaper admin restore`

Restore a verified backup into a new deployment directory

**Usage:** `librepaper admin restore [OPTIONS] <BACKUP> <DIRECTORY>`

###### **Arguments:**

* `<BACKUP>`: Backup directory to read
* `<DIRECTORY>`: New deployment directory to create

###### **Options:**

* `--config <PATH>`

  Default value: `/etc/librepaper/librepaper.toml`



## `librepaper admin moderate`

Block accounts or hide and restore projects. This command connects directly to the deployment database and requires database access

**Usage:** `librepaper admin moderate <COMMAND>`

###### **Subcommands:**

* `block-account`: Revoke an account's sessions and deny future access and writes
* `unblock-account`: Restore an account blocked by moderation
* `erase-account`: Erase an account and every project it owns, as the owner's Settings button does. Projects purge after the deletion grace. This cannot be undone
* `hide-project`: Hide a project from every document and asset route without deleting it
* `unhide-project`: Restore public access to a project hidden by moderation



## `librepaper admin moderate block-account`

Revoke an account's sessions and deny future access and writes

**Usage:** `librepaper admin moderate block-account [OPTIONS] --actor <ACTOR> --reason <REASON> <ACCOUNT>`

###### **Arguments:**

* `<ACCOUNT>`: Account UUID or unique provider handle

###### **Options:**

* `--actor <ACTOR>`
* `--reason <REASON>`
* `--config <PATH>`

  Default value: `/etc/librepaper/librepaper.toml`



## `librepaper admin moderate unblock-account`

Restore an account blocked by moderation

**Usage:** `librepaper admin moderate unblock-account [OPTIONS] --actor <ACTOR> --reason <REASON> <ACCOUNT>`

###### **Arguments:**

* `<ACCOUNT>`: Account UUID or unique provider handle

###### **Options:**

* `--actor <ACTOR>`
* `--reason <REASON>`
* `--config <PATH>`

  Default value: `/etc/librepaper/librepaper.toml`



## `librepaper admin moderate erase-account`

Erase an account and every project it owns, as the owner's Settings button does. Projects purge after the deletion grace. This cannot be undone

**Usage:** `librepaper admin moderate erase-account [OPTIONS] --actor <ACTOR> --reason <REASON> <ACCOUNT>`

###### **Arguments:**

* `<ACCOUNT>`: Account UUID or unique provider handle

###### **Options:**

* `--actor <ACTOR>`
* `--reason <REASON>`
* `--config <PATH>`

  Default value: `/etc/librepaper/librepaper.toml`



## `librepaper admin moderate hide-project`

Hide a project from every document and asset route without deleting it

**Usage:** `librepaper admin moderate hide-project [OPTIONS] --actor <ACTOR> --reason <REASON> <PROJECT>`

###### **Arguments:**

* `<PROJECT>`: Project slug

###### **Options:**

* `--actor <ACTOR>`
* `--reason <REASON>`
* `--config <PATH>`

  Default value: `/etc/librepaper/librepaper.toml`



## `librepaper admin moderate unhide-project`

Restore public access to a project hidden by moderation

**Usage:** `librepaper admin moderate unhide-project [OPTIONS] --actor <ACTOR> --reason <REASON> <PROJECT>`

###### **Arguments:**

* `<PROJECT>`: Project slug

###### **Options:**

* `--actor <ACTOR>`
* `--reason <REASON>`
* `--config <PATH>`

  Default value: `/etc/librepaper/librepaper.toml`



## `librepaper list`

List your documents

**Usage:** `librepaper list [OPTIONS]`

###### **Options:**

* `--server <URL>`: Deployment to talk to
* `--token <TOKEN>`: A credential to use instead of the one `login` stored



## `librepaper export`

Export a complete independent copy of the project

**Usage:** `librepaper export [OPTIONS] <ID> <DIR>`

###### **Arguments:**

* `<ID>`: A full slug, or one of the short handles `list` prints
* `<DIR>`: A new directory to create

###### **Options:**

* `--at <LABEL>`: Export the project as it stood at this label instead of its live state; requesting a historical export waits for the server to build it
* `--key <LINK>`: An editor share link, or the key from one: exported as its holder rather than as your sign-in
* `--server <URL>`: Deployment to talk to
* `--token <TOKEN>`: A credential to use instead of the one `login` stored



## `librepaper pull`

Bring a project's named versions into a directory

In a git repository each version becomes a commit on refs/librepaper/main, which is then merged; elsewhere the newest version is written into the directory.

**Usage:** `librepaper pull [OPTIONS] [URL]`

###### **Arguments:**

* `<URL>`: The project page, https://<site>/docs/<slug>; needed the first time

###### **Options:**

* `--dir <DIR>`: The directory to bring the project into

  Default value: `.`
* `--no-merge`: In a git repository, fetch the versions and stop before the merge
* `--mine <PATH>`: Outside a repository, keep this file as it is when both sides changed it
* `--theirs <PATH>`: Outside a repository, take the web version of this file when both sides changed it
* `--preview`: Show how the project and this folder differ, without linking or writing anything; only for a folder that is not linked yet
* `--branch <BRANCH>`: In a git repository, the branch to sync; needed the first time, then it must match the checkout
* `--token <TOKEN>`: A login token or a project token to use instead of the stored sign-in



## `librepaper push`

Send a directory's changes to a project as one named version

With --title, publish the directory as a new project instead.

**Usage:** `librepaper push [OPTIONS] [URL]`

###### **Arguments:**

* `<URL>`: The project page, https://<site>/docs/<slug>; needed the first time

###### **Options:**

* `--dir <DIR>`: The directory to send

  Default value: `.`
* `--message <TEXT>`: What to call the version; in a git repository, the latest commit's subject
* `--author <NAME>`: Who made the change; in a git repository, the latest commit's author
* `--title <TEXT>`: Publish the directory as a new project with this title
* `--branch <BRANCH>`: In a git repository, the branch to sync; needed the first time, then it must match the checkout
* `--server <URL>`: Deployment to talk to
* `--token <TOKEN>`: A credential to use instead of the one `login` stored



## `librepaper mcp`

Serve a project's document tools to an MCP client over stdio

Give it an agent token in LIBREPAPER_AGENT_TOKEN. The token names its server and its project, so no other flag is needed. Register it with Claude Code once with `claude mcp add librepaper --scope user -- librepaper mcp`.

**Usage:** `librepaper mcp [OPTIONS]`

###### **Options:**

* `--server <URL>`: Deployment to talk to
* `--token <TOKEN>`: A credential to use instead of the one `login` stored




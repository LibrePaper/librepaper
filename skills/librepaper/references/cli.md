# The librepaper command

The `librepaper` command signs in to a server, lists and exports projects, syncs folders, serves the document tools over MCP, and administers a server. Every command, flag and environment variable is in [cli-reference.md](cli-reference.md), which is generated from the CLI itself. This file says which command fits a task and what the reference cannot.

Install it as [agent-setup.md](agent-setup.md) step 2 shows. `librepaper --version` prints the installed release. When the reference and `librepaper <command> --help` disagree, the installed binary wins.

## Which command for which task

| Task | Use |
|---|---|
| Read, comment on, suggest changes to or edit a project | MCP or the HTTP routes with an agent token ([agent.md](agent.md)), not the CLI |
| Register LibrePaper with an MCP client | `librepaper mcp` ([agent-setup.md](agent-setup.md)) |
| Keep a folder or Git repository in step with a project | `librepaper pull` and `librepaper push` ([sync.md](sync.md)) |
| List the user's projects | `librepaper list` |
| Take a complete copy of a project, now or at a named version | `librepaper export` |
| Run or administer a server | `librepaper admin ...` ([host-setup.md](host-setup.md), [host-operate.md](host-operate.md)) |

## Credentials

Three environment variables matter. The two token variables hold different credentials, so do not mix them up.

| Variable | Credential | Read by |
|---|---|---|
| `LIBREPAPER_AGENT_TOKEN` | An `lpt_` agent token for one project | `librepaper mcp` |
| `LIBREPAPER_TOKEN` | An `lpt_` project token, or another credential instead of the stored sign-in | `pull`, `push`, `list`, `export` |
| `LIBREPAPER_SERVER` | Not a credential: the server's address, such as `https://app.librepaper.org` | `login`, `list`, `export`, and `push --title`. `pull` and `push` otherwise take the server from the project address |

- `librepaper login` signs this computer in for 90 days. It prints a code and a URL, and the user approves it in a browser. An agent cannot finish it alone, so hand it to the user.
- `librepaper logout` forgets the stored sign-in on this computer. A token copied elsewhere keeps working until it expires or is revoked.
- Never print a token whole or write it into a file the user did not ask for. Read it into the environment without echo, as [agent-setup.md](agent-setup.md) shows.

## List and export

```sh
# Pick the server once per shell
export LIBREPAPER_SERVER=https://app.librepaper.org
librepaper list                                 # id, date and title of each project
librepaper export <id> ./paper-copy             # a complete independent copy of the live project
librepaper export <id> ./paper-copy --at "v1"   # the project as it stood at a named version
librepaper export <id> ./paper-copy --key <URL> # as the holder of an editor share link, not as the signed-in user
```

- `<id>` is a full slug or a short handle that `list` printed.
- The target directory must not exist yet. `export` creates it.
- An export at a named version waits while the server builds it.
- An export is a copy, not a sync. To keep a folder in step, use `pull` instead.

## Regenerating the reference

In the LibrePaper repository, `make cli-reference` rewrites [cli-reference.md](cli-reference.md) from the clap definitions in `crates/librepaper/src/cli/`. A test in `crates/librepaper/tests/cli_surface.rs` fails when the committed file is stale, so a change to a command, a flag or its help text regenerates the file in the same commit.

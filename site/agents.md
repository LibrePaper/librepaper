---
title: "Agents"
---

Give an agent a LibrePaper link and it can work on the document with that link's permissions. A read link reads, a comment link also annotates, and an edit link also changes source. Signing in supplies attribution and satisfies the deployment's sign-in policy; it does not give an agent using a read link the owner's editing rights.

LibrePaper bundles three agent skills and their reference files in the binary, and every deployment serves them over HTTP at `/skills/<name>/SKILL.md`. An agent can read them from there without installing anything.

Reading a skill and holding it are different things. An agent that discovers skills through a directory needs the files on disk, because that is where its harness reads the frontmatter that grants tool permissions; a copy fetched over HTTP is documentation and carries no authority. Install them with:

```sh
npx skills add LibrePaper/librepaper
```

Updating LibrePaper updates its bundled instructions at the same time, so add them again after an upgrade. Check `librepaper --version` and the required commands' `--help` for compatibility; using bundled skills does not require an online latest-release check. The sidebar's connection prompt tells the agent how to read them.

- [`librepaper-document`](https://github.com/LibrePaper/librepaper/blob/main/skills/librepaper-document/SKILL.md): read, comment on, and edit through the configured MCP tools.
- [`librepaper-pair`](https://github.com/LibrePaper/librepaper/blob/main/skills/librepaper-pair/SKILL.md): pair live in the sidebar chat.
- [`librepaper-write`](https://github.com/LibrePaper/librepaper/blob/main/skills/librepaper-write/SKILL.md): proofread, tighten, rewrite, and explain with anchored suggestions an editor reviews.

The skills teach agents to use LibrePaper's MCP tools. Direct document operations are deliberately not duplicated as shell commands.

## Runner and Codex integration

The robot icon opens the assistant panel. **Copy setup prompt** gives your existing agent instructions to start a local runner. The runner owns a separate Codex session and keeps its connection open while the model works. Codex must be installed and authenticated locally; LibrePaper does not receive its model credentials or run inference on the document server.

```sh
librepaper agent connect "$LIBREPAPER_DOCUMENT" \
  "$LIBREPAPER_CONVERSATION" --background
```

The setup prompt supplies `LIBREPAPER_CHAT_TOKEN` separately from the document link. Both are required: a conversation token does not widen document access. The browser reports queued work, progress, completion, and failures. Follow-up requests can be submitted while a task runs; cancellation requests stop active work where supported and never undo document changes already made.

The runner configures the Codex thread with LibrePaper's MCP tools. It starts the bundled stdio adapter as `librepaper agent mcp -`, inheriting the protected `LIBREPAPER_DOCUMENT` environment value. The model uses `document_read`, `document_propose`, `document_apply`, `document_comment`, and `document_result`; document links and tokens are never MCP tool arguments. Hosts that need a standalone adapter can register the same command with `codex mcp add librepaper -- librepaper agent mcp -` and provide `LIBREPAPER_DOCUMENT` in the host's protected environment.

## Workflows and review interface

Select a passage to Tighten, Rewrite, or Explain. Address a comment from its thread, or ask the assistant to fix a diagnostic. Requests carry their captured source context and revision. Suggestions use the ordinary review interface, with Accept, Reject, and Refine; refinement updates the existing proposal and refuses changes to a proposal that was already decided or modified elsewhere.

`document_read` provides focused queries for files, headings, source sections, passages, comment threads, bibliography source, and changes since a checkpoint.

Candidate verification uses the browser's renderer on temporary source files. It does not apply the candidate to the collaborative document. A render result belongs to that candidate revision; unavailable renderers or a disconnected browser cannot produce a successful verification.

## Local state and session management

The runner keeps task continuity and writing preferences locally. The browser keeps its assistant history on the same device. The document server relays messages without storing transcripts. Reconnection reconciles known task IDs; it does not blindly repeat uncertain work. Use the panel's New conversation control to clear its history and start a fresh channel, and the runner's `status` and `stop` commands to inspect or end the local process.

See the [assistant protocol](https://github.com/LibrePaper/librepaper/blob/main/docs/protocol/chat.md) and [document operations](https://github.com/LibrePaper/librepaper/blob/main/docs/protocol/room-v1.md).

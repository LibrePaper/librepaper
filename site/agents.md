---
title: "Agents"
---

Give an agent a LibrePaper link and it can work on the document with that link's permissions. A comment link annotates and suggests; an edit link also changes source. There is no read-only assistant: the document's tool surface admits a commenter at minimum, so a read link would give an agent no tools at all. Signing in supplies attribution and satisfies the deployment's sign-in policy; it does not widen what a link grants.

Connecting an agent is a click in the sidebar, not a prompt you paste. The browser cannot install software or start processes, and asking a language model to do it on the browser's behalf is exactly what a coding agent's permission system is built to refuse. So the local LibrePaper app does it instead: the browser asks the computer directly, and every step is either one command a person types or one button a person presses.

## Setup, once per computer

Install the app and pair this browser with it. The app prints a six digit code when it starts; enter it in the sidebar. This is the same pairing the local compiler uses, so if you already paired for Quarto or native TeX there is nothing to do.

```sh
curl -fsSL https://librepaper.org/install.sh | sh
```

Afterwards the sidebar lists the coding agents actually installed on that machine, because only the local app can read a PATH.

## Starting the assistant

Pick an agent, pick what it may do, and press **Start assistant**. The local app starts the runner and the runner drives the agent you chose. You then work with it in the sidebar: ask for changes in **Chat**, or pick one from **Tasks** with a passage selected.

That is the only way an agent is connected. LibrePaper never writes another tool's configuration and there is no separate terminal mode: the document, the conversation and the review interface are in one place, which is the point.

Bring your own model is literal here: the agent you picked is already installed and already signed in, LibrePaper never sees a model name or a credential, and no inference runs on the document server. The sidebar names the running agent, so the claim is checkable rather than asserted.

The document reaches the session as an MCP server handed to the agent over ACP. The model uses `document_read`, `document_propose`, `document_apply`, `document_comment` and `document_result`; document links and tokens are never MCP tool arguments. The command names a connection rather than a link:

```
librepaper mcp --connection thesis
```

This computer holds what the name means, so a document key never reaches a command line, a shell history or a transcript; a rotated link is repaired in one place; and the settings page lists what agents here can reach and how to remove each. Disconnecting the site from the app drops its connections too.

Agents differ in how they reach ACP, and the sidebar says which case yours is rather than leaving you to find out:

| Agent | Route | What the sidebar does |
|---|---|---|
| opencode | `opencode acp`, native | Starts it; no adapter, nothing to fetch |
| Gemini CLI | `gemini --experimental-acp`, native | Starts it |
| Claude Code | separate npm package | Fetched on first start; the panel says so beforehand |
| Pi | `npx -y pi-acp` | Same, plus a note in the panel that the adapter's MCP support is incomplete, so the document tools may not reach it |
| Codex | separate binary | Names the missing adapter instead of hiding the button |

Nothing is fetched without the panel saying so first.

For anything else, including agents LibrePaper has never heard of, tell this computer how to run it:

```sh
librepaper local agent add myagent --label "My Agent" -- myagent --acp
```

It then appears in the sidebar's agent list like any other. `librepaper local agent list` and `remove` manage the declarations. This is a local command rather than a sidebar field on purpose: the browser chooses *which* agent to drive, never *what command* to run, the same rule the build protocol follows by refusing a caller-supplied command.

Declaring an id that is already in the table overrides how that agent is driven rather than adding a second entry for it, so a changed entry point or a locally built adapter wins over the compiled-in default. The table is a starting guess; this computer is the authority.

The browser reports queued work, progress, completion and failures. Follow-up requests can be submitted while a task runs. Cancellation stops active work where supported and never undoes document changes already made. Permission requests from the agent appear in the sidebar with the options the agent itself offered.

What a task achieved is read from the operation journal, where every receipt the document service issued is recorded. The agent is asked for an answer in prose and nothing else, so no model can report a suggestion it did not create.

## Skills

LibrePaper bundles two agent skills and their reference files in the binary, and every deployment serves them over HTTP at `/skills/<name>/SKILL.md`. An agent can read them from there without installing anything.

- [`librepaper-document`](https://github.com/LibrePaper/librepaper/blob/main/skills/librepaper-document/SKILL.md): read, comment on, and edit through the configured MCP tools.
- [`librepaper-write`](https://github.com/LibrePaper/librepaper/blob/main/skills/librepaper-write/SKILL.md): proofread, tighten, rewrite, and explain with anchored suggestions an editor reviews.

Neither is needed: the runner supplies the writing instructions to the session it starts. Install them on disk only if you also use those conventions elsewhere.

```sh
npx skills add LibrePaper/librepaper
```

The skills teach agents to use LibrePaper's MCP tools. Direct document operations are deliberately not duplicated as shell commands.

## Workflows and review interface

Select a passage to Tighten, Rewrite, or Explain. Address a comment from its thread, or ask the assistant to fix a diagnostic. Requests carry their captured source context and revision. Suggestions use the ordinary review interface, with Accept, Reject, and Refine; refinement updates the existing proposal and refuses changes to a proposal that was already decided or modified elsewhere.

`document_read` provides focused queries for files, headings, source sections, passages, comment threads, bibliography source, and changes since a checkpoint.

Candidate verification uses the browser's renderer on temporary source files. It does not apply the candidate to the collaborative document. A render result belongs to that candidate revision; unavailable renderers or a disconnected browser cannot produce a successful verification.

## Local state and session management

The runner keeps task continuity and writing preferences locally. The browser keeps its assistant history on the same device. The document server relays messages without storing transcripts. Reconnection reconciles known task IDs; it does not blindly repeat uncertain work. Use the panel's New conversation control to clear its history and start a fresh channel, and the sidebar's Stop control to detach the assistant.

See the [assistant protocol](https://github.com/LibrePaper/librepaper/blob/main/docs/protocol/chat.md) and [document operations](https://github.com/LibrePaper/librepaper/blob/main/docs/protocol/room-v2.md).

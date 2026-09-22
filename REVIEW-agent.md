# Review of LLM agent interaction

Implementation follow-up (September 21, 2026): the changes accompanying this review address the six findings below while preserving local agent execution, the chat relay, and constrained document tools. The sidebar now shows active and queued requests, Stop controls, accurate access descriptions, partial effects, and session boundaries. The findings below document the pre-fix working tree, not remaining defects.

Central validation: the Rust library suite passed (485 tests; 93 tests ignored), with the additional recovery regression passing in the assistant suite. Workspace Clippy, formatting, targeted JavaScript tests, and both assistant browser suites passed. The optional Svelte type check reports the same 446 errors as unchanged main, with no new diagnostics.

I’d keep the overall architecture: **local agent execution, a lightweight chat relay, and document changes through constrained MCP tools**. The strongest improvements would come from making task intent, execution state, and effects consistent across those boundaries.

The foundations are good: immutable source handles, candidate verification without modifying the live document, bounded queues, credential separation, and receipt-based suggestion attribution. But several integration gaps currently undermine those protections.

This review covers the working tree as inspected on September 21, 2026, including existing uncommitted changes. No implementation files were modified during the review.

The findings and recommendations were checked again against the working tree on the same date. All six findings below remain supported; their fixes have been narrowed where the original wording overlooked existing behavior or promised more than the current contracts can provide.

## Issues to fix first

### 1. [P1] Ordinary selections cannot enable Tighten or Rewrite

The Reader now captures a rendered quotation without source identity or revision. The assistant still considers a selection editable only when it has those older fields. Consequently, normal Reader selections fail the assistant’s capability checks.

I reproduced this using the Reader’s actual selection-capture function and the production assistant helpers.

There is a related revision mismatch in comment actions: `askCommentAssistant()` supplies an annotation’s `source_sequence` as `revision`, while the MCP selection reader compares that value with a tree digest. Source-anchored comment tasks can therefore fail before reaching the model.

**Fix:** connect the assistant's selection gating to the existing server-side resolution path: `prepare_task_context()` already calls `document_read`, whose `locate_selection()` resolves rendered quotations against an immutable view without trusting a browser-supplied source path. Preserve and validate the captured render identity through this path, and resolve comment context from its server-owned anchor. Distinguish `render_digest`, `source_sequence`, `comment_version`, and `tree_digest` explicitly; do not pass a source sequence as a tree digest.

References: [Reader selection capture](web/src/components/Reader.svelte#L543), [comment task preparation](web/src/components/Reader.svelte#L566), [attachment requirements](web/src/lib/assistant.js#L25), [existing task preparation](crates/librepaper/src/assistant/runtime.rs#L190), [revision comparison](crates/librepaper/src/server/mcp.rs#L672), [existing selection resolver](crates/librepaper/src/server/mcp.rs#L852).

### 2. [P1] Cancellation reports termination before the old agent has been dropped

On cancellation timeout, the runner reports that the agent process was terminated, then executes:

```rust
agent = acp::Agent::start(...).await?;
```

The existing `agent` remains alive while the replacement initializes. That initialization can take up to 60 seconds. The execution-epoch file also remains available during this interval.

This creates a window where the user has been told execution stopped while the previous agent can still be running.

**Fix:** explicitly shut down and await the old agent before starting its replacement. Fence further document operations during shutdown. Refresh the journal and report confirmed effects alongside unresolved operations; stopping the local agent does not establish the outcome of remote operations already in flight. Do not report process termination before it is confirmed.

Reference: [cancellation timeout handling](crates/librepaper/src/assistant/runtime.rs#L470).

### 3. [P2] A restarted task can remain “Working” indefinitely in the browser

Transient activity updates increment `event_seq` without persisting it. Restart recovery increments the last persisted sequence once. The browser rejects any sequence less than or equal to one it already received.

For example: persisted sequence 2 → transient sequence 4 → crash → recovery sequence 3. The browser discards the interruption.

I reproduced that rejection in the production browser client.

**Fix:** separate transient activity from durable task revisions, or order events by a persisted runner generation plus sequence. Terminal recovery must supersede activity from the previous generation.

References: [transient sequencing](crates/librepaper/src/assistant/runtime.rs#L143), [restart recovery](crates/librepaper/src/assistant/task.rs#L230), [browser ordering](web/src/lib/agent-client.js#L388).

### 4. [P2] The effect summary misses important outcomes

Several paths weaken the promise that the UI reports what actually happened:

- Results are assembled only for `EndTurn`; cancellation, refusal, and failure omit already-confirmed effects.
- The browser’s synthetic cancellation/failure message drops `context.results`, even if the task event includes it. I reproduced this.
- `effects.confirmed` contains suggestion IDs only, excluding successful applications, comments, and refinements.
- Independent batches put failures inside `items[]`, but the journal’s refusal decoder checks only top-level errors. A partially failed batch can appear cleanly finished.

**Fix:** use one effect decoder and one finalization path for every terminal outcome. Track typed effects and per-item outcomes, independently of whether the model completed normally.

References: [turn finalization](crates/librepaper/src/assistant/runtime.rs#L658), [journal decoding](crates/librepaper/src/assistant/journal.rs#L56), [batch response](crates/librepaper/src/server/mcp/operations.rs#L322), [browser terminal messages](web/src/lib/agent-client.js#L401).

### 5. [P2] The access choices imply behavior that never reaches the runner

“Comment” and “Track changes” both resolve to commenter access. The connection stores the selected mode, but assistant startup extracts only its link. The runner receives no behavioral distinction.

Conversely, “Edit” promises direct changes, while the bundled instructions still default to suggestions even with an editor link.

**Fix:** separate the permission ceiling from the requested behavior. Either collapse equivalent choices or pass an explicit mode through startup and task execution.

References: [access choices](web/src/components/reader/Agent.svelte#L78), [startup drops connection mode](crates/librepaper/src/local/assistant.rs#L39), [suggestion default](skills/librepaper-write/SKILL.md#L24).

### 6. [P2] Conflicting instructions are being injected into actual model sessions

These are operational conflicts, beyond stale documentation:

- The writing skill recommends atomic batches for coherent changes without distinguishing publication modes. Publishing multiple suggestions atomically is refused; atomic multi-patch private candidates remain supported and can be applied when authorized.
- The skill says the runner supplies a structured final-answer schema; the runner explicitly requests prose.
- The skill recommends recovering uncertain writes with `document_result`; operation lookup cannot return a retained receipt.
- The runner says to stop on every tool refusal, while other instructions explain how to recover from expired views and epochs.

**Fix:** align the bundled skill with the runner's prose-only final answer and receipt-derived effects. Document independent suggestion publication separately from atomic private staging. Define recovery by the operation's state as well as its error: rereading can refresh context, but must preserve the authorized occurrence and must not trigger a new mutation after an uncertain write. Operation lookup currently returns `outcome_unknown`; candidate/render lookup is a separate supported path. Preserve uncertainty when authoritative evidence is unavailable, and stop on revoked authority.

References: [injected instructions](crates/librepaper/src/assistant/context.rs#L12), [bundled writing rules](skills/librepaper-write/SKILL.md#L26), [atomic publication refusal](crates/librepaper/src/server/mcp/operations.rs#L852).

## Architectural priorities

These are design options, not prerequisites for fixing the confirmed defects above.

### 1. Make task intent a real execution contract

Today, the system has document permissions, task kinds, scopes, and prose instructions, but they do not form one enforceable contract.

I would introduce an internal task specification along these lines:

```js
{
  taskId,
  kind: "tighten",
  scope: { viewId, rangeIds },
  effects: {
    suggest: true,
    apply: false,
    comment: false
  },
  validation: "source"
}
```

The browser expresses intent; the server resolves its scope; the runner supplies model context; document operations enforce the allowed effects and mutation scope.

This would make “Explain” reliably read-only and “Tighten this passage” reliably confined to that passage, while still allowing broader context reads. It would also simplify prompts: they could concentrate on writing quality instead of repeatedly explaining execution rules.

### 2. Make uncertain outcomes resolvable

This is the highest-value robustness investment.

The server deliberately retains admission records without committed operation receipts. That prevents blind reexecution, but a dropped response leaves the user with uncertainty that `document_result` cannot resolve. The code explicitly returns `outcome_unknown`.

I understand the motivation to avoid maintaining another effect log. I would first investigate exposing outcomes through existing domain command identities and document history. Where that is insufficient, retain a compact outcome record transactionally with the mutation:

- Operation identity and request digest.
- Terminal status.
- Effect IDs and resulting revision.
- Structured refusal information.

This does not require storing chat transcripts. It would let recovery answer “three suggestions were created” instead of “inspect the document.”

Reference: [operation lookup behavior](crates/librepaper/src/server/mcp/operations.rs#L996).

### 3. Give the runner one explicit task state machine

State is currently spread across `Task`, `Active`, admissions, journal entries, transport queues, and browser task/message records. Several findings above arise because one path updates only some of those representations.

I would extract a reducer:

```text
current state + event → next state + effects to execute
```

Events would include admission, dispatch, activity, permission request, tool outcome, cancellation, connection loss, and agent exit. Persistence and transport would execute the reducer’s outputs.

The practical benefits:

- Every terminal path produces an effect summary.
- Recovery ordering becomes explicit.
- Cancellation ordering can be represented and tested explicitly; the process shutdown and execution fencing still need to be implemented.
- Tests can exercise lifecycle transitions without launching an LLM.
- Blocking context reads and agent initialization can run outside the control loop, keeping cancellation responsive.

I would retain the separate transport worker; that separation is useful.

### 4. Establish readiness and conversation continuity explicitly

The startup preflight proves the runner can reach the document endpoint. It does not prove the selected ACP agent actually loaded the MCP tools. Session creation is currently sufficient to signal ACP readiness.

Use observable startup stages: agent launching, session initialized, document bridge connected, tools discovered. Only advertise full readiness after the required stages succeed.

Restarts also deliberately create fresh model sessions while preserving the browser transcript. That is a reasonable compatibility decision, but it creates a misleading conversational experience: the user sees previous exchanges that the model cannot remember.

On restart, either supply a bounded continuation summary or show a visible conversation boundary. A continuation summary should include relevant requests, confirmed effects, outstanding questions, and preferences; old source handles should be refreshed.

References: [startup and fresh-session rationale](crates/librepaper/src/assistant/runtime.rs#L341), [ACP readiness](crates/librepaper/src/assistant/acp.rs#L199).

## User-experience improvements

- **Stream the answer.** ACP chunks already arrive, but the runner buffers them until completion. Send throttled updates with stable task/message identity. Preserve partial answers on interruption and indicate truncation instead of silently cutting at 32 KiB. [Current buffering](crates/librepaper/src/assistant/runtime.rs#L620).
- **Expose Stop task, queued work, and New conversation.** The client has cancellation and conversation-ending methods, but the panel does not call them. Show the active task, queued requests, and their controls together.
- **Show useful activity.** The adapter reduces tool calls and plans to generic strings, and the panel usually shows only “Working.” Display safe summaries such as “Reading introduction” or “Checking candidate compilation,” plus elapsed time.
- **Make follow-up behavior explicit.** Requests submitted during execution are queued; steering is advertised as unsupported. Label this “Queue next request” so “actually, leave the citations alone” is not mistaken for an immediate correction.
- **Restore the actual running configuration.** After remount, browser history resumes, but selected agent/access and local runner identity are not restored. `assistantStatus()` can establish whether a known connection/conversation is running, but its status contains a configuration hash rather than the selected agent and access mode. Full restoration needs the connection identity and non-secret configuration metadata to be retained or exposed explicitly. Otherwise the UI can offer a restart without explaining what is already running.
- **Improve permission cards.** They currently retain the tool title and option labels, discarding richer operation information. Show bounded command/file/diff details when available. Preserve permission-option kinds rather than visually emphasizing whichever option appears first. [Permission translation](crates/librepaper/src/assistant/acp.rs#L171).
- **Add a visible Send button.** Keyboard submission is useful, but a button improves discoverability, touch use, and accessibility. [Composer](web/src/components/ChatComposer.svelte#L83).

## Smaller simplifications

- Replace string task statuses with the existing enum throughout the runner.
- Preserve structured error codes and recovery actions; avoid collapsing unrelated read failures into “selection is stale.”
- Bound final effect summaries to the chat context budget, with counts and a way to retrieve details.
- Centralize duplicated wire vocabulary and limits, with shared contract fixtures across Rust and JavaScript.
- Move operation-key bookkeeping away from the model where practical. Asking an LLM to construct timestamped random identifiers adds avoidable failure modes; preserve explicit identity for retries.
- Split `Agent.svelte` into a controller and smaller setup, task, and permission views.
- Render assistant answers with a restricted Markdown renderer, retaining the existing safe handling of arbitrary text.

## Validation and implementation order

The five focused JavaScript test files passed:

- `web/tests/unit/agent-client.mjs`
- `web/tests/unit/assistant.mjs`
- `web/tests/unit/assistant-review.mjs`
- `web/tests/unit/reader-assistant.mjs`
- `web/tests/unit/assistant-preview.mjs`

The recheck reran all five files successfully with `node --test` and independently reproduced three problems against production browser modules: selection gating, rejection of recovery sequences, and loss of cancellation results in transcript messages. Rust findings were checked by tracing the implementation and existing tests; no Rust tests, live ACP agent, or PostgreSQL integration tests were run for this recheck.

The most valuable additional test would be a deterministic fake ACP executable exercising the real runner and document service. Cover cancellation after a committed suggestion, crash after transient progress, partial batch failure, missing MCP tools, comment refinement, and restart followed by a context-dependent follow-up.

My order would be:

1. Repair selection/revision contracts and cancellation ordering.
2. Unify effect reporting and fix recovery sequencing.
3. Make operation outcomes recoverable.
4. Add streaming, task controls, and configuration restoration.
5. Introduce the explicit task contract and extract the state machine.

I would request changes on the selection and lifecycle issues before treating the assistant as dependable. The underlying design is worth preserving; the main work is making its promises hold across the full interaction.

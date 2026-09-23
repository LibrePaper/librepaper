# Future agent interaction work

## Priorities

### Enforce task scope and permitted effects

Introduce a server-enforced task contract that binds the task identity, resolved source scope, allowed effects, and validation requirements. Resolve browser intent against immutable document views, pass the contract through the runner, and enforce it at document operations.

An Explain task should permit reads but no mutations. A passage-level Tighten task should allow suggestions only within the authorized passage while allowing broader context reads. Refreshing expired handles must preserve that scope. Document access remains the permission ceiling; task intent can narrow it further.

Cover attempts to widen the selected range, apply a suggestion without authorization, create comments during a read-only task, or reuse a contract for another task or document.

### Exercise the runner with a deterministic ACP agent

Build a fake ACP executable that drives the real runner and document service without an external model. Cover:

- Cancellation after a committed suggestion and before its response reaches the runner.
- Crash and restart after streamed answer and activity events.
- Independent batches containing committed, refused, and uncertain items.
- Missing MCP tools, delayed discovery, and stale readiness markers.
- Permission requests, comment refinement, and transport loss.
- Restart followed by a context-dependent follow-up, checking the model session boundary.

Assert durable task state, process shutdown, operation recovery, and the browser-visible event sequence together.

## Conditional architectural work

Consider extracting a task-state reducer if the integration harness exposes repeated lifecycle inconsistencies or new features make transitions difficult to maintain. Model admission, dispatch, activity, permission requests, tool outcomes, cancellation, connection loss, and agent exit as explicit events. Keep persistence and transport as effects of transitions, and keep slow context reads and agent initialization outside the responsive control loop.

Consider opt-in conversation continuation after restart. A bounded summary should carry relevant requests, confirmed effects, outstanding questions, and preferences. Refresh source handles and preserve uncertainty about unresolved writes before supplying the summary to a fresh model session.

## Optional usability and maintenance work

- Show bounded, useful activity summaries and elapsed time, such as reading a section or checking candidate compilation.
- Preserve structured read-error codes and recovery actions through task preparation.
- Use the task-status enum throughout the runner.
- Share wire-contract fixtures and limits across Rust and JavaScript.
- Move operation-key generation into trusted tooling while retaining explicit retry identity.
- Split the assistant panel into a controller and smaller setup, task, and permission views when further UI changes justify it.
- Render assistant answers with a restricted Markdown renderer that safely handles untrusted text.

Start with the task contract and ACP integration harness. Use their findings to decide whether the conditional refactors justify their cost.

# Agent instructions

These rules apply to every coding agent working in this repository, Claude and Codex alike.

## Work in a git worktree

Never edit code in the main checkout. Every code change happens in a git worktree on its own branch, created with `git worktree add` (or the agent's built-in worktree isolation). This includes subagents: each one gets its own worktree so parallel work never collides. Never use `git stash` to move work around; use a worktree or `git diff`.

## Orchestrate, do not implement

The main agent plans, delegates, reviews and verifies. It does not write the first draft of the code.

1. **Split the work.** Break the task into independent pieces with clear file boundaries and a precise brief for each: what to change, where, and what done looks like.
2. **Implement with cheap subagents in parallel.** Hand each piece to a subagent on the cheapest capable model (Haiku under Claude, the smallest mini model under Codex). Launch independent pieces at the same time, each in its own worktree.
3. **Review centrally.** When the subagents finish, the main agent reads every diff itself: correctness, consistency across pieces, and fit with the surrounding code. Merge the pieces together only after this review.
4. **Run checks centrally.** Only after review does the main agent run tests, builds, linters and type checks, once, on the combined result.
5. **Delegate fixes.** Problems found in review or checks go back to cheap subagents with a specific brief, again in parallel where possible. Repeat review and checks.
6. **Fix it yourself only at the very end.** The main agent edits code directly only for the last small corrections, once delegation has stopped paying off.


## Edge cases and over-engineering

Before adding complexity for a speculative or vanishingly unlikely race or security edge case, explain the concrete failure mode, likelihood, and complexity cost to the user and get their buy-in. Do not silently expand scope for theoretical robustness.

## Subagents do not run tests or checks

Subagents must not run tests, builds, linters, type checkers, formatters that rewrite other files, or any other verification command. They write code and report what they changed. Say so explicitly in every subagent brief. All verification belongs to the main agent after review.

## Run only the checks your change needs

The Rust code is a workspace of internal sub-crates under `crates/`.

Between phases and before handing back, run only the tests and checks that match the files you changed. Do not run the full suite by habit.

- One Rust crate under `crates/`: run `cargo check -p <crate>`, `cargo nextest run -p <crate>`, and `cargo clippy -p <crate> --all-targets -- -D warnings`. When the crate's public API changed, run the same commands for each crate that depends on it.
- Several crates, or `Cargo.toml` or `Cargo.lock`: run `make test-rust`.
- Code under `web/src` or `web/tests`: run the affected test files with `node --test <file>` from `web/`. Unit and integration files need `--experimental-transform-types`. When `.svelte` or `.js` types changed, also run `cd web && bun run check:types`. `make test-web` covers all of it.
- A browser behavior change: run the relevant `web/tests/browser/*.mjs` file. No `make` target runs these.
- A script under `tools/` or `web/tools/`: run its own `*.test.mjs` with `node --test <file>`.
- Documentation or prose only: run nothing.

Run `make test` only when one change spans both the web and Rust halves, or when it touches shared build inputs such as the Makefile, the pins, or the wire contract between the browser and the server.

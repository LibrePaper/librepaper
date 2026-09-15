# What is left

Written 2026-09-15. `SPEC-loro.md` §6 is the record of the migration itself;
this is only the queue of what has not been done.

## Done since the last version of this file

**The client→server proposal round trip has coverage.**
`room::proposal_round_trip_tests` drives a real `Room` against a real
catalogue through open → update → decide → resolve, with a second writer
typing throughout. Four cases, `#[ignore]`d like the rest of the Postgres
coverage:

```sh
LIBREPAPER_TEST_POSTGRES_URL=... \
  cargo test -p librepaper --lib proposal_round_trip -- --ignored --test-threads=1
```

**`proposal-open` honours the base it is sent.** §5.1 said the message carries
the branch's base frontier; `open_proposal` recorded the room's own
`state_frontiers()` instead, so between an author's fork and their open,
anybody else's commit moved the proposal's recorded fork point. It now takes
the client's base, forks at it as the check that it can, and refuses an
unreachable one (`UnknownBase`, `retry: true`) rather than substituting its
own. The client commits before reading its frontier, so the base names only
operations already sent ahead of the open on the same socket.

Worth being accurate about the severity, having claimed worse earlier: this
was a divergence from the spec and from where the author is actually typing,
**not** a demonstrated case of lost or misattributed text.
`declining_a_proposal_does_not_revert_a_concurrent_writer` reproduces the race
and passes either way -- Loro's merge converges, because both sides'
operations are concurrent and both stay in the graph.

## What is actually left

1. **Three §9 follow-ups**, still deliberately deferred: evaluate `LoroTree`
   to collapse `files` + `paths`; reconsider checkpoint density now that
   contract 6 makes every intermediate state reachable; and delete
   `max_encoded_snapshot_bytes`'s speculative-encode machinery.

2. **The fork has not been sent upstream.** `web/vendor/loro-codemirror/`
   carries five fixes, and its README says what to delete when a release
   contains them. Pushing to someone else's repository is yours to decide.

3. **The browser tests are not in `make test`.** They are `make browser`, and
   the Makefile says why. Nothing is wrong with that, but a green `make test`
   is not "everything passes" and has been read that way at least once.

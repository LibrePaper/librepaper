# What is left

Written 2026-09-15. `SPEC-loro.md` §6 is the record of the migration itself;
this is only the queue of what has not been done.

## Done since the last version of this file

**The three §9 follow-ups are settled.** Two were questions and are now
answered in the spec rather than left standing; one was code and is deleted.

- **§9.3, `LoroTree`: no.** The collapse would buy the identity-preserving
  rename that two maps already give. What a tree is uniquely good at is
  resolving concurrent *moves*, and there is no move here to resolve --
  `rename_path` writes one string, and directories exist only as text inside
  path strings. Adopting it means modelling directories and migrating the wire
  format across three implementations to arrive at today's behaviour. Revisit
  if moving a directory becomes something a user can do.
- **§9.4, checkpoint density: unchanged.** Contract 6 does make every
  intermediate state reachable, but *through the Loro oplog*; archives are what
  make recovery format-independent (contract 8), so thinning them thins the one
  record that does not depend on the CRDT. §7.3 also has an archive fetch at
  0.08 ms against 3.9 ms to check out an old version. The cadence is already
  bounded at both ends.
- **§9.5, speculative-encode machinery: deleted.** `checked_edit` had already
  stopped speculating; what remained was `Session::encoded_bound`, written in
  four places and read in none, with `admitted_bound` and `repaired_bytes`
  feeding it and nothing else. The `max_encoded_snapshot_bytes` ceiling stays
  -- it is §3.6's escape valve, enforced against a real encode.

**A green `make test` no longer reads as "everything passes".** It now ends by
naming what it did not run, and `make check-all` runs the suite and the browser
components together. The Postgres cases stay out of both: they want a database
to point at and they TRUNCATE it.

## What is actually left

1. **The fork has not been sent upstream.** `web/vendor/loro-codemirror/`
   carries five fixes, and its README says what to delete when a release
   contains them. Pushing to someone else's repository is yours to decide.

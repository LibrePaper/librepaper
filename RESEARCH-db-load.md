These are directions in which we could investigation to reduce DB loads.

• Agreed. If 1,000 continuously active editors is a real target, a 500 ms durable commit cadence is probably the wrong default. The biggest savings require changing the write cadence or
  relaxing commit-before-relay—not micro-optimizing PostgreSQL.

  The strongest knobs, in order:

  1. Increase the batching window dramatically

  For 1,000 independently edited documents:

   Maximum batch interval    Approx. commits/sec
  ━━━━━━━━━━━━━━━━━━━━━━━━  ━━━━━━━━━━━━━━━━━━━━━
   500 ms                                  2,000
  ────────────────────────  ─────────────────────
   2 seconds                                 500
  ────────────────────────  ─────────────────────
   5 seconds                                 200
  ────────────────────────  ─────────────────────
   10 seconds                                100
  ────────────────────────  ─────────────────────
   30 seconds                                 33

  Moving from 500 ms to 10 seconds cuts transaction frequency by about 95%. Local typing remains instantaneous; collaborators see changes later.

  A good policy would be:

  - Flush after 1–2 seconds of inactivity.
  - Enforce a maximum 10–15 second delay during continuous typing.
  - Immediately flush at semantic barriers: comments, proposals, checkpoints, explicit save, and disconnect.

  This could potentially bring the database into the hundreds-to-low-thousands-per-month range rather than many thousands, depending on provider and workload.

  2. Keep immediate relay, but acknowledge only after durability

  This is the most consequential architectural option:

  - Apply and relay ordinary CRDT edits immediately.
  - Persist them in 10–30-second batches.
  - Report “synced” only after PostgreSQL commits.
  - Clients retain unconfirmed operations locally and resend them after reconnect.
  - Comments, proposal decisions, restores, and other semantic commands atomically commit any source operations they depend upon.

  The spec explicitly acknowledges this as a valid alternative contract, although its current target chooses commit-before-relay. Revising that decision would retain responsive
  collaboration while reducing typing commits by roughly 15–60×.

  Its cost is architectural complexity: the server again has accepted-but-not-yet-durable state. That requires well-tested restart/reupload behavior and honest durability indicators. But
  if the cost constraint is firm, this is likely the best overall tradeoff.

  3. Batch by document, not editor

  A hundred editors typing in one document should produce one document commit per interval—not one hundred.

  The document owner should:

  - Collect all causally compatible updates during the window.
  - Validate contributors independently.
  - Commit one combined Loro delta.
  - Return durable vector coverage to every contributing client.

  This is already compatible with the proposed ownership model and makes collaborative hotspots much cheaper.

  4. Use adaptive batching

  One fixed interval is inefficient. The server can vary the window based on load:

  - Low load: 0.5–2 seconds.
  - Moderate load: 5 seconds.
  - High load: 10–15 seconds.
  - Database stress: bounded 30-second maximum, with explicit “sync delayed” status.

  This preserves low latency when capacity is available without provisioning for the worst-case 500 ms rate. The behavior should be one documented commit model, not a hidden durability
  fallback.

  5. Avoid complete-history work on every batch

  The spec’s initial implementation allows a full encoded-history measurement for every candidate, but that will become expensive for long-lived documents.

  Use:

  - Incremental current-source and file-count accounting.
  - Conservative accumulated-history bounds.
  - Exact full-history measurement only near a quota boundary.
  - Cached immutable measurements keyed by committed frontier.
  - Bounded validation of incoming update structure before import.

  Final admission remains exact where necessary, but ordinary keystrokes stop repeatedly exporting the entire operation graph.

  6. Tune compaction for write reduction

  Compaction can amplify the original write load. Useful adjustments include:

  - Compact based on accumulated bytes, not merely row count.
  - Avoid compaction during every small-update burst.
  - Run it at controlled concurrency.
  - Build bases from immutable committed cuts.
  - Deduplicate identical archive outputs.
  - Keep source-revision metadata small and separate from update bytes.

  Compaction should reduce read/recovery costs without becoming a second continuous writer.

  7. Reduce PostgreSQL work per commit

  Each source transaction should perform only the necessary operations:

  - One document-row update.
  - One update-log insert.
  - One revision-index insert.
  - Receipt insertion only for semantic commands, not typing.
  - Compaction scheduling only when crossing a threshold.
  - No eager comment reattachment.
  - No archive or rendered-artifact write.

  Prepared statements, a sensible connection pool, WAL compression, and restrained indexes help, but these are secondary to transaction frequency.

  8. Separate typing durability from semantic durability

  Not every event needs the same latency:

  - Typing: batch aggressively.
  - Comments and proposal decisions: commit immediately and atomically with required source.
  - Presence: never persist.
  - Rendering notifications: never persist.
  - Anchor-position caches: compute on demand.
  - Checkpoint archives: asynchronous.

  This prevents expensive immediate transactions from becoming the default for all activity.

  My recommendation would be to revise the spec to one of these two profiles:

  - Cost-first, responsive collaboration: immediate relay, 10–30-second durable batching, durable acknowledgements, mandatory client persistence/reupload.
  - Strict commit-before-relay: adaptive 2–10-second batching, accepting visibly slower remote collaboration.

  For 1,000 continuously active documents, I would not ship 500 ms commit-before-relay. A 10-second maximum reduces the theoretical rate from 2,000 to 100 commits/sec; a 30-second
  durability cadence reduces it to about 33/sec. Those are qualitatively different infrastructure requirements.

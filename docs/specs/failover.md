# SPEC: failover and recovery

## Scope

KomoDoc has local and hosted profiles built on the same batching code. Their
durability differs:

- Local process recovery uses the deployment disk; disk-loss recovery restores
  a matched SQLite, object and secret backup.
- Hosted recovery uses the authoritative Turso catalogue and R2 objects.

Both begin with one active server, automatic process restart and manually
controlled recovery. Automatic cross-host takeover applies to the hosted
profile and must retain one active writer. Recovery-time targets and standby
costs remain unset until measured.

## Guarantees

A saved edit has durable segment data and a committed catalogue reference.
Local mode promises recovery from the deployment disk and its completed
backups; if the deployment disk is lost, changes since the last completed backup
are outside that guarantee. Hosted mode
promises that losing the application host does not lose a saved edit. Relayed
but unconfirmed browser edits may need retry and are never labelled saved.

Hosted recovery must use Turso, R2 and externally supplied credentials and
secrets; the failed host's disk and cooperation are unavailable. Public reads
may continue when safe. Protected operations fail retryably when authorization
or writer ownership cannot be established.

Restoring an older catalogue in either profile requires one of the named
complete recovery points in [catalog.md](catalog.md#secrets-and-recovery), with
independent object copies and both matching secret versions. An arbitrary Turso
restore position does not establish recoverability of its referenced R2 data.
The document recovery window and backup retention are separate policies.

## Fencing

For local recovery, stop the old process and acquire the deployment's OS lock
before opening or restoring its files. Automatic cross-host promotion is not a
local-profile feature.

For hosted recovery, the hard problem is preventing an old server from writing
after a replacement starts. An OS lock protects one host only. A health check,
timeout or belief that a VM stopped is not fencing.

Store an active writer generation at the Turso primary. Before automatic
takeover, every catalogue mutation and journal publication must validate that
generation inside its primary transaction. On an uncertain lease or commit,
stop publishing and reconciling acknowledgements until ownership is known.

SQL fencing does not stop an old cleanup worker from deleting R2 objects. The
handoff must also fence destructive bucket access, including requests already
in flight. Acceptable designs may use confirmed per-host credential revocation,
verified host termination or a separately fenced deletion service. The chosen
method must pass failure tests before automatic promotion is enabled.

## Hosted recovery path

1. Detect that the active server is unavailable.
2. Fence its Turso mutations and destructive R2 access.
3. Start or promote one replacement with a new writer generation.
4. Read current primary metadata and reconcile ambiguous publications,
   lifecycle operations and reservations.
5. Rebuild the bounded journal manifest and tail, then admit writes.
6. Complete primary-to-replica synchronization before protected reads. Reconnect
   clients and revalidate account sessions and document access before accepting
   queued CRDT updates. A closed retry epoch requires resynchronization against
   the preserved CRDT before sending updates under the current epoch.

A warm standby may keep disposable caches and a read replica current, but it
must still reconcile against the Turso primary before serving writes. Shared
Turso or R2 outages cannot be repaired by starting another application server;
affected operations remain suspended until the store recovers.

## Release stages

The initial release ships supervised restart and tested recovery runbooks for
both profiles. Local tests recover committed WAL/object state after restart and
restore a matched catalogue, object and secret backup after disk loss.
Hosted tests recover on a fresh host. The hosted format preserves writer
generations and immutable journal keys so automatic takeover does not require a
storage migration.

The next stage automates replacement only after all gates below pass. A warm
standby is an optimization added when measured recovery time justifies its
compute and routing cost.

## Automatic-takeover gates

- Resume a presumed-dead server after promotion and prove that its catalogue
  mutations and journal publications fail.
- Delay an old R2 deletion until after promotion and prove that current recovery
  data survives, including failed credential revocation and network partitions.
- Recover every acknowledged sequence after host loss and reconcile lost commit
  responses without duplicate acknowledgement.
- Resume interrupted creates, replacements, deletions, journal retirement and
  capacity reservations, including maintenance borrowing at full ordinary quota.
- Restore a named recovery point after normal zero-window reclamation has
  removed its original live objects; verify source state and sealed links.
- Reject reconnecting clients whose access was revoked during the outage.
- Recover on a fresh host at the 1,000-active-document target and publish
  measured detection, fencing, startup and first-serve times.
- Include standby, routing and recovery traffic in the measured cost.

Until these pass, host replacement is manual. A visible outage is preferable
to two writers or silent loss of saved work.

Automatic local-profile failover, simultaneous application writers, geographic
redundancy and guaranteed recovery of unconfirmed browser edits are out of
scope.

[catalog.md](catalog.md) defines catalogue security and lifecycle rules.
[persistence.md](persistence.md) defines the journal and save guarantee.
[sync.md](sync.md) defines client synchronization.

# 12. Remote backup ownership

Status: implemented for single-authority deployments; concurrent multi-host
callers remain unsupported. Inherits
[umbrella section 12](../../../SPEC-refactor.md#12-make-remote-backup-creation-and-cleanup-mutually-exclusive).

## Decision and scope

The required property is that cleanup cannot delete objects belonging to a
creator that can still publish a completion marker. A final absence check cannot
establish that property because publication can occur after the check.

### Who can call the API, and what the writer lock guarantees

The blob-backup API (`create_backup`, `remove_incomplete_backup`) has no
production callers today. Its possible callers are the deployment's own
processes: the server (`server/serve.rs`, which takes
`DeploymentPaths::writer_lock` at startup and hands it to the room set),
the command line (`storage/backup.rs`'s local backup path and seeding, which
take the same lock), and any future maintenance or HTTP endpoint inside one of
those two processes. All of them address one deployment directory and one blob
namespace, `recovery/`.

The deployment writer lock is an `fs2` exclusive lock on
`<state>/writer.lock`. It guarantees exactly one holder for one deployment
directory on one host, and the operating system releases it when the holding
process exits, including on a crash. It cannot constrain an independent process
on another host writing the same bucket, and it is not a lease: it carries no
fencing token that the object store would check.

### Supported deployment assumption

One authority per deployment namespace, which is what the rest of the roadmap
already assumes: the writer lock, the catalogue-owned journal gate, and the
local CLI's offline lock all describe a single writing process per deployment.
Under that assumption the writer lock is an enforceable ownership boundary for
the backup namespace, so this track extends it rather than inventing a second
mechanism. Two processes on different hosts writing one bucket concurrently is
outside what this lock can express and stays unsupported; it is not approximated
with a timeout, a heartbeat, or a last-minute `NotFound` check. Distributed
conditional claims and fencing, with their own crash and stale-owner recovery,
remain a separate protocol proposal.

### The capability

`BackupOwnership` is evidence of that exclusivity rather than an assertion of
it. It can only be produced by `BackupOwnership::acquire`, which takes the
deployment writer lock, or by `BackupOwnership::adopt`, for a process that
already holds that lock. It holds the lock file for its whole lifetime, is not
`Clone`, and is borrowed by `create_backup` and `remove_incomplete_backup`, so
no caller can start remote creation or cleanup without it. Its scope is the
`recovery/` namespace of one deployment; its lifetime is the lifetime of the
value, which spans private object writes, completion publication, and cleanup.

One owner may legitimately run several backups at once, so ownership also holds
a set of backup ids with an operation in flight. A create and a cleanup of the
same id therefore cannot interleave inside the owning process either; the
second caller is rejected with an error rather than queued, because waiting
behind work it cannot observe would hide the exclusion it depends on.

The local CLI path keeps its offline lock: it now acquires the same capability,
so there is one lock and one concept rather than two. The completion marker
stays an immutable create-only `swap(key, body, "")`, and a transient read
failure is still not object absence: both the creation precondition and the
cleanup completion checks fail closed on any error other than `NotFound`.

### Crash and stale-owner behaviour

A crashed owner cannot resume: the process is gone, and with it the `File` and
the flock. The lock the operating system released is the only thing a recovering
owner needs, and it must take it before touching anything — reclamation happens
through `remove_incomplete_backup`, which requires the capability. Abandoned
private objects of an incomplete backup are ordinary garbage until then, because
a backup is advertised by its manifest and never by copied objects. A completed
backup is never reclaimed: the manifest check rejects it as immutable.

The case this lock cannot cover is a second writer on another host. It is not
approximated here; that deployment shape stays unsupported.

## Acceptance and delivery

- Deterministic barriers interleave creation, cleanup, and competing creation
  around final manifest checks and publication; the ownership boundary excludes
  unsafe interleavings rather than merely reducing their probability.
- Completed backups remain verifiable and unchanged.
- Crash recovery establishes exclusive ownership before reclaiming abandoned
  attempts; transient reads preserve potentially complete backups.
- API callers cannot initiate remote creation/cleanup without acquiring the
  required ownership capability. Document supported deployment assumptions.

## Implementation evidence

Before, `create_backup` and `remove_incomplete_backup` took only a blob store
and a backup id and documented an external serialization obligation the API
could not enforce; cleanup's protection was a `NotFound` check that a creator
could invalidate immediately afterwards by publishing. After, both take
`&BackupOwnership`. The signature is the enforcement: the compiler rejects a
call without it, and the only ways to obtain one are taking the deployment
writer lock or adopting a lock already held. `create_local_backup` now acquires
the same capability in place of its private `acquire_offline_lock` call, with
identical locking behaviour.

`storage::backup::tests::ownership_excludes_cleanup_and_competing_creation_at_publication`
holds a creator inside the window between its last private object write and its
completion marker, using a blob store that blocks in `swap` of the manifest key
until the test releases it. In that window a cleanup and a competing creator for
the same id both run: both are rejected, the creator then publishes, the backup
verifies, and its copied object still holds the original bytes. Without
ownership this is exactly the interleaving in which cleanup observes `NotFound`
and deletes objects that a completing backup is about to claim.

`completed_backup_is_unchanged_by_a_cleanup_attempt` shows a published backup
rejected as immutable and still verifiable.
`abandoned_attempt_is_reclaimed_after_ownership_is_reacquired` shows that a
second owner cannot exist while the first is alive, that dropping the first
(what the operating system does when a process crashes) releases the lock, and
that reclamation of the abandoned private object happens only through the
reacquired capability. `backup_ownership_is_unforgeable_and_exclusive` asserts
the exclusivity of acquisition and, through a probe whose inherent method is
selected only when the type is `Clone`, that the capability cannot be copied.
The two pre-existing transient-read regressions still hold: a failing manifest
read aborts creation before any copy and preserves the bytes of a possibly
complete backup instead of deleting them.

Limitations. Ownership is a process-level file lock, not a lease with a fencing
token, so it protects a single-authority deployment and nothing more; a second
writer on another host remains unsupported and undetected. Same-id operations
inside one process are excluded by rejection, not by queueing, so a caller that
wants to retry must do so itself. `BackupOwnership::adopt` trusts its caller to
pass the deployment writer lock and to keep it locked; the server does not yet
call the backup API at all, so no production path exercises it. The umbrella's
remaining work — distributed conditional claims, fencing, and stale-owner
recovery across hosts — is unchanged and still a separate proposal.

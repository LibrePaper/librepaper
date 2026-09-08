# 12. Remote backup ownership

Status: proposed, required before exposing the blob-backup API to concurrent
callers. Inherits [umbrella section 12](../../../SPEC-refactor.md#12-make-remote-backup-creation-and-cleanup-mutually-exclusive).

## Decision and scope

The required property is that cleanup cannot delete objects belonging to a
creator that can still publish a completion marker. A final absence check cannot
establish that property because publication can occur after the check.

Do not select an ownership-guard API until its acquisition mechanism is known.
First establish whether all supported callers share one enforceable deployment
lock and backup namespace. If they do, prefer extending that existing ownership
boundary and make the API require evidence of it. If independent processes can
write the namespace, a local guard is insufficient: either keep concurrent use
unsupported or design provider-enforced claims and fencing as a separate change.
An arbitrary caller-supplied token is not evidence of exclusivity.

Before implementation, specify who acquires that exclusivity, its scope and
lifetime, and how crashes release it without permitting an old writer to resume.
If the remote deployment cannot supply enforceable exclusive ownership, keep
that concurrent API unavailable; do not substitute a last-minute NotFound check.
Distributed conditional claims, fencing, and stale-owner recovery require a
separate protocol proposal.

Whichever mechanism is chosen must hold ownership across private object writes, completion publication, and any
cleanup of the same backup ID. Keep the local CLI's offline lock and immutable
create-only completion marker. A transient read failure is not object absence.

## Acceptance and delivery

- Deterministic barriers interleave creation, cleanup, and competing creation
  around final manifest checks and publication; the ownership boundary excludes
  unsafe interleavings rather than merely reducing their probability.
- Completed backups remain verifiable and unchanged.
- Crash recovery establishes exclusive ownership before reclaiming abandoned
  attempts; transient reads preserve potentially complete backups.
- API callers cannot initiate remote creation/cleanup without acquiring the
  required ownership capability. Document supported deployment assumptions.

Until the acquisition/recovery contract is implemented and tested, retain the
documented external serialization requirement and do not expose concurrent use.

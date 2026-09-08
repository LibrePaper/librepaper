# 7. Validated internal commands

Status: implemented on `refactor/commands`. Inherits [umbrella section 7](../../../SPEC-refactor.md#7-validate-commands-before-operation-specific-processing).

## Scope and implementation

Retain the current wire adapter and convert it to internal command variants
containing only each operation's required fields. Reject unknown discriminators
with a specific protocol error before charging the comment mutation allowance.
Validate required fields with operation-specific errors.

`room::Message` remains the serde-compatible wire shape. Its `into_command`
adapter creates `Command::{Comment,Reply,Resolve,Delete,Anchor,Accept,Reject}`
and carries `temp_id` and `request_id` through every error and success path.
The room and server dispatch paths consume the validated command variants;
they do not use the adapter as an unused parallel type. Raw fields used in
request digests are carried directly into the typed mutation arms, so durable
receipt identity and successful retries remain byte-compatible.

Malformed command validation runs after the existing frame-size and socket
abuse gates and after the existing authorization gate, but before room rate
accounting, checkpoint preparation, or mutation. Multipart Yjs assembly, chat
deduplication, and Yjs frame handling keep their independent bounded controls.
Unknown operation names retain the existing `unknown message type` text while
missing fields identify their operation, for example `reply requires
comment_id`.

Keep frame-size, connection, and malformed-traffic abuse limits before expensive
processing. Preserve existing receipt digests, successful retry behavior, and
correlation fields; internal validation must not silently recanonicalize a
persisted request identity. Coordinate error mapping with track 6.

## Acceptance and delivery

Ship adapter, dispatch, and mapping together without changing the public wire
format or requiring a client migration.

- Existing representative client frames decode with their current defaults.
- Unknown kinds do not consume mutation allowance; malformed traffic remains
  bounded by independent abuse controls.
- Missing/invalid fields identify the intended operation and retain request IDs.
- Oversized frames remain bounded; successful retries neither mutate nor charge
  twice. Test stored-receipt compatibility where validation changes processing.

Focused adapter tests cover unknown-kind correlation, operation-specific
required fields, field-preserving retry identity, malformed-anchor rollback
correlation, and the guarantee that an unknown command does not consume the
next valid comment's allowance. A retry with an empty reply body also follows
the established idempotent receipt path. Existing room and socket tests remain
the compatibility suite for persisted receipts, frame limits, and correlation
fields.

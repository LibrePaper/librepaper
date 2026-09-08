# 7. Validated internal commands

Status: proposed. Inherits [umbrella section 7](../../../SPEC-refactor.md#7-validate-commands-before-operation-specific-processing).

## Scope and implementation

Retain the current wire adapter and convert it to internal command variants
containing only each operation's required fields. Reject unknown discriminators
with a specific protocol error before charging the comment mutation allowance.
Validate required fields with operation-specific errors.

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

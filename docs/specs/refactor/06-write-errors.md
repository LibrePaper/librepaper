# 6. Explicit room write errors

Status: proposed. Inherits [umbrella section 6](../../../SPEC-refactor.md#6-make-write-refusal-and-error-handling-explicit).

## Scope and implementation

Introduce typed distinctions for fenced/read-only, permission denied, quota
exceeded, not found, stale/conflicting input, invalid input, and storage failure.
Integrate permanent-size and temporary-capacity errors delivered by track 10.
Keep useful existing `AcceptError` distinctions and underlying logging context.

Convert silent mutators, including `set_main_file`, `add_text`, and `name_asset`,
to explicit results. Audit all publication, restore, rendering, server, and CLI
callers: stop dependent work on refusal and keep a valid empty CRDT update
distinct from an error.

Centralize HTTP/socket mapping while retaining wire fields, request IDs,
temporary IDs, and client-safe messages. Define status, retry policy, and cleanup
from error types. Preserve durable authority checks and legacy lease checks.

## Delivery and acceptance

Narrow error additions required by earlier correctness tracks ship with those
tracks. Complete the remaining caller migrations here.

- Read-only/refused mutations prevent dependent publication work.
- Quota errors retain their intended HTTP status and correlation fields.
- Changing error wording cannot alter status, retry, or cleanup behavior.
- Audit for remaining substring classification and silently ignored write results.
- Exercise storage failures and valid empty updates separately from refusals.

# Editing through MCP

## Captured source is the contract

Read the relevant source with `document_read`. Each returned `range_id` is
bound to the immutable view in which it was captured. Pass its `view_id` and
operation epoch to `document_propose`; do not calculate offsets or source
hashes yourself.

A patch contains a `range_id`, its complete replacement, and optionally a
short reviewer note. Use `find` only to distinguish one exact occurrence
inside a captured range. Include other read handles in `dependencies` when the
change relies on them.

## Proposals and direct edits

Suggestions are inert and are the default, including for an editor. Direct
application requires explicit user authorization and the editor role ceiling.
Use `publish: "private"` when constructing a candidate that should not create
review annotations. Call `document_apply` only after both conditions are met,
using the returned candidate ID and a new operation identity.

Use an atomic multi-patch batch only with `publish: "private"` for one staged
candidate. Published multi-suggestion proposals always use
`batch: "independent"`; inspect every item result. Separate published
suggestions cannot make a coherent change atomic. Never weaken `exact_tree`
consistency merely to bypass a conflict or split a refusal to evade limits.

## Projects and compilation

Read the manifest or file list before changing a multi-file project and capture
the range from the correct path. A range belongs to that file and view; it is
not interchangeable with a similar passage elsewhere.

When compilation is required, request `validation: "compile"`. Pending render
work must be checked with `document_result`. Only render evidence tied to the
candidate revision verifies that candidate.

## Conflicts and retries

If the source changed, read fresh evidence and reconcile the intended change
with it. Do not substitute the latest revision silently. A lost response leaves
the write unknown: inspect it without inventing a new operation ID. The service
does not retain committed receipts for operation lookup, so lookup cannot make
an admitted write safe to replay. Retry only through a supported idempotent
path with the exact original identity and arguments. A bounded reread can
refresh expired read context but cannot assume the same passage or revision or
replay an uncertain write.

Avoid broad formatter rewrites unless requested. They create needless review
noise and can disturb anchors throughout the document.

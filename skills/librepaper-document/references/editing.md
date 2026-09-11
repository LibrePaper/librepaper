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

Suggestions are inert and are the default, including for an editor. Use
`publish: "private"` when constructing a candidate that should not create
review annotations. Call `document_apply` only after the user has authorized a
direct edit, using the returned candidate ID and a new operation identity.

Use an atomic batch for one coherent change. Use independent items only when
each can stand on its own, and inspect every item result. Never weaken
`exact_tree` consistency merely to bypass a conflict.

## Projects and compilation

Read the manifest or file list before changing a multi-file project and capture
the range from the correct path. A range belongs to that file and view; it is
not interchangeable with a similar passage elsewhere.

When compilation is required, request `validation: "compile"`. Pending render
work must be checked with `document_result`. Only render evidence tied to the
candidate revision verifies that candidate.

## Conflicts and retries

If the source changed, read fresh evidence and reconcile the intended change
with it. Do not substitute the latest revision silently. For a lost response,
reuse the exact original operation identity and arguments or retrieve its
receipt with `document_result`; a new ID represents new intent and could
duplicate an effect.

Avoid broad formatter rewrites unless requested. They create needless review
noise and can disturb anchors throughout the document.

---
title: "Sharing and access"
---

A document says who may do what to it. There are four roles, as a ladder, each
including the ones beneath it:

| Role | May |
| --- | --- |
| reader | open the document and read its comments |
| commenter | comment, reply, resolve; delete their own |
| editor | edit the source; delete any comment |
| owner | share, transfer, destroy |

A document is shared with links, not people, and a link is the only way in.
The owner mints and manages read, comment, and edit links in the browser's
**Share** pane. Links can be labelled, given a comments-per-hour budget, set to
expire, rotated, or revoked.

Each link contains a key in its fragment. A fragment is never sent to a server,
so the key lands in no access log and on no `Referer` header. Minting a role's
link again rotates it: the old key dies and the new one takes over, which is
how a leaked link is killed without losing the role it stood for. Links expire
after seven days by default; the browser's Share pane can set a different
expiry, including no expiry.

A link may have a label, which is only the owner's memo about what the one
role link is for, and a comments-per-hour budget. Every person or machine
holding that link shares its budget, even from different addresses; without a
custom budget, the deployment's ordinary comment limit applies. The label
survives a rotation unless another is supplied, and both values disappear when
the link is revoked. These controls are beside each role in the browser's
**Share** pane.

A read, comment, or edit link requires the holder to sign in. A link grants its
named role only after sign-in; deployment policy can still restrict which
signed-in accounts may publish or comment. On the public LibrePaper service,
any signed-in GitHub or Google account may publish. Comments from signed-in
users show the account's display name, or its handle when no display name is
available. Visitor-authored legacy comments use a per-document pseudonym.

A stranger -- anyone with the URL and no live link -- is answered exactly as
a deleted document answers. The reading frame is served from a separate
documents host that holds no sign-in of yours, so the reader fetches a
short-lived token on the origin that does and puts it on the frame's URL;
that is what lets an HTML document's own scripts run for whoever may read it
and for nobody else.

Named grants -- an editor or a commenter added by GitHub login, from before
links existed -- are legacy, but remain revocable by that login. `list` marks
documents shared with you with the role you hold.

Sharing, transfer, deletion, comments, suggestions, decisions, editing,
history, comparisons, restores, and labels are browser workflows. The **Share**,
**Files**, **Comments**, and **History** controls in the reader provide these
operations with the document visible beside them.

## Deleting a document

Owners delete a document, including its history and comments, from the
browser's document management controls. Nothing else on the server is touched.

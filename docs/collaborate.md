---
title: "Collaborate"
---

Share with links, not people. The owner mints a read, comment or edit link in the Sharing panel. Each role includes the ones beneath it.

| Role | May |
| --- | --- |
| reader | open the document and read its comments |
| commenter | comment, reply, resolve; delete their own comments |
| editor | edit the source; delete any comment |
| owner | share, destroy |

- Every link needs a sign-in. A reader, commenter or editor signs in with GitHub or Google the first time they open the link, and the browser keeps the key after that.
- The key is in the link's fragment, so it reaches no server log and no `Referer` header.
- A document can have several links for each role. Give each a name. Only the owner sees the names.
- The Links tab lists every live link. Copy puts its URL on the clipboard. The trash button revokes it after you confirm, and the link stops working at once. Expired links are not listed.
- For an agent on another machine, use an agent token instead. See [Remote agents](agents.html#remote-agents).
- Moving a file does not rewrite the references inside source files.
- Copying a checkpoint link keeps your share key. Restore records the current version before applying the earlier one.
- Typst's HTML preview does not reproduce all PDF formatting. Choose PDF from the View menu. The first load fetches the renderer. Typst needs about 13 MB, and LaTeX needs about 6 MB plus the packages a document asks for.

## Comments

A comment follows its passage when the passage is moved by cut and paste or drag and drop, within a file or into another file, and when a rejected tracked deletion brings the passage back. If a passage is gone, its comment shows the passage in words, and an editor can select the passage and choose "Attach to selection". "See it as it was" opens the history at the moment the comment was made.

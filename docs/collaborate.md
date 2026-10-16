---
title: "Collaborate"
---

Share with links, not people. The owner mints a read, comment or edit link in the Share pane. Each role includes the ones beneath it.

| Role | May |
| --- | --- |
| reader | open the document and read its comments |
| commenter | comment, reply, resolve; delete their own comments |
| editor | edit the source; delete any comment |
| owner | share, transfer, destroy |

- Every link needs a sign-in. A reader, commenter or editor signs in with GitHub or Google the first time they open the link, and the browser keeps the key after that.
- The key is in the link's fragment, so it reaches no server log and no `Referer` header. Replacing a link kills the old key.
- Moving a file does not rewrite the references inside source files.
- Copying a checkpoint link keeps your share key. Restore records the current version before applying the earlier one.
- Typst's HTML preview does not reproduce all PDF formatting. Choose PDF from the View menu. The first load fetches the renderer. Typst needs about 13 MB, and LaTeX needs about 6 MB plus the packages a document asks for.

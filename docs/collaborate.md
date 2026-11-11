---
title: "Collaborate"
---

Share with links, not people. The owner mints a read, comment, suggest or edit link in the Share panel. Every link opens the whole project: each file, its preview and the comments. Each role includes the ones beneath it.

| Role | May |
| --- | --- |
| reader | open every file and its preview, and read the comments |
| commenter | comment, reply, resolve; delete their own comments |
| suggester | change the text only as tracked changes, which they can withdraw; no accepting, no file or settings changes |
| editor | edit the source; accept or reject suggestions; delete any comment |
| owner | share, destroy |

- Each link has a "Who can use it" setting. "People who sign in" is the default: a reader, commenter, suggester or editor signs in with GitHub or Google the first time they open the link, and the browser keeps the key after that. "Anyone with the link" needs no sign-in, but only if the host allows it. The host turns this on for read and comment links, and an edit or suggest link always needs sign-in.
- A suggest or edit link works only for accounts that the deployment's `publishers` setting admits. For any other account it acts as a comment link. See [Sign-in](host/simple.html#sign-in).
- In a suggest link, track changes is always on and cannot be switched off. The editor enforces this, not the server, so someone with a modified browser could edit directly. Every edit is still attributed to its author and can be restored from History. The Share panel shows this warning.
- The key is in the link's fragment, so it reaches no server log and no `Referer` header.
- A document can have several links for each role. Give each a name. Only the owner sees the names.
- The Links tab lists every live link. Copy puts its URL on the clipboard. The trash button revokes it after you confirm, and the link stops working at once. Expired links are not listed.
- For an agent you start yourself, on any machine, use an agent token instead. Make it in the Agent panel's Agent tab. See [Agent tokens](agents.html#agent-tokens).
- Moving a file does not rewrite the references inside source files.
- Copying a checkpoint link keeps your share key. Restore records the current version before applying the earlier one.
- Typst's HTML preview does not reproduce all PDF formatting. Choose PDF from the View menu. The first load fetches the renderer. Typst needs about 13 MB, and LaTeX needs about 6 MB plus the packages a document asks for.

## Preview

The preview shows the file you are editing. A chapter that cannot render on its own shows inside the file that includes it, and the preview says which. The eye in Files locks the preview on a file, so you can edit any other file while it stays put.

## Comments

A comment follows its passage when the passage is moved by cut and paste or drag and drop, within a file or into another file, and when a rejected tracked deletion brings the passage back. If a passage is gone, its comment shows the passage in words, and an editor can select the passage and choose "Attach to selection".

## Authors

The Authors tab in History tints the open file by who wrote each passage, and lists each person's share. Hover a passage for the name and date. Click a person to show only their text. Text written before authorship was recorded shows as Unknown author.

Authorship is informative, not proof. An editor with a modified client can still make new text appear under a co-author's name, so do not use it to settle disputes.

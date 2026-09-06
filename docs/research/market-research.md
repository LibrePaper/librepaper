# Overleaf Feature Research

Research notes on Overleaf's feature set, compiled from Overleaf's own marketing
pages and documentation (September 2026). Purpose: a checklist of what a
collaborative browser-based document editor is expected to do, to inform the
komodoc live markdown editor.

Sources are listed at the bottom.

## 1. Editing

- **Code editor** — syntax highlighting for LaTeX, autocomplete of commands and
  environments, real-time (as-you-type) error checking, bracket matching.
- **Visual editor** — WYSIWYG-ish mode that hides most markup; user can toggle
  freely between visual and code views on the same document.
- **Rich insert UI** — tables and figures inserted from a toolbar/dialog rather
  than hand-written markup.
- **Symbol palette** *(premium)* — clickable grid of math symbols/operators that
  inserts the corresponding markup.
- **Spell check** — Aspell-backed, multi-language, wavy-underline highlighting,
  selectable dictionary, switchable off.
- **Word count** — on-demand count over the compiled document (not raw source).
- **Search and replace** within the source, plus find across project files.
- **Keyboard shortcuts**, with optional **Vim and Emacs keybindings**.
- **File tree** — create/rename/delete/move files and folders, upload files,
  multi-file projects with includes.
- **Editor layout control** — split editor/preview, editor-only, preview-only,
  detachable preview into a second browser window.

## 2. Compile / preview loop

- **No local setup** — full TeX distribution server-side; no installation or
  package management by the user.
- **Real-time preview** — recompile on demand or automatically; PDF rendered in
  an embedded viewer.
- **Selectable TeX Live version** per project (reproducibility / old templates).
- **SyncTeX** — bidirectional jump between a source line and its position in the
  rendered PDF, via arrows on the editor/preview divider.
- **Error, warning, and log panel** — parsed compiler output with jump-to-line;
  raw logs also available.
- **Compile timeout tiers** — free accounts get a short compile budget; premium
  plans get extended compile time. (Relevant to komodoc's sandbox cost bounding.)
- **Stop-on-first-error vs. full-run** compile modes.
- **Automatic bibliography compilation** — no separate bibtex/biber invocation.

## 3. Collaboration

- **Project sharing** — invite by email as editor / reviewer / viewer, or share
  by link (read-write link, read-only link).
- **Simultaneous multi-user editing** with live cursors and collaborator edits
  visible in real time.
- **Comments** — anchored to a text range, threaded, resolvable.
- **Track changes** *(premium)* — real-time; each collaborator's insertions and
  deletions attributed and individually acceptable/rejectable.
- **Review panel** — aggregated list of open comments and pending changes.
- **In-document chat** — project-wide chat sidebar separate from comments.
- **Ownership transfer** and per-collaborator permission changes.
- **Collaborator limits by plan**: Free 1 editor/reviewer, Student 10,
  Standard 10, Pro unlimited; **viewers unlimited on all plans**.
- Premium capabilities (track changes, history, compile time) are **scoped to
  the project owner's subscription**, not per-collaborator — a paying owner's
  free collaborators get the features on that project.

## 4. History and versioning

- **Full project history** *(premium)* — timeline of all changes; free accounts
  are capped at the last 24 hours.
- **Version comparison** — diff between two points in time, per file.
- **Labelled versions** — name a point in history (e.g. "submitted draft").
- **Restore** a file or the whole project to an earlier state.
- **Attribution** — history entries show which collaborator made each change.

## 5. Bibliography and references

- **`.bib` files as first-class project files**, compiled inline.
- **Advanced reference search** *(premium)* — inline citation autocomplete
  searching by key, author, or title.
- **Reference manager integrations** *(premium)* — Zotero, Mendeley, ReadCube;
  linked account, library synced into the project as a `.bib`.

## 6. Integrations

- **Git bridge** *(premium)* — treat the project as a remote git repo; clone,
  push, pull from a local checkout. Enables full offline work.
  - Caveat Overleaf documents: **pushing via git can lose or displace track
    changes and comments**; they advise not mixing git with active reviewing.
- **GitHub synchronization** *(premium)* — link a project to a GitHub repo, with
  push/pull through the web UI rather than a local clone.
- **Dropbox synchronization** *(premium)* — two-way folder sync.
- **Grammarly** support in the editor.
- **Overleaf AI / Writefull** (free tier with daily caps, unlimited on premium):
  - LaTeX table generator from an image or a text prompt.
  - Equation generator from a prompt or an image.
  - TeXGPT: generate figures and LaTeX code, produce outlines and skeleton
    documents, explain content, give writing feedback.
  - LaTeX error assistance: explain a compile error and suggest a fix.
  - Research-tailored language feedback: grammar, spelling, word choice,
    sentence structure.
  - Paraphrase tool: rewrite selection, summarize, synonyms, generate abstract
    and title.

## 7. Templates and publishing

- **Template gallery** — journal articles, theses, CVs/résumés, posters,
  presentations, assignments; start a project from any of them.
- **Publisher templates** — official journal classes maintained with publishers.
- **User-submitted templates** to the public gallery.
- **Submission integrations** — send a project directly to some journals /
  preprint servers.
- **Export** — download the compiled PDF, or the project as a zip of sources.

## 8. Project management

- Project dashboard with **tags/folders**, archive, trash, rename, copy/clone.
- Upload an existing project as a zip.
- Search across projects by name/owner.

## 9. Accounts, admin, deployment

- SSO, multiple emails per account, institutional affiliation, account deletion.
- **Group subscriptions** — admin console, seat management, group SSO, usage
  reporting.
- **Overleaf Commons** — site-wide institutional licence, enrollment and
  reporting tooling.
- **On-premises**: Server Pro (commercial, includes the git bridge, SSO, LDAP)
  and Community Edition (open source, AGPL, `overleaf/overleaf` on GitHub),
  deployed via the Overleaf Toolkit.

## 10. Takeaways for komodoc

Features that plausibly map onto a live markdown/Typst editor, ranked by how
much they define the product:

1. Real-time multi-user editing with visible cursors — table stakes.
2. Compile-preview loop with SyncTeX-style source↔output position mapping.
3. Comments anchored to text ranges + resolvable threads.
4. Track changes with per-change accept/reject.
5. Project history with diff and restore.
6. Dual visual/source editing modes on one document.
7. Link-based sharing with viewer/reviewer/editor roles, unlimited viewers.
8. Compile budget tiering — Overleaf's model (short free timeout, longer paid)
   is a direct precedent for bounding sandbox cost.
9. Git bridge for offline work, with the honest caveat that it does not
   round-trip review metadata.
10. Templates gallery as the on-ramp for new users.

## Sources

- https://www.overleaf.com/about/features-overview
- https://docs.overleaf.com/getting-started/free-and-premium-plans/premium-features
- https://docs.overleaf.com/getting-started/free-and-premium-plans/plan-limits
- https://docs.overleaf.com/collaborating/collaborating-in-overleaf
- https://docs.overleaf.com/integrations-and-add-ons/git-integration-and-github-synchronization/git-integration
- https://docs.overleaf.com/integrations-and-add-ons/git-integration-and-github-synchronization/github-synchronization
- https://docs.overleaf.com/integrations-and-add-ons/ai-features
- https://www.overleaf.com/about/ai-features
- https://docs.overleaf.com/getting-started/how-do-i-use-overleaf/redesigned-overleaf-editor
- https://docs.overleaf.com/llms.txt
- https://github.com/overleaf/overleaf

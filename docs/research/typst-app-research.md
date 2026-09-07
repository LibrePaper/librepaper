# Typst Web App (typst.app) Feature Research

Research notes on the official Typst web app, compiled from typst.app's own
documentation and pricing pages (September 2026). Companion to
`docs/research/market-research.md` (Overleaf). Purpose: a checklist of what a
collaborative browser-based document editor is expected to do, to inform the
komodoc live markdown/Typst editor.

Typst is deliberately a smaller, newer product than Overleaf: the app is the
commercial layer on top of an open-source (Apache-2.0) compiler, so the free
tier is generous on *editing* and the paid tier gates *collaboration workflow*
and *integrations* rather than compute.

Sources are listed at the bottom.

## 1. Editing

- **Compiler error assistance** (free tier) — errors explained inline.
- Per-user editor settings: spellcheck, text size, editor font.
- **Search** across the project.

## 2. Compile / export

- **Export formats**: PDF, PNG (zipped for multi-page), SVG (zipped for
  multi-page), and **HTML** (experimental, 0.14.0+, must be enabled under
  "Experimental compiler features" in project settings).
- Export settings live in the "Export and Preview" menu, **persist across
  sessions and sync to collaborators** — export config is project state, not
  user state.
- **No per-plan compile timeout tier.** Unlike Overleaf, Typst does not sell
  compile time; the limits are storage and file count instead (see §7). Worth
  noting for komodoc's cost-bounding design — Typst's compiler is fast enough
  that they don't need to meter it.

## 3. Collaboration

- **Project members** — signed-in users with permanent access that survives
  revoking a share link.
- **Comments** *(Pro for the project owner to enable)*:
  - anchored to a selected text range, and stay anchored as the document
    changes;
  - yellow highlight in the text plus a gutter icon;
  - threaded replies;
  - resolve/unresolve, with a "Show resolved" checkbox in the Improve panel;
  - only the author can edit their own message;
  - **1,000 comments/replies per project** cap;
  - comments are excluded from PDF export.
- **No track changes.** There is no per-change accept/reject workflow; this is a
  long-standing open request (typst/typst discussion #1499). Comments are the
  entire review surface.
- **No in-document chat** equivalent to Overleaf's.

## 4. Teams and organization

- **Teams** — users create/join teams to jointly own a set of projects; every
  team member can manage the team's projects as if they owned them.
- **Team roles** — administrators control invitations and membership; regular
  members cannot manage membership.
- A team needs **at least one Pro-subscribed administrator** to unlock Pro
  features for the team's projects.
- **Folders** *(Pro)* for organizing projects.
- **Team-wide private packages and templates** *(Pro)* — the org-internal
  package registry is a core selling point.

## 5. History and versioning

- Typst's own history story is thin: there is **no documented full-project
  history browser, diff, or labelled-version feature** comparable to Overleaf's.
  Versioning is effectively delegated to **Git Sync** (§6).
- This is the clearest capability gap versus Overleaf, and the clearest place a
  competing product can differentiate.

## 6. Integrations

- **Git Sync** *(Pro to link; experimental)*:
  - GitHub and GitLab only; other forges unsupported (self-hosted instances
    configurable for on-premises deployments);
  - bidirectional — pull remote changes in, commit and push local edits out;
  - **Pro is only required to link** a project to a repo; afterwards *any*
    collaborator with write access can push and pull;
  - textual merges handled automatically; conflicts requiring manual resolution:
    file deleted remotely but modified locally, binary updated on both sides,
    same file created on both sides;
  - **forbidden files**: archives (`.zip`, `.tar`) and executables are not
    synced through the app, though they stay in the repo;
  - cannot connect an existing project to a *non-empty* repository directory —
    needs an empty directory or a new branch;
  - documented guarantee: **never force-pushes / never overwrites history**.
- **Reference Sync** *(Pro to create)* — Zotero and Mendeley:
  - link the account in settings, then "Sync from Zotero/Mendeley" in the file
    browser's advanced options creates a synced `.bib`;
  - output is BibLaTeX; **refreshes every five minutes while the project is
    open**; a timer shows last sync; manual force-resync available;
  - synced files are read-only in-app; disconnecting turns the file back into a
    plain editable text file;
  - once created, any write-access collaborator can read and force-sync it —
    but if the *creator leaves the project*, the file degrades to a plain text
    file.

## 7. Plans and limits

| | Free ($0) | Pro ($7.99/mo, ~14% off annual) | On-Premises (custom, ≥5 seats) |
|---|---|---|---|
| Create/edit projects | ✓ | ✓ | ✓ |
| Share and collaborate | ✓ | ✓ | ✓ |
| Community packages & templates | ✓ | ✓ | ✓ |
| Convert from LaTeX/Word/Markdown | ✓ | ✓ | ✓ |
| Compiler error assistance | ✓ | ✓ | ✓ |
| Comments / review | — | ✓ | ✓ |
| Private packages & templates | — | ✓ | ✓ |
| GitHub/GitLab sync (experimental) | — | ✓ | ✓ |
| Zotero/Mendeley sync | — | ✓ | ✓ |
| Email invitations | — | ✓ | ✓ |
| Presentation mode + drawing | — | ✓ | ✓ |
| Folders | — | ✓ | ✓ |
| Storage | 200 MB | 2 GB | — |
| Files per project | 100 | 1,000 | — |
| Self-hosted in own data center | — | — | ✓ |
| Org-wide package & font distribution | — | — | ✓ |
| LDAP access control | — | — | ✓ |
| Priority support | — | — | ✓ |

## 10. Typst vs. Overleaf: the deltas

Where Typst is **ahead**:
- Genuinely instant preview (incremental compiler), no compile-time metering.
- Real-time collaboration is free, not paywalled.
- Presentation mode with laser pointer and ink annotation.
- Private package registry for a team — no Overleaf equivalent.
- HTML export path (experimental) alongside PDF.
- Git sync usable by all collaborators once linked, and it never force-pushes.

Where Typst is **behind**:
- **No track changes / accept-reject review workflow at all.**
- **No project history, diff, restore, or labelled versions.**
- No AI assistance in-app.
- No visual/WYSIWYG editing mode — source only.
- No SyncTeX-equivalent source↔preview position jumping documented.
- No word count, no symbol palette, no Vim/Emacs keybindings documented.
- Much smaller template gallery; no publisher/journal submission pipeline.
- Git sync is experimental and refuses non-empty target directories.

## 11. Takeaways for komodoc

1. **Live preview must feel instant.** Typst set the bar; a debounce-and-render
   loop that feels laggy reads as a worse product regardless of features.
2. **Range-anchored, threaded, resolvable comments** are the minimum viable
   review surface — Typst ships a full Pro tier on essentially this alone.
3. **History/diff/restore is the open gap** in the Typst ecosystem. If komodoc
   wants a wedge against typst.app specifically, that is it.
4. **Don't meter compiles if you don't have to** — but komodoc's sandbox cost
   constraint is real, so Overleaf's timeout tiering remains the fallback model.
5. **Per-user vs. project-scoped state** is a real design decision: Typst makes
   preview target user-local and export settings project-global. Worth copying
   that split.
6. **Git sync that never force-pushes**, refuses to clobber, and surfaces
   conflicts explicitly is a good conservative model to imitate.
7. **Storage + file-count limits** are a simpler, more predictable free-tier
   lever than compute limits.
8. Presentation mode is a cheap, high-delight feature once you already render
   pages — laser pointer, ink, and lights-off are small additions.
9. The closed web app around an open compiler leaves room for an open
   collaborative workspace; that is the space komodoc sits in.

## Sources

- https://typst.app/docs/web-app/
- https://typst.app/docs/web-app/concepts/
- https://typst.app/docs/web-app/export-and-preview/
- https://typst.app/docs/web-app/comments/
- https://typst.app/docs/web-app/git-sync/
- https://typst.app/docs/web-app/reference-sync/
- https://typst.app/docs/web-app/presentation-mode/
- https://typst.app/pricing/
- https://typst.app/open-source/
- https://github.com/typst/typst
- https://github.com/typst/typst/discussions/1499 (track changes request)
- https://forum.typst.app/t/introducing-collabst-a-self-hosted-collaborative-workspace-for-typst/8856

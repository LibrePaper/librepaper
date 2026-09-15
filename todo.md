  The migration itself is essentially done

  Four of five phases pass their gates. yrs, yjs, y-protocols, y-codemirror.next, y-indexeddb are gone; the server, editor, persistence, presence and fuzz targets all run on Loro; §1.2's
  four proposal mechanisms are one.

  Phase 2 is the only one open, on a single scenario in editor-browser.mjs.

  What's actually left

  1. Two browser tests. editor (one scenario, in the forked binding) and insert (a menu destructuring a tile off an empty list — not a collaboration bug). 22 of 24 pass.

  2. The review UI is half-built, and the half that exists isn't connected. This is the biggest gap, and I should be blunt that it's bigger than my last summaries implied:

  - proposal-marks.js is written and tested — but grep says it's referenced zero times in Editor.svelte. It renders nothing today.
  - Changes.svelte still carries 19 references to the deleted revision mechanism, and has no test coverage. That's where the margin cards belong.
  - So: a reviewer still cannot see or act on a proposal. The model is complete and tested; the interface is a layer with no caller.

  3. Tracked editing is still dead. It has been since the cutover — deliberately, since the branch model replaced it, but nothing has restored the user-facing behaviour.

  4. Three §9 follow-ups, all genuinely deferrable: evaluate LoroTree to collapse files+paths; reconsider checkpoint density now every intermediate state is reachable; and the last of the
  max_encoded_snapshot_bytes machinery.

  5. The fork needs sending upstream. web/vendor/loro-codemirror/ has four fixes and a README saying what to delete when a release contains them. I haven't opened that PR — pushing to
  someone else's repo is yours to decide.

  What I'd do next

  Wire proposal-marks into the editor and build the cards. Everything else is finishing touches; this is the difference between a working review model and one nobody can use.

  And before touching Changes.svelte, write a browser test for that panel — it has no coverage, 19 stale references, and this project has now shown repeatedly that untested UI here is
  where things quietly stop working.

✻ Worked for 10s · done 9:28 AM

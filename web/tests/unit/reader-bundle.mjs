// The published version, and whether what is on screen has moved past it.
//
// `document.sha` cannot answer that question -- it is the main source file's
// digest and a project is a tree -- so staleness is computed from the whole
// tree and the renderer's identity. What is checked here is that the answer
// is never optimistic: a comparison that could not be made says stale, and a
// comparison overtaken by an edit says nothing at all.
import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

import { capturePreviewTree } from "../../src/lib/assistant-preview.js";
import { snapshotDigest } from "../../src/lib/tree-digest.js";

const { createBundle } = await loadRunes(
  new URL("../../src/lib/reader/bundle.svelte.js", import.meta.url),
);

const TREE = { main: "main.md", texts: { "main.md": "source" }, files: {}, digests: {} };
// The digest the module will actually compute for that tree, taken the same
// way it takes it. Writing a made-up one here would check nothing.
const SOURCE_SHA = await snapshotDigest(capturePreviewTree(TREE));
const published = { id: "pub-1", source_sha256: SOURCE_SHA, render_config_sha256: "config-sha" };

function build(over = {}) {
  const said = [];
  const facts = { canEdit: true, hasSession: true, source: 0, renderable: true, ...over.facts };
  const readers = [];
  const bundle = createBundle({
    slug: "paper",
    liveTree: () => TREE,
    committedTree: () => TREE,
    heading: async () => "Title",
    gather: async () => ({ assets: {}, urls: {} }),
    facts: () => facts,
    say: (message) => said.push(message),
    sha256: async () => "config-sha",
    renderers: {
      htmlConfiguration: async () => ({ identity: { renderer: "markdown" } }),
      render: async () => ({ html: "<p>page</p>", ok: true, diagnostics: [] }),
      ...over.renderers,
    },
    publisher: { publish: async () => ({ id: "pub-2" }), ...over.publisher },
    // The one step that needs a DOM to walk. What it produces is checked in
    // tests/unit/bundle.mjs; here it only has to be a bundle.
    buildBundle: async (html) => ({ html, assets: {} }),
    makeReader: (options) => {
      const reader = {
        options,
        refreshes: 0,
        disposed: false,
        refresh: async () => { reader.refreshes++; return over.refreshed ?? published; },
        dispose: () => { reader.disposed = true; },
        announce: () => {},
      };
      readers.push(reader);
      return reader;
    },
    ...over.extra,
  });
  return { bundle, said, facts, readers };
}

// A tree whose digest matches what was published is not stale; one that does
// not, is.
{
  const { bundle } = build();
  bundle.state.bundle = published;
  await bundle.refreshStatus();
  assert.equal(bundle.state.stale, false, "same tree, same renderer: nothing to republish");

  // Compared against the bundle the document currently has, so that is
  // what is replaced: a comparison handed some other bundle is one whose
  // answer nobody is waiting for, and is dropped.
  bundle.state.bundle = { ...published, source_sha256: "something-else" };
  await bundle.refreshStatus();
  assert.equal(bundle.state.stale, true, "a different tree has moved past the bundle");

  bundle.state.bundle = { ...published, render_config_sha256: "other-config" };
  await bundle.refreshStatus();
  assert.equal(bundle.state.stale, true, "so has a different renderer identity");

  // And that dropping is itself the point: a status for a bundle the
  // document has since replaced must not overwrite the current answer.
  bundle.state.bundle = published;
  bundle.state.stale = true;
  await bundle.refreshStatus({ id: "pub-0", source_sha256: SOURCE_SHA, render_config_sha256: "config-sha" });
  assert.equal(bundle.state.stale, true, "a superseded comparison leaves the answer alone");
}

// A renderer identity that cannot be resolved must not be read as a match.
{
  const { bundle } = build({
    renderers: { htmlConfiguration: async () => { throw new Error("no renderer"); } },
  });
  bundle.state.bundle = published;
  await bundle.refreshStatus();
  assert.equal(bundle.state.stale, true, "a comparison that could not be made says stale");
}

// A comparison overtaken by an edit writes nothing: what it measured is a
// tree nobody is looking at any more.
{
  const { bundle, facts } = build({
    renderers: {
      htmlConfiguration: async () => { facts.source += 1; return { identity: {} }; },
    },
  });
  bundle.state.bundle = { ...published, source_sha256: "something-else" };
  bundle.state.stale = false;
  await bundle.refreshStatus();
  assert.equal(bundle.state.stale, false, "an overtaken comparison leaves the answer alone");
}

// A reader who cannot edit has nothing to compare and nothing to say.
{
  const { bundle } = build({ facts: { canEdit: false } });
  bundle.state.bundle = published;
  bundle.state.stale = true;
  await bundle.refreshStatus();
  assert.equal(bundle.state.stale, false);
}

// Publishing waits for the published version to have been read: an update
// has to name the bundle it expects to replace.
{
  const { bundle } = build();
  bundle.watchAsEditor();
  await assert.rejects(bundle.publish(), /Checking the published version/);
}

{
  const { bundle } = build({ facts: { hasSession: false } });
  bundle.state.metadataReady = true;
  await assert.rejects(bundle.publish(), /Only the current editable document can be published/);
}

// A document that will not render as HTML is not published as a broken one.
{
  const { bundle } = build({
    renderers: { render: async () => ({ html: "<p>x</p>", ok: true, diagnostics: [{ severity: "error" }] }) },
  });
  bundle.state.metadataReady = true;
  await assert.rejects(bundle.publish(), /Fix its render errors/);
}

// The ordinary publish: the expected bundle is named. Nothing is said
// out of band -- the Share panel's button reads "Publishing…" throughout.
{
  const sent = [];
  const { bundle, said } = build({
    publisher: {
      publish: async (request) => {
        sent.push(request);
        request.onProgress?.({ phase: "published" });
        return { id: "pub-2" };
      },
    },
  });
  bundle.state.bundle = published;
  bundle.state.metadataReady = true;
  const result = await bundle.publish();
  assert.equal(result.id, "pub-2");
  assert.equal(sent[0].expectedBundleId, "pub-1", "an update names what it replaces");
  assert.equal(bundle.state.bundle.id, "pub-2");
  assert.equal(bundle.state.update, false);
  assert.deepEqual(said, [], "an editor publishing is watching the button that says so");
  assert.equal(bundle.state.stale, false, "what was just published is what is on screen");
}

// Somebody else published first. The reader is told to look at theirs rather
// than having this one overwrite it.
{
  const { bundle, readers } = build({
    publisher: {
      publish: async () => { throw Object.assign(new Error("conflict"), { status: 409 }); },
    },
  });
  bundle.watchAsEditor();
  bundle.state.metadataReady = true;
  await assert.rejects(bundle.publish(), /A newer published version is now current/);
  assert.equal(readers.at(-1).refreshes, 1, "and the newer one is read before they are told");
}

// After publishing, whether the screen already differs from what was sent is
// asked rather than assumed. An edit landing while that question is being
// answered makes the bundle stale.
{
  let configurations = 0;
  const { bundle, facts } = build({
    renderers: {
      htmlConfiguration: async () => {
        // The second call is the one taken against the live tree after the
        // bundle went up; an edit arriving then is what this is about.
        if (++configurations === 2) facts.source += 1;
        return { identity: { renderer: "markdown" } };
      },
    },
  });
  bundle.state.metadataReady = true;
  await bundle.publish();
  assert.equal(bundle.state.stale, true, "typing through a publish leaves it out of date");
}

// And a publish nobody typed through leaves it current.
{
  const { bundle } = build();
  bundle.state.metadataReady = true;
  await bundle.publish();
  assert.equal(bundle.state.stale, false);
}

// A visitor is offered a newer version rather than having it swapped under
// them; accepting it is what clears the offer.
{
  const { bundle, readers } = build();
  bundle.watchAsVisitor({});
  const { onBundle } = readers.at(-1).options;
  onBundle({ id: "pub-1" });
  assert.equal(bundle.state.bundle.id, "pub-1");
  onBundle({ id: "pub-1", pending_id: "pub-2" });
  assert.equal(bundle.state.update, true, "a pending newer version is an offer");
  assert.equal(bundle.state.bundle.id, "pub-1", "and does not replace what they are reading");
  await bundle.acceptUpdate();
  assert.equal(bundle.state.update, false);
  assert.equal(readers.at(-1).refreshes, 1);
}

// The first source of a session triggers one metadata read, not one per edit.
{
  const { bundle } = build();
  assert.equal(bundle.begin(), false, "nothing to read before there is a reader");
  bundle.watchAsEditor();
  assert.equal(bundle.begin(), true);
  assert.equal(bundle.begin(), false, "later edits are not a new session");
}

// The id an annotation is filed against, under either of the two names a
// bundle answers to.
{
  const { bundle } = build();
  assert.equal(bundle.id(), "");
  bundle.state.bundle = { id: "pub-1" };
  assert.equal(bundle.id(), "pub-1");
  bundle.state.bundle = { id: "pub-1", bundle_id: "canonical" };
  assert.equal(bundle.id(), "canonical");
}

// Keeping the reader version current is this module's job, not a button's.
// A document somebody holds a link to, whose draft has moved past what was
// published, republishes itself.
{
  const { bundle } = build();
  bundle.state.metadataReady = true;
  bundle.state.bundle = { ...published, source_sha256: "something-else" };
  bundle.state.stale = true;
  await bundle.catchUp();
  assert.equal(bundle.state.bundle.id, "pub-2", "the draft reached readers unasked");
  assert.equal(bundle.state.blocked, "");
}

// Every document keeps a reader version, shared or not: a document has one
// the way it has a title, and an author who wants somewhere nothing is
// prepared for readers opens another project.
{
  let published_ = 0;
  const { bundle } = build({
    publisher: { publish: async () => { published_ += 1; return { id: "pub-2" }; } },
  });
  bundle.state.metadataReady = true;
  bundle.state.bundle = null;
  await bundle.catchUp();
  assert.equal(published_, 1, "a document never shared still has a reader version");
}

// A draft that does not render has nothing to publish. Readers keep the last
// version that did, and the author is told why rather than left watching a
// bundle that never arrives.
{
  let published_ = 0;
  const { bundle } = build({
    facts: { renderable: false },
    publisher: { publish: async () => { published_ += 1; return { id: "pub-2" }; } },
  });
  bundle.state.metadataReady = true;
  bundle.state.bundle = published;
  bundle.state.stale = true;
  await bundle.catchUp();
  assert.equal(published_, 0, "a broken draft does not replace a working bundle");
  assert.match(bundle.state.blocked, /does not render/);
}

// A draft already published is left alone: catching up twice is not two
// bundles.
{
  let published_ = 0;
  const { bundle } = build({
    publisher: { publish: async () => { published_ += 1; return { id: "pub-2" }; } },
  });
  bundle.state.metadataReady = true;
  bundle.state.bundle = published;
  bundle.state.stale = false;
  await bundle.catchUp();
  assert.equal(published_, 0, "nothing has moved, so nothing is rebuilt");
}

// A failed publish is reported where the author is, and does not leave the
// document looking as though it is still working on one.
{
  const { bundle } = build({
    publisher: { publish: async () => { throw new Error("the bundle would not upload"); } },
  });
  bundle.state.metadataReady = true;
  bundle.state.bundle = { ...published, source_sha256: "something-else" };
  bundle.state.stale = true;
  await bundle.catchUp();
  assert.equal(bundle.state.publishing, false);
  assert.match(bundle.state.blocked, /would not upload/);
}

// A document still waiting for its first page is not a document that will not
// render. It goes back on the clock rather than being given up on: an edit was
// the only other thing that armed the timer, so a document opened and shared
// without being typed into got one attempt, and a commenter was left reading
// "Nothing to read yet" until somebody happened to touch the source.
{
  let published_ = 0;
  const { bundle, facts } = build({
    facts: { renderable: false, rendering: true },
    publisher: { publish: async () => { published_ += 1; return { id: "pub-2" }; } },
  });
  bundle.state.metadataReady = true;
  bundle.state.bundle = null;

  // The quiet window is half a minute, so what is checked is that another one
  // was started, not that it elapsed.
  const armed = [];
  const real = globalThis.setTimeout;
  globalThis.setTimeout = (fn, ms) => { armed.push(ms); return { unref() {} }; };
  try {
    await bundle.catchUp();
  } finally {
    globalThis.setTimeout = real;
  }
  assert.equal(published_, 0, "there is no page to publish yet");
  assert.equal(bundle.state.blocked, "", "and nothing to tell the author, because nothing is wrong");
  assert.deepEqual(armed, [30_000], "it asks again after another quiet window");

  // And when the page does arrive, the attempt it was waiting for succeeds.
  facts.renderable = true;
  facts.rendering = false;
  await bundle.catchUp();
  assert.equal(published_, 1, "the reader version arrives without anybody editing the source");
}

console.log("reader bundle: staleness is pessimistic, the reader version keeps itself current");

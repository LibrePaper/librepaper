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

const { createPublication } = await loadRunes(
  new URL("../../src/lib/reader/publication.svelte.js", import.meta.url),
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
  const publication = createPublication({
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
    // tests/unit/publication.mjs; here it only has to be a bundle.
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
  return { publication, said, facts, readers };
}

// A tree whose digest matches what was published is not stale; one that does
// not, is.
{
  const { publication } = build();
  publication.state.publication = published;
  await publication.refreshStatus();
  assert.equal(publication.state.stale, false, "same tree, same renderer: nothing to republish");

  // Compared against the publication the document currently has, so that is
  // what is replaced: a comparison handed some other publication is one whose
  // answer nobody is waiting for, and is dropped.
  publication.state.publication = { ...published, source_sha256: "something-else" };
  await publication.refreshStatus();
  assert.equal(publication.state.stale, true, "a different tree has moved past the publication");

  publication.state.publication = { ...published, render_config_sha256: "other-config" };
  await publication.refreshStatus();
  assert.equal(publication.state.stale, true, "so has a different renderer identity");

  // And that dropping is itself the point: a status for a publication the
  // document has since replaced must not overwrite the current answer.
  publication.state.publication = published;
  publication.state.stale = true;
  await publication.refreshStatus({ id: "pub-0", source_sha256: SOURCE_SHA, render_config_sha256: "config-sha" });
  assert.equal(publication.state.stale, true, "a superseded comparison leaves the answer alone");
}

// A renderer identity that cannot be resolved must not be read as a match.
{
  const { publication } = build({
    renderers: { htmlConfiguration: async () => { throw new Error("no renderer"); } },
  });
  publication.state.publication = published;
  await publication.refreshStatus();
  assert.equal(publication.state.stale, true, "a comparison that could not be made says stale");
}

// A comparison overtaken by an edit writes nothing: what it measured is a
// tree nobody is looking at any more.
{
  const { publication, facts } = build({
    renderers: {
      htmlConfiguration: async () => { facts.source += 1; return { identity: {} }; },
    },
  });
  publication.state.publication = { ...published, source_sha256: "something-else" };
  publication.state.stale = false;
  await publication.refreshStatus();
  assert.equal(publication.state.stale, false, "an overtaken comparison leaves the answer alone");
}

// A reader who cannot edit has nothing to compare and nothing to say.
{
  const { publication } = build({ facts: { canEdit: false } });
  publication.state.publication = published;
  publication.state.stale = true;
  await publication.refreshStatus();
  assert.equal(publication.state.stale, false);
}

// Publishing waits for the published version to have been read: an update
// has to name the publication it expects to replace.
{
  const { publication } = build();
  publication.watchAsEditor();
  await assert.rejects(publication.publish(), /Checking the published version/);
}

{
  const { publication } = build({ facts: { hasSession: false } });
  publication.state.metadataReady = true;
  await assert.rejects(publication.publish(), /Only the current editable document can be published/);
}

// A document that will not render as HTML is not published as a broken one.
{
  const { publication } = build({
    renderers: { render: async () => ({ html: "<p>x</p>", ok: true, diagnostics: [{ severity: "error" }] }) },
  });
  publication.state.metadataReady = true;
  await assert.rejects(publication.publish(), /Fix its render errors/);
}

// The ordinary publish: the expected publication is named. Nothing is said
// out of band -- the Share panel's button reads "Publishing…" throughout.
{
  const sent = [];
  const { publication, said } = build({
    publisher: {
      publish: async (request) => {
        sent.push(request);
        request.onProgress?.({ phase: "published" });
        return { id: "pub-2" };
      },
    },
  });
  publication.state.publication = published;
  publication.state.metadataReady = true;
  const result = await publication.publish();
  assert.equal(result.id, "pub-2");
  assert.equal(sent[0].expectedPublicationId, "pub-1", "an update names what it replaces");
  assert.equal(publication.state.publication.id, "pub-2");
  assert.equal(publication.state.update, false);
  assert.deepEqual(said, [], "an editor publishing is watching the button that says so");
  assert.equal(publication.state.stale, false, "what was just published is what is on screen");
}

// Somebody else published first. The reader is told to look at theirs rather
// than having this one overwrite it.
{
  const { publication, readers } = build({
    publisher: {
      publish: async () => { throw Object.assign(new Error("conflict"), { status: 409 }); },
    },
  });
  publication.watchAsEditor();
  publication.state.metadataReady = true;
  await assert.rejects(publication.publish(), /A newer published version is now current/);
  assert.equal(readers.at(-1).refreshes, 1, "and the newer one is read before they are told");
}

// After publishing, whether the screen already differs from what was sent is
// asked rather than assumed. An edit landing while that question is being
// answered makes the publication stale.
{
  let configurations = 0;
  const { publication, facts } = build({
    renderers: {
      htmlConfiguration: async () => {
        // The second call is the one taken against the live tree after the
        // bundle went up; an edit arriving then is what this is about.
        if (++configurations === 2) facts.source += 1;
        return { identity: { renderer: "markdown" } };
      },
    },
  });
  publication.state.metadataReady = true;
  await publication.publish();
  assert.equal(publication.state.stale, true, "typing through a publish leaves it out of date");
}

// And a publish nobody typed through leaves it current.
{
  const { publication } = build();
  publication.state.metadataReady = true;
  await publication.publish();
  assert.equal(publication.state.stale, false);
}

// A visitor is offered a newer version rather than having it swapped under
// them; accepting it is what clears the offer.
{
  const { publication, readers } = build();
  publication.watchAsVisitor({});
  const { onPublication } = readers.at(-1).options;
  onPublication({ id: "pub-1" });
  assert.equal(publication.state.publication.id, "pub-1");
  onPublication({ id: "pub-1", pending_id: "pub-2" });
  assert.equal(publication.state.update, true, "a pending newer version is an offer");
  assert.equal(publication.state.publication.id, "pub-1", "and does not replace what they are reading");
  await publication.acceptUpdate();
  assert.equal(publication.state.update, false);
  assert.equal(readers.at(-1).refreshes, 1);
}

// The first source of a session triggers one metadata read, not one per edit.
{
  const { publication } = build();
  assert.equal(publication.begin(), false, "nothing to read before there is a reader");
  publication.watchAsEditor();
  assert.equal(publication.begin(), true);
  assert.equal(publication.begin(), false, "later edits are not a new session");
}

// The id an annotation is filed against, under either of the two names a
// publication answers to.
{
  const { publication } = build();
  assert.equal(publication.id(), "");
  publication.state.publication = { id: "pub-1" };
  assert.equal(publication.id(), "pub-1");
  publication.state.publication = { id: "pub-1", publication_id: "canonical" };
  assert.equal(publication.id(), "canonical");
}

// Keeping the reader version current is this module's job, not a button's.
// A document somebody holds a link to, whose draft has moved past what was
// published, republishes itself.
{
  const { publication } = build();
  publication.state.metadataReady = true;
  publication.state.publication = { ...published, source_sha256: "something-else" };
  publication.state.stale = true;
  await publication.catchUp();
  assert.equal(publication.state.publication.id, "pub-2", "the draft reached readers unasked");
  assert.equal(publication.state.blocked, "");
}

// Every document keeps a reader version, shared or not: a document has one
// the way it has a title, and an author who wants somewhere nothing is
// prepared for readers opens another project.
{
  let published_ = 0;
  const { publication } = build({
    publisher: { publish: async () => { published_ += 1; return { id: "pub-2" }; } },
  });
  publication.state.metadataReady = true;
  publication.state.publication = null;
  await publication.catchUp();
  assert.equal(published_, 1, "a document never shared still has a reader version");
}

// A draft that does not render has nothing to publish. Readers keep the last
// version that did, and the author is told why rather than left watching a
// publication that never arrives.
{
  let published_ = 0;
  const { publication } = build({
    facts: { renderable: false },
    publisher: { publish: async () => { published_ += 1; return { id: "pub-2" }; } },
  });
  publication.state.metadataReady = true;
  publication.state.publication = published;
  publication.state.stale = true;
  await publication.catchUp();
  assert.equal(published_, 0, "a broken draft does not replace a working publication");
  assert.match(publication.state.blocked, /does not render/);
}

// A draft already published is left alone: catching up twice is not two
// publications.
{
  let published_ = 0;
  const { publication } = build({
    publisher: { publish: async () => { published_ += 1; return { id: "pub-2" }; } },
  });
  publication.state.metadataReady = true;
  publication.state.publication = published;
  publication.state.stale = false;
  await publication.catchUp();
  assert.equal(published_, 0, "nothing has moved, so nothing is rebuilt");
}

// A failed publish is reported where the author is, and does not leave the
// document looking as though it is still working on one.
{
  const { publication } = build({
    publisher: { publish: async () => { throw new Error("the bundle would not upload"); } },
  });
  publication.state.metadataReady = true;
  publication.state.publication = { ...published, source_sha256: "something-else" };
  publication.state.stale = true;
  await publication.catchUp();
  assert.equal(publication.state.publishing, false);
  assert.match(publication.state.blocked, /would not upload/);
}

console.log("reader publication: staleness is pessimistic, the reader version keeps itself current");

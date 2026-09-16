// The published version of a document: what it is, whether the source has
// moved past it, and what publishing a new one involves.
//
// A publication is an immutable bundle served from another origin. Two
// questions follow from that and both are answered here. Which publication is
// current -- the page watches for one arriving, because somebody else may
// publish while this reader is looking. And whether what is on screen is
// still what was published, which `document.sha` cannot answer: that is the
// main source file's digest alone, and a project is a tree.
//
// So "stale" is computed from two digests, the whole source tree and the
// renderer's identity, and it is deliberately pessimistic: a renderer
// identity that cannot be resolved marks the publication stale rather than
// claiming a match it could not establish.

import { createPublicationPublisher, createPublicationReader, sha256 as defaultSha256 } from "../publication.js";
import { capturePreviewTree } from "../assistant-preview.js";
import { buildDisplayBundle } from "../publication-builder.js";
import { snapshotDigest } from "../tree-digest.js";
import * as defaultRenderers from "../renderers.js";

export function createPublication({
  slug,
  key = "",
  renderers = defaultRenderers,
  sha256 = defaultSha256,
  publisher = createPublicationPublisher({ slug, key }),
  makeReader = createPublicationReader,
  // Turning a rendered page into a standalone bundle needs a DOM to walk,
  // which is why it is an argument: everything else here can be checked
  // without a browser, and this is the one step that cannot.
  buildBundle = buildDisplayBundle,
  /// The source as it stands, for publishing: the editor's own text included.
  liveTree,
  /// The source as the collaboration session has committed it. Status runs
  /// from the source observer, and CodeMirror can still be displaying its
  /// previous transaction at that point, so the committed tree is the honest
  /// one to compare against.
  committedTree,
  /// The document's title, for the renderer.
  heading,
  /// The figures the tree names. Publishing is strict: a bundle with a hole
  /// in it is worse than no bundle.
  gather,
  /// What is true right now: `{ canEdit, hasSession, source }`. Re-read after
  /// every await, because publishing takes seconds and a reader can type
  /// through all of them.
  facts,
  /// Said only to a link-holder, who is shown the published bundle itself:
  /// when its metadata cannot be read there is nothing on their screen to
  /// hang the reason on. An editor has the Share panel, which draws
  /// `metadataFailed` with a Retry beside it, and is told nothing here.
  say = () => {},
  disposed = () => false,
}) {
  const state = $state({
    /// The publication the document currently has, or null.
    publication: null,
    /// Somebody else published while this reader was looking, and they have
    /// not caught up with it yet.
    update: false,
    /// The source has moved past what was published.
    stale: false,
    /// Whether the published version has been read. Publishing waits for it:
    /// an update has to name the publication it expects to replace.
    metadataReady: false,
    metadataFailed: false,
  });

  let reader = null;
  // Whether the first metadata read has been scheduled for this session.
  let started = false;

  /// Watch for the publication an editor is working against.
  function watchAsEditor() {
    state.metadataReady = false;
    state.metadataFailed = false;
    started = false;
    reader?.dispose();
    reader = makeReader({
      slug,
      key,
      onPublication: (value) => { state.publication = value; },
      // The Share panel draws `metadataFailed` as a line with a Retry
      // button beside it, which is where an editor is when this matters.
      onError: () => { state.metadataFailed = true; },
    });
    return reader;
  }

  /// Watch as a reader who was given a link: they are shown the published
  /// bundle itself, and a newer one arriving is an offer to refresh rather
  /// than something to swap under them mid-sentence.
  function watchAsVisitor({ onPublication }) {
    reader?.dispose();
    reader = makeReader({
      slug,
      key,
      onPublication: (value) => {
        if (value?.pending_id && value.pending_id !== state.publication?.id) {
          state.update = true;
          return;
        }
        state.publication = value;
        state.update = false;
        onPublication?.(value);
      },
      onError: (error) => {
        state.metadataFailed = true;
        say(error.message || "The published version of this document could not be loaded.");
      },
    });
    return reader;
  }

  function dispose() {
    reader?.dispose();
    reader = null;
  }

  /// The first metadata read of a session, once and only once.
  function begin() {
    if (started || !reader) return false;
    started = true;
    return true;
  }

  function announce(event) {
    reader?.announce(event);
  }

  /// The reader asked for the newer published version they were offered.
  /// Clearing the offer first is what makes the button stop saying there is
  /// one while its refresh is in flight.
  function acceptUpdate() {
    state.update = false;
    return reader?.refresh();
  }

  /// Whether what is on screen has moved past what was published.
  async function refreshStatus(publication = state.publication) {
    const at = facts();
    if (!at.canEdit || !publication?.source_sha256) {
      state.stale = false;
      return;
    }
    const source = at.source;
    const current = () => facts().source === source && publication === state.publication;
    try {
      const tree = capturePreviewTree(committedTree());
      const revision = await snapshotDigest(tree);
      const configuration = await renderers.htmlConfiguration(tree);
      const renderConfig = await sha256(JSON.stringify(configuration.identity || {}));
      if (!current()) return;
      state.stale = revision !== publication.source_sha256
        || renderConfig !== publication.render_config_sha256;
    } catch {
      // If the local renderer identity cannot be resolved, do not claim that
      // the source matches the publication.
      if (current()) state.stale = true;
    }
  }

  /// Read the published version again, and say whether the source has moved
  /// past it.
  async function refreshMetadata() {
    if (!reader) return null;
    state.metadataReady = false;
    state.metadataFailed = false;
    const publication = await reader.refresh();
    if (disposed() || !facts().canEdit) return null;
    if (!state.metadataFailed) {
      await refreshStatus(publication);
      state.metadataReady = true;
    }
    return publication;
  }

  /// Publish what is on screen.
  ///
  /// The document is rendered here rather than reusing whatever the preview
  /// last produced: a publication is a standalone bundle and has to be built
  /// from a tree captured on purpose, with every figure present.
  async function publish() {
    const at = facts();
    if (!at.canEdit || !at.hasSession) throw new Error("Only the current editable document can be published.");
    if (!state.metadataReady) throw new Error("Checking the published version. Try again in a moment.");
    const tree = capturePreviewTree(liveTree());
    const sourceRevision = await snapshotDigest(tree);
    const configuration = await renderers.htmlConfiguration(tree);
    const gathered = await gather(tree.digests);
    tree.assets = { ...tree.assets, ...gathered.assets };
    tree.urls = { ...tree.urls, ...gathered.urls };
    const rendered = await renderers.render(tree, await heading(tree), { format: "html", manual: true, configuration });
    if (!rendered?.html || rendered.ok === false || rendered.diagnostics?.some((item) => item.severity === "error")) {
      throw new Error("The captured document could not be rendered as HTML. Fix its render errors and publish again.");
    }
    const bundle = await buildBundle(rendered.html, tree);
    let publication;
    try {
      publication = await publisher.publish({
        ...bundle,
        sourceRevision,
        renderConfig: configuration.identity,
        expectedPublicationId: state.publication?.id || null,
      });
    } catch (error) {
      if (error?.status === 409) {
        await refreshMetadata();
        throw new Error("A newer published version is now current. Review it, then choose Publish update again.");
      }
      throw error;
    }
    state.publication = publication;
    state.update = false;
    // Publishing takes seconds, and the reader can type through all of them.
    // Whether what is now on screen already differs from what was just
    // published is asked here rather than assumed to be "no".
    const source = facts().source;
    const liveCaptured = capturePreviewTree(liveTree());
    const [liveRevision, liveConfiguration] = await Promise.all([
      snapshotDigest(liveCaptured),
      renderers.htmlConfiguration(liveCaptured),
    ]);
    const liveRenderConfig = await sha256(JSON.stringify(liveConfiguration.identity || {}));
    state.stale = facts().source !== source
      || liveRevision !== sourceRevision
      || liveRenderConfig !== await sha256(JSON.stringify(configuration.identity || {}));
    return publication;
  }

  /// The id a new annotation is filed against.
  function id() {
    return state.publication?.publication_id || state.publication?.id || "";
  }

  return {
    state, watchAsEditor, watchAsVisitor, dispose, begin, announce, acceptUpdate,
    refreshStatus, refreshMetadata, publish, id,
  };
}

// The published version of a document: what it is, whether the source has
// moved past it, and what publishing a new one involves.
//
// A bundle is an immutable bundle served from another origin. Two
// questions follow from that and both are answered here. Which bundle is
// current -- the page watches for one arriving, because somebody else may
// publish while this reader is looking. And whether what is on screen is
// still what was published, which `document.sha` cannot answer: that is the
// main source file's digest alone, and a project is a tree.
//
// So "stale" is computed from two digests, the whole source tree and the
// renderer's identity, and it is deliberately pessimistic: a renderer
// identity that cannot be resolved marks the bundle stale rather than
// claiming a match it could not establish.

import { createBundlePublisher, createBundleReader, sha256 as defaultSha256 } from "../bundle.js";
import { capturePreviewTree } from "../assistant-preview.js";
import { buildDisplayBundle } from "../bundle-builder.js";
import { snapshotDigest } from "../tree-digest.js";
import * as defaultRenderers from "../renderers.js";

export function createBundle({
  slug,
  key = "",
  renderers = defaultRenderers,
  sha256 = defaultSha256,
  publisher = createBundlePublisher({ slug, key }),
  makeReader = createBundleReader,
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
    /// The bundle the document currently has, or null.
    bundle: null,
    /// Somebody else published while this reader was looking, and they have
    /// not caught up with it yet.
    update: false,
    /// The source has moved past what was published.
    stale: false,
    /// Whether the published version has been read. Publishing waits for it:
    /// an update has to name the bundle it expects to replace.
    metadataReady: false,
    metadataFailed: false,
    /// A bundle is being built and uploaded right now.
    publishing: false,
    /// Why the reader version is not current, when it is not. A render error
    /// is the usual reason and is the author's to fix; readers keep the last
    /// version that did render until they do.
    blocked: "",
  });

  let reader = null;
  // Whether the first metadata read has been scheduled for this session.
  let started = false;

  /// Watch for the bundle an editor is working against.
  function watchAsEditor() {
    state.metadataReady = false;
    state.metadataFailed = false;
    started = false;
    reader?.dispose();
    reader = makeReader({
      slug,
      key,
      onBundle: (value) => { state.bundle = value; },
      // The Share panel draws `metadataFailed` as a line with a Retry
      // button beside it, which is where an editor is when this matters.
      onError: () => { state.metadataFailed = true; },
    });
    return reader;
  }

  /// Watch as a reader who was given a link: they are shown the published
  /// bundle itself, and a newer one arriving is an offer to refresh rather
  /// than something to swap under them mid-sentence.
  function watchAsVisitor({ onBundle }) {
    reader?.dispose();
    reader = makeReader({
      slug,
      key,
      onBundle: (value) => {
        if (value?.pending_id && value.pending_id !== state.bundle?.id) {
          state.update = true;
          return;
        }
        state.bundle = value;
        state.update = false;
        onBundle?.(value);
      },
      onError: (error) => {
        state.metadataFailed = true;
        say(error.message || "The published version of this document could not be loaded.");
      },
    });
    return reader;
  }

  /// How long the source must sit still before the reader version is rebuilt.
  /// Matches the room's own quiet-checkpoint window: publishing renders the
  /// whole document again, so it waits for the same moment the room decides
  /// the document is at rest rather than racing every keystroke.
  const QUIET_MS = 30_000;
  let quiet = null;

  /// Keep the reader version current without being asked.
  ///
  /// Readers are never given the source -- they hold a link, and the source
  /// room refuses them -- so "what a reader sees" can only ever be a bundle
  /// somebody rendered for them. The rendering happens in this browser,
  /// because this is where the engines are. What used to be a button is
  /// therefore not a decision about whether readers may see the latest
  /// version; it was only ever the moment this tab got around to building it.
  /// So it builds it whenever the document has been left alone long enough to
  /// be worth naming, and says so rather than asking.
  ///
  /// Every document does this, shared or not. Whether anybody holds a link is
  /// not this code's question: a document has a reader version the way it has
  /// a title, and an author who wants somewhere nothing is prepared for
  /// readers opens another project.
  function keepCurrent() {
    clearTimeout(quiet);
    quiet = setTimeout(() => void catchUp(), QUIET_MS);
    // A pending rebuild is not a reason for a process to stay alive. In a
    // browser this is a number and there is nothing to unreference; under
    // Node, where the checks run, it is a handle that would hold the run
    // open for the whole quiet window.
    quiet?.unref?.();
  }

  async function catchUp() {
    const at = facts();
    if (disposed() || !at.canEdit || !at.hasSession) return;
    if (state.publishing || !state.metadataReady) return;
    if (state.bundle && !state.stale) {
      state.blocked = "";
      return;
    }
    // A draft that does not render has nothing to publish. Readers keep the
    // last version that did, which is the honest outcome -- but the author is
    // told, because from here it looks like nothing is happening.
    if (!at.renderable) {
      // Unless it is still rendering, which is a moment rather than a verdict
      // and so goes back on the clock instead of being given up on.
      //
      // An edit was the only other thing that armed this timer, so a document
      // opened and shared without being typed into had exactly one attempt:
      // the one its quiet window bought, which for a PDF format can elapse
      // before the engine has produced a first page. Giving up there left a
      // commenter reading "Nothing to read yet" for as long as nobody
      // happened to touch the source.
      if (at.rendering) {
        keepCurrent();
        return;
      }
      state.blocked = "This draft does not render, so readers still have the last version that did.";
      return;
    }
    state.publishing = true;
    try {
      await publish();
      state.blocked = "";
    } catch (error) {
      state.blocked = error?.message || "The reader version could not be updated.";
    } finally {
      state.publishing = false;
    }
  }

  function dispose() {
    clearTimeout(quiet);
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
  async function refreshStatus(bundle = state.bundle) {
    const at = facts();
    if (!at.canEdit || !bundle?.source_sha256) {
      state.stale = false;
      return;
    }
    const source = at.source;
    const current = () => facts().source === source && bundle === state.bundle;
    try {
      const tree = capturePreviewTree(committedTree());
      const revision = await snapshotDigest(tree);
      const configuration = await renderers.htmlConfiguration(tree);
      const renderConfig = await sha256(JSON.stringify(configuration.identity || {}));
      if (!current()) return;
      state.stale = revision !== bundle.source_sha256
        || renderConfig !== bundle.render_config_sha256;
    } catch {
      // If the local renderer identity cannot be resolved, do not claim that
      // the source matches the bundle.
      if (current()) state.stale = true;
    }
  }

  /// Read the published version again, and say whether the source has moved
  /// past it.
  async function refreshMetadata() {
    if (!reader) return null;
    state.metadataReady = false;
    state.metadataFailed = false;
    const bundle = await reader.refresh();
    if (disposed() || !facts().canEdit) return null;
    if (!state.metadataFailed) {
      await refreshStatus(bundle);
      state.metadataReady = true;
      // A document shared but never published, or left behind by an earlier
      // session, catches up on its own once what it is behind is known.
      keepCurrent();
    }
    return bundle;
  }

  /// Publish what is on screen.
  ///
  /// The document is rendered here rather than reusing whatever the preview
  /// last produced: a bundle is a standalone bundle and has to be built
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
    // The rendering and its assets, ready to store; `bundle` below is what
    // the server made of them.
    const built = await buildBundle(rendered.html, tree);
    let bundle;
    try {
      bundle = await publisher.publish({
        ...built,
        sourceRevision,
        renderConfig: configuration.identity,
        expectedBundleId: state.bundle?.id || null,
      });
    } catch (error) {
      if (error?.status === 409) {
        await refreshMetadata();
        throw new Error("A newer published version is now current. Review it, then choose Publish update again.");
      }
      throw error;
    }
    state.bundle = bundle;
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
    return bundle;
  }

  /// The id a new annotation is filed against.
  function id() {
    return state.bundle?.bundle_id || state.bundle?.id || "";
  }

  return {
    state, watchAsEditor, watchAsVisitor, dispose, begin, announce, acceptUpdate,
    refreshStatus, refreshMetadata, publish, keepCurrent, catchUp, id,
  };
}

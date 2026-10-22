<script>
  import { onMount } from "svelte";
  import { mode, onMode } from "../lib/theme.js";
  import { createFrameMessageReceiver } from "../lib/frame-messages.js";
  // The document, on its own origin, in a frame.
  //
  // Nothing here can touch it: the agent injected into it does the DOM work
  // and reports back. Anchoring stays on this side -- the agent sends text,
  // this sends back the offsets to paint.
  //
  // Everything arriving from the frame is untrusted. The injected agent
  // shares an origin with the document, and a hostile document can rewrite it.
  //
  // `controls` is whatever the format in the frame can be asked for -- page,
  // zoom, search and a cursor tool for a PDF, nothing for flowing HTML. It is
  // a snippet rather than anything this component knows about, and it stands
  // in a slim band across the top of the pane, above the frame and never over
  // it, so no control can cover a page. The pane is a column: the band takes
  // the height it needs and the frame the rest, and a format whose snippet
  // renders nothing leaves the frame the whole pane, so it loses no height at
  // all. What is being previewed is said in the Files pane, by the eye beside
  // the file, rather than spelled out again here.
  //
  // A thin progress line along the top edge signals a render under way without
  // covering the document, and nothing shows for renders under the
  // steady-busy threshold.
  let { src, docsOrigin, onmessage, onload, grabbing = false, away = false, controls, busy = false, progress = null } = $props();

  let frame = $state(null);
  let viewport = $state(null);
  let heldWidth = $state(0);
  let heldHeight = $state(0);
  let receiver;
  onMount(() => {
    receiver = createFrameMessageReceiver({
      getFrame: () => frame,
      getSrc: () => src,
      getDocsOrigin: () => docsOrigin,
      onmessage: (message) => onmessage?.(message),
    });
    window.addEventListener("message", receiver.receive);
    const observer = new ResizeObserver(() => {
      if (away || !viewport) return;
      heldWidth = viewport.clientWidth;
      heldHeight = viewport.clientHeight;
    });
    observer.observe(viewport);
    const stopMode = onMode((next) => tell({ type: "theme", mode: next }));
    const standard = () => document.fullscreenElement === frame;
    const current = () => standard() || /** @type {any} */ (document).webkitFullscreenElement === frame;
    let presenting = false;
    const changed = () => {
      const on = !!frame && current();
      if (on === presenting) return;
      presenting = on;
      tell({ type: "viewer-present", on });
    };
    document.addEventListener("fullscreenchange", changed);
    document.addEventListener("webkitfullscreenchange", changed);
    return () => {
      document.removeEventListener("fullscreenchange", changed);
      document.removeEventListener("webkitfullscreenchange", changed);
      stopMode();
      window.removeEventListener("message", receiver.receive);
      receiver.dispose();
      observer.disconnect();
    };
  });

  /// `transfer` is for the one message that carries megabytes: a LaTeX
  /// document's pages arrive as PDF bytes, and handing the buffer over rather
  /// than copying it saves the copy on every recompile. Everything else is
  /// small and is cloned, as it always was.
  export function tell(message, transfer) {
    if (!frame?.contentWindow) return false;
    let target;
    let trusted;
    try {
      target = new URL(src, location.href);
      trusted = new URL(docsOrigin, location.href).origin;
    } catch {
      return false;
    }
    if (!trusted || trusted === location.origin || target.origin !== trusted) return false;
    frame.contentWindow.postMessage({ librepaper: true, ...message }, trusted, transfer);
    return true;
  }

  /// Presentation mode. A message carries no user activation, so the frame
  /// cannot go fullscreen itself; this has to run inside the click, with
  /// nothing awaited first. The frame is told once the browser has actually
  /// changed state, which also covers leaving with Escape.
  export function present() {
    if (!frame) return;
    if (frame.requestFullscreen) frame.requestFullscreen().catch(() => {});
    else frame.webkitRequestFullscreen?.();
  }

  /// Where "Focus Preview" goes. The frame rather than the section around it:
  /// focusing the iframe puts the keyboard inside the document, so the arrow
  /// keys scroll the pages instead of whatever was focused behind them.
  export function focus() {
    frame?.focus();
  }

</script>

<section class="viewport" class:away bind:this={viewport} inert={away}
         style:--held-width="{heldWidth}px" style:--held-height="{heldHeight}px">
  {#if busy}
    <div class="render-line" role="progressbar" aria-label="Rendering preview" aria-valuemin="0" aria-valuemax={progress ? progress.total : undefined} aria-valuenow={progress ? progress.done : undefined}>
      <span class="render-line-bar" class:indeterminate={!progress} style:width={progress ? `${Math.min(100, (progress.done / progress.total) * 100)}%` : null}></span>
    </div>
  {/if}
  {@render controls?.()}
  <!-- allow-same-origin refers to the document's own origin, not this one, so
       the agent can read the document while the document can read nothing
       here. While a separator is being dragged the frame is deafened: it
       swallows pointer events while it has them, and the drag would be lost
       over it. -->
  {#key src}
    <iframe
      bind:this={frame}
      title="Document"
      {src}
      onload={() => {
        receiver?.frameLoaded();
        onload?.();
        tell({ type: "reader-ready" });
        tell({ type: "theme", mode: mode() });
      }}
      style:pointer-events={grabbing ? "none" : null}
      allow="fullscreen"
      sandbox="allow-same-origin allow-scripts allow-popups allow-popups-to-escape-sandbox allow-forms"
    ></iframe>
  {/key}
</section>

<style>
  .render-line {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 2px;
    overflow: hidden;
    pointer-events: none;
    z-index: 9;
  }

  .render-line-bar {
    height: 100%;
    background: var(--color-brand);
    transition: width 120ms linear;
  }

  .render-line-bar.indeterminate {
    width: 35%;
    animation: render-sweep 1.1s ease-in-out infinite;
  }

  @keyframes render-sweep {
    from { transform: translateX(-100%); }
    to { transform: translateX(285%); }
  }

  @media (prefers-reduced-motion: reduce) {
    .render-line-bar.indeterminate {
      animation: none;
      width: 100%;
      opacity: .5;
    }
  }
</style>

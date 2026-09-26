<script>
  import { onMount } from "svelte";
  // The document, on its own origin, in a frame.
  //
  // Nothing here can touch it: the agent injected into it does the DOM work
  // and reports back. Anchoring stays on this side -- the agent sends text,
  // this sends back the offsets to paint.
  //
  // Everything arriving from the frame is untrusted. The agent shares an
  // origin with the document, and a hostile document can rewrite it.
  //
  // `controls` is whatever the format in the frame can be asked for -- zoom
  // and a cursor tool for a PDF, nothing for flowing HTML. It is a snippet
  // rather than anything this component knows about, and it floats over the
  // document rather than standing in a row above it: a band across the top of
  // the pane cost every format a strip of the document's height, including
  // the formats with nothing to put in it. What is being previewed is said in
  // the Files pane, by the eye beside the file, rather than spelled out again
  // here.
  let { src, docsOrigin, onmessage, onload, grabbing = false, away = false, controls } = $props();

  let frame = $state(null);
  let viewport = $state(null);
  let heldWidth = $state(0);
  let heldHeight = $state(0);
  onMount(() => {
    window.addEventListener("message", receive);
    const observer = new ResizeObserver(() => {
      if (away || !viewport) return;
      heldWidth = viewport.clientWidth;
      heldHeight = viewport.clientHeight;
    });
    observer.observe(viewport);
    return () => {
      window.removeEventListener("message", receive);
      observer.disconnect();
    };
  });

  /// `transfer` is for the one message that carries megabytes: a LaTeX
  /// document's pages arrive as PDF bytes, and handing the buffer over rather
  /// than copying it saves the copy on every recompile. Everything else is
  /// small and is cloned, as it always was.
  export function tell(message, transfer) {
    if (!frame?.contentWindow) return false;
    const origin = frame.src ? new URL(frame.src).origin : docsOrigin;
    if (!origin) return false;
    frame.contentWindow.postMessage({ librepaper: true, ...message }, origin, transfer);
    return true;
  }

  /// Where "Focus Preview" goes. The frame rather than the section around it:
  /// focusing the iframe puts the keyboard inside the document, so the arrow
  /// keys scroll the pages instead of whatever was focused behind them.
  export function focus() {
    frame?.focus();
  }

  function receive(event) {
    const origin = frame?.src ? new URL(frame.src).origin : docsOrigin;
    if (!origin || event.origin !== origin || event.source !== frame?.contentWindow) return;
    const message = event.data;
    if (!message || message.librepaper !== true) return;
    onmessage(message);
  }
</script>

<section class="viewport" class:away bind:this={viewport} inert={away}
         style:--held-width="{heldWidth}px" style:--held-height="{heldHeight}px">
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
        onload?.();
        tell({ type: "reader-ready" });
      }}
      style:pointer-events={grabbing ? "none" : null}
      sandbox="allow-same-origin allow-scripts allow-popups allow-forms"
    ></iframe>
  {/key}
</section>

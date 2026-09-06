<script>
  // The document, on its own origin, in a frame.
  //
  // Nothing here can touch it: the agent injected into it does the DOM work
  // and reports back. Anchoring stays on this side -- the agent sends text,
  // this sends back the offsets to paint.
  //
  // Everything arriving from the frame is untrusted. The agent shares an
  // origin with the document, and a hostile document can rewrite it.
  let { src, docsOrigin, onmessage, grabbing = false, away = false } = $props();

  let frame = $state(null);

  /// `transfer` is for the one message that carries megabytes: a LaTeX
  /// document's pages arrive as PDF bytes, and handing the buffer over rather
  /// than copying it saves the copy on every recompile. Everything else is
  /// small and is cloned, as it always was.
  export function tell(message, transfer) {
    if (!docsOrigin || !frame?.contentWindow) return false;
    frame.contentWindow.postMessage({ komodoc: true, ...message }, docsOrigin, transfer);
    return true;
  }

  function receive(event) {
    if (!docsOrigin || event.origin !== docsOrigin || event.source !== frame?.contentWindow) return;
    const message = event.data;
    if (!message || message.komodoc !== true) return;
    onmessage(message);
  }
</script>

<svelte:window onmessage={receive} />

<section class="viewport" class:away>
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
      style:pointer-events={grabbing ? "none" : null}
      sandbox="allow-same-origin allow-scripts allow-popups allow-forms"
    ></iframe>
  {/key}
</section>

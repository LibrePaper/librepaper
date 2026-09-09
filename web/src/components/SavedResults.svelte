<script>
  import { safeFragment } from "../lib/results-content.js";
  let { items = [], assets = {}, renderId = "", canComment = false, selectedRegion = null, oncomment } = $props();
  let drawingItem = $state(null);
  let drag = $state(null);
  const point = (event) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return { x:Math.max(0, Math.min(100, 100 * (event.clientX - rect.left) / rect.width)),
      y:Math.max(0, Math.min(100, 100 * (event.clientY - rect.top) / rect.height)) };
  };
  const rectangle = (a, b) => ({ x:Math.min(a.x,b.x), y:Math.min(a.y,b.y), w:Math.abs(a.x-b.x), h:Math.abs(a.y-b.y) });
  const regionStyle = (region) => `left:${region.x}%;top:${region.y}%;width:${region.w}%;height:${region.h}%`;
  function startRegion(event, item) {
    if (drawingItem !== item || event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    const start = point(event);
    drag = { item, start, current:start, pointer:event.pointerId };
  }
  function finishRegion(event, item) {
    if (drag?.item !== item || drag.pointer !== event.pointerId) return;
    const region = rectangle(drag.start, point(event));
    drag = null;
    if (region.w < 0.5 || region.h < 0.5) return;
    drawingItem = null;
    oncomment?.(item, { width:event.currentTarget.naturalWidth, height:event.currentTarget.naturalHeight,
      region:{ ...region, image_digest:item.digest, image_index:0 } });
  }
  const tableDocument = (text) => '<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src \'none\'; style-src \'unsafe-inline\'"><style>body{font:14px system-ui}table{border-collapse:collapse}td,th{padding:.4rem;border:1px solid currentColor}</style>' + safeFragment(text);
</script>

<p class="text-xs text-surface-600-400">Saved render: {renderId}. Discussions stay attached to these exact results.</p>
{#each items as item (`${item.cell.id}:${item.output.ordinal}`)}
  <section class="border-surface-300-700 rounded border p-3 flex flex-col gap-2">
    <strong class="text-sm">{item.cell.label || item.cell.id} · result {item.output.ordinal + 1}</strong>
    {#if item.output.kind === "image" && assets[item.output.asset]}
      <div class="relative self-start max-w-full">
        <img class="block max-w-full" style={drawingItem === item ? "touch-action:none;cursor:crosshair" : ""}
          src={assets[item.output.asset]} alt={item.output.caption || "Saved computation result"}
          onpointerdown={(event) => startRegion(event,item)}
          onpointermove={(event) => { if (drag?.item === item && drag.pointer === event.pointerId) drag = { ...drag, current:point(event) }; }}
          onpointerup={(event) => finishRegion(event,item)} onpointercancel={() => { drag = null; }} />
        {#if drag?.item === item}<span class="absolute border-2 border-primary-500 bg-primary-500/20 pointer-events-none" style={regionStyle(rectangle(drag.start,drag.current))}></span>{/if}
        {#if selectedRegion}<span class="absolute border-2 border-primary-500 bg-primary-500/20 pointer-events-none" style={regionStyle(selectedRegion)}></span>{/if}
      </div>
    {:else if item.output.kind === "table"}
      <iframe title="Saved result table" sandbox="" srcdoc={tableDocument(item.output.text || "")} class="w-full h-64 border-0"></iframe>
    {:else}
      <pre class="whitespace-pre-wrap overflow-auto text-sm">{item.output.text || "This result cannot be displayed inline."}</pre>
    {/if}
    {#if item.output.caption}<p class="text-sm">{item.output.caption}</p>{/if}
    {#if canComment && /^[a-f0-9]{64}$/.test(item.digest)}
      <button class="btn btn-sm preset-tonal-surface self-start" onclick={() => oncomment?.(item)}>Comment on this result</button>
      {#if item.output.kind === "image" && assets[item.output.asset]}
        <button class="btn btn-sm preset-tonal-surface self-start" aria-pressed={drawingItem === item}
          onclick={() => { drag = null; drawingItem = drawingItem === item ? null : item; }}>{drawingItem === item ? "Cancel region selection" : "Comment on a region"}</button>
        {#if drawingItem === item}<p class="text-sm">Drag a rectangle over the saved image, then write your comment.</p>{/if}
      {/if}
    {/if}
  </section>
{:else}
  <p>No individually captured results are available in this render.</p>
{/each}

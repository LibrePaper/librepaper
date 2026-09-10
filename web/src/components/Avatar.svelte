<script>
  import Icon from "./Icon.svelte";
  // Who said something, as a small round badge: the initials of the name on a
  // colour that is the same every time that name appears, so a reader learns
  // to tell the participants apart without reading. The full name lives in
  // the tooltip. An agent wears the bot icon instead of letters, and an
  // account whose provider has a picture of it wears that: the initials are
  // painted underneath, so a picture that never arrives -- offline, blocked,
  // or a stale URL -- leaves the badge everyone else already has.
  let { name = "", key = "", icon = "", title = "", size = 7, src = "" } = $props();
  let broken = $state("");
  const PALETTE = ["primary", "secondary", "tertiary", "success", "warning", "error"].map((tone) => `var(--color-${tone}-500)`);
  function colour(seed) {
    let hash = 0;
    for (const char of String(seed)) hash = (hash * 31 + char.codePointAt(0)) >>> 0;
    return PALETTE[hash % PALETTE.length];
  }
  function initials(value) {
    const words = String(value || "?").trim().replace(/^@/, "").split(/\s+/).filter(Boolean);
    const letters = words.length > 1 ? words.slice(0, 2).map((word) => word[0]) : [words[0]?.slice(0, 1) || "?"];
    return letters.join("").toUpperCase();
  }
  const seed = $derived(key || name);
</script>

<span class="avatar" style:background={colour(seed)} style:--avatar-size={`calc(var(--spacing) * ${size})`} title={title || name} aria-label={title || name} role="img">
  {#if icon}<Icon name={icon} />{:else}{initials(name)}{/if}
  {#if src && src !== broken}
    <img class="avatar-picture" {src} alt="" referrerpolicy="no-referrer" onerror={() => { broken = src; }} />
  {/if}
</span>

<style>
  .avatar { position: relative; display: inline-grid; place-items: center; width: var(--avatar-size); height: var(--avatar-size); flex-shrink: 0; border-radius: 50%; overflow: hidden; color: white; font-size: var(--panel-meta-size); font-weight: 600; line-height: 1; user-select: none; vertical-align: middle; }
  .avatar :global(svg) { width: 60%; height: 60%; }
  .avatar-picture { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; }
</style>

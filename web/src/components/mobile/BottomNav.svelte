<script>
  import Icon from "../Icon.svelte";
  import { m } from "../../lib/paraglide/messages.js";

  // The three places a phone reader can be: the document, its comments, and
  // everything else behind More. Anything that belongs to the editor lives in
  // the keyboard bar instead, so this row never has to know about editing.
  let {
    active = "",
    commentsDisabled = false,
    commentsDot = false,
    moreOpen = false,
    ondocument = () => {},
    oncomments = () => {},
    onmore = () => {},
  } = $props();
</script>

<!-- A div around the nav, because the page puts this directly under <body>,
     where the site bar's `body > nav` rule would otherwise pin it to the top. -->
<div class="mobile-bottom-nav"><nav aria-label={m.mobilenav_navigation()}>
  <button
    type="button"
    data-destination="document"
    class:on={active === "document"}
    aria-current={active === "document" ? "page" : undefined}
    onclick={(event) => ondocument(event)}
  >
    <Icon name="file-text" size={20} />
    <span class="label">{m.common_document()}</span>
  </button>
  <button
    type="button"
    data-destination="comments"
    class:on={active === "comments"}
    aria-current={active === "comments" ? "page" : undefined}
    disabled={commentsDisabled}
    onclick={(event) => oncomments(event)}
  >
    <span class="icon-wrap">
      <Icon name="message-square" size={20} />
      {#if commentsDot}<span class="dot" aria-hidden="true"></span>{/if}
    </span>
    <span class="label">{m.common_comments()}</span>
  </button>
  <button
    type="button"
    data-destination="more"
    class:on={moreOpen}
    aria-haspopup="dialog"
    aria-expanded={moreOpen}
    onclick={(event) => onmore(event)}
  >
    <Icon name="ellipsis" size={20} />
    <span class="label">{m.mobilenav_more()}</span>
  </button>
</nav></div>

<style>
  .mobile-bottom-nav {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 20;
    height: calc(3.5rem + env(safe-area-inset-bottom));
    padding-bottom: env(safe-area-inset-bottom);
    border-top: 1px solid var(--color-pane-edge);
    background: var(--color-shell);
  }

  nav {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    height: 100%;
  }

  /* Each destination fills its column, so the whole cell is the touch target
     and it is never smaller than 44px. */
  button {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.125rem;
    min-width: 0;
    min-height: 44px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--color-text-secondary);
    font: inherit;
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  button:focus-visible {
    outline: 2px solid var(--color-focus);
    outline-offset: -2px;
  }

  button.on,
  button[aria-current="page"] {
    color: var(--color-brand);
  }

  .label {
    font-size: 0.75rem;
    line-height: 1;
  }

  .icon-wrap {
    position: relative;
    display: flex;
  }

  .dot {
    position: absolute;
    top: -1px;
    right: -4px;
    width: 7px;
    height: 7px;
    border-radius: 9999px;
    background: var(--color-brand);
  }
</style>

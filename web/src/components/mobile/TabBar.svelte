<script>
  import Icon from "../Icon.svelte";
  import { m } from "../../lib/paraglide/messages.js";

  // Where a phone reader can be: the document, the files, and the comments,
  // and More for everything else. Files and Comments are left out where the
  // reader is not offered them (a read link has no comments).
  let {
    active = "",
    filesShown = true,
    filesDisabled = false,
    commentsShown = true,
    commentsDot = false,
    moreOpen = false,
    offlineNote = "",
    ondocument = () => {},
    onfiles = () => {},
    oncomments = () => {},
    onmore = () => {},
    // While the page scrolls down the bar steps aside, like the mode switch.
    // Focus landing in it brings it back, so a keyboard user is never left
    // tabbing into an invisible bar.
    hidden = false,
    onreveal = () => {},
  } = $props();

  const moreLabel = $derived(offlineNote ? `${m.mobilenav_more()}, ${offlineNote}` : m.mobilenav_more());
</script>

<!-- A div around the nav, not the nav itself: the page puts this directly under
     <body>, where the site bar's global `body > nav` rule would apply. -->
<div class="mobile-tab-bar" class:hidden onfocusin={onreveal}>
  <nav aria-label={m.mobilenav_navigation()}>
    <button
      type="button"
      data-destination="document"
      aria-current={active === "document" ? "page" : undefined}
      onclick={(event) => ondocument(event)}
    >
      <Icon name="file-text" size={20} />
      <span class="label">{m.common_document()}</span>
    </button>
    {#if filesShown}
      <button
        type="button"
        data-destination="files"
        aria-current={active === "files" ? "page" : undefined}
        disabled={filesDisabled}
        onclick={(event) => onfiles(event)}
      >
        <Icon name="folder" size={20} />
        <span class="label">{m.common_files()}</span>
      </button>
    {/if}
    {#if commentsShown}
      <button
        type="button"
        data-destination="comments"
        aria-current={active === "comments" ? "page" : undefined}
        onclick={(event) => oncomments(event)}
      >
        <span class="icon-wrap">
          <Icon name="message-square" size={20} />
          {#if commentsDot}<span class="dot" aria-hidden="true"></span>{/if}
        </span>
        <span class="label">{m.common_comments()}</span>
      </button>
    {/if}
    <button
      type="button"
      class="mobile-more-trigger"
      data-destination="more"
      class:open={moreOpen}
      aria-label={moreLabel}
      title={offlineNote ? moreLabel : undefined}
      aria-haspopup="dialog"
      aria-expanded={moreOpen}
      onclick={(event) => onmore(event)}
    >
      <span class="icon-wrap">
        <Icon name="ellipsis" size={20} />
        {#if offlineNote}<span class="dot offline" aria-hidden="true"></span>{/if}
      </span>
      <span class="label">{m.mobilenav_more()}</span>
    </button>
  </nav>
</div>

<style>
  .mobile-tab-bar {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 20;
    height: calc(3.5rem + env(safe-area-inset-bottom));
    padding-bottom: env(safe-area-inset-bottom);
    padding-inline: env(safe-area-inset-left) env(safe-area-inset-right);
    border-top: 1px solid var(--color-pane-edge);
    background: var(--color-shell);
  }

  /* The bar steps aside while the page scrolls down, with the mode switch, which
     gives the document its room back. It comes back the moment focus lands in it. */
  .mobile-tab-bar.hidden:not(:focus-within) {
    transform: translateY(100%);
    visibility: hidden;
  }

  @media (prefers-reduced-motion: no-preference) {
    .mobile-tab-bar {
      transition: transform 180ms ease, visibility 180ms ease;
    }
  }

  nav {
    display: flex;
    height: 100%;
  }

  /* Each takes the room its word needs and they share what is left: "Commentaires"
     does not fit a quarter of a 320px bar, and "Files" does not need one. */
  button {
    display: flex;
    flex: 1 1 auto;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.25rem;
    min-width: 0;
    min-height: 2.75rem;
    padding: 0 0.125rem;
    border: 0;
    border-radius: var(--radius-base);
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

  button[aria-current="page"],
  button.open {
    color: var(--color-brand);
  }

  .label {
    max-width: 100%;
    overflow: hidden;
    font-size: 0.75rem;
    line-height: 1;
    text-overflow: ellipsis;
    white-space: nowrap;
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

  .dot.offline {
    background: var(--color-error-solid);
    box-shadow: 0 0 0 2px var(--color-shell);
  }
</style>

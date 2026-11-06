<script>
  import Icon from "../Icon.svelte";
  import { m } from "../../lib/paraglide/messages.js";

  // Where a phone reader can be: the document, the files, and the comments.
  // Files and Comments are left out where the reader is not offered them (a
  // read link has no comments). Everything else is behind More.
  let {
    active = "",
    filesShown = true,
    filesDisabled = false,
    commentsShown = true,
    commentsDot = false,
    ondocument = () => {},
    onfiles = () => {},
    oncomments = () => {},
  } = $props();
</script>

<div class="mobile-header-tabs" role="group" aria-label={m.mobilenav_navigation()}>
  <button
    type="button"
    data-destination="document"
    aria-current={active === "document" ? "page" : undefined}
    onclick={(event) => ondocument(event)}
  >
    <Icon name="file-text" size={18} />
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
      <Icon name="folder" size={18} />
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
        <Icon name="message-square" size={18} />
        {#if commentsDot}<span class="dot" aria-hidden="true"></span>{/if}
      </span>
      <span class="label">{m.common_comments()}</span>
    </button>
  {/if}
</div>

<style>
  .mobile-header-tabs {
    display: flex;
    flex: 1 1 0;
    min-width: 0;
  }

  /* Each takes the room its word needs and they share what is left: "Commentaires"
     does not fit a third of a 320px bar, and "Fichiers" does not need one. */
  button {
    display: flex;
    flex: 1 1 auto;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.125rem;
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

  button[aria-current="page"] {
    color: var(--color-brand);
  }

  .label {
    max-width: 100%;
    overflow: hidden;
    font-size: 0.6875rem;
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
</style>

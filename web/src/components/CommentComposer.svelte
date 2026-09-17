<script>
  import { isSuggestion } from "../lib/annotating.js";

  // Writing the note, in the column where it will live rather than in a
  // dialog over the document it is about. It is the same card as the notes
  // around it, in the place the new one will take, so the passage stays
  // visible and ringed while the words are found for it.
  let {
    // The anchor being written about: what the selection bar captured.
    pending,
    // The verb chosen on the bar -- `comment` or `suggest`. Highlighting
    // never reaches here: the passage is the whole annotation.
    verb = "comment",
    identity = "",
    commentingAs = "Anonymous",
    // Signed out, and the document is one that insists on an account.
    needsLogin = false,
    signInHref = "",
    // What the replacement field starts with, worked out by the page.
    prefill = "",
    // Answers false when the room refused the submission, which leaves the
    // words where they are rather than throwing them away.
    onsend,
    oncancel,
  } = $props();

  const suggesting = $derived(isSuggestion(verb));
  let body = $state("");
  let proposed = $state(prefill);
  let field = $state(null);

  const quotation = $derived(
    pending?.point ? "A note at this point" : `“${pending?.exact ?? ""}”`,
  );

  $effect(() => {
    field?.focus({ preventScroll: true });
  });

  function send() {
    if (needsLogin) return;
    if (!suggesting && !body.trim()) return;
    if (onsend?.({ body, proposed: suggesting ? proposed : undefined }) === false) return;
    body = "";
    proposed = "";
  }

  // The keys the reply box already uses, so writing a note and answering one
  // are the same gesture. A suggestion is exempt from Enter: a replacement is
  // prose that may want its own line breaks, and losing one to a send is
  // worse than reaching for the button.
  function key(event) {
    if (event.key === "Escape") {
      event.preventDefault();
      oncancel?.();
      return;
    }
    if (suggesting || event.key !== "Enter" || event.shiftKey || event.ctrlKey || event.isComposing || event.keyCode === 229) return;
    event.preventDefault();
    send();
  }
</script>

<article id="composer" class="card composer preset-outlined-primary-500 p-2"
         aria-label={suggesting ? "Suggest a change" : "New comment"}>
  <div class="flex flex-col gap-2">
    <blockquote class="quote border-primary-500 border-l-2 pl-3 text-sm">{quotation}</blockquote>

    {#if needsLogin}
      <p class="text-sm">
        <a class="anchor" href={signInHref}>Sign in</a>
        to {suggesting ? "suggest a change to" : "comment on"} this document.
      </p>
    {:else}
      <!-- No name to type: the server hands out a per-document pseudonym for
           an anonymous commenter, so this is only ever a statement. -->
      <p class="panel-meta">{suggesting ? "suggesting" : "commenting"} as {identity || commentingAs}</p>
    {/if}

    {#if suggesting}
      <label class="label">
        <span class="label-text">Suggested replacement</span>
        <textarea
          class="textarea"
          rows="4"
          maxlength="5000"
          disabled={needsLogin}
          bind:this={field}
          bind:value={proposed}
          onkeydown={key}
          placeholder="Leave empty to suggest deleting the passage"
        ></textarea>
      </label>
      <label class="label">
        <span class="label-text">Note (optional)</span>
        <textarea class="textarea" rows="2" maxlength="5000" disabled={needsLogin}
                  bind:value={body} onkeydown={key}></textarea>
      </label>
    {:else}
      <textarea
        class="textarea"
        aria-label="Comment"
        rows="3"
        maxlength="5000"
        disabled={needsLogin}
        bind:this={field}
        bind:value={body}
        onkeydown={key}
        placeholder="Comment · Enter sends · Esc cancels"
      ></textarea>
    {/if}

    <div class="flex items-center justify-end gap-2">
      <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => oncancel?.()}>Cancel</button>
      <button type="button" class="btn btn-sm preset-filled-primary-500" data-send
              disabled={needsLogin || (!suggesting && !body.trim())}
              onclick={send}>{suggesting ? "Suggest" : "Comment"}</button>
    </div>
  </div>
</article>

<style>
  /* The one card in the column that is not yet an annotation, ringed like the
     selected one because the passage it is about is ringed in the document. */
  .composer { box-shadow: 0 0 0 1px var(--color-primary-500); }
  .quote { overflow-wrap: anywhere; }
</style>

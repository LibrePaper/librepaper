<script>
  // Writing the note, in the column where it will live rather than in a
  // dialog over the document it is about. It is the same card as the notes
  // around it, in the place the new one will take, so the passage stays
  // visible and ringed while the words are found for it.
  let {
    // The anchor being written about: what the selection bar captured.
    pending,
    identity = "",
    commentingAs = "Anonymous",
    // Signed out, and the document is one that insists on an account.
    needsLogin = false,
    signInHref = "",
    // Answers false when the room refused the submission, which leaves the
    // words where they are rather than throwing them away.
    onsend,
    oncancel,
  } = $props();

  let body = $state("");
  let field = $state(null);

  const quotation = $derived(
    `“${pending?.exact ?? ""}”`,
  );

  $effect(() => {
    field?.focus({ preventScroll: true });
  });

  function send() {
    if (needsLogin) return;
    if (!body.trim()) return;
    if (onsend?.({ body }) === false) return;
    body = "";
  }

  // The keys the reply box already uses, so writing a note and answering one
  // are the same gesture.
  function key(event) {
    if (event.key === "Escape") {
      event.preventDefault();
      oncancel?.();
      return;
    }
    if (event.key !== "Enter" || event.shiftKey || event.ctrlKey || event.isComposing || event.keyCode === 229) return;
    event.preventDefault();
    send();
  }
</script>

<article id="composer" class="card composer lp-control-outline-brand p-2" aria-label="New comment">
  <div class="flex flex-col gap-2">
    <blockquote class="quote border-[var(--color-brand)] border-l-2 pl-3 text-sm">{quotation}</blockquote>

    {#if needsLogin}
      <p class="text-sm">
        <a class="anchor" href={signInHref}>Sign in</a>
        to comment on this document.
      </p>
    {:else}
      <!-- No name to type: the server hands out a per-document pseudonym for
           an anonymous commenter, so this is only ever a statement. -->
      <p class="panel-meta">commenting as {identity || commentingAs}</p>
    {/if}

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

    <div class="flex items-center justify-end gap-2">
      <button type="button" class="btn btn-sm lp-control-outline" onclick={() => oncancel?.()}>Cancel</button>
      <button type="button" class="btn btn-sm lp-control-brand" data-send
              disabled={needsLogin || !body.trim()}
              onclick={send}>Comment</button>
    </div>
  </div>
</article>

<style>
  /* The one card in the column that is not yet an annotation, ringed like the
     selected one because the passage it is about is ringed in the document. */
  .composer { box-shadow: 0 0 0 1px var(--color-brand); }
  .quote { overflow-wrap: anywhere; }
</style>

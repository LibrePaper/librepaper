<script>
  import { m } from "../lib/paraglide/messages.js";
  import { isSendKey } from "../lib/send-key.js";
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
    // The words being written. They are the list's to keep (`bind:body`): this
    // card's place in the column follows the cards around it, so it can be
    // taken down and put up again mid-sentence.
    body = $bindable(""),
  } = $props();

  let field = $state(null);
  let card = $state(null);

  // A remark about the whole document quotes nothing, so the draft names what
  // it is about instead.
  const quotation = $derived(
    pending?.whole
      ? m.common_whole_document()
      : m.commentcomposer_quotation({ exact: pending?.exact ?? "" }),
  );

  // The card mounts as the Comments tab is opened, and that tab or its panel
  // may not be showing yet, so focus is retried for a short while until the
  // field is visible, then the card is scrolled into place and focused.
  $effect(() => {
    if (!field) return;
    let frame = 0;
    let tries = 0;
    const attempt = () => {
      const visible = field.checkVisibility?.() ?? field.offsetParent !== null;
      if (visible) {
        // The card first, for its quotation; then the field, which a long
        // quotation can push below the column's edge.
        card?.scrollIntoView({ block: "nearest" });
        field.scrollIntoView({ block: "nearest" });
        field.focus({ preventScroll: true });
        return;
      }
      if (++tries < 30) frame = requestAnimationFrame(attempt);
    };
    frame = requestAnimationFrame(attempt);
    return () => cancelAnimationFrame(frame);
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
    if (!isSendKey(event)) return;
    event.preventDefault();
    send();
  }
</script>

<article bind:this={card} id="composer" class="card composer" aria-label={m.commentcomposer_aria()}>
  <div class="flex flex-col gap-2">
    <blockquote class="quote border-[var(--color-brand)] border-l-2 pl-3 text-sm">{quotation}</blockquote>

    {#if needsLogin}
      <p class="text-sm">
        <a class="anchor" href={signInHref}>{m.common_sign_in()}</a>
        {m.commentcomposer_sign_in_rest()}
      </p>
    {:else}
      <!-- No name to type: the server hands out a per-document pseudonym for
           an anonymous commenter, so this is only ever a statement. -->
      <p class="panel-meta">{m.commentcomposer_commenting_as({ name: identity || commentingAs })}</p>
    {/if}

    <textarea
      class="textarea"
      aria-label={m.commentcomposer_field()}
      rows="3"
      maxlength="5000"
      disabled={needsLogin}
      bind:this={field}
      bind:value={body}
      onkeydown={key}
      placeholder={m.commentcomposer_placeholder()}
    ></textarea>

    <div class="flex items-center justify-end gap-2">
      <button type="button" class="btn btn-sm lp-control-tonal-brand" onclick={() => oncancel?.()}>{m.common_cancel()}</button>
      <button type="button" class="btn btn-sm lp-control-brand" data-send
              disabled={needsLogin || !body.trim()}
              onclick={send}>{m.common_comment()}</button>
    </div>
  </div>
</article>

<style>
  /* The one card in the column that is not yet an annotation. It is the same
     box as the comment cards around it (CommentCard.svelte), so its words line
     up with theirs, and its border is the ring: the passage it is about is
     ringed in the document. The ring is the border rather than a shadow
     because the list this sits in scrolls, and a shadow drawn outside the card
     is clipped at the list's edges. */
  .card.composer { box-sizing: border-box; min-width: 0; padding: 10px 13px; border: 1px solid var(--color-focus); border-radius: 9px; background: var(--color-raised); }
  .quote { overflow-wrap: anywhere; }
  /* Phones: the composer's field and buttons are finger-sized, with 16px text. */
  @media (max-width: 767px) {
    textarea { font-size: 1rem; }
    .btn { min-height: 2.75rem; }
  }
</style>

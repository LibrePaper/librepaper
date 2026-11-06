<script>
  import Avatar from "../Avatar.svelte";
  import { m } from "../../lib/paraglide/messages.js";

  // Who else is in the document, written out. A phone has no hover to reveal
  // a name on an avatar, so the name sits beside each one. Renders nothing
  // when nobody else is here.
  let { participants = [] } = $props();
</script>

{#if participants.length}
  <section class="mobile-presence" aria-label={m.mobilenav_people_here()}>
    <ul class="chips">
      {#each participants as participant (participant.key)}
        <li class="chip">
          <Avatar name={participant.name} key={participant.key} colour={participant.colour} size={6} />
          <span class="name">{participant.name}</span>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  .mobile-presence {
    padding: 0.5rem 1rem;
    border-bottom: 1px solid var(--color-pane-edge);
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .chip {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    min-width: 0;
    max-width: 100%;
  }

  .name {
    min-width: 0;
    max-width: 10rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.8125rem;
    color: var(--color-text);
  }
</style>

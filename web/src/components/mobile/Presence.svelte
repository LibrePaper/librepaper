<script>
  import Avatar from "../Avatar.svelte";
  import { m } from "../../lib/paraglide/messages.js";

  // Who else is in the document, written out. A phone has no hover to reveal
  // a name on an avatar, so the name sits beside each one. Each avatar is a
  // button that goes to that person (`onjump`). Renders nothing when nobody
  // else is here.
  let { participants = [], onjump = () => {} } = $props();
</script>

{#if participants.length}
  <section class="mobile-presence" aria-label={m.mobilenav_people_here()}>
    <ul class="chips">
      {#each participants as participant (participant.key)}
        <li class="chip">
          <button type="button" class="jump" data-tip={participant.name} aria-label={m.common_go_to_cursor({ name: participant.name })} onclick={() => onjump(participant)}>
            <Avatar name={participant.name} key={participant.key} colour={participant.colour} size={6} />
          </button>
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
    flex-wrap: nowrap;
    overflow-x: auto;
    scrollbar-width: none;
    gap: 0.5rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .chip {
    display: flex;
    align-items: center;
    flex: none;
    gap: 0.375rem;
  }

  .jump {
    display: inline-flex;
    flex: none;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: none;
    cursor: pointer;
  }

  .jump:focus-visible {
    outline: 2px solid var(--color-focus);
    outline-offset: 2px;
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

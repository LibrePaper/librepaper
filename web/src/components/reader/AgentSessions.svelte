<script>
  import { untrack } from "svelte";
  import Agent from "./Agent.svelte";
  import IconButton from "../IconButton.svelte";
  import {
    MAX_AGENT_SESSIONS, loadSessions, saveSessions, addSession, removeSession, selectSession,
  } from "../../lib/agent-sessions.js";
  import { m } from "../../lib/paraglide/messages.js";

  // Several chat sessions on one document. Each session is its own Agent, and
  // the inactive ones stay mounted: their agents keep working, their sockets
  // stay open and their access grants keep renewing.
  let { slug, request = null, ...rest } = $props();

  // localStorage can be absent or throw (private windows, blocked site data).
  // Without it the sessions still work, they just do not survive a reload.
  function storage() {
    try {
      return globalThis.localStorage ?? null;
    } catch {
      return null;
    }
  }

  let state = $state(loadSessions(storage(), location.origin, slug));

  function commit(next) {
    state = next;
    try {
      saveSessions(storage(), location.origin, slug, next);
    } catch {
      // Keeping the sessions in memory is enough for this visit.
    }
  }

  // What each session reports about itself: its agent, and whether it is
  // working or waiting for permission. The switcher shows it in the list.
  let statuses = $state({});
  // The Agent instances, by session key, for closing them on delete.
  const instances = {};

  // An external request (such as "ask the agent about this comment") goes only
  // to the session that was active when it arrived, not to whichever is
  // active later.
  let routed = $state({ key: "", value: null });
  $effect(() => {
    const value = request;
    routed = { key: untrack(() => state.active), value };
  });

  function label(s) {
    const status = statuses[s.key];
    const base = status?.agent
      ? m.agent_session_label_agent({ number: s.number, agent: status.agent })
      : m.agent_session_label({ number: s.number });
    if (status?.needsInput) return m.agent_session_needs_input({ label: base });
    if (status?.working) return m.agent_session_working({ label: base });
    return base;
  }

  let closing = $state(false);

  async function remove(key) {
    closing = true;
    try {
      await instances[key]?.close();
    } catch {
      // The session is deleted either way.
    } finally {
      closing = false;
    }
    delete instances[key];
    delete statuses[key];
    commit(removeSession(state, key));
  }
</script>

{#snippet switcher()}
  <span class="session-switcher">
    <select class="session-select" aria-label={m.agent_session_choose()} value={state.active}
            onchange={(event) => commit(selectSession(state, event.currentTarget.value))}>
      {#each state.sessions as s (s.key)}<option value={s.key}>{label(s)}</option>{/each}
    </select>
    <IconButton icon="plus" label={state.sessions.length >= MAX_AGENT_SESSIONS
                ? m.agent_session_limit({ count: MAX_AGENT_SESSIONS })
                : m.agent_new_conversation()}
                tone="plain" size="btn-icon-sm" disabled={state.sessions.length >= MAX_AGENT_SESSIONS}
                onclick={() => commit(addSession(state))} />
    <IconButton icon="trash" label={m.agent_session_delete()} tone="plain" size="btn-icon-sm"
                disabled={closing} onclick={() => void remove(state.active)} />
  </span>
{/snippet}

{#each state.sessions as s (s.key)}
  <div class="agent-session" class:inactive={s.key !== state.active}>
    <Agent {...rest} {slug} session={s.key} active={s.key === state.active} switcher={switcher}
           request={routed.key === s.key ? routed.value : null}
           onstate={(v) => statuses[s.key] = v} bind:this={instances[s.key]} />
  </div>
{/each}

<style>
  .agent-session { display: contents; }
  .agent-session.inactive { display: none; }
  .session-switcher { display:inline-flex; align-items:center; gap:calc(var(--spacing) * .5); min-width:0; }
  .session-select { border:0; background:transparent; color:inherit; font:inherit; padding:0; cursor:pointer;
    min-width:0; max-width:12rem; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
</style>

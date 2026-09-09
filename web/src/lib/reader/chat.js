// A pending acknowledgement is a browser resource: its timer must not remain
// alive after the reader has gone away, and a disconnect settles every send.
export function createPendingChat({
  timeoutMs = 5000,
  createId = () => globalThis.crypto.randomUUID(),
  setTimer = globalThis.setTimeout,
  clearTimer = globalThis.clearTimeout,
  send,
}) {
  const pending = new Map();
  let disposed = false;

  function settle(id, accepted) {
    const item = pending.get(id);
    if (!item) return false;
    clearTimer(item.timer);
    pending.delete(id);
    item.resolve(accepted);
    return true;
  }

  function sendMessage(text) {
    if (disposed) return Promise.resolve(false);
    const temp_id = createId();
    return new Promise((resolve) => {
      const timer = setTimer(() => settle(temp_id, false), timeoutMs);
      pending.set(temp_id, { resolve, timer });
      const result = send({ type: "chat", body: text, temp_id });
      if (result?.ok !== true) settle(temp_id, false);
    });
  }

  function disconnect() {
    for (const id of [...pending.keys()]) settle(id, false);
  }

  function dispose() {
    disposed = true;
    disconnect();
  }

  return {
    send: sendMessage,
    acknowledge: settle,
    has: (id) => pending.has(id),
    disconnect,
    dispose,
    get size() { return pending.size; },
  };
}

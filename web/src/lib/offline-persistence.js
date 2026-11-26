import { LoroDoc, VersionVector } from "loro-crdt";
import { projectIdentityKey } from "./project-identity.js";
import { openOfflineDatabase, requestResult, STATES, transactionDone, UPDATES } from "./offline-projects.js";

// Every stored delta row is one Loro import when the project reopens offline,
// and compaction re-exports the whole history on the main thread. Typing can
// write up to three commits per keystroke with tiny deltas, so the row count
// is the bound that fires first; 256 keeps a reopen to at most 256 small
// imports after the snapshot and compacts about every 85 to 256 keystrokes.
// Large deltas are capped by bytes instead.
const COMPACT_AFTER_UPDATES = 256;
const COMPACT_AFTER_BYTES = 4 * 1024 * 1024;
// The offline copy is written after a short pause rather than on every
// keystroke, so typing does not export the document and open an IndexedDB
// transaction each time. At most this much typing can be missing from the
// offline copy if the tab dies; the server has it anyway.
const WRITE_DELAY = 250;

export function durableProjectPersistence(identity, indexedDB_ = globalThis.indexedDB, legacyIdentity = null) {
  const key = projectIdentityKey(identity);
  const legacyKey = legacyIdentity ? projectIdentityKey(legacyIdentity) : null;
  return {
    open(doc, events) {
      let database;
      let closed = false;
      let dirty = false;
      let writing = false;
      let stored = false;
      let record = null;
      let persistedVector = null;
      // Bytes of delta rows not yet folded into the snapshot, kept in memory
      // so a steady-state write never has to read them back.
      let pendingBytes = 0;
      let compactScheduled = false;
      // Set when hydration found a stored copy that cannot be joined. The
      // next write replaces it with a snapshot of the live document.
      let replaceStored = false;
      // The stored pieces of a copy that could not be loaded on its own, kept
      // so the session can replay them once it has joined the server's base.
      let deferred = [];
      const facts = { found: false, snapshotBytes: 0, rows: 0, pendingPeers: 0, discarded: false, deferredPieces: 0 };
      // Set by close() so the final write can run after `closed` is true,
      // including the re-runs that writes and compaction chain onto themselves.
      let finishing = false;
      let writeTimer = null;
      let pendingFlush = Promise.resolve();
      let unsubscribe;
      let hydrating = true;
      const tab = globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random()}`;
      let batch = 0;

      const updateId = () => `${tab}:${String(batch += 1).padStart(12, "0")}`;

      function updatesFor(transaction, project = key) {
        return requestResult(transaction.objectStore(UPDATES).index("project").getAll(project));
      }

      async function putState() {
        if ((closed && !finishing) || writing || !dirty) return pendingFlush;
        // A discarded copy keeps its rows until the session has taken them for
        // replay: replacing it earlier would delete edits the join can recover.
        if (replaceStored && deferred.length) return pendingFlush;
        dirty = false;
        writing = true;
        events.writing(1);
        pendingFlush = (async () => {
          try {
            database ||= await openOfflineDatabase(indexedDB_);
            const version = doc.oplogVersion().encode();
            // A delta row is exported from persistedVector, so that vector must
            // cover the shallow root. If not, the row depends on trimmed history
            // and the stored copy is replaced instead.
            if (!replaceStored && doc.isShallow()
                && !(persistedVector && [0, 1].includes(VersionVector.decode(persistedVector).compare(doc.shallowSinceVV())))) {
              replaceStored = true;
            }
            const update = replaceStored ? null : doc.export({
              mode: "update",
              ...(persistedVector ? { from: VersionVector.decode(persistedVector) } : {}),
            });
            if (!replaceStored && record?.version === 3 && record.snapshot) {
              // Steady state: the snapshot record is untouched, so write only
              // the new delta row. The row count is read from the index
              // (no values), so rows other tabs added or compacted away are
              // still counted correctly.
              const transaction = database.transaction(UPDATES, "readwrite");
              const done = transactionDone(transaction);
              const updates = transaction.objectStore(UPDATES);
              const counted = requestResult(updates.index("project").count(key));
              updates.add({ project: key, id: updateId(), update, vector: version, bytes: update.byteLength });
              const count = (await counted) + 1;
              await done;
              pendingBytes += update.byteLength;
              persistedVector = version;
              stored = true;
              writing = false;
              if (count >= COMPACT_AFTER_UPDATES || pendingBytes >= COMPACT_AFTER_BYTES) scheduleCompaction();
              if (dirty) return putState();
              events.persisted();
              return;
            }
            const transaction = database.transaction([STATES, UPDATES], "readwrite");
            const done = transactionDone(transaction);
            const states = transaction.objectStore(STATES);
            const updates = transaction.objectStore(UPDATES);
            const [current, pending] = await Promise.all([
              requestResult(states.get(key)),
              updatesFor(transaction),
            ]);
            const replacing = replaceStored;
            const count = pending.length + 1;
            const bytes = pending.reduce((sum, item) => sum + Number(item.bytes || item.update?.byteLength || 0), 0)
              + (update?.byteLength || 0);
            let compacted = false;
            let next;
            if (replacing) {
              // The discarded copy and its delta rows are dropped; the live
              // document already holds everything they would have added.
              next = { version: 3, snapshot: doc.export({ mode: "snapshot" }), vector: version };
              for (const item of pending) updates.delete([key, item.id]);
              compacted = true;
            } else {
              const entry = { project: key, id: updateId(), update, vector: version, bytes: update.byteLength };
              updates.add(entry);
              next = current?.version === 3 ? current : {
                version: 3,
                snapshot: current?.snapshot || (current instanceof Uint8Array ? current : null),
              };
              if (!next.snapshot || count >= COMPACT_AFTER_UPDATES || bytes >= COMPACT_AFTER_BYTES) {
                // Seeded with a real snapshot, never an update export: an update
                // from a shallow document goes pending in a fresh doc. With no
                // stored snapshot the live doc's snapshot already holds `update`.
                const merging = new LoroDoc();
                if (next.snapshot) merging.import(new Uint8Array(next.snapshot));
                else merging.import(doc.export({ mode: "snapshot" }));
                for (const item of pending) merging.import(new Uint8Array(item.update));
                merging.import(update);
                const snapshot = merging.export({ mode: "snapshot" });
                next = { ...next, version: 3, snapshot, vector: merging.oplogVersion().encode() };
                for (const item of pending) updates.delete([key, item.id]);
                updates.delete([key, entry.id]);
                compacted = true;
              }
            }
            states.put(next, key);
            await done;
            replaceStored = false;
            record = next;
            pendingBytes = compacted ? 0 : bytes;
            persistedVector = version;
            stored = true;
            writing = false;
            if (dirty) return putState();
            events.persisted();
          } catch (error) {
            writing = false;
            events.failed(error);
            throw error;
          }
        })();
        pendingFlush.catch(() => {});
        return pendingFlush;
      }

      function scheduleCompaction() {
        if (compactScheduled || closed) return;
        compactScheduled = true;
        const run = () => { compactScheduled = false; void compact(); };
        if (typeof globalThis.requestIdleCallback === "function") globalThis.requestIdleCallback(run, { timeout: 5000 });
        else setTimeout(run, 200);
      }

      // Folds the stored deltas into the snapshot off the typing path.
      async function compact() {
        if (closed) return;
        if (writing) { scheduleCompaction(); return; }
        writing = true;
        pendingFlush = (async () => {
          try {
            database ||= await openOfflineDatabase(indexedDB_);
            const transaction = database.transaction([STATES, UPDATES], "readwrite");
            const done = transactionDone(transaction);
            const states = transaction.objectStore(STATES);
            const updates = transaction.objectStore(UPDATES);
            const [current, pending] = await Promise.all([requestResult(states.get(key)), updatesFor(transaction)]);
            if (current?.version === 3 && current.snapshot) {
              const merging = new LoroDoc();
              merging.import(new Uint8Array(current.snapshot));
              for (const item of pending) merging.import(new Uint8Array(item.update));
              const next = { ...current, version: 3, snapshot: merging.export({ mode: "snapshot" }), vector: merging.oplogVersion().encode() };
              for (const item of pending) updates.delete([key, item.id]);
              states.put(next, key);
              await done;
              record = next;
              pendingBytes = 0;
            } else {
              await done;
            }
          } catch (error) {
            events.failed(error);
          }
          writing = false;
          // Chained through the returned promise, not `void` in a finally, so
          // close() waits for a change that arrived during compaction too.
          if (dirty) return putState();
        })();
        pendingFlush.catch(() => {});
      }

      // The offline copy is a recovery point, not merely an outbox. Persist
      // imports from the room as well as local edits so an explicitly flushed
      // document is exactly the document that will open offline. The saved
      // state itself is imported before this subscription is installed, which
      // is the sole update that must not dirty the record again.
      function changed() {
        if (closed || hydrating) return;
        dirty = true;
        if (writeTimer === null) {
          writeTimer = setTimeout(() => { writeTimer = null; void putState(); }, WRITE_DELAY);
        }
      }

      // Browsers can discard a hidden tab without running timers, so a pending
      // write starts as the page hides instead of waiting for the pause.
      function pageHidden() {
        if (!dirty) return;
        clearTimeout(writeTimer);
        writeTimer = null;
        void putState();
      }

      const hydration = (async () => {
        try {
          database = await openOfflineDatabase(indexedDB_);
          let saved = await requestResult(database.transaction(STATES, "readonly").objectStore(STATES).get(key));
          if (!saved && legacyKey && legacyKey !== key) {
            saved = await requestResult(database.transaction(STATES, "readonly").objectStore(STATES).get(legacyKey));
            if (saved) {
              const migration = database.transaction(STATES, "readwrite");
              migration.objectStore(STATES).put(saved, key);
              migration.objectStore(STATES).delete(legacyKey);
              await transactionDone(migration);
            }
          }
          if (closed) return;
          if (saved) {
            facts.found = true;
            // Everything is imported into a scratch doc first. A stored copy
            // that still leaves changes pending once every piece is in (an
            // update export taken from a shallow document, say) would otherwise
            // block the server's base in the live doc, so it is discarded and
            // replaced on the next write. Pieces are judged together because
            // delta rows arrive in key order, not causal order.
            const legacy = saved.version !== 2 && saved.version !== 3;
            const scratch = new LoroDoc();
            // v1 records were a single full-history update. Read them once;
            // the next transaction rewrites them in the incremental form.
            const pieces = [];
            if (legacy) pieces.push(new Uint8Array(saved));
            else {
              if (saved.snapshot) pieces.push(new Uint8Array(saved.snapshot));
              for (const update of saved.updates || []) pieces.push(new Uint8Array(update));
            }
            facts.snapshotBytes = (legacy ? saved.byteLength : saved.snapshot?.byteLength) || 0;
            const transaction = database.transaction(UPDATES, "readonly");
            const pending = await updatesFor(transaction);
            facts.rows = pending.length;
            for (const item of pending) pieces.push(new Uint8Array(item.update));
            // The first piece goes in alone: Loro loads a shallow snapshot's
            // state only on its own into an empty doc, and drops it inside a
            // batch. The rows after it are batched, so their order does not matter.
            const first = pieces.length ? scratch.import(pieces[0]) : null;
            const rest = pieces.length > 1 ? scratch.importBatch(pieces.slice(1)) : null;
            const unresolved = Math.max(first?.pending?.size || 0, rest?.pending?.size || 0);
            facts.pendingPeers = unresolved;
            if (unresolved) {
              facts.discarded = true;
              replaceStored = true;
              deferred = pieces;
              facts.deferredPieces = pieces.length;
            } else {
              stored = true;
              doc.import(scratch.export({ mode: "snapshot" }));
              if (legacy) {
                persistedVector = doc.oplogVersion().encode();
                record = { version: 2, snapshot: new Uint8Array(saved), updates: [], bytes: saved.byteLength, vector: persistedVector };
              } else {
                record = saved;
                persistedVector = saved.vector ? new Uint8Array(saved.vector) : doc.oplogVersion().encode();
              }
              pendingBytes = pending.reduce((sum, item) => sum + Number(item.bytes || item.update?.byteLength || 0), 0);
              if (pending.length) persistedVector = doc.oplogVersion().encode();
            }
          }
          hydrating = false;
          unsubscribe = doc.subscribe(changed);
          if (closed) {
            unsubscribe();
            unsubscribe = undefined;
            return;
          }
          if (typeof globalThis.addEventListener === "function") globalThis.addEventListener("pagehide", pageHidden);
          events.hydrated();
        } catch (error) {
          hydrating = false;
          if (closed) return;
          events.failed(error);
        }
      })();

      return {
        hydration,
        hydrationFacts() {
          return { ...facts };
        },
        /// The bytes of a stored copy that could not be loaded on its own. The
        /// session replays them once it has joined; taking them hands them over once.
        takeDeferred() {
          const bytes = deferred;
          deferred = [];
          return bytes;
        },
        async flush() {
          await hydration;
          // An explicit flush writes now, so the pending pause is not needed.
          clearTimeout(writeTimer);
          writeTimer = null;
          const current = doc.oplogVersion().encode();
          if (!stored || !persistedVector || current.byteLength !== persistedVector.byteLength
              || current.some((byte, index) => byte !== persistedVector[index])) dirty = true;
          if (dirty && !closed) await putState();
          await pendingFlush;
        },
        // Deletes this project's offline copy: the snapshot record and every
        // delta row for it, in one transaction. Nothing may write after this,
        // so `closed` is set first; an in-flight write is waited out before
        // the delete so it cannot recreate the record.
        async discard() {
          await hydration;
          closed = true;
          clearTimeout(writeTimer);
          writeTimer = null;
          await pendingFlush.catch(() => {});
          database ||= await openOfflineDatabase(indexedDB_);
          const transaction = database.transaction([STATES, UPDATES], "readwrite");
          const done = transactionDone(transaction);
          const states = transaction.objectStore(STATES);
          const updates = transaction.objectStore(UPDATES);
          const pending = await updatesFor(transaction);
          for (const item of pending) updates.delete([key, item.id]);
          states.delete(key);
          await done;
        },
        // There is no `confirm(coverage)` any more, and there is no stored
        // coverage to confirm. The server used to send the Loro vector it
        // had durably covered on every acknowledgement, and this kept it so
        // a restart could tell which of its own updates were already safe.
        // The log removes it: nothing is persisted per
        // batch and no replay cursor exists, so there is no ordering between
        // cursor persistence and update persistence to get wrong. What is
        // here is the whole document, and reconnecting exports whatever the
        // server's vector says it is missing.
        close() {
          // The final write has to start before `closed` stops new writes, and
          // it must not be skipped while the hydration is still running.
          finishing = true;
          clearTimeout(writeTimer);
          writeTimer = null;
          if (dirty && !hydrating) void putState();
          closed = true;
          unsubscribe?.();
          if (typeof globalThis.removeEventListener === "function") globalThis.removeEventListener("pagehide", pageHidden);
          void pendingFlush.catch(() => {}).finally(() => database?.close());
        },
      };
    },
  };
}

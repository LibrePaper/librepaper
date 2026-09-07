#!/usr/bin/env node
// Planning scenarios, not measured traffic or a complete service bill.
// Unbatched snapshot baseline; the shared journal needs a separate workload model.
// Run: node docs/research/storage-cost-model.mjs [--json]
// Override top-level workload inputs: --input /path/to/inputs.json
// Prices verified 2026-09-07; sources and limits are in storage-costs.md.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export const INPUTS = {
  documents: 1000,
  daysPerMonth: 30,
  activeHoursPerDocumentPerDay: 24,
  snapshotSeconds: [15, 60, 120],
  checkpointRatesPerActiveHour: [0, 4, 12, 30],
  // Target SQL layout: no bucket manifest/index update. These are assumptions.
  changedBlobsPerCheckpoint: 2,
  treePutsPerCheckpoint: 1,
  extraPutsPerCheckpoint: 0,
  // Include extra checkpoint session writes here if they cannot share a save.
  extraClassAMonthly: 0,
  readsPerSnapshot: 0,
  readsPerCheckpoint: 0,
  extraClassBMonthly: 0,
  // Separate stores and units. Supply retention/backup/version overhead too.
  r2StorageGbMonth: 5,
  tursoDatabaseGb: 1,
  replicas: 1,
  syncPagesPerCheckpoint: 4,
  syncBytesPerPage: 4096,
  extraSyncGbMonthly: 0,
  logicalRowsPerCheckpoint: 2,
  // Actual provider metrics, not inferred from the logical row count.
  providerRowsReadMonthly: null,
  providerRowsWrittenMonthly: null,
};

export const PRICES = {
  r2: { freeGbMonth: 10, freeClassA: 1e6, freeClassB: 10e6,
    usdPerGbMonth: 0.015, usdPerMillionA: 4.5, usdPerMillionB: 0.36 },
  tursoFree: { gb: 5, reads: 500e6, writes: 10e6, syncGb: 3 },
  tursoDeveloper: { baseUsd: 5.99, gb: 9, reads: 2.5e9, writes: 25e6,
    syncGb: 10, usdPerGb: 0.75, usdPerBillionReads: 1,
    usdPerMillionWrites: 1, usdPerSyncGb: 0.35 },
};

const usd = n => Number(n.toFixed(6));
const over = (usage, allowance) => Math.max(0, usage - allowance);

export function calculate(inputs, snapshotSeconds, checkpointsPerActiveHour) {
  for (const [key, value] of Object.entries(inputs)) {
    if (Array.isArray(value)) continue;
    if (value === null && key.startsWith('providerRows')) continue;
    if (typeof value !== 'number' || !Number.isFinite(value) || value < 0)
      throw new Error(`${key} must be a finite nonnegative number`);
  }
  if (inputs.activeHoursPerDocumentPerDay > 24)
    throw new Error('An active day cannot exceed 24 hours');
  if (!Number.isFinite(snapshotSeconds) || snapshotSeconds <= 0)
    throw new Error('snapshotSeconds must be positive');
  if (!Number.isFinite(checkpointsPerActiveHour) || checkpointsPerActiveHour < 0)
    throw new Error('checkpointsPerActiveHour must be nonnegative');

  const hours = inputs.documents * inputs.daysPerMonth * inputs.activeHoursPerDocumentPerDay;
  const snapshots = hours * 3600 / snapshotSeconds;
  const checkpoints = hours * checkpointsPerActiveHour;
  const checkpointPuts = checkpoints * (inputs.changedBlobsPerCheckpoint
    + inputs.treePutsPerCheckpoint + inputs.extraPutsPerCheckpoint);
  const classA = snapshots + checkpointPuts + inputs.extraClassAMonthly;
  const classB = snapshots * inputs.readsPerSnapshot
    + checkpoints * inputs.readsPerCheckpoint + inputs.extraClassBMonthly;
  const r = PRICES.r2;
  // Apply each free allowance ONCE to the combined workload, then round up.
  const classAUsd = Math.ceil(over(classA, r.freeClassA) / 1e6) * r.usdPerMillionA;
  const classBUsd = Math.ceil(over(classB, r.freeClassB) / 1e6) * r.usdPerMillionB;
  const storageUsd = Math.ceil(over(inputs.r2StorageGbMonth, r.freeGbMonth)) * r.usdPerGbMonth;

  const syncGb = checkpoints * inputs.syncPagesPerCheckpoint
    * inputs.syncBytesPerPage * inputs.replicas / 1e9 + inputs.extraSyncGbMonthly;
  const d = PRICES.tursoDeveloper;
  const knownSubtotal = d.baseUsd + over(inputs.tursoDatabaseGb, d.gb) * d.usdPerGb
    + over(syncGb, d.syncGb) * d.usdPerSyncGb;
  const rowsKnown = inputs.providerRowsReadMonthly !== null && inputs.providerRowsWrittenMonthly !== null;
  const rowCharges = rowsKnown
    ? over(inputs.providerRowsReadMonthly, d.reads) / 1e9 * d.usdPerBillionReads
      + over(inputs.providerRowsWrittenMonthly, d.writes) / 1e6 * d.usdPerMillionWrites
    : null;
  const f = PRICES.tursoFree;
  const exceedsFree = inputs.tursoDatabaseGb > f.gb || syncGb > f.syncGb
    || (inputs.providerRowsReadMonthly !== null && inputs.providerRowsReadMonthly > f.reads)
    || (inputs.providerRowsWrittenMonthly !== null && inputs.providerRowsWrittenMonthly > f.writes);

  return {
    snapshotSeconds, checkpointsPerActiveHour, activeDocumentHours: hours,
    snapshots, checkpoints, checkpointPuts, classA, classB,
    r2: { classAUsd: usd(classAUsd), classBUsd: usd(classBUsd), storageUsd: usd(storageUsd),
      modeledSubtotalUsd: usd(classAUsd + classBUsd + storageUsd) },
    turso: { logicalCheckpointRows: checkpoints * inputs.logicalRowsPerCheckpoint,
      syncGb, freeFits: exceedsFree ? false : rowsKnown ? true : null,
      developerBaseStorageSyncUsd: usd(knownSubtotal),
      developerModeledSubtotalUsd: rowsKnown ? usd(knownSubtotal + rowCharges) : null },
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const inputAt = process.argv.indexOf('--input');
  const overrides = inputAt < 0 ? {} : JSON.parse(readFileSync(process.argv[inputAt + 1], 'utf8'));
  for (const key of Object.keys(overrides)) {
    if (!Object.hasOwn(INPUTS, key)) throw new Error(`Unknown input: ${key}`);
  }
  const inputs = { ...INPUTS, ...overrides };
  const rows = inputs.snapshotSeconds.flatMap(s => inputs.checkpointRatesPerActiveHour.map(c => calculate(inputs, s, c)));
  if (process.argv.includes('--json')) console.log(JSON.stringify({ inputs, prices: PRICES, rows }, null, 2));
  else {
    console.log('Planning scenarios for continuously dirty rooms; all prices USD. See storage-costs.md.');
    console.table(rows.map(x => ({ saveSeconds: x.snapshotSeconds, checkpointsPerHour: x.checkpointsPerActiveHour,
      sessionPUTs: x.snapshots, checkpointPUTs: x.checkpointPuts,
      R2modeledUSD: x.r2.modeledSubtotalUsd, assumedSyncGB: Number(x.turso.syncGb.toFixed(3)),
      TursoBaseStorageSyncUSD: x.turso.developerBaseStorageSyncUsd })));
    console.log('Not a total service bill. Turso row charges remain unknown until provider metrics are supplied.');
  }
}

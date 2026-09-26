#!/usr/bin/env node
// Read-only inventory. Pass disjoint object/backup roots to avoid double counting.
import { lstat, readdir } from "node:fs/promises";
import { resolve, join } from "node:path";

if (process.argv.length < 3) throw new Error("usage: node filesystem.mjs PATH [PATH ...]");
for (const argument of process.argv.slice(2)) {
  const root = resolve(argument);
  const totals = { root, files: 0, logical_bytes: 0, allocated_bytes: 0, skipped_symlinks: 0 };
  async function visit(path) {
    const stat = await lstat(path);
    if (stat.isSymbolicLink()) {
      totals.skipped_symlinks++;
    } else if (stat.isDirectory()) {
      for (const name of await readdir(path)) await visit(join(path, name));
    } else if (stat.isFile()) {
      totals.files++;
      totals.logical_bytes += stat.size;
      totals.allocated_bytes += stat.blocks * 512;
    }
  }
  await visit(root);
  console.log(JSON.stringify(totals));
}

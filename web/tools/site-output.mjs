import { mkdir, rm } from "node:fs/promises";

export async function prepareSiteOutput(outDir) {
  await rm(outDir, { recursive: true, force: true });
  await mkdir(outDir, { recursive: true });
}

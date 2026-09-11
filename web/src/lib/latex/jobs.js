// Job identity.
//
// A `Job` is the immutable identity a compile receives before anything is
// staged and that every result -- browser, local Biber, native --
// echoes back unchanged (SPEC "Project configuration and identity"). It says
// exactly what was asked for (project, source, engine, release) and exactly
// when (a monotonically increasing generation), so a late result from a
// superseded snapshot can be recognised as such by comparing identities
// rather than by trusting arrival order.
//
// `snapshot` is the identity the rest of this package keys everything on:
// staged "generated" reuse and the session route's native attempt budget,
// and bibliography cache invalidation on a settings change all compare
// snapshots, never trees. It intentionally excludes the project id and the
// generation -- two jobs for the same source, engine and release must share
// a snapshot even across a page reload, which is what lets a still-warm
// bibliography cache or a still-valid generated aux state survive one.

import { sha256HexOfText } from "../digest.js";

/// Builds a Job from what the controller already knows: `inputs` is the
/// source/asset digest (`tree-digest.js`'s `snapshotDigest`, computed by the
/// caller so this module stays synchronous over its own concern), `engine`
/// and `release` are already resolved to concrete values -- never `"auto"`
/// or `null` -- because the snapshot identity must be the same for two
/// compiles that resolve to the same engine, and must differ the moment
/// either does not.
export async function makeJob({ project, generation, tree, inputs, engine, release }) {
  const snapshot = await sha256HexOfText(`${inputs}\n${engine}\n${release}`);
  return {
    id: `${project}:${generation}`,
    project,
    generation,
    snapshot,
    inputs,
    main: tree?.main ?? "",
    engine,
    release,
  };
}

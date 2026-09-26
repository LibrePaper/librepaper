// A cached CRDT may only rejoin the exact remote document that created it.
// Keep this value independent of URLs used for navigation: slugs can be
// reused, and the same slug on two servers names two different projects.
export const PROJECT_IDENTITY_VERSION = 2;

export function projectIdentity({ server, documentId, slug, createdAt, access = "" }) {
  const origin = new URL(server || globalThis.location?.origin || "http://localhost").origin;
  if (documentId) return { version: PROJECT_IDENTITY_VERSION, origin, documentId, access };
  if (!slug || !createdAt) throw new Error("project identity requires immutable document identity");
  // Legacy identities are recovery-only. They are migrated after an online
  // response proves which immutable UUID the slug currently names.
  return { version: 1, origin, slug, createdAt };
}

export function projectIdentityKey(identity) {
  return JSON.stringify([
    identity?.version,
    identity?.origin,
    identity?.documentId || identity?.slug,
    identity?.documentId ? identity?.access || "" : identity?.createdAt,
  ]);
}

export function sameProject(left, right) {
  return projectIdentityKey(left) === projectIdentityKey(right);
}

export function assertSameProject(local, remote) {
  if (!sameProject(local, remote)) {
    const error = new Error("The remote document is not the project stored on this device.");
    error.code = "project-identity-changed";
    throw error;
  }
}

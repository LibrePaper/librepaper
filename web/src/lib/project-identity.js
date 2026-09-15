// A cached CRDT may only rejoin the exact remote document that created it.
// Keep this value independent of URLs used for navigation: slugs can be
// reused, and the same slug on two servers names two different projects.
export const PROJECT_IDENTITY_VERSION = 1;

export function projectIdentity({ server, slug, createdAt }) {
  const origin = new URL(server || globalThis.location?.origin || "http://localhost").origin;
  if (!slug || !createdAt) throw new Error("project identity requires slug and creation identity");
  return { version: PROJECT_IDENTITY_VERSION, origin, slug, createdAt };
}

export function projectIdentityKey(identity) {
  return JSON.stringify([
    identity?.version,
    identity?.origin,
    identity?.slug,
    identity?.createdAt,
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

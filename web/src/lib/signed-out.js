// Whether this tab has signed out, or heard another tab do so. It lives apart
// from local-data.js because storage.js must consult it
// before writing a secret, and local-data.js itself imports storage.js.
let forgotten = false;
export const localDataForgotten = () => forgotten;
export function markSignedOut() { forgotten = true; }

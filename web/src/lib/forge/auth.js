import { SHELL_HEADERS } from "../api.js";
import { forgetToken, readToken, writeToken } from "./store.js";

// The GitHub sign-in. The server relays the code exchange and the refresh and
// keeps nothing; the browser keeps the token record in its local database.
const PROVIDER = "github";
const AUTHORIZE = "/api/forge/github/authorize";
const REFRESH = "/api/forge/github/refresh";
const MESSAGE = "librepaper-forge-token";
const REFRESH_MARGIN_MS = 60_000;

function record(answer, now = Date.now()) {
  return {
    access_token: answer.access_token,
    refresh_token: answer.refresh_token ?? null,
    expires_at: now + Number(answer.expires_in) * 1000,
    refresh_expires_at: answer.refresh_token_expires_in == null
      ? null
      : now + Number(answer.refresh_token_expires_in) * 1000,
  };
}

/// Opens GitHub's consent page in a popup and resolves with the saved token
/// record once the callback page posts it back. Rejects with the message the
/// callback reported, or when the popup closes first.
export function connect() {
  return new Promise((resolve, reject) => {
    const popup = window.open(AUTHORIZE, "librepaper-github", "popup,width=720,height=760");
    if (!popup) {
      reject(new Error("The browser blocked the GitHub window. Allow pop-ups for this site and try again."));
      return;
    }
    let timer = 0;
    const finish = () => {
      window.removeEventListener("message", onMessage);
      clearInterval(timer);
    };
    const onMessage = (event) => {
      if (event.origin !== location.origin || event.source !== popup) return;
      const data = event.data;
      if (data?.type !== MESSAGE) return;
      finish();
      if (data.error || !data.access_token) {
        reject(new Error(data.error || "GitHub did not return a token."));
        return;
      }
      const token = record(data);
      writeToken(PROVIDER, token).then(() => resolve(token), reject);
    };
    window.addEventListener("message", onMessage);
    timer = setInterval(() => {
      if (!popup.closed) return;
      finish();
      reject(new Error("The GitHub window was closed before sign-in finished."));
    }, 500);
  });
}

let refreshing = null;

async function refresh(token) {
  const response = await fetch(REFRESH, {
    method: "POST",
    headers: { "content-type": "application/json", ...SHELL_HEADERS },
    body: JSON.stringify({ refresh_token: token.refresh_token }),
  });
  if (response.status === 401 || response.status === 403) {
    await forgetToken(PROVIDER);
    return null;
  }
  if (!response.ok) throw new Error(`could not refresh the GitHub sign-in (${response.status})`);
  const next = record(await response.json());
  await writeToken(PROVIDER, next);
  return next;
}

/// A valid access token, refreshed when it is within a minute of expiring, or
/// null when there is none or GitHub refused the refresh.
export async function accessToken() {
  const token = await readToken(PROVIDER);
  if (!token?.access_token) return null;
  if (token.expires_at - Date.now() > REFRESH_MARGIN_MS) return token.access_token;
  if (!token.refresh_token || (token.refresh_expires_at && token.refresh_expires_at <= Date.now())) {
    await forgetToken(PROVIDER);
    return null;
  }
  refreshing ??= refresh(token).finally(() => {
    refreshing = null;
  });
  const next = await refreshing;
  return next?.access_token ?? null;
}

/// Forgets the saved token, for a disconnect or a refused sign-in.
export const signOutOfGitHub = () => forgetToken(PROVIDER);

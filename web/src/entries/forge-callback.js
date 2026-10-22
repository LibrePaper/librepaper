import { start as startTheme } from "../lib/theme.js";

startTheme();

// The server wrote GitHub's answer into the data block above. It goes to the
// window that opened this one, addressed to this origin only, and the window
// closes. A page opened without an opener has nobody to tell.
const status = document.getElementById("forge-status");
let message = null;
try {
  message = JSON.parse(document.getElementById("forge-token").textContent);
} catch {
  message = null;
}

if (message && window.opener) {
  window.opener.postMessage(message, location.origin);
  window.close();
} else {
  status.textContent = "You can close this window.";
}

/// <reference types="vite/client" />

declare const __KATEX__: string;
declare const __APP_VERSION__: string;
declare const __APP_COMMIT__: string;
declare const __APP_ORIGIN__: string;

declare module "*.css";

// Published by the desktop app before the page's own scripts run.
interface Window {
  __LIBREPAPER_DESKTOP__?: { readonly base: string; readonly version: string; readonly zoom?: number };
}

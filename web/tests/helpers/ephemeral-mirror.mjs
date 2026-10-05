// Serve a local wasm-latex mirror over an ephemeral HTTPS origin, under the
// latex/ prefix of an asset mirror.  The production server accepts HTTPS
// mirrors only, and browser checks should exercise the same cross-origin
// fetch/CORS path as a deployment.  `url` is the asset mirror to hand the
// server (`assets.mirror` in its TOML config); `latexUrl` is where the LaTeX
// files live beneath it.
import { execFileSync } from "node:child_process";
import { createServer } from "node:https";
import { mkdtempSync, readFileSync, readdirSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, sep } from "node:path";

export async function ephemeralMirror(directory, releaseId) {
  const root = resolve(directory);
  if (!statSync(root).isDirectory()) throw new Error(`mirror is not a directory: ${directory}`);
  // The mirror may hold several releases; serve the one assets.lock pins.
  releaseId ??= readFileSync(new URL("../../../assets.lock", import.meta.url), "utf8")
    .split("\n").find((line) => line.startsWith("latex "))?.split(/\s+/)[3];
  if (!/^[a-f0-9]{64}$/.test(releaseId)) throw new Error(`invalid releaseId: ${releaseId}`);
  const tls = mkdtempSync(join(tmpdir(), "librepaper-mirror-tls-"));
  const key = join(tls, "key.pem");
  const cert = join(tls, "cert.pem");
  execFileSync("openssl", [
    "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", key,
    "-out", cert, "-subj", "/CN=localhost", "-days", "1",
  ], { stdio: "ignore" });
  const server = createServer({ key: readFileSync(key), cert: readFileSync(cert) }, (request, response) => {
    if (request.method !== "GET" && request.method !== "HEAD") {
      response.writeHead(405, { allow: "GET, HEAD", "access-control-allow-origin": "*" });
      response.end();
      return;
    }
    try {
      const requested = decodeURIComponent(new URL(request.url || "/", "https://localhost/").pathname).replace(/^\/+/, "");
      if (!requested.startsWith("latex/")) throw new Error("outside the latex prefix");
      const relative = requested.slice("latex/".length);
      const file = resolve(root, relative);
      if (file !== root && !file.startsWith(root + sep)) throw new Error("mirror path escaped root");
      if (!statSync(file).isFile()) throw new Error("mirror file not found");
      const body = readFileSync(file);
      const type = file.endsWith(".json") ? "application/json"
        : file.endsWith(".js") ? "text/javascript"
        : file.endsWith(".wasm") ? "application/wasm"
        : file.endsWith(".css") ? "text/css"
        : file.endsWith(".gz") ? "application/gzip"
        : "application/octet-stream";
      response.writeHead(200, {
        "access-control-allow-origin": "*",
        "cache-control": "public, max-age=31536000, immutable",
        "content-type": type,
        "content-length": body.length,
      });
      if (request.method === "HEAD") response.end();
      else response.end(body);
    } catch {
      response.writeHead(404, { "access-control-allow-origin": "*", "cache-control": "no-store" });
      response.end();
    }
  });
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  return {
    url: `https://localhost:${server.address().port}/`,
    latexUrl: `https://localhost:${server.address().port}/latex/${releaseId}/`,
    releaseId,
    server,
    tls,
  };
}

// Serve a local wasm-latex mirror over an ephemeral HTTPS origin.  The
// production server accepts HTTPS mirrors only, and browser checks should
// exercise the same cross-origin fetch/CORS path as a deployment.
import { execFileSync } from "node:child_process";
import { createServer } from "node:https";
import { mkdtempSync, readFileSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, sep } from "node:path";

export async function ephemeralMirror(directory) {
  const root = resolve(directory);
  if (!statSync(root).isDirectory()) throw new Error(`mirror is not a directory: ${directory}`);
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
      const relative = decodeURIComponent(new URL(request.url || "/", "https://localhost/").pathname).replace(/^\/+/, "");
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
        "cache-control": relative === "manifest.json" ? "no-store" : "public, max-age=31536000, immutable",
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
    server,
    tls,
  };
}

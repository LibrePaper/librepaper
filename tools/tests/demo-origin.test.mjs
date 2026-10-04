import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const makefile = join(root, "Makefile");
const make = execFileSync("/bin/sh", ["-c", "command -v make"], { encoding: "utf8" }).trim();
const inheritedProductionServer = "https://komodoc.example/";

async function makeFixture({ running = false, port, commandLineServer } = {}) {
  const directory = await mkdtemp(join(tmpdir(), "librepaper-demo-origin-"));
  const mockBinDir = join(directory, "mock-bin");
  const binPath = join(directory, "dist", "librepaper");
  const recursiveMakeLog = join(directory, "recursive-make.log");
  const companionLog = join(directory, "companion.log");
  const bunLog = join(directory, "bun.log");
  const toolsLog = join(directory, "tools.log");

  await Promise.all([
    mkdir(join(directory, "tools"), { recursive: true }),
    mkdir(join(directory, "web"), { recursive: true }),
    mkdir(join(directory, "dist"), { recursive: true }),
    mkdir(mockBinDir, { recursive: true }),
  ]);

  await writeFile(join(directory, "mock-make"),
    '#!/bin/sh\ncase " $* " in *" site "*) target=site;; *" serve "*) target=serve;; *) target=unknown;; esac\nprintf "%s\\targs=%s\\tserver=%s\\tapp=%s\\n" "$target" "$*" "${LIBREPAPER_SERVER:-}" "${LIBREPAPER_APP_ORIGIN:-}" >> "$MOCK_RECURSIVE_MAKE_LOG"\nif [ "$target" = serve ] && [ "${MOCK_COMPANION_RUNNING:-0}" != 1 ]; then attempts=0; while ! grep -q \'^start\' "$MOCK_COMPANION_LOG" 2>/dev/null; do attempts=$((attempts + 1)); [ "$attempts" -lt 50 ] || break; sleep 0.1; done; fi\n',
    { mode: 0o755 });

  await writeFile(join(mockBinDir, "bun"),
    '#!/bin/sh\nprintf "%s\\tapp=%s\\n" "$*" "${LIBREPAPER_APP_ORIGIN:-}" >> "$MOCK_BUN_LOG"\ncase "$*" in *serve:site*) sleep 60 & child=$!; trap \'kill "$child" 2>/dev/null || true; wait "$child" 2>/dev/null || true\' EXIT INT TERM; wait "$child";; esac\n',
    { mode: 0o755 });
  await writeFile(join(mockBinDir, "firefox"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });

  await writeFile(join(directory, "tools", "deploy-assets"),
    '#!/bin/sh\nprintf "%s\\n" "$*" >> "$MOCK_TOOLS_LOG"\n',
    { mode: 0o755 });
  await writeFile(join(directory, "dist", "librepaper"),
    '#!/bin/sh\ncase "$1" in\n  status)\n    printf "Companion status\\nrunning at http://localhost:8763\\n"\n    printf "status\\t%s\\n" "${LIBREPAPER_SERVER:-}" >> "$MOCK_COMPANION_LOG"\n    [ "${MOCK_COMPANION_RUNNING:-0}" = 1 ]\n    ;;\n  start)\n    printf "start\\t%s\\t%s\\n" "$*" "${LIBREPAPER_SERVER:-}" >> "$MOCK_COMPANION_LOG"\n    sleep 60 & child=$!\n    trap \'kill "$child" 2>/dev/null || true; wait "$child" 2>/dev/null || true\' EXIT INT TERM\n    wait "$child"\n    ;;\n  *) printf "unexpected\\t%s\\n" "$*" >> "$MOCK_COMPANION_LOG"; exit 2;;\nesac\n',
    { mode: 0o755 });

  return {
    directory,
    binPath,
    recursiveMakeLog,
    companionLog,
    bunLog,
    toolsLog,
    port,
    commandLineServer,
    running,
  };
}

function runDemoRun(fixture) {
  const args = [
    `--old-file=${fixture.binPath}`,
    "--no-print-directory",
    "-f",
    makefile,
    "SHELL=/bin/sh",
    `BIN=${fixture.binPath}`,
    `PORT=${fixture.port}`,
    `SITE_PORT=${fixture.port + 1}`,
    "DATA=demo-data",
    "SIMULATE_ACTIVITY=0",
    `MAKE=${join(fixture.directory, "mock-make")}`,
    "LIBREPAPER_DATABASE_URL=postgres://unused",
  ];
  if (fixture.commandLineServer) args.push(`LIBREPAPER_SERVER=${fixture.commandLineServer}`);
  args.push("demo-run");

  return execFileSync(make, args, {
    cwd: fixture.directory,
    encoding: "utf8",
    timeout: 20000,
    env: {
      ...process.env,
      PATH: `${join(fixture.directory, "mock-bin")}:${process.env.PATH}`,
      MAKEFLAGS: "",
      MFLAGS: "",
      MAKELEVEL: "",
      LIBREPAPER_SERVER: inheritedProductionServer,
      LIBREPAPER_APP_ORIGIN: "",
      MOCK_RECURSIVE_MAKE_LOG: fixture.recursiveMakeLog,
      MOCK_COMPANION_LOG: fixture.companionLog,
      MOCK_BUN_LOG: fixture.bunLog,
      MOCK_TOOLS_LOG: fixture.toolsLog,
      MOCK_COMPANION_RUNNING: fixture.running ? "1" : "0",
    },
  });
}

async function logs(fixture) {
  const [makeCalls, companionCalls] = await Promise.all([
    readFile(fixture.recursiveMakeLog, "utf8"),
    readFile(fixture.companionLog, "utf8"),
  ]);
  return {
    makeCalls: makeCalls.trim().split("\n").map((line) => {
      const [target, args, server, app] = line.split("\t");
      return {
        target,
        args: args.slice("args=".length),
        server: server.slice("server=".length),
        app: app.slice("app=".length),
      };
    }),
    companionCalls: companionCalls.trim().split("\n").map((line) => line.split("\t")),
  };
}

for (const scenario of [
  { name: "inherited production server", port: 9137 },
  { name: "command-line server", port: 9237, commandLineServer: "https://caller.example/" },
]) {
  test(`demo-run gives the local companion origin precedence over the ${scenario.name}`, async () => {
    const fixture = await makeFixture(scenario);
    try {
      runDemoRun(fixture);
      const { makeCalls, companionCalls } = await logs(fixture);
      const localServer = `http://localhost:${scenario.port}`;
      const starts = companionCalls.filter(([kind]) => kind === "start");
      assert.equal(starts.length, 1, "demo-run starts the companion when status reports it stopped");
      assert.equal(starts[0][2], localServer, "start receives LIBREPAPER_SERVER for the app on PORT");

      const serve = makeCalls.find((call) => call.target === "serve");
      assert.ok(serve, "demo-run invokes the recursive serve target");
      assert.equal(serve.server, localServer, "recursive serve receives the local app origin");
      assert.ok(serve.args.split(/\s+/).includes(`LIBREPAPER_SERVER=${localServer}`),
        "the local origin is passed explicitly to recursive make");
      assert.equal(serve.app, "", "serve does not inherit the site's app-build origin");
    } finally {
      await rm(fixture.directory, { recursive: true, force: true });
    }
  });
}

test("demo-run leaves an already running companion untouched but scopes recursive serve locally", async () => {
  const fixture = await makeFixture({ running: true, port: 9337 });
  try {
    runDemoRun(fixture);
    const { makeCalls, companionCalls } = await logs(fixture);
    assert.ok(companionCalls.length > 0);
    assert.ok(companionCalls.every(([kind]) => kind === "status"), "an already running companion is only inspected");
    assert.equal(companionCalls.some(([kind]) => kind === "start"), false);
    assert.ok(companionCalls.every(([, server]) => server === "http://localhost:9337"),
      "status inspection receives the same local origin");

    const serve = makeCalls.find((call) => call.target === "serve");
    assert.ok(serve);
    assert.equal(serve.server, "http://localhost:9337");
  } finally {
    await rm(fixture.directory, { recursive: true, force: true });
  }
});

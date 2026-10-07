import assert from "node:assert/strict";
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const makefile = join(root, "Makefile");
const make = execFileSync("/bin/sh", ["-c", "command -v make"], { encoding: "utf8" }).trim();
const inheritedProductionServer = "https://komodoc.example/";

async function makeFixture({ running = false, port = 8081, sitePort = 8082, commandLineServer, config, upFailure = false, logsFailure = false, waitForLogs = false } = {}) {
  const directory = await mkdtemp(join(tmpdir(), "librepaper-demo-origin-"));
  const mockBinDir = join(directory, "mock-bin");
  const binPath = join(directory, "dist", "librepaper");
  const recursiveMakeLog = join(directory, "recursive-make.log");
  const companionLog = join(directory, "companion.log");
  const bunLog = join(directory, "bun.log");
  const toolsLog = join(directory, "tools.log");
  const dockerLog = join(directory, "docker.log");

  await Promise.all([
    mkdir(join(directory, "tools", "assets"), { recursive: true }),
    mkdir(join(directory, "tools", "dev", "demo"), { recursive: true }),
    mkdir(join(directory, "deploy"), { recursive: true }),
    ...["web/src", "web/public", "crates", "docs/examples"].map((path) =>
      mkdir(join(directory, path), { recursive: true })),
    mkdir(join(directory, "dist"), { recursive: true }),
    mkdir(mockBinDir, { recursive: true }),
  ]);

  await writeFile(join(directory, "tools", "dev", "demo-compose"),
    await readFile(join(root, "tools", "dev", "demo-compose")), { mode: 0o755 });
  await writeFile(join(directory, "tools", "dev", "demo", "config.toml"),
    await readFile(join(root, "tools", "dev", "demo", "config.toml")));
  await writeFile(join(directory, "deploy", "librepaper.toml"),
    await readFile(join(root, "deploy", "librepaper.toml")));
  await writeFile(join(directory, "deploy", "resticprofile.toml"), "# empty backup config\n");
  if (config) await writeFile(join(directory, "custom.toml"), "[server]\naddress = \"0.0.0.0:8080\"\n");

  await writeFile(join(directory, "mock-make"),
    '#!/bin/sh\ncase " $* " in *" site "*) target=site;; *" serve "*) target=serve;; *) target=unknown;; esac\nprintf "%s\\targs=%s\\tserver=%s\\tapp=%s\\n" "$target" "$*" "${LIBREPAPER_SERVER:-}" "${LIBREPAPER_APP_ORIGIN:-}" >> "$MOCK_RECURSIVE_MAKE_LOG"\nif [ "$target" = serve ] && [ "${MOCK_COMPANION_RUNNING:-0}" != 1 ]; then attempts=0; while ! grep -q \'^start\' "$MOCK_COMPANION_LOG" 2>/dev/null; do attempts=$((attempts + 1)); [ "$attempts" -lt 50 ] || break; sleep 0.1; done; fi\n',
    { mode: 0o755 });

  await writeFile(join(mockBinDir, "bun"),
    '#!/bin/sh\nprintf "%s\\tapp=%s\\n" "$*" "${LIBREPAPER_APP_ORIGIN:-}" >> "$MOCK_BUN_LOG"\ncase "$*" in *serve:site*) sleep 60 & child=$!; trap \'kill "$child" 2>/dev/null || true; wait "$child" 2>/dev/null || true\' EXIT INT TERM; wait "$child";; esac\n',
    { mode: 0o755 });
  await writeFile(join(mockBinDir, "firefox"), '#!/bin/sh\nprintf "%s\\n" "$*" >> "$MOCK_FIREFOX_LOG"\n', { mode: 0o755 });

  await writeFile(join(mockBinDir, "docker"), [
    "#!/bin/sh",
    "set -eu",
    "printf '%s|%s|%s|%s|%s|%s\\n' \"$*\" \"${DEMO_PROJECT:-}\" \"${LIBREPAPER_DEMO_APP_ORIGIN:-}\" \"${LIBREPAPER_DEMO_SITE_ORIGIN:-}\" \"${LIBREPAPER_DEMO_CONFIG:-}\" \"${LIBREPAPER_DEMO_BACKUP_CONFIG:-}\" >> \"$MOCK_DOCKER_LOG\"",
    "case \"$*\" in",
    "  *'compose version --short'*) printf '2.24.4\\n' ;;",
    "  info) : ;;",
    "  'volume inspect '*) echo 'Error: no such volume' >&2; exit 1 ;;",
    "  *' up --build --detach --wait --wait-timeout 180'*) [ \"$MOCK_DEMO_UP_FAILURE\" = 0 ] ;;",
    "  *' logs --follow'*)",
    "    if [ \"$MOCK_COMPANION_RUNNING\" = 0 ]; then attempts=0; while ! grep -q '^start' \"$MOCK_COMPANION_LOG\" 2>/dev/null; do attempts=$((attempts + 1)); [ \"$attempts\" -lt 100 ] || break; sleep 0.05; done; fi",
    "    [ \"$MOCK_DEMO_LOGS_FAILURE\" = 0 ] || exit 42",
    "    if [ \"$MOCK_DEMO_LOGS_WAIT\" = 1 ]; then",
    "      sleep 60 & child=$!",
    "      trap 'kill \"$child\" 2>/dev/null || true; wait \"$child\" 2>/dev/null || true; exit 0' INT TERM",
    "      wait \"$child\"",
    "    fi",
    "    ;;",
    "  *' logs --tail=100'*) : ;;",
    "  *' down --remove-orphans'*) : ;;",
    "  *) echo \"unexpected docker command: $*\" >&2; exit 93 ;;",
    "esac",
    "",
  ].join("\n"), { mode: 0o755 });

  await writeFile(join(directory, "tools", "assets", "mirror"),
    '#!/bin/sh\nprintf "%s\\n" "$*" >> "$MOCK_TOOLS_LOG"\n',
    { mode: 0o755 });
  await writeFile(join(directory, "dist", "librepaper"), [
    "#!/bin/sh",
    "case \"$1\" in",
    "  status)",
    "    printf 'Companion status\\nrunning at http://localhost:8763\\n'",
    "    printf 'status\\t%s\\n' \"${LIBREPAPER_SERVER:-}\" >> \"$MOCK_COMPANION_LOG\"",
    "    [ \"${MOCK_COMPANION_RUNNING:-0}\" = 1 ]",
    "    ;;",
    "  start)",
    "    printf 'start\\t%s\\t%s\\n' \"$*\" \"${LIBREPAPER_SERVER:-}\" >> \"$MOCK_COMPANION_LOG\"",
    "    sleep 60 & child=$!",
    "    trap 'echo stopped >> \"$MOCK_COMPANION_LOG\"; kill \"$child\" 2>/dev/null || true; wait \"$child\" 2>/dev/null || true; exit 0' INT TERM",
    "    wait \"$child\"",
    "    ;;",
    "  *) printf 'unexpected\\t%s\\n' \"$*\" >> \"$MOCK_COMPANION_LOG\"; exit 2 ;;",
    "esac",
    "",
  ].join("\n"), { mode: 0o755 });

  return {
    directory,
    binPath,
    recursiveMakeLog,
    companionLog,
    bunLog,
    toolsLog,
    dockerLog,
    port,
    sitePort,
    commandLineServer,
    config,
    running,
    upFailure,
    logsFailure,
    waitForLogs,
  };
}

function demoRunArguments(fixture) {
  const args = [
    `--old-file=${fixture.binPath}`,
    "--no-print-directory",
    "-f",
    makefile,
    "SHELL=/bin/sh",
    `BIN=${fixture.binPath}`,
    `MAKE=${join(fixture.directory, "mock-make")}`,
  ];
  if (fixture.commandLineServer) args.push(`LIBREPAPER_SERVER=${fixture.commandLineServer}`);
  if (fixture.config) args.push(`CONFIG=${fixture.config}`);
  args.push(`PORT=${fixture.port}`, `SITE_PORT=${fixture.sitePort}`, "OPEN=0");
  args.push("demo-run");
  return args;
}

function demoRunEnvironment(fixture) {
  const environment = {
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
    MOCK_DOCKER_LOG: fixture.dockerLog,
    MOCK_FIREFOX_LOG: join(fixture.directory, "firefox.log"),
    MOCK_COMPANION_RUNNING: fixture.running ? "1" : "0",
    MOCK_DEMO_UP_FAILURE: fixture.upFailure ? "1" : "0",
    MOCK_DEMO_LOGS_FAILURE: fixture.logsFailure ? "1" : "0",
    MOCK_DEMO_LOGS_WAIT: fixture.waitForLogs ? "1" : "0",
    DEMO_PROJECT: `librepaper-demo-test-${process.pid}`,
  };
  delete environment.CONFIG;
  return environment;
}

function runDemoRun(fixture) {
  return spawnSync(make, demoRunArguments(fixture), {
    cwd: fixture.directory,
    encoding: "utf8",
    timeout: 20000,
    env: demoRunEnvironment(fixture),
  });
}

function startDemoRun(fixture) {
  const environment = {
    ...demoRunEnvironment(fixture),
    CONFIG: fixture.config || "tools/dev/demo/config.toml",
    BIN: fixture.binPath,
    PORT: String(fixture.port),
    SITE_PORT: String(fixture.sitePort),
    OPEN: "0",
  };
  return spawn(join(fixture.directory, "tools", "dev", "demo-compose"), ["run"], {
    cwd: fixture.directory,
    stdio: "ignore", env: environment,
  });
}

async function waitForDockerLog(fixture, pattern) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const contents = await readFile(fixture.dockerLog, "utf8").catch(() => "");
    if (pattern.test(contents)) return contents;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(`timed out waiting for docker log matching ${pattern}`);
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
  { name: "inherited production server", commandLineServer: null },
  { name: "command-line server", commandLineServer: "https://caller.example/" },
]) {
  test(`demo-run gives the local companion origin precedence over the ${scenario.name}`, async () => {
    const fixture = await makeFixture({ port: 8081, commandLineServer: scenario.commandLineServer });
    try {
      const result = runDemoRun(fixture);
      assert.equal(result.status, 0, result.stderr || result.stdout);
      const { makeCalls, companionCalls } = await logs(fixture);
      const localServer = `http://localhost:${fixture.port}`;
      const starts = companionCalls.filter(([kind]) => kind === "start");
      assert.equal(starts.length, 1, "demo-run starts the companion when status reports it stopped");
      assert.equal(starts[0][2], localServer, "start receives LIBREPAPER_SERVER for the local app");

      const siteBuild = makeCalls.find((call) => call.target === "site");
      assert.ok(siteBuild, "demo-run builds the site before launching containers");
      assert.equal(siteBuild.app, localServer, "the built site points at the local app");
      assert.equal(makeCalls.some((call) => call.target === "serve"), false,
        "the Docker stack replaces native serve in the demo path");
      const dockerLog = await readFile(fixture.dockerLog, "utf8");
      assert.match(dockerLog, new RegExp(`--project-name librepaper-demo-test-${process.pid}`));
      assert.match(dockerLog, new RegExp(`\\|${localServer}\\|http://localhost:${fixture.sitePort}\\|`));
      assert.match(dockerLog, new RegExp(`\\|${fixture.directory}/tools/dev/demo/config\\.toml\\|${fixture.directory}/deploy/resticprofile\\.toml$`, "m"));
    } finally {
      await rm(fixture.directory, { recursive: true, force: true });
    }
  });
}

test("demo-run leaves an already running companion untouched", async () => {
  const fixture = await makeFixture({ running: true });
  try {
    const result = runDemoRun(fixture);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const { makeCalls, companionCalls } = await logs(fixture);
    assert.ok(companionCalls.length > 0);
    assert.ok(companionCalls.every(([kind]) => kind === "status"), "an already running companion is only inspected");
    assert.equal(companionCalls.some(([kind]) => kind === "start"), false);
    assert.ok(companionCalls.every(([, server]) => server === `http://localhost:${fixture.port}`),
      "status inspection receives the same local origin");

    assert.ok(makeCalls.some((call) => call.target === "site"));
    const dockerLog = await readFile(fixture.dockerLog, "utf8");
    assert.match(dockerLog, /--project-name librepaper-demo-test-/);
  } finally {
    await rm(fixture.directory, { recursive: true, force: true });
  }
});

test("demo-run passes an explicit container-ready CONFIG through to the launcher", async () => {
  const fixture = await makeFixture({ config: "custom.toml" });
  try {
    const result = runDemoRun(fixture);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const dockerLog = await readFile(fixture.dockerLog, "utf8");
    assert.match(dockerLog, new RegExp(`\\|${fixture.directory}/custom\\.toml\\|${fixture.directory}/deploy/resticprofile\\.toml$`, "m"));
  } finally {
    await rm(fixture.directory, { recursive: true, force: true });
  }
});

test("demo startup failure shows logs and removes only demo containers", async () => {
  const fixture = await makeFixture({ upFailure: true });
  try {
    const result = runDemoRun(fixture);
    assert.notEqual(result.status, 0);
    const dockerLog = await readFile(fixture.dockerLog, "utf8");
    assert.match(dockerLog, / up --build --detach --wait --wait-timeout 180/);
    assert.match(dockerLog, / logs --tail=100/);
    assert.match(dockerLog, / down --remove-orphans/);
    assert.equal((await readFile(fixture.companionLog, "utf8").catch(() => "")).includes("start\t"), false);
  } finally {
    await rm(fixture.directory, { recursive: true, force: true });
  }
});

test("a log failure stops only the companion started for this demo", async () => {
  const fixture = await makeFixture({ logsFailure: true });
  try {
    const result = runDemoRun(fixture);
    assert.notEqual(result.status, 0);
    const dockerLog = await readFile(fixture.dockerLog, "utf8");
    assert.match(dockerLog, / logs --follow/);
    assert.match(dockerLog, / logs --tail=100/);
    assert.match(dockerLog, / down --remove-orphans/);
    const companionLog = await readFile(fixture.companionLog, "utf8");
    assert.match(companionLog, /^start\t/m);
    assert.match(companionLog, /^stopped$/m);
  } finally {
    await rm(fixture.directory, { recursive: true, force: true });
  }
});

test("SIGTERM stops the log follower, owned companion and demo containers", async () => {
  const fixture = await makeFixture({ waitForLogs: true });
  let child;
  try {
    child = startDemoRun(fixture);
    await waitForDockerLog(fixture, / logs --follow/);
    child.kill("SIGTERM");
    let timeout;
    const exitCode = await Promise.race([new Promise((resolveExit, reject) => {
      child.once("error", reject);
      child.once("exit", (code, signal) => resolveExit(code ?? signal));
    }), new Promise((_, reject) => { timeout = setTimeout(() => reject(new Error("launcher did not exit after SIGTERM")), 5000); })]);
    clearTimeout(timeout);
    assert.notEqual(exitCode, 0, "the signaled launcher exits without reporting success");
    const dockerLog = await readFile(fixture.dockerLog, "utf8");
    assert.match(dockerLog, / down --remove-orphans/);
    const companionLog = await readFile(fixture.companionLog, "utf8");
    assert.match(companionLog, /^stopped$/m);
  } finally {
    if (child && child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    await rm(fixture.directory, { recursive: true, force: true });
  }
});

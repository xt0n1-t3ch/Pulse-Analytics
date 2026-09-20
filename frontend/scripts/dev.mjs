import { randomBytes } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const workspaceRoot = path.resolve(frontendRoot, "..");
const token = randomBytes(32).toString("hex");
// The bridge child and Vite config must share the same per-run secret. Keeping
// it in this process environment also lets the programmatic Vite server install
// its authenticated same-origin proxy instead of serving a disconnected UI.
process.env.PULSE_DEV_BRIDGE_TOKEN = token;

function cliValue(name, fallback) {
  const exact = process.argv.indexOf(name);
  if (exact >= 0 && process.argv[exact + 1] && !process.argv[exact + 1].startsWith("--")) {
    return process.argv[exact + 1];
  }
  const prefixed = process.argv.find((arg) => arg.startsWith(`${name}=`));
  return prefixed ? prefixed.slice(name.length + 1) : fallback;
}

const viteHost = cliValue("--host", "127.0.0.1");
const vitePort = Number(cliValue("--port", process.env.PULSE_DEV_PORT ?? "1420"));
const bridgePort = Number(cliValue("--bridge-port", process.env.PULSE_DEV_BRIDGE_PORT ?? "1421"));
if (viteHost !== "127.0.0.1" && viteHost !== "localhost") {
  throw new Error("Pulse dev mode is loopback-only; --host must be 127.0.0.1 or localhost.");
}
if (![vitePort, bridgePort].every((port) => Number.isInteger(port) && port >= 1024 && port <= 65535)
    || vitePort === bridgePort) {
  throw new Error("Pulse dev mode requires distinct UI and bridge ports between 1024 and 65535.");
}
process.env.PULSE_DEV_PORT = String(vitePort);
process.env.PULSE_DEV_BRIDGE_PORT = String(bridgePort);
const env = { ...process.env };

function buildBridge() {
  const args = [
    "build",
    "-p",
    "pulse",
    "--bin",
    "pulse-dev-bridge",
    "--message-format=json-render-diagnostics",
  ];
  if (env.PULSE_CODEX_CORE_PATH) {
    args.push("--config", `patch.\"https://github.com/xt0n1-t3ch/Codex-Discord-Rich-Presence\".codex-presence-core.path=${JSON.stringify(path.resolve(env.PULSE_CODEX_CORE_PATH))}`);
  }
  const result = spawnSync("cargo", args, {
    cwd: workspaceRoot,
    env,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error) throw result.error;
  for (const line of (result.stdout ?? "").split(/\r?\n/)) {
    if (!line) continue;
    try {
      const message = JSON.parse(line);
      if (message.reason === "compiler-message" && message.message?.rendered) {
        process.stderr.write(message.message.rendered);
      }
    } catch {
      process.stderr.write(`${line}\n`);
    }
  }
  if (result.status !== 0) {
    if (result.stderr) process.stderr.write(result.stderr);
    throw new Error(`cargo build failed with exit code ${result.status ?? "unknown"}`);
  }
  const artifacts = (result.stdout ?? "")
    .split(/\r?\n/)
    .filter(Boolean)
    .flatMap((line) => {
      try {
        const message = JSON.parse(line);
        return message.reason === "compiler-artifact"
          && message.target?.name === "pulse-dev-bridge"
          && message.executable
          ? [message.executable]
          : [];
      } catch {
        return [];
      }
    });
  const executable = artifacts.at(-1);
  if (!executable) {
    throw new Error("cargo build did not report the pulse-dev-bridge executable");
  }
  return executable;
}

const executable = buildBridge();
const bridge = spawn(executable, [], {
  cwd: workspaceRoot,
  env,
  stdio: "inherit",
  windowsHide: true,
});

let shuttingDown = false;
let server;

function waitForExit(child, timeoutMs) {
  if (child.exitCode != null) return Promise.resolve(true);
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      child.off("exit", onExit);
      resolve(false);
    }, timeoutMs);
    function onExit() {
      clearTimeout(timer);
      resolve(true);
    }
    child.once("exit", onExit);
  });
}

async function shutdown(exitCode = 0) {
  if (shuttingDown) return;
  shuttingDown = true;
  await server?.close();
  if (bridge.exitCode == null) {
    bridge.kill();
    if (!(await waitForExit(bridge, 2_000)) && bridge.exitCode == null) {
      bridge.kill("SIGKILL");
      await waitForExit(bridge, 2_000);
    }
  }
  process.exitCode = exitCode;
}

bridge.once("error", (error) => {
  process.stderr.write(`pulse-dev-bridge failed to start: ${error.message}\n`);
  if (!shuttingDown) void shutdown(1);
});
bridge.once("exit", (code) => {
  if (!shuttingDown) void shutdown(code ?? 1);
});
process.once("SIGINT", () => void shutdown());
process.once("SIGTERM", () => void shutdown());

async function waitForBridge() {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    if (bridge.exitCode != null) {
      throw new Error(`pulse-dev-bridge exited with code ${bridge.exitCode}`);
    }
    try {
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 400);
      const response = await fetch(`http://127.0.0.1:${bridgePort}/invoke`, {
        method: "POST",
        signal: controller.signal,
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ command: "get_health", args: {} }),
      });
      clearTimeout(timeout);
      if (response.ok) return;
    } catch {
      // The bridge is still binding; retry within the bounded startup window.
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("pulse-dev-bridge did not become ready within 5 seconds");
}

try {
  await waitForBridge();
  server = await createServer({
    root: frontendRoot,
    server: { host: viteHost, port: vitePort, strictPort: true },
  });
  await server.listen();
  server.printUrls();
} catch (error) {
  await shutdown(1);
  throw error;
}

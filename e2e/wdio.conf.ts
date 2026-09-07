// WebdriverIO against tauri-driver (§ "Testing on Linux and Windows").
// Runs only where tauri-driver exists - Linux (WebKitWebDriver) and Windows
// (msedgedriver) - and expects a seeded, offline app (examples/e2e_seed.rs)
// and an already-built debug binary; the VM jobs provide both.
//
//   E2E_APP            path to the app binary (default: the debug build)
//   E2E_NATIVE_DRIVER  Windows: path to msedgedriver.exe matching WebView2
import { spawn, type ChildProcess } from "node:child_process";
import { connect } from "node:net";
import { resolve } from "node:path";

const exe = process.platform === "win32" ? "exodium.exe" : "exodium";
const application = resolve(
  process.env.E2E_APP ?? resolve(import.meta.dirname, "../src-tauri/target/debug", exe),
);

let tauriDriver: ChildProcess | undefined;

export const config: WebdriverIO.Config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  // One session for every spec (nested array): after a session ends,
  // tauri-driver answers the next one with hyper "SendRequest" errors only.
  specs: [["./specs/**/*.e2e.ts"]],
  maxInstances: 1,
  // `tauri:options` is tauri-driver's own key, unknown to WDIO's types.
  capabilities: [
    {
      // tauri-driver speaks classic WebDriver only; without this WDIO 9
      // asks for BiDi and waits on a websocket that never comes.
      "wdio:enforceWebDriverClassic": true,
      "tauri:options": { application },
    } as WebdriverIO.Capabilities,
  ],
  logLevel: "warn",
  // 60 s, not 20: the Windows guest answers a catalogue search in 5-10 s.
  waitforTimeout: 60_000,
  connectionRetryTimeout: 120_000,
  connectionRetryCount: 2,
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 120_000 },

  // One driver for the whole run, not one per session: a kill in
  // afterSession and a respawn in beforeSession race for the port, and the
  // second spec then talks to a dead socket.
  onPrepare: async () => {
    const args = process.env.E2E_NATIVE_DRIVER
      ? ["--native-driver", process.env.E2E_NATIVE_DRIVER]
      : [];
    tauriDriver = spawn("tauri-driver", args, { stdio: ["ignore", process.stdout, process.stderr] });
    await waitForPort(4444, 15_000);
  },
  onComplete: () => {
    tauriDriver?.kill();
  },
};

function waitForPort(port: number, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  return new Promise((done, fail) => {
    const attempt = () => {
      const sock = connect({ port, host: "127.0.0.1" });
      sock.once("connect", () => { sock.destroy(); done(); });
      sock.once("error", () => {
        sock.destroy();
        if (Date.now() > deadline) { fail(new Error(`tauri-driver did not listen on ${port}`)); }
        else { setTimeout(attempt, 250); }
      });
    };
    attempt();
  });
}

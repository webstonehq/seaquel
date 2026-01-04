import { spawn, ChildProcess, execSync } from "child_process";
import path from "path";
import os from "os";
import { fileURLToPath } from "url";
import {
  clearAppState,
  seedConnectionState,
  createTestConnection,
} from "./e2e-tests/helpers/app-actions.js";

// ES module equivalent of __dirname
const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// Track tauri-driver processes per worker
const tauriDrivers: Map<string, ChildProcess> = new Map();
let portCounter = 0;

// Get the built app path
function getAppPath(): string {
  const basePath = path.join(__dirname, "src-tauri", "target", "release");

  if (process.platform === "darwin") {
    return path.join(basePath, "bundle", "macos", "Seaquel.app");
  } else if (process.platform === "win32") {
    return path.join(basePath, "Seaquel.exe");
  }
  return path.join(basePath, "seaquel");
}

export const config = {
  runner: "local",
  specs: ["./e2e-tests/specs/**/*.spec.ts"],
  exclude: [],
  maxInstances: 4,
  capabilities: [
    {
      maxInstances: 4,
      "tauri:options": {
        application: getAppPath(),
      },
    },
  ],
  logLevel: "info",
  bail: 0,
  waitforTimeout: 10000,
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: {
    ui: "bdd",
    timeout: 60000,
  },

  onPrepare: async function () {
    console.log("Building Tauri application...");
    execSync("npm run tauri build", { stdio: "inherit" });
  },

  beforeSession: async function (_config, _capabilities, specs) {
    const port = 4444 + portCounter++;
    const workerId = `worker-${port}`;
    const specPath = specs[0] || "";

    // Set worker ID for test isolation (used by both app and test helpers)
    process.env.SEAQUEL_TEST_WORKER_ID = workerId;

    console.log(`Starting tauri-driver on port ${port} for ${workerId}`);

    // Set up state BEFORE the app starts
    // Check if this spec file needs seeded connections
    if (specPath.includes("connections-grid")) {
      // Seed with a test connection for tests that need existing connections
      const testConnection = createTestConnection({
        id: "test-conn-1",
        name: "Test PostgreSQL",
        type: "postgres",
        host: "localhost",
        port: 5432,
        databaseName: "testdb",
        username: "testuser",
      });
      await seedConnectionState([testConnection]);
      console.log(`Seeded test connection for ${specPath}`);
    } else {
      // Clear state for tests that expect empty state
      await clearAppState();
      console.log(`Cleared app state for ${specPath}`);
    }

    const tauriDriverPath = path.join(
      os.homedir(),
      ".cargo",
      "bin",
      "tauri-driver",
    );

    const tauriDriver = spawn(tauriDriverPath, ["--port", String(port)], {
      stdio: ["ignore", "pipe", "pipe"],
    });

    tauriDrivers.set(workerId, tauriDriver);

    // Wait for tauri-driver to be ready
    await new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => {
        resolve(); // Proceed even if no "Listening" message
      }, 3000);

      tauriDriver.stdout?.on("data", (data) => {
        const output = data.toString();
        console.log(`tauri-driver [${port}]: ${output}`);
        if (output.includes("Listening")) {
          clearTimeout(timeout);
          resolve();
        }
      });

      tauriDriver.stderr?.on("data", (data) => {
        console.error(`tauri-driver error [${port}]: ${data.toString()}`);
      });

      tauriDriver.on("error", (err) => {
        clearTimeout(timeout);
        reject(err);
      });
    });

    // Update the port for this session
    _config.port = port;
  },

  afterSession: async function (_config, _capabilities, _specs) {
    const workerId = `worker-${_config.port}`;
    const tauriDriver = tauriDrivers.get(workerId);

    if (tauriDriver) {
      console.log(`Stopping tauri-driver for ${workerId}`);
      tauriDriver.kill();
      tauriDrivers.delete(workerId);
    }
  },

  after: async function () {
    // Clean up test store directories
    const storePattern = path.join(
      os.homedir(),
      process.platform === "darwin"
        ? "Library/Application Support"
        : process.platform === "win32"
          ? "AppData/Roaming"
          : ".local/share",
      "app.seaquel.desktop.test-*",
    );

    // Note: Cleanup is done per-test in app-actions.ts
  },

  hostname: "127.0.0.1",
  port: 4444, // Default, overridden per session
};

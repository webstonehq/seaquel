import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { gunzipSync } from "node:zlib";
import { describe, expect, it } from "vitest";
import {
  assetName,
  BINS,
  builtPath,
  cargoBuildArgs,
  gzipFile,
  gzipName,
  parseArgs,
  RELEASE_PROFILE,
} from "./build-cli.mjs";

describe("build-cli arguments", () => {
  it("builds seaquel-cli when no --bin is given", () => {
    expect(parseArgs([])).toEqual({
      release: false,
      target: null,
      bin: "seaquel-cli",
      gzip: false,
      help: false,
    });
    expect(parseArgs(["--release", "--target", "aarch64-apple-darwin"])).toEqual({
      release: true,
      target: "aarch64-apple-darwin",
      bin: "seaquel-cli",
      gzip: false,
      help: false,
    });
  });

  it("takes --bin seaquel-tui in either form", () => {
    expect(parseArgs(["--bin", "seaquel-tui"]).bin).toBe("seaquel-tui");
    expect(parseArgs(["--release", "--bin=seaquel-tui"])).toMatchObject({
      release: true,
      bin: "seaquel-tui",
    });
    expect(parseArgs(["--bin", "seaquel-cli"]).bin).toBe("seaquel-cli");
  });

  it("refuses an unknown --bin, naming the ones it knows", () => {
    expect(() => parseArgs(["--bin", "seaquel"])).toThrow(
      "unknown --bin `seaquel`. Use one of: seaquel-cli, seaquel-tui, seaquel-duckdb.",
    );
    expect(() => parseArgs(["--bin"])).toThrow(
      "--bin needs a binary: seaquel-cli, seaquel-tui, seaquel-duckdb.",
    );
    expect(() => parseArgs(["--frobnicate"])).toThrow("unknown argument `--frobnicate`");
  });

  it("knows each binary's package", () => {
    expect(BINS).toEqual({
      "seaquel-cli": "seaquel-cli",
      "seaquel-tui": "seaquel-tui",
      "seaquel-duckdb": "seaquel-duckdb",
    });
  });

  it("takes --bin seaquel-duckdb and --gzip", () => {
    expect(parseArgs(["--release", "--bin", "seaquel-duckdb", "--gzip"])).toEqual({
      release: true,
      target: null,
      bin: "seaquel-duckdb",
      gzip: true,
      help: false,
    });
    expect(cargoBuildArgs({ bin: "seaquel-duckdb", release: true, target: null })).toEqual([
      "build",
      "-p",
      "seaquel-duckdb",
      "--bin",
      "seaquel-duckdb",
      "--profile",
      "terminal-release",
    ]);
  });

  it("names the gzipped asset after the binary's", () => {
    expect(gzipName("seaquel-duckdb", "aarch64-apple-darwin")).toBe(
      "seaquel-duckdb-aarch64-apple-darwin.gz",
    );
    expect(gzipName("seaquel-duckdb", "x86_64-pc-windows-msvc")).toBe(
      "seaquel-duckdb-x86_64-pc-windows-msvc.exe.gz",
    );
  });

  it("names the asset per target, with .exe on Windows", () => {
    expect(assetName("seaquel-tui", "aarch64-apple-darwin")).toBe(
      "seaquel-tui-aarch64-apple-darwin",
    );
    expect(assetName("seaquel-tui", "x86_64-pc-windows-msvc")).toBe(
      "seaquel-tui-x86_64-pc-windows-msvc.exe",
    );
    expect(assetName("seaquel-cli", "aarch64-pc-windows-msvc")).toBe(
      "seaquel-cli-aarch64-pc-windows-msvc.exe",
    );
  });
});

describe("build-cli cargo invocation", () => {
  it("builds a release with the size-tuned terminal profile", () => {
    expect(RELEASE_PROFILE).toBe("terminal-release");
    expect(cargoBuildArgs({ bin: "seaquel-tui", release: true, target: null })).toEqual([
      "build",
      "-p",
      "seaquel-tui",
      "--bin",
      "seaquel-tui",
      "--profile",
      "terminal-release",
    ]);
    expect(
      cargoBuildArgs({
        bin: "seaquel-cli",
        release: true,
        target: "x86_64-pc-windows-msvc",
      }),
    ).toEqual([
      "build",
      "-p",
      "seaquel-cli",
      "--bin",
      "seaquel-cli",
      "--profile",
      "terminal-release",
      "--target",
      "x86_64-pc-windows-msvc",
    ]);
  });

  it("keeps debug builds on the dev profile", () => {
    expect(cargoBuildArgs({ bin: "seaquel-cli", release: false, target: null })).toEqual([
      "build",
      "-p",
      "seaquel-cli",
      "--bin",
      "seaquel-cli",
    ]);
  });

  it("finds the binary in the profile's directory, under the triple when one was passed", () => {
    const t = join("/t", "target");
    expect(
      builtPath(t, {
        bin: "seaquel-tui",
        release: true,
        target: null,
        triple: "aarch64-apple-darwin",
      }),
    ).toBe(join(t, "terminal-release", "seaquel-tui"));
    expect(
      builtPath(t, {
        bin: "seaquel-cli",
        release: true,
        target: "x86_64-pc-windows-msvc",
        triple: "x86_64-pc-windows-msvc",
      }),
    ).toBe(join(t, "x86_64-pc-windows-msvc", "terminal-release", "seaquel-cli.exe"));
    expect(
      builtPath(t, {
        bin: "seaquel-cli",
        release: false,
        target: null,
        triple: "aarch64-apple-darwin",
      }),
    ).toBe(join(t, "debug", "seaquel-cli"));
  });
});

describe("build-cli gzip", () => {
  it("writes a .gz that unpacks to the same bytes, replacing an old one", () => {
    const dir = mkdtempSync(join(tmpdir(), "build-cli-gz-"));
    try {
      const src = join(dir, "bin");
      const bytes = Buffer.alloc(300_000);
      for (let i = 0; i < bytes.length; i++) bytes[i] = (i * 7919) % 251;
      writeFileSync(src, bytes);
      const dest = join(dir, "bin.gz");
      writeFileSync(dest, "stale");
      gzipFile(src, dest);
      expect(gunzipSync(readFileSync(dest)).equals(bytes)).toBe(true);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("build-cli run as a program", () => {
  // npm, a symlinked checkout or a linked bin dir can put a symlink in
  // argv[1]; the script still has to run.
  it("runs when started through a symlink", () => {
    const dir = mkdtempSync(join(tmpdir(), "build-cli-"));
    try {
      const link = join(dir, "build-cli.mjs");
      symlinkSync(resolve(import.meta.dirname, "build-cli.mjs"), link);
      const r = spawnSync(process.execPath, [link, "--help"], {
        encoding: "utf8",
      });
      expect(r.status).toBe(0);
      expect(r.stdout).toContain("usage: node scripts/build-cli.mjs");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("does nothing when imported", () => {
    // Importing this file (above) built nothing and printed no usage; a
    // bad argument from the command line still fails.
    const r = spawnSync(
      process.execPath,
      [resolve(import.meta.dirname, "build-cli.mjs"), "--bin", "x"],
      {
        encoding: "utf8",
      },
    );
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("unknown --bin `x`");
  });
});

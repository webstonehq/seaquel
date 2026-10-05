// The DuckDB helper's pin in the release job (the desktop DuckDB helper plan,
// Task 9): what `release-pin.mjs` exports for the app's build, how it checks
// the built app, and that the uploaded .gz is the one it hashed.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  appBinaryCandidates,
  carriesPin,
  compiledText,
  envLines,
  helperAsset,
  pinOf,
  productName,
  recordName,
} from "./release-pin.mjs";

const script = resolve(import.meta.dirname, "release-pin.mjs");

describe("the pin's values", () => {
  it("is the file's byte size and lowercase SHA-256", () => {
    const bytes = Buffer.from("hello");
    expect(pinOf(bytes)).toEqual({
      size: 5,
      sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    });
  });

  it("exports plain decimal and 64 hex digits with no prefix, as src-tauri/build.rs reads them", () => {
    const pin = pinOf(Buffer.alloc(12_345_678, 1));
    const lines = envLines(pin).trimEnd().split("\n");
    expect(lines).toHaveLength(2);
    expect(lines[0]).toBe("SEAQUEL_DUCKDB_HELPER_SIZE=12345678");
    expect(lines[1]).toMatch(/^SEAQUEL_DUCKDB_HELPER_SHA256=[0-9a-f]{64}$/);
  });

  it("names the asset Core asks for", () => {
    expect(helperAsset("aarch64-apple-darwin")).toBe("seaquel-duckdb-aarch64-apple-darwin.gz");
    expect(helperAsset("x86_64-pc-windows-msvc")).toBe(
      "seaquel-duckdb-x86_64-pc-windows-msvc.exe.gz",
    );
    expect(recordName("x86_64-unknown-linux-gnu")).toBe("duckdb-pin-x86_64-unknown-linux-gnu.json");
  });

  it("finds the compiled text (`<size>:<sha256>`, build.rs's pin_text) in a binary", () => {
    const pin = { size: 4096, sha256: "ab".repeat(32) };
    expect(compiledText(pin)).toBe(`4096:${"ab".repeat(32)}`);
    const binary = Buffer.concat([
      Buffer.alloc(1000, 7),
      Buffer.from(compiledText(pin)),
      Buffer.alloc(10),
    ]);
    expect(carriesPin(binary, pin)).toBe(true);
    expect(carriesPin(binary, { ...pin, size: 4097 })).toBe(false);
    expect(carriesPin(binary, { ...pin, sha256: "cd".repeat(32) })).toBe(false);
  });

  it("looks for the app where a workspace build puts it", () => {
    expect(appBinaryCandidates("aarch64-apple-darwin", { repo: "/r" })).toEqual([
      "/r/target/aarch64-apple-darwin/release/seaquel",
      "/r/src-tauri/target/aarch64-apple-darwin/release/seaquel",
    ]);
    expect(
      appBinaryCandidates("x86_64-pc-windows-msvc", { repo: "/r", cargoTargetDir: "/t" })[0],
    ).toBe("/t/x86_64-pc-windows-msvc/release/seaquel.exe");
  });
});

describe("release-pin.mjs in a release job", () => {
  const target = "x86_64-unknown-linux-gnu";
  let repo;
  let env;
  const run = (...args) =>
    spawnSync(process.execPath, [script, ...args, "--target", target], {
      encoding: "utf8",
      env: { ...process.env, RELEASE_PIN_ROOT: repo, GITHUB_ENV: env, CARGO_TARGET_DIR: "" },
    });
  const gz = (t = target) => join(repo, "src-tauri", "binaries", helperAsset(t));
  const record = () =>
    JSON.parse(readFileSync(join(repo, "release-pins", recordName(target)), "utf8"));
  const app = () => join(repo, "target", target, "release", "seaquel");

  beforeEach(() => {
    repo = mkdtempSync(join(tmpdir(), "release-pin-"));
    env = join(repo, "github_env");
    writeFileSync(env, "EARLIER=1\n");
    mkdirSync(join(repo, "src-tauri", "binaries"), { recursive: true });
    writeFileSync(gz(), Buffer.from([0x1f, 0x8b, 8, 0, 1, 2, 3, 4]));
    writeFileSync(
      join(repo, "src-tauri", "tauri.conf.json"),
      JSON.stringify({ productName: "Seaquel Test" }),
    );
  });
  afterEach(() => rmSync(repo, { recursive: true, force: true }));

  it("exports the pin to $GITHUB_ENV and records it", () => {
    const r = run("export");
    expect(r.status, r.stderr).toBe(0);
    const sha256 = createHash("sha256").update(readFileSync(gz())).digest("hex");
    expect(readFileSync(env, "utf8")).toBe(
      `EARLIER=1\nSEAQUEL_DUCKDB_HELPER_SIZE=8\nSEAQUEL_DUCKDB_HELPER_SHA256=${sha256}\n`,
    );
    expect(record()).toEqual({ target, asset: helperAsset(target), size: 8, sha256 });
  });

  it("refuses to export without the gzipped helper", () => {
    rmSync(gz());
    const r = run("export");
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("build, sign and gzip the helper first");
  });

  it("records a pinned app, and fails naming an app that isn't", () => {
    expect(run("export").status).toBe(0);
    const { size, sha256 } = record();
    mkdirSync(join(repo, "target", target, "release"), { recursive: true });

    writeFileSync(
      app(),
      Buffer.concat([Buffer.alloc(64), Buffer.from(`${size}:${sha256}`), Buffer.alloc(64)]),
    );
    let r = run("verify-app");
    expect(r.status, r.stderr).toBe(0);
    expect(record().app).toEqual({ pinned: true });

    // Built without the pin (or with another .gz's).
    writeFileSync(
      app(),
      Buffer.concat([Buffer.alloc(64), Buffer.from(`${size}:${"0".repeat(64)}`)]),
    );
    r = run("verify-app");
    expect(r.status).toBe(1);
    expect(r.stderr).toContain(`doesn't hold ${size}:${sha256}`);
    expect(record().app.pinned).toBe(false);
  });

  it("fails when the app binary isn't there, naming where it looked", () => {
    expect(run("export").status).toBe(0);
    const r = run("verify-app");
    expect(r.status).toBe(1);
    expect(r.stderr).toContain(join("target", target, "release", "seaquel"));
    expect(record().app.pinned).toBe(false);
    expect(record().app.reason).toContain("no app binary found");
  });

  it("on macOS also checks the bundle's binary, the updater's copy, by the product name", () => {
    const mac = "aarch64-apple-darwin";
    writeFileSync(gz(mac), Buffer.from("mac helper gz"));
    const runMac = (...args) =>
      spawnSync(process.execPath, [script, ...args, "--target", mac], {
        encoding: "utf8",
        env: { ...process.env, RELEASE_PIN_ROOT: repo, GITHUB_ENV: env, CARGO_TARGET_DIR: "" },
      });
    expect(runMac("export").status).toBe(0);
    const rec = JSON.parse(readFileSync(join(repo, "release-pins", recordName(mac)), "utf8"));
    const pinned = Buffer.from(`..${rec.size}:${rec.sha256}..`);
    const release = join(repo, "target", mac, "release");
    mkdirSync(release, { recursive: true });
    writeFileSync(join(release, "seaquel"), pinned);

    // No bundle: refused, not skipped.
    let r = runMac("verify-app");
    expect(r.status).toBe(1);
    expect(r.stderr).toContain(join("bundle", "macos", "Seaquel Test.app", "Contents", "MacOS"));

    const macos = join(release, "bundle", "macos", "Seaquel Test.app", "Contents", "MacOS");
    mkdirSync(macos, { recursive: true });
    writeFileSync(join(macos, "seaquel"), Buffer.from("an unpinned build"));
    r = runMac("verify-app");
    expect(r.status).toBe(1);
    expect(r.stderr).toContain(join("Contents", "MacOS", "seaquel"));

    writeFileSync(join(macos, "seaquel"), pinned);
    r = runMac("verify-app");
    expect(r.status, r.stderr).toBe(0);
    expect(
      JSON.parse(readFileSync(join(repo, "release-pins", recordName(mac)), "utf8")).app,
    ).toEqual({ pinned: true });
  });

  it("reads the product name from tauri.conf.json", () => {
    expect(productName(repo)).toBe("Seaquel Test");
    expect(productName(resolve(import.meta.dirname, ".."))).toBe("Seaquel");
  });
});

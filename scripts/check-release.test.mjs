// The release check:
// a draft is ready only when every target has its pinned app, its
// helper as pinned, its CLI and TUI and its updater entry, and the release
// workflow is wired so the pin reaches the app and the check runs last.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { parse } from "yaml";
import { assetName } from "./build-cli.mjs";
import { checkRelease, RELEASE_TARGETS, targetOfPlatform } from "./check-release.mjs";
import { helperAsset, recordName } from "./release-pin.mjs";

const root = resolve(import.meta.dirname, "..");
const TAG = "v2026.10.0";
const sha = (text) => createHash("sha256").update(text).digest("hex");
const helperBytes = (target) => Buffer.from(`gzipped helper for ${target}`);

const PLATFORM = {
  "aarch64-apple-darwin": ["darwin-aarch64", "Seaquel_aarch64.app.tar.gz"],
  "x86_64-apple-darwin": ["darwin-x86_64", "Seaquel_x64.app.tar.gz"],
  "x86_64-unknown-linux-gnu": ["linux-x86_64", "Seaquel_2026.10.0_amd64.AppImage"],
  "aarch64-unknown-linux-gnu": ["linux-aarch64", "Seaquel_2026.10.0_aarch64.AppImage"],
  "x86_64-pc-windows-msvc": ["windows-x86_64", "Seaquel_26.10.0_x64-setup.exe"],
  "aarch64-pc-windows-msvc": ["windows-aarch64", "Seaquel_26.10.0_arm64-setup.exe"],
};

/** A complete draft, as the jobs leave it. */
function complete() {
  const records = {};
  const assets = [];
  const files = {};
  const platforms = {};
  for (const target of RELEASE_TARGETS) {
    const bytes = helperBytes(target);
    const helper = helperAsset(target);
    records[target] = {
      target,
      asset: helper,
      size: bytes.length,
      sha256: sha(bytes),
      app: { pinned: true },
    };
    assets.push({ name: helper, size: bytes.length });
    files[helper] = { size: bytes.length, sha256: sha(bytes) };
    assets.push({ name: assetName("seaquel-cli", target), size: 1 });
    assets.push({ name: assetName("seaquel-tui", target), size: 1 });
    const [key, bundle] = PLATFORM[target];
    assets.push({ name: bundle, size: 1 }, { name: `${bundle}.sig`, size: 1 });
    platforms[key] = {
      signature: "sig",
      url: `https://github.com/webstonehq/seaquel/releases/download/${TAG}/${bundle}`,
    };
  }
  assets.push({ name: "latest.json", size: 1 });
  return {
    tag: TAG,
    jobs: {
      "publish-tauri": "success",
      "publish-cli": "success",
      "fix-updater-manifest": "success",
    },
    records,
    assets,
    release: { isDraft: true, isPrerelease: true },
    files,
    latest: { version: "2026.10.0", platforms },
  };
}

const without = (list, name) => list.filter((a) => a.name !== name);

describe("checkRelease", () => {
  it("passes a complete draft", () => {
    expect(checkRelease(complete())).toEqual([]);
  });

  it("fails naming a helper missing from the draft", () => {
    const input = complete();
    input.assets = without(input.assets, "seaquel-duckdb-aarch64-pc-windows-msvc.exe.gz");
    delete input.files["seaquel-duckdb-aarch64-pc-windows-msvc.exe.gz"];
    expect(checkRelease(input)).toEqual([
      "aarch64-pc-windows-msvc: seaquel-duckdb-aarch64-pc-windows-msvc.exe.gz is missing from the draft",
    ]);
  });

  it("fails when the draft's helper isn't the file the app was pinned to", () => {
    const input = complete();
    const name = helperAsset("x86_64-apple-darwin");
    input.files[name] = { ...input.files[name], sha256: "f".repeat(64) };
    expect(checkRelease(input)).toEqual([
      `x86_64-apple-darwin: ${name} in the draft has sha256 ${"f".repeat(64)}, the app was pinned to ${input.records["x86_64-apple-darwin"].sha256}`,
    ]);

    const sized = complete();
    sized.assets = sized.assets.map((a) => (a.name === name ? { ...a, size: a.size + 1 } : a));
    expect(checkRelease(sized)[0]).toMatch(
      /^x86_64-apple-darwin: .* bytes in the draft, the app was pinned to/,
    );
  });

  it("fails when an app isn't pinned, or its job left no record", () => {
    const input = complete();
    input.records["x86_64-unknown-linux-gnu"].app = {
      pinned: false,
      reason: "the app binary doesn't hold the pin",
    };
    delete input.records["aarch64-apple-darwin"].app;
    delete input.records["aarch64-unknown-linux-gnu"];
    expect(checkRelease(input)).toEqual(
      [
        "aarch64-apple-darwin: the app isn't pinned to its helper (not checked: the app's build didn't finish)",
        "aarch64-unknown-linux-gnu: no record of its release job (it failed or didn't finish)",
        "x86_64-unknown-linux-gnu: the app isn't pinned to its helper (the app binary doesn't hold the pin)",
      ].sort(
        (a, b) =>
          RELEASE_TARGETS.indexOf(a.split(":")[0]) - RELEASE_TARGETS.indexOf(b.split(":")[0]),
      ),
    );
  });

  it("fails when publish-cli (or any job before it) didn't succeed", () => {
    const input = complete();
    input.jobs["publish-cli"] = "skipped";
    expect(checkRelease(input)).toEqual(['job publish-cli ended "skipped", not "success"']);
  });

  it("fails when the CLI or TUI is missing", () => {
    const input = complete();
    input.assets = without(input.assets, "seaquel-tui-x86_64-pc-windows-msvc.exe");
    expect(checkRelease(input)).toEqual([
      "x86_64-pc-windows-msvc: seaquel-tui-x86_64-pc-windows-msvc.exe is missing from the draft",
    ]);
  });

  it("checks latest.json: every target, no other platform, real assets, the tag's version", () => {
    const input = complete();
    delete input.latest.platforms["linux-aarch64"];
    input.latest.platforms["linux-armv7"] = { signature: "s", url: "https://x/y.AppImage" };
    input.latest.platforms["darwin-aarch64"].url =
      "https://github.com/o/r/releases/download/v/Gone.tar.gz";
    input.latest.version = "26.10.0";
    expect(checkRelease(input)).toEqual([
      'latest.json\'s version is "26.10.0", not "2026.10.0"',
      "latest.json's darwin-aarch64 points at Gone.tar.gz, which isn't in the draft",
      "latest.json names platform linux-armv7, which no release job builds",
      "aarch64-unknown-linux-gnu: latest.json has no entry for it, so its app doesn't update",
    ]);
    expect(checkRelease({ ...complete(), latest: null })).toEqual([
      "latest.json is missing from the draft or isn't the updater's manifest",
    ]);
  });

  it("fails when the release isn't a pre-release (promoting it is the owner's later step)", () => {
    expect(
      checkRelease({ ...complete(), release: { isDraft: true, isPrerelease: false } }),
    ).toEqual([
      `${TAG} isn't a pre-release: promoting it is the step after the manual checks, not before this one`,
    ]);
  });

  it("fails when the draft can't be read at all", () => {
    const problems = checkRelease({ ...complete(), assets: null, files: {} });
    expect(problems[0]).toBe(`no draft release ${TAG} could be read`);
    expect(problems).toContain(
      "aarch64-apple-darwin: seaquel-duckdb-aarch64-apple-darwin.gz is missing from the draft",
    );
  });

  it("maps updater platform keys, installer suffixes included, to targets", () => {
    expect(targetOfPlatform("darwin-aarch64")).toBe("aarch64-apple-darwin");
    expect(targetOfPlatform("darwin-x86_64-app")).toBe("x86_64-apple-darwin");
    expect(targetOfPlatform("linux-x86_64-appimage")).toBe("x86_64-unknown-linux-gnu");
    expect(targetOfPlatform("windows-aarch64-nsis")).toBe("aarch64-pc-windows-msvc");
    expect(targetOfPlatform("linux-i686")).toBeNull();
    expect(targetOfPlatform("ios-aarch64")).toBeNull();
  });
});

describe("check-release.mjs on a downloaded draft", () => {
  let dir;
  const write = (path, data) => {
    mkdirSync(join(dir, path, ".."), { recursive: true });
    writeFileSync(join(dir, path), data);
  };
  const run = (...jobs) =>
    spawnSync(
      process.execPath,
      [
        resolve(import.meta.dirname, "check-release.mjs"),
        "--tag",
        TAG,
        "--dir",
        dir,
        ...jobs.flatMap((j) => ["--job", j]),
      ],
      { encoding: "utf8" },
    );

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "check-release-"));
    const draft = complete();
    for (const target of RELEASE_TARGETS) {
      write(join("pins", recordName(target)), JSON.stringify(draft.records[target]));
      write(join("download", helperAsset(target)), helperBytes(target));
    }
    write(
      "release.json",
      JSON.stringify({
        isDraft: true,
        isPrerelease: true,
        assets: draft.assets.map((a) => ({ ...a, url: "u", state: "uploaded" })),
      }),
    );
    write(join("download", "latest.json"), JSON.stringify(draft.latest));
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  it("passes a complete draft", () => {
    const r = run("publish-tauri=success", "publish-cli=success");
    expect(r.status, r.stdout + r.stderr).toBe(0);
    expect(r.stdout).toContain("is complete");
  });

  it("hashes the downloaded helpers: one re-gzipped after the pin fails, named", () => {
    write(
      join("download", helperAsset("aarch64-apple-darwin")),
      Buffer.from("gzipped helper for aarch64-apple-darwiN"),
    );
    const r = run("publish-tauri=success");
    expect(r.status).toBe(1);
    expect(r.stdout).toContain(
      "::error::aarch64-apple-darwin: seaquel-duckdb-aarch64-apple-darwin.gz in the draft has sha256",
    );
    expect(r.stderr).toContain("NOT ready to publish (1 problem)");
  });

  it("fails naming a helper the draft lacks", () => {
    rmSync(join(dir, "download", helperAsset("x86_64-pc-windows-msvc")));
    const assets = JSON.parse(readFileSync(join(dir, "release.json"), "utf8"));
    assets.assets = without(assets.assets, helperAsset("x86_64-pc-windows-msvc"));
    write("release.json", JSON.stringify(assets));
    const r = run();
    expect(r.status).toBe(1);
    expect(r.stdout).toContain(
      "::error::x86_64-pc-windows-msvc: seaquel-duckdb-x86_64-pc-windows-msvc.exe.gz is missing from the draft",
    );
  });

  it("fails on an empty folder (nothing downloaded is never a pass)", () => {
    rmSync(dir, { recursive: true, force: true });
    mkdirSync(dir);
    const r = run("publish-tauri=success", "publish-cli=success", "fix-updater-manifest=success");
    expect(r.status).toBe(1);
    expect(r.stdout).toContain(`::error::no draft release ${TAG} could be read`);
    expect(r.stdout).toContain("latest.json is missing");
  });
});

// The workflow, parsed.
describe("release.yml", () => {
  const workflow = parse(readFileSync(join(root, ".github/workflows/release.yml"), "utf8"));
  const steps = workflow.jobs["publish-tauri"].steps;
  const at = (name) => {
    const i = steps.findIndex((s) => s.name === name);
    if (i < 0) throw new Error(`publish-tauri has no step "${name}"`);
    return i;
  };
  /** What a step does, as text: its `run`, `uses` and `with`. */
  const text = (step) =>
    [step.run ?? "", step.uses ?? "", JSON.stringify(step.with ?? {})].join("\n");
  /** Whether a step builds, signs or gzips the DuckDB helper, whatever its flags. */
  const touchesHelper = (step) => {
    const t = text(step);
    const helper = /seaquel-duckdb/.test(t);
    return (
      /gzip/i.test(t) ||
      (helper &&
        /build-cli\.mjs|cargo\s+(build|rustc)|codesign|trusted-signing-cli|signtool/.test(t))
    );
  };

  it("builds exactly the targets check-release expects", () => {
    const targets = workflow.jobs["publish-tauri"].strategy.matrix.include.map((m) => m.target);
    expect(targets).toEqual(RELEASE_TARGETS);
  });

  it("pins the helper's .gz before the app's build, and uploads it after, unchanged", () => {
    const gzip = at("Gzip the signed DuckDB helper");
    const pin = at("Pin the DuckDB helper for the app's build");
    const app = at("build and publish");
    const verify = at("Check the app carries the pin");
    const upload = at("Upload the DuckDB helper to the draft");
    const order = [gzip, pin, app, verify, upload];
    expect(order).toEqual(order.toSorted((a, b) => a - b));
    expect(steps[pin].run).toContain(
      "node scripts/release-pin.mjs export --target ${{ matrix.target }}",
    );
    expect(steps[verify].run).toContain(
      "node scripts/release-pin.mjs verify-app --target ${{ matrix.target }}",
    );
    expect(steps[upload].uses).toMatch(/^actions\/github-script@/);
    expect(steps[upload].with.script).toContain("upload-helper-asset.mjs");
    expect(steps[upload].env.RELEASE_ID).toBe("${{ steps.tauri.outputs.releaseId }}");
    // Before the pin, the helper is built, signed and gzipped; after it,
    // nothing touches it.
    expect(
      steps
        .slice(0, pin)
        .filter(touchesHelper)
        .map((s) => s.name),
    ).toEqual([
      "Build seaquel-duckdb",
      "Sign macOS DuckDB helper",
      "Sign Windows DuckDB helper",
      "Gzip the signed DuckDB helper",
    ]);
    expect(
      steps
        .slice(pin + 1)
        .filter(touchesHelper)
        .map((s) => s.name ?? s.uses),
    ).toEqual([]);
  });

  // A copy downloaded in a
  // browser keeps the quarantine flag, so the three macOS binaries are
  // notarized. Notarizing doesn't change a file's bytes, but it runs before
  // the gzip and the pin all the same, so the pinned .gz is of a notarized
  // binary and nothing touches it after the pin.
  const APPLE_ENV = {
    APPLE_ID: "${{ secrets.APPLE_ID }}",
    APPLE_PASSWORD: "${{ secrets.APPLE_PASSWORD }}",
    APPLE_TEAM_ID: "${{ secrets.APPLE_TEAM_ID }}",
  };
  const notarizing = (step) => /notarize-macos\.sh|notarytool/.test(text(step));

  it("signs the three macOS binaries with a secure timestamp", () => {
    for (const name of ["Sign macOS DuckDB helper", "Sign macOS CLI", "Sign macOS TUI"]) {
      const run = steps[at(name)].run;
      expect(run, name).toMatch(/^codesign /);
      expect(run, name).toContain("--options runtime");
      expect(run, name).toContain("--timestamp");
    }
  });

  it("notarizes the helper after signing it and before its gzip and pin", () => {
    const sign = at("Sign macOS DuckDB helper");
    const notarize = at("Notarize macOS DuckDB helper");
    const gzip = at("Gzip the signed DuckDB helper");
    const pin = at("Pin the DuckDB helper for the app's build");
    expect([sign, notarize, gzip, pin]).toEqual(
      [sign, notarize, gzip, pin].toSorted((a, b) => a - b),
    );
    const step = steps[notarize];
    expect(step.if).toBe("runner.os == 'macOS'");
    expect(step.env).toEqual(APPLE_ENV);
    expect(step.run).toBe(
      "bash scripts/notarize-macos.sh src-tauri/binaries/seaquel-duckdb-${{ matrix.target }}",
    );
  });

  it("notarizes the CLI and the TUI after signing them and before uploading them", () => {
    const notarize = at("Notarize macOS CLI and TUI");
    const upload = (bin) => steps.findIndex((s) => s.with?.name === `${bin}-\${{ matrix.target }}`);
    for (const bin of ["seaquel-cli", "seaquel-tui"]) {
      const sign = at(`Sign macOS ${bin === "seaquel-cli" ? "CLI" : "TUI"}`);
      expect(sign, bin).toBeLessThan(notarize);
      expect(upload(bin), bin).toBeGreaterThan(notarize);
    }
    const step = steps[notarize];
    expect(step.if).toBe("runner.os == 'macOS'");
    expect(step.env).toEqual(APPLE_ENV);
    expect(step.run).toBe(
      "bash scripts/notarize-macos.sh src-tauri/binaries/seaquel-cli-${{ matrix.target }} src-tauri/binaries/seaquel-tui-${{ matrix.target }}",
    );
  });

  it("notarizes in exactly those two steps, through the shared script", () => {
    expect(steps.filter(notarizing).map((s) => s.name)).toEqual([
      "Notarize macOS DuckDB helper",
      "Notarize macOS CLI and TUI",
    ]);
    const pin = at("Pin the DuckDB helper for the app's build");
    expect(
      steps.slice(pin + 1).filter((s) => notarizing(s) && /seaquel-duckdb/.test(text(s))),
    ).toEqual([]);
  });

  it("requires the pin in the app's build, and creates the draft unchecked and as a pre-release", () => {
    const app = steps[at("build and publish")];
    expect(app.id).toBe("tauri");
    expect(app.uses).toMatch(/^tauri-apps\/tauri-action@/);
    expect(app.env.SEAQUEL_DUCKDB_HELPER_REQUIRE_PIN).toBe("1");
    expect(app.with.releaseName).toBe("NOT CHECKED: Release ${{ github.ref_name }}");
    expect(app.with.releaseDraft).toBe(true);
    expect(app.with.prerelease).toBe(true);
  });

  it("uses no gh in the matrix job (not promised on every runner image)", () => {
    expect(steps.filter((s) => /\bgh\s/.test(s.run ?? "")).map((s) => s.name)).toEqual([]);
  });

  it("records each target's pin even when its job fails", () => {
    const record = steps.find((s) => s.with?.name === "duckdb-pin-${{ matrix.target }}");
    expect(record.uses).toMatch(/^actions\/upload-artifact@/);
    expect(record.if).toBe("always()");
    expect(record.with.path).toBe("release-pins/duckdb-pin-${{ matrix.target }}.json");
  });

  it("leaves the helpers out of publish-cli", () => {
    expect(JSON.stringify(workflow.jobs["publish-cli"])).not.toContain("seaquel-duckdb");
  });

  it("runs check-release after every other job, whatever they ended with", () => {
    const job = workflow.jobs["check-release"];
    const others = Object.keys(workflow.jobs).filter((id) => id !== "check-release");
    const byName = (a, b) => a.localeCompare(b);
    expect(job.needs.toSorted(byName)).toEqual(others.toSorted(byName));
    expect(job.if).toBe("always()");
    const run = job.steps.find((s) => s.name === "Check the draft release").run;
    for (const need of others)
      expect(run).toContain(`--job "${need}=\${{ needs.${need}.result }}"`);
    expect(run).toContain("node scripts/check-release.mjs");
  });

  it("makes the title the gate: Release <tag> only on a pass", () => {
    const run = workflow.jobs["check-release"].steps.find(
      (s) => s.name === "Check the draft release",
    ).run;
    const pass = run.indexOf('--title "Release $TAG"');
    const failTitle = run.indexOf('--title "NOT READY (check-release failed): Release $TAG"');
    expect(pass).toBeGreaterThan(-1);
    expect(failTitle).toBeGreaterThan(pass);
    expect(run.slice(run.lastIndexOf('if [ "$status" -eq 0 ]', pass), pass)).toContain(
      'if [ "$status" -eq 0 ]',
    );
    expect(run).toContain("--json assets,isDraft,isPrerelease");
  });
});

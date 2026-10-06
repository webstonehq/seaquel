# Beta Update Channel Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let a desktop user opt into a Beta update channel that also offers published GitHub pre-releases, without changing what stable users get.

**Architecture:** A new Core setting `updateChannel` (`stable`/`beta`) picks the updater endpoint at run time in `src-tauri` (`updater_builder().endpoints(...)`). The website gets a beta route that serves `latest.json` of the newest published release by semver, pre-releases included. Betas are tagged `vX.Y.Z-beta.N` and never promoted.

**Tech Stack:** Rust (seaquel-workspace, src-tauri, tauri-plugin-updater 2.12), Svelte 5 + paraglide, SvelteKit on Cloudflare (website repo `../seaquel-app/main/packages/marketing`), vitest.

**Design:** `docs/plans/2026-10-05-beta-update-channel-design.md`.

**Git:** The user's rules forbid `git add`/`commit`. Each task ends with a checkpoint (tests green, nothing committed). Never commit.

---

### Task 1: `updateChannel` setting key (seaquel-workspace)

**Files:**
- Modify: `crates/seaquel-workspace/src/state.rs` (`SettingKey` enum ~383, `ALL` ~411, `as_str` ~423, `check_setting_set` ~485)
- Test: `crates/seaquel-workspace/tests/state_plan.rs` (`each_setting_value_is_checked` ~198)

**Step 1: Write the failing test.** In `each_setting_value_is_checked`, after the `skippedUpdateVersion` asserts, add:

```rust
    assert!(ok("updateChannel", "stable") && ok("updateChannel", "beta"));
    for v in ["", "Beta", "nightly", "beta ", "stable\0"] {
        assert!(!ok("updateChannel", v), "{v}");
    }
```

**Step 2: Run it.** `cargo test -p seaquel-workspace --test state_plan each_setting_value_is_checked`
Expected: FAIL (`"updateChannel" isn't a setting`).

**Step 3: Implement.**
- Add the variant after `SkippedUpdateVersion`:

```rust
    /// `stable` or `beta`: which feed the desktop updater asks. Unset is
    /// the app's own default (a pre-release build follows beta).
    #[serde(rename = "updateChannel")]
    UpdateChannel,
```

- `ALL`: size `9`, add `SettingKey::UpdateChannel` after `SkippedUpdateVersion`.
- `as_str`: `SettingKey::UpdateChannel => "updateChannel",`
- `check_setting_set` `match k`: `SettingKey::UpdateChannel => v == "stable" || v == "beta",` (next to `PendingChangesEnabled`).

**Step 4: Run.** `cargo test -p seaquel-workspace --test state_plan` and `cargo test -p seaquel-core --test state`
Expected: PASS. The round-trip and fuzz loops over `SettingKey::ALL` (~166, ~769) cover the new key automatically. The frozen parity fixtures (`tests/fixtures/state`, see its README) are not re-recorded: no existing case changes.

**Step 5: Regenerate TS types.** `npm run types:gen`, then `git diff --stat src/lib/types/generated/`
Expected: `SettingKey.ts` gains `"updateChannel"`. Nothing else changes.

**Checkpoint:** no commit.

---

### Task 2: Pure channel helpers (src-tauri)

**Files:**
- Create: `src-tauri/src/update_channel.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod update_channel;` with the other `mod` lines)

**Step 1: Write the module with its tests first** (the implementation stubs return wrong values so the tests fail):

```rust
//! The desktop updater's channel: which feed it asks, read from the
//! `updateChannel` setting (see `docs/plans/2026-10-05-beta-update-channel-design.md`).

use tauri::Url;

const STABLE_ENDPOINT: &str =
    "https://seaquel.app/updates/check/{{target}}/{{arch}}/{{current_version}}";
const BETA_ENDPOINT: &str =
    "https://seaquel.app/updates/check/beta/{{target}}/{{arch}}/{{current_version}}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdateChannel {
    Stable,
    Beta,
}

impl UpdateChannel {
    /// The stored setting, or the default for this build when it is unset
    /// or unreadable: a pre-release build (`2026.10.0-beta.2`) follows beta.
    pub(crate) fn resolve(stored: Option<&str>, version: &tauri::utils::config::semver::Version) -> Self {
        match stored {
            Some("beta") => UpdateChannel::Beta,
            Some("stable") => UpdateChannel::Stable,
            _ if !version.pre.is_empty() => UpdateChannel::Beta,
            _ => UpdateChannel::Stable,
        }
    }

    pub(crate) fn endpoint(self) -> Url {
        let s = match self {
            UpdateChannel::Stable => STABLE_ENDPOINT,
            UpdateChannel::Beta => BETA_ENDPOINT,
        };
        Url::parse(s).expect("the updater endpoints are valid URLs")
    }
}

/// Whether the downloaded update may be installed: only the version it was
/// downloaded as, and only on the channel that downloaded it.
pub(crate) fn pending_matches(
    pending: (UpdateChannel, &str),
    now: (UpdateChannel, &str),
) -> bool {
    pending == now
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::utils::config::semver::Version;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn the_stored_channel_wins() {
        assert_eq!(UpdateChannel::resolve(Some("beta"), &v("2026.9.2")), UpdateChannel::Beta);
        assert_eq!(UpdateChannel::resolve(Some("stable"), &v("2026.10.0-beta.1")), UpdateChannel::Stable);
    }

    #[test]
    fn unset_follows_the_build() {
        assert_eq!(UpdateChannel::resolve(None, &v("2026.9.2")), UpdateChannel::Stable);
        assert_eq!(UpdateChannel::resolve(None, &v("2026.10.0-beta.2")), UpdateChannel::Beta);
        // A value Core would refuse reads as unset.
        assert_eq!(UpdateChannel::resolve(Some("nightly"), &v("2026.9.2")), UpdateChannel::Stable);
    }

    #[test]
    fn each_channel_has_its_feed() {
        assert_eq!(UpdateChannel::Stable.endpoint().path(), "/updates/check/%7B%7Btarget%7D%7D/%7B%7Barch%7D%7D/%7B%7Bcurrent_version%7D%7D");
        assert!(UpdateChannel::Beta.endpoint().path().starts_with("/updates/check/beta/"));
    }

    #[test]
    fn the_stable_feed_matches_the_config() {
        let conf = include_str!("../tauri.conf.json");
        assert!(conf.contains(STABLE_ENDPOINT), "tauri.conf.json's updater endpoint changed");
    }

    #[test]
    fn a_pending_update_installs_only_as_downloaded() {
        let b = UpdateChannel::Beta;
        let s = UpdateChannel::Stable;
        assert!(pending_matches((b, "2026.10.0-beta.1"), (b, "2026.10.0-beta.1")));
        assert!(!pending_matches((b, "2026.10.0-beta.1"), (b, "2026.10.0-beta.2")));
        assert!(!pending_matches((b, "2026.10.0-beta.1"), (s, "2026.10.0-beta.1")));
    }
}
```

Note: if `tauri::utils::config::semver` isn't the path, use the type `app.package_info().version` has (`tauri::semver::Version` in some versions). Check with `grep -rn "pub use semver" ~/.cargo/registry/src/*/tauri-2*/src/lib.rs`. Don't add a `semver` dependency unless neither re-export exists.

**Step 2: Run.** `cargo test -p seaquel update_channel`
Expected: PASS. `the_stable_feed_matches_the_config` guards the duplicated URL. If the `path()` assertion's percent-encoding differs, assert on what `Url` actually produces; the updater replaces both raw and `%7B%7B…%7D%7D` forms (`tauri-plugin-updater-2.12.0/src/updater.rs:459-480`).

**Checkpoint:** no commit.

---

### Task 3: Use the channel in the updater (src-tauri)

**Files:**
- Modify: `src-tauri/src/lib.rs`: `PendingUpdate` (~165), `install_update` (~860), `check_for_update_command` (~902), `check_for_update` (~1232)

**Step 1: Channel lookup + updater builder.** Add next to `check_for_update`:

```rust
/// The channel the user chose (`updateChannel`), or this build's default
/// when it's unset or storage can't be read: an update check never fails
/// on storage.
async fn update_channel(app: &tauri::AppHandle) -> UpdateChannel {
    let core = app.state::<Core>();
    let desktop = app.state::<DesktopWorkspace>();
    let stored = match desktop.workspace(&core).await {
        Ok(ws) => ws.get_setting("updateChannel").await.ok().and_then(|s| s.value),
        Err(_) => None,
    };
    UpdateChannel::resolve(stored.as_deref(), &app.package_info().version)
}

fn channel_updater(
    app: &tauri::AppHandle,
    channel: UpdateChannel,
) -> tauri_plugin_updater::Result<tauri_plugin_updater::Updater> {
    app.updater_builder().endpoints(vec![channel.endpoint()])?.build()
}
```

Check: `Workspace::get_setting` is at `crates/seaquel-core/src/state.rs:1039` and returns `Result<Seqd<Option<String>>>`. Reading it at startup opens storage early. That's fine: `workspace()` keeps a lasting failure, and the GUI's first call reports it as before.

**Step 2: `PendingUpdate` holds what it downloaded.**

```rust
struct PendingUpdate {
    /// The downloaded installer, with the channel and version it came as.
    download: Mutex<Option<(UpdateChannel, String, Vec<u8>)>>,
}
```

Update its `Default`/`manage` site (`grep -n PendingUpdate src-tauri/src/lib.rs`).

**Step 3: `check_for_update`** (startup + background download): `let channel = update_channel(&app).await;`, use `channel_updater(&app, channel)?.check()`. Store `Some((channel, update.version.clone(), bytes))`.

**Step 4: `check_for_update_command`:** resolve the channel and check with `channel_updater`. Before checking, drop a pending download from another channel:

```rust
    let channel = update_channel(&app).await;
    if let Ok(mut d) = app.state::<PendingUpdate>().download.lock() {
        if d.as_ref().is_some_and(|(c, _, _)| *c != channel) {
            *d = None;
        }
    }
```

**Step 5: `install_update`:** take the download, re-check on the current channel, and install only when `update_channel::pending_matches((c, &v), (channel, &update.version))`. Otherwise return:

```rust
CommandError { message: "The downloaded update no longer matches the update channel. Check for updates again.".into(), code: "UPDATE_STALE".into() }
```

Keep the existing log lines and error mapping style.

**Step 6: Build and test.**
`cargo clippy -p seaquel --all-targets -- -D warnings` and `cargo test -p seaquel`
Expected: clean, PASS.

**Checkpoint:** no commit.

---

### Task 4: Update store follows the channel (frontend)

**Files:**
- Modify: `src/lib/stores/update.svelte.ts`
- Create: `src/lib/stores/update.svelte.test.ts`

The channel is not added to the state-replay `view` (`state-replay.svelte.test.ts:810`). That would change frozen fixtures.

**Step 1: Failing test.** Model the mocks on `src/lib/stores/license-nudge.svelte.test.ts` (how it stubs `getSettings`/Core):

```ts
import { describe, it, expect, vi, beforeEach } from "vitest";
// mock $lib/api/tauri: checkForUpdate resolves null
// mock settings as license-nudge.svelte.test.ts does; record settingSet calls

describe("UpdateStore channel", () => {
  it("defaults to null (the app decides) and saves a choice", async () => {
    const s = new UpdateStore();
    await s.initialize();
    expect(s.channel).toBeNull();
    await s.setChannel("beta");
    expect(s.channel).toBe("beta");
    // assert settingSet("updateChannel", "beta") was sent
  });

  it("switching clears a found or downloaded update and checks again", async () => {
    const s = new UpdateStore();
    s.setUpdateDownloaded({ version: "2026.10.0-beta.1" } as never);
    await s.setChannel("stable");
    expect(s.updateInfo).toBeNull();
    expect(s.isDownloaded).toBe(false);
    expect(checkForUpdate).toHaveBeenCalledOnce();
  });
});
```

**Step 2: Run.** `npx vitest run src/lib/stores/update.svelte.test.ts`
Expected: FAIL (`setChannel` missing).

**Step 3: Implement** in `UpdateStore`:

```ts
  channel = $state<"stable" | "beta" | null>(null);
  private readonly storedChannel = new StoredSetting("updateChannel", (v) => {
    this.channel = v === "stable" || v === "beta" ? v : null;
  });
```

- Register `storedChannel` in the constructor's `onStoredChange` handler next to `this.stored`.
- Load it in `initialize()`.
- Add the method:

```ts
  /** Saves the channel, forgets an update found on the old one and checks the new feed. */
  async setChannel(value: "stable" | "beta"): Promise<void> {
    this.channel = value;
    this.updateInfo = null;
    this.isDownloaded = false;
    try {
      await this.storedChannel.set(value);
      const { checkForUpdate } = await import("$lib/api/tauri");
      const info = await checkForUpdate();
      if (info) this.setUpdateAvailable(info);
    } catch (error) {
      console.error("Failed to switch update channel:", error);
    }
  }
```

**Step 3b: `UPDATE_STALE`.** `install_update` now fails with code `UPDATE_STALE` when the downloaded update no longer matches the channel. In `install()`'s catch, when `errorCode(error) === "UPDATE_STALE"` (`errorCode` from `$lib/core/client`, or however the Tauri invoke error carries `code`; check), clear `updateInfo`/`isDownloaded` and re-run the check, so the badge doesn't stay up over nothing. Add a test for it.

**Step 4: Run.** The test above plus `npx vitest run src/lib/hooks/database/state-replay.svelte.test.ts`
Expected: PASS, with replay unchanged.

**Checkpoint:** no commit.

---

### Task 5: Settings UI section

**Files:**
- Create: `src/lib/components/settings/general/updates-section.svelte`
- Modify: `src/lib/stores/settings-dialog.svelte.ts` (add `"updates"` to `SettingsSection`, `sectionToGroup` → `general`, and `groupSections.general` after `"app-info"`)
- Modify: `src/lib/components/settings/settings-tab-view.svelte` (nav item `...(isTauri() ? [{ id: "updates" as const, name: m.settings_updates(), icon: DownloadIcon }] : [])` after app-info, and `{#if isTauri() && shouldShowSection("updates")}<UpdatesSection />{/if}` after `<AppInfoSection>`)
- Modify: `messages/en.json`

**Step 1: Strings** (`messages/en.json`, near `settings_version`):

```json
  "settings_updates": "Updates",
  "settings_updates_description": "Choose which releases Seaquel updates to",
  "settings_update_channel": "Update channel",
  "settings_update_channel_stable": "Stable",
  "settings_update_channel_beta": "Beta",
  "settings_update_channel_beta_help": "Betas arrive before stable releases and may have bugs.",
  "settings_update_channel_stay": "You'll stay on {version} until a newer stable release is out.",
```

**Step 2: Component.** Follow `appearance/theme-section.svelte` exactly (header block, `flex items-center justify-between` row, `Select type="single"` with `SelectTrigger class="w-32"`). Logic:

```ts
  import { getVersion } from "@tauri-apps/api/app";
  import { updateStore } from "$lib/stores/update.svelte.js";

  let appVersion = $state("");
  onMount(async () => { appVersion = await getVersion(); });

  const isPrerelease = $derived(appVersion.includes("-"));
  const current = $derived(updateStore.channel ?? (isPrerelease ? "beta" : "stable"));
```

- `onValueChange={(v) => updateStore.setChannel(v as "stable" | "beta")}`.
- Under the row, when `current === "beta"`, show `m.settings_update_channel_beta_help()`.
- When `current === "stable" && isPrerelease`, show `m.settings_update_channel_stay({ version: appVersion })`.

Do not touch `src/lib/components/ui/*`.

**Step 3: Svelte autofixer.** Run the `svelte-autofixer` MCP tool on the new component until it reports nothing.

**Step 4: Check.** `npm run check`
Expected: 0 errors.

**Step 5: Translate.** Dispatch the `i18n-translator` agent for the new `en.json` keys.

**Checkpoint:** no commit.

---

### Task 6: Website beta feed (`../seaquel-app/main/packages/marketing`)

**Files:**
- Modify: `src/lib/server/releases.ts` (add the pure picker + shared fetch)
- Create: `src/lib/server/releases.test.ts`
- Modify: `src/routes/updates/check/[target]/[arch]/[current_version]/+server.ts`
- Create: `src/routes/updates/check/beta/[target]/[arch]/[current_version]/+server.ts`

**Step 1: Failing tests** (`releases.test.ts`):

```ts
import { describe, it, expect } from "vitest";
import { compareVersions, pickRelease } from "./releases";

const r = (tag: string, o: { draft?: boolean; prerelease?: boolean; latestJson?: boolean } = {}) => ({
  tag_name: tag, draft: !!o.draft, prerelease: !!o.prerelease,
  assets: o.latestJson === false ? [] : [{ name: "latest.json", browser_download_url: `u/${tag}`, size: 1 }],
});

describe("compareVersions", () => {
  it("orders calendar versions and betas by semver", () => {
    expect(compareVersions("2026.10.0-beta.3", "2026.10.0")).toBeLessThan(0);
    expect(compareVersions("2026.10.0-beta.10", "2026.10.0-beta.2")).toBeGreaterThan(0);
    expect(compareVersions("2026.10.0", "2026.9.2")).toBeGreaterThan(0);
  });
});

describe("pickRelease", () => {
  const list = [
    r("v2026.10.0-beta.2", { prerelease: true }),
    r("v2026.10.0-beta.3", { draft: true, prerelease: true }),
    r("v2026.9.3"),
    r("v2026.9.2"),
  ];
  it("stable ignores drafts and pre-releases", () => {
    expect(pickRelease(list, "stable")?.tag_name).toBe("v2026.9.3");
  });
  it("beta takes the newest published by version", () => {
    expect(pickRelease(list, "beta")?.tag_name).toBe("v2026.10.0-beta.2");
    expect(pickRelease([...list, r("v2026.10.0")], "beta")?.tag_name).toBe("v2026.10.0");
  });
  it("skips a release without latest.json", () => {
    expect(pickRelease([r("v2026.9.4", { latestJson: false }), ...list], "stable")?.tag_name).toBe("v2026.9.3");
  });
  it("skips tags it can't read", () => {
    expect(pickRelease([r("nightly"), ...list], "stable")?.tag_name).toBe("v2026.9.3");
  });
});
```

**Step 2: Run.** `npx vitest run src/lib/server/releases.test.ts`
Expected: FAIL (exports missing).

**Step 3: Implement** in `releases.ts` (no new dependency):

```ts
export type Channel = "stable" | "beta";

const VERSION = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/;

/** Semver order for `YYYY.M.P[-pre]` tags; NaN when either can't be read. */
export function compareVersions(a: string, b: string): number {
  const pa = VERSION.exec(a), pb = VERSION.exec(b);
  if (!pa || !pb) return NaN;
  for (let i = 1; i <= 3; i++) {
    const d = Number(pa[i]) - Number(pb[i]);
    if (d) return d;
  }
  if (!pa[4] || !pb[4]) return pa[4] ? -1 : pb[4] ? 1 : 0;
  const xa = pa[4].split("."), xb = pb[4].split(".");
  for (let i = 0; i < Math.max(xa.length, xb.length); i++) {
    if (xa[i] === undefined) return -1;
    if (xb[i] === undefined) return 1;
    const na = /^\d+$/.test(xa[i]), nb = /^\d+$/.test(xb[i]);
    if (na && nb) { const d = Number(xa[i]) - Number(xb[i]); if (d) return d; }
    else if (na !== nb) return na ? -1 : 1;
    else if (xa[i] !== xb[i]) return xa[i] < xb[i] ? -1 : 1;
  }
  return 0;
}

/** The release `channel` updates to: published, with `latest.json`, newest by version. */
export function pickRelease<R extends GitHubRelease>(releases: R[], channel: Channel): R | null {
  return releases
    .filter((r) => !r.draft && (channel === "beta" || !r.prerelease))
    .filter((r) => VERSION.test(r.tag_name))
    .filter((r) => r.assets?.some((a) => a.name === "latest.json"))
    .reduce<R | null>((best, r) => (!best || compareVersions(r.tag_name, best.tag_name) > 0 ? r : best), null);
}
```

Also export `GitHubRelease`. Then move the route's fetch/cache body into `export async function latestJsonFor(channel: Channel, platform: App.Platform | undefined): Promise<Response>`. It uses cache key `updates:latest-json` (stable, TTL 3600, unchanged) or `updates:latest-json:beta` (TTL 600), with `pickRelease(releases, channel)` in place of the `find`. Keep every existing log line and 204 fallback.

**Step 4: Routes.** Stable `+server.ts` → `export const GET = ({ params, platform }) => { console.log(...); return latestJsonFor("stable", platform); }`. The new beta route is the same with `"beta"`. SvelteKit prefers the static `beta` segment over `[target]`, and the depths differ anyway.

**Step 5: Run.** `npm test` and `npm run check` in `packages/marketing`
Expected: PASS, 0 errors.

**Step 6: Behaviour change to flag.** Stable now picks by version, not by GitHub list order. It also skips a release without `latest.json` instead of answering 204. Mention both to the user.

**Checkpoint:** no commit (website repo too).

---

### Task 7: Docs

**Files:**
- Modify: `CLAUDE.md` ("Releasing a New Version": new subsection after step 7)
- Modify: `src-tauri/CLAUDE.md` (updater paragraph)
- Modify: `src-tauri/CLAUDE.md` also: it says "Storage opens lazily on the first storage call (`DesktopWorkspace`)"; the startup update check now reads `updateChannel`, so storage opens at startup too. Correct it.
- Modify: `crates/seaquel-core/CLAUDE.md` (~line 45, the prose list of setting keys: add `updateChannel`)

**Step 1:** Add to root `CLAUDE.md`:

```markdown
### Beta releases

Tag a beta `vX.Y.Z-beta.N` (same files to bump, same workflow) and publish it as it is, a pre-release: never untick "pre-release". The app's Beta channel (`updateChannel`, Settings → General → Updates) asks `seaquel.app/updates/check/beta/…`, which serves the newest published release by version, pre-releases included; Stable sees only promoted ones. So step 6's "publish as a pre-release first" reaches Beta users at once. A pre-release build with no channel set follows Beta. The app never downgrades: a beta user who switches to Stable stays put until stable passes their version.
```

**Step 2:** In `src-tauri/CLAUDE.md`, near the updater/CSP notes:

```markdown
The updater's endpoint is chosen per check (`update_channel.rs`): `tauri.conf.json`'s `plugins.updater.endpoints` is the stable feed and must stay equal to `STABLE_ENDPOINT` (a test checks). Every check and `install_update` go through `channel_updater`; `PendingUpdate` keeps the channel and version it downloaded, and installs only if a re-check on the current channel returns the same.
```

**Checkpoint:** no commit.

---

### Task 8: Verification

1. `cargo test -p seaquel-workspace -p seaquel-core -p seaquel`, `cargo clippy -p seaquel --all-targets -- -D warnings`
2. `npm run check`, `npm test`
3. Website: `npm test`, `npm run check`
4. Manual (needs the website's beta route deployed first, or a temporary local edit of `BETA_ENDPOINT` to a dev URL that you revert afterwards):
   - Run `npm run tauri:dev`, open Settings → Updates, and switch to Beta. Expected: the check runs. With no newer pre-release, you see "up to date" or nothing, and there are no errors in the log viewer.
   - Switch back to Stable. With the dev app on a non-pre-release version, no note appears.
5. Release dry run (owner): tag a throwaway `v2026.10.0-beta.0`, publish it as a pre-release, install the current stable on a test machine, switch to Beta, update, then switch back to Stable and confirm the "stay on" note.

Report results faithfully, including anything skipped.

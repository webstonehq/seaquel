# Beta update channel

Seaquel has one release channel. This adds an opt-in Beta channel in the same
app, so the owner (and anyone who opts in) gets new builds before stable users.

## Decisions

- **Same app, opt-in channel.** No separate identifier or data dir. A beta's
  storage migrations run against the real `seaquel.db`.
- **No downgrades.** Switching Beta → Stable on a beta build stays on that build
  until stable passes it. Downgrading would run old code against a database
  newer migrations already touched.
- **Versions:** betas are `YYYY.month.patch-beta.N` (as `v2026.4.8-beta.1`
  already was). Semver puts `2026.10.0-beta.3 < 2026.10.0`, so beta users move
  onto the final release by themselves. Tags, `release.yml`, `check-release`
  and the helper pin are unchanged.

## Feeds

| GitHub release state   | Stable feed | Beta feed |
|------------------------|-------------|-----------|
| Draft                  | no          | no        |
| Published, pre-release | no          | yes       |
| Published, latest      | yes         | yes       |

Beta tags stay pre-releases forever. Stable tags are promoted as today. Step 6
of "Releasing" already publishes every release as a pre-release first, so that
window now reaches beta users before stable ones.

### Website (`seaquel-app`, `packages/marketing`)

- New route `/updates/check/beta/[target]/[arch]/[current_version]`: returns
  `latest.json` of the newest published release, pre-release or not, chosen
  by semver (not list order). Its own KV key, TTL about 10 minutes.
- The stable route is unchanged in behaviour.
- The "newest valid release" pick moves into `lib/server/releases.ts` and both
  routes share it. A release without `latest.json` is skipped, not answered
  with 204.

## App

### Setting

`SettingKey::UpdateChannel`, stored as `updateChannel`, value `"stable"` or
`"beta"` (anything else refused). Unset: a build whose version has a
pre-release suffix defaults to Beta, any other to Stable.

### Desktop (`src-tauri/src/lib.rs`)

- `channel_updater(app)` reads the channel through `DesktopWorkspace` and
  builds `app.updater_builder().endpoints([url]).build()`. The startup check,
  `check_for_update_command` and `install_update` all use it. The config's
  static endpoint stays as the stable fallback.
- `PendingUpdate` holds the downloaded version with the bytes. A channel
  switch clears it, and `install_update` installs only if its re-check returns
  that same version (today it installs the bytes under whatever the re-check
  returns).

### UI

Settings → General, an "Updates" row with a Stable/Beta select.

- To Beta: one line on what betas are, then an immediate check through the
  existing update badge.
- To Stable on a beta build: inline note "You'll stay on <version> until a
  newer stable release is out."
- `skippedUpdateVersion` works on both channels. New strings in `en.json`,
  translated with the i18n agent.

## Out of scope

A beta download page, beta CLI/TUI channels (they follow the app's version and
fetch the helper for `v<version>`, which works for beta tags), a separate beta
identifier.

## Testing

- Core: `updateChannel` validation.
- Desktop: pure helpers `default_channel(version)`, `endpoint_for(channel)`,
  `pending_matches(stored, checked)`.
- Website: release picker fixtures (drafts skipped, pre-releases only on beta,
  semver ordering incl. `2026.10.0` over `2026.10.0-beta.3`, missing
  `latest.json` skipped).
- Frontend: switching channel clears downloaded state and triggers a check.
- Manual: a throwaway `-beta.0` tag installed over a stable build; Beta → badge
  → install → back to Stable → note.

## Docs

- Root `CLAUDE.md` "Releasing": a "Beta releases" subsection (tag
  `vX.Y.Z-beta.N`, keep it a pre-release, never promote; step 6's window now
  reaches beta users).
- `src-tauri/CLAUDE.md`: the channel/updater rule.
- The website repo: the new route.

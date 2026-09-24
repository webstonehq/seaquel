# Unifying Cloud and Self-Hosted

**Date:** 2026-05-19
**Goal:** Cloud and self-hosted Seaquel run the same code path. Deployment location is the only difference.

## Summary

Today, "are we Cloud?" is a runtime branch (`isCloudTenant()` in `src/lib/server/cloud.ts:80`) keyed off `SEAQUEL_TENANT_ID`. It forks signup, the app-shell gate, the license UI, and the team API. This design deletes the branch. Both modes use one license-anchored path, talking to the same public control plane, with a persisted grace-period cache so self-hosted survives connectivity gaps.

## Decisions

| # | Decision | Choice |
|---|---|---|
| 1 | Access model | License-anchored everywhere |
| 2 | Offline policy | Cached grace period (persisted, two TTLs) |
| 3 | Install identity | License key is the anchor; first owner-key signup self-registers the install. No `isCloudTenant()` branch |
| 4 | Team joining | Every member pastes a license key from the subscription seat pool. No invite links |

## 1. Core inversion: delete the mode switch

- Remove `isCloudTenant()`. `SEAQUEL_TENANT_ID` is no longer a mode discriminator anywhere.
- Tenant identity derives from the **owner's license key → subscription**, resolved by the control plane. Both modes carry: `SEAQUEL_AUTH_SECRET` (session signing, self-generated) and `SEAQUEL_CONTROL_URL` (defaults to `https://seaquel.app`).
- `event.locals.tenant` is **always** populated when there's a session and a bound member — never `null` as a "self-hosted" signal. "No tenant yet" becomes a real state (install not yet self-registered), not a deployment mode.
- Cloud's only privilege: seaquel-app pre-seeds the owner's license key into first-run UI. Self-hosted users paste the same key. Identical code, identical UI, identical control-plane calls.

## 2. Control-plane contract

Credential model changes from per-tenant provisioned secret to license key.

### New endpoint

- **`POST /api/cloud/register-install`** — first owner-key signup. Body: owner license key + a locally-generated `installId` (random UUID, persisted in `auth.db`). Control plane resolves key → subscription, asserts owner-tier, records install↔subscription link, returns `TenantContext` (slug, tier, `seatLimit`, `currentPeriodEnd`, status). **Idempotent**: re-registering the same `installId`+subscription returns the existing record.

### Credential header

All `/api/cloud/*` calls send `X-Install-Id: <installId>` + `X-License-Key: <owner key>` instead of `X-Tenant-Auth: <provisioned secret>`. The control plane authorizes by resolving the key to the subscription that owns the install.

### Existing endpoints (shape unchanged, key off `installId`)

- `tenant-info`
- `verify-membership-license` — checks member's key against the install's owner subscription's seat pool
- `bind-member` / `unbind-member`
- `members`

`SEAQUEL_TENANT_ID` is replaced by the persisted `installId` — generated locally in both modes, never injected. seaquel-app's "provisioning" shrinks to: spin the container, pre-fill the owner key field.

## 3. Shared grace-period cache

Current in-memory 5-minute cache (`cloud.ts:68-69,93`) dies on restart — useless for self-hosted. Replace with persistence in `auth.db`.

### Schema

Extend `member_license` (or sibling `license_validation` table) with:
- `last_validated_at`
- `cached_status`, `cached_tier`, `cached_seat_info`
- `grace_until`

One row per bound member; owner's row also carries install/tenant-level fields.

### Two TTLs (env-overridable, identical defaults in both modes)

- **Soft TTL** (re-check interval, default ~24h) — the layout gate refreshes only when stale. Small in-memory layer (existing 5-min cache) on top to absorb request bursts.
- **Hard TTL** (grace window, default ~14 days) — every successful refresh sets `grace_until = now + hard_ttl`.

### Gate decision logic (shared, no deployment branch)

1. Soft TTL fresh → serve from cache, no network.
2. Soft TTL stale → try refresh.
   - Success → update cache, advance `grace_until`.
   - Failure (unreachable) → fall back to cache **if `now < grace_until`**; the app works normally.
3. `now ≥ grace_until` with no successful refresh → redirect to **`/revalidate`** (reuses the `/suspended` shell at `src/routes/(app)/suspended/`): read-only, "we couldn't confirm your license — reconnect or re-enter key." Distinct from `/suspended` (which is an explicit control-plane "suspended" status).

Cloud exercises only branches 1 and 2-success in practice; self-hosted gets the full ladder — same code, no branch on deployment.

## 4. File-by-file changes

### Control-plane client
- **Rename** `src/lib/server/cloud.ts` → `src/lib/server/licensing.ts` (the word "cloud" stops meaning anything).
- Delete `isCloudTenant()`, `cloudConfig()`'s tenant-id gate, `requireConfig()`.
- Add `registerInstall()`.
- Swap `X-Tenant-Auth` for `X-Install-Id` + `X-License-Key` (`cloud.ts:174-188`).
- Add persisted-cache read/write helpers backed by `auth.db`.

### Server-side gates
- **`src/hooks.server.ts:6,77-91`** — drop the `isCloudTenant()` branch. Always resolve install + member context via the soft-TTL cache (network only when stale). `event.locals.tenant` always set when bound.
- **`src/routes/(app)/+layout.server.ts:29-46`** — remove the `if (locals.tenant)` cloud-only wrapper; membership + grace ladder always runs. Add `/revalidate` redirect; keep `/suspended` for explicit suspension.

### Signup
- **`src/routes/api/signup/+server.ts:60-97`** — delete `cloudMode`. License key always required. First signup with an owner key calls `registerInstall()`; members hit `verifyMembershipLicense` against the install's seat pool. Rollback logic unchanged.
- **`src/routes/signup/+page.svelte` + `+page.server.ts`** — remove the `isCloud` derived; key field always renders. Cloud merely pre-fills it.

### UI
- **`src/lib/components/settings/general/license-section.svelte:75-87`** — delete the "self-hosted; no license attached" branch; tenant license card always renders in the web build.

### Team API
- **`src/routes/api/team/+server.ts`, `[containerUserId]/+server.ts:18`** — remove the `isCloudTenant()` 400 guard; team management always available.

### Types
- **`src/app.d.ts:13-14`** — update the `locals.tenant` doc (no longer "null when self-hosted").

### Persistence
- Add `install_id` (generated once at first boot) + the validation-cache columns to `auth.db` schema near `src/lib/server/member-license.ts` / `storage.ts`.

### Env
- Remove `SEAQUEL_TENANT_ID`.
- `SEAQUEL_CONTROL_URL` defaults to `https://seaquel.app`.
- Add optional `SEAQUEL_LICENSE_SOFT_TTL` and `SEAQUEL_LICENSE_GRACE_TTL`.

## 5. Migration, docs, testing

### Migrating existing Cloud tenants (no forced re-signup)

- **Control plane (seaquel-app):** backfill an install record for every existing tenant, reusing the current `tenantId` value as `installId`, linked to its subscription. Existing members keep working.
- **Container boot shim:** if a legacy `SEAQUEL_TENANT_ID` is present and no `install_id` row exists, adopt that value as `install_id` once, then never read the env again. One-time migration; deleted after a release or two.
- **Cache backfill:** on first boot post-upgrade, seed every existing `member_license` row with `last_validated_at = now`, `grace_until = now + grace_ttl` so the rollout never locks anyone out mid-flight.

### README / docs corrections

These fix discrepancies already present in `README.md`:

- Delete the `/api/tenant/status → needsBootstrap` line (`README.md:170`) — that route never existed.
- Rewrite `README.md:98-100`: drop the "first user = Owner, subsequent need invite links" claim. Real model: owner signs up with an owner license key; teammates paste member keys from the subscription's seat pool. No invite links.
- Env table (`README.md:114-127`): a license key is now required for self-hosted too; document `SEAQUEL_CONTROL_URL` default and the two TTL vars; remove any `SEAQUEL_TENANT_ID` mention.
- Mirror env changes into `deploy/docker/.env.example`.

### Testing (TDD, per project workflow)

- **Unit:** cache ladder (all 4 branches), `registerInstall()` idempotency, seat-limit enforcement.
- **Integration:** owner-key signup → register-install; member-key → bind/reject; gate redirects (`/login`, `/signup?reason=membership`, `/suspended`, `/revalidate`).
- **e2e:** update the Docker local-test recipe in the README to the new env/flow.

## Cross-repo dependency

`register-install`, license-as-credential, and seat-pool resolution are **server-side endpoints in seaquel-app** (`/Users/m/projects/github/webstonehq/seaquel-app/main`). This is a coordinated two-repo change; the seaquel side cannot ship until the control plane supports the new contract. Implementation plans need to sequence the two repos, with seaquel-app changes landing first behind a feature flag so the seaquel container can verify against staging before the public contract switches.

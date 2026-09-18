<script lang="ts">
  /**
   * Settings → General → Offline license bundle.
   *
   * Compact summary section shown in the in-app settings tab. Mirrors the
   * client-fetch pattern used by `team-section.svelte` — `/api/airgap/bundle`
   * GET tells us whether a bundle is installed and what it covers. The
   * full management UI (replace / remove) lives at `/settings/airgap`,
   * which this section links to via a button.
   *
   * Web-build only. The Tauri desktop build doesn't expose an airgap
   * server, and the desktop license model is a per-user activation key —
   * the operator-level bundle concept doesn't apply.
   */
  import { onMount } from "svelte";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import ArrowRightIcon from "@lucide/svelte/icons/arrow-right";

  interface BundleStatus {
    present: boolean;
    tier?: string;
    seats?: number;
    notAfter?: number;
    issuedAt?: number;
    importedAt?: number;
    pubkeyFingerprint?: string;
    payloadSha256?: string;
    revokedKeyCount?: number;
    expired?: boolean;
  }

  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let status = $state<BundleStatus>({ present: false });
  // Mode is sourced from /api/account/tenant in this app, but for an
  // initial summary the bundle presence alone is a reliable enough
  // signal: bundle present ↔ install is in airgap mode. The full
  // /settings/airgap page reads the authoritative install_cache.mode.
  const mode = $derived<"airgap" | "online">(
    status.present ? "airgap" : "online",
  );

  async function refresh() {
    try {
      const res = await fetch("/api/airgap/bundle");
      if (!res.ok) {
        // 401/403 mean the caller isn't a bound member — surface the
        // not-applicable case quietly rather than as a hard error.
        if (res.status === 401 || res.status === 403) {
          status = { present: false };
          loadError = null;
          loading = false;
          return;
        }
        loadError = `Couldn't load bundle status (${res.status})`;
        loading = false;
        return;
      }
      const body = (await res.json()) as BundleStatus;
      status = body;
      loadError = null;
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  onMount(refresh);

  function formatExpiry(unixSeconds: number | undefined): string {
    if (!unixSeconds) return "—";
    try {
      return new Date(unixSeconds * 1000).toLocaleDateString();
    } catch {
      return String(unixSeconds);
    }
  }

  function shortFingerprint(fp: string | undefined): string {
    if (!fp) return "";
    return fp.length > 8 ? `${fp.slice(0, 8)}…` : fp;
  }
</script>

<div class="space-y-4" data-section="airgap">
  <div class="flex items-start justify-between gap-4">
    <div>
      <h3 class="text-lg font-medium">Offline license bundle</h3>
      <p class="text-muted-foreground text-sm">
        Run this install without calling the seaquel.app license server.
      </p>
    </div>
    <Badge variant={mode === "airgap" ? "default" : "secondary"}>
      {mode === "airgap" ? "AIRGAP" : "ONLINE"}
    </Badge>
  </div>

  {#if loading}
    <p class="text-muted-foreground text-sm">Loading…</p>
  {:else if loadError}
    <p class="text-destructive text-sm">{loadError}</p>
  {:else if status.present}
    <div class="rounded-lg border p-4 text-sm">
      <div class="grid grid-cols-[140px_1fr] gap-2">
        <span class="text-muted-foreground">Tier</span>
        <span class="font-medium">{status.tier}</span>
      </div>
      <div class="grid grid-cols-[140px_1fr] gap-2">
        <span class="text-muted-foreground">Seats</span>
        <span>{status.seats}</span>
      </div>
      <div class="grid grid-cols-[140px_1fr] gap-2">
        <span class="text-muted-foreground">Expires</span>
        <span>
          {formatExpiry(status.notAfter)}
          {#if status.expired}
            <span class="text-destructive ml-1">(expired)</span>
          {/if}
        </span>
      </div>
      <div class="grid grid-cols-[140px_1fr] gap-2">
        <span class="text-muted-foreground">Signing key</span>
        <span class="font-mono">
          {shortFingerprint(status.pubkeyFingerprint)}
        </span>
      </div>
    </div>
  {:else}
    <p class="text-muted-foreground text-sm">
      No bundle imported. Validation goes through the seaquel.app control
      plane.
    </p>
  {/if}

  <div>
    <Button variant="outline" size="sm" href="/settings/airgap">
      Open offline bundle settings
      <ArrowRightIcon class="ml-1 size-3.5" />
    </Button>
  </div>
</div>

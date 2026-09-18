/**
 * Web-side equivalent of `licenseStore` for the per-tenant cloud build.
 *
 * Sourced from the control plane (via the local `/api/team` proxy,
 * which already returns the calling user's membership row). Read-only:
 * Activate/Deactivate are not user-actionable on the web — the license
 * is implicit from tenant membership and changes via Dodo billing.
 *
 * Used by `license-section.svelte` when `isWeb()`. Tauri continues to
 * use the keychain-backed `licenseStore` instead.
 */

interface TenantLicenseState {
  loaded: boolean;
  loadError: string | null;
  tier: "individual" | "business" | "personal" | null;
  status: "active" | "suspended" | "provisioning" | "failed" | "deleting" | null;
  ownerEmail: string;
  maskedLicenseKey: string;
  role: "owner" | "member" | null;
  currentPeriodEnd: Date | null;
  seatLimit: number;
  memberCount: number;
}

function emptyState(): TenantLicenseState {
  return {
    loaded: false,
    loadError: null,
    tier: null,
    status: null,
    ownerEmail: "",
    maskedLicenseKey: "",
    role: null,
    currentPeriodEnd: null,
    seatLimit: 0,
    memberCount: 0,
  };
}

let state = $state<TenantLicenseState>(emptyState());

export const tenantLicenseStore = {
  get loaded() {
    return state.loaded;
  },
  get loadError() {
    return state.loadError;
  },
  get tier() {
    return state.tier;
  },
  get status() {
    return state.status;
  },
  get ownerEmail() {
    return state.ownerEmail;
  },
  get maskedLicenseKey() {
    return state.maskedLicenseKey;
  },
  get role() {
    return state.role;
  },
  get currentPeriodEnd() {
    return state.currentPeriodEnd;
  },
  get seatLimit() {
    return state.seatLimit;
  },
  get memberCount() {
    return state.memberCount;
  },

  async refresh(): Promise<void> {
    try {
      const [teamRes, infoRes] = await Promise.all([
        fetch("/api/team"),
        fetch("/api/account/tenant"),
      ]);
      if (!teamRes.ok || !infoRes.ok) {
        state = {
          ...emptyState(),
          loaded: true,
          loadError: `Couldn't load license (${teamRes.status}/${infoRes.status})`,
        };
        return;
      }
      const team = (await teamRes.json()) as {
        members: Array<{
          containerUserId: string;
          role: "owner" | "member";
          email: string;
          maskedLicenseKey: string;
        }>;
        currentUserRole: "owner" | "member";
      };
      const info = (await infoRes.json()) as {
        tier: TenantLicenseState["tier"];
        status: TenantLicenseState["status"];
        ownerEmail: string;
        seatLimit: number;
        currentPeriodEnd: string | null;
      };
      const me = team.members.find((m) => m.role === team.currentUserRole);
      state = {
        loaded: true,
        loadError: null,
        tier: info.tier,
        status: info.status,
        ownerEmail: info.ownerEmail,
        maskedLicenseKey: me?.maskedLicenseKey ?? "",
        role: team.currentUserRole,
        currentPeriodEnd: info.currentPeriodEnd ? new Date(info.currentPeriodEnd) : null,
        seatLimit: info.seatLimit,
        memberCount: team.members.length,
      };
    } catch (e) {
      state = {
        ...emptyState(),
        loaded: true,
        loadError: e instanceof Error ? e.message : String(e),
      };
    }
  },
};

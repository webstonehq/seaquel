import { hostname } from "@tauri-apps/plugin-os";
import { getKeyringService } from "$lib/services/keyring";
import { activateLicense, validateLicense, deactivateLicense, getUsername } from "$lib/api/tauri";
import type { LicenseResponse } from "$lib/api/tauri";
import { getDatabase, licenseRepo } from "$lib/storage";

export type LicenseStatus = "personal" | "active" | "expired" | "invalid";
export type LicenseTier = "personal" | "individual" | "business";

interface PersistedLicenseState {
  status: LicenseStatus;
  tier: LicenseTier;
  maskedKey: string | null;
  instanceId: string | null;
  customerName: string | null;
  expiresAt: string | null;
  lastValidatedAt: string | null;
}

const REVALIDATION_INTERVAL_MS = 12 * 60 * 60 * 1000; // 12 hours
const RETRY_INTERVAL_MS = 60 * 60 * 1000; // 1 hour

const TIER_LABELS: Record<LicenseTier, string> = {
  personal: "Personal Use",
  individual: "Commercial - Individual",
  business: "Commercial - Business",
};

const STATUS_LABELS: Partial<Record<LicenseStatus, string>> = {
  expired: "License Expired",
  invalid: "License Invalid",
};

class LicenseStore {
  status = $state<LicenseStatus>("personal");
  tier = $state<LicenseTier>("personal");
  maskedKey = $state<string | null>(null);
  instanceId = $state<string | null>(null);
  customerName = $state<string | null>(null);
  expiresAt = $state<string | null>(null);
  lastValidatedAt = $state<string | null>(null);
  isActivating = $state(false);
  activationError = $state<string | null>(null);

  private initialized = false;
  private revalidationTimer: ReturnType<typeof setTimeout> | null = null;

  get badgeLabel(): string {
    // Show status label for expired/invalid, otherwise show tier label
    if (this.status === "expired" || this.status === "invalid") {
      return STATUS_LABELS[this.status] ?? TIER_LABELS[this.tier];
    }
    return TIER_LABELS[this.tier];
  }

  async initialize(): Promise<void> {
    if (this.initialized) return;

    try {
      const db = await getDatabase();
      const persisted = (await licenseRepo.load(db)) as PersistedLicenseState | null;

      if (persisted) {
        this.status = persisted.status;
        this.tier = persisted.tier;
        this.maskedKey = persisted.maskedKey;
        this.instanceId = persisted.instanceId;
        this.customerName = persisted.customerName;
        this.expiresAt = persisted.expiresAt;
        this.lastValidatedAt = persisted.lastValidatedAt;
      }

      this.initialized = true;

      // Schedule background revalidation if we have an active license
      if (this.status === "active" && this.instanceId) {
        this.scheduleNextRevalidation();
      }
    } catch (error) {
      console.error("Failed to load license state:", error);
      this.initialized = true;
    }
  }

  async activate(key: string): Promise<boolean> {
    this.isActivating = true;
    this.activationError = null;

    try {
      const [host, user] = await Promise.all([hostname(), getUsername()]);
      const instanceName = `${host ?? "unknown"}__${user}`;
      const response = await activateLicense(key, instanceName);

      this.applyResponse(response, key);
      await this.storeKey(key);
      await this.persist();
      this.scheduleNextRevalidation();

      return true;
    } catch (error: unknown) {
      const err = error as Record<string, unknown>;
      // oxlint-disable-next-line typescript/no-base-to-string
      const message = err?.message ? String(err.message) : String(error);
      this.activationError = message;
      return false;
    } finally {
      this.isActivating = false;
    }
  }

  async deactivate(): Promise<boolean> {
    try {
      const key = await this.loadKey();
      if (key && this.instanceId) {
        await deactivateLicense(key, this.instanceId);
      }
    } catch (error) {
      console.warn("Failed to deactivate on server:", error);
    }

    // Always clear local state regardless of server response
    this.clearRevalidationTimer();
    this.status = "personal";
    this.tier = "personal";
    this.maskedKey = null;
    this.instanceId = null;
    this.customerName = null;
    this.expiresAt = null;
    this.lastValidatedAt = null;

    await this.clearKey();
    await this.persist();

    return true;
  }

  async revalidateInBackground(): Promise<void> {
    try {
      const key = await this.loadKey();
      if (!key || !this.instanceId) return;

      const response = await validateLicense(key, this.instanceId);

      if (response.status === "active") {
        this.applyResponse(response, key);
        await this.persist();
        this.scheduleNextRevalidation();
      } else {
        // License no longer active — mark as invalid, preserving license info
        await this.markAsInvalid(response.status === "expired" ? "expired" : "invalid");
      }
    } catch (error) {
      console.warn("License revalidation failed, will retry:", error);
      // Keep cached status, retry in 1 hour
      this.scheduleRevalidationIn(RETRY_INTERVAL_MS);
    }
  }

  private applyResponse(response: LicenseResponse, key: string): void {
    this.status = response.status === "active" ? "active" : "expired";
    // Only update tier if response has a valid commercial tier, otherwise keep existing
    const validCommercialTiers: LicenseTier[] = ["individual", "business"];
    if (validCommercialTiers.includes(response.tier as LicenseTier)) {
      this.tier = response.tier as LicenseTier;
    } else if (this.tier === "personal") {
      // If we don't have a commercial tier stored, default to individual
      this.tier = "individual";
    }
    // Keep existing tier for all other cases (tier already set to individual/business)
    this.maskedKey = maskKey(key);
    this.instanceId = response.instance_id ?? this.instanceId;
    this.expiresAt = response.expires_at;
    this.lastValidatedAt = new Date().toISOString();
  }

  private async revertToPersonal(): Promise<void> {
    this.clearRevalidationTimer();
    this.status = "personal";
    this.tier = "personal";
    this.maskedKey = null;
    this.instanceId = null;
    this.customerName = null;
    this.expiresAt = null;
    this.lastValidatedAt = null;

    await this.clearKey();
    await this.persist();
  }

  private async markAsInvalid(status: "expired" | "invalid"): Promise<void> {
    this.clearRevalidationTimer();
    this.status = status;
    // Keep tier, maskedKey, instanceId, expiresAt intact so user sees what happened
    // Keep the key in keyring to allow retry
    await this.persist();
  }

  async retryValidation(): Promise<boolean> {
    if (this.status !== "expired" && this.status !== "invalid") {
      return false;
    }

    try {
      const key = await this.loadKey();
      if (!key || !this.instanceId) return false;

      const response = await validateLicense(key, this.instanceId);

      if (response.status === "active") {
        this.applyResponse(response, key);
        await this.persist();
        this.scheduleNextRevalidation();
        return true;
      } else {
        await this.markAsInvalid(response.status === "expired" ? "expired" : "invalid");
        return false;
      }
    } catch (error) {
      console.warn("License retry validation failed:", error);
      return false;
    }
  }

  private scheduleNextRevalidation(): void {
    this.clearRevalidationTimer();

    if (!this.lastValidatedAt) {
      void this.revalidateInBackground();
      return;
    }

    const lastValidated = new Date(this.lastValidatedAt).getTime();
    const nextValidation = lastValidated + REVALIDATION_INTERVAL_MS;
    const msRemaining = nextValidation - Date.now();

    if (msRemaining <= 0) {
      void this.revalidateInBackground();
    } else {
      this.scheduleRevalidationIn(msRemaining);
    }
  }

  private scheduleRevalidationIn(ms: number): void {
    this.clearRevalidationTimer();
    this.revalidationTimer = setTimeout(() => {
      void this.revalidateInBackground();
    }, ms);
  }

  private clearRevalidationTimer(): void {
    if (this.revalidationTimer) {
      clearTimeout(this.revalidationTimer);
      this.revalidationTimer = null;
    }
  }

  private async storeKey(key: string): Promise<void> {
    try {
      const keyring = getKeyringService();
      await keyring.setLicenseKey(key);
    } catch (error) {
      console.error("Failed to store license key in keyring:", error);
    }
  }

  private async loadKey(): Promise<string | null> {
    try {
      const keyring = getKeyringService();
      return await keyring.getLicenseKey();
    } catch (error) {
      console.error("Failed to load license key from keyring:", error);
      return null;
    }
  }

  private async clearKey(): Promise<void> {
    try {
      const keyring = getKeyringService();
      await keyring.deleteLicenseKey();
    } catch (error) {
      console.error("Failed to clear license key from keyring:", error);
    }
  }

  private async persist(): Promise<void> {
    try {
      const db = await getDatabase();
      const state: PersistedLicenseState = {
        status: this.status,
        tier: this.tier,
        maskedKey: this.maskedKey,
        instanceId: this.instanceId,
        customerName: this.customerName,
        expiresAt: this.expiresAt,
        lastValidatedAt: this.lastValidatedAt,
      };
      await licenseRepo.save(db, state);
    } catch (error) {
      console.error("Failed to persist license state:", error);
    }
  }
}

function maskKey(key: string): string {
  if (key.length <= 8) return "****";
  return key.slice(0, 4) + "****" + key.slice(-4);
}

export const licenseStore = new LicenseStore();

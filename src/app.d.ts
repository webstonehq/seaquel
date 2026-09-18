// See https://svelte.dev/docs/kit/types#app.d.ts for more info
import type { User, Session } from "$lib/server/session-types";
import type { TenantContext, LicenseState } from "$lib/server/licensing";

declare global {
  namespace App {
    interface Locals {
      /** Authenticated user for this request, or null if unauthenticated. */
      user: User | null;
      /** Active session, or null if unauthenticated. */
      session: Session | null;
      /**
       * Tenant context resolved from install_cache + control plane.
       * Null only when no owner has registered an install yet, or when
       * grace has expired and the control plane is unreachable.
       * Populated in `hooks.server.ts` for every request.
       */
      tenant: TenantContext | null;
      /** Coarse state of the licensing gate for this request. */
      licenseState: LicenseState["kind"];
    }
  }
}

export {};

/**
 * Surfaces the install/tenant context (if any) to the signup form so
 * the UI can render "Join {tenant.slug}" copy. Pre-registration (no
 * install_cache row yet) returns tenant: null and the form runs the
 * first-owner self-register flow on submit.
 */
import type { PageServerLoad } from "./$types";

// The root layout sets `prerender = true` for the desktop adapter-static
// build. Override here: install_cache is read per-request via
// locals.tenant, so a prerendered /signup would bake in a stale
// `tenant: null` snapshot and the form would render the wrong copy
// after the install has been registered. Desktop's adapter-static is
// configured with `fallback: "index.html"`, so non-prerendered routes
// still resolve via the SPA shell.
export const prerender = false;

export const load: PageServerLoad = ({ locals }) => {
  return {
    tenant: locals.tenant
      ? {
          slug: locals.tenant.slug,
          status: locals.tenant.status,
          ownerEmail: locals.tenant.ownerEmail,
        }
      : null,
  };
};

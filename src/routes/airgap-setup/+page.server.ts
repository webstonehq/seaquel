/**
 * `/airgap-setup` is web-only. Override the root layout's prerender flag
 * the same way `/signup` does — otherwise the desktop adapter-static
 * build would bake in a snapshot of the page that we don't need (Tauri
 * never visits this route), and worse, the web build would prerender it
 * and skip server-side logic on subsequent visits.
 *
 * No data is loaded server-side: the page is intentionally usable when
 * `locals.user` is null (fresh install, bundle not yet imported, no
 * session). All state — file selection, upload result — is client-side.
 */
import type { PageServerLoad } from "./$types";

export const prerender = false;

export const load: PageServerLoad = () => {
  return {};
};

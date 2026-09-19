-- Track whether this install is currently operating in 'online' or 'airgap' mode.
--
-- 'online' (default) means the install talks to the control plane directly;
-- 'airgap' means it relies on a signed bundle uploaded via /api/airgap/bundle.
-- Set by `/api/airgap/bundle` POST (→ 'airgap'), DELETE (→ 'online'), and by
-- online `registerInstall` success (→ 'online'). Read by `/revalidate` to
-- show mode-aware copy.
alter table "install_cache" add column "mode" text not null default 'online';

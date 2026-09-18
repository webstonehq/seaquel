-- Singleton holder for the most recently imported air-gap bundle.
--
-- In air-gap mode the operator uploads a signed JSON envelope from the
-- control plane. We persist the full raw envelope (BLOB) so it can be
-- re-verified on every boot against the embedded public key, plus a hot
-- JSON projection of the verified `BundlePayload` for cheap reads by the
-- licensing dispatcher and the /settings/airgap UI.
--
-- One row at most — the `CHECK (id = 1)` constraint enforces singleton
-- semantics. `imported_at`, `not_after`, and `issued_at` are unix seconds.
create table if not exists "airgap_bundle" (
  "id"                 integer not null primary key check ("id" = 1),
  "raw_envelope"       blob    not null,
  "verified_payload"   text    not null,
  "pubkey_fingerprint" text    not null,
  "imported_at"        integer not null,
  "not_after"          integer not null,
  "payload_sha256"     text    not null,
  "issued_at"          integer not null
);

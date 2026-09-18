-- Mark when a member's license was revoked by a bundle import.
--
-- In air-gap mode, every uploaded bundle carries a `revocations` list of
-- license keys that have been refunded/canceled upstream. The bundle
-- import handler walks `member_license` and stamps `revoked_at` (unix
-- seconds) on every matching row. The hooks gate (Task 7) treats any
-- non-NULL `revoked_at` as an immediate hard-block — no grace window.
--
-- The index supports `WHERE revoked_at IS NOT NULL` predicates used by
-- the hooks gate and the membership listing UI.
alter table "member_license" add column "revoked_at" integer;
create index "member_license_revoked_idx" on "member_license" ("revoked_at");

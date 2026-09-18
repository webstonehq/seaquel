-- Install identity + cached cloud/license validation state.
--
-- Supports the unified cloud/self-hosted code path: every install has a
-- stable `install_id` minted once at first boot (never injected from env),
-- and the most recent successful control-plane validation is cached locally
-- so the app can keep running offline until `grace_until`.
--
-- `install` and `install_cache` are singleton tables — the `CHECK (id = 1)`
-- constraint enforces a single row each. Per-member validation state is
-- attached to the existing `member_license` table via additive columns so
-- older code paths reading the original columns keep working during rollout.

-- One row per install. Generated once at first boot, never injected from env.
create table if not exists "install" (
  "id"         integer not null primary key check ("id" = 1), -- singleton row
  "install_id" text not null unique,
  "created_at" integer not null
);

-- Cached install/tenant context. One singleton row alongside `install`.
create table if not exists "install_cache" (
  "id"                 integer not null primary key check ("id" = 1),
  "tenant_id"          text,
  "slug"               text,
  "status"             text,             -- provisioning | active | suspended | failed | deleting
  "tier"               text,
  "seat_limit"         integer,
  "current_period_end" text,             -- ISO8601 or NULL
  "last_validated_at"  integer not null, -- unix seconds
  "grace_until"        integer not null  -- unix seconds; cache valid offline until this
);

-- Per-member validation cache. Extends the existing member_license model
-- without altering the original columns so reads from older code paths
-- keep working during rollout.
alter table "member_license" add column "last_validated_at" integer;
alter table "member_license" add column "cached_status"     text;
alter table "member_license" add column "cached_tier"       text;
alter table "member_license" add column "grace_until"       integer;

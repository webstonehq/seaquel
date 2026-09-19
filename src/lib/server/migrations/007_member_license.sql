-- Local mirror of "this user used this license to join this tenant".
-- Authoritative copy lives on the control plane in `tenant_members`;
-- the local row exists so the (app) layout guard can answer "is the
-- current Better Auth user a paid member?" without a network round-trip
-- on every request.
--
-- One row per Better Auth user (containers are single-tenant — the
-- "tenant" axis is implicit). `license_key` is UNIQUE so the same key
-- can't be re-bound to a different local user without an explicit
-- unbind first. `control_member_id` is the back-reference to the
-- `tenant_members.id` on seaquel-app, returned by `/api/cloud/bind-member`.
create table "member_license" (
  "user_id"           text not null primary key references "user" ("id") on delete cascade,
  "license_key"       text not null unique,
  "bound_at"          integer not null,
  "control_member_id" text not null
);

create index "member_license_license_key_idx" on "member_license" ("license_key");

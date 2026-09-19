-- Mark which local `member_license` row belongs to the install's owner.
--
-- The control plane authenticates `/api/cloud/*` calls with the owner's
-- license key (sent as `X-License-Key`). To locate that key without a
-- network round-trip we flag the owner's row here. Task 15 sets this to
-- 1 for the first user who signs up with an owner-tier key; for everyone
-- else it stays at the default 0.
alter table "member_license" add column "is_owner" integer not null default 0;

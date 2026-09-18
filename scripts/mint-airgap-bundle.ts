/**
 * Mint a signed air-gapped license bundle for local self-hosted testing.
 *
 * Normally bundles are minted by the seaquel.app control plane and downloaded
 * from the owner's dashboard. This script lets you produce the same artifact
 * locally with a dev keypair, so you can exercise the offline flow without
 * standing up the control plane.
 *
 * Usage:
 *
 *   node scripts/mint-airgap-bundle.ts \
 *     --owner-key=DEV-OWNER-KEY-001 \
 *     --member-key=DEV-MEMBER-KEY-001 \
 *     --out=/tmp/test.bundle
 *
 * Required for the receiving container:
 *
 *   docker run \
 *     -e SEAQUEL_BUNDLE_TRUSTED_PUBKEY=<fingerprint>:<pubkey-hex> \
 *     ...
 *
 * The script prints the exact env-var line to stderr after minting.
 *
 * Options:
 *   --seed-hex=<64-hex>       Use a specific Ed25519 seed (32 bytes). Lets you
 *                             pin SEAQUEL_BUNDLE_TRUSTED_PUBKEY ahead of time
 *                             and mint many bundles with the same trust anchor.
 *                             Omit to generate a fresh random seed (printed on
 *                             stderr so you can record it).
 *   --owner-key=<str>         License key for the bundle's owner seat.
 *                             Default: "DEV-OWNER-KEY-001".
 *   --member-key=<str>        Optional license key for one member seat. Pass
 *                             multiple times for multiple members.
 *   --revoked-keys=<csv>      Comma-separated keys to mark as revoked in this
 *                             bundle. Triggers the revocation walk on import.
 *   --subscription-id=<str>   Default: "sub_dev_local_001".
 *   --tenant-slug=<str>       Default: "self-localtest".
 *   --tier=<str>              Default: "business".
 *   --seats=<n>               Default: 5.
 *   --not-after=<unix-s>      Bundle expiry (unix seconds). Default: now + 24h.
 *   --out=<path>              Write the bundle JSON to this file. If omitted,
 *                             the JSON is written to stdout.
 *   --only-derive             Derive the trust anchor from --seed-hex (or a
 *                             freshly generated seed) and exit without
 *                             minting a bundle. Prints the
 *                             SEAQUEL_BUNDLE_TRUSTED_PUBKEY=… line on stdout
 *                             so it can be captured directly; the
 *                             PROD_TRUSTED_PUBKEYS snippet and the seed go to
 *                             stderr. Use this when baking a production
 *                             signing key's public half into
 *                             `src/lib/server/airgap/bundle-store.ts`, where
 *                             the private seed lives as a Cloudflare Worker
 *                             secret on seaquel-app and never touches a
 *                             bundle minted here.
 *
 * This file is a Node 22+ TypeScript script (Node strips type annotations
 * natively). Run directly with `node scripts/mint-airgap-bundle.ts`.
 */
import { writeFileSync } from "node:fs";
import { argv, stderr, stdout } from "node:process";
import * as ed from "@noble/ed25519";
import {
  bytesToHex,
  canonicalize,
  fingerprintPubkey,
  type CanonicalValue,
} from "../src/lib/server/airgap/canonical.ts";
import type { BundlePayload, SignedEnvelope } from "../src/lib/server/airgap/types.ts";

interface CliArgs {
  "seed-hex"?: string;
  "owner-key"?: string;
  "member-keys": string[];
  "revoked-keys"?: string;
  "subscription-id"?: string;
  "tenant-slug"?: string;
  tier?: string;
  seats?: string;
  "not-after"?: string;
  out?: string;
  "only-derive"?: string;
  help?: boolean;
}

function parseArgs(): CliArgs {
  const args: CliArgs = { "member-keys": [] };
  for (const raw of argv.slice(2)) {
    if (raw === "--help" || raw === "-h") {
      args.help = true;
      continue;
    }
    const m = raw.match(/^--([^=]+)(?:=(.*))?$/);
    if (!m) {
      throw new Error(`unrecognised arg: ${raw}`);
    }
    const [, key, value = "true"] = m;
    if (key === "member-key") {
      args["member-keys"].push(value);
    } else if (key in args || !args.help) {
      (args as unknown as Record<string, unknown>)[key] = value;
    }
  }
  return args;
}

/**
 * Presence test for a bare boolean flag. `parseArgs` stores `--foo` as the
 * string "true", so anything present counts except an explicit `=false`.
 */
function flagEnabled(value: string | undefined): boolean {
  return value !== undefined && value !== "false";
}

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.replace(/^0x/i, "").toLowerCase();
  if (clean.length === 0 || clean.length % 2 !== 0) {
    throw new Error(`bad hex length: ${clean.length}`);
  }
  if (!/^[0-9a-f]+$/.test(clean)) {
    throw new Error(`hex has non-hex characters`);
  }
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

function base64UrlEncode(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

const HELP = `\
mint-airgap-bundle — produce a signed Seaquel air-gapped license bundle.

Usage:
  node scripts/mint-airgap-bundle.ts [options]

Required (or sensible defaults):
  --owner-key=<str>       Default: DEV-OWNER-KEY-001
  --member-key=<str>      Repeatable. Each adds one member seat.

Optional:
  --seed-hex=<64-hex>     Specific Ed25519 seed (32 bytes hex). Generates one if omitted.
  --revoked-keys=<csv>    Keys to mark as revoked in this bundle.
  --subscription-id=<str> Default: sub_dev_local_001
  --tenant-slug=<str>     Default: self-localtest
  --tier=<str>            Default: business
  --seats=<n>             Default: 5
  --not-after=<unix-s>    Default: now + 86400 (24 h)
  --out=<path>            Write to file (default: stdout).
  --only-derive           Print the trust anchor for the seed and exit —
                          no bundle is minted. stdout carries the
                          SEAQUEL_BUNDLE_TRUSTED_PUBKEY=… line; the
                          PROD_TRUSTED_PUBKEYS snippet goes to stderr.

After minting, stderr prints the exact SEAQUEL_BUNDLE_TRUSTED_PUBKEY=… line
you need to set on the seaquel container.
`;

async function main(): Promise<void> {
  const args = parseArgs();
  if (args.help) {
    stderr.write(HELP);
    return;
  }

  // 1. Resolve signing key.
  let seed: Uint8Array;
  if (args["seed-hex"]) {
    seed = hexToBytes(args["seed-hex"]);
    if (seed.length !== 32) {
      throw new Error(`--seed-hex must decode to 32 bytes, got ${seed.length}`);
    }
  } else {
    seed = crypto.getRandomValues(new Uint8Array(32));
  }
  const pubkey = await ed.getPublicKeyAsync(seed);
  const fingerprint = await fingerprintPubkey(pubkey);

  // 1b. `--only-derive` short-circuit. Nothing below this point runs: there
  // is no payload to build and no bundle to sign. The env-var line goes to
  // stdout (it is this mode's primary output, the way the envelope is in a
  // normal run) so it can be piped or captured; everything else is stderr.
  if (flagEnabled(args["only-derive"])) {
    const envLine = `SEAQUEL_BUNDLE_TRUSTED_PUBKEY=${fingerprint}:${bytesToHex(pubkey)}`;
    if (args.out) {
      writeFileSync(args.out, `${envLine}\n`);
      stderr.write(`✓ Wrote trust anchor to ${args.out}\n`);
    } else {
      stdout.write(`${envLine}\n`);
    }
    stderr.write(`\nTrust anchor:\n`);
    stderr.write(`  fingerprint  ${fingerprint}\n`);
    stderr.write(`  pubkeyHex    ${bytesToHex(pubkey)}\n`);
    stderr.write(`\nPaste into PROD_TRUSTED_PUBKEYS in src/lib/server/airgap/bundle-store.ts:\n`);
    stderr.write(`  {\n`);
    stderr.write(`    fingerprint: "${fingerprint}",\n`);
    stderr.write(`    pubkeyHex: "${bytesToHex(pubkey)}",\n`);
    stderr.write(`  },\n`);
    if (!args["seed-hex"]) {
      stderr.write(`\nGenerated seed — this is the private half. Store it as the\n`);
      stderr.write(`SEAQUEL_BUNDLE_SIGNING_PRIVATE_KEY secret on seaquel-app; never commit it:\n`);
      stderr.write(`  ${bytesToHex(seed)}\n`);
    }
    return;
  }

  // 2. Build payload.
  const now = Math.floor(Date.now() / 1000);
  const notAfter = args["not-after"] ? Number(args["not-after"]) : now + 86_400;
  const ownerKey = args["owner-key"] ?? "DEV-OWNER-KEY-001";
  const memberKeys = args["member-keys"];
  const seatCount = args.seats ? Number(args.seats) : 5;
  const seatTokens: BundlePayload["seat_tokens"] = [
    { key: ownerKey, role: "owner" },
    ...memberKeys.map((k) => ({ key: k, role: "member" as const })),
  ];
  // Pad with vacant placeholders to reach the declared seat count, mirroring
  // what the control-plane endpoint does in seaquel-app/.../airgap/bundle.
  const tenantSlug = args["tenant-slug"] ?? "self-localtest";
  for (let i = seatTokens.length; i < seatCount; i++) {
    seatTokens.push({
      key: `airgap-vacant-${tenantSlug}-${i}`,
      role: "member",
    });
  }
  const payload: BundlePayload = {
    version: 1,
    issued_at: now,
    not_before: now - 60,
    not_after: notAfter,
    subscription_id: args["subscription-id"] ?? "sub_dev_local_001",
    tenant_slug: tenantSlug,
    tier: args.tier ?? "business",
    seats: seatCount,
    seat_tokens: seatTokens,
    revoked_keys: args["revoked-keys"]
      ? args["revoked-keys"]
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean)
      : [],
    issued_by_install_id: null,
  };

  // 3. Sign over canonical bytes.
  const canonical = canonicalize(payload as unknown as CanonicalValue);
  const sig = await ed.signAsync(canonical, seed);
  const envelope: SignedEnvelope = {
    payload: base64UrlEncode(canonical),
    sig: base64UrlEncode(sig),
    pubkey_fingerprint: fingerprint,
  };
  const envelopeJson = JSON.stringify(envelope);

  // 4. Emit envelope (stdout or file).
  if (args.out) {
    writeFileSync(args.out, envelopeJson);
    stderr.write(`✓ Wrote bundle to ${args.out} (${envelopeJson.length} bytes)\n`);
  } else {
    stdout.write(envelopeJson);
    stdout.write("\n");
  }

  // 5. Print trust info + summary to stderr.
  stderr.write(`\nTrust info — set this on the seaquel container:\n`);
  stderr.write(`  SEAQUEL_BUNDLE_TRUSTED_PUBKEY=${fingerprint}:${bytesToHex(pubkey)}\n`);
  if (!args["seed-hex"]) {
    stderr.write(`\nGenerated keypair (record to mint more bundles with the same trust anchor):\n`);
    stderr.write(`  --seed-hex=${bytesToHex(seed)}\n`);
  }
  stderr.write(`\nBundle summary:\n`);
  stderr.write(`  subscription_id  ${payload.subscription_id}\n`);
  stderr.write(`  tenant_slug      ${payload.tenant_slug}\n`);
  stderr.write(`  tier             ${payload.tier}\n`);
  stderr.write(`  seats            ${payload.seats}\n`);
  stderr.write(
    `  not_after        ${new Date(payload.not_after * 1000).toISOString()} (in ${Math.round((payload.not_after - now) / 3600)} h)\n`,
  );
  stderr.write(`  owner seat       ${ownerKey}\n`);
  for (const k of memberKeys) {
    stderr.write(`  member seat      ${k}\n`);
  }
  if (payload.revoked_keys.length > 0) {
    stderr.write(`  revoked_keys     ${payload.revoked_keys.join(", ")}\n`);
  }
}

main().catch((e: unknown) => {
  stderr.write(`Error: ${e instanceof Error ? e.message : String(e)}\n`);
  process.exit(1);
});

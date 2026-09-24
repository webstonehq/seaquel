# Source / binary license split

**Date:** 2026-09-18
**Status:** Accepted

## Problem

The repository ships a single MIT `LICENSE` and a README that advertises
Seaquel as "Free and open source", with a comparison table scoring Seaquel
"Free" against DBeaver's "Freemium". Meanwhile `seaquel.app` already sells
Individual and Business commercial licenses, and the desktop app already
carries `personal` / `individual` / `business` tiers with keyring storage and
an activation UI in settings.

The repo and the product therefore tell different stories. The website terms
try to tell the right one but contradict themselves: §3 reads "The Software
source code is released under the MIT License and is free to use for personal,
non-commercial purposes", which is incoherent — MIT grants commercial use
unconditionally. §2 defines "Software" as the application generally, never
separating binaries Webstone distributes from source a user builds.

## Decision

Adopt the yaak.app model: **the source is MIT; the binaries we distribute have
their own terms.**

Three artifact classes, each with its own grant:

| Artifact | Terms |
| --- | --- |
| Source Code (`webstonehq/seaquel`) | MIT, unconditionally — including building and running your own binaries at work |
| Official Binaries (desktop builds from `seaquel.app/download`, GitHub Releases, in-app updater) | Free for personal, non-commercial use; Commercial Use requires a paid license |
| Official Server (`ghcr.io/webstonehq/seaquel`, Seaquel Cloud) | Subscription required for all use, personal included |

### Self-built binaries

Accepted openly and stated plainly: anyone who clones and builds is governed by
MIT alone, so a company may self-build and use Seaquel at work at no cost.
Purchases are driven by convenience — signed builds, auto-update, support — not
by license enforcement. The only limit is trademark: self-builds may not be
distributed under the Seaquel name, logo, or branding.

### Authority

`seaquel.app/terms` is the single source of truth for the binary terms. The
repository keeps only the MIT `LICENSE` plus a README section that links out.
No second license file in the repo — one more file is one more thing to drift.

## Changes

### `webstonehq/seaquel` (branch: `main`)

`LICENSE` stays byte-identical MIT. License scanners (GitHub's detector, SPDX
tooling, `cargo about`) match on exact text, and a prepended note would break
detection and cost the MIT signal being kept deliberately. `"license": "MIT"`
in `package.json` and `Cargo.toml` likewise stays — it describes the source
package, which is genuinely MIT.

`README.md`, four edits:

1. Badge row gains a "Free for personal use" badge linking to `/pricing`,
   alongside the existing MIT badge.
2. Tagline: "Free and open source" → "Open source, free for personal use."
3. Comparison table: the `Free` row becomes `Free for personal use` —
   Seaquel ✅, DBeaver ✅, TablePlus ❌, DataGrip ❌, pgAdmin ✅. This row is
   the actively misleading one today, scoring Seaquel above DBeaver on a
   dimension where the two are now comparable.
4. `## License` section replaces bare `[MIT](LICENSE)` with the three-class
   split and links to `seaquel.app/terms`.

### `seaquel-app/main` (marketing site)

`routes/terms/+page.svelte`:

- **§2 Definitions** — replace `"Software"` with `"Source Code"`,
  `"Official Binaries"`, `"Official Server"`. `"Commercial Use"` keeps its
  current wording; it is already close to yaak's and needs no size threshold.
- **§3 License Grant** — replaced outright, one paragraph per artifact class.
- **§7 Intellectual Property** — add that self-built binaries may not carry
  Seaquel branding.
- Effective date bumped to the deploy date, per §11.

`routes/download/+page.svelte` — a notice line under the platform cards:
"Free for personal use. Using Seaquel at work? · Terms". The download page is
where an Official Binary is acquired, so it is where notice belongs.

Marketing copy elsewhere already reads correctly (`nav-header`,
`comparison-section`, `cta-section` all say "Free for personal use") and is
left alone.

## Out of scope

- No changes to `license.svelte.ts`, the settings activation UI, or
  `messages/en.json`. The tiers and activation flow already match this model.
- No first-run prompt, nag screen, or feature gating on the personal tier.
  The desktop model stays honour-system, like yaak's.
- No change to the self-host signup gate. The Official Server clause describes
  today's behaviour rather than altering it.
- No CLA. Inbound MIT contributions already grant sublicensing rights, so
  contributions can ship in differently-licensed binaries. This is why yaak
  does not run one either.

## Note

The `main` README has no self-host/Docker section; that content lives on the
`seaquel-cloud` branch. The Official Server sentence lands on `main` anyway
(Cloud is live and sold), and when `seaquel-cloud` merges its self-host section
should link to the same `## License` section rather than restating terms.

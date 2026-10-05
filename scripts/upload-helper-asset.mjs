// Uploads a target's DuckDB helper `.gz` into the draft release, from
// `release.yml`'s matrix job through `actions/github-script` (the desktop
// DuckDB helper plan, Task 9 review I2).
//
// Not `gh release upload`: `gh` isn't promised on every runner image the
// matrix uses (`windows-11-arm`, `ubuntu-24.04-arm` are partner images), and
// installing it there (winget or choco on Windows ARM) isn't reliable
// either. `actions/github-script` runs on the runner's own Node, as
// `tauri-action` does, with an authenticated Octokit, so the upload needs
// nothing the job doesn't already have.
//
// The file is read once: its bytes are hashed and compared with the pin's
// record (scripts/release-pin.mjs `export`), and those same bytes are
// uploaded, so what reaches the draft is exactly what the app was pinned to.
// An asset of the same name is replaced (a re-run). GitHub's answer is
// checked too: its size, and its `digest` when the API gives one.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { helperAsset, pinOf, RECORD_DIR, recordName } from "./release-pin.mjs";

/**
 * The draft release for `tag`: the one `tauri-action` reported, else the only
 * release (draft or not) with that tag name.
 */
export async function findRelease(github, { owner, repo }, { tag, releaseId }) {
  if (releaseId) {
    const { data } = await github.rest.repos.getRelease({
      owner,
      repo,
      release_id: Number(releaseId),
    });
    return data;
  }
  const all = await github.paginate(github.rest.repos.listReleases, { owner, repo, per_page: 100 });
  const matching = all.filter((r) => r.tag_name === tag);
  if (matching.length !== 1) {
    throw new Error(`expected one release tagged ${tag}, found ${matching.length}`);
  }
  return matching[0];
}

/**
 * Uploads `src-tauri/binaries/<asset>` for `target` into the draft.
 * @param {{ github: any, context: { repo: { owner: string, repo: string } },
 *   root: string, target: string, tag: string, releaseId?: string, log?: (s: string) => void }} args
 */
export async function uploadHelper({
  github,
  context,
  root,
  target,
  tag,
  releaseId,
  log = console.log,
}) {
  const name = helperAsset(target);
  const record = JSON.parse(readFileSync(join(root, RECORD_DIR, recordName(target)), "utf8"));
  if (record.asset !== name) throw new Error(`the record names ${record.asset}, not ${name}`);
  const bytes = readFileSync(join(root, "src-tauri", "binaries", name));
  const now = pinOf(bytes);
  if (now.size !== record.size || now.sha256 !== record.sha256) {
    throw new Error(
      `${name} changed after the app was pinned (${now.size} bytes, sha256 ${now.sha256})`,
    );
  }

  const { owner, repo } = context.repo;
  const release = await findRelease(github, { owner, repo }, { tag, releaseId });
  for (const old of release.assets ?? []) {
    if (old.name === name) {
      await github.rest.repos.deleteReleaseAsset({ owner, repo, asset_id: old.id });
      log(`replaced the draft's earlier ${name}`);
    }
  }
  const { data } = await github.rest.repos.uploadReleaseAsset({
    owner,
    repo,
    release_id: release.id,
    name,
    data: bytes,
    headers: { "content-type": "application/gzip", "content-length": bytes.length },
  });
  if (data.size !== record.size) {
    throw new Error(`GitHub stored ${name} as ${data.size} bytes, not ${record.size}`);
  }
  if (data.digest && data.digest !== `sha256:${record.sha256}`) {
    throw new Error(
      `GitHub stored ${name} with digest ${data.digest}, not sha256:${record.sha256}`,
    );
  }
  log(`uploaded ${name} (${record.size} bytes, sha256 ${record.sha256}) to release ${release.id}`);
}

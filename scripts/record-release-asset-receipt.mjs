import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";

// Post-publish receipt: downloads every asset of the tagged release and
// records its name, size, and SHA-256 so the release contract (source /
// version / tag / asset identity) has a durable, reviewable receipt.

const [tag, outputPath] = process.argv.slice(2);
if (!tag || !outputPath) {
  console.error("usage: node scripts/record-release-asset-receipt.mjs <tag> <output.json>");
  process.exit(1);
}

const repo = process.env.GITHUB_REPOSITORY ?? "DaleXiao/Cindx";
const token = process.env.GITHUB_TOKEN ?? "";
const headers = {
  accept: "application/vnd.github+json",
  ...(token ? { authorization: `Bearer ${token}` } : {})
};

const releaseResponse = await fetch(
  `https://api.github.com/repos/${repo}/releases/tags/${encodeURIComponent(tag)}`,
  { headers }
);
if (!releaseResponse.ok) {
  console.error(`release lookup failed: ${releaseResponse.status}`);
  process.exit(1);
}
const release = await releaseResponse.json();

const assets = [];
for (const asset of release.assets ?? []) {
  const download = await fetch(asset.browser_download_url, {
    headers: { accept: "application/octet-stream" }
  });
  if (!download.ok) {
    console.error(`asset download failed: ${asset.name} (${download.status})`);
    process.exit(1);
  }
  const bytes = Buffer.from(await download.arrayBuffer());
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  if (bytes.length !== asset.size) {
    console.error(`asset size mismatch: ${asset.name}`);
    process.exit(1);
  }
  assets.push({ name: asset.name, size: bytes.length, sha256 });
}

if (assets.length === 0) {
  console.error("the release carries no assets");
  process.exit(1);
}

writeFileSync(
  outputPath,
  `${JSON.stringify(
    {
      schema: "cindx.release-asset-receipt.v1",
      repository: repo,
      tag,
      commit: release.target_commitish,
      recorded_at_ms: Date.now(),
      assets
    },
    null,
    2
  )}\n`
);
console.log(`recorded ${assets.length} asset receipt(s) for ${tag}`);

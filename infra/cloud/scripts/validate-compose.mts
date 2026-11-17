import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../../../", import.meta.url));
const base = path.join(root, "docker-compose.yml"),
  override = path.join(root, "infra/cloud/compose.storage-aws.yml");
const source =
  (await fs.readFile(base, "utf8")) + (await fs.readFile(override, "utf8"));
const vars = [
  ...new Set([...source.matchAll(/\$\{([A-Z0-9_]+)/g)].map((x) => x[1])),
];
const synthetic: Record<string, string> = { PATH: process.env.PATH || "" };
for (const name of vars)
  synthetic[name] = name.includes("PORT")
    ? "15432"
    : name === "JWT_JWKS"
      ? '{"keys":[]}'
      : "synthetic";
synthetic.CBGPU_AWS_REGION = "ap-northeast-2";
synthetic.CBGPU_AWS_STORAGE_BUCKET =
  "cbgpu-synthetic-storage-no-cloud-resources";
const dir = await fs.mkdtemp(path.join(os.tmpdir(), "cbgpu-cloud-config-"));
try {
  const envFile = path.join(dir, "synthetic.env");
  await fs.writeFile(
    envFile,
    Object.entries(synthetic)
      .filter(([k]) => k !== "PATH")
      .map(([k, v]) => k + "=" + v)
      .join("\n"),
    { mode: 0o600 },
  );
  const config = JSON.parse(
    execFileSync(
      "docker",
      [
        "compose",
        "--project-name",
        "cbgpu-cloud-config-only",
        "--env-file",
        envFile,
        "--file",
        base,
        "--file",
        override,
        "config",
        "--format",
        "json",
      ],
      { env: synthetic, encoding: "utf8", maxBuffer: 4 * 1024 * 1024 },
    ),
  );
  const storage = config.services.storage;
  assert.equal(
    storage.image,
    "supabase/storage-api:v1.81.0@sha256:98ffec5d0678803783c30e404c27f4ec65e97efbe014088f2a86b4b8b54b9f54",
  );
  assert.equal(storage.environment.STORAGE_BACKEND, "s3");
  assert.equal(
    storage.environment.GLOBAL_S3_ENDPOINT,
    "https://s3.ap-northeast-2.amazonaws.com",
  );
  assert.equal(storage.environment.GLOBAL_S3_FORCE_PATH_STYLE, "false");
  assert.equal(storage.environment.AWS_ACCESS_KEY_ID, "");
  assert.equal(storage.environment.AWS_SECRET_ACCESS_KEY, "");
  assert.equal(storage.environment.AWS_SESSION_TOKEN, "");
  assert.equal(storage.environment.POSTGREST_URL, "http://rest:3000");
  assert.ok(
    storage.environment.DATABASE_URL.includes("supabase_storage_admin"),
  );
  console.log(
    "Existing pinned Supabase Storage S3 override validated with synthetic configuration; no container or AWS resource started.",
  );
} finally {
  await fs.rm(dir, { recursive: true, force: true });
}

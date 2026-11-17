/** Build context exporter. No AWS calls; no .env, private data, state or symlink traversal. */
import fs from "node:fs/promises";
import crypto from "node:crypto";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../../../", import.meta.url)),
  destination = process.argv[2];
if (
  !destination ||
  !path.isAbsolute(destination) ||
  !path.relative(root, destination).startsWith(".." + path.sep)
)
  throw Error(
    "Provide a new absolute context directory outside the repository.",
  );
const sourceCommit = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: root,
  encoding: "utf8",
}).trim();
const sourceDirty =
  execFileSync(
    "git",
    [
      "status",
      "--porcelain",
      "--",
      "rust/",
      "supabase/migrations/",
      "schema/data/",
      "infra/cloud/",
      "LICENSE",
      "NOTICE",
      "THIRD_PARTY_NOTICES.md",
    ],
    { cwd: root, encoding: "utf8" },
  ).trim().length > 0;
if (process.argv.includes("--require-clean") && sourceDirty)
  throw Error(
    "Source snapshot has uncommitted changes; no release context created.",
  );
await fs.mkdir(destination, { mode: 0o700 });
const manifest: Array<{ path: string; sha256: string; bytes: number }> = [];
async function copy(source: string, target: string) {
  const bytes = await fs.readFile(source);
  await fs.mkdir(path.dirname(path.join(destination, target)), {
    recursive: true,
  });
  await fs.writeFile(path.join(destination, target), bytes);
  manifest.push({
    path: target,
    sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
    bytes: bytes.length,
  });
}
const files = execFileSync(
  "git",
  [
    "ls-files",
    "--cached",
    "--others",
    "--exclude-standard",
    "-z",
    "--",
    "rust/",
    "supabase/migrations/",
    "schema/data/",
  ],
  { cwd: root, encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
)
  .split("\0")
  .filter(Boolean);
let copied = 0;
for (const relative of new Set(files)) {
  if (
    relative
      .split("/")
      .some((s) =>
        [
          "target",
          "node_modules",
          ".git",
          "private",
          "backups",
          "uploads",
          ".venv",
        ].includes(s),
      ) ||
    /(?:^|\/)(?:\.env(?:\.|$)|credentials\.toml$)|\.(?:db|sqlite|sqlite3|pem|key|jks|keystore)$/i.test(
      relative,
    )
  )
    continue;
  if (
    !/\.(?:rs|toml|lock|sql|csv|json|txt|md|html|css|ts|svg|png)$/.test(
      relative,
    )
  )
    continue;
  if (
    relative.startsWith("schema/data/") &&
    !/^schema\/data\/neis-high-schools-\d{4}-\d{2}-\d{2}\.json$/.test(relative)
  )
    throw Error(
      "Only the public source school catalog is permitted in schema/data.",
    );
  const source = path.join(root, relative);
  if ((await fs.realpath(source).catch(() => source)) !== source)
    throw Error("Source parent symlinks are not allowed in release contexts.");
  let stat;
  try {
    stat = await fs.lstat(source);
  } catch {
    continue;
  }
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 32 * 1024 * 1024)
    throw Error(
      "Source context contains an unsupported file; inspect the allowlist.",
    );
  const target = path.join(destination, relative);
  await fs.mkdir(path.dirname(target), { recursive: true });
  await copy(source, relative);
  copied++;
}
for (const d of ["supabase/migrations", "schema/data"])
  await fs.mkdir(path.join(destination, d), { recursive: true });
for (const [source, target] of [
  ["Dockerfile.native-api", "Dockerfile"],
  ["healthcheck.rs", "healthcheck.rs"],
])
  await copy(fileURLToPath(new URL("../" + source, import.meta.url)), target);
for (const license of ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.md"])
  await copy(path.join(root, license), "licenses/cbgpu/" + license);
await fs.writeFile(
  path.join(destination, "context-manifest.json"),
  JSON.stringify(
    {
      sourceCommit,
      sourceDirty,
      files: manifest.sort((a, b) => a.path.localeCompare(b.path)),
    },
    null,
    2,
  ) + "\n",
);
await fs.writeFile(
  path.join(destination, ".dockerignore"),
  "**/target\n**/.env*\n**/private\n**/*.sqlite*\n**/*.db\n",
);
console.log(
  JSON.stringify({
    context: destination,
    sourceFiles: copied,
    sourceCommit,
    sourceDirty,
    privateRuntimeIncluded: false,
    awsCalls: false,
  }),
);

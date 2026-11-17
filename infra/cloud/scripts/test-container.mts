/** Only creates/removes its own credential-free local fixture; never touches production containers. */
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
const candidate=process.env.CBGPU_CLOUD_TEST_PACKAGE||'cbgpu-schools';
if(!['cbgpu-schools','cbgpu-ad-revenue'].includes(candidate))throw Error('Unsupported local container fixture package');
const revenue=candidate==='cbgpu-ad-revenue';
if (process.env.DOCKER_HOST && !process.env.DOCKER_HOST.startsWith("unix://"))
  throw Error("Only a local Unix Docker socket is permitted.");
const endpoint = JSON.parse(
  execFileSync(
    "docker",
    ["context", "inspect", "--format", "{{json .Endpoints.docker.Host}}"],
    { encoding: "utf8" },
  ),
);
if (typeof endpoint !== "string" || !endpoint.startsWith("unix://"))
  throw Error("Remote/cloud Docker contexts are not permitted.");
let id = "";
const scratch = await fs.mkdtemp(
  path.join(os.tmpdir(), "cbgpu-cloud-image-proof-"),
);
try {
  id = execFileSync(
    "docker",
    [
      "run",
      "--detach",
      "--pull=never",
      "--name",
      "cbgpu-cloud-review-" + crypto.randomUUID(),
      "--network",
      "none",
      "--read-only",
      "--cap-drop",
      "ALL",
      "--security-opt",
      "no-new-privileges:true",
      "--user",
      "65532:65532",
      "--cpus",
      "1",
      "--memory",
      "256m",
      "--pids-limit",
      "64",
      "--env",
      revenue?"CBGPU_AD_REVENUE_ENABLED=false":"CBGPU_MULTISCHOOL_V1=false",
      "--env",
      revenue?"CBGPU_AD_REVENUE_LISTEN=127.0.0.1:3549":"CBGPU_SCHOOLS_LISTEN=127.0.0.1:3546",
      "--env",
      revenue?"CBGPU_AD_REVENUE_BACKEND_URL=http://127.0.0.1:1":"CBGPU_SCHOOLS_BACKEND_URL=http://127.0.0.1:1",
      "--env",
      revenue?"CBGPU_AD_REVENUE_ANON_KEY=synthetic-no-production-access":"CBGPU_SCHOOLS_ANON_KEY=synthetic-no-production-access",
      "--env",
      revenue?"CBGPU_HEALTH_PORT=3549":"CBGPU_HEALTH_PORT=3546",
      revenue?"cbgpu/native-ad-revenue:cloud-review":"cbgpu/native-school-api:cloud-review",
    ],
    { encoding: "utf8" },
  ).trim();
  assert.match(id, /^[a-f0-9]{64}$/);
  let healthy = false;
  for (let i = 0; i < 100; i++) {
    if (
      spawnSync("docker", ["exec", id, "/cbgpu-healthcheck"], {
        stdio: "ignore",
      }).status === 0
    ) {
      healthy = true;
      break;
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  assert.ok(healthy, "Real disabled Rust API health probe failed");
  const state = JSON.parse(
    execFileSync("docker", ["inspect", id], { encoding: "utf8" }),
  )[0];
  assert.equal(state.Config.User, "65532:65532");
  assert.equal(state.HostConfig.ReadonlyRootfs, true);
  assert.equal(state.HostConfig.NetworkMode, "none");
  assert.equal(state.HostConfig.Privileged, false);
  assert.deepEqual(state.Mounts, []);
  assert.ok(
    !state.HostConfig.PortBindings ||
      Object.keys(state.HostConfig.PortBindings).length === 0,
  );
  execFileSync("docker", [
    "cp",
    id + ":/usr/share/cbgpu/context-manifest.json",
    path.join(scratch, "manifest.json"),
  ]);
  execFileSync("docker", [
    "cp",
    id + ":/usr/share/licenses",
    path.join(scratch, "licenses"),
  ]);
  const provenance = JSON.parse(
    await fs.readFile(path.join(scratch, "manifest.json"), "utf8"),
  );
  assert.match(provenance.sourceCommit, /^[a-f0-9]{40}$/);
  assert.equal(typeof provenance.sourceDirty, "boolean");
  assert.ok(
    provenance.files.some(
      (f: { path: string; sha256: string }) =>
        f.path === "rust/Cargo.lock" && /^[a-f0-9]{64}$/.test(f.sha256),
    ),
  );
  assert.ok(
    (
      await fs.readFile(path.join(scratch, "licenses/cbgpu/LICENSE"), "utf8")
    ).includes("Apache License"),
  );
  assert.ok(
    (
      await fs.readdir(path.join(scratch, "licenses/third-party"), {
        recursive: true,
      })
    ).length > 20,
    "Bundled dependencies must retain their license notices",
  );
  execFileSync("docker", ["stop", "--time", "10", id], { stdio: "ignore" });
  const exited = JSON.parse(
    execFileSync("docker", ["inspect", id], { encoding: "utf8" }),
  )[0];
  assert.equal(
    exited.State.ExitCode,
    0,
    "SIGTERM must exit through the server graceful shutdown handler",
  );
  console.log(
    "Native API container health, non-root/read-only/isolation and graceful SIGTERM passed; no production/AWS changes.",
  );
} finally {
  await fs.rm(scratch, { recursive: true, force: true });
  if (/^[a-f0-9]{64}$/.test(id))
    spawnSync("docker", ["rm", "--force", id], { stdio: "ignore" });
}

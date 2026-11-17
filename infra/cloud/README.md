# Cloud readiness tooling

Source-only AWS preparation. Current production stays unchanged. Read
[the deployment audit](../../docs/deployment/cloud-readiness.md) before using it.
There is deliberately no `deploy` script, ECS service, public ingress or database
resource. Synthesis does not make the data layer portable or authorize a move.

```bash
cd infra/cloud
npm ci --ignore-scripts
npm test
npm run synth
npm run validate:compose
```

The pinned CDK CLI uses `--no-lookups --no-version-reporting`; no AWS credentials
or account lookup is required for these checks. Generated `dist/`, `cdk.out/` and
`node_modules/` stay out of source control. The template is a future review
artifact, not a deployable promise until the missing data/network gates pass.

## Real native API container

Never send a live checkout with private files as a Docker build context. Export a
new allowlisted context outside the repository:

```bash
npm run prepare:context -- /tmp/cbgpu-cloud-review-context
CBGPU_CLOUD_CONTEXT=/tmp/cbgpu-cloud-review-context docker compose \
  -p cbgpu-cloud-review -f compose.native-review.yml config --quiet
# Local build and disposable, network-none fixture only:
docker build -t cbgpu/native-school-api:cloud-review \
  -f /tmp/cbgpu-cloud-review-context/Dockerfile /tmp/cbgpu-cloud-review-context
npm run test:container
```

`Dockerfile.native-api` supports the existing Rust packages `cbgpu-schools`,
`cbgpu-accounts`, the optional GPU registry, and `cbgpu-ad-revenue`. It compiles an exact Cargo lockfile,
uses digest-pinned base images, runs as UID65532, and includes a shell-free Rust
loopback health probe. APIs keep their private-loopback binding. A future ingress
sidecar must share the task network and preserve the current trusted-host policy;
these APIs are not changed to listen publicly just to accommodate a container.

For the optional official-report API, build with
`--build-arg CBGPU_PACKAGE=cbgpu-ad-revenue` and set `CBGPU_HEALTH_PORT=3549` at
runtime. Its reporting flag remains off by default. `/health` is a liveness
probe, not proof of configured database authority, imported official reports,
provider approval or revenue. No image build enables serving or imports data.
After building the local image as `cbgpu/native-ad-revenue:cloud-review`, run
`CBGPU_CLOUD_TEST_PACKAGE=cbgpu-ad-revenue npm run test:container` to verify the
disabled binary, bundled provenance/licenses, non-root read-only execution and
graceful shutdown in a disposable network-none container. The default test still
targets the school API; neither test loads credentials or contacts an ad provider.

The standalone `compose.native-review.yml` is disabled, credential-free and has no
published ports or mounted data. It is **not** an override for the production
Compose project. No container in this image installs drivers, mounts the Docker
socket, executes tenant code or accesses a school GPU.

## Storage override

`compose.storage-aws.yml` uses the actual pinned Supabase Storage S3 adapter, with
`CBGPU_AWS_REGION` and `CBGPU_AWS_STORAGE_BUCKET` supplied explicitly. The config
validator creates only a temporary synthetic env file and calls Compose `config`;
it does not start containers, load a real `.env`, copy data or contact AWS.
Keep database objects/roles, Storage RLS and exact object bytes/names together
through a separately approved restore/cutover rehearsal.

The exported context includes a per-file SHA-256 manifest with source commit and
working-tree status, and the image retains that manifest plus project and Cargo
registry license/notice files. Use `--require-clean` when creating a release
context; local candidate builds truthfully record uncommitted source instead of
claiming it matches a commit. No image is pushed by this tooling.

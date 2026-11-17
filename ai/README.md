# CBGPU local AI

Maintained AI application code is in the native Rust crates under
[`rust/crates`](../rust/crates). The API, PostgreSQL store, reservation scheduler,
model workers, document/OCR worker, robotics protocols and corpus permissions
have separate implementations and tests. See the
[native service guide](../rust/crates/cbgpu-ai/README.md).

Production is not switched by removing old source files. Existing immutable
legacy releases and their private state remain intact until the reviewed drain,
backup, PostgreSQL import and routing handoff succeed. Backend delivery holds AI
changes rather than pointing the operating legacy service at missing source.

## Model assets

[`models.json`](./models.json) pins public repository revisions, data-file patterns,
licenses and the native Whisper/VAD SHA256 values. No model weights are committed.
The native preparer replaces the retired setup/download scripts:

```bash
cargo build --locked --manifest-path rust/Cargo.toml -p cbgpu-ai \
  --bin cbgpu-ai-prepare-models
rust/target/debug/cbgpu-ai-prepare-models ai/models.json /absolute/private/models
# After reviewing the offline plan:
rust/target/debug/cbgpu-ai-prepare-models ai/models.json /absolute/private/models --apply
```

The default plan performs no network requests or filesystem writes. Explicit
apply downloads only pinned non-executable public data, never uses a cached Hub
token, checks sizes and LFS SHA256/Git blob identities, records final SHA256 hashes
and refuses to overwrite changed existing files. Interrupted partial files are
removed. Whisper uses native GGML files in `transcribe-native`, not CTranslate2
weights. Native CUDA/llama libraries, FFmpeg and the OCR image are separately
reviewed runtime prerequisites; this command installs no drivers, Python runtime,
packages, services or GPU jobs.

## Verification

```bash
cargo test --locked --manifest-path rust/Cargo.toml -p cbgpu-ai
cargo test --locked --manifest-path rust/Cargo.toml -p cbgpu-ai-handoff
```

CI also runs disposable PostgreSQL tests for store/import equality, ownership,
scheduler preemption and module revocation, and the no-new-writes rollback gate.
The OCR container uses the static Rust document worker plus native Tesseract and
Poppler. Model and notebook preparation tests use synthetic data and fake process
adapters; they do not launch work on a school GPU.

The downloader follows the [Hub metadata contract](https://huggingface.co/docs/huggingface_hub/package_reference/hf_api).
Vendor model licenses remain listed in the pinned manifest.

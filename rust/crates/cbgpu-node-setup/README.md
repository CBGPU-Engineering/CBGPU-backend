# CBGPU Node Setup

A small Rust setup application with a local Korean browser UI. Opening the
AppImage starts a private, short-lived setup session. It validates downloaded
node artifacts against the official manifest, imports school enrollment files,
runs explicitly requested diagnostics, and installs the existing reviewed node
user service. Its explicit image-preparation button pulls only the fixed public digest through the local Docker socket, using an empty private Docker CLI configuration instead of personal registry credentials. It never asks for a CBGPU password or changes a driver, Docker
permission, existing identity, or existing service.

Linux installation currently supports Ubuntu x86_64 with an existing NVIDIA
driver and user systemd. Notebook mode additionally requires Docker access,
NVIDIA Container Toolkit and the approved Jupyter image. The application shows
prerequisite failures instead of claiming that installation means execution is
ready. Physical GPU admission remains the node executor's responsibility.

The Windows executable opens the same setup UI and supports read-only WSL2
Ubuntu diagnostics. An experimental WSL adapter is implemented and unit tested,
but installation stays disabled until a real Windows/WSL environment is
validated. A Windows executable is not a native Windows GPU daemon. No Windows
GPU compatibility is claimed by the packaging workflow's build/version checks.

The localhost listener requires an exact host and origin, a session cookie and a
CSRF proof. A one-use launcher is stored in a current-user-only file and POSTs its
capability to the local listener. The capability is never put in a URL, process
argument or log. Anonymous localhost requests cannot acquire a setup session.
Every local API also requires an origin-and-port-scoped access proof, stored only in sessionStorage after private bootstrap. A cookie observed by another localhost port cannot read state or perform installation. This local proof is not a CBGPU account or node credential. School enrollment files are mode 600 on Linux; Windows upload data stays in
memory. The temporary session is removed at exit and expires after 20 minutes.

## Build

```bash
npm ci --ignore-scripts
node rust/crates/cbgpu-node-setup/build-web.mts
cargo test --locked --manifest-path rust/crates/cbgpu-node-setup/Cargo.toml
cargo build --locked --release --manifest-path rust/crates/cbgpu-node-setup/Cargo.toml
```

For the Linux AppImage, add the `x86_64-unknown-linux-musl` Rust target and build
that target, then run `packaging/package-appimage.mts STATIC_BINARY OUTPUT_DIR`.
The packager verifies pinned appimagetool and runtime checksums. If those rolling
upstream assets are replaced, packaging deliberately fails until their new
checksums are reviewed. The distributed application is Apache-2.0; the bundled
AppImage runtime is MIT licensed. Keep the AppImage runtime's upstream copyright
and license notices when redistributing it.

AppImage execution permissions and FUSE availability are operating-system
prerequisites. `APPIMAGE_EXTRACT_AND_RUN=1` supports environments without FUSE.
The desktop launcher opens a local browser; a headless server should use the
existing node CLI rather than an installation UI it cannot display.

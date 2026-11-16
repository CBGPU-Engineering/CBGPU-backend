# Third-party components

CBGPU's original code is Apache-2.0 except components with their own license.
This inventory identifies the main boundaries; it is not a replacement for each
component's complete license text or a guarantee about every transitive package.

| Component | Use | License/source |
| --------- | --- | -------------- |
| `latex/` | CBGPU's Overleaf integration and compile adapter | AGPL-3.0; [local license](https://github.com/CBGPU-Engineering/CBGPU-latex/blob/main/LICENSE) |
| Overleaf Community Edition | Optional editor image | AGPL-3.0; [source](https://github.com/overleaf/overleaf); image/source instructions in `latex/README.md` |
| xterm and addon-fit | Vendored browser terminal | MIT; license files retained in `developer/static/vendor/` |
| Supabase self-hosted stack | Auth, REST, Storage, Realtime and database services | Multiple component licenses; consult the pinned images and [upstream repository](https://github.com/supabase/supabase) |
| PostgreSQL | Database | PostgreSQL License; [source](https://www.postgresql.org/) |
| Flutter | Native app framework | BSD-3-Clause; [source](https://github.com/flutter/flutter) |
| TeX Live | Optional compile environment | Multiple package licenses; [distribution copying conditions](https://tug.org/texlive/copying.html) |
| MongoDB and Redis images | Optional Overleaf dependencies | Version-dependent upstream licenses; inspect the exact pinned images before redistributing |
| Fonts, icons and bundled arcade assets | Product UI/assets | Preserve upstream notices; they are not relicensed by the root Apache declaration |

## npm dependencies

Committed package lockfiles and generated SBOMs record npm dependencies.
Lockfile license metadata may be incomplete. Before redistribution, inspect the actual installed package's
LICENSE/NOTICE and any package-specific restrictions. Do not assume all packages
share the application's Apache license.

CI and source releases also produce a CycloneDX SBOM for the web component using
`npm sbom --sbom-format cyclonedx --omit=dev`. It covers installed web production
npm packages only. Docker images, Python, Dart and operating-system packages need
separate inventories for a complete deployment SBOM.

## Local AI runtime and models

Native candidates use Candle (MIT/Apache-2.0), Whisper/whisper-rs (MIT),
llama.cpp (MIT), Poppler (GPL), ImageMagick (ImageMagick License), and Tesseract
(Apache-2.0). OCR system packages remain separately licensed programs in the
runtime image. Preserve their image package notices when redistributing it.

`rust/crates/cbgpu-ai-inference/src/ffi.rs` was generated with bindgen 0.72.1
from llama.cpp commit `0c1e57098bba43ac29e6e3b677cdceebdd22334f`, bundled
with llama-cpp-python 0.3.36. The header SHA-256 is
`d5cf9203bea63b575e76cba4881f29cfd1b611451578b01582b75b72ecbb04bc`.
The matching MIT notice is retained in the crate's `LLAMA_LICENSE`. The worker
loads the native shared library directly and refuses an unreviewed library ABI;
it does not invoke the Python package.

The native Whisper large-v3 asset is the MIT-licensed ggml conversion at
`ggerganov/whisper.cpp`, revision `5359861c739e955e79d9a303bcbc70fb988958b1`,
SHA-256 `64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2`.
Model weights are downloaded separately and are not committed to this repository.

The optional local AI service downloads Qwen2.5-7B-Instruct GGUF (Apache-2.0), multilingual E5-small (MIT), and faster-whisper large-v3 (MIT); exact repositories and revisions are in `ai/models.json`. Model weights and inference virtual environments are not distributed in this repository. llama-cpp-python and faster-whisper are MIT, PyTorch is BSD-style, sentence-transformers and transformers are Apache-2.0, pypdf is BSD-3-Clause, and PyAV is BSD-3-Clause with separately licensed FFmpeg components. See upstream package/model licenses before redistributing a runtime image.

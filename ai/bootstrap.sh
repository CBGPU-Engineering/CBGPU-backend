#!/usr/bin/env bash
set -euo pipefail
umask 077
AI_STATE="${CBGPU_AI_STATE:-$HOME/.local/share/cbgpu-ai}"
mkdir -p "$AI_STATE"
python3 -m venv "$AI_STATE/bootstrap"
"$AI_STATE/bootstrap/bin/pip" install --disable-pip-version-check 'uv==0.9.9'
export UV_PYTHON_INSTALL_DIR="$AI_STATE/python"
"$AI_STATE/bootstrap/bin/uv" python install 3.12
if [[ ! -x "$AI_STATE/venv/bin/python" ]]; then
  "$AI_STATE/bootstrap/bin/uv" venv --python 3.12 "$AI_STATE/venv"
fi
"$AI_STATE/bootstrap/bin/uv" pip install --python "$AI_STATE/venv/bin/python" 'pip==25.3'
"$AI_STATE/venv/bin/pip" install --disable-pip-version-check 'torch==2.7.1' --index-url https://download.pytorch.org/whl/cu126
"$AI_STATE/venv/bin/pip" install --disable-pip-version-check -r "$(dirname "$0")/requirements.txt"
"$AI_STATE/venv/bin/pip" install --disable-pip-version-check 'llama-cpp-python==0.3.16' --only-binary=llama-cpp-python --extra-index-url https://abetlen.github.io/llama-cpp-python/whl/cu124

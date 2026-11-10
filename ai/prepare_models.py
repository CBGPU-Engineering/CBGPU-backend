"""Download pinned, non-executable model assets once; no account data is sent."""
import json, os
from pathlib import Path
from huggingface_hub import snapshot_download
state=Path(os.environ.get('CBGPU_AI_STATE',Path.home()/'.local/share/cbgpu-ai'))
models=json.loads(Path(__file__).with_name('models.json').read_text())
for name, config in models.items():
    print('Preparing',name,flush=True)
    snapshot_download(config['repo'],revision=config['revision'],allow_patterns=config['files'],local_dir=state/'models'/name,max_workers=3,token=False)
print('Pinned model assets ready',flush=True)

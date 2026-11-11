"""Private asynchronous document preparation and reusable extraction cache."""
import json
import os
import subprocess
import threading
import uuid
from pathlib import Path

from core import Failure, atomic_json

IMAGE_TYPES = {'.png', '.jpg', '.jpeg', '.webp', '.tif', '.tiff'}
CACHE_VERSION = 1


class FilePreparation:
    def __init__(self, store):
        self.store = store
        self.lock = threading.RLock()
        self.active = {}
        self.slots = threading.BoundedSemaphore(2)
        self.root = store.root / 'extracted'
        self.root.mkdir(mode=0o700, exist_ok=True)
        self.root.chmod(0o700)

    def read(self, owner, id):
        self.store.get_upload(owner, id)
        try:
            value = json.loads((self.root / (id+'.json')).read_text())
            return value if value.get('version') == CACHE_VERSION else None
        except (OSError, ValueError):
            return None

    def status(self, owner, id):
        upload = self.store.get_upload(owner, id)
        with self.lock:
            value = self.active.get(id) or self.read(owner, id) or {'state': 'new'}
            if value['state'] == 'processing' and id not in self.active:
                value = {'state': 'failed', 'error': '문서 처리가 중단됐어요. 다시 시도해 주세요.'}
            return {key: item for key, item in {**value, 'id': id, 'name': upload['name']}.items()
                    if key in {'id', 'name', 'state', 'error', 'progress', 'pages', 'ocr_pages',
                               'weak_pages', 'empty_pages', 'truncated'}}

    def start(self, owner, id):
        from chat_files import MAX_BYTES, TEXT_TYPES
        upload = self.store.get_upload(owner, id)
        suffix = Path(upload['name']).suffix.lower()
        maximum = 200 * 1024**2 if suffix == '.pdf' else MAX_BYTES
        if upload['size'] > maximum or suffix not in TEXT_TYPES | IMAGE_TYPES | {'.pdf'}:
            raise Failure('8MB 이하의 PDF, 이미지, CSV 또는 텍스트 파일을 올려 주세요.')
        with self.lock:
            if self.status(owner, id)['state'] in {'ready', 'processing'}:
                return self.status(owner, id)
            if not self.slots.acquire(blocking=False):
                raise Failure('다른 파일을 읽고 있어요. 잠시 후 다시 시도해 주세요.', 429)
            value = {'state': 'processing', 'progress': 0, 'version': CACHE_VERSION}
            self.active[id] = value
            atomic_json(self.root / (id+'.json'), value)
            threading.Thread(target=self.run, args=(owner, id, suffix), daemon=True).start()
            return self.status(owner, id)

    def run(self, owner, id, suffix):
        name = 'cbgpu-ocr-'+str(uuid.uuid4())
        process = None
        timer = None
        try:
            if suffix in IMAGE_TYPES | {'.pdf'}:
                script = Path(__file__).with_name('ocr') / 'extract.py'
                source = (self.store.root / 'uploads' / id).resolve(strict=True)
                args = ['docker', 'run', '--rm', '--name', name, '--network', 'none',
                        '--label', 'cbgpu.ocr.managed=true', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
                        '--read-only', '--pids-limit', '64', '--cpus', '2', '--memory', '1g',
                        '--tmpfs', '/tmp:rw,nosuid,nodev,size=128m',
                        '--user', f'{os.getuid()}:{os.getgid()}',
                        '--mount', f'type=bind,src={source},dst=/input,readonly',
                        '--mount', f'type=bind,src={script},dst=/extract.py,readonly',
                        'cbgpu-ocr:1', '/input', suffix]
                process = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
                def expire():
                    subprocess.run(['docker', 'rm', '-f', name], timeout=10,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                timer = threading.Timer(240, expire)
                timer.start()
                result = None
                for line in process.stdout:
                    if len(line) > 3_000_000:
                        raise ValueError('문서가 너무 커요. 파일을 나누어 올려 주세요.')
                    event = json.loads(line)
                    if event.get('error'):
                        raise ValueError(event['error'])
                    if 'result' in event:
                        result = event['result']
                    else:
                        with self.lock:
                            self.active[id].update({key: event[key] for key in ['progress','pages','ocr_pages'] if key in event})
                process.wait(timeout=10)
                if process.returncode or result is None:
                    raise ValueError('문서 처리 시간이 초과됐어요. 파일을 나누어 다시 올려 주세요.')
            else:
                from chat_files import extract
                result = extract(self.store.root / 'uploads' / id, suffix)
            if not any(text.strip() for _, text in result['chunks']):
                raise ValueError('읽을 수 있는 텍스트가 없어요.')
            value = {'version': CACHE_VERSION, 'state': 'ready', 'result': result,
                     'pages': result.get('pages', 1), 'progress': result.get('pages', 1),
                     'ocr_pages': len(result.get('ocr_page_numbers', [])),
                     'weak_pages': result.get('weak_pages', []), 'empty_pages': result.get('empty_pages', []),
                     'truncated': bool(result.get('truncated'))}
        except Exception as error:
            value = {'version': CACHE_VERSION, 'state': 'failed',
                     'error': str(error)[:300] if isinstance(error, ValueError) else '파일을 읽지 못했어요. 다시 시도해 주세요.'}
        finally:
            if timer:
                timer.cancel()
            if process is not None:
                try:
                    subprocess.run(['docker', 'rm', '-f', name], timeout=10,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    process.wait(timeout=10)
                except (OSError, subprocess.SubprocessError):
                    process.kill()
                finally:
                    process.stdout.close()
        try:
            with self.lock:
                # A deleted upload must never regain a cached result.
                self.store.get_upload(owner, id)
                atomic_json(self.root / (id+'.json'), value)
        except Failure:
            pass
        finally:
            with self.lock:
                self.active.pop(id, None)
            self.slots.release()

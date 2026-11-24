"""Configuration and message validation for the scheduled native Qwen worker."""
import base64
import binascii
import os
import re
from pathlib import Path

MODEL_ID = 'cbgpu-qwen38'
CONTEXT_WINDOW = 65536
MAX_OUTPUT = 4096
IMAGE_PREFIXES = ('data:image/png;base64,', 'data:image/jpeg;base64,', 'data:image/webp;base64,')
RUNTIME_ENV = ('CBGPU_AI_QWEN38_RUNTIME', 'CBGPU_AI_QWEN38_ASSETS',
               'CBGPU_AI_QWEN38_GPU_UUID', 'CBGPU_AI_QWEN38_VULKAN_INDEX')


def config():
    env = {key: os.environ.get(key, '') for key in RUNTIME_ENV}
    worker = Path(os.environ.get('CBGPU_AI_QWEN38_WORKER', ''))
    runtime, assets = map(Path, (env[RUNTIME_ENV[0]], env[RUNTIME_ENV[1]]))
    gpu, index = env[RUNTIME_ENV[2]], env[RUNTIME_ENV[3]]
    if not (worker.is_absolute() and worker.is_file() and os.access(worker, os.X_OK)
            and runtime.is_absolute() and assets.is_absolute()
            and all(path.is_file() for path in (runtime / 'llama-server', runtime / 'vulkan/libggml-vulkan.so',
                    assets / 'Qwen3.8-27B-UD-IQ4_XS.gguf', assets / 'mmproj-F16.gguf'))
            and re.fullmatch(r'GPU-[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}', gpu)
            and index in ('0', '1', '2', '3')):
        return None
    return worker, gpu, env


def image_dimensions(header, data):
    if header == 'data:image/png;base64' and data.startswith(b'\x89PNG\r\n\x1a\n') and data[12:16] == b'IHDR' and len(data) >= 24:
        return int.from_bytes(data[16:20], 'big'), int.from_bytes(data[20:24], 'big')
    if header == 'data:image/jpeg;base64' and data.startswith(b'\xff\xd8'):
        offset = 2
        while offset + 4 < len(data) and data[offset] == 0xff:
            marker = data[offset + 1]
            if marker == 0xff:
                offset += 1
                continue
            size = int.from_bytes(data[offset + 2:offset + 4], 'big')
            if size < 2 or offset + size + 2 > len(data) or marker in (0xda, 0xd9):
                break
            if marker in (0xc0, 0xc1, 0xc2, 0xc3, 0xc5, 0xc6, 0xc7, 0xc9, 0xca, 0xcb, 0xcd, 0xce, 0xcf) and size >= 7:
                return int.from_bytes(data[offset + 7:offset + 9], 'big'), int.from_bytes(data[offset + 5:offset + 7], 'big')
            offset += size + 2
    if header == 'data:image/webp;base64' and data[:4] == b'RIFF' and data[8:12] == b'WEBP':
        kind = data[12:16]
        if kind == b'VP8X' and len(data) >= 30:
            return 1 + int.from_bytes(data[24:27], 'little'), 1 + int.from_bytes(data[27:30], 'little')
        if kind == b'VP8L' and len(data) >= 25 and data[20] == 0x2f:
            bits = int.from_bytes(data[21:25], 'little')
            return 1 + (bits & 0x3fff), 1 + ((bits >> 14) & 0x3fff)
        if kind == b'VP8 ' and len(data) >= 30 and data[23:26] == b'\x9d\x01\x2a':
            return int.from_bytes(data[26:28], 'little') & 0x3fff, int.from_bytes(data[28:30], 'little') & 0x3fff
    raise ValueError('Embedded image does not match its media type')


def multipart(content):
    if not isinstance(content, list) or not 1 <= len(content) <= 16:
        raise ValueError('Use text or image chat messages')
    parts, length, images = [], 0, 0
    for part in content:
        if not isinstance(part, dict):
            raise ValueError('Invalid chat content')
        if part.get('type') == 'text' and isinstance(part.get('text'), str):
            length += len(part['text'])
            parts.append({'type': 'text', 'text': part['text']})
            continue
        image = part.get('image_url')
        url = image.get('url', '') if isinstance(image, dict) else ''
        if part.get('type') != 'image_url' or not isinstance(url, str) or not url.startswith(IMAGE_PREFIXES):
            raise ValueError('Only embedded PNG, JPEG and WebP images are supported')
        header, encoded = url.split(',', 1)
        if len(encoded) > 2_000_000:
            raise ValueError('Use an image smaller than 1.5 MB')
        try:
            content_bytes = base64.b64decode(encoded, validate=True)
        except (ValueError, binascii.Error) as error:
            raise ValueError('Invalid embedded image') from error
        width, height = image_dimensions(header, content_bytes)
        if not 1 <= width <= 8192 or not 1 <= height <= 8192 or width * height > 16_777_216:
            raise ValueError('Use an image up to 16 megapixels')
        images += 1
        parts.append({'type': 'image_url', 'image_url': {'url': url}})
    return parts, length, images

import base64
import os
import struct
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from catalog import models
from core import Failure, Store
from qwen_compat import MODEL_ID, devices, multipart


class QwenCompatibilityTest(unittest.TestCase):
    def test_device_map_uses_distinct_uuid_and_vulkan_pairs(self):
        first = 'GPU-00000000-0000-4000-8000-000000000001'
        second = 'GPU-00000000-0000-4000-8000-000000000002'
        with patch.dict(os.environ, {'CBGPU_AI_QWEN38_DEVICES': f'{first}=0,{second}=2'}, clear=True):
            self.assertEqual(devices(), {first: '0', second: '2'})
        with patch.dict(os.environ, {'CBGPU_AI_QWEN38_DEVICES': f'{first}=0,{second}=0'}, clear=True):
            self.assertEqual(devices(), {})
        with patch.dict(os.environ, {'CBGPU_AI_QWEN38_GPU_UUID': first, 'CBGPU_AI_QWEN38_VULKAN_INDEX': '0'}, clear=True):
            self.assertEqual(devices(), {first: '0'})

    def test_qwen_requires_a_configured_worker(self):
        with patch.dict(os.environ, {}, clear=True), tempfile.TemporaryDirectory() as state:
            self.assertNotIn(MODEL_ID, [entry['id'] for entry in models(state)])
            with self.assertRaises(Failure) as error:
                Store.validate(None, 'owner', 'chat', {'model': MODEL_ID, 'messages': [{'role': 'user', 'content': 'hello'}]})
            self.assertEqual(error.exception.status, 503)

    def test_long_messages_apply_only_to_qwen(self):
        message = {'role': 'user', 'content': 'long text ' * 10_000}
        with self.assertRaises(Failure) as error:
            Store.validate(None, 'owner', 'chat', {'model': 'cbgpu-chat', 'messages': [message]})
        self.assertEqual(error.exception.status, 413)
        with patch('qwen_compat.config', return_value=(Path('/run/qwen'), 'gpu', {})):
            result = Store.validate(None, 'owner', 'chat', {'model': MODEL_ID, 'messages': [message]})
            self.assertEqual(result['messages'], [message])

    def test_image_shape_and_source_are_bounded(self):
        def image(url):
            return [{'type': 'image_url', 'image_url': {'url': url}}]

        def png_header(width, height):
            return 'data:image/png;base64,' + base64.b64encode(
                b'\x89PNG\r\n\x1a\n' + b'\0\0\0\x0dIHDR' + struct.pack('!II', width, height)
            ).decode()

        self.assertEqual(multipart(image(png_header(256, 512)))[2], 1)
        for source in (png_header(8192, 8192), 'https://example.com/photo.png', 'data:image/png;base64,broken'):
            with self.subTest(source=source), self.assertRaises(ValueError):
                multipart(image(source))

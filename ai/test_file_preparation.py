import tempfile
import time
import unittest
import uuid

from core import Failure, Store, atomic_json
from chat_files import evidence
from file_preparation import FilePreparation


class PreparationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.store = Store(self.temp.name)
        self.owner = str(uuid.uuid4())
        self.other = str(uuid.uuid4())

    def tearDown(self):
        self.store.db.close()
        self.temp.cleanup()

    def test_private_preparation_survives_restart_and_cleanup(self):
        upload = self.store.upload(self.owner, 'report.txt', [b'The voltage is 12.5 V.'])
        files = self.store.preparation
        with self.assertRaises(Failure):
            files.start(self.other, upload['id'])
        files.start(self.owner, upload['id'])
        for _ in range(500):
            state = files.status(self.owner, upload['id'])
            if state['state'] != 'processing':
                break
            time.sleep(.01)
        self.assertEqual(state['state'], 'ready')
        cached = FilePreparation(self.store)
        self.assertEqual(cached.status(self.owner, upload['id'])['state'], 'ready')
        with self.assertRaises(Failure):
            cached.read(self.other, upload['id'])
        self.assertIn('12.5', evidence(self.store, self.owner, [upload['id']], 'voltage')[0]['excerpt'])
        with self.assertRaises(Failure):
            self.store.delete_upload(self.other,upload['id'])
        self.store.delete_upload(self.owner,upload['id'])
        self.assertFalse((files.root/(upload['id']+'.json')).exists())

    def test_interrupted_processing_can_be_retried(self):
        upload = self.store.upload(self.owner, 'note.txt', [b'Private note'])
        files = self.store.preparation
        atomic_json(files.root/(upload['id']+'.json'), {'version': 1, 'state': 'processing'})
        self.assertEqual(files.status(self.owner, upload['id'])['state'], 'failed')
        files.start(self.owner, upload['id'])
        for _ in range(500):
            if files.status(self.owner, upload['id'])['state'] == 'ready':
                break
            time.sleep(.01)
        self.assertEqual(files.status(self.owner, upload['id'])['state'], 'ready')

    def test_invalid_upload_and_unreadable_text_have_explicit_errors(self):
        with self.assertRaises(Failure):
            self.store.preparation.start(self.owner, '../outside')
        upload = self.store.upload(self.owner, 'bad.txt', [b'\xff\xff'])
        self.store.preparation.start(self.owner, upload['id'])
        for _ in range(500):
            state = self.store.preparation.status(self.owner, upload['id'])
            if state['state'] != 'processing':
                break
            time.sleep(.01)
        self.assertEqual(state['state'], 'failed')
        self.assertTrue(state['error'])

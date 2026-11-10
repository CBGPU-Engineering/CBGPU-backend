import tempfile,unittest,uuid
from pathlib import Path
from core import Store,Failure
class ArtifactSafety(unittest.TestCase):
 def setUp(self):self.tmp=tempfile.TemporaryDirectory();self.store=Store(self.tmp.name);self.owner=str(uuid.uuid4());self.job=self.store.enqueue(self.owner,'python',{'code':'pass'});self.folder=self.store.root/'jobs'/self.job['id'];self.output=self.folder/'workspace/output';self.output.mkdir(parents=True)
 def tearDown(self):self.store.db.close();self.tmp.cleanup()
 def test_output_root_symlink_cannot_expose_host_files(self):
  outside=self.store.root/'outside';outside.mkdir();(outside/'private.txt').write_text('private fixture');self.output.rmdir();self.output.symlink_to(outside,target_is_directory=True);self.assertEqual(self.store.artifacts(self.folder),[])
  with self.assertRaises(Failure):self.store.artifact(self.owner,self.job['id'],'private.txt')
 def test_open_descriptor_survives_path_swap_without_following_replacement(self):
  (self.output/'answer.txt').write_text('owned output');file=self.store.artifact(self.owner,self.job['id'],'answer.txt');(self.output/'answer.txt').unlink();outside=self.store.root/'private.txt';outside.write_text('outside fixture');(self.output/'answer.txt').symlink_to(outside)
  try:self.assertEqual(file.read_text(),'owned output')
  finally:file.close()
  with self.assertRaises(Failure):self.store.artifact(self.owner,self.job['id'],'answer.txt')
 def test_named_pipes_are_rejected_without_blocking(self):
  import os
  os.mkfifo(self.output/'pipe')
  with self.assertRaises(Failure):self.store.artifact(self.owner,self.job['id'],'pipe')

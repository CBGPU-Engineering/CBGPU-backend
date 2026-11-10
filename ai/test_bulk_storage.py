import tempfile,unittest,uuid
from pathlib import Path
from unittest.mock import patch
from core import Store,Failure
from resources import prepare_workspace
from scheduler import Scheduler
class BulkStorageTests(unittest.TestCase):
 def test_new_bulk_files_use_second_volume_and_old_data_stays(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=Path(tmp);state=root/'state';bulk=root/'data';owner=str(uuid.uuid4());old=Store(state);legacy=old.upload(owner,'old.txt',[b'old']);old.db.close();store=Store(state,bulk)
   try:
    upload=store.upload(owner,'new.txt',[b'new']);self.assertFalse((state/'uploads'/legacy['id']).is_symlink());self.assertEqual((state/'uploads'/legacy['id']).read_bytes(),b'old');self.assertEqual((state/'uploads'/upload['id']).resolve(),bulk/'uploads'/upload['id'])
    job=store.enqueue(owner,'python',{'code':"print('fixture')"});workspace=prepare_workspace(store,job,{'code':"print('fixture')"});self.assertEqual(workspace.resolve(),bulk/'workspaces'/job['id']);(workspace/'output'/'result.txt').write_text('owned');artifact=store.artifact(owner,job['id'],'result.txt')
    try:self.assertEqual(artifact.read_text(),'owned')
    finally:artifact.close()
    store.remove_job_files(job['id']);store.remove_upload(upload['id']);self.assertFalse((bulk/'workspaces'/job['id']).exists());self.assertFalse((bulk/'uploads'/upload['id']).exists());self.assertTrue((state/'uploads'/legacy['id']).exists())
   finally:store.db.close()
 def test_missing_mount_fails_closed(self):
  with tempfile.TemporaryDirectory() as tmp,patch.dict('os.environ',{'CBGPU_AI_REQUIRE_DATA_MOUNT':'1'}),patch('os.path.ismount',return_value=False):
   store=Store(Path(tmp)/'state',Path(tmp)/'data')
   try:
    with self.assertRaises(Failure):store.upload(str(uuid.uuid4()),'new.txt',[b'x'])
   finally:store.db.close()
 def test_workers_cannot_take_another_workers_device_or_job(self):
  with tempfile.TemporaryDirectory() as tmp:
   store=Store(tmp);job=store.enqueue(str(uuid.uuid4()),'chat',{'messages':[{'role':'user','content':'test'}]});a=Scheduler(store,'/none','/none','none');b=Scheduler(store,'/none','/none','none');store.ai_leases['GPU-test']=job['id'];store.ai_active.add(job['id'])
   try:
    with patch('resources.start_task') as start:b.execute(job,{'uuid':'GPU-test'});b.execute(job,{'uuid':'GPU-other'});start.assert_not_called()
    store.execute("UPDATE jobs SET state='deleted' WHERE id=?",(job['id'],));b.cleanup();self.assertTrue((store.root/'jobs'/job['id']).exists())
   finally:store.db.close()

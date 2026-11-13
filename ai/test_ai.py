import io,json,sys,tempfile,threading,unittest,uuid,zipfile,os,sqlite3
from pathlib import Path
from unittest.mock import patch,MagicMock
from types import SimpleNamespace
sys.path.insert(0,str(Path(__file__).parent))
from core import Store,Failure,extract_zip,atomic_json
from scheduler import Scheduler
from server import AIHTTPServer
import urllib.request,urllib.error
OWNER='11111111-1111-4111-8111-111111111111';OTHER='22222222-2222-4222-8222-222222222222'
class StoreTests(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.store=Store(self.tmp.name);self.disk=patch('shutil.disk_usage',return_value=SimpleNamespace(free=100*1024**3));self.disk.start()
 def tearDown(self):self.store.db.close();self.tmp.cleanup();self.disk.stop()
 def chat(self):return self.store.enqueue(OWNER,'chat',{'messages':[{'role':'user','content':'synthetic'}]})
 def test_owner_isolation(self):
  job=self.chat();self.assertEqual(self.store.job(OWNER,job['id'])['state'],'queued')
  with self.assertRaises(Failure):self.store.job(OTHER,job['id'])
  file=self.store.upload(OWNER,'sample.wav',[b'audio'])
  with self.assertRaises(Failure):self.store.enqueue(OTHER,'transcribe',{'upload_id':file['id']})
 def test_key_secret_is_one_time_and_revocable(self):
  value=self.store.create_key(OWNER,'test');self.assertEqual(self.store.key_owner(value['key']),OWNER);self.assertNotIn('key',self.store.keys(OWNER)[0]);self.assertNotIn(value['key'],json.dumps(self.store.keys(OWNER)));self.assertEqual(self.store.keys(OTHER),[])
  self.store.execute('UPDATE api_keys SET revoked=1 WHERE id=?',(value['id'],))
  with self.assertRaises(Failure):self.store.key_owner(value['key'])
 def test_queue_and_model_input_limits(self):
  for _ in range(5):self.chat()
  with self.assertRaises(Failure):self.chat()
  for kind,body in [('chat',{'messages':[{'role':'tool','content':'x'}]}),('embed',{'input':['x']*65}),('python',{'code':'pass','entrypoint':'../escape.py'}),('python',{'code':'pass','minutes':121}),('search',{'query':'q','candidates':[{'id':'x','text':'a'*8001}]})]:
   with self.assertRaises(Failure):self.store.validate(OWNER,kind,body)
 def test_upload_stream_is_private_and_single_use(self):
  value=self.store.upload(OWNER,'../../clip.wav',[b'first',b'second']);self.assertEqual(value['name'],'clip.wav');self.assertEqual(value['size'],11);job=self.store.enqueue(OWNER,'transcribe',{'upload_id':value['id']});self.assertEqual(self.store.job(OWNER,job['id'])['kind'],'transcribe')
  with self.assertRaises(Failure):self.store.enqueue(OWNER,'transcribe',{'upload_id':value['id']})
 def test_native_downloads_are_owner_bound_and_expire(self):
  job=self.chat();folder=self.store.root/'jobs'/job['id'];atomic_json(folder/'result.json',{'text':'private result'});self.store.execute("UPDATE jobs SET state='succeeded' WHERE id=?",(job['id'],));ticket=self.store.download_ticket(OWNER,job['id'],'text');self.assertEqual(self.store.download(OWNER,ticket['id']),b'private result')
  with self.assertRaises(Failure):self.store.download(OTHER,ticket['id'])
  self.store.execute('UPDATE downloads SET expires=0 WHERE id=?',(ticket['id'],))
  with self.assertRaises(Failure):self.store.download(OWNER,ticket['id'])
 def test_upload_stops_before_exhausting_host_storage(self):
  with patch('shutil.disk_usage',return_value=SimpleNamespace(free=1024)):
   with self.assertRaises(Failure):self.store.upload(OWNER,'a.wav',[b'a'])
  self.assertEqual(list((Path(self.tmp.name)/'uploads').iterdir()),[])
 def test_zip_traversal_and_symlinks_are_rejected(self):
  for name,symlink in [('../escape',False),('/absolute',False),('symlink',True)]:
   data=io.BytesIO()
   with zipfile.ZipFile(data,'w') as z:
    i=zipfile.ZipInfo(name)
    if symlink:i.external_attr=(0o120777)<<16
    z.writestr(i,'target')
   data.seek(0)
   with self.assertRaises(Failure):extract_zip(data,Path(self.tmp.name)/'work')
 def test_artifacts_cannot_escape_owner_workspace(self):
  job=self.chat();root=self.store.root/'jobs'/job['id']/'workspace/output';root.mkdir(parents=True);(root/'answer.txt').write_text('answer');(root/'escape').symlink_to('/etc/passwd');self.assertEqual(self.store.artifact(OWNER,job['id'],'answer.txt').read_text(),'answer')
  for owner,name in [(OTHER,'answer.txt'),(OWNER,'../input.json'),(OWNER,'escape')]:
   with self.assertRaises(Failure):self.store.artifact(owner,job['id'],name)
class SchedulerTests(StoreTests):
 def run_preemption(self,reason):
  job=self.chat();folder=self.store.root/'jobs'/job['id'];(folder/'workspace').mkdir();(folder/'workspace/checkpoint').write_text('saved')
  device={'index':0,'uuid':'GPU-test','available':True,'reserved':False};scheduler=Scheduler(self.store,'/unused/python','/unused/runner','trusted-image');scheduler.stop_event.wait=lambda _:False
  process=MagicMock();process.pid=123;process.poll.return_value=None
  checks=[([device],[])]
  if reason=='reservation':checks.append(([{**device,'reserved':True}],[]))
  elif reason=='user':checks.append(([device],[{'uuid':'GPU-test','pid':999}]))
  else:checks.append(RuntimeError('database unavailable'))
  with patch('resources.snapshot',side_effect=checks),patch('resources.start_task',return_value=(process,MagicMock())),patch('resources.own_pids',return_value={123}),patch('resources.stop_task') as stop:
   scheduler.execute(job,device);stop.assert_called_once_with(process,job)
  self.assertEqual(self.store.job(OWNER,job['id'])['state'],'paused');self.assertEqual((folder/'workspace/checkpoint').read_text(),'saved')
 def test_reservations_preempt_only_the_ai_worker(self):self.run_preemption('reservation')
 def test_notebook_process_preempts_ai(self):self.run_preemption('user')
 def test_unknown_reservations_pause_instead_of_running_blind(self):self.run_preemption('unknown')
 def test_reserved_device_is_never_started(self):
  job=self.chat();s=Scheduler(self.store,'x','x','x');device={'uuid':'GPU-x','available':False}
  with patch('resources.snapshot',return_value=([device],[])),patch('resources.start_task') as start:s.execute(job,device);start.assert_not_called()
  self.assertEqual(self.store.job(OWNER,job['id'])['state'],'queued')
 def test_cancelled_job_cannot_be_resurrected_during_start(self):
  job=self.chat();self.store.execute("UPDATE jobs SET state='cancelled' WHERE id=?",(job['id'],));s=Scheduler(self.store,'x','x','x');device={'index':0,'uuid':'GPU-x','available':True}
  with patch('resources.snapshot',return_value=([device],[])),patch('resources.start_task') as start:s.execute(job,device);start.assert_not_called()
  self.assertEqual(self.store.job(OWNER,job['id'])['state'],'cancelled')
class SchedulerFaultTests(unittest.TestCase):
 def test_file_handle_exhaustion_is_reported_without_private_exception_text(self):
  import errno
  store=SimpleNamespace(lock=threading.RLock());scheduler=Scheduler(store,'x','x','x')
  scheduler.stop_event.wait=lambda _:scheduler.stop_event.set()
  with patch('resources.snapshot',side_effect=OSError(errno.EMFILE,'private diagnostic')),self.assertLogs('scheduler',level='ERROR') as logs:scheduler.loop()
  self.assertEqual(scheduler.health['reason'],'file_handles_exhausted')
  self.assertNotIn('private diagnostic',json.dumps(scheduler.health)+str(logs.output))
 def test_queue_failure_is_not_misreported_as_gpu_failure(self):
  store=SimpleNamespace(lock=threading.RLock());scheduler=Scheduler(store,'x','x','x')
  scheduler.stop_event.wait=lambda _:scheduler.stop_event.set()
  with patch('resources.snapshot',return_value=([],[])),patch.object(store,'execute',create=True,side_effect=RuntimeError('private query')),self.assertLogs('scheduler',level='ERROR'):scheduler.loop()
  self.assertEqual(scheduler.health['reason'],'queue_failed')
  self.assertNotIn('reservation',scheduler.health['detail'])

class CorpusTests(unittest.TestCase):
 def test_background_corpus_connections_close_without_waiting_for_gc(self):
  from corpus import Corpus
  original=sqlite3.connect;opened=[]
  class TrackedConnection(sqlite3.Connection):
   def close(self):
    self.was_closed=True
    return super().close()
  def connect(*args,**kwargs):
   connection=original(*args,**kwargs,factory=TrackedConnection);connection.was_closed=False;opened.append(connection);return connection
  with tempfile.TemporaryDirectory() as folder:
   try:
    with patch('corpus.sqlite3.connect',side_effect=connect):
     corpus=Corpus(folder,'/unused/python','/unused/worker');cycles=0
     def tick(_):
      nonlocal cycles
      cycles+=1
      if cycles>=80:corpus.stop.set()
     corpus.stop.wait=tick
     with patch.object(corpus,'metadata',return_value={'materials':[],'papers':[]}):corpus.loop()
    self.assertGreaterEqual(len(opened),161)
    self.assertTrue(all(c.was_closed for c in opened),'SQLite transaction context does not close a connection; close every indexing connection explicitly')
    with original(corpus.path) as db:self.assertEqual(db.execute('SELECT count(*) FROM documents').fetchone()[0],0)
   finally:
    for c in opened:c.close()
 def test_corpus_does_not_include_private_notes_and_removes_revoked_sources(self):
  from corpus import Corpus
  with tempfile.TemporaryDirectory() as folder:
   corpus=Corpus(folder,'/unused/python','/unused/worker');fake={'materials':[],'papers':[]}
   with sqlite3.connect(corpus.path) as db:
    db.execute("INSERT INTO documents VALUES('material','removed','v1','ready',0)");db.execute("INSERT INTO chunks VALUES('material','removed',0,'previously shared','[]')")
   corpus.stop.wait=lambda _:corpus.stop.set() or True
   with patch.object(corpus,'metadata',return_value=fake):corpus.loop()
   with sqlite3.connect(corpus.path) as db:self.assertEqual(db.execute('SELECT count(*) FROM chunks').fetchone()[0],0)
   query=[]
   with patch('resources.run',side_effect=lambda args:query.append(args[-1]) or '{"materials":[],"papers":[]}'):corpus.metadata()
   self.assertIn("status='ready'",query[0]);self.assertIn("status='published'",query[0]);self.assertNotIn('research_notes',query[0])

class HTTPTests(StoreTests):
 def test_internal_auth_owner_scoping_and_chunked_upload(self):
  server=AIHTTPServer(('127.0.0.1',0),self.store,SimpleNamespace(health={'status':'ok'},current=None),'synthetic-internal-key');thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();base='http://127.0.0.1:'+str(server.server_port)
  def request(path,owner=OWNER,key='synthetic-internal-key',body=None):return urllib.request.urlopen(urllib.request.Request(base+path,data=body,headers={'x-cbgpu-ai-key':key,'x-cbgpu-owner':owner,'Content-Type':'application/json'}),timeout=5)
  try:
   with self.assertRaises(urllib.error.HTTPError) as err:request('/jobs',key='bad')
   self.assertEqual(err.exception.code,401);err.exception.close()
   job=self.chat()
   with self.assertRaises(urllib.error.HTTPError) as err:request('/jobs/'+job['id'],OTHER)
   self.assertEqual(err.exception.code,404);err.exception.close()
   with request('/uploads?name=test.wav',body=iter([b'one',b'two'])) as response:self.assertEqual(json.load(response)['size'],6)
  finally:server.shutdown();server.server_close();thread.join()
if __name__=='__main__':unittest.main()

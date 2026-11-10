import json,tempfile,time,unittest,uuid
from pathlib import Path
from core import Store,Failure,atomic_json
from conversations import Conversations

class ConversationTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.store=Store(self.tmp.name);self.chats=Conversations(self.store);self.owner=str(uuid.uuid4());self.other=str(uuid.uuid4())
    def tearDown(self):self.store.db.close();self.tmp.cleanup()
    def completed(self,chat,text='Synthetic answer'):
        job=chat['messages'][-1]['job_id'];atomic_json(self.store.root/'jobs'/job/'result.json',{'choices':[{'message':{'content':text},'finish_reason':'stop'}]});self.store.execute("UPDATE jobs SET state='succeeded' WHERE id=?",(job,));return job
    def test_owner_bound_read_write_and_history(self):
        chat=self.chats.create(self.owner);self.assertEqual(self.chats.list(self.other),[])
        for action in [lambda:self.chats.get(self.other,chat['id']),lambda:self.chats.rename(self.other,chat['id'],'no'),lambda:self.chats.delete(self.other,chat['id']),lambda:self.chats.send(self.other,chat['id'],'no')]:
            with self.assertRaises(Failure) as error:action()
            self.assertEqual(error.exception.status,404)
        self.chats.rename(self.owner,chat['id'],'My title');self.assertEqual(self.chats.list(self.owner)[0]['title'],'My title')
    def test_context_completion_restart_and_duplicate_prevention(self):
        chat=self.chats.create(self.owner);chat=self.chats.send(self.owner,chat['id'],'First question')
        with self.assertRaises(Failure) as error:self.chats.send(self.owner,chat['id'],'Duplicate')
        self.assertEqual(error.exception.status,409);first=self.completed(chat)
        read=self.chats.get(self.owner,chat['id']);self.assertEqual(read['messages'][-1]['content'],'Synthetic answer')
        second=self.chats.send(self.owner,chat['id'],'Follow up');job=second['messages'][-1]['job_id'];saved=json.loads((self.store.root/'jobs'/job/'input.json').read_text())
        self.assertEqual([m['role'] for m in saved['messages']],['system','user','assistant','user']);self.assertEqual(saved['messages'][2]['content'],'Synthetic answer')
        self.assertEqual(Conversations(self.store).get(self.owner,chat['id'])['messages'][0]['content'],'First question')
    def test_delete_cancels_related_work_and_cannot_resurrect(self):
        chat=self.chats.create(self.owner);chat=self.chats.send(self.owner,chat['id'],'Delete me');job=chat['messages'][-1]['job_id'];self.chats.delete(self.owner,chat['id'])
        self.assertEqual(self.store.execute('SELECT state FROM jobs WHERE id=?',(job,)).fetchone()[0],'deleted');self.assertEqual(self.chats.list(self.owner),[])
        with self.assertRaises(Failure):self.chats.get(self.owner,chat['id'])
    def test_cancelled_turn_is_not_silently_used_as_answered_context(self):
        chat=self.chats.create(self.owner);chat=self.chats.send(self.owner,chat['id'],'Unanswered question');self.store.execute("UPDATE jobs SET state='cancelled' WHERE id=?",(chat['messages'][-1]['job_id'],));chat=self.chats.send(self.owner,chat['id'],'New question')
        payload=json.loads((self.store.root/'jobs'/chat['messages'][-1]['job_id']/'input.json').read_text());self.assertEqual([m['role'] for m in payload['messages']],['system','user'])
    def test_context_and_input_are_bounded(self):
        chat=self.chats.create(self.owner)
        with self.assertRaises(Failure):self.chats.send(self.owner,chat['id'],'a'*8001)
        for i in range(4):chat=self.chats.send(self.owner,chat['id'],'Q'*7000);self.completed(chat,'A'*7000)
        chat=self.chats.send(self.owner,chat['id'],'Follow up');payload=json.loads((self.store.root/'jobs'/chat['messages'][-1]['job_id']/'input.json').read_text());self.assertLess(sum(len(m['content']) for m in payload['messages']),24000);self.assertTrue(chat['context_truncated'])

    def test_http_keep_alive_consumes_create_and_cancel_bodies(self):
        import http.client,threading
        from types import SimpleNamespace
        from server import AIHTTPServer
        server=AIHTTPServer(('127.0.0.1',0),self.store,SimpleNamespace(health={},current=None),'test-key');thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start();conn=http.client.HTTPConnection('127.0.0.1',server.server_address[1],timeout=5)
        headers={'x-cbgpu-ai-key':'test-key','x-cbgpu-owner':self.owner,'Content-Type':'application/json'}
        try:
            conn.request('POST','/chats','{}',headers);response=conn.getresponse();self.assertEqual(response.status,201);chat=json.loads(response.read())['chat']
            conn.request('GET','/chats',headers=headers);response=conn.getresponse();self.assertEqual(response.status,200);response.read()
            conn.request('POST','/chats/'+chat['id']+'/messages',json.dumps({'content':'test'}),headers);response=conn.getresponse();self.assertEqual(response.status,202);job=json.loads(response.read())['chat']['messages'][-1]['job_id']
            conn.request('POST','/jobs/'+job+'/cancel','{}',headers);response=conn.getresponse();self.assertEqual(response.status,200);response.read()
            conn.request('GET','/chats/'+chat['id'],headers=headers);response=conn.getresponse();self.assertEqual(response.status,200);self.assertEqual(json.loads(response.read())['chat']['messages'][-1]['state'],'cancelled')
        finally:conn.close();server.shutdown();server.server_close();thread.join(timeout=2)

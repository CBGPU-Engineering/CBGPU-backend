import json,sys,tempfile,types,unittest,uuid
from pathlib import Path
from unittest.mock import patch
from core import Store,atomic_json,Failure
from conversations import Conversations
from incremental import PartialWriter,complete
class StreamingTests(unittest.TestCase):
 def test_partial_is_live_private_and_not_committed_as_final(self):
  with tempfile.TemporaryDirectory() as tmp:
   store=Store(tmp)
   try:
    chats=Conversations(store);owner=str(uuid.uuid4());chat=chats.send(owner,chats.create(owner)['id'],'Question');job=chat['messages'][-1]['job_id'];folder=store.root/'jobs'/job
    store.execute("UPDATE jobs SET state='running' WHERE id=?",(job,));writer=PartialWriter(folder);writer('안녕');writer.flush()
    self.assertEqual(chats.live(owner,chat['id'])['message']['content'],'안녕')
    self.assertEqual(store.execute("SELECT content FROM chat_messages WHERE job_id=?",(job,)).fetchone()[0],'')
    with self.assertRaises(Failure):chats.live(str(uuid.uuid4()),chat['id'])
    writer('하세요');writer.flush();self.assertEqual(chats.get(owner,chat['id'])['messages'][-1]['content'],'안녕하세요')
    atomic_json(folder/'result.json',{'choices':[{'message':{'content':'안녕하세요!'},'finish_reason':'stop'}]});store.execute("UPDATE jobs SET state='succeeded' WHERE id=?",(job,))
    self.assertEqual(chats.live(owner,chat['id'])['message']['content'],'안녕하세요!')
    self.assertEqual(Conversations(store).get(owner,chat['id'])['messages'][-1]['content'],'안녕하세요!')
   finally:store.db.close()
 def test_runner_forwards_real_deltas_before_generation_finishes(self):
  received=[]
  class Model:
   def create_chat_completion(self,**options):
    self.options=options
    def chunks():
     yield {'choices':[{'delta':{'content':'안녕'},'finish_reason':None}]}
     self_seen=list(received);assert self_seen==['안녕']
     yield {'choices':[{'delta':{'content':'하세요'},'finish_reason':None}]}
     yield {'choices':[{'delta':{},'finish_reason':'length'}]}
    return chunks()
  with patch.dict(sys.modules,{'llama_cpp':types.SimpleNamespace(LogitsProcessorList=list)}):
   model=Model();result=complete(model,{'messages':[]},received.append)
  self.assertTrue(model.options['stream']);self.assertEqual(received,['안녕','하세요']);self.assertEqual(result['choices'][0]['message']['content'],'안녕하세요');self.assertEqual(result['choices'][0]['finish_reason'],'length')

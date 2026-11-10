import json,tempfile,unittest,uuid
from pathlib import Path
from core import Store,Failure
from tool_protocol import prepare,decode,validate_tools
from catalog import models,model_file
TOOLS=[{'type':'function','function':{'name':'read','description':'Read a file','parameters':{'type':'object','properties':{'filePath':{'type':'string'}},'required':['filePath']}}}]
class ToolTests(unittest.TestCase):
 def result(self,value):return {'choices':[{'message':{'role':'assistant','content':json.dumps(value)},'finish_reason':'stop'}]}
 def test_tool_proposals_are_standard_json_not_execution(self):
  value=decode(self.result({'type':'tool','name':'read','arguments':{'filePath':'README.md'}}),TOOLS,'required');call=value['choices'][0]['message']['tool_calls'][0];self.assertEqual(call['function']['name'],'read');self.assertEqual(json.loads(call['function']['arguments']),{'filePath':'README.md'});self.assertEqual(value['choices'][0]['finish_reason'],'tool_calls')
  for value in [{'type':'tool','name':'arbitrary_host_shell','arguments':{}},{'type':'tool','name':'read','arguments':'not an object'},{'type':'final','content':'invented answer'}]:
   with self.assertRaises(ValueError):decode(self.result(value),TOOLS,'required')
 def test_tool_result_roundtrip_is_text_only(self):
  messages=[{'role':'user','content':'Read README'},{'role':'assistant','content':None,'tool_calls':[{'id':'call_fixture','type':'function','function':{'name':'read','arguments':'{}'}}]},{'role':'tool','content':'Synthetic file text','tool_call_id':'call_fixture'}];out,mode=prepare(messages,TOOLS,'auto');self.assertTrue(mode);self.assertTrue(all(isinstance(m['content'],str) and m['role'] in ['system','assistant','user'] for m in out));self.assertIn('Synthetic file text',out[-1]['content']);self.assertEqual(decode(self.result({'type':'final','content':'Done'}),TOOLS,'auto')['choices'][0]['message']['content'],'Done')
 def test_request_limits_model_whitelist_and_pinned_paths(self):
  with tempfile.TemporaryDirectory() as root:
   store=Store(root)
   try:
    for id in ['cbgpu-chat','cbgpu-coder','cbgpu-pro']:
     value=store.validate(str(uuid.uuid4()),'chat',{'model':id,'messages':[{'role':'user','content':'Fixture'}],'tools':TOOLS});self.assertEqual(value['model'],id);self.assertTrue(str(model_file(root,id)).startswith(root))
    with self.assertRaises(Failure):store.validate(str(uuid.uuid4()),'chat',{'model':'../../host-secret','messages':[{'role':'user','content':'Fixture'}]})
    with self.assertRaises(Failure):store.validate(str(uuid.uuid4()),'chat',{'messages':[{'role':'user','content':'Fixture'}],'tools':TOOLS*25})
    self.assertEqual(len([m for m in models(root) if 'text' in m.get('capabilities',[])]),3)
   finally:store.db.close()

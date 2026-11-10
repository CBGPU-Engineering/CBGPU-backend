import json,tempfile,time,unittest,uuid
from core import Store,Failure,atomic_json
from robotics import Robotics
CAPS=[{'name':'navigate_to','description':'Navigate to a known waypoint','parameters':{'type':'object','properties':{'waypoint':{'type':'string','maxLength':32,'enum':['desk','charging_station']}},'required':['waypoint'],'additionalProperties':False},'max_duration_s':10}]
class RoboticsTests(unittest.TestCase):
 def setUp(self):self.tmp=tempfile.TemporaryDirectory();self.store=Store(self.tmp.name);self.robotics=Robotics(self.store);self.owner=str(uuid.uuid4());self.other=str(uuid.uuid4())
 def tearDown(self):self.store.db.close();self.tmp.cleanup()
 def create(self,**fields):return self.robotics.create(self.owner,{'instruction':'Go to charging station','robot_id':'test-robot','capabilities':CAPS,'observations':{'location':'desk'},'observed_at':time.time(),**fields})
 def result(self,id,plan):
  atomic_json(self.store.root/'jobs'/id/'result.json',{'choices':[{'message':{'tool_calls':[{'function':{'name':'propose_robot_plan','arguments':json.dumps(plan)}}]}}]});self.store.execute("UPDATE jobs SET state='succeeded' WHERE id=?",(id,))
 def plan(self):return {'summary':'Move to charging station','needs_clarification':False,'question':'','steps':[{'capability':'navigate_to','arguments':{'waypoint':'charging_station'},'duration_s':8}]}
 def test_private_async_plan_and_stable_envelope(self):
  job=self.create();self.assertEqual(job['state'],'queued')
  for action in [lambda:self.robotics.get(self.other,job['id']),lambda:self.robotics.cancel(self.other,job['id'])]:
   with self.assertRaises(Failure) as e:action()
   self.assertEqual(e.exception.status,404)
  self.result(job['id'],self.plan());mission=self.robotics.get(self.owner,job['id'])['mission'];self.assertTrue(mission['requires_local_approval']);self.assertEqual(mission,self.robotics.get(self.owner,job['id'])['mission']);self.assertEqual(mission['robot_id'],'test-robot')
 def test_stale_unbounded_and_nonfinite_requests_fail(self):
  for fields in [{'observed_at':time.time()-50},{'capabilities':[{'name':'shell'}]},{'observations':{'value':float('nan')}},{'instruction':'x'*2001}]:
   with self.assertRaises(Failure):self.create(**fields)
 def test_unknown_capability_wrong_arguments_and_expired_observation_fail(self):
  for mutate in [lambda p:p['steps'][0].update(capability='shell'),lambda p:p['steps'][0]['arguments'].update(command='anything'),lambda p:p['steps'][0].update(duration_s=999),lambda p:p['steps'][0]['arguments'].update(waypoint='unknown')]:
   job=self.create();plan=self.plan();mutate(plan);self.result(job['id'],plan);self.assertEqual(self.robotics.get(self.owner,job['id'])['state'],'failed');self.store.execute("UPDATE jobs SET state='deleted' WHERE id=?",(job['id'],))
  job=self.create();self.result(job['id'],self.plan());self.store.execute('UPDATE robot_plans SET observed=0 WHERE id=?',(job['id'],));self.assertEqual(self.robotics.get(self.owner,job['id'])['state'],'failed')
 def test_clarification_does_not_create_steps(self):
  job=self.create();self.result(job['id'],{'summary':'Need a known destination','needs_clarification':True,'question':'Which waypoint?','steps':[]});mission=self.robotics.get(self.owner,job['id'])['mission'];self.assertEqual(mission['plan']['steps'],[])

 def test_truncated_model_context_cannot_issue_a_mission(self):
  job=self.create();self.result(job['id'],self.plan());path=self.store.root/'jobs'/job['id']/'result.json';value=json.loads(path.read_text());value['truncated']=True;atomic_json(path,value);result=self.robotics.get(self.owner,job['id']);self.assertEqual(result['state'],'failed');self.assertNotIn('mission',result)

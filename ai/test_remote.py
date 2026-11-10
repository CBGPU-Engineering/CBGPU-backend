import tempfile,unittest,time,uuid
from core import Store,Failure
from robot_remote import RemoteRobots
MAP={'width':4,'height':4,'resolution':.5,'origin':{'x':0,'y':0,'yaw':0},'frame':'map','session':'mapping-1','version':1,'rle':[0,8,-1,4,100,4]}
class RemoteTests(unittest.TestCase):
 def setUp(self):self.tmp=tempfile.TemporaryDirectory();self.store=Store(self.tmp.name);self.remote=RemoteRobots(self.store);self.owner=str(uuid.uuid4());self.other=str(uuid.uuid4());self.created=self.remote.create(self.owner,'Test Pi');self.id=self.created['device']['id'];self.session=str(uuid.uuid4());self.token=self.remote.enroll({'pair_code':self.created['pair_code'],'agent_session':self.session})['device_token'];self.lease=str(uuid.uuid4())
 def tearDown(self):self.store.db.close();self.tmp.cleanup()
 def sync(self,**extra):return self.remote.sync({'device_token':self.token,'agent_session':self.session,'telemetry':{'armed':True,'ready':True,'emergency_stop':False,'navigation_enabled':True,'pose':{'x':.25,'y':.25,'yaw':0},'map_session':'mapping-1','map_version':1,'sensor_age_s':0},'map':MAP,**extra})
 def enable(self):self.sync();self.remote.control(self.owner,self.id,{'lease':self.lease})
 def goal(self,**fields):return self.remote.goal(self.owner,self.id,{'lease':self.lease,'x':.75,'y':.75,'yaw':0,'map_session':'mapping-1','map_version':1,**fields})
 def test_pair_once_and_cross_account_boundaries(self):
  with self.assertRaises(Failure):self.remote.enroll({'pair_code':self.created['pair_code'],'agent_session':self.session})
  for action in [lambda:self.remote.state(self.other,self.id),lambda:self.remote.stop(self.other,self.id),lambda:self.remote.revoke(self.other,self.id),lambda:self.remote.control(self.other,self.id,{'lease':self.lease})]:
   with self.assertRaises(Failure) as e:action()
   self.assertEqual(e.exception.status,404)
  self.assertEqual(self.remote.list(self.other),[]);self.assertNotIn('token_hash',str(self.remote.list(self.owner)))
 def test_live_map_goal_dispatch_ack_and_stop(self):
  self.enable();command=self.goal()['command'];reply=self.sync();self.assertEqual(reply['command']['id'],command['id']);self.sync(ack={'id':command['id'],'state':'running'});self.assertEqual(self.remote.state(self.owner,self.id)['command']['state'],'running');self.remote.stop(self.owner,self.id);reply=self.sync();self.assertFalse(reply['operator_live']);self.assertGreater(reply['stop_epoch'],0);self.assertIsNone(reply['command'])
 def test_unknown_occupied_outside_and_stale_map_rejected(self):
  self.enable()
  for fields in [{'x':.25,'y':1.25},{'x':.25,'y':1.75},{'x':100},{'x':float('nan')},{'map_session':'old'},{'map_version':100}]:
   with self.assertRaises(Failure):self.goal(**fields)
 def test_lease_expiry_and_revoked_device(self):
  self.enable();self.goal();self.store.execute('UPDATE remote_robots SET lease_until=0 WHERE id=?',(self.id,));reply=self.sync();self.assertFalse(reply['operator_live']);self.assertEqual(self.remote.state(self.owner,self.id)['command']['state'],'cancelled');self.remote.revoke(self.owner,self.id)
  with self.assertRaises(Failure):self.sync()
 def test_duplicate_agent_and_map_version_conflicts(self):
  self.sync()
  with self.assertRaises(Failure):self.sync(agent_session=str(uuid.uuid4()))
  with self.assertRaises(Failure):self.sync(map={**MAP,'rle':[0,16]})
 def test_sensor_age_disables_navigation(self):
  self.sync();self.remote.control(self.owner,self.id,{'lease':self.lease});self.sync(telemetry={'armed':True,'ready':True,'emergency_stop':False,'navigation_enabled':True,'pose':{'x':0,'y':0,'yaw':0},'map_session':'mapping-1','map_version':1,'sensor_age_s':9})
  with self.assertRaises(Failure):self.goal()

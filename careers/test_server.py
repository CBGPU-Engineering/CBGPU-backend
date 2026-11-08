import importlib.util,json,os,tempfile,threading,unittest,urllib.request,urllib.error
from pathlib import Path
spec=importlib.util.spec_from_file_location('careers',Path(__file__).with_name('server.py'));s=importlib.util.module_from_spec(spec);spec.loader.exec_module(s)
class Reception(unittest.TestCase):
 @classmethod
 def setUpClass(cls):
  cls.temp=tempfile.TemporaryDirectory();s.STATE=Path(cls.temp.name);s.initialize();cls.http=s.ThreadingHTTPServer(('127.0.0.1',0),s.Handler);cls.base='http://127.0.0.1:'+str(cls.http.server_port);threading.Thread(target=cls.http.serve_forever,daemon=True).start()
 @classmethod
 def tearDownClass(cls):cls.http.shutdown();cls.http.server_close();cls.temp.cleanup()
 def post(self,data,origin=s.ORIGIN):
  req=urllib.request.Request(self.base+'/api/applications',data=json.dumps(data).encode(),headers={'Origin':origin,'Content-Type':'application/json'})
  try:
   with urllib.request.urlopen(req) as r:return r.status,json.load(r)
  except urllib.error.HTTPError as r:
   with r:return r.code,json.load(r)
 def form(self):return dict(role='frontend',name='Test applicant',email='test@example.com',motivation='A test motivation that is longer than twenty characters.',consent=True,token=s.token())
 def test_onboarding_is_an_independent_rendered_page(self):
  with urllib.request.urlopen(self.base+'/onboarding') as response:page=response.read().decode()
  self.assertIn('CBGPU Engineering',page);self.assertIn('GitHub ID',page);self.assertIn('join.slack.com/t/cbgpuengineering',page);self.assertNotIn('cbgpu.atlassian.net',page);self.assertIn('Jira 초대받을 이메일',page);self.assertIn('수동으로 초대',page);self.assertIn('src="/onboarding.js"',page);self.assertNotIn('src="/app.js"',page)
 def test_accepts_once_and_keeps_private(self):
  data=self.form();status,result=self.post(data);self.assertEqual(status,201);self.assertEqual(self.post(data),(200,result))
  with s.connect() as db:self.assertEqual(db.execute('SELECT count(*) FROM applications WHERE id=?',(result['reference'],)).fetchone()[0],1)
  with self.assertRaises(urllib.error.HTTPError) as ctx:urllib.request.urlopen(self.base+'/api/applications')
  self.assertEqual(ctx.exception.code,404);ctx.exception.close()
 def test_rejects_cross_origin_and_unsigned_forms(self):
  self.assertEqual(self.post(self.form(),'https://evil.example')[0],403);data=self.form();data['token']='forged';self.assertEqual(self.post(data)[0],400)
 def test_requires_consent_and_valid_role(self):
  data=self.form();data['consent']=False;self.assertEqual(self.post(data)[0],400);data=self.form();data['role']='root';self.assertEqual(self.post(data)[0],400)
 def test_cleans_old_data_without_email(self):
  status,res=self.post(self.form());self.assertEqual(status,201)
  with s.connect() as db:db.execute('UPDATE applications SET created=0 WHERE id=?',(res['reference'],))
  s.process_outbox()
  with s.connect() as db:self.assertIsNone(db.execute('SELECT id FROM applications WHERE id=?',(res['reference'],)).fetchone())
if __name__=='__main__':unittest.main()

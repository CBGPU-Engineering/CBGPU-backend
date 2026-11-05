import importlib.util,json,sqlite3,tempfile,unittest
from pathlib import Path
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('agent',Path(__file__).with_name('agent.py'));a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a)
class Gate(unittest.TestCase):
 def test_main_ci_must_match_sha_repository_and_push_event(self):
  sha='a'*40;repo='CBGPU-Engineering/CBGPU-web'
  def api(event):return [{'object':{'sha':sha}},{'workflow_runs':[{'head_sha':sha,'head_branch':'main','event':event,'status':'completed','conclusion':'success','repository':{'full_name':repo},'html_url':'https://github.com/check'}]}]
  with patch.object(a,'github',side_effect=api('pull_request')):
   with self.assertRaises(a.Deferred):a.candidate(repo)
  with patch.object(a,'github',side_effect=api('push')):self.assertEqual(a.candidate(repo)[0],sha)
 def test_unexpected_repo_is_rejected_before_any_network(self):
  with patch.object(a,'github') as api:
   with self.assertRaises(a.DeliveryError):a.candidate('other/project')
   api.assert_not_called()
 def test_failed_health_restores_previous_unit_and_legacy_endpoint(self):
  with tempfile.TemporaryDirectory() as tmp,patch.object(a,'SETTINGS',Path(tmp)),patch.object(a,'ROOT',Path(tmp)),patch.object(a,'command'),patch.object(a,'health',side_effect=[a.DeliveryError('bad release'),None]) as health:
   p=Path(tmp)/'web.service';p.write_text('previous')
   with self.assertRaises(a.DeliveryError):a.switch({'web.service':'candidate'},[{'url':'http://127.0.0.1:3000/api/health','commit':'x'}])
   self.assertEqual(p.read_text(),'previous');self.assertEqual(health.call_args.kwargs['url'],'http://127.0.0.1:3000/')
 def test_schema_changes_cannot_reach_checkout_or_service_restart(self):
  with patch.object(a,'changed_paths',return_value=['schema/change.sql']),patch.object(a,'checkout') as checkout:
   with self.assertRaises(a.Deferred):a.deploy_backend({},'a'*40,'repo','b'*40,None)
   checkout.assert_not_called()
 def test_running_user_build_is_detected_without_mutation(self):
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp)/'private/frontend-deployments';p.mkdir(parents=True)
   with sqlite3.connect(p/'deploy.sqlite') as c:c.execute('CREATE TABLE deployments(status TEXT)');c.execute("INSERT INTO deployments VALUES('building')")
   self.assertTrue(a.busy_backend({'runtime_backend':tmp}))
   with sqlite3.connect(p/'deploy.sqlite') as c:c.execute("UPDATE deployments SET status='ready'")
   self.assertFalse(a.busy_backend({'runtime_backend':tmp}))
 def test_compare_rejects_divergent_main_history(self):
  with patch.object(a,'github',return_value={'status':'diverged','files':[]}):
   with self.assertRaises(a.Deferred):a.changed_paths('repo','a','b')
class RetainedAssets(unittest.TestCase):
 def test_keeps_prior_public_chunks_without_overwriting_current_or_copying_server_secrets(self):
  with tempfile.TemporaryDirectory() as tmp,patch.object(a,'ROOT',Path(tmp)):
   root=Path(tmp)/'releases/web';old=root/('a'*40)/'web';new=root/('b'*40)/'web'
   for p in [old,new]:(p/'.next-production/static/chunks').mkdir(parents=True);(p/'.delivery-built').touch()
   (old/'.next-production/static/chunks/old.js').write_text('old');(old/'.next-production/static/chunks/shared.js').write_text('previous')
   (new/'.next-production/static/chunks/shared.js').write_text('current');(old/'.next-production/server').mkdir();(old/'.next-production/server/secret').write_text('private')
   (old/'.next-production/static/chunks/link.js').symlink_to(old/'.next-production/server/secret')
   self.assertEqual(a.retain_web_assets(new),1)
   self.assertEqual((new/'.next-production/static/chunks/old.js').read_text(),'old')
   self.assertEqual((new/'.next-production/static/chunks/shared.js').read_text(),'current')
   self.assertFalse((new/'.next-production/static/chunks/link.js').exists())
   self.assertNotIn('chunks/old.js',json.loads((new/'.delivery-own-static.json').read_text()))

if __name__=='__main__':unittest.main()

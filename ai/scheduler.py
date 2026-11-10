import json, os, shutil, subprocess, threading, time
from pathlib import Path
from core import atomic_json,Failure
import resources

class Scheduler:
    def __init__(self,store,python,runner,image):
        self.store=store;self.python=Path(python);self.runner=Path(runner);self.image=image;self.stop_event=threading.Event();self.current=None;self.health={'status':'starting','gpus':[]};self.last_owner=None
    def update(self,id,state,**fields):
        fields={'state':state,'updated':time.time(),**fields};self.store.execute('UPDATE jobs SET '+','.join(k+'=?' for k in fields)+' WHERE id=?',(*fields.values(),id))
    def start(self):
        # Only this application's labelled containers are reconciled after a service crash.
        try:
            names=resources.run(['docker','ps','-a','--filter','label=cbgpu.ai.managed=true','--format','{{.Names}}']).splitlines()
            for name in names:
                if name.startswith('cbgpu-ai-'):resources.run(['docker','rm','-f',name],8)
        except (OSError,subprocess.SubprocessError):pass
        self.store.execute("UPDATE jobs SET state=CASE WHEN crashes>=2 THEN 'failed' ELSE 'paused' END,crashes=crashes+1,progress='Unexpected worker restart; inspect task log if repeated',updated=? WHERE state='running'",(time.time(),))
        self.thread=threading.Thread(target=self.loop,daemon=True);self.thread.start()
    def loop(self):
        while not self.stop_event.is_set():
            try:
                cards,_=resources.snapshot();self.health={'status':'ok','checked_at':time.time(),'gpus':[{'id':c['index']+1,'name':c['name'],'reserved':c['reserved'],'available':c['available']} for c in cards]}
                candidates=[dict(r) for r in self.store.execute("SELECT * FROM jobs WHERE state IN ('queued','paused') ORDER BY CASE WHEN kind='python' THEN 1 ELSE 0 END,updated,created").fetchall()]
                free=next((c for c in cards if c['available']),None)
                if candidates and free:
                    # Round-robin owners among the same priority; one account cannot occupy every free slot.
                    priority=candidates[0]['kind']=='python';same=[c for c in candidates if (c['kind']=='python')==priority]
                    job=next((c for c in same if c['owner']!=self.last_owner),same[0]);self.last_owner=job['owner']
                    if not resources.owner_allowed(job['owner']):self.update(job['id'],'cancelled',error='Account access is no longer available');continue
                    self.execute(job,free)
                self.cleanup()
            except Exception:
                self.health={'status':'unavailable','checked_at':time.time(),'gpus':[],'detail':'GPU or reservation status could not be verified. New tasks are paused.'}
            self.stop_event.wait(2)
    def execute(self,job,gpu):
        folder=self.store.root/'jobs'/job['id'];process=None;log=None;start=time.monotonic();elapsed=0;state='failed';message='The task could not start';failed_start=False;self.current=job['id']
        try:
            # Recheck immediately before acquiring a device; the dashboard may have changed during file preparation.
            if job['kind']=='python':resources.prepare_workspace(self.store,job,json.loads((folder/'input.json').read_text()))
            cards,_=resources.snapshot()
            if not any(c['uuid']==gpu['uuid'] and c['available'] for c in cards):return
            if shutil.disk_usage(self.store.root).free<20*1024**3:raise RuntimeError('AI storage is running low')
            changed=self.store.execute("UPDATE jobs SET state='running',gpu=?,attempts=?,started=?,updated=?,error=NULL,progress=? WHERE id=? AND state IN ('queued','paused')",(gpu['index']+1,job['attempts']+1,time.time(),time.time(),'Running on GPU '+str(gpu['index']+1),job['id']))
            if not changed.rowcount:return
            process,log=resources.start_task(self.store,job,gpu,self.python,self.runner,self.image)
            data=json.loads((folder/'input.json').read_text());limit=data.get('minutes',60)*60 if job['kind']=='python' else 3600
            previous=float(job.get('runtime') or 0);last_account=time.monotonic()
            while process.poll() is None:
                if self.stop_event.wait(1):state='paused';message='Service restart; waiting to resume';break
                elapsed=time.monotonic()-start
                if shutil.disk_usage(self.store.root).free<10*1024**3:state='failed';message='AI storage is running low';break
                if (folder/'log.txt').exists() and (folder/'log.txt').stat().st_size>60*1024**2:state='failed';message='Task log exceeded 60 MB';break
                record=self.store.execute('SELECT state FROM jobs WHERE id=?',(job['id'],)).fetchone()
                if not record or record['state'] in ['cancelled','deleted']:state=record['state'] if record else 'deleted';message='Cancelled by owner';break
                if previous+elapsed>limit:state='failed';message='Task runtime limit reached';break
                try:
                    cards,pids=resources.snapshot();card=next(c for c in cards if c['uuid']==gpu['uuid']);owned=resources.own_pids(process,job)
                    self.health={'status':'ok','checked_at':time.time(),'active_job_kind':job['kind'],'active_gpu':gpu['index']+1,'gpus':[{'id':c['index']+1,'reserved':c['reserved'],'available':c['available']} for c in cards]}
                    if card['reserved'] or any(p['uuid']==gpu['uuid'] and p['pid'] not in owned for p in pids):state='paused';message='GPU reservation or notebook workload takes priority';break
                    if time.monotonic()-last_account>20:
                        last_account=time.monotonic()
                        if not resources.owner_allowed(job['owner']):state='cancelled';message='Account access revoked';break
                except Exception:state='paused';message='GPU reservation check unavailable; paused safely';break
                if job['kind']=='python' and elapsed>60 and self.store.execute("SELECT count(*) FROM jobs WHERE kind<>'python' AND state IN ('queued','paused')").fetchone()[0]:state='paused';message='Short AI task takes priority; workspace is preserved';break
                if elapsed>30 and int(elapsed)%10==0:
                    if sum(p.stat().st_size for p in (folder/'workspace').rglob('*') if p.is_file() and not p.is_symlink())>2*1024**3:state='failed';message='Workspace exceeded 2 GB';break
                progress=folder/'progress.json'
                if progress.exists():
                    try:self.store.execute('UPDATE jobs SET progress=?,updated=? WHERE id=?',(json.loads(progress.read_text()).get('message','')[:200],time.time(),job['id']))
                    except (OSError,ValueError):pass
            else:
                elapsed=time.monotonic()-start
                if process.returncode==0:
                    state='succeeded';message='Completed'
                    if job['kind']=='python':atomic_json(folder/'result.json',{'text':'Python task completed. Download files saved in /workspace/output.','checkpoint_directory':'/workspace'})
                    elif not (folder/'result.json').is_file():state='failed';message='Task finished without a result'
                else:state='failed';message='Task failed. See the task log for details.'
        except Failure as error:failed_start=True;state='failed';message=str(error)
        except Exception:failed_start=True;state='failed';message='Could not run this task. Check its log and runtime setup.'
        finally:
            if process:resources.stop_task(process,job)
            if log:log.close()
            self.current=None
            if process:
                self.store.execute('UPDATE jobs SET runtime=runtime+? WHERE id=?',(max(elapsed,time.monotonic()-start),job['id']))
            latest=self.store.execute('SELECT state FROM jobs WHERE id=?',(job['id'],)).fetchone()
            if latest and latest['state'] in ['cancelled','deleted']:state=latest['state']
            if process or failed_start:self.update(job['id'],state,progress=message,error=message if state=='failed' else None,finished=time.time() if state not in ['paused','queued'] else None)
    def cleanup(self):
        now=time.time();self.store.execute('DELETE FROM downloads WHERE expires<? OR job_id NOT IN (SELECT id FROM jobs)',(now,))
        for r in self.store.execute("SELECT id FROM jobs WHERE state='deleted' OR (state NOT IN ('queued','running','paused') AND updated<?)",(now-30*86400,)).fetchall():
            if r['id']==self.current:continue
            folder=self.store.root/'jobs'/r['id']
            try:
                data=json.loads((folder/'input.json').read_text());id=data.get('upload_id')
                if id:(self.store.root/'uploads'/id).unlink(missing_ok=True);self.store.execute('DELETE FROM uploads WHERE id=?',(id,))
            except (OSError,ValueError):pass
            shutil.rmtree(folder,ignore_errors=True);self.store.execute('DELETE FROM jobs WHERE id=?',(r['id'],))
        for r in self.store.execute('SELECT id FROM uploads WHERE created<? AND (job_id IS NULL OR job_id NOT IN (SELECT id FROM jobs WHERE state IN (\'queued\',\'running\',\'paused\')))',(now-14*86400,)).fetchall():
            (self.store.root/'uploads'/r['id']).unlink(missing_ok=True);self.store.execute('DELETE FROM uploads WHERE id=?',(r['id'],))
    def close(self):self.stop_event.set();self.thread.join(timeout=12)

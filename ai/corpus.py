"""Search cache for ready shared materials and published papers ONLY.
Private research notes never enter this corpus; the website supplies them under caller RLS.
"""
import json,os,signal,sqlite3,subprocess,threading,time,urllib.request,uuid
from pathlib import Path
import resources
from core import valid_id

class Corpus:
    def __init__(self,state,python,worker):
        self.root=Path(state);self.python=Path(python);self.worker=Path(worker);self.stop=threading.Event();self.process=None;self.health={'state':'starting','indexed':0};self.revision=json.loads(Path(__file__).with_name('models.json').read_text())['embed']['revision'];self.path=self.root/'corpus.sqlite'
        with sqlite3.connect(self.path) as db:
            db.executescript('''PRAGMA journal_mode=WAL;CREATE TABLE IF NOT EXISTS documents(kind TEXT NOT NULL,id TEXT NOT NULL,version TEXT NOT NULL,state TEXT NOT NULL,checked REAL NOT NULL,PRIMARY KEY(kind,id));CREATE TABLE IF NOT EXISTS chunks(kind TEXT NOT NULL,id TEXT NOT NULL,number INTEGER NOT NULL,text TEXT NOT NULL,vector TEXT NOT NULL,PRIMARY KEY(kind,id,number));CREATE INDEX IF NOT EXISTS chunks_document ON chunks(kind,id);''')
        self.path.chmod(0o600);(self.root/'corpus-work').mkdir(exist_ok=True,mode=0o700)
    def metadata(self):
        query="SELECT json_build_object('materials',(SELECT coalesce(json_agg(t),'[]'::json) FROM (SELECT id,file_name AS name,size_bytes AS size,created_at::text AS version FROM public.shared_materials WHERE status='ready' ORDER BY created_at DESC LIMIT 1000)t),'papers',(SELECT coalesce(json_agg(t),'[]'::json) FROM (SELECT id,current_version::text AS version FROM public.preprints WHERE status='published' ORDER BY published_at DESC LIMIT 1000)t))"
        return json.loads(resources.run(['docker','exec','cbgpu-supabase-db','psql','-X','-U','supabase_admin','-d','postgres','-At','-v','ON_ERROR_STOP=1','-c',query]))
    def start(self):self.thread=threading.Thread(target=self.loop,daemon=True);self.thread.start()
    def loop(self):
        while not self.stop.is_set():
            try:
                metadata=self.metadata();allowed={('material',str(r['id'])) for r in metadata['materials']}|{('paper',str(r['id'])) for r in metadata['papers']}
                with sqlite3.connect(self.path,timeout=20) as db:
                    for kind,id in db.execute('SELECT kind,id FROM documents').fetchall():
                        if (kind,id) not in allowed:db.execute('DELETE FROM documents WHERE kind=? AND id=?',(kind,id));db.execute('DELETE FROM chunks WHERE kind=? AND id=?',(kind,id))
                for kind,rows in [('material',metadata['materials']),('paper',metadata['papers'])]:
                    for row in rows:
                        if self.stop.is_set():return
                        id=str(row['id'])
                        if not valid_id(id):continue
                        version=str(row['version'])+':'+self.revision
                        with sqlite3.connect(self.path) as db:old=db.execute('SELECT version,state,checked FROM documents WHERE kind=? AND id=?',(kind,id)).fetchone()
                        if old and old[0]==version and (old[1] in ['ready','unsupported'] or time.time()-old[2]<3600):continue
                        self.index(kind,row,version)
                with sqlite3.connect(self.path) as db:count=db.execute("SELECT count(*) FROM documents WHERE state='ready'").fetchone()[0]
                self.health={'state':'ready','indexed':count,'checked_at':time.time()}
            except Exception:self.health={**self.health,'state':'unavailable','checked_at':time.time()}
            self.stop.wait(60)
    def index(self,kind,row,version):
        id=str(row['id']);source=None;temporary=None;output=self.root/'corpus-work'/(id+'.json');state='error'
        try:
            if kind=='material':
                if row.get('size',0)>200*1024**2:raise ValueError('Unsupported size')
                name=row['name'];root=Path(os.environ.get('CBGPU_AI_MATERIALS_ROOT','/home/cbgpu/cbgpu-materials'));hex=uuid.UUID(id).hex;source=root/hex[:2]/hex
                if source.is_symlink() or not source.is_file() or source.stat().st_size!=row.get('size'):raise ValueError('File unavailable')
            else:
                name=id+'.pdf';source=self.root/'corpus-work'/(id+'.pdf');temporary=source
                req=urllib.request.Request('http://127.0.0.1:3000/api/arxiv/papers/'+id+'/pdf',headers={'Host':'arxiv.cbgpu.com'})
                with urllib.request.urlopen(req,timeout=20) as response,source.open('wb') as file:
                    total=0
                    while chunk:=response.read(1024*1024):
                        total+=len(chunk)
                        if total>21*1024**2:raise ValueError('Unsupported size')
                        file.write(chunk)
            if not name.lower().endswith(('.pdf','.txt','.md','.csv','.tex','.py','.log')):state='unsupported';return
            site=self.python.parent.parent/'lib/python3.12/site-packages';libraries=[str(p) for p in (site/'nvidia').glob('*/lib')]+[str(site/'torch/lib')]
            env={'PATH':'/usr/bin:/bin','HOME':str(self.root),'LANG':'C.UTF-8','CUDA_VISIBLE_DEVICES':'','OMP_NUM_THREADS':'4','MKL_NUM_THREADS':'4','OPENBLAS_NUM_THREADS':'4','TOKENIZERS_PARALLELISM':'false','HF_HUB_OFFLINE':'1','HF_HUB_DISABLE_TELEMETRY':'1','LD_LIBRARY_PATH':':'.join(libraries)}
            self.process=subprocess.Popen(['prlimit','--as=17179869184','--cpu=150','--fsize=16777216','--',str(self.python),str(self.worker),str(source),name,str(self.root),str(output)],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
            try:self.process.wait(timeout=180)
            except subprocess.TimeoutExpired:os.killpg(self.process.pid,signal.SIGKILL);self.process.wait();raise ValueError('Indexing time limit')
            if self.process.returncode or not output.exists():raise ValueError('No extractable text')
            parts=json.loads(output.read_text())
            with sqlite3.connect(self.path,timeout=20) as db:
                db.execute('DELETE FROM chunks WHERE kind=? AND id=?',(kind,id));db.executemany('INSERT INTO chunks VALUES(?,?,?,?,?)',[(kind,id,i,c['text'],json.dumps(c['vector'])) for i,c in enumerate(parts)])
            state='ready'
        except Exception:pass
        finally:
            self.process=None;output.unlink(missing_ok=True)
            if temporary:temporary.unlink(missing_ok=True)
            with sqlite3.connect(self.path,timeout=20) as db:
                db.execute('INSERT OR REPLACE INTO documents VALUES(?,?,?,?,?)',(kind,id,version,state,time.time()))
                if state!='ready':db.execute('DELETE FROM chunks WHERE kind=? AND id=?',(kind,id))
    def close(self):
        self.stop.set()
        if self.process and self.process.poll() is None:
            try:os.killpg(self.process.pid,signal.SIGTERM)
            except ProcessLookupError:pass
        self.thread.join(timeout=5)

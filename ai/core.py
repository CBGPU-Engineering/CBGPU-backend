"""Private persistent queue, ownership checks, bounded uploads and artifact paths."""
import hashlib, hmac, json, os, secrets, sqlite3, threading, time, uuid, zipfile, shutil
from pathlib import Path, PurePosixPath

KINDS={'chat','transcribe','embed','search','document','python'}
LIVE={'queued','running','paused'}
MAX_UPLOAD=200*1024*1024
class Failure(Exception):
    def __init__(self,message,status=400): super().__init__(message); self.status=status

def valid_id(value):
    try: return str(uuid.UUID(str(value)))==value
    except (ValueError,TypeError,AttributeError): return False

def safe_relative(value):
    p=PurePosixPath(str(value))
    if not value or p.is_absolute() or '..' in p.parts or '\\' in str(value) or '\x00' in str(value): raise Failure('Invalid file path')
    return p

def atomic_json(path,value):
    temporary=path.with_suffix('.tmp');temporary.write_text(json.dumps(value,ensure_ascii=False));temporary.chmod(0o600);temporary.replace(path)

class Store:
    def __init__(self,state):
        self.root=Path(state).resolve();self.root.mkdir(parents=True,exist_ok=True,mode=0o700);self.root.chmod(0o700)
        for name in ['jobs','uploads','models']: (self.root/name).mkdir(exist_ok=True,mode=0o700)
        self.lock=threading.RLock();self.db=sqlite3.connect(self.root/'queue.sqlite',check_same_thread=False,timeout=20);self.db.row_factory=sqlite3.Row
        self.db.executescript('''PRAGMA journal_mode=WAL;
          CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY,owner TEXT NOT NULL,kind TEXT NOT NULL,title TEXT NOT NULL,state TEXT NOT NULL,created REAL NOT NULL,updated REAL NOT NULL,gpu INTEGER,attempts INTEGER DEFAULT 0,progress TEXT DEFAULT '',error TEXT,started REAL,finished REAL,runtime REAL NOT NULL DEFAULT 0,crashes INTEGER NOT NULL DEFAULT 0);
          CREATE INDEX IF NOT EXISTS jobs_owner ON jobs(owner,created);
          CREATE TABLE IF NOT EXISTS uploads(id TEXT PRIMARY KEY,owner TEXT NOT NULL,name TEXT NOT NULL,size INTEGER NOT NULL,created REAL NOT NULL,job_id TEXT);
          CREATE TABLE IF NOT EXISTS api_keys(id TEXT PRIMARY KEY,owner TEXT NOT NULL,name TEXT NOT NULL,prefix TEXT NOT NULL,hash TEXT NOT NULL UNIQUE,created REAL NOT NULL,last_used REAL,revoked INTEGER DEFAULT 0);
          CREATE TABLE IF NOT EXISTS downloads(id TEXT PRIMARY KEY,owner TEXT NOT NULL,job_id TEXT NOT NULL,kind TEXT NOT NULL,path TEXT,expires REAL NOT NULL);
          CREATE TABLE IF NOT EXISTS vectors(hash TEXT PRIMARY KEY,vector TEXT NOT NULL,updated REAL NOT NULL);
        ''');self.db.commit();(self.root/'queue.sqlite').chmod(0o600)
        for table,column,definition in [('jobs','runtime','REAL NOT NULL DEFAULT 0'),('jobs','crashes','INTEGER NOT NULL DEFAULT 0'),('uploads','job_id','TEXT')]:
            if column not in {r['name'] for r in self.db.execute('PRAGMA table_info('+table+')')}:
                self.db.execute('ALTER TABLE '+table+' ADD COLUMN '+column+' '+definition)
        self.db.commit()
    def execute(self,query,args=()):
        with self.lock:
            cur=self.db.execute(query,args);self.db.commit();return cur
    def upload(self,owner,name,reader):
        name=Path(name.replace('\\','/')).name[:180] or 'upload';id=str(uuid.uuid4());target=self.root/'uploads'/id
        if self.execute('SELECT count(*) FROM uploads WHERE owner=? AND created>?',(owner,time.time()-86400)).fetchone()[0]>=30: raise Failure('Upload limit reached today',429)
        owned=self.execute('SELECT coalesce(sum(size),0) FROM uploads WHERE owner=?',(owner,)).fetchone()[0]
        size=0
        try:
            with target.open('xb') as f:
                target.chmod(0o600)
                for chunk in reader:
                    size+=len(chunk)
                    if shutil.disk_usage(self.root).free<20*1024**3:raise Failure('AI storage is running low. Try again after old results are removed.',507)
                    if owned+size>2*1024**3:raise Failure('Your AI uploads exceed 2 GB. Delete old tasks first.',413)
                    if size>MAX_UPLOAD: raise Failure('Upload limit is 200 MB',413)
                    f.write(chunk)
            if not size: raise Failure('The file is empty')
            self.execute('INSERT INTO uploads(id,owner,name,size,created) VALUES(?,?,?,?,?)',(id,owner,name,size,time.time()))
            return {'id':id,'name':name,'size':size}
        except BaseException: target.unlink(missing_ok=True);raise
    def get_upload(self,owner,id):
        if not valid_id(id):raise Failure('Upload not found',404)
        row=self.execute('SELECT * FROM uploads WHERE id=? AND owner=?',(id,owner)).fetchone()
        if not row or not (self.root/'uploads'/id).is_file(): raise Failure('Upload not found',404)
        return dict(row)
    def validate(self,owner,kind,data):
        if kind not in KINDS or not isinstance(data,dict): raise Failure('Unsupported task')
        if kind=='chat':
            messages=data.get('messages');limit=data.get('max_tokens',512)
            if not isinstance(messages,list) or not 1<=len(messages)<=32 or any(not isinstance(m,dict) or m.get('role') not in ['system','user','assistant'] or not isinstance(m.get('content'),str) for m in messages): raise Failure('Use text chat messages')
            if sum(len(m['content']) for m in messages)>24000: raise Failure('Text is too long',413)
            if type(limit)!=int or not 1<=limit<=2048: raise Failure('max_tokens must be 1–2048')
            data={'messages':[{'role':m['role'],'content':m['content']} for m in messages],'max_tokens':limit}
        elif kind in ['embed','search']:
            if kind=='embed':
                texts=data.get('input');texts=[texts] if isinstance(texts,str) else texts
                if not isinstance(texts,list) or not 1<=len(texts)<=64 or any(not isinstance(t,str) or len(t)>8000 for t in texts): raise Failure('Provide 1–64 texts of at most 8,000 characters')
                data={'input':texts}
            else:
                candidates=data.get('candidates');q=data.get('query')
                if not isinstance(q,str) or not q.strip() or len(q)>1000 or not isinstance(candidates,list) or len(candidates)>500: raise Failure('Invalid search input')
                if any(not isinstance(c,dict) or not isinstance(c.get('id'),str) or len(c['id'])>150 or not isinstance(c.get('text'),str) or len(c['text'])>8000 for c in candidates): raise Failure('Invalid search candidates')
                data={'query':q,'candidates':[{'id':c['id'],'text':c['text'],'label':str(c.get('label',''))[:200],'href':str(c.get('href',''))[:500],'snippet':str(c.get('snippet',''))[:350]} for c in candidates],'corpus_ids':[str(c['id']) for c in candidates if c['id'].startswith(('material:','paper:'))]}
        elif kind in ['transcribe','document']:
            upload=self.get_upload(owner,data.get('upload_id'));language=data.get('language') or None
            if language not in [None,'ko','en','ja','zh']: raise Failure('Unsupported language')
            data={'upload_id':upload['id'],'filename':upload['name'],'language':language}
        elif kind=='python':
            code=data.get('code','');upload_id=data.get('upload_id');entrypoint=data.get('entrypoint','main.py');safe_relative(entrypoint)
            if not isinstance(code,str) or len(code)>100000: raise Failure('Python source is too long')
            if upload_id: self.get_upload(owner,upload_id)
            if not code.strip() and not upload_id: raise Failure('Upload a ZIP or enter Python code')
            if not entrypoint.endswith('.py'): raise Failure('Entrypoint must be a Python file')
            minutes=data.get('minutes',30)
            if type(minutes)!=int or not 1<=minutes<=120: raise Failure('Duration must be 1–120 minutes')
            data={'code':code,'upload_id':upload_id,'entrypoint':str(entrypoint),'minutes':minutes}
        return data
    def enqueue(self,owner,kind,data,title=''):
        if not valid_id(owner): raise Failure('Invalid owner',401)
        data=self.validate(owner,kind,data)
        with self.lock:
            if data.get('upload_id') and self.execute('SELECT job_id FROM uploads WHERE id=?',(data['upload_id'],)).fetchone()['job_id']:raise Failure('This upload already belongs to a task; upload another copy')
            if self.execute("SELECT count(*) FROM jobs WHERE state IN ('queued','running','paused')").fetchone()[0]>=200: raise Failure('Queue is full. Try again later.',429)
            if self.execute("SELECT count(*) FROM jobs WHERE owner=? AND state IN ('queued','running','paused')",(owner,)).fetchone()[0]>=5: raise Failure('Finish or cancel an existing task first (maximum 5 active tasks).',429)
            id=str(uuid.uuid4());folder=self.root/'jobs'/id;folder.mkdir(mode=0o700);atomic_json(folder/'input.json',data)
            self.execute('INSERT INTO jobs(id,owner,kind,title,state,created,updated) VALUES(?,?,?,?,?,?,?)',(id,owner,kind,str(title or kind)[:160],'queued',time.time(),time.time()))
            if data.get('upload_id'):self.execute('UPDATE uploads SET job_id=? WHERE id=?',(id,data['upload_id']))
        return self.job(owner,id)
    def job(self,owner,id,detail=True):
        row=self.execute('SELECT * FROM jobs WHERE id=? AND owner=?',(id,owner)).fetchone()
        if not row: raise Failure('Task not found',404)
        result=dict(row);folder=self.root/'jobs'/id
        if detail and result['state']=='succeeded' and (folder/'result.json').exists():
            try:result['result']=json.loads((folder/'result.json').read_text())
            except (OSError,ValueError):result['error']='Result is temporarily unavailable'
        if detail and (folder/'log.txt').exists():
            with (folder/'log.txt').open('rb') as stream:
                stream.seek(max(0,(folder/'log.txt').stat().st_size-24000));result['logs']=stream.read(24000).decode('utf8',errors='replace')
        if detail:result['artifacts']=self.artifacts(folder)
        return result
    def artifacts(self,folder):
        output=folder/'workspace'/'output'
        if not output.is_dir():return []
        result=[]
        for path in output.rglob('*'):
            if path.is_symlink() or not path.is_file() or any(p.is_symlink() for p in path.parents if p!=output):continue
            if len(result)>=1000:break
            result.append({'path':str(path.relative_to(output)),'size':path.stat().st_size})
        return result
    def artifact(self,owner,id,relative):
        self.job(owner,id,False);safe_relative(relative);root=self.root/'jobs'/id/'workspace'/'output';path=root/relative
        if not path.is_file() or path.is_symlink() or root.resolve() not in path.resolve().parents or any(p.is_symlink() for p in path.parents if p!=root): raise Failure('File not found',404)
        return path
    def download_ticket(self,owner,id,kind,path=''):
        job=self.job(owner,id)
        if job['state']=='deleted':raise Failure('Task not found',404)
        if kind=='artifact':self.artifact(owner,id,path)
        elif kind in ['text','srt']:
            result=job.get('result') or {}
            value=result.get('srt') if kind=='srt' else result.get('text') or (result.get('choices') or [{}])[0].get('message',{}).get('content')
            if not isinstance(value,str):raise Failure('Result not found',404)
        else:raise Failure('Invalid download type')
        self.execute('DELETE FROM downloads WHERE expires<?',(time.time(),))
        if self.execute('SELECT count(*) FROM downloads WHERE owner=?',(owner,)).fetchone()[0]>=100:raise Failure('Too many active downloads',429)
        ticket=str(uuid.uuid4());self.execute('INSERT INTO downloads VALUES(?,?,?,?,?,?)',(ticket,owner,id,kind,path,time.time()+600));return {'id':ticket}
    def download(self,owner,id):
        record=self.execute('SELECT * FROM downloads WHERE id=? AND owner=? AND expires>?',(id,owner,time.time())).fetchone()
        if not record:raise Failure('Download not found or expired',404)
        job=self.job(owner,record['job_id'])
        if job['state']=='deleted':raise Failure('Task not found',404)
        if record['kind']=='artifact':return self.artifact(owner,record['job_id'],record['path'])
        result=job.get('result') or {};value=result.get('srt') if record['kind']=='srt' else result.get('text') or (result.get('choices') or [{}])[0].get('message',{}).get('content')
        if not isinstance(value,str):raise Failure('Result not found',404)
        return value.encode('utf8')
    def keys(self,owner):return [dict(r) for r in self.execute('SELECT id,name,prefix,created,last_used FROM api_keys WHERE owner=? AND revoked=0 ORDER BY created DESC',(owner,)).fetchall()]
    def create_key(self,owner,name):
        if len(self.keys(owner))>=5:raise Failure('Maximum 5 API keys',429)
        token='cbgpu_ai_'+secrets.token_urlsafe(32);id=str(uuid.uuid4());prefix=token[:17]
        self.execute('INSERT INTO api_keys(id,owner,name,prefix,hash,created) VALUES(?,?,?,?,?,?)',(id,owner,str(name or 'API key')[:80],prefix,hashlib.sha256(token.encode()).hexdigest(),time.time()))
        return {'id':id,'key':token,'prefix':prefix}
    def key_owner(self,token):
        row=self.execute('SELECT id,owner FROM api_keys WHERE hash=? AND revoked=0',(hashlib.sha256(str(token).encode()).hexdigest(),)).fetchone()
        if not row:raise Failure('Invalid API key',401)
        self.execute('UPDATE api_keys SET last_used=? WHERE id=?',(time.time(),row['id']));return row['owner']

def extract_zip(source,destination):
    destination.mkdir(parents=True,exist_ok=True,mode=0o700)
    size=0
    with zipfile.ZipFile(source) as z:
        if len(z.infolist())>10000:raise Failure('ZIP contains too many files')
        for item in z.infolist():
            safe_relative(item.filename)
            if (item.external_attr>>16)&0o170000==0o120000:raise Failure('ZIP symlinks are not supported')
            size+=item.file_size
            if size>2*1024**3:raise Failure('Expanded ZIP exceeds 2 GB')
            path=destination/item.filename
            if item.is_dir():path.mkdir(parents=True,exist_ok=True,mode=0o700);continue
            path.parent.mkdir(parents=True,exist_ok=True,mode=0o700)
            with z.open(item) as source_file,path.open('wb') as target:
                copied=0
                while chunk:=source_file.read(1024*1024):
                    if shutil.disk_usage(destination).free<10*1024**3:raise Failure('AI storage is running low',507)
                    copied+=len(chunk)
                    if copied>item.file_size or copied>2*1024**3:raise Failure('ZIP file is too large')
                    target.write(chunk)

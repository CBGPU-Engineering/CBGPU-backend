"""Loopback-only AI control plane. Browser identity is verified by the Next.js gateway."""
import argparse, hmac, json, os, signal, socket, threading, time, urllib.parse
from collections import defaultdict,deque
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path
from core import Store,Failure,MAX_UPLOAD,valid_id
from scheduler import Scheduler
from corpus import Corpus
from conversations import Conversations
from catalog import models

class AIHTTPServer(ThreadingHTTPServer):
    daemon_threads=True
    def __init__(self,address,store,scheduler,key,corpus=None):
        super().__init__(address,Handler);self.store=store;self.scheduler=scheduler;self.key=key;self.corpus=corpus;self.conversations=Conversations(store);self.requests=defaultdict(deque);self.rate_lock=threading.Lock()
    def rate(self,owner):
        with self.rate_lock:
            now=time.time();q=self.requests[owner]
            while q and q[0]<now-60:q.popleft()
            if len(q)>=40:raise Failure('Too many AI requests; try again shortly',429)
            q.append(now)
            if len(self.requests)>5000:
                for k in list(self.requests):
                    if not self.requests[k] or self.requests[k][-1]<now-60:self.requests.pop(k,None)

class Handler(BaseHTTPRequestHandler):
    server_version='CBGPU-AI';protocol_version='HTTP/1.1'
    def log_message(self,*args):pass
    def setup(self):super().setup();self.connection.settimeout(60)
    def reply(self,status,value):
        body=json.dumps(value,ensure_ascii=False).encode();self.send_response(status);self.send_header('Content-Type','application/json; charset=utf-8');self.send_header('Content-Length',str(len(body)));self.send_header('Cache-Control','no-store');self.send_header('X-Content-Type-Options','nosniff');self.end_headers();self.wfile.write(body)
    def chunks(self,maximum):
        length=self.headers.get('Content-Length');encoding=self.headers.get('Transfer-Encoding','').lower();size=0
        if encoding=='chunked':
            while True:
                line=self.rfile.readline(100)
                if not line.endswith(b'\r\n'):raise Failure('Invalid upload framing')
                try:amount=int(line.split(b';')[0].strip(),16)
                except ValueError:raise Failure('Invalid upload framing')
                if amount==0:
                    # Only an empty trailer is accepted from our own gateway.
                    if self.rfile.readline(100)!=b'\r\n':raise Failure('Upload trailers are not supported')
                    break
                if amount<0 or size+amount>maximum:raise Failure('Upload is too large',413)
                remaining=amount
                while remaining:
                    chunk=self.rfile.read(min(1024*1024,remaining))
                    if not chunk:raise Failure('Upload was interrupted')
                    remaining-=len(chunk);size+=len(chunk);yield chunk
                if self.rfile.read(2)!=b'\r\n':raise Failure('Invalid upload framing')
        else:
            try:remaining=int(length or '0')
            except ValueError:raise Failure('Invalid upload length')
            if not 0<=remaining<=maximum:raise Failure('Upload is too large',413)
            while remaining:
                chunk=self.rfile.read(min(1024*1024,remaining))
                if not chunk:raise Failure('Upload was interrupted')
                remaining-=len(chunk);yield chunk
    def body(self):
        if hasattr(self,'_body_value'):return self._body_value
        try:
            self._body_value=json.loads(b''.join(self.chunks(3*1024*1024)));return self._body_value
        except (ValueError,UnicodeError):raise Failure('Invalid JSON')
    def dispatch(self):
        parsed=urllib.parse.urlsplit(self.path);parts=[p for p in parsed.path.split('/') if p];store=self.server.store
        if parts==['health'] and self.command=='GET':self.reply(200,{'status':'ok','commit':os.environ.get('CBGPU_RELEASE_SHA','local'),'scheduler':self.server.scheduler.health});return
        if not hmac.compare_digest(self.headers.get('x-cbgpu-ai-key',''),self.server.key):raise Failure('Internal authentication required',401)
        owner=self.headers.get('x-cbgpu-owner','')
        if parts==['authenticate-key'] and self.command=='POST':self.reply(200,{'owner':store.key_owner(self.body().get('key',''))});return
        if not valid_id(owner):raise Failure('Authentication required',401)
        if self.command!='GET':self.server.rate(owner)
        if self.command=='POST' and parts!=['uploads']:self.body()
        if parts==['models'] and self.command=='GET':self.reply(200,{'object':'list','data':models(store.root)});return
        if parts==['status'] and self.command=='GET':
            counts={r[0]:r[1] for r in store.execute("SELECT state,count(*) FROM jobs WHERE state IN ('queued','running','paused') GROUP BY state").fetchall()}
            self.reply(200,{'scheduler':self.server.scheduler.health,'queue':counts,'corpus':self.server.corpus.health if self.server.corpus else {},'models':[{'id':'cbgpu-chat','task':'chat'},{'id':'cbgpu-embeddings','task':'embeddings'},{'id':'cbgpu-whisper','task':'transcription'}]});return
        if parts==['uploads'] and self.command=='POST':self.reply(201,store.upload(owner,urllib.parse.parse_qs(parsed.query).get('name',['upload'])[0],self.chunks(MAX_UPLOAD)));return
        if len(parts)==2 and parts[0]=='downloads' and valid_id(parts[1]) and self.command=='GET':
            value=store.download(owner,parts[1]);size=len(value) if isinstance(value,bytes) else value.stat().st_size
            self.send_response(200);self.send_header('Content-Type','application/octet-stream');self.send_header('Content-Length',str(size));self.send_header('Cache-Control','no-store');self.end_headers()
            if isinstance(value,bytes):self.wfile.write(value)
            else:
                with value.open('rb') as stream:
                    while chunk:=stream.read(1024*1024):self.wfile.write(chunk)
            return
        if parts==['chats']:
            if self.command=='GET':self.reply(200,{'chats':self.server.conversations.list(owner)});return
            if self.command=='POST':self.reply(201,{'chat':self.server.conversations.create(owner)});return
        if len(parts) in [2,3] and parts[0]=='chats' and valid_id(parts[1]):
            chats=self.server.conversations;id=parts[1]
            if len(parts)==2:
                if self.command=='GET':self.reply(200,{'chat':chats.get(owner,id)});return
                if self.command=='POST':self.reply(200,{'chat':chats.rename(owner,id,self.body().get('title'))});return
                if self.command=='DELETE':chats.delete(owner,id);self.reply(200,{'ok':True});return
            if parts[2:]==['messages'] and self.command=='POST':self.reply(202,{'chat':chats.send(owner,id,self.body().get('content'),self.body().get('model','cbgpu-chat'))});return
        if parts==['keys']:
            if self.command=='GET':self.reply(200,{'keys':store.keys(owner)});return
            if self.command=='POST':self.reply(201,store.create_key(owner,self.body().get('name')));return
        if len(parts)==2 and parts[0]=='keys' and self.command=='DELETE':store.execute('UPDATE api_keys SET revoked=1 WHERE id=? AND owner=?',(parts[1],owner));self.reply(200,{'ok':True});return
        if parts==['jobs']:
            if self.command=='GET':
                rows=store.execute("SELECT id FROM jobs WHERE owner=? AND state<>'deleted' ORDER BY created DESC LIMIT 100",(owner,)).fetchall();self.reply(200,{'jobs':[store.job(owner,r['id'],False) for r in rows]});return
            if self.command=='POST':
                data=self.body()
                if not isinstance(data,dict):raise Failure('Invalid task')
                self.reply(202,{'job':store.enqueue(owner,data.get('kind'),data.get('input'),data.get('title',''))});return
        if len(parts)>=2 and parts[0]=='jobs' and valid_id(parts[1]):
            id=parts[1];job=store.job(owner,id)
            if len(parts)==2 and self.command=='GET':self.reply(200,{'job':job});return
            if len(parts)==2 and self.command=='DELETE':store.execute("UPDATE jobs SET state='deleted',updated=? WHERE id=? AND owner=?",(time.time(),id,owner));self.reply(200,{'ok':True});return
            if parts[2:]==['download'] and self.command=='POST':
                data=self.body();self.reply(201,store.download_ticket(owner,id,data.get('kind'),data.get('path','')));return
            if parts[2:]==['cancel'] and self.command=='POST':store.execute("UPDATE jobs SET state='cancelled',progress='Cancelled by owner',updated=? WHERE id=? AND owner=? AND state IN ('queued','running','paused')",(time.time(),id,owner));self.reply(200,{'ok':True});return
            if parts[2:]==['retry'] and self.command=='POST':
                if job['state'] not in ['failed','cancelled']:raise Failure('Only failed or cancelled tasks can be retried')
                if self.server.scheduler.current==id:raise Failure('The task is still stopping. Try again shortly.',409)
                saved=json.loads((store.root/'jobs'/id/'input.json').read_text())
                if saved.get('upload_id') and not (job['kind']=='python' and (store.root/'jobs'/id/'workspace/.prepared').exists()):store.get_upload(owner,saved['upload_id'])
                with store.lock:
                    if store.execute("SELECT count(*) FROM jobs WHERE state IN ('queued','running','paused') AND owner=?",(owner,)).fetchone()[0]>=5:raise Failure('Maximum 5 active tasks',429)
                    if store.execute("SELECT count(*) FROM jobs WHERE state IN ('queued','running','paused')").fetchone()[0]>=200:raise Failure('Queue is full',429)
                    store.execute("UPDATE jobs SET state='queued',runtime=0,finished=NULL,error=NULL,progress='',updated=? WHERE id=? AND owner=? AND state IN ('failed','cancelled')",(time.time(),id,owner));self.reply(200,{'job':store.job(owner,id)});return
            if parts[2:]==['artifact'] and self.command=='GET':
                relative=urllib.parse.parse_qs(parsed.query).get('path',[''])[0];file=store.artifact(owner,id,relative);self.send_response(200);self.send_header('Content-Type','application/octet-stream');self.send_header('Content-Length',str(file.stat().st_size));self.send_header('Cache-Control','no-store');self.end_headers()
                with file.open('rb') as stream:
                    while chunk:=stream.read(1024*1024):self.wfile.write(chunk)
                return
        raise Failure('Not found',404)
    def handle_request(self):
        if hasattr(self,'_body_value'):del self._body_value
        try:self.dispatch()
        except Failure as e:self.close_connection=True;self.reply(e.status,{'message':str(e)})
        except (BrokenPipeError,ConnectionResetError,TimeoutError):self.close_connection=True
        except Exception:self.close_connection=True;self.reply(500,{'message':'AI service could not process this request'})
    do_GET=handle_request;do_POST=handle_request;do_DELETE=handle_request

def main():
    os.umask(0o077);parser=argparse.ArgumentParser();parser.add_argument('--port',type=int,default=3550);args=parser.parse_args()
    state=Path(os.environ.get('CBGPU_AI_STATE',Path.home()/'.local/share/cbgpu-ai'));store=Store(state)
    keyfile=Path(os.environ.get('CBGPU_AI_KEY_FILE',Path.home()/'.config/cbgpu-ai-internal-key'))
    if not keyfile.exists():
        import secrets
        keyfile.parent.mkdir(parents=True,exist_ok=True);keyfile.write_text(secrets.token_hex(32));keyfile.chmod(0o600)
    scheduler=Scheduler(store,state/'venv/bin/python',Path(__file__).with_name('runner.py'),os.environ.get('CBGPU_AI_JOB_IMAGE','pytorch/pytorch:2.7.1-cuda12.6-cudnn9-runtime'))
    corpus=Corpus(state,state/'venv/bin/python',Path(__file__).with_name('corpus_worker.py'))
    server=AIHTTPServer(('127.0.0.1',args.port),store,scheduler,keyfile.read_text().strip(),corpus);scheduler.start();corpus.start()
    def shutdown(*_):threading.Thread(target=server.shutdown,daemon=True).start()
    signal.signal(signal.SIGTERM,shutdown);signal.signal(signal.SIGINT,shutdown)
    try:server.serve_forever()
    finally:scheduler.close();corpus.close();server.server_close()
if __name__=='__main__':main()

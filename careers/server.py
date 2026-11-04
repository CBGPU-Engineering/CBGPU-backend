#!/usr/bin/env python3
"""Dependency-free recruitment service. Private SQLite storage; retryable mail outbox."""
import contextlib
import argparse, hashlib, hmac, json, os, re, secrets, sqlite3, threading, time, urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit
ROOT=Path(__file__).resolve().parent
ROLES=json.loads((ROOT/'roles.json').read_text())
STATE=Path(os.environ.get('CAREERS_STATE_DIR',str(ROOT.parent/'private'/'careers')))
ORIGIN=os.environ.get('CAREERS_ORIGIN','https://careers.cbgpu.com')
DB=None; KEY=None

@contextlib.contextmanager
def connect():
 c=sqlite3.connect(STATE/'applications.sqlite',timeout=15);c.row_factory=sqlite3.Row
 try:
  with c:yield c
 finally:c.close()

def initialize():
 global KEY
 os.umask(0o077);STATE.mkdir(parents=True,exist_ok=True);STATE.chmod(0o700)
 key=STATE/'form.key'
 try:
  with key.open('x') as f:f.write(secrets.token_hex(32))
 except FileExistsError:pass
 KEY=key.read_text().strip()
 with connect() as c:
  c.executescript('''PRAGMA journal_mode=WAL;
PRAGMA secure_delete=ON;
CREATE TABLE IF NOT EXISTS applications(id TEXT PRIMARY KEY, token_hash TEXT UNIQUE NOT NULL, created INTEGER NOT NULL, role TEXT NOT NULL, name TEXT NOT NULL, email TEXT NOT NULL, affiliation TEXT NOT NULL, motivation TEXT NOT NULL, experience TEXT NOT NULL, portfolio TEXT NOT NULL, consent_version TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'received', notified INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS requests(ip TEXT NOT NULL, created INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS request_ip ON requests(ip,created);''')

def signature(value):return hmac.new(KEY.encode(),value.encode(),hashlib.sha256).hexdigest()
def token():
 value=f'{int(time.time())}.{secrets.token_hex(16)}';return value+'.'+signature(value)
def valid_token(value):
 try:
  stamp,nonce,sig=value.split('.');return bool(re.fullmatch('[a-f0-9]{32}',nonce)) and 0<=time.time()-int(stamp)<7200 and hmac.compare_digest(sig,signature(stamp+'.'+nonce))
 except (ValueError,AttributeError):return False

def validate(data):
 if not isinstance(data,dict):raise ValueError('지원서 형식을 확인해주세요.')
 if data.get('role') not in {r['id'] for r in ROLES}:raise ValueError('지원 분야를 확인해주세요.')
 if data.get('consent') is not True:raise ValueError('개인정보 수집·이용 동의가 필요해요.')
 if not valid_token(data.get('token','')):raise ValueError('지원 화면이 만료됐어요. 내용을 복사한 후 페이지를 새로고침해주세요.')
 if data.get('website'):raise ValueError('자동 접수는 허용하지 않아요.')
 fields={}
 for key,limit in [('name',60),('email',180),('affiliation',100),('motivation',4000),('experience',4000),('portfolio',500)]:
  value=data.get(key,'')
  if not isinstance(value,str) or len(value)>limit or '\x00' in value:raise ValueError('입력 내용의 길이나 형식을 확인해주세요.')
  fields[key]=value.strip()
 if not fields['name'] or not re.fullmatch(r'[^\s@]+@[^\s@]+\.[^\s@]+',fields['email']) or len(fields['motivation'])<20:raise ValueError('이름·이메일과 20자 이상의 지원 동기를 확인해주세요.')
 if fields['portfolio']:
  url=urlsplit(fields['portfolio'])
  if url.scheme not in ['http','https'] or not url.hostname or url.username:raise ValueError('포트폴리오는 http 또는 https 링크로 입력해주세요.')
 return fields

class Handler(BaseHTTPRequestHandler):
 server_version='CBGPU';sys_version=''
 def log_message(self,*args):pass # Never log applicant data or tokens.
 def send(self,status,body,kind='application/json; charset=utf-8',extra=None):
  if isinstance(body,(dict,list)):body=json.dumps(body,ensure_ascii=False).encode()
  elif isinstance(body,str):body=body.encode()
  self.send_response(status)
  headers={'Content-Type':kind,'Content-Length':str(len(body)),'Cache-Control':'no-store','X-Content-Type-Options':'nosniff','Referrer-Policy':'strict-origin-when-cross-origin','X-Frame-Options':'DENY','Permissions-Policy':'camera=(), microphone=(), geolocation=()','Content-Security-Policy':"default-src 'self'; script-src 'self'; style-src 'self' https://fonts.googleapis.com; font-src 'self' https://fonts.gstatic.com; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'",'Strict-Transport-Security':'max-age=31536000'}
  headers.update(extra or {})
  for k,v in headers.items():self.send_header(k,v)
  self.end_headers();self.wfile.write(body)
 def do_GET(self):
  path=urlsplit(self.path).path
  if path=='/health':return self.send(200,{'status':'ok'})
  if path=='/api/roles':return self.send(200,ROLES)
  if path=='/api/form':return self.send(200,{'token':token()})
  if path=='/robots.txt':return self.send(200,f'User-agent: *\nDisallow: /api/\nDisallow: /apply/\nSitemap: {ORIGIN}/sitemap.xml\n','text/plain; charset=utf-8')
  if path=='/sitemap.xml':return self.send(200,'<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">'+''.join(f'<url><loc>{ORIGIN}{p}</loc></url>' for p in ['/','/onboarding']+['/roles/'+r['id'] for r in ROLES])+'</urlset>','application/xml')
  assets={'/style.css':('style.css','text/css; charset=utf-8'),'/app.js':('app.js','text/javascript; charset=utf-8'),'/onboarding.js':('onboarding.js','text/javascript; charset=utf-8'),'/logo.png':('logo.png','image/png')}
  if path in assets:
   name,kind=assets[path];return self.send(200,(ROOT/'public'/name).read_bytes(),kind)
  if path in ['/','/privacy','/onboarding'] or re.fullmatch(r'/(roles|apply)/('+'|'.join(r['id'] for r in ROLES)+')/?',path):
   page=(ROOT/'public'/('onboarding.html' if path=='/onboarding' else 'index.html')).read_text()
   for role in ROLES:
    if path=='/roles/'+role['id']:
     import html
     page=page.replace('CBGPU Careers — 학교의 일상을 바꾸는 팀',html.escape(role['title'])+' | CBGPU Careers')
   return self.send(200,page,'text/html; charset=utf-8')
  self.send(404,{'error':'찾을 수 없는 페이지예요.'})
 def do_POST(self):
  if urlsplit(self.path).path!='/api/applications':return self.send(404,{'error':'Not found'})
  if self.headers.get('Origin')!=ORIGIN:return self.send(403,{'error':'모집 사이트에서 직접 제출해주세요.'})
  if self.headers.get_content_type()!='application/json':return self.send(415,{'error':'JSON 요청이 필요해요.'})
  try:
   length=int(self.headers.get('Content-Length','0'))
   if length<1 or length>30000:return self.send(413,{'error':'지원서 크기가 너무 커요.'})
   data=json.loads(self.rfile.read(length));fields=validate(data);now=int(time.time())
   ip=signature(self.headers.get('CF-Connecting-IP') or self.client_address[0]);digest=signature(data['token'])
   with connect() as c:
    c.execute('BEGIN IMMEDIATE')
    existing=c.execute('SELECT id FROM applications WHERE token_hash=?',(digest,)).fetchone()
    if existing:return self.send(200,{'reference':existing['id']})
    if c.execute('SELECT count(*) FROM requests WHERE ip=? AND created>?',(ip,now-3600)).fetchone()[0]>=5:return self.send(429,{'error':'접수 요청이 많아요. 한 시간 뒤 다시 시도해주세요.'},'application/json; charset=utf-8',{'Retry-After':'3600'})
    ref='CB-'+time.strftime('%y%m%d')+'-'+secrets.token_hex(4).upper()
    c.execute('INSERT INTO applications(id,token_hash,created,role,name,email,affiliation,motivation,experience,portfolio,consent_version) VALUES(?,?,?,?,?,?,?,?,?,?,?)',(ref,digest,now,data['role'],*(fields[k] for k in ['name','email','affiliation','motivation','experience','portfolio']),'2026-11-04'))
    c.execute('INSERT INTO requests VALUES(?,?)',(ip,now));c.commit()
   self.send(201,{'reference':ref})
  except (ValueError,TypeError,UnicodeDecodeError) as e:self.send(400,{'error':str(e) if isinstance(e,ValueError) and not isinstance(e,json.JSONDecodeError) else '지원서 형식을 확인해주세요.'})
  except Exception:self.send(503,{'error':'접수를 완료하지 못했어요. 잠시 후 다시 시도해주세요.'})
 def setup(self):
  super().setup();self.connection.settimeout(15)

def process_outbox():
 env_path=os.environ.get('CAREERS_MAIL_ENV')
 values={}
 if env_path:
  for line in Path(env_path).read_text().splitlines():
   if '=' in line and not line.startswith('#'):
    k,v=line.split('=',1);values[k]=v.strip('"\'')
 key=values.get('SMTP_PASS') or os.environ.get('RESEND_API_KEY')
 with connect() as c:
  now=int(time.time());c.execute('DELETE FROM applications WHERE created<?',(now-90*86400,));c.execute('DELETE FROM requests WHERE created<?',(now-86400,));c.commit()
  rows=c.execute('SELECT id,role FROM applications WHERE notified=0 ORDER BY created LIMIT 10').fetchall() if key else []
 for row in rows:
  role=next(r for r in ROLES if r['id']==row['role'])
  body={'from':'CBGPU Careers <info@cbgpu.com>','to':['info@cbgpu.com'],'subject':f"[CBGPU 지원 접수] {role['name']} · {row['id']}",'html':f"<div style='font-family:Arial,sans-serif;max-width:560px;margin:auto;padding:32px'><img src='https://careers.cbgpu.com/logo.png' width='40' alt='CBGPU'><h1>새로운 동료의 지원서가 도착했어요.</h1><p>{role['name']}</p><p>접수 번호: <b>{row['id']}</b></p><p>개인정보 보호를 위해 지원 내용은 메일에 포함하지 않았어요. 개발자 터미널에서 careers 관리 명령으로 확인해주세요.</p><a href='https://developers.cbgpu.com' style='background:#3182f6;color:white;padding:14px 20px;display:inline-block;border-radius:10px;text-decoration:none'>개발자 페이지 열기</a></div>"}
  req=urllib.request.Request('https://api.resend.com/emails',data=json.dumps(body).encode(),headers={'Authorization':'Bearer '+key,'Content-Type':'application/json','Idempotency-Key':'careers-'+row['id']},method='POST')
  try:
   with urllib.request.urlopen(req,timeout=20) as response:json.load(response)
   with connect() as c:c.execute('UPDATE applications SET notified=1 WHERE id=?',(row['id'],))
  except Exception:print('Recruitment notification pending; retrying later.',flush=True);break

def worker():
 while True:
  try:process_outbox()
  except Exception:print('Recruitment maintenance will retry.',flush=True)
  time.sleep(60)

def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--port',type=int,default=3530);parser.add_argument('--list',action='store_true');parser.add_argument('--show');parser.add_argument('--delete');parser.add_argument('--status',choices=['received','contacted','joined','closed']);args=parser.parse_args();initialize()
 if args.list:
  with connect() as c:print(json.dumps([dict(x) for x in c.execute('SELECT id,role,name,email,status,created,notified FROM applications ORDER BY created DESC')],ensure_ascii=False,indent=2))
 elif args.show:
  with connect() as c:
   if args.status:c.execute('UPDATE applications SET status=? WHERE id=?',(args.status,args.show))
   row=c.execute('SELECT id,created,role,name,email,affiliation,motivation,experience,portfolio,status,notified FROM applications WHERE id=?',(args.show,)).fetchone();print(json.dumps(dict(row) if row else None,ensure_ascii=False,indent=2))
 elif args.delete:
  with connect() as c:c.execute('DELETE FROM applications WHERE id=?',(args.delete,));print('Deleted if present.')
 else:
  threading.Thread(target=worker,daemon=True).start();server=ThreadingHTTPServer(('127.0.0.1',args.port),Handler);server.daemon_threads=True;print(f'CBGPU Careers listening on 127.0.0.1:{args.port}',flush=True);server.serve_forever()
if __name__=='__main__':main()

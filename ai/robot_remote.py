"""Owner-only robot registry, outbound device sync, control leases and stale-goal rejection."""
import base64,hashlib,hmac,json,secrets,threading,time,uuid
from collections import defaultdict,deque
from core import Failure,valid_id
from remote_protocol import grid,pose,free_goal,check,finite
class RemoteRobots:
 def __init__(self,store):
  self.store=store;self.rates=defaultdict(deque);self.lock=threading.RLock()
  store.db.executescript('''CREATE TABLE IF NOT EXISTS remote_robots(id TEXT PRIMARY KEY,owner TEXT NOT NULL,name TEXT NOT NULL,created REAL NOT NULL,revoked INTEGER DEFAULT 0,pair_hash TEXT,pair_until REAL,token_hash TEXT,agent_session TEXT,seen REAL DEFAULT 0,lease TEXT,lease_until REAL DEFAULT 0,telemetry TEXT,map TEXT,image TEXT,stop_epoch INTEGER DEFAULT 0); CREATE INDEX IF NOT EXISTS remote_robots_owner ON remote_robots(owner); CREATE TABLE IF NOT EXISTS robot_commands(id TEXT PRIMARY KEY,device TEXT NOT NULL,kind TEXT NOT NULL,state TEXT NOT NULL,created REAL NOT NULL,expires REAL NOT NULL,lease TEXT,payload TEXT,error TEXT,updated REAL NOT NULL); CREATE INDEX IF NOT EXISTS robot_commands_device ON robot_commands(device,created);''');store.db.commit()
  if 'accepted_at' not in {r['name'] for r in store.execute('PRAGMA table_info(robot_commands)').fetchall()}:store.execute('ALTER TABLE robot_commands ADD COLUMN accepted_at REAL')
 def hash(self,value):return hashlib.sha256(value.encode()).hexdigest()
 def owned(self,owner,id):
  row=self.store.execute('SELECT * FROM remote_robots WHERE id=? AND owner=? AND revoked=0',(id,owner)).fetchone()
  if not row:raise Failure('기기를 찾을 수 없어요.',404)
  return dict(row)
 def list(self,owner):return [{'id':r['id'],'name':r['name'],'online':time.time()-r['seen']<4,'paired':bool(r['token_hash']),'last_seen':r['seen']} for r in self.store.execute('SELECT id,name,seen,token_hash FROM remote_robots WHERE owner=? AND revoked=0 ORDER BY created DESC LIMIT 20',(owner,)).fetchall()]
 def public(self,row):return {'id':row['id'],'name':row['name'],'created':row['created'],'online':time.time()-row['seen']<4,'last_seen':row['seen'],'paired':bool(row['token_hash']),'control_until':row['lease_until'],'telemetry':json.loads(row['telemetry']) if row['telemetry'] else None,'map':json.loads(row['map']) if row['map'] else None,'image':row['image'],'command':self.latest(row['id']),'stop_epoch':row['stop_epoch']}
 def latest(self,id):
  row=self.store.execute('SELECT id,kind,state,created,expires,payload,error,updated,accepted_at FROM robot_commands WHERE device=? ORDER BY created DESC LIMIT 1',(id,)).fetchone()
  return {**dict(row),'payload':json.loads(row['payload'])} if row else None
 def create(self,owner,name):
  if not isinstance(name,str) or not 1<=len(name.strip())<=60:raise Failure('기기 이름은 1–60자로 입력하세요.')
  with self.store.lock:
   if self.store.execute('SELECT count(*) FROM remote_robots WHERE owner=? AND revoked=0',(owner,)).fetchone()[0]>=10:raise Failure('기기는 최대 10개 등록할 수 있어요.',429)
   id=str(uuid.uuid4());code=secrets.token_urlsafe(24);now=time.time();self.store.execute('INSERT INTO remote_robots(id,owner,name,created,pair_hash,pair_until) VALUES(?,?,?,?,?,?)',(id,owner,name.strip(),now,self.hash(code),now+600));return {'device':self.public(self.owned(owner,id)),'pair_code':code,'expires_at':now+600}
 def re_pair(self,owner,id):
  self.stop(owner,id);code=secrets.token_urlsafe(24);self.store.execute('UPDATE remote_robots SET token_hash=NULL,pair_hash=?,pair_until=?,agent_session=NULL,seen=0 WHERE id=? AND owner=?',(self.hash(code),time.time()+600,id,owner));return {'pair_code':code,'expires_at':time.time()+600}
 def state(self,owner,id):
  row=self.owned(owner,id)
  if time.time()-row['seen']>4 or row['lease_until']<time.time():self.store.execute("UPDATE robot_commands SET state='cancelled',error='Device or operator disconnected',updated=? WHERE device=? AND state IN ('queued','dispatched','running')",(time.time(),id))
  return self.public(row)
 def control(self,owner,id,body):
  with self.store.lock:
   row=self.owned(owner,id);lease=body.get('lease');now=time.time()
   if body.get('release') is True:
    if row['lease']!=lease:raise Failure('제어 권한이 없어요.',409)
    self.stop(owner,id);return {'released':True}
   if row['lease_until']>now and row['lease']!=lease:raise Failure('다른 창에서 제어 중이에요.',409)
   if not valid_id(lease):raise Failure('Invalid control lease')
   if now-row['seen']>4:raise Failure('기기가 오프라인이에요.',409)
   self.store.execute('UPDATE remote_robots SET lease=?,lease_until=? WHERE id=?',(lease,now+10,id));return {'lease':lease,'expires_at':now+10}
 def goal(self,owner,id,body):
  with self.store.lock:
   row=self.owned(owner,id);now=time.time();telemetry=json.loads(row['telemetry'] or '{}');m=json.loads(row['map'] or 'null')
   if now-row['seen']>2 or row['lease_until']<now or row['lease']!=body.get('lease'):raise Failure('연결·제어 권한을 다시 확인하세요.',409)
   if not telemetry.get('armed') or not telemetry.get('ready') or telemetry.get('emergency_stop') or not telemetry.get('navigation_enabled'):raise Failure('라즈에서 이동을 활성화하고 지도·센서 상태를 확인하세요.',409)
   if not m or body.get('map_session')!=m['session'] or (type(body.get('map_version')) is not int or body['map_version']>m['version']):raise Failure('지도가 바뀌었어요. 새 지도에서 목표를 다시 선택하세요.',409)
   try:free_goal(m,body.get('x'),body.get('y'));check(finite(body.get('yaw',0)) and abs(body.get('yaw',0))<=math_tau(),'Invalid goal orientation')
   except ValueError as e:raise Failure(str(e))
   if self.store.execute("SELECT count(*) FROM robot_commands WHERE device=? AND state IN ('queued','dispatched','running')",(id,)).fetchone()[0]:raise Failure('현재 목표를 취소한 뒤 새 목표를 보내세요.',409)
   command=str(uuid.uuid4());payload={'x':body['x'],'y':body['y'],'yaw':body.get('yaw',0),'frame':m['frame'],'map_session':m['session'],'map_version':m['version']};self.store.execute('INSERT INTO robot_commands(id,device,kind,state,created,expires,lease,payload,error,updated) VALUES(?,?,?,?,?,?,?,?,?,?)',(command,id,'navigate','queued',now,now+15,row['lease'],json.dumps(payload),None,now));return {'command':self.latest(id)}
 def stop(self,owner,id):
  with self.store.lock:
   self.owned(owner,id);now=time.time();self.store.execute("UPDATE robot_commands SET state='cancelled',error='Stopped by operator',updated=? WHERE device=? AND state IN ('queued','dispatched','running')",(now,id));self.store.execute('UPDATE remote_robots SET stop_epoch=stop_epoch+1,lease_until=0,lease=NULL WHERE id=?',(id,));return {'ok':True}
 def revoke(self,owner,id):self.stop(owner,id);self.store.execute('UPDATE remote_robots SET revoked=1,token_hash=NULL,pair_hash=NULL,telemetry=NULL,map=NULL,image=NULL WHERE id=? AND owner=?',(id,owner));return {'ok':True}
 def enroll(self,body):
  code=body.get('pair_code');session=body.get('agent_session')
  if not isinstance(code,str) or len(code)>100 or not valid_id(session):raise Failure('Invalid pairing request',401)
  with self.store.lock:
   row=self.store.execute('SELECT * FROM remote_robots WHERE pair_hash=? AND pair_until>? AND revoked=0',(self.hash(code),time.time())).fetchone()
   if not row:raise Failure('연결 코드가 잘못되었거나 만료됐어요.',401)
   token='cbgpu_robot_'+secrets.token_urlsafe(40);self.store.execute('UPDATE remote_robots SET pair_hash=NULL,pair_until=0,token_hash=?,agent_session=?,seen=0 WHERE id=?',(self.hash(token),session,row['id']));return {'device_id':row['id'],'device_token':token,'name':row['name']}
 def auth(self,token):
  if not isinstance(token,str) or not token.startswith('cbgpu_robot_') or len(token)>150:raise Failure('Device authentication required',401)
  row=self.store.execute('SELECT * FROM remote_robots WHERE token_hash=? AND revoked=0',(self.hash(token),)).fetchone()
  if not row:raise Failure('Device credential revoked or invalid',401)
  return dict(row)
 def sync(self,body):
  row=self.auth(body.get('device_token'));session=body.get('agent_session');now=time.time()
  with self.lock:
   q=self.rates[row['id']]
   while q and q[0]<now-60:q.popleft()
   if len(q)>=600:raise Failure('Device sync rate exceeded',429)
   q.append(now)
  if not valid_id(session):raise Failure('Invalid agent session')
  with self.store.lock:
   row=dict(self.store.execute('SELECT * FROM remote_robots WHERE id=?',(row['id'],)).fetchone())
   if row['agent_session']!=session and now-row['seen']<5:raise Failure('Another agent is connected',409)
   if row['agent_session']!=session:self.store.execute('UPDATE remote_robots SET agent_session=?,stop_epoch=stop_epoch+1,lease_until=0 WHERE id=?',(session,row['id']))
   t=body.get('telemetry');check_data(t)
   old=json.loads(row['map'] or 'null');new=body.get('map')
   if new is not None:
    try:new=grid(new)
    except ValueError as e:raise Failure(str(e))
    if old and new['session']==old['session'] and new['version']<old['version']:raise Failure('Map version moved backwards',409)
    if old and new['session']==old['session'] and new['version']==old['version'] and new!=old:raise Failure('Map content changed without a new version',409)
    if old and new['session']!=old['session']:
     self.store.execute("UPDATE robot_commands SET state='cancelled',error='Mapping session changed',updated=? WHERE device=? AND state IN ('queued','dispatched','running')",(now,row['id']));self.store.execute('UPDATE remote_robots SET stop_epoch=stop_epoch+1,lease_until=0 WHERE id=?',(row['id'],))
    old=new
   if not old or t.get('map_session')!=old['session'] or t.get('map_version')!=old['version']:t['ready']=False
   image=body.get('image')
   if image is not None:
    if not isinstance(image,str) or len(image)>350000:raise Failure('Image exceeds 256 KB',413)
    try:raw=base64.b64decode(image,validate=True);check(raw.startswith(b'\xff\xd8') and raw.endswith(b'\xff\xd9') and len(raw)<=256000,'Use a bounded JPEG preview')
    except Exception:raise Failure('Invalid JPEG preview')
   self.store.execute('UPDATE remote_robots SET seen=?,agent_session=?,telemetry=?,map=?,image=CASE WHEN ? IS NULL THEN image ELSE ? END WHERE id=?',(now,session,json.dumps(t),json.dumps(old) if old else None,image,image,row['id']))
   ack=body.get('ack')
   if ack is not None:
    if not isinstance(ack,dict) or not valid_id(ack.get('id')) or ack.get('state') not in ['running','succeeded','failed','cancelled']:raise Failure('Invalid command acknowledgement')
    self.store.execute("UPDATE robot_commands SET state=?,error=?,updated=?,accepted_at=coalesce(accepted_at,?) WHERE id=? AND device=? AND state IN ('dispatched','running')",(ack['state'],str(ack.get('error',''))[:300] or None,now,now,ack['id'],row['id']))
   self.store.execute("UPDATE robot_commands SET state='expired',error='Goal delivery expired',updated=? WHERE device=? AND state IN ('queued','dispatched') AND expires<?",(now,row['id'],now))
   current=dict(self.store.execute('SELECT * FROM remote_robots WHERE id=?',(row['id'],)).fetchone());live=current['lease_until']>now
   if not live:self.store.execute("UPDATE robot_commands SET state='cancelled',error='Operator control lease expired',updated=? WHERE device=? AND state IN ('queued','dispatched','running')",(now,row['id']))
   command=self.store.execute("SELECT * FROM robot_commands WHERE device=? AND state IN ('queued','dispatched') AND expires>? ORDER BY created LIMIT 1",(row['id'],now)).fetchone() if live else None
   if command:self.store.execute("UPDATE robot_commands SET state='dispatched',updated=? WHERE id=?",(now,command['id']))
   self.store.execute('DELETE FROM robot_commands WHERE updated<?',(now-7*86400,));return {'server_time':now,'operator_live':live,'stop_epoch':current['stop_epoch'],'command':{'id':command['id'],'expires_at':command['expires'],'payload':json.loads(command['payload'])} if command else None}
def math_tau():return 6.283185307179586
def check_data(t):
 if not isinstance(t,dict):raise Failure('Telemetry must be an object')
 try:
  check(len(json.dumps(t,allow_nan=False))<=16000,'Telemetry is too large');check(all(type(t.get(k)) is bool for k in ['armed','ready','emergency_stop','navigation_enabled']),'Telemetry state flags must be booleans')
  if t.get('pose') is not None:t['pose']=pose(t['pose'])
  check(finite(t.get('sensor_age_s')) and t['sensor_age_s']>=0,'Invalid sensor age')
  if t['sensor_age_s']>1 or t.get('pose') is None:t['ready']=False
  check(isinstance(t.get('map_session',''),str) and type(t.get('map_version',0)) is int,'Invalid map telemetry')
 except (ValueError,TypeError) as e:raise Failure(str(e))

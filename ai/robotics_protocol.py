"""CBGPU Robotics v1: bounded capability contracts, never executable server code."""
import hashlib,json,math,re,time,uuid
VERSION=1
class Invalid(ValueError):pass

def require(ok,message):
 if not ok:raise Invalid(message)
def bounded(value,limit,name):
 try:raw=json.dumps(value,allow_nan=False,ensure_ascii=False)
 except (TypeError,ValueError,RecursionError):raise Invalid(name+' must be finite JSON')
 require(len(raw.encode())<=limit,name+' is too large');return raw
def schema_check(s,depth=0):
 require(isinstance(s,dict) and depth<=4,'Invalid parameter schema')
 t=s.get('type');allowed={'type','description','enum'}
 require(t in ['object','array','string','number','integer','boolean'],'Unsupported schema type')
 if t=='object':
  allowed|={'properties','required','additionalProperties'};p=s.get('properties');r=s.get('required',[])
  require(isinstance(p,dict) and len(p)<=16 and s.get('additionalProperties') is False,'Objects need properties and additionalProperties: false')
  require(isinstance(r,list) and all(isinstance(k,str) and k in p for k in r) and len(r)==len(set(r)),'Invalid required fields')
  for k,v in p.items():require(bool(re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]{0,39}',k)),'Invalid parameter name');schema_check(v,depth+1)
 elif t=='array':
  allowed|={'items','maxItems','minItems'};require(type(s.get('maxItems')) is int and 0<=s['maxItems']<=16,'Arrays require maxItems <= 16');require(type(s.get('minItems',0)) is int and 0<=s.get('minItems',0)<=s['maxItems'],'Invalid array minimum');schema_check(s.get('items'),depth+1)
 elif t=='string':
  allowed|={'maxLength','minLength'};require(type(s.get('maxLength')) is int and 1<=s['maxLength']<=256,'Strings require maxLength <= 256');require(type(s.get('minLength',0)) is int and 0<=s.get('minLength',0)<=s['maxLength'],'Invalid string minimum')
 elif t in ['number','integer']:
  allowed|={'minimum','maximum'}
  for key in ['minimum','maximum']:require(type(s.get(key)) in [int,float] and math.isfinite(s[key]),'Numbers require finite bounds')
  require(s['minimum']<=s['maximum'],'Reversed numeric bounds')
 require(set(s)<=allowed,'Unsupported schema keyword')
 require('description' not in s or isinstance(s['description'],str) and len(s['description'])<=300,'Invalid schema description')
 if 'enum' in s:
  require(isinstance(s['enum'],list) and 1<=len(s['enum'])<=32,'Invalid enum')
  for v in s['enum']:validate(v,{k:x for k,x in s.items() if k!='enum'})

def validate(v,s):
 t=s['type']
 if t=='object':
  require(isinstance(v,dict) and set(v)<=set(s['properties']) and set(s.get('required',[]))<=set(v),'Unexpected or missing arguments')
  for k,x in v.items():validate(x,s['properties'][k])
 elif t=='array':
  require(isinstance(v,list) and s.get('minItems',0)<=len(v)<=s['maxItems'],'Invalid array length')
  for x in v:validate(x,s['items'])
 elif t=='string':require(isinstance(v,str) and s.get('minLength',0)<=len(v)<=s['maxLength'],'Invalid string')
 elif t=='boolean':require(type(v) is bool,'Expected boolean')
 else:require(type(v) in ([int] if t=='integer' else [int,float]) and math.isfinite(v) and s['minimum']<=v<=s['maximum'],'Number outside allowed bounds')
 if 'enum' in s:require(any(type(v) is type(x) and v==x for x in s['enum']),'Value is not an allowed choice')

def capabilities(value):
 bounded(value,14000,'Capabilities');require(isinstance(value,list) and 1<=len(value)<=12,'Provide 1–12 capabilities');names=set()
 for c in value:
  require(isinstance(c,dict) and set(c)=={'name','description','parameters','max_duration_s'},'Invalid capability fields')
  require(isinstance(c['name'],str) and re.fullmatch(r'[a-z][a-z0-9_]{0,39}',c['name']) and c['name'] not in names,'Invalid or duplicate capability');names.add(c['name'])
  require(isinstance(c['description'],str) and 1<=len(c['description'])<=300,'Invalid capability description')
  require(type(c['max_duration_s']) in [int,float] and math.isfinite(c['max_duration_s']) and 0<c['max_duration_s']<=30,'Capability duration must be <= 30 seconds');schema_check(c['parameters']);require(c['parameters']['type']=='object','Capability parameters must be an object')
 return value

def digest(value):return hashlib.sha256(json.dumps(capabilities(value),sort_keys=True,separators=(',',':'),ensure_ascii=False,allow_nan=False).encode()).hexdigest()
def robot_id(value):require(isinstance(value,str) and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_-]{0,79}',value),'Invalid robot_id');return value

def proposal(value,caps):
 bounded(value,12000,'Plan');require(isinstance(value,dict) and set(value)=={'summary','needs_clarification','question','steps'},'Invalid plan fields')
 require(isinstance(value['summary'],str) and len(value['summary'])<=1000,'Invalid plan summary');require(type(value['needs_clarification']) is bool and isinstance(value['question'],str) and len(value['question'])<=1000,'Invalid clarification')
 steps=value['steps'];require(isinstance(steps,list) and len(steps)<=6,'Maximum 6 steps');mapping={c['name']:c for c in capabilities(caps)};total=0
 require((value['needs_clarification'] and bool(value['question'].strip()) and not steps) or (not value['needs_clarification'] and bool(steps)),'A plan needs steps, or an explicit clarification question')
 for step in steps:
  require(isinstance(step,dict) and set(step)=={'capability','arguments','duration_s'},'Invalid step fields');require(isinstance(step['capability'],str) and step['capability'] in mapping,'Unknown capability');c=mapping[step['capability']];validate(step['arguments'],c['parameters']);duration=step['duration_s'];require(type(duration) in [int,float] and math.isfinite(duration) and 0<duration<=c['max_duration_s'],'Invalid step duration');total+=duration
 require(total<=60,'Plan duration exceeds 60 seconds');return value

def envelope(value,caps,expected_robot,now=None):
 now=time.time() if now is None else now;require(isinstance(value,dict),'Invalid mission');require(value.get('protocol_version')==VERSION,'Unsupported protocol')
 try:require(str(uuid.UUID(value['id']))==value['id'],'Invalid mission ID')
 except (KeyError,ValueError,TypeError,AttributeError):raise Invalid('Invalid mission ID')
 require(value.get('robot_id')==robot_id(expected_robot) and value.get('capabilities_digest')==digest(caps),'Robot or capability mismatch')
 for k in ['issued_at','expires_at','observed_at']:require(type(value.get(k)) in [int,float] and math.isfinite(value[k]),'Invalid mission timestamp')
 require(now-120<=value['observed_at']<=now+5 and value['observed_at']<=value['issued_at']+5 and now-120<=value['issued_at']<=now+5 and now<value['expires_at']<=min(value['issued_at']+120,value['observed_at']+120),'Expired or stale mission')
 require(value.get('requires_local_approval') is True,'Local approval is required');return proposal(value.get('plan'),caps)

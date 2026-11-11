"""Read-only web search and bounded public-page excerpts. No shell, browser JS, private IPs or credentials."""
import hashlib,html,http.client,ipaddress,json,re,socket,ssl,threading,time,urllib.parse,xml.etree.ElementTree as ET
from html.parser import HTMLParser
from core import Failure

class Text(HTMLParser):
 def __init__(self):super().__init__();self.parts=[];self.hidden=0
 def handle_starttag(self,tag,attrs):
  if tag in ['script','style','noscript','svg']:self.hidden+=1
 def handle_endtag(self,tag):
  if tag in ['script','style','noscript','svg']:self.hidden=max(0,self.hidden-1)
 def handle_data(self,data):
  if not self.hidden:self.parts.append(data)

class Results(HTMLParser):
 def __init__(self):super().__init__();self.hits=[];self.capture='';self.parts=[];self.url=''
 def handle_starttag(self,tag,attrs):
  attrs=dict(attrs)
  if tag=='a' and 'result-link' in attrs.get('class',''):
   self.capture='title';self.parts=[];link=urllib.parse.urljoin('https://lite.duckduckgo.com',attrs.get('href',''));self.url=urllib.parse.parse_qs(urllib.parse.urlsplit(link).query).get('uddg',[link])[0]
  if tag=='td' and 'result-snippet' in attrs.get('class','') and self.hits:self.capture='snippet';self.parts=[]
 def handle_data(self,data):
  if self.capture:self.parts.append(data)
 def handle_endtag(self,tag):
  if tag=='a' and self.capture=='title':
   self.hits.append({'title':' '.join(' '.join(self.parts).split())[:180],'url':self.url,'snippet':'','content':''});self.capture=''
  elif tag=='td' and self.capture=='snippet':
   snippet=' '.join(' '.join(self.parts).split())[:500];self.hits[-1].update(snippet=snippet,content=snippet);self.capture=''

def public_target(url):
 parsed=urllib.parse.urlsplit(url)
 if parsed.scheme not in ['https','http'] or not parsed.hostname or parsed.username or parsed.password or parsed.port not in [None,80,443] or len(url)>4096:raise Failure('Only public HTTP/HTTPS pages are supported')
 host=parsed.hostname.encode('idna').decode();port=parsed.port or (443 if parsed.scheme=='https' else 80)
 records=socket.getaddrinfo(host,port,type=socket.SOCK_STREAM);addresses=list(dict.fromkeys(r[4][0] for r in records))
 if not addresses or any(not ipaddress.ip_address(address).is_global for address in addresses):raise Failure('Private and internal network addresses are not allowed')
 return parsed,host,port,addresses[0]

def fetch_public(url,maximum=1024*1024):
 for _ in range(3):
  parsed,host,port,address=public_target(url)
  if parsed.scheme=='https':
   class Pinned(http.client.HTTPSConnection):
    def connect(self):self.sock=self._context.wrap_socket(socket.create_connection((address,port),timeout=8),server_hostname=host)
   connection=Pinned(host,port,timeout=8,context=ssl.create_default_context())
  else:
   class Pinned(http.client.HTTPConnection):
    def connect(self):self.sock=socket.create_connection((address,port),timeout=8)
   connection=Pinned(host,port,timeout=8)
  try:
   connection.request('GET',urllib.parse.urlunsplit(('', '',parsed.path or '/',parsed.query,'')),headers={'Host':host,'User-Agent':'Mozilla/5.0 (compatible; CBGPU-Search/1.0)','Accept':'text/html,application/rss+xml,application/xml,text/plain','Accept-Encoding':'identity'})
   sock=connection.sock
   def timeout():
    try:sock.shutdown(socket.SHUT_RDWR)
    except Exception:pass
   timer=threading.Timer(12,timeout);timer.daemon=True;timer.start()
   response=connection.getresponse()
   if response.status in [301,302,303,307,308]:url=urllib.parse.urljoin(url,response.getheader('Location') or '');continue
   if response.status!=200:raise Failure('This public page could not be read',502)
   content_type=response.getheader('Content-Type','').lower()
   if not any(t in content_type for t in ['text/','xml','html']):raise Failure('Only text web pages are supported')
   body=response.read(maximum+1)
   if len(body)>maximum:raise Failure('Web page is too large')
   return body.decode('utf8',errors='replace'),url
  finally:
   if 'timer' in locals():timer.cancel()
   connection.close()
 raise Failure('Too many redirects')

class WebSearch:
 def __init__(self):self.gate=threading.BoundedSemaphore(2);self.lock=threading.Lock();self.cache={};self.usage={}
 def search(self,owner,query,compare=False):
  if not isinstance(query,str) or not query.strip():raise Failure('검색할 질문을 입력해 주세요.')
  query=query.strip()[:500];query=re.sub(r'cbgpu_ai_[A-Za-z0-9_-]+|sk-[A-Za-z0-9_-]+|eyJ[A-Za-z0-9_.-]{30,}','[credential removed]',query)
  key=(owner,compare,hashlib.sha256(query.encode()).hexdigest());now=time.time()
  with self.lock:
   cached=self.cache.get(key)
   if cached and now-cached[0]<120:return cached[1]
   recent=[v for v in self.usage.get(owner,[]) if now-v<60]
   if len(recent)>=6:raise Failure('웹 검색은 분당 6번까지 이용할 수 있어요.',429)
   self.usage[owner]=recent+[now]
  if not self.gate.acquire(blocking=False):raise Failure('다른 웹 검색을 처리 중이에요. 잠시 후 다시 시도해 주세요.',429)
  try:
   hits=[]
   direct=re.search(r'https?://[^\s<>]+',query)
   if direct:
    try:
     body,url=fetch_public(direct.group(0).rstrip('.,)'));parser=Text();parser.feed(body);content=' '.join(' '.join(parser.parts).split())[:2200];hits.append({'title':urllib.parse.urlsplit(url).hostname,'url':url,'snippet':content[:250],'content':content})
    except Exception:pass
   body,_=fetch_public('https://lite.duckduckgo.com/lite/?q='+urllib.parse.quote(query),512*1024)
   if 'anomaly.js' in body or 'anomaly-modal' in body:raise Failure('검색엔진에서 요청을 제한했어요. 잠시 후 다시 시도해 주세요.',503)
   parser=Results();parser.feed(body)
   for hit in parser.hits[:8]:
    try:public_target(hit['url'])
    except Exception:continue
    if hit['title'] and not any(h['url']==hit['url'] for h in hits):hits.append(hit)
   if not hits:raise Failure('웹 검색 결과를 찾지 못했어요. 질문을 바꾸거나 웹 검색을 꺼 주세요.',502)
   from concurrent.futures import ThreadPoolExecutor
   def enrich(hit):
    try:
     body,url=fetch_public(hit['url']);parser=Text();parser.feed(body);content=' '.join(' '.join(parser.parts).split())[:1800]
     if content:hit={**hit,'url':url,'content':content}
    except Exception:pass
    return hit
   if compare:
    # Prefer different publishers before fetching full page excerpts.
    unique=[]
    domains=set()
    for hit in hits:
     host=urllib.parse.urlsplit(hit['url']).hostname or ''
     domain=host.removeprefix('www.')
     if domain in domains:continue
     domains.add(domain);unique.append(hit)
    hits=unique+ [hit for hit in hits if hit not in unique]
   count=4 if compare else 2
   with ThreadPoolExecutor(max_workers=2) as pool:hits=list(pool.map(enrich,hits[:count]))+hits[count:]
   result={'query':query,'sources':hits[:6],'searched_at':time.time(),'provider':'DuckDuckGo web search'}
   with self.lock:
    self.cache[key]=(now,result)
    if len(self.cache)>200:self.cache={k:v for k,v in self.cache.items() if now-v[0]<120}
    if len(self.usage)>1000:self.usage={k:[v for v in values if now-v<60] for k,values in self.usage.items() if any(now-v<60 for v in values)}
   return result
  except Failure:raise
  except Exception:raise Failure('웹 검색에 연결하지 못했어요. 검색을 끄거나 잠시 후 다시 시도해 주세요.',502)
  finally:self.gate.release()

"""Account-owned conversation history backed by the existing private GPU queue."""
import json,time,uuid
from core import Failure,LIVE,valid_id

class Conversations:
    def __init__(self,store):
        self.store=store
        store.db.executescript('''CREATE TABLE IF NOT EXISTS conversations(id TEXT PRIMARY KEY,owner TEXT NOT NULL,title TEXT NOT NULL,created REAL NOT NULL,updated REAL NOT NULL);
        CREATE INDEX IF NOT EXISTS conversations_owner ON conversations(owner,updated);
        CREATE TABLE IF NOT EXISTS chat_messages(id TEXT PRIMARY KEY,conversation TEXT NOT NULL,role TEXT NOT NULL,content TEXT NOT NULL,job_id TEXT,created REAL NOT NULL);
        CREATE INDEX IF NOT EXISTS messages_conversation ON chat_messages(conversation,created);''');store.db.commit()
    def list(self,owner):
        return [dict(r) for r in self.store.execute('SELECT id,title,created,updated FROM conversations WHERE owner=? ORDER BY updated DESC LIMIT 100',(owner,)).fetchall()]
    def create(self,owner):
        if not valid_id(owner):raise Failure('Invalid owner',401)
        with self.store.lock:
            if len(self.list(owner))>=100:raise Failure('대화가 100개예요. 사용하지 않는 대화를 삭제해 주세요.',429)
            id=str(uuid.uuid4());now=time.time();self.store.execute('INSERT INTO conversations VALUES(?,?,?,?,?)',(id,owner,'새 대화',now,now))
            return self.get(owner,id)
    def owned(self,owner,id):
        row=self.store.execute('SELECT * FROM conversations WHERE id=? AND owner=?',(id,owner)).fetchone()
        if not row:raise Failure('대화를 찾을 수 없어요.',404)
        return dict(row)
    def get(self,owner,id):
        chat=self.owned(owner,id);messages=[]
        for r in self.store.execute('SELECT * FROM chat_messages WHERE conversation=? ORDER BY created,id',(id,)).fetchall():
            message=dict(r);message['state']='succeeded'
            if r['job_id'] and not r['content']:
                try:
                    job=self.store.job(owner,r['job_id']);message.update(state=job['state'],progress=job.get('progress',''),error=job.get('error'),truncated=bool((job.get('result') or {}).get('truncated')))
                    result=job.get('result') or {};choices=result.get('choices') or []
                    if job['state']=='succeeded' and choices:
                        message['content']=choices[0]['message']['content'];message['truncated']=message['truncated'] or choices[0].get('finish_reason')=='length'
                        self.store.execute('UPDATE chat_messages SET content=? WHERE id=?',(message['content'],r['id']))
                except Failure:message.update(state='failed',error='답변 보관 기간이 지났어요. 질문을 다시 보내 주세요.')
            messages.append(message)
        chat.pop('owner',None);chat['messages']=messages;return chat
    def rename(self,owner,id,title):
        self.owned(owner,id)
        if not isinstance(title,str) or not title.strip() or len(title)>100:raise Failure('대화 제목은 1~100자로 입력해 주세요.')
        self.store.execute('UPDATE conversations SET title=?,updated=? WHERE id=? AND owner=?',(title.strip(),time.time(),id,owner));return self.get(owner,id)
    def delete(self,owner,id):
        with self.store.lock:
            self.owned(owner,id)
            self.store.execute("UPDATE jobs SET state='deleted',updated=? WHERE owner=? AND id IN (SELECT job_id FROM chat_messages WHERE conversation=?)",(time.time(),owner,id))
            self.store.execute('DELETE FROM chat_messages WHERE conversation=?',(id,));self.store.execute('DELETE FROM conversations WHERE id=? AND owner=?',(id,owner))
    def send(self,owner,id,content):
        if not isinstance(content,str) or not content.strip() or len(content)>8000:raise Failure('질문은 1~8,000자로 입력해 주세요.')
        with self.store.lock:
            chat=self.get(owner,id);rows=chat['messages']
            if len(rows)>=200:raise Failure('새 대화에서 이어가 주세요. 한 대화에 최대 100번 질문할 수 있어요.')
            if any(r['state'] in LIVE for r in rows):raise Failure('이 대화의 답변을 기다리거나 생성을 중지해 주세요.',409)
            system={'role':'system','content':'당신은 학교생활, 연구와 개발을 돕는 CBGPU AI입니다. 사용자의 언어로 명확하고 친절하게 답하세요. 모르는 사실은 모른다고 말하고, 제공되지 않은 자료를 읽었다고 주장하지 마세요. Markdown과 코드 블록을 적절하게 사용하세요.'}
            prior=[{'role':r['role'],'content':r['content']} for r in rows if r['state']=='succeeded' and r['content']]
            # A failed/cancelled answer must not leave consecutive unanswered user turns in context.
            prior=[r for i,r in enumerate(prior) if r['role']=='assistant' or i+1<len(prior) and prior[i+1]['role']=='assistant']
            context=[];size=len(content)+len(system['content'])
            for message in reversed(prior):
                if size+len(message['content'])>22000 or len(context)>=28:break
                context.insert(0,message);size+=len(message['content'])
            if context and context[0]['role']=='assistant':context.pop(0)
            job=self.store.enqueue(owner,'chat',{'messages':[system,*context,{'role':'user','content':content.strip()}],'max_tokens':1536},chat['title'] if rows else content[:70])
            now=time.time();self.store.execute('INSERT INTO chat_messages VALUES(?,?,?,?,?,?)',(str(uuid.uuid4()),id,'user',content.strip(),None,now));self.store.execute('INSERT INTO chat_messages VALUES(?,?,?,?,?,?)',(str(uuid.uuid4()),id,'assistant','',job['id'],now+0.0001))
            self.store.execute('UPDATE conversations SET updated=?,title=? WHERE id=?',(now,chat['title'] if rows else content.strip()[:70],id))
            result=self.get(owner,id);result['context_truncated']=len(context)<len(prior);return result

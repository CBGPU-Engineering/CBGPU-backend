"""Real llama.cpp token streaming, with sampled-token accounting and bounded atomic snapshots."""
import time
from core import atomic_json
class PartialWriter:
    def __init__(self,folder):self.folder=folder;self.text='';self.last=0
    def __call__(self,delta):
        if not delta:return
        self.text+=delta
        if time.monotonic()-self.last>=.08:self.flush()
    def flush(self):atomic_json(self.folder/'partial.json',{'text':self.text,'updated':time.time()});self.last=time.monotonic()

def complete(model,options,emit=None,chat=True):
    method=model.create_chat_completion if chat else model.create_completion
    if emit is None:return method(**options)
    from llama_cpp import LogitsProcessorList
    class Count:
        prompt=0;sampled=0
        def __call__(self,ids,scores):
            if not self.sampled:self.prompt=len(ids)
            self.sampled+=1;return scores
    count=Count();text='';base={};finish='stop'
    for chunk in method(**options,stream=True,logits_processor=LogitsProcessorList([count])):
        base={k:chunk[k] for k in ['id','created','model'] if k in chunk};choices=chunk.get('choices') or []
        if not choices:continue
        choice=choices[0];delta=(choice.get('delta') or {}).get('content','') if chat else choice.get('text','')
        if delta:text+=delta;emit(delta)
        if choice.get('finish_reason'):finish=choice['finish_reason']
    output={**base,'object':'chat.completion' if chat else 'text_completion','choices':[{'index':0,**({'message':{'role':'assistant','content':text}} if chat else {'text':text}),'finish_reason':finish}],'usage':{'prompt_tokens':count.prompt,'completion_tokens':count.sampled,'total_tokens':count.prompt+count.sampled}}
    return output

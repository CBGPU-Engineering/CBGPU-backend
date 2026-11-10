"""Pinned GPT-OSS with native Harmony reasoning/tools. Only final answers or validated tool proposals leave the worker."""
import json,re,sys,time,uuid
from incremental import complete

def final_text(raw):
    marker='<|channel|>final<|message|>'
    if marker not in raw:raise ValueError('추론 예산 안에 최종 답변을 완성하지 못했어요. 노력 정도를 낮추거나 질문을 나눠 주세요.')
    return re.split(r'<\|(?:return|fim_suffix|end|start)\|>',raw.rsplit(marker,1)[1])[0].strip()

def schema_type(schema,depth=0):
    if not isinstance(schema,dict) or depth>5:return 'unknown'
    if isinstance(schema.get('enum'),list):return ' | '.join(json.dumps(v,ensure_ascii=False) for v in schema['enum'][:30]) or 'unknown'
    if schema.get('type') in ['integer','number']:return 'number'
    if schema.get('type') in ['string','boolean','null']:return schema['type']
    if schema.get('type')=='array':return 'Array<'+schema_type(schema.get('items',{}),depth+1)+'>'
    if schema.get('type')=='object' or 'properties' in schema:
        required=schema.get('required',[])
        return '{'+','.join(json.dumps(k)+('' if k in required else '?')+':'+schema_type(v,depth+1) for k,v in schema.get('properties',{}).items())+'}'
    alternatives=schema.get('anyOf') or schema.get('oneOf')
    return ' | '.join(schema_type(v,depth+1) for v in alternatives[:10]) if isinstance(alternatives,list) else 'unknown'

def tool_header(raw,aliases):
    current=raw.rsplit('<|start|>assistant',1)[-1];header,separator,body=current.partition('<|message|>')
    match=re.search(r'to=functions\.([A-Za-z0-9_]+)',header)
    if not match:return None
    alias=match.group(1)
    if alias not in aliases:raise ValueError('Unknown GPT-OSS tool proposal')
    content=re.split(r'<\|(?:call|end|return|fim_suffix)\|>',body)[0].strip();arguments=json.loads(content)
    if not isinstance(arguments,dict):raise ValueError('Tool arguments must be an object')
    return {'id':'call_'+uuid.uuid4().hex[:24],'type':'function','function':{'name':aliases[alias],'arguments':json.dumps(arguments,ensure_ascii=False)}}

def chat(state,messages,max_tokens,model_path,tools=None,tool_choice='auto',effort='medium',emit=None):
    runtime=state/'llama-next'
    if not runtime.is_dir():raise ValueError('GPT-OSS runtime is not installed')
    sys.path.insert(0,str(runtime))
    from llama_cpp import Llama
    model=Llama(model_path=str(model_path),n_gpu_layers=-1,n_ctx=8192,n_batch=128,n_threads=4,flash_attn=False,verbose=False)
    tools=tools or [];enabled=bool(tools) and tool_choice!='none';aliases={'f'+str(i)+'_'+re.sub('[^A-Za-z0-9_]','_',t['function']['name']):t['function']['name'] for i,t in enumerate(tools)};names={v:k for k,v in aliases.items()}
    def token(text,special=False):return model.tokenize(text.encode('utf8'),add_bos=False,special=special)
    systems='You are a helpful assistant.\nReasoning: '+effort+'\n'
    if enabled:systems+="Calls to these tools must go to the commentary channel: 'functions'.\n"
    systems+='\n\n'.join(m.get('content') or '' for m in messages if m['role']=='system')
    project=re.search(r'CBGPU_PROJECT_ROOT: ([^\n]+)',systems)
    if project:systems='Actual project root: '+project.group(1)+'. Never invent /path/to paths.\n'+systems
    if tool_choice=='required':systems+='\nYou must call one available function this turn.'
    if isinstance(tool_choice,dict):systems+='\nCall only functions.'+names[tool_choice['function']['name']]+' this turn.'
    declarations='namespace functions {\n'
    for tool in tools:
        f=tool['function'];declarations+='// '+str(f.get('description','')).replace('\n',' ')[:200]+'\ntype '+names[f['name']]+' = (_: '+schema_type(f.get('parameters',{}))+') => any;\n'
    declarations+='} // namespace functions'
    budget=8192-max_tokens-200;system_tokens=token(systems);declaration_tokens=token(declarations) if enabled else []
    if len(declaration_tokens)>int(budget*.55):raise ValueError('Too many tool definitions for this model context')
    cap=min(len(system_tokens),max(100,int(budget*.7)-len(declaration_tokens)));truncated=len(system_tokens)>cap;system_tokens=system_tokens[:cap];budget-=len(system_tokens)+len(declaration_tokens)
    segments=[];call_names={}
    for message in messages:
        role=message['role']
        if role=='system':continue
        if role=='assistant' and message.get('tool_calls'):
            for call in message['tool_calls']:
                name=call['function']['name'];alias=names.get(name,'legacy_tool');call_names[call['id']]=alias;segments.append(('assistant to=functions.'+alias+'<|channel|>commentary<|constrain|>json',call['function']['arguments'],'<|call|>'))
        elif role=='tool':segments.append(('functions.'+call_names.get(message.get('tool_call_id'),'legacy_tool')+' to=assistant<|channel|>commentary',message.get('content') or '','<|end|>'))
        else:segments.append(('assistant<|channel|>final' if role=='assistant' else 'user',message.get('content') or '','<|end|>'))
    turns=[]
    for header,text,end in reversed(segments):
        content=token(text)
        if len(content)>budget:
            if turns:truncated=True;break
            content=content[:budget];truncated=True
        turns.insert(0,(header,content,end));budget-=len(content)
        if budget<=0:truncated=True;break
    prompt=token('<|start|>system<|message|>',True)+system_tokens+token('<|end|>',True)
    if enabled:prompt+=token('<|start|>developer<|message|>',True)+declaration_tokens+token('<|end|>',True)
    for header,content,end in turns:prompt+=token('<|start|>'+header+'<|message|>',True)+content+token(end,True)
    prompt+=token('<|start|>assistant',True)
    streamed='';visible=0
    def delta(text):
        nonlocal streamed,visible
        streamed+=text;marker='<|channel|>final<|message|>'
        if emit and tool_choice not in ['required'] and not isinstance(tool_choice,dict) and marker in streamed:
            answer=streamed.rsplit(marker,1)[1];answer=re.split(r'<\|(?:return|fim_suffix|end|start)\|>',answer)[0].lstrip()
            if len(answer)>visible:emit(answer[visible:]);visible=len(answer)
    result=complete(model,{'prompt':prompt,'max_tokens':max_tokens,'temperature':.3,'stop':['<|return|>','<|fim_suffix|>','<|call|>']},delta if emit else None,chat=False)
    raw=result['choices'][0]['text'];call=tool_header(raw,aliases) if enabled else None
    if call:
        if isinstance(tool_choice,dict) and call['function']['name']!=tool_choice['function']['name']:raise ValueError('Model did not select the required function')
        message={'role':'assistant','content':None,'tool_calls':[call]};finish='tool_calls'
    else:
        if tool_choice=='required' or isinstance(tool_choice,dict):raise ValueError('Model did not return the required function')
        message={'role':'assistant','content':final_text(raw)};finish=result['choices'][0].get('finish_reason','stop')
    return {'id':'chatcmpl-'+uuid.uuid4().hex,'object':'chat.completion','created':int(time.time()),'model':'cbgpu-oss','choices':[{'index':0,'message':message,'finish_reason':finish}],'usage':result.get('usage',{}),'truncated':truncated,'cbgpu_effort':effort,'review_passes':0,'native_reasoning':True}

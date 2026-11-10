"""Text-only tool protocol adapter. It returns proposals; never executes a tool on the host."""
import json,re,uuid

def validate_tools(tools):
    if not isinstance(tools,list) or not 1<=len(tools)<=20 or len(json.dumps(tools))>24000:raise ValueError('Use 1–20 tool definitions within 24 KB')
    names=set()
    for tool in tools:
        f=tool.get('function',{}) if isinstance(tool,dict) else {}
        if tool.get('type')!='function' or not isinstance(f.get('name'),str) or not re.fullmatch(r'[A-Za-z_][A-Za-z0-9_-]{0,63}',f['name']) or f['name'] in names or not isinstance(f.get('parameters',{}),dict):raise ValueError('Invalid function tool')
        names.add(f['name'])
    return tools

def prepare(messages,tools,choice):
    output=[]
    for m in messages:
        role=m['role'];content=m.get('content') or ''
        if role=='tool':role='user';content='Tool result for '+str(m.get('tool_call_id',''))+':\n'+content
        if m.get('tool_calls'):content+='\nRequested tools: '+json.dumps(m['tool_calls'],ensure_ascii=False)
        output.append({'role':role,'content':content})
    if not tools or choice=='none':return output,False
    required=choice=='required' or isinstance(choice,dict)
    instruction='You are a coding agent with these available function tools. Never claim a tool ran before its result is provided. Return EXACTLY one JSON object. To request a tool: {"type":"tool","name":"FUNCTION_NAME","arguments":{...}}. To answer without a tool: {"type":"final","content":"your answer"}. '+('You MUST request one tool this turn. ' if required else '')+'Tools:\n'+json.dumps(tools,ensure_ascii=False)
    if isinstance(choice,dict):instruction+='\nUse only function: '+choice['function']['name']
    for message in messages:
        match=re.search(r'CBGPU_PROJECT_ROOT: ([^\n]+)',message.get('content') or '')
        if match:instruction='Actual project root: '+match.group(1)+'. Use this real path, never invent /path/to paths.\n'+instruction;break
    output.insert(0,{'role':'system','content':instruction});return output,True

def decode(result,tools,choice):
    message=result['choices'][0]['message'];raw=json.loads(message['content']);kind=raw.get('type');names={t['function']['name'] for t in tools}
    if kind=='tool':
        name=raw.get('name');args=raw.get('arguments')
        if name not in names or not isinstance(args,dict):raise ValueError('Model returned an invalid tool proposal')
        if isinstance(choice,dict) and name!=choice['function']['name']:raise ValueError('Model selected a different required tool')
        message.update(content=None,tool_calls=[{'id':'call_'+uuid.uuid4().hex[:24],'type':'function','function':{'name':name,'arguments':json.dumps(args,ensure_ascii=False)}}]);result['choices'][0]['finish_reason']='tool_calls'
    elif kind=='final' and choice not in ['required'] and not isinstance(choice,dict) and isinstance(raw.get('content'),str):message['content']=raw['content'];result['choices'][0]['finish_reason']='stop'
    else:raise ValueError('Model did not return the requested tool/answer format')
    return result

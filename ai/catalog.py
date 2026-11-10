"""Public model identifiers map only to pinned local assets, never caller-supplied paths."""
import json,os
from pathlib import Path
TEXT_MODELS={'cbgpu-oss':('oss','GPT-OSS 20B','OpenAI gpt-oss 20B · 추론과 분석'),'cbgpu-chat':('chat','CBGPU Chat','Qwen2.5 7B · 빠른 대화'),'cbgpu-coder':('coder','CBGPU Coder','Qwen2.5 Coder 7B · 코드와 에이전트'),'cbgpu-pro':('pro','CBGPU Pro','Qwen2.5 14B · 글쓰기와 분석')}
def model_file(state,id):
    if id not in TEXT_MODELS:raise ValueError('Unknown chat model')
    config=json.loads(Path(__file__).with_name('models.json').read_text())[TEXT_MODELS[id][0]]
    return Path(state)/'models'/TEXT_MODELS[id][0]/config['files'][0]
def models(state=None):
    state=Path(state or os.environ.get('CBGPU_AI_STATE',Path.home()/'.local/share/cbgpu-ai'));items=[]
    for id,(_,name,description) in TEXT_MODELS.items():
        config=json.loads(Path(__file__).with_name('models.json').read_text())[TEXT_MODELS[id][0]]
        ready=(id!='cbgpu-oss' or (state/'llama-next').is_dir()) and all((state/'models'/TEXT_MODELS[id][0]/f).is_file() for f in config['files'])
        items.append({'id':id,'object':'model','owned_by':'cbgpu','name':name,'description':description,'ready':ready,'context_window':8192,'max_output_tokens':4096 if id=='cbgpu-oss' else 2048,'capabilities':['text','tools']})
    items.extend([{'id':'cbgpu-embeddings','object':'model','owned_by':'cbgpu','name':'Multilingual E5','ready':(state/'models/embed/model.safetensors').exists(),'capabilities':['embeddings']},{'id':'cbgpu-whisper','object':'model','owned_by':'cbgpu','name':'Whisper large-v3','ready':(state/'models/transcribe/model.bin').exists(),'capabilities':['transcription']}]);return items

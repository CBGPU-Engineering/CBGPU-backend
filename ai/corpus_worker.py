"""CPU-only indexing of an already approved shared/public file. No GPU or network."""
import json,os,sys
from pathlib import Path

def extract(file,name):
    if name.lower().endswith('.pdf'):
        from pypdf import PdfReader
        reader=PdfReader(file)
        if reader.is_encrypted:raise ValueError('Encrypted PDF')
        text='\n'.join(page.extract_text() or '' for page in list(reader.pages)[:100])
    elif name.lower().endswith(('.txt','.md','.csv','.tex','.py','.log')):
        with file.open('r',encoding='utf8',errors='replace') as f:text=f.read(200000)
    else:raise ValueError('Unsupported document format')
    text=text[:200000]
    return [' '.join(text[i:i+650].split()) for i in range(0,len(text),500) if text[i:i+650].strip()]

def main():
    file,name,state,output=sys.argv[1:];chunks=extract(Path(file),name)
    if not chunks:raise ValueError('No extractable text')
    from sentence_transformers import SentenceTransformer
    model=SentenceTransformer(str(Path(state)/'models/embed'),device='cpu',local_files_only=True,trust_remote_code=False)
    vectors=model.encode(['passage: '+s for s in chunks],batch_size=8,normalize_embeddings=True,show_progress_bar=False)
    Path(output).write_text(json.dumps([{'text':text,'vector':v.tolist()} for text,v in zip(chunks,vectors)],ensure_ascii=False));Path(output).chmod(0o600)
if __name__=='__main__':main()

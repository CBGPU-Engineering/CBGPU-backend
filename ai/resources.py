"""Reservation-first allocation. Never stop, reset, attach to or write into a user's notebook."""
import csv, datetime, json, os, signal, subprocess, time
from pathlib import Path
from core import Failure, valid_id

def run(args,timeout=8):
    return subprocess.run(args,capture_output=True,text=True,timeout=timeout,check=True).stdout

def reservation_snapshot():
    query="SELECT coalesce(json_agg(json_build_object('gpu',gpu_id,'start',extract(epoch from start_time),'end',extract(epoch from end_time))),'[]'::json) FROM public.reservations WHERE status='approved' AND end_time>now() AND start_time<now()+interval '5 minutes'"
    return json.loads(run(['docker','exec','cbgpu-supabase-db','psql','-X','-U','supabase_admin','-d','postgres','-At','-v','ON_ERROR_STOP=1','-c',query]))

def owner_allowed(id):
    if not valid_id(id):return False
    query=f"SELECT EXISTS(SELECT 1 FROM public.profiles p JOIN auth.users u ON u.id=p.id WHERE p.id='{id}'::uuid AND p.is_approved IS DISTINCT FROM FALSE AND u.deleted_at IS NULL AND (u.banned_until IS NULL OR u.banned_until<=now()))"
    return run(['docker','exec','cbgpu-supabase-db','psql','-X','-U','supabase_admin','-d','postgres','-At','-c',query]).strip()=='t'

def devices():
    text=run(['nvidia-smi','--query-gpu=index,uuid,name,memory.used,utilization.gpu','--format=csv,noheader,nounits'])
    return [{'index':int(r[0]),'uuid':r[1].strip(),'name':r[2].strip(),'memory':int(r[3]),'utilization':int(r[4])} for r in csv.reader(text.splitlines())]

def processes():
    text=run(['nvidia-smi','--query-compute-apps=gpu_uuid,pid','--format=csv,noheader,nounits'])
    return [{'uuid':r[0].strip(),'pid':int(r[1])} for r in csv.reader(text.splitlines()) if len(r)==2]

def snapshot():
    reservations=reservation_snapshot();cards=devices();pids=processes();blocked={int(r['gpu']) for r in reservations}
    for card in cards:
        card['reserved']=card['index']+1 in blocked
        card['available']=not card['reserved'] and card['memory']<100 and not any(p['uuid']==card['uuid'] for p in pids)
    return cards,pids

def prepare_workspace(store,job,data):
    from core import extract_zip, safe_relative
    folder=store.root/'jobs'/job['id'];workspace=folder/'workspace';workspace.mkdir(exist_ok=True,mode=0o700);(workspace/'output').mkdir(exist_ok=True,mode=0o700)
    if not (workspace/'.prepared').exists():
        if data.get('upload_id'):extract_zip(store.root/'uploads'/data['upload_id'],workspace)
        if data.get('code'): (workspace/'main.py').write_text(data['code'])
        (workspace/'.prepared').touch()
    entry=workspace/str(safe_relative(data.get('entrypoint','main.py')))
    if not entry.is_file() or entry.is_symlink() or workspace.resolve() not in entry.resolve().parents:raise Failure('Entrypoint was not found in the ZIP')
    return workspace

def start_task(store,job,gpu,python,runner,image):
    folder=store.root/'jobs'/job['id'];data=json.loads((folder/'input.json').read_text());log=(folder/'log.txt').open('ab',buffering=0)
    if job['kind']=='python':
        workspace=prepare_workspace(store,job,data);name='cbgpu-ai-'+job['id']
        args=['docker','run','--rm','--name',name,'--label','cbgpu.ai.managed=true','--label','cbgpu.ai.job='+job['id'],'--gpus','device='+gpu['uuid'],'--network','none','--cap-drop','ALL','--security-opt','no-new-privileges','--read-only','--pids-limit','256','--memory','16g','--cpus','4','--shm-size','256m','--tmpfs','/tmp:rw,nosuid,nodev,size=256m','--user',f'{os.getuid()}:{os.getgid()}','--mount',f'type=bind,src={workspace},dst=/workspace','--workdir','/workspace','--env','HOME=/tmp','--env','PYTHONDONTWRITEBYTECODE=1','--env','OMP_NUM_THREADS=4',image,'python','-u',data.get('entrypoint','main.py')]
    else:
        args=[str(python),str(runner),job['kind'],str(folder),str(store.root)]
    site=Path(python).parent.parent/'lib/python3.12/site-packages'
    libraries=[str(p) for p in (site/'nvidia').glob('*/lib')]+[str(site/'torch/lib')]
    env={'PATH':'/usr/bin:/bin','HOME':str(store.root),'LANG':'C.UTF-8','CUDA_VISIBLE_DEVICES':gpu['uuid'],'OMP_NUM_THREADS':'4','OPENBLAS_NUM_THREADS':'4','MKL_NUM_THREADS':'4','TOKENIZERS_PARALLELISM':'false','HF_HUB_OFFLINE':'1','HF_HUB_DISABLE_TELEMETRY':'1','LD_LIBRARY_PATH':':'.join(libraries)}
    try:process=subprocess.Popen(['prlimit','--fsize=67108864','--',*args],stdout=log,stderr=log,env=env,start_new_session=True)
    except BaseException:log.close();raise
    return process,log

def stop_task(process,job):
    if job['kind']=='python':
        name='cbgpu-ai-'+job['id']
        try:
            label=run(['docker','inspect','--format','{{index .Config.Labels "cbgpu.ai.job"}}',name],3).strip()
            if label==job['id']:run(['docker','stop','-t','3',name],6)
        except (subprocess.SubprocessError,OSError):pass
    if process.poll() is None:
        try:os.killpg(process.pid,signal.SIGTERM)
        except ProcessLookupError:pass
        try:process.wait(4)
        except subprocess.TimeoutExpired:
            try:os.killpg(process.pid,signal.SIGKILL)
            except ProcessLookupError:pass
            process.wait(3)

def own_pids(process,job):
    if job['kind']!='python':return {process.pid}
    try:return {int(p.strip()) for p in run(['docker','top','cbgpu-ai-'+job['id'],'-eo','pid'],3).splitlines()[1:] if p.strip().isdigit()}
    except (subprocess.SubprocessError,OSError):return set()

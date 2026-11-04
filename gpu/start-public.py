"""Start an additional base-path-aware notebook server without stopping existing kernels."""
import ast,json,os,pathlib,subprocess,sys
number=int(sys.argv[1]);base=pathlib.Path('/root/.jupyter');base.mkdir(mode=0o700,exist_ok=True)
pidfile=base/'cbgpu-public.pid'
try:
 pid=int(pidfile.read_text());os.kill(pid,0)
 if b'cbgpu-public.py' in pathlib.Path('/proc/%s/cmdline'%pid).read_bytes():print('running');sys.exit(0)
except (OSError,ValueError):pass
config=base/'jupyter_server_config.py'
if not config.exists():print('password configuration not ready');sys.exit(2)
values={}
for n in ast.parse(config.read_text()).body:
 if isinstance(n,ast.Assign):
  try:values[ast.unparse(n.targets[0])]=ast.literal_eval(n.value)
  except (ValueError,TypeError):pass
password=values.get('c.ServerApp.password') or values.get('c.NotebookApp.password')
if not password:print('password configuration not ready');sys.exit(2)
settings="""c = get_config()
c.ServerApp.ip = '0.0.0.0'
c.ServerApp.port = 8889
c.ServerApp.port_retries = 0
c.ServerApp.open_browser = False
c.ServerApp.allow_root = True
c.ServerApp.root_dir = '/root'
c.ServerApp.allow_remote_access = True
c.ServerApp.trust_xheaders = True
c.ServerApp.disable_check_xsrf = False
c.ServerApp.cookie_options = {'secure': True, 'samesite': 'Lax'}
c.IdentityProvider.token = ''
"""+f"c.ServerApp.base_url = '/{number}/'\nc.PasswordIdentityProvider.hashed_password = {password!r}\n"
public=base/'cbgpu-public.py';public.write_text(settings);public.chmod(0o600)
log=open(base/'cbgpu-public.log','ab',buffering=0)
proc=subprocess.Popen(['jupyter','notebook','--config='+str(public)],stdin=subprocess.DEVNULL,stdout=log,stderr=log,start_new_session=True)
pidfile.write_text(str(proc.pid));pidfile.chmod(0o600);print('started')

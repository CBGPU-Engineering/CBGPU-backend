import json,time,base64,urllib.request,urllib.parse
from pathlib import Path
from cryptography.hazmat.primitives.serialization import load_pem_private_key
from cryptography.hazmat.primitives.asymmetric import padding
from cryptography.hazmat.primitives import hashes
root=Path(__file__).resolve().parents[1]
secrets=json.loads((root/'private/source-function-secrets.json').read_text())
account=json.loads(secrets['FIREBASE_SERVICE_ACCOUNT_JSON'])
b64=lambda value:base64.urlsafe_b64encode(json.dumps(value,separators=(',',':')).encode()).decode().rstrip('=')
now=int(time.time())
unsigned=b64({'alg':'RS256','typ':'JWT'})+'.'+b64({'iss':account['client_email'],'scope':'https://www.googleapis.com/auth/firebase.messaging','aud':account['token_uri'],'iat':now,'exp':now+300})
key=load_pem_private_key(account['private_key'].encode(),password=None)
signature=key.sign(unsigned.encode(),padding.PKCS1v15(),hashes.SHA256())
jwt=unsigned+'.'+base64.urlsafe_b64encode(signature).decode().rstrip('=')
request=urllib.request.Request(account['token_uri'],data=urllib.parse.urlencode({'grant_type':'urn:ietf:params:oauth:grant-type:jwt-bearer','assertion':jwt}).encode(),headers={'Content-Type':'application/x-www-form-urlencoded'})
with urllib.request.urlopen(request,timeout=20) as response:result=json.load(response)
assert result.get('access_token')
print('Firebase push credentials authenticated successfully; no notification sent.')

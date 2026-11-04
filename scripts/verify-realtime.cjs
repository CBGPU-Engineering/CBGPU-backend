const fs=require('fs'); const path=require('path');
const root=path.resolve(__dirname,'..');
const env=Object.fromEntries(fs.readFileSync(path.join(root,'.env'),'utf8').split('\n').filter(l=>l&&!l.startsWith('#')&&l.includes('=')).map(l=>{const i=l.indexOf('=');return[l.slice(0,i),l.slice(i+1)]}));
const ws=new WebSocket((process.argv[2]||'ws://127.0.0.1:54321')+'/realtime/v1/websocket?apikey='+encodeURIComponent(env.ANON_KEY)+'&vsn=1.0.0');
const timeout=setTimeout(()=>{console.error('Realtime websocket timeout');process.exit(1)},15000);
ws.addEventListener('open',()=>ws.send(JSON.stringify({topic:'realtime:backend-verification',event:'phx_join',payload:{config:{broadcast:{self:false},presence:{key:''},postgres_changes:[]},access_token:env.ANON_KEY},ref:'1'})));
ws.addEventListener('message',e=>{const message=JSON.parse(e.data);if(message.event==='phx_reply'&&message.ref==='1'){clearTimeout(timeout);ws.close();if(message.payload.status!=='ok'){console.error('Realtime subscription rejected:',message.payload);process.exitCode=1}else console.log('Realtime WebSocket connection and channel join: OK')}});
ws.addEventListener('error',()=>{clearTimeout(timeout);console.error('Realtime connection error');process.exit(1)});

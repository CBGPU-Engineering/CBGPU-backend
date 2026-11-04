'use strict';
(() => {
 let term=null,fit=null,socket=null,connecting=false,visible=false,generation=0,resizeObserver=null;
 const $=id=>document.getElementById(id);
 const palette=()=>document.documentElement.dataset.theme==='dark'?{background:'#17171c',foreground:'#ececef',cursor:'#8ab8ff',selectionBackground:'#334766',black:'#202027',red:'#ff9ba5',green:'#68c98e',yellow:'#e2bd72',blue:'#74aaff',magenta:'#ce9eff',cyan:'#72d4d4',white:'#ececef',brightBlack:'#929bab'}:{background:'#ffffff',foreground:'#191f28',cursor:'#004fe8',selectionBackground:'#dbe8ff',black:'#191f28',red:'#b42332',green:'#276743',yellow:'#856016',blue:'#004fe8',magenta:'#8755aa',cyan:'#077d86',white:'#e4e4e7',brightBlack:'#62626c'};
 function status(text){$('terminal-status').textContent=text}
 function send(data){if(socket?.readyState===WebSocket.OPEN)socket.send(JSON.stringify(data))}
 function size(){if(!visible||!fit)return;try{fit.fit();send({type:'resize',cols:term.cols,rows:term.rows})}catch{}}
 function setup(){if(term)return;term=new Terminal({fontFamily:'ui-monospace, SFMono-Regular, Consolas, monospace',fontSize:13,lineHeight:1.25,cursorBlink:true,scrollback:5000,theme:palette(),allowProposedApi:false,screenReaderMode:true});fit=new FitAddon.FitAddon();term.loadAddon(fit);term.open($('terminal-mount'));term.onData(data=>send({type:'input',data}));resizeObserver=new ResizeObserver(size);resizeObserver.observe($('terminal-mount'));term.writeln('Connect to open or resume your Bash workspace.');requestAnimationFrame(size)}
 async function connect(){if(connecting||socket?.readyState===WebSocket.OPEN)return;connecting=true;const attempt=++generation;setup();status('Connecting…');$('terminal-connect').disabled=true;
  try{
   const sessionResponse=await fetch('/api/session',{credentials:'same-origin',cache:'no-store'});const session=await sessionResponse.json();if(!session.authenticated)throw Error('Sign in before connecting.');
   const response=await fetch('/api/terminal/session',{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json','X-CSRF-Token':session.csrf_token},body:'{}'});const data=await response.json();if(attempt!==generation)return;if(!response.ok)throw Error(data.error||'Could not create a terminal session.');
   const ws=new WebSocket((location.protocol==='https:'?'wss://':'ws://')+location.host+'/terminal?ticket='+encodeURIComponent(data.ticket));socket=ws;
   ws.onopen=()=>{if(socket!==ws)return;connecting=false;term.clear();status('Connected · cbgpu');$('terminal-connect').disabled=true;$('terminal-disconnect').disabled=false;size();term.focus()};
   ws.onmessage=event=>{if(socket!==ws)return;try{const m=JSON.parse(event.data);if(m.type==='output')term.write(m.data);else if(m.type==='exit')term.writeln('\r\n[Shell exited: '+m.exitCode+']');else if(m.type==='error')term.writeln('\r\n'+m.message)}catch{}};
   ws.onerror=()=>{if(socket===ws)status('Connection failed')};
   ws.onclose=()=>{if(socket!==ws)return;socket=null;connecting=false;status('Disconnected');$('terminal-connect').disabled=false;$('terminal-disconnect').disabled=true;term.writeln('\r\n[Disconnected. Connect to resume your session.]')};
  }catch(e){if(attempt!==generation)return;connecting=false;status(e.message);$('terminal-connect').disabled=false}
 }
 function disconnect(){generation++;if(socket){socket.close();socket=null}connecting=false;$('terminal-connect').disabled=false;$('terminal-disconnect').disabled=true;status('Disconnected')}
 window.addEventListener('cbgpu-panel',event=>{visible=event.detail==='terminal';if(visible){setup();requestAnimationFrame(()=>{size();term.focus()})}});
 window.addEventListener('cbgpu-theme',()=>{if(term)term.options.theme=palette()});
 window.addEventListener('cbgpu-terminal-logout',()=>{disconnect();resizeObserver?.disconnect();term?.dispose();term=null;fit=null;visible=false;$('terminal-mount').replaceChildren()});
 window.addEventListener('beforeunload',()=>socket?.close());
 $('terminal-connect').addEventListener('click',connect);$('terminal-disconnect').addEventListener('click',disconnect);
 const keys={escape:'\x1b',tab:'\t',interrupt:'\x03',up:'\x1b[A',down:'\x1b[B'};
 for(const button of document.querySelectorAll('[data-terminal-key]'))button.addEventListener('click',()=>{send({type:'input',data:keys[button.dataset.terminalKey]});term?.focus()});
})();

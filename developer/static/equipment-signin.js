'use strict';
// Same account-only session API, theme, CSS and login markup as the developer portal.
(()=>{
 const form=document.getElementById('login-form'),email=document.getElementById('login-email'),password=document.getElementById('password'),button=document.getElementById('login-button'),message=document.getElementById('login-message');
 function ready(error=''){email.disabled=false;password.disabled=false;button.disabled=false;button.textContent='Sign in';message.textContent=error;message.classList.toggle('error',!!error)}
 async function session(){try{const response=await fetch('/portal/api/session',{cache:'no-store'});const value=await response.json();if(value.authenticated){location.replace('/manage/manufacturing');return}ready()}catch{ready('Unable to check your session. Please try signing in.')}}
 form.addEventListener('submit',async event=>{event.preventDefault();const credentials={email:email.value.trim(),password:password.value};button.disabled=true;email.disabled=true;password.disabled=true;password.value='';button.textContent='Signing in…';message.textContent='';try{const response=await fetch('/portal/api/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(credentials)});const result=await response.json();if(!response.ok||!result.authenticated)throw Error(result.error||'Sign-in could not be completed.');location.replace('/manage/manufacturing')}catch(error){ready(error.message);password.focus()}});
 void session();
})();

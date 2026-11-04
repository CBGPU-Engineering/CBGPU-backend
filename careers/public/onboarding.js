'use strict';
const button=document.querySelector('#copy-intro');
if(button)button.addEventListener('click',async()=>{try{await navigator.clipboard.writeText(document.querySelector('.welcome-template').textContent);button.textContent='복사했어요 ✓'}catch{button.textContent='위 양식을 직접 선택해 복사해주세요.'}});

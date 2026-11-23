export {};
'use strict';
const button=document.querySelector<HTMLElement>('#copy-intro');
if(button)button.addEventListener('click',async()=>{try{await navigator.clipboard.writeText(document.querySelector<HTMLElement>('.welcome-template').textContent);document.querySelector<HTMLElement>('#copy-intro-status').textContent='양식을 복사했어요. Slack에 붙여넣고 내용을 채워주세요.'}catch{document.querySelector<HTMLElement>('#copy-intro-status').textContent='복사하지 못했어요. 위 양식을 직접 선택해 복사해주세요.'}});

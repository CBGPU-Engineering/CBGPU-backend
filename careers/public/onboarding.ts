export {};
'use strict';
const button=document.querySelector<HTMLElement>('#copy-intro');
const status=document.querySelector<HTMLElement>('#copy-intro-status');
function feedback(message:string,fallback:string){if(status)status.textContent=message;else if(button)button.textContent=fallback}
if(button)button.addEventListener('click',async()=>{try{await navigator.clipboard.writeText(document.querySelector<HTMLElement>('.welcome-template').textContent);feedback('양식을 복사했어요. Slack에 붙여 넣고 내용을 채워 주세요.','복사했어요 ✓')}catch{feedback('복사하지 못했어요. 위 양식을 직접 선택해 복사해 주세요.','위 양식을 직접 선택해 복사해 주세요.')}});

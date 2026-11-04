import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { renderEmail } from '../email/render.ts';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const flows = {
  confirmation: ['이메일 주소를 확인해 주세요', '아래 버튼을 눌러 CBGPU 계정의 이메일 주소를 확인해 주세요.', '이메일 확인하기'],
  recovery: ['비밀번호를 다시 설정하세요', 'CBGPU 계정의 비밀번호 재설정 요청이 접수되었습니다. 아래 버튼에서 새 비밀번호를 설정할 수 있어요.', '비밀번호 재설정'],
  magic_link: ['CBGPU에 로그인하세요', '아래 버튼을 누르면 CBGPU에 로그인할 수 있어요.', 'CBGPU 로그인'],
  invite: ['CBGPU에 초대되었어요', 'CBGPU에서 학교생활에 필요한 서비스를 함께 이용하세요. 아래 버튼에서 계정 설정을 시작할 수 있어요.', '초대 수락하기'],
  email_change: ['새 이메일 주소를 확인해 주세요', 'CBGPU 계정의 이메일 주소 변경 요청이 접수되었습니다. 아래 버튼을 눌러 변경을 확인해 주세요.', '이메일 변경 확인'],
};
const folder=path.join(root,'volumes/email-templates');mkdirSync(folder,{recursive:true});
for (const [name,[title,description,buttonLabel]] of Object.entries(flows)) {
  const url='{{ .ConfirmationURL }}';
  const bodyHtml='<p style="margin:0;font-size:13px;line-height:1.8;color:#6b7280;">버튼이 작동하지 않으면 아래 링크를 복사해 브라우저에서 열어 주세요.</p><p style="margin:8px 0 0;font-size:12px;line-height:1.7;overflow-wrap:anywhere;word-break:break-all;"><a href="'+url+'" style="color:#004fe8;">'+url+'</a></p>';
  writeFileSync(path.join(folder,name+'.html'),renderEmail({title,preview:title+' · CBGPU',description,afterButtonHtml:bodyHtml,buttonLabel,buttonUrl:url,note:'직접 요청한 메일이 아니라면 링크를 누르지 않고 무시하셔도 됩니다. 이 링크를 다른 사람과 공유하지 마세요.'}));
}
writeFileSync(path.join(folder,'reauthentication.html'),renderEmail({title:'본인 확인 코드 안내',preview:'CBGPU 본인 확인 코드',description:'요청한 작업을 계속하려면 아래 코드를 CBGPU에 입력해 주세요.',bodyHtml:'<p style="padding:20px;background:#f4f6fb;border:1px solid #e4e7ec;border-radius:12px;font-size:30px;line-height:1.5;letter-spacing:6px;text-align:center;font-weight:800;color:#004fe8;">{{ .Token }}</p>',buttonLabel:'CBGPU 열기',buttonUrl:'https://cbgpu.com',note:'본인 확인 코드는 다른 사람과 공유하지 마세요. 직접 요청하지 않았다면 이 메일을 무시하셔도 됩니다.'}));
for(const [name,title,description] of [
 ['password_changed_notification','비밀번호가 변경되었어요','CBGPU 계정의 비밀번호가 변경되었습니다. 본인이 변경하지 않았다면 비밀번호를 재설정하고 문의해 주세요.'],
 ['email_changed_notification','이메일 주소가 변경되었어요','CBGPU 계정의 이메일 주소가 변경되었습니다. 본인이 변경하지 않았다면 문의해 주세요.'],
]) writeFileSync(path.join(folder,name+'.html'),renderEmail({title,preview:title+' · CBGPU',description,buttonLabel:'CBGPU 계정 확인',buttonUrl:'https://cbgpu.com/profile'}));
// Both runtimes use identical, generated markup without a runtime dependency.
const source=readFileSync(path.join(root,'email/render.ts'),'utf8');
writeFileSync(path.join(root,'../FrontEnd/cbgpu/src/lib/email.ts'),source);
writeFileSync(path.join(root,'volumes/functions/_shared/email.ts'),source);
console.log('Generated eight branded authentication templates and shared email renderers.');

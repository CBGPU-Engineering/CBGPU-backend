/** Cookie isolation at every untrusted HTTP/WS upstream boundary. No values are logged. */
import type {IncomingHttpHeaders, OutgoingHttpHeaders} from 'node:http';
export function protectedCookieName(name:string):boolean{return /^(?:sb-cbgpu-|cbgpu|__Host-cbgpu|__Secure-cbgpu)/i.test(name)}
function hostname(value:string):string{return value.trim().toLowerCase().replace(/:\d+$/,'').replace(/\.$/,'')}
function domain(value:string):string|null{const result=value.trim().replace(/^"|"$/g,'').replace(/^\./,'').toLowerCase();return result&&!result.endsWith('.')&&!result.includes('..')&&/^[a-z0-9][a-z0-9.-]*[a-z0-9]$/.test(result)?result:null}
export function safeSetCookie(value:string,requestHost:string):boolean{
 if(/[\r\n\0]/.test(value))return false;const parts=value.split(';'),pair=parts.shift()!.trim(),equal=pair.indexOf('=');if(equal<=0)return false;const name=pair.slice(0,equal).trim();if(!/^[!#$%&'*+\-.^_\x60|~0-9A-Za-z]+$/.test(name)||protectedCookieName(name))return false;
 let scoped:string|null|undefined;for(const attribute of parts){const i=attribute.indexOf('=');if(i<0)continue;if(attribute.slice(0,i).trim().toLowerCase()!=='domain')continue;if(scoped!==undefined)return false;scoped=domain(attribute.slice(i+1));if(!scoped)return false;}
 if(scoped===undefined)return true;const host=hostname(requestHost);if(!host)return false;if(host==='cbgpu.com'||host.endsWith('.cbgpu.com'))return scoped===host&&scoped!=='cbgpu.com';if(scoped==='cbgpu.com'||scoped.endsWith('.cbgpu.com'))return false;
 return scoped.includes('.')&&(host===scoped||host.endsWith('.'+scoped));
}
export function requestCookieBoundary<T extends IncomingHttpHeaders>(source:T):T{const headers={...source};for(const key of Object.keys(headers))if(key.toLowerCase().startsWith('x-cbgpu-'))delete headers[key];if(headers.cookie){const value=headers.cookie.split(';').filter(part=>{const i=part.indexOf('=');return i>0&&!protectedCookieName(part.slice(0,i).trim())}).map(part=>part.trim()).filter(Boolean).join('; ');if(value)headers.cookie=value;else delete headers.cookie;}return headers}
export function responseCookieBoundary<T extends IncomingHttpHeaders|OutgoingHttpHeaders>(source:T,host:string):T{const headers={...source};for(const key of Object.keys(headers)){if(key.toLowerCase()!=='set-cookie')continue;const raw=headers[key];const values=Array.isArray(raw)?raw:typeof raw==='string'?[raw]:[];const safe=values.filter(value=>typeof value==='string'&&safeSetCookie(value,host));if(safe.length)headers[key]=safe;else delete headers[key];}return headers}

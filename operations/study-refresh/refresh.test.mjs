import test from 'node:test';import assert from 'node:assert/strict';
import {claims,shouldRenew,validateReplacement} from './refresh.mjs';
const now=1700000000000,token=(hours,userId=12)=>'x.'+Buffer.from(JSON.stringify({exp:now/1000+hours*3600,userId})).toString('base64url')+'.x';
test('renew before expiry and after expiry, never healthy long-lived tokens',()=>{assert.equal(shouldRenew(token(25),now),false);assert.equal(shouldRenew(token(24),now),true);assert.equal(shouldRenew(token(-1),now),true);assert.throws(()=>claims('invalid'));});
test('replacement must belong to same school identity and extend expiry',()=>{assert.doesNotThrow(()=>validateReplacement(token(5),token(72),now));assert.throws(()=>validateReplacement(token(5),token(72,13),now));assert.throws(()=>validateReplacement(token(5),token(4),now));assert.throws(()=>validateReplacement(token(72),token(48),now));});

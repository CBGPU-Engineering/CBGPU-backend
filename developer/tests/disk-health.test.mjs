import test from 'node:test';
import assert from 'node:assert/strict';
import {diskHealth} from '../disk-health.mjs';
const io = entries => ({statSync: p => {if (!entries[p]) throw Error('missing'); return {dev:entries[p].dev}}, statfsSync: p => ({bsize:1,...entries[p]})});
test('combines both SSDs by capacity while retaining individual disk health', () => {
  const d = diskHealth(['root','data'], io({root:{dev:1,blocks:100,bfree:2,bavail:1},data:{dev:2,blocks:400,bfree:390,bavail:380}}));
  assert.equal(d.total_bytes,500); assert.equal(d.used_bytes,108); assert.equal(d.available_bytes,381);
  assert.equal(d.percent,21.6); assert.equal(d.healthy,false);
});
test('unmounted data directory never counts the root filesystem twice', () => {
  const disk={dev:1,blocks:100,bfree:40,bavail:35};
  const d=diskHealth(['root','data','missing'],io({root:disk,data:disk}));
  assert.equal(d.total_bytes,100); assert.equal(d.disks.length,1); assert.equal(d.healthy,true);
  assert.equal(diskHealth(['missing'],io({})).percent,null);
});

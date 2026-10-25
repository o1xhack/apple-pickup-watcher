import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const html=fs.readFileSync(new URL('../web/index.html',import.meta.url),'utf8');
const handler=html.split('\n').find(line=>line.startsWith("$('repeat-in-stock').onchange="));
function setup(api) {
 const input={checked:true,disabled:false};const messages=[];
 const c=vm.createContext({$:()=>input,api,message:s=>messages.push(s),sync:async()=>{input.checked=false;}});
 vm.runInContext('let repeatBusy=false;'+handler,c);
 return {input,messages};
}
test('repeat alert toggle prevents duplicate writes while pending and resyncs saved state',async()=>{
 let release;let calls=0;const gate=new Promise(resolve=>{release=resolve});
 const {input}=setup(async(path,body)=>{calls++;assert.equal(path,'/api/repeat-alerts');assert.equal(body.enabled,true);await gate;});
 const pending=input.onchange();assert.equal(input.disabled,true);
 await input.onchange();assert.equal(calls,1);
 release();await pending;assert.equal(input.disabled,false);assert.equal(input.checked,false);
});
test('failed repeat alert update restores server state and releases the control',async()=>{
 const {input,messages}=setup(async()=>{throw new Error('write failed')});
 await input.onchange();assert.equal(input.disabled,false);assert.equal(input.checked,false);assert.equal(messages[0],'write failed');
});

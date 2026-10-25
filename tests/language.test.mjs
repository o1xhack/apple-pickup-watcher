import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import vm from 'node:vm';
import {loadSource} from './load-source.mjs';

test('native UI translates counts, colors and labels and switches back without changing source text',()=>{
 const {t,setUiLanguage}=loadSource('src/lib/i18n.ts');
 assert.equal(t('暂停'),'暂停');
 setUiLanguage('en');
 assert.equal(t('暂停'),'Pause');
 assert.equal(t('已选 12 家门店'),'12 stores selected');
 assert.equal(t('已选 2 种容量'),'2 storage options selected');
 assert.equal(t('有货重复提醒（全部监控）'),'Repeat in-stock alerts (all targets)');
 setUiLanguage('zh');
 assert.equal(t('暂停'),'暂停');
});

test('English product display retains the Apple color; Chinese retains bilingual names',()=>{
 const en=loadSource('src/lib/display.ts',s=>s==='@/lib/i18n'?{getLanguage:()=> 'en'}:undefined);
 assert.equal(en.productLabel('iPhone Duo 1TB Star White'),'iPhone Duo 1TB Star White');
 const zh=loadSource('src/lib/display.ts');
 assert.equal(zh.productLabel('iPhone Duo 1TB Star White'),'iPhone Duo 1TB 星白色 · Star White');
});

function web(language='en',hash=''){
 const session=new Map();let cleanUrl='';
 const cache=new Map([['apw-language',language]]);let reloads=0;const select={value:language,disabled:false};const requests=[];
 const context=vm.createContext({localStorage:{getItem:k=>cache.get(k),setItem:(k,v)=>cache.set(k,v)},location:{href:'http://127.0.0.1:17842/',hash,pathname:'/',search:'',reload:()=>reloads++},history:{replaceState:(_state,_title,url)=>{cleanUrl=url}},document:{getElementById:()=>select,documentElement:{},createTreeWalker:()=>({nextNode:()=>false}),querySelectorAll:()=>[]},NodeFilter:{SHOW_TEXT:4},MutationObserver:class {observe(){}},queueMicrotask,URL,URLSearchParams,sessionStorage:{getItem:key=>session.get(key),setItem:(key,value)=>session.set(key,value)},window:{alert(){}},fetch:async(url,opts)=>{requests.push({url:String(url),...opts});return {ok:true,json:async()=>JSON.parse(opts.body)}}});
 const data=fs.readFileSync(new URL('../web/translations.json',import.meta.url),'utf8');const code=fs.readFileSync(new URL('../web/i18n.js',import.meta.url),'utf8');vm.runInContext('const translations='+data+';\n'+code,context);
 return {context,cache,select,requests,session,cleanUrl:()=>cleanUrl,reloads:()=>reloads};
}
test('remote language is authoritative across browsers; invalid values do not reload',()=>{
 const app=web('en');vm.runInContext("syncLanguage('zh')",app.context);
 assert.equal(app.cache.get('apw-language'),'zh');assert.equal(app.reloads(),1);
 vm.runInContext("syncLanguage('invalid')",app.context);assert.equal(app.reloads(),1);
});
test('web selection saves only language to backend, then follows confirmed response',async()=>{
 const app=web('zh');app.select.value='en';await app.select.onchange();
 assert.equal(app.requests.length,1);assert.deepEqual(JSON.parse(app.requests[0].body),{language:'en'});
 assert.equal(app.requests[0].headers['X-APW-Request'],'1');assert.equal(app.cache.get('apw-language'),'en');
 assert.equal(app.reloads(),1);assert.equal(app.select.disabled,false);
});

 test('LAN link credentials are kept out of URL queries and sent on API calls',async()=>{
 const token='test-access-token';const app=web('zh','#access='+token);
 assert.equal(app.cleanUrl(),'/');assert.equal(app.session.get('apw-access-token'),token);
 app.select.value='en';await app.select.onchange();
 assert.equal(app.requests[0].headers.Authorization,'Bearer '+token);
 assert.deepEqual(JSON.parse(app.requests[0].body),{language:'en'});
 });

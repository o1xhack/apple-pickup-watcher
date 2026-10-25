import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import fs from 'node:fs';
const language='const translations = '+fs.readFileSync(new URL('../web/translations.json',import.meta.url),'utf8')+';\n'+fs.readFileSync(new URL('../web/i18n.js',import.meta.url),'utf8').replace('initializeLanguage();','');
const statistics=fs.readFileSync(new URL('../web/statistics.js',import.meta.url),'utf8').split("$('tab-monitor').onclick=")[0];
function context(lang='zh') {
 const elements=new Map();const element=id=>{if(!elements.has(id))elements.set(id,{value:'',checked:false});return elements.get(id)};
 const c=vm.createContext({localStorage:{getItem:()=>lang},Date,Intl,URLSearchParams,$:element});
 vm.runInContext(language+'\n'+statistics,c);return {c,element};
}
test('LA date filters include full local dates and handle DST rather than assuming 24 hours',()=>{
 const {c}=context();assert.equal(vm.runInContext("laDay('2026-10-24')",c),Date.parse('2026-10-24T07:00:00Z'));
 assert.equal(vm.runInContext("laDay('2026-11-01',1)-laDay('2026-11-01')",c),25*3600000);
 assert.equal(vm.runInContext("laDay('2026-03-08',1)-laDay('2026-03-08')",c),23*3600000);
});
test('custom date range, deleted targets, and English labels keep their distinct meanings',()=>{
 const {c,element}=context('en');element('stats-range').value='custom';element('stats-from').value='2026-10-23';element('stats-to').value='2026-10-24';element('stats-all-targets').checked=true;
 assert.equal(vm.runInContext("statisticsParams().get('fromMs')",c),String(Date.parse('2026-10-23T07:00:00Z')));
 assert.equal(vm.runInContext("statisticsParams().get('toMs')",c),String(Date.parse('2026-10-25T07:00:00Z')-1));
 assert.equal(vm.runInContext("statisticsParams().get('allTargets')",c),'true');
 assert.equal(vm.runInContext("tr('未开放检查次数')",c),'Pickup-not-open observations');
 assert.equal(vm.runInContext("tr('未知或失败次数')",c),'Unknown / failed observations');
 assert.equal(vm.runInContext("tr('检查次数不代表手机台数；持续有货不代表多次补货；未知不等于无货。')",c),'Checks are not a count of physical phones. Repeated in-stock observations are not separate restocks. Unknown does not mean out of stock.');
 element('stats-from').value='';assert.throws(()=>vm.runInContext('statisticsParams()',c));
});

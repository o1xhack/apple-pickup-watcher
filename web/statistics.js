'use strict';
const statsTimeZone='America/Los_Angeles';
let statisticsData=null,statsOffset=0,statsToken=0;
const statsDate=ms=>new Intl.DateTimeFormat(webLanguage==='en'?'en-US':'zh-CN',{timeZone:statsTimeZone,dateStyle:'medium',timeStyle:'medium'}).format(ms);
function laDay(date,extra=0){
 const [y,m,d]=date.split('-').map(Number);const wanted=Date.UTC(y,m-1,d+extra);let stamp=wanted;
 for(let i=0;i<3;i++){const p=Object.fromEntries(new Intl.DateTimeFormat('en-US',{timeZone:statsTimeZone,year:'numeric',month:'numeric',day:'numeric',hour:'numeric',minute:'numeric',second:'numeric',hourCycle:'h23'}).formatToParts(stamp).map(x=>[x.type,x.value]));stamp+=wanted-Date.UTC(+p.year,+p.month-1,+p.day,+p.hour,+p.minute,+p.second)}
 return stamp;
}
function todayLA(){return new Intl.DateTimeFormat('en-CA',{timeZone:statsTimeZone,year:'numeric',month:'2-digit',day:'2-digit'}).format(Date.now())}
function statisticsParams(){const q=new URLSearchParams();const mode=$('stats-range').value;if(mode==='all')q.set('fromMs','0');if(mode==='today')q.set('fromMs',String(laDay(todayLA())));if(mode==='custom'){
 if(!$('stats-from').value||!$('stats-to').value)throw Error(tr('请选择开始和结束日期'));
 q.set('fromMs',String(laDay($('stats-from').value)));q.set('toMs',String(laDay($('stats-to').value,1)-1));
 }q.set('allTargets',String($('stats-all-targets').checked));return q}
function number(n){return Number(n).toLocaleString(webLanguage==='en'?'en-US':'zh-CN')}
function metric(label,value){const box=el('div',undefined,'metric');box.append(el('strong',number(value)),el('span',tr(label)));return box}
function statisticsTable(headers,rows){const wrap=el('div',undefined,'table-scroll'),table=el('table');const head=el('tr');headers.forEach(h=>head.append(el('th',tr(h))));const thead=el('thead');thead.append(head);table.append(thead);const tbody=el('tbody');for(const cells of rows){const row=el('tr');cells.forEach(c=>row.append(el('td',c)));tbody.append(row)}table.append(tbody);wrap.append(table);return wrap}
function productParts(name){const match=name.match(/^(.*?)\s+(\d+(?:GB|TB))\s*(.*)$/);return match?[match[1],match[2],colorLabel(match[3])]:[productLabel(name),'—','—']}
function reportPeriod(data){const from=data.fromMs===0?(data.firstCheckedMs||data.toMs):data.fromMs;const locale=webLanguage==='en'?'en-US':'zh-CN';const day=new Intl.DateTimeFormat(locale,{timeZone:statsTimeZone,month:'short',day:'numeric'});const year=new Intl.DateTimeFormat(locale,{timeZone:statsTimeZone,year:'numeric'});return `${day.format(from)} – ${day.format(data.toMs)} · ${year.format(data.toMs)}`}
function renderLiveStatus(){if(!snapshot)return;const counts={in_stock:0,out_of_stock:0,pending:0,unknown:0};for(const r of snapshot.rows){const a=r.availability;const k=a.kind==='unknown'?(a.reason==='pickup_not_open'?'pending':'unknown'):a.kind;if(k in counts)counts[k]++}
 const retrying=snapshot.rows.some(r=>r.availability.reason==='blocked');const pill=$('report-live');pill.textContent=tr(!snapshot.running?'已暂停':retrying?'等待重试':'监控中');pill.className='live-pill'+(!snapshot.running?' paused':retrying?' retrying':'');
 $('report-status').replaceChildren(...[['有货',counts.in_stock],['无货',counts.out_of_stock],['未开放',counts.pending],['未知',counts.unknown]].map(([label,n])=>{const item=el('div',undefined,'inventory-item');item.append(el('strong',number(n)),el('span',tr(label)));return item}));
}
function renderStatistics(data){
 statisticsData=data;$('stats-error').textContent='';$('stats-period').textContent=reportPeriod(data);
 const models=[...new Set(data.products.map(p=>productParts(p.productName)[0]))];$('report-title').textContent=models.join(' / ')||'Apple';
 $('report-scope').textContent=`${number(data.stores.length)} ${tr('家门店')} · ${number(data.products.length)} ${tr('种配置')}`;
 $('stats-metrics').replaceChildren(...[['扫描轮次',data.rounds],['检查次数',data.checks],['有货记录',data.inStockChecks],['扫描门店',data.stores.length],['监控配置',data.products.length],['请求错误',data.network.errors||0]].map(([l,v])=>metric(l,v)));
 $('report-result').textContent=tr(data.inStockChecks?'已记录到可取货库存':'暂未发现可取货库存');
 $('report-updated').textContent=data.lastCheckedMs?tr('最近采样')+' · '+new Intl.DateTimeFormat(webLanguage==='en'?'en-US':'zh-CN',{timeZone:statsTimeZone,month:'short',day:'numeric',hour:'2-digit',minute:'2-digit'}).format(data.lastCheckedMs):tr('暂无采样');
 $('stats-network').textContent=`${tr('应用全部 HTTP 请求')} ${number(data.network.requests)} · HTTP 541 ${number(data.network.blocked541)}`;renderLiveStatus();
 $('stats-products').replaceChildren(statisticsTable(['机型','容量','颜色','零件号','检查次数','有货检查次数'],data.products.map(p=>[...productParts(p.productName),p.partNumber,number(p.checks),number(p.inStockChecks)])));
 $('stats-stores').replaceChildren(statisticsTable(['门店','检查次数','有货检查次数'],data.stores.map(s=>[s.storeTitle,number(s.checks),number(s.inStockChecks)])));
 $('stats-history').replaceChildren(statisticsTable(['时间','机型 / 配置','门店','结果'],data.history.map(h=>[statsDate(h.checkedAtMs),productLabel(h.productName),h.storeTitle,`${tr(describe({kind:h.availability,reason:h.reason,message:h.message})[0])}${h.message?' · '+h.message:''}${h.discovery?' · '+tr(h.discovery==='restock_candidate'?'无货→有货候选':'首次或恢复后发现'):''}`])));
 $('stats-page').textContent=`${number(Math.min(data.offset+1,data.checks))}–${number(Math.min(data.offset+data.history.length,data.checks))} / ${number(data.checks)}`;
 $('stats-prev').disabled=data.offset===0;$('stats-next').disabled=!data.hasMore||data.offset>=10000;$('stats-share').disabled=false;
 if(!data.checks)$('stats-error').textContent=tr('此范围没有已保存的采样记录');
}
async function loadStatistics(reset=true){const token=++statsToken;if(reset)statsOffset=0;$('stats-error').textContent=tr('正在读取历史…');$('stats-share').disabled=true;try{const q=statisticsParams();if(!reset&&statisticsData){q.set('fromMs',String(statisticsData.fromMs));q.set('toMs',String(statisticsData.toMs))}q.set('offset',String(statsOffset));const value=await api('/api/statistics?'+q);if(token!==statsToken)return;renderStatistics(value)}catch(e){if(token===statsToken)$('stats-error').textContent=e.message}}
async function loadRunSummary(){renderLiveStatus()}
function showTab(statistics){document.body.classList.toggle('statistics-active',statistics);$('monitor-view').classList.toggle('hide',statistics);$('statistics-view').classList.toggle('hide',!statistics);$('tab-monitor').classList.toggle('primary',!statistics);$('tab-statistics').classList.toggle('primary',statistics);if(statistics)void loadStatistics();else if(!catalog&&snapshot)void loadCatalog(snapshot.locale).catch(e=>message(e.message))}
async function shareStatistics(){const d=statisticsData;if(!d)return;const lines=[tr('Apple 库存监控统计'),`${statsDate(d.fromMs===0?(d.firstCheckedMs||d.toMs):d.fromMs)} → ${statsDate(d.toMs)} (America/Los_Angeles)`,`${tr('扫描轮次')}: ${number(d.rounds)}; ${tr('配置×门店检查次数')}: ${number(d.checks)}; ${tr('检查目标数')}: ${number(d.targets)}`,`${tr('有货检查次数')}: ${number(d.inStockChecks)}; ${tr('无货检查次数')}: ${number(d.outOfStockChecks)}; ${tr('未开放检查次数')}: ${number(d.notOpenChecks)}; ${tr('未知或失败次数')}: ${number(d.unknownChecks)}`,`${tr('无货→有货候选')}: ${number(d.restockCandidates)}`,...d.products.map(p=>`${productLabel(p.productName)} (${p.partNumber}): ${number(p.checks)} ${tr('次检查')}, ${number(p.inStockChecks)} ${tr('次有货检查')}`),tr('扫描门店')+': '+d.stores.map(s=>s.storeTitle).join(', '),tr('检查次数不代表手机台数；持续有货不代表多次补货；未知不等于无货。')];const text=lines.join('\n');try{if(navigator.share){await navigator.share({title:tr('Apple 库存监控统计'),text})}else if(navigator.clipboard){await navigator.clipboard.writeText(text);$('stats-error').textContent=tr('统计已复制，可粘贴分享')}else{const box=el('textarea',text);document.body.append(box);box.select();if(!document.execCommand('copy'))throw Error(tr('复制失败'));box.remove();$('stats-error').textContent=tr('统计已复制，可粘贴分享')}}catch(e){if(e.name!=='AbortError')$('stats-error').textContent=e.message}}
$('tab-monitor').onclick=()=>showTab(false);$('tab-statistics').onclick=()=>showTab(true);$('stats-load').onclick=()=>loadStatistics();$('stats-range').onchange=()=>{$('stats-dates').classList.toggle('hide',$('stats-range').value!=='custom')};$('stats-all-targets').onchange=()=>loadStatistics();$('stats-prev').onclick=()=>{statsOffset=Math.max(0,statsOffset-100);loadStatistics(false)};$('stats-next').onclick=()=>{statsOffset+=100;loadStatistics(false)};$('stats-share').onclick=shareStatistics;
$('stats-capture').onclick=()=>document.body.classList.add('capture-view');$('capture-exit').onclick=()=>document.body.classList.remove('capture-view');
$('stats-from').value=todayLA();$('stats-to').value=todayLA();
void loadRunSummary();if(new URLSearchParams(location.search).get('view')==='statistics')showTab(true);
setInterval(()=>{void loadRunSummary();if(!$('statistics-view').classList.contains('hide')&&statsOffset===0&&$('stats-range').value!=='custom')void loadStatistics()},60000);

'use strict';
// The secret stays in the fragment/session storage, never URL queries or access logs.
function initializeAccess(){
 const accessFragment=new URLSearchParams((location.hash||'').slice(1)).get('access');
 if(accessFragment){sessionStorage.setItem('apw-access-token',accessFragment);history.replaceState(null,'',location.pathname+location.search)}
}
function accessHeaders(){const token=globalThis.sessionStorage?.getItem('apw-access-token');return token?{Authorization:'Bearer '+token}:{}}
const webLanguage=localStorage.getItem('apw-language')==='en'?'en':'zh';
const translationKeys=Object.keys(translations).sort((a,b)=>b.length-a.length);
function tr(text){if(webLanguage!=='en')return String(text);let out=String(text);if(translations[out])return translations[out];for(const key of translationKeys)out=out.split(key).join(translations[key]);return out.replace(/Apple 每 (\d+) seconds查询/g,'Apple checks every $1 seconds').replace(/已选/g,'selected')}
function translatePage(){document.documentElement.lang=webLanguage==='en'?'en':'zh-CN';document.title=tr('Apple 取货监控');if(webLanguage==='en'){
 const walker=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT);const nodes=[];while(walker.nextNode()){const n=walker.currentNode;if(!['SCRIPT','STYLE','OPTION'].includes(n.parentElement?.tagName)&&!n.parentElement?.closest('[translate="no"]'))nodes.push(n)}
 for(const n of nodes){const value=tr(n.nodeValue);if(value!==n.nodeValue)n.nodeValue=value}
 for(const node of document.querySelectorAll('option')){const value=tr(node.textContent);if(value!==node.textContent)node.textContent=value}
 for(const node of document.querySelectorAll('[placeholder]')){const value=tr(node.placeholder);if(value!==node.placeholder)node.placeholder=value}
 }}
function syncLanguage(language){
 if(language!=="zh"&&language!=="en")return true;
 localStorage.setItem('apw-language',language);
 if(language!==webLanguage){location.reload();return false}
 return true;
}
function initializeLanguage(){initializeAccess();const select=document.getElementById('language');select.value=webLanguage;select.onchange=async()=>{const next=select.value;select.disabled=true;try{const response=await fetch(new URL('api/language',location.href),{method:'POST',headers:{...accessHeaders(),'Content-Type':'application/json','X-APW-Request':'1'},body:JSON.stringify({language:next})});if(!response.ok)throw Error('Unable to save language');const saved=await response.json();syncLanguage(saved.language)}catch(error){select.value=webLanguage;window.alert(String(error))}finally{select.disabled=false}};translatePage();if(webLanguage==='en'){let scheduled=false;new MutationObserver(()=>{if(scheduled)return;scheduled=true;queueMicrotask(()=>{scheduled=false;translatePage()})}).observe(document.body,{childList:true,subtree:true,characterData:true})}}
initializeLanguage();

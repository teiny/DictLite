'use strict';
const $=selector=>document.querySelector(selector);
const esc=value=>String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const api=(action,payload={})=>window.__TAURI__.core.invoke('dispatch',{action,payload});
const state={revision:0,request:0,snapshot:null,groups:[],source:null,word:'',language:'auto',pinned:false};
function renderPin(){const label=state.pinned?'取消置顶':'置顶窗口';$('#pin').setAttribute('aria-pressed',String(state.pinned));$('#pin').setAttribute('aria-label',label);$('#pin').title=label;}
async function togglePin(){const button=$('#pin');if(button.disabled)return;button.disabled=true;try{state.pinned=await api('set_popup_pinned',{pinned:!state.pinned});renderPin();}finally{button.disabled=false;}}
function message(text){$('#content').innerHTML=`<p class="message">${esc(text)}</p>`;$('#open-main').hidden=true;}
function render(){
  const group=state.groups.find(g=>g.dictionary===state.source);if(!group)return;
  const first=group.entries[0];const dictionary=id=>state.snapshot.settings.dictionaries.find(d=>d.id===id)?.name||'词典';
  $('#content').innerHTML=`<h1 class="word">${esc(state.word)}</h1><div class="phonetics">${first.pronunciations.map(p=>esc(p.text)).join(' / ')} <button class="speak" aria-label="朗读词条">朗读</button></div><div class="tabs" role="tablist" aria-label="词典来源">${state.groups.map(g=>`<button class="tab ${g.dictionary===state.source?'active':''}" role="tab" aria-selected="${g.dictionary===state.source}" data-source="${esc(g.dictionary)}" title="${esc(dictionary(g.dictionary))}">${esc(dictionary(g.dictionary))}</button>`).join('')}</div>${group.entries.map((e,i)=>`<section class="variant">${group.entries.length>1?`<h3>释义 ${i+1}${e.pronunciations.length?' · '+e.pronunciations.map(p=>esc(p.text)).join(' / '):''}</h3>`:''}${e.senses.length?e.senses.map(s=>`<div class="sense"><span class="pos">${esc(s.pos)}</span><div><p>${esc(s.translation)}</p>${s.note?`<p class="note">${esc(s.note)}</p>`:''}</div></div>`).join(''):`<p class="body">${esc(e.body)}</p>`}</section>`).join('')}`;
  $('#open-main').hidden=false;
}
async function loadSource(source,request=state.request){
  const group=state.groups.find(g=>g.dictionary===source);if(!group)return;
  const entries=await Promise.all(group.entries.map((e,i)=>e.body?e:api('detail',{dictionary:source,entry_id:e.entry_id??e.id,record:i===0})));
  if(request!==state.request)return;group.entries=entries;state.source=source;render();$('#content').scrollTop=0;
}
async function lookup(payload){
  if(!payload?.query||payload.revision<=state.revision)return;state.revision=payload.revision;state.pinned=!!payload.pinned;renderPin();const request=++state.request;
  state.groups=[];message('正在查询 '+payload.query+'…');
  try{
    state.snapshot=await api('snapshot');const matches=await api('search',{query:payload.query,mode:'auto'});
    if(request!==state.request)return;
    const item=matches.find(m=>m.word.toLowerCase()===payload.query.toLowerCase())||matches[0];
    if(!item){message(state.snapshot.libraries.some(d=>d.enabled&&d.available)?`没有找到“${payload.query}”，可在主窗口中调整查询。`:'请先在主窗口导入或启用词典。');state.word=payload.query;state.language='auto';state.source=null;$('#open-main').hidden=false;return;}
    state.word=item.word;state.language=item.language;
    const entries=await api('lookup',{word:item.word,language:item.language});if(request!==state.request)return;
    state.groups=DictLiteEntries.groupSources(entries);state.source=state.groups[0]?.dictionary;
    if(!state.source){message('没有找到这个词。');return;}await loadSource(state.source,request);if(request===state.request)await api('popup_ready',{revision:state.revision});
  }catch(e){if(request===state.request)message(String(e));}
}
document.addEventListener('keydown',e=>{if(e.key==='Escape'){e.preventDefault();api('close_popup').catch(()=>{});}});
$('#close').addEventListener('click',()=>api('close_popup').catch(()=>{}));
$('#pin').addEventListener('click',()=>togglePin().catch(e=>$('#status').textContent=String(e)));
$('#open-main').addEventListener('click',()=>api('open_main_entry',{word:state.word,language:state.language,source:state.source}).catch(e=>message(String(e))));
$('#content').addEventListener('click',e=>{const b=e.target.closest('button');if(b?.dataset.source){const request=++state.request;loadSource(b.dataset.source,request).catch(e=>message(String(e)));}else if(b?.classList.contains('speak'))api('speak',{text:state.word,language:state.language==='zh'?'zh-CN':'en-US'}).catch(e=>message(String(e)));});
(async()=>{await window.__TAURI__.event.listen('popup-query',e=>lookup(e.payload));await lookup(await api('popup_query'));})().catch(e=>message(String(e)));

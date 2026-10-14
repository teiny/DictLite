'use strict';
const $ = selector => document.querySelector(selector);
const esc = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const icon = name => `<svg aria-hidden="true"><use href="#i-${name}"/></svg>`;
const api = (action, payload = {}) => window.__TAURI__.core.invoke('dispatch', {action,payload});
const state = {view:'search',snapshot:null,matches:[],cursor:0,entries:[],source:null,word:'',language:'auto',examples:false,open:false,composing:false,request:0,lookupRequest:0,selectedFile:null,conversion:null,converted:null,busy:false};
let toastTimer, saveQueue = Promise.resolve(), settingsRevision = 0;
function toast(text) {clearTimeout(toastTimer);$('#toast').textContent=String(text);$('#toast').hidden=false;toastTimer=setTimeout(()=>$('#toast').hidden=true,4500);}
const general = () => state.snapshot.settings.general;
const dictionary = id => state.snapshot.settings.dictionaries.find(d=>d.id===id);
const current = () => state.entries.find(e=>e.dictionary===state.source) || state.entries[0];
const sourceGroups = () => DictLiteEntries.groupSources(state.entries);
const sourceEntries = () => state.entries.filter(e=>e.dictionary===state.source);
function entryDefinition(item,index,total) {
  return `${total>1?`<h3 class="variant-heading">释义 ${index+1}${item.pronunciations.length?' · '+item.pronunciations.map(p=>esc(p.text)).join(' / '):''}</h3>`:''}${item.senses.length?`<div class="definition-list">${item.senses.map((s,i)=>`<div class="definition"><span class="definition-number">${String(i+1).padStart(2,'0')}</span><span class="part-of-speech">${esc(s.pos)}</span><div><p class="definition-text">${esc(s.translation)}</p>${s.note?`<p class="definition-note">${esc(s.note)}</p>`:''}</div></div>`).join('')}</div>`:`<div class="dictionary-body">${esc(item.body)}</div>`}`;
}
const isSaved = item => state.snapshot.saved.some(r=>r.word===item.word&&r.language===item.language);
function record(item) {return {word:item.word,language:item.language,gloss:item.gloss,pronunciation:item.pronunciations[0]?.text||''};}
async function refresh() {state.snapshot=await api('snapshot');$('#saved-count').textContent=state.snapshot.saved.length;$('#data-summary').textContent=`${state.snapshot.libraries.filter(d=>d.enabled&&d.available).length} 套词典`;}
function suggestions(open) {state.open=!!(open&&general().suggestions&&$('#query').value.trim());$('#suggestions').hidden=!state.open;$('#query').setAttribute('aria-expanded',String(state.open));if(state.open&&state.matches.length)$('#query').setAttribute('aria-activedescendant',`candidate-${state.cursor}`);else $('#query').removeAttribute('aria-activedescendant');}
function renderMatches() {
  $('#candidate-count').textContent=state.matches.length;
  $('#candidates').innerHTML=state.matches.length?state.matches.map((c,i)=>`<button type="button" class="candidate ${i===state.cursor?'selected':''}" role="option" aria-selected="${i===state.cursor}" tabindex="-1" id="candidate-${i}" data-candidate="${i}"><span class="candidate-copy"><span class="candidate-word">${esc(c.word)}</span><span class="candidate-gloss">${c.rank>=3?'释义匹配 · ':''}${esc(c.gloss)}</span></span>${icon('arrow')}</button>`).join(''):'<p class="candidates-empty">没有匹配的词条，可以调整查询方向或检查已启用的词典。</p>';
  $('#clear-query').hidden=!$('#query').value;
}
async function find(open=true) {const request=++state.request;const query=$('#query').value;const matches=await api('search',{query,mode:$('#language-mode').value});if(request!==state.request)return;state.matches=matches;state.cursor=0;renderMatches();suggestions(open);}
function empty(title,text,action='') {return `<div class="empty-state">${icon('book')}<h2>${esc(title)}</h2><p>${esc(text)}</p>${action?`<div class="empty-suggestions"><button class="word-chip" data-view="libraries">${action} ${icon('arrow')}</button></div>`:''}</div>`;}
function renderEntry() {
  if(!state.snapshot.settings.dictionaries.length){$('#entry').innerHTML=empty('从一本词典开始','导入 DictLite 词典，或将已有 MDX 转换后导入。','导入词典');return;}
  if(!state.snapshot.libraries.some(d=>d.enabled&&d.available)){$('#entry').innerHTML=empty('没有可用的词典','请在词典管理中启用或导入词典。','管理词典');return;}
  const item=current();
  if(!item){$('#entry').innerHTML=empty(state.word?'没有找到这个词':'输入一个词，开始查询',state.word?'可以换个词，或调整查询方向。':'支持英文单词、中文词语和释义反查。');return;}
  if(!item.body){$('#entry').innerHTML=empty('正在读取词条','');return;}
  const group=sourceEntries();const examples=group.flatMap(e=>e.examples);const related=[...new Set(group.flatMap(e=>e.related))];
  const saved=isSaved(item);const phonetics=item.pronunciations.map(p=>{const label={uk:'英',us:'美',pinyin:'拼音',other:'读音'}[p.kind]||'读音';const lang=p.kind==='uk'?'en-GB':p.kind==='us'?'en-US':item.language==='zh'?'zh-CN':'en-US';return `<span class="pronunciation"><span class="pronunciation-label">${label}</span>${esc(p.text)}<button class="audio-button" data-speak="${lang}" aria-label="${label}朗读">${icon('audio')}</button></span>`;}).join('');
  $('#entry').innerHTML=`<article class="entry-header"><div class="entry-topline"><div class="entry-path">${item.language==='zh'?'汉 → 英':'英 → 汉'}</div><div class="entry-actions"><button id="save-word" class="small-icon ${saved?'saved':''}" aria-pressed="${saved}" aria-label="${saved?'移出单词本':'加入单词本'}" title="${saved?'移出单词本':'加入单词本'}">${icon('bookmark')}</button><button id="copy-word" class="small-icon" aria-label="复制词条" title="复制词条">${icon('copy')}</button></div></div><h2 class="entry-title ${item.language==='zh'?'chinese':''}">${esc(item.word)}</h2><div class="pronunciations">${phonetics||`<button class="text-button" data-speak="${item.language==='zh'?'zh-CN':'en-US'}">${icon('audio')} 朗读</button>`}</div></article>
  <div class="source-tabs" role="tablist" aria-label="词典来源">${sourceGroups().map(e=>`<button class="source-tab ${e.dictionary===state.source?'active':''}" data-source="${esc(e.dictionary)}" role="tab" aria-selected="${e.dictionary===state.source}" title="${esc(dictionary(e.dictionary)?.name)}">${icon('book')}<span class="source-tab-name">${esc(dictionary(e.dictionary)?.name)}</span></button>`).join('')}</div>
  <div class="entry-body"><div class="definition-heading"><span class="section-title">释义</span><span>${group.length>1?`${group.length} 个条目`:(item.senses.length?`${item.senses.length} 个义项`:'')}</span></div>${group.map((entry,i)=>`<section class="entry-variant">${entryDefinition(entry,i,group.length)}</section>`).join('')}
  ${examples.length?`<div class="example-section ${state.examples?'open':''}"><button id="toggle-examples" class="example-toggle" aria-expanded="${state.examples}"><span>双语例句</span><span>${state.examples?'收起':'展开'} ${icon('chevron')}</span></button><div class="example-copy" ${state.examples?'':'hidden'}>${examples.map(e=>`<p>${esc(e.text)}</p>${e.translation?`<p>${esc(e.translation)}</p>`:''}`).join('')}</div></div>`:''}</div>
  ${related.length?`<div class="related-section"><div class="related-heading">相关词条</div><div class="related-words">${related.map(w=>`<button class="word-chip" data-word="${esc(w)}">${esc(w)}</button>`).join('')}</div>`:''}<div class="entry-footer"><span>${icon('book')}${esc(dictionary(item.dictionary)?.name)}</span></div>`;
}
async function lookup(word,language='auto',recordHistory=true) {
  const request=++state.lookupRequest;state.request++;state.word=word;state.language=language;$('#query').value=word;state.view='search';suggestions(false);
  const entries=await api('lookup',{word,language,record:recordHistory});if(request!==state.lookupRequest)return;state.entries=entries;
  if(!entries.some(e=>e.dictionary===state.source))state.source=entries[0]?.dictionary;
  if(state.entries.length)await loadSource(state.source,recordHistory,request);
  if(request!==state.lookupRequest)return;state.examples=dictionary(state.source)?.examples==='expanded';await refresh();setView('search');saveSession();
}
async function loadSource(source,recordHistory=false,request=state.lookupRequest){const group=sourceGroups().find(g=>g.dictionary===source);if(!group)return;const details=await Promise.all(group.entries.map((e,i)=>e.body?Promise.resolve(e):api('detail',{dictionary:source,entry_id:e.entry_id??e.id,record:recordHistory&&i===0})));if(request!==state.lookupRequest)return;state.entries=state.entries.map(e=>e.dictionary===source?({...e,...details.find(d=>(d.id??d.entry_id)===(e.entry_id??e.id))}):e);}
async function submit() {if(state.composing||!$('#query').value.trim())return;const oldCursor=state.open?state.cursor:0;await find(false);const candidate=state.matches[Math.min(oldCursor,state.matches.length-1)];if(candidate)await lookup(candidate.word,candidate.language);else{state.word=$('#query').value;state.entries=[];renderEntry();}}
function renderCollection() {
  const saved=state.view==='saved';const list=saved?state.snapshot.saved:state.snapshot.history;
  $('#collection-view').innerHTML=`<div class="view-heading"><div><h1>${saved?'单词本':'查词历史'}</h1><p>${saved?'把值得记住的词留下来。':general().history?'最近查过的词，随时回来查看。':'已暂停新增记录，已有记录仍然保留。'}</p></div>${!saved&&list.length?'<button id="clear-history" class="text-button">清空历史</button>':''}</div>${list.length?`<div class="collection-header">${list.length} 个词条</div><div class="collection-grid">${list.map(r=>`<button class="collection-card" data-word="${esc(r.word)}" data-language="${esc(r.language)}"><h2>${esc(r.word)}</h2><p>${esc(r.gloss)}</p><div class="card-meta"><span>${esc(r.pronunciation)}</span>${icon('arrow')}</div></button>`).join('')}</div>`:`<div class="entry-panel">${empty(saved?'单词本还是空的':'暂无查词记录',saved?'在词条右上角点击书签，即可收藏。':'查询过的词会显示在这里。')}</div>`}`;
}
function renderLibraries() {
  $('#libraries-view').innerHTML=`<div class="view-heading"><div><h1>词典管理</h1><p>名称、顺序和阅读方式，按你的习惯安排。</p></div><div class="library-heading-actions"><span class="settings-status" data-save-status>已保存</span><button id="import-dictionary" class="import-dictionary-button">${icon('folder')}导入词典</button></div></div>${state.snapshot.libraries.length?`<div class="library-list">${state.snapshot.libraries.map((d,i)=>`<article class="library-card"><div class="library-card-top"><span class="library-order">${String(i+1).padStart(2,'0')}</span><span class="library-art">${icon('book')}</span><div class="dictionary-title"><h2>${esc(d.name)}</h2><p>${d.available?`${d.entries.toLocaleString()} 个词条${d.quality === "needs_review" ? " · 有内容提示" : ""}`:esc(d.error)}</p></div><div class="dictionary-order-buttons"><button class="small-icon" data-move="${i}" data-direction="-1" ${i===0?'disabled':''} aria-label="上移${esc(d.name)}">${icon('up')}</button><button class="small-icon" data-move="${i}" data-direction="1" ${i===state.snapshot.libraries.length-1?'disabled':''} aria-label="下移${esc(d.name)}">${icon('down')}</button></div><button class="toggle" role="switch" data-enable="${esc(d.id)}" aria-label="启用${esc(d.name)}" aria-checked="${d.enabled}"></button></div><div class="dictionary-fields"><div><label for="name-${i}">显示名称</label><input class="dictionary-name" id="name-${i}" data-name="${esc(d.id)}" value="${esc(d.name)}" required></div><div><label for="example-${i}">例句展示</label><select id="example-${i}" data-examples="${esc(d.id)}"><option value="collapsed" ${d.examples==='collapsed'?'selected':''}>默认收起</option><option value="expanded" ${d.examples==='expanded'?'selected':''}>默认展开</option></select></div></div></article>`).join('')}</div>`:`<div class="entry-panel">${empty('还没有词典','点击右上角导入词典，即可开始查词。')}</div>`}`;
}
function setting(label,text,key) {return `<div class="setting-row"><div class="setting-copy"><label>${label}</label><p>${text}</p></div><button class="toggle" role="switch" data-setting="${key}" aria-label="${label}" aria-checked="${general()[key]}"></button></div>`;}
function renderSettings() {
  $('#settings-view').innerHTML=`<div class="view-heading"><div><h1>设置</h1><p>更改即时生效，自动保存。</p></div><span class="settings-status" data-save-status>已保存</span></div><section class="settings-file"><div>${icon('folder')}<div><strong>设置与迁移</strong><p>导出设置保留使用偏好。完整迁移时请同时携带词库和个人数据。</p></div></div><div class="settings-file-actions"><button id="import-settings">导入设置</button><button id="export-settings">导出设置</button></div></section><section class="settings-card"><h2>${icon('settings')}启动与后台</h2>${setting('随系统启动','登录系统后启动，在托盘中等待使用。','launchAtStartup')}${setting('关闭窗口时最小化到托盘','保留入口，方便下次唤起。','closeToTray')}<div class="setting-row"><div class="setting-copy"><label for="release-minutes">窗口隐藏后释放界面资源</label><p>等待指定分钟后释放。0 表示立即释放。</p></div><div class="minutes-control"><input id="release-minutes" type="number" min="0" max="4294967295" step="1" value="${general().releaseMinutes}" required aria-label="释放界面等待分钟数"><span>分钟</span></div></div></section><section class="settings-card"><h2>${icon('search')}查词</h2>${setting('显示搜索建议','输入时显示候选词，关闭后仍可回车查词。','suggestions')}${setting('记录查词历史','关闭后停止新增，已有记录仍然保留。','history')}${setting('Ctrl+C+C 快速查词','选中文字后快速按两次 Ctrl+C，在鼠标旁弹出简易释义。','quickLookup')}<p class="shortcut-note">${state.snapshot.shortcut_error?'快捷查词暂不可用：'+esc(state.snapshot.shortcut_error):'选中文字后快速按两次 Ctrl+C，在鼠标旁查看释义。按 Esc 或点击其他窗口关闭。'}</p></section><p class="data-directory">数据目录：${esc(state.snapshot.data_directory)}</p><button id="quit-app" class="text-button">退出 DictLite</button>`;
}
function setView(view) {state.view=view;suggestions(false);$('#search-view').hidden=view!=='search';$('#collection-view').hidden=!['saved','history'].includes(view);$('#libraries-view').hidden=view!=='libraries';$('#settings-view').hidden=view!=='settings';document.querySelectorAll('.nav-item').forEach(b=>{b.classList.toggle('active',b.dataset.view===view);if(b.dataset.view===view)b.setAttribute('aria-current','page');else b.removeAttribute('aria-current');});if(view==='search')renderEntry();else if(view==='libraries')renderLibraries();else if(view==='settings')renderSettings();else renderCollection();saveSession();}
function saveSession() {api('session',{view:state.view,word:state.word,language:state.language,source:state.source}).catch(()=>{});}
async function saveSettings() {
  const revision=++settingsRevision;
  const config=structuredClone(state.snapshot.settings);document.querySelectorAll('[data-save-status]').forEach(n=>n.textContent='正在保存…');
  saveQueue=saveQueue.catch(()=>{}).then(()=>api('save_settings',config));
  await saveQueue;if(revision!==settingsRevision)return;await refresh();document.querySelectorAll('[data-save-status]').forEach(n=>n.textContent='已保存');
}
function updateImport() {const mdx=$('#dictionary-import-format').value==='mdx';$('#dictionary-import-filename').textContent=state.selectedFile?.split(/[\\/]/).pop()||'尚未选择文件';$('#dictionary-import-filesize').textContent=state.selectedFile||'选择要导入的词典文件';$('#run-dictionary-import').disabled=!state.selectedFile||state.busy;$('#run-dictionary-import').textContent=state.converted?'导入转换结果':mdx?'转换词典':'校验并导入';$('#dictionary-import-flow').innerHTML=(mdx?['读取 MDX 文字内容','去除媒体并生成 DictLite 词典','检查结果并导入']:['校验词库格式和版本','检查词条与数据关系','导入词典']).map((s,i)=>`<div><span>${i+1}</span>${s}</div>`).join('');$('#stop-conversion').hidden=!state.busy;$('#dictionary-import-format').disabled=state.busy;$('#choose-dictionary-file').disabled=state.busy;}
function importResult(message,type='') {$('#dictionary-import-result').className=`import-result ${type}`;$('#dictionary-import-result').textContent=message;}
async function runImport() {
  if(state.busy)return;
  if($('#dictionary-import-format').value==='mdx'&&!state.converted){state.busy=true;updateImport();importResult('正在读取词典…');try{state.conversion=await api('convert',{path:state.selectedFile});}catch(e){state.busy=false;updateImport();throw e;}return;}
  const result=state.converted;
  if(result?.quality_status==='needs_review'&&!confirm('转换结果含有需要核对的字符或内容。请查看问题摘要，确认仍要导入吗？'))return;
  let acceptReview=result?.quality_status==='needs_review';
  if(!result){const meta=await api('inspect_dictionary',{path:state.selectedFile});if(meta.quality==='needs_review'){if(!confirm('这个词库含有需要核对的内容提示。确认仍要导入吗？'))return;acceptReview=true;}}
  await api('import_dictionary',{path:result?.output||state.selectedFile,accept_review:acceptReview});
  if(result)await api('cleanup_conversion',{task_id:result.task_id});
  state.converted=null;state.selectedFile=null;$('#dictionary-import-dialog').close();await refresh();setView('libraries');toast('词典已导入。');
}
async function conversionEvent({payload:v}) {
  if(v.type!=='finished'){const label={preflight:'检查文件',extracting:'转换词条',resolving:'解析词条跳转',validating:'校验输出'}[v.stage]||'处理词典';importResult(`${label}${v.total?` · ${v.processed||0} / ${v.total}`:''}…`);return;}
  state.busy=false;state.conversion=null;
  if(v.status==='succeeded'){state.converted=v;const report=v.details;importResult(`转换完成：${v.entries.toLocaleString()} 个词条，${v.aliases} 个跳转。${report?.fallback?`其中 ${report.fallback} 个以完整文字正文保存。`:''}${v.quality_status==='needs_review'?'检测到需要核对的字符或内容，请检查后决定是否导入。':''}${report?.issues?.length?'\n'+report.issues.slice(0,8).join('\n'):''}`);}
  else importResult(v.status==='cancelled'?'转换已取消。':v.error||'转换失败。','error');
  if(!$('#dictionary-import-dialog').open)$('#dictionary-import-dialog').showModal();updateImport();
}
async function click(event) {
  if(!event.target.closest('.search-area'))suggestions(false);const b=event.target.closest('button,a');if(!b)return;
  if(b.matches('.brand')){event.preventDefault();setView('search');return;}
  if(b.dataset.view){setView(b.dataset.view);return;}
  if(b.dataset.candidate!==undefined){const c=state.matches[Number(b.dataset.candidate)];await lookup(c.word,c.language);return;}
  if(b.dataset.word){$('#language-mode').value='auto';await lookup(b.dataset.word,b.dataset.language||'auto');return;}
  if(b.dataset.source){state.source=b.dataset.source;renderEntry();await loadSource(state.source);state.examples=dictionary(state.source)?.examples==='expanded';renderEntry();saveSession();return;}
  if(b.dataset.setting){const key=b.dataset.setting,old=general()[key];general()[key]=!old;try{await saveSettings();}catch(e){general()[key]=old;renderSettings();throw e;}renderSettings();return;}
  if(b.dataset.enable){const d=dictionary(b.dataset.enable);d.enabled=!d.enabled;await saveSettings();state.entries=state.entries.filter(e=>dictionary(e.dictionary)?.enabled);if(state.word)await lookup(state.word,state.language,false);setView('libraries');return;}
  if(b.dataset.move!==undefined){const list=state.snapshot.settings.dictionaries,i=Number(b.dataset.move),j=i+Number(b.dataset.direction);[list[i],list[j]]=[list[j],list[i]];await saveSettings();renderLibraries();return;}
  if(b.dataset.speak){await api('speak',{text:current().word,language:b.dataset.speak});return;}
  switch(b.id){
    case 'clear-query':$('#query').value='';state.request++;state.matches=[];renderMatches();suggestions(false);$('#query').focus();break;
    case 'save-word':await api('toggle_saved',record(current()));await refresh();renderEntry();break;
    case 'copy-word':await navigator.clipboard.writeText(DictLiteEntries.entryText(sourceEntries()));toast('词条已复制。');break;
    case 'toggle-examples':state.examples=!state.examples;renderEntry();break;
    case 'clear-history':await api('clear_history');await refresh();renderCollection();break;
    case 'export-settings':if(await api('export_settings',state.snapshot.settings))toast('设置已导出。');break;
    case 'import-settings':if(await api('import_settings')){await refresh();state.entries=[];setView('settings');toast('设置已导入。');}break;
    case 'quit-app':await api('quit');break;
    case 'import-dictionary':if(!state.busy&&!state.converted){state.selectedFile=null;importResult('选择要导入的词典。');}updateImport();$('#dictionary-import-dialog').showModal();break;
    case 'choose-dictionary-file':{const path=await api('choose_dictionary',{format:$('#dictionary-import-format').value});if(path){if(state.converted)await api('cleanup_conversion',{task_id:state.converted.task_id});state.converted=null;state.selectedFile=path;updateImport();importResult('文件已选择。');}break;}
    case 'run-dictionary-import':await runImport();break;
    case 'stop-conversion':await api('cancel_conversion');importResult('正在停止转换…');break;
    case 'close-dictionary-import':case 'cancel-dictionary-import':if(state.busy){toast('转换仍在进行，可点击“停止转换”取消。');return;}if(state.converted){await api('cleanup_conversion',{task_id:state.converted.task_id});state.converted=null;}$('#dictionary-import-dialog').close();break;
  }
}
document.addEventListener('click',e=>click(e).catch(error=>{toast(error);if($('#dictionary-import-dialog').open)importResult(error,'error');document.querySelectorAll('[data-save-status]').forEach(n=>n.textContent='保存失败');}));
document.addEventListener('change',async e=>{try{const t=e.target;if(t.dataset.name){const value=t.value.trim();if(!value){t.setCustomValidity('名称不能为空');t.reportValidity();return;}dictionary(t.dataset.name).name=value;await saveSettings();renderLibraries();}else if(t.dataset.examples){dictionary(t.dataset.examples).examples=t.value;await saveSettings();}else if(t.id==='release-minutes'){if(!t.checkValidity()||!Number.isSafeInteger(t.valueAsNumber)){t.reportValidity();return;}general().releaseMinutes=t.valueAsNumber;await saveSettings();}else if(t.id==='dictionary-import-format'){if(state.converted)await api('cleanup_conversion',{task_id:state.converted.task_id});state.converted=null;state.selectedFile=null;updateImport();importResult('请选择词典文件。');}}catch(error){toast(error);}});
document.addEventListener('input',e=>e.target.setCustomValidity?.(''));
$('#search-form').addEventListener('submit',e=>{e.preventDefault();submit().catch(toast);});
$('#query').addEventListener('input',()=>{if(!state.composing)find().catch(toast);});
$('#query').addEventListener('focus',()=>{if($('#query').value)find().catch(toast);});
$('#query').addEventListener('compositionstart',()=>{state.composing=true;suggestions(false);});
$('#query').addEventListener('compositionend',()=>{state.composing=false;find().catch(toast);});
$('#query').addEventListener('keydown',e=>{if(e.isComposing||state.composing)return;if(e.key==='Escape'){e.preventDefault();suggestions(false);}if(general().suggestions&&['ArrowUp','ArrowDown'].includes(e.key)&&state.matches.length){e.preventDefault();state.cursor=(state.cursor+(e.key==='ArrowDown'?1:-1)+state.matches.length)%state.matches.length;renderMatches();suggestions(true);$('#candidates .selected')?.scrollIntoView({block:'nearest'});}});
$('#candidates').addEventListener('pointerdown',e=>e.preventDefault());
$('#language-mode').addEventListener('change',()=>find().catch(toast));
document.addEventListener('focusin',e=>{if(!e.target.closest('.search-area')&&state.snapshot)suggestions(false);});
document.addEventListener('keydown',e=>{if($('#dictionary-import-dialog').open)return;if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='k'){e.preventDefault();setView('search');$('#query').focus();$('#query').select();}if(e.key==='Enter'&&e.target.matches('.dictionary-name')){e.preventDefault();e.target.blur();}});
$('#dictionary-import-dialog').addEventListener('cancel',e=>{if(state.busy)e.preventDefault();});
async function initialize(){
  $('.app-shell').inert=true;await refresh();renderMatches();await window.__TAURI__.event.listen('conversion',conversionEvent);await window.__TAURI__.event.listen('shortcut-error',e=>toast(e.payload));await window.__TAURI__.event.listen('open-entry',async({payload:s})=>{try{state.source=s.source;await lookup(s.word,s.language,false);}catch(e){toast(e);}});const session=await api('get_session');
  if(session?.word){state.source=session.source;await lookup(session.word,session.language,false);}
  setView(session?.view||'search');$('.app-shell').inert=false;
  const conversion=await api('conversion_status');if(conversion.active){state.busy=true;state.conversion={task_id:conversion.active};$('#dictionary-import-format').value='mdx';updateImport();importResult('词典正在转换，完成后可查看结果。');}else if(conversion.result?.status==='succeeded'){await conversionEvent({payload:conversion.result});}
}
initialize().catch(error=>{$('.app-shell').inert=false;$('#entry').innerHTML=empty('暂时无法加载',String(error));});




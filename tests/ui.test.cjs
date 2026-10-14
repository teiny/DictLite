const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const path=require('node:path');
const utils=require('../ui/entry-utils.js');
function environment(file){
  const nodes=new Map();const node=selector=>{if(!nodes.has(selector))nodes.set(selector,{innerHTML:'',hidden:false,scrollTop:0,attributes:{},addEventListener(){},setAttribute(k,v){this.attributes[k]=v;},removeAttribute(k){delete this.attributes[k];}});return nodes.get(selector);};
  const context=vm.createContext({DictLiteEntries:utils,document:{querySelector:node,querySelectorAll:()=>[],addEventListener(){}},window:{__TAURI__:{core:{invoke:async()=>[]}}},setTimeout,clearTimeout,structuredClone,console});
  let code=fs.readFileSync(path.join(__dirname,'../ui',file),'utf8');
  code=code.replace(/^initialize\(\)\.catch.*$/m,'').replace(/^\(async\(\)=>\{await window\.__TAURI__.*$/m,'');
  vm.runInContext(code,context);return {context,node};
}
const details=[
  {id:1,dictionary:'zh',word:'喂',language:'zh',body:'喂\nwéi\nhello (when answering the phone)',pronunciations:[],senses:[],examples:[],related:[]},
  {id:2,dictionary:'zh',word:'喂',language:'zh',body:'喂\nwèi\nto feed',pronunciations:[],senses:[],examples:[],related:[]},
  {id:3,dictionary:'zh',word:'喂',language:'zh',body:'喂\nwèi\nto call out',pronunciations:[],senses:[],examples:[],related:[]},
  {id:4,dictionary:'zh',word:'喂',language:'zh',body:'喂\nwéi\nhey',pronunciations:[],senses:[],examples:[],related:[]},
];
test('dictionary grouping preserves order and distinct homographs',()=>{
  const groups=utils.groupSources([...details,details[0],{...details[0],dictionary:'other'}]);
  assert.deepEqual(groups.map(g=>g.dictionary),['zh','other']);assert.equal(groups[0].entries.length,4);
  const copied=utils.entryText(groups[0].entries);for(const text of ['hello (when answering the phone)','to feed','to call out','hey'])assert.ok(copied.includes(text));
});
test('main view loads every homograph and renders a single dictionary tab',async()=>{
  const {context,node}=environment('app.js');const calls=[];
  context.window.__TAURI__.core.invoke=async(_,command)=>{calls.push(command.payload);return details.find(e=>e.id===command.payload.entry_id);};
  context.fixture=details;vm.runInContext(`state.snapshot={settings:{general:{},dictionaries:[{id:'zh',name:'汉语字典'}]},libraries:[{enabled:true,available:true}],saved:[]};state.entries=fixture.map(e=>({dictionary:e.dictionary,entry_id:e.id,word:e.word,language:e.language}));state.source='zh';`,context);
  await vm.runInContext(`loadSource('zh',true)`,context);vm.runInContext('renderEntry()',context);
  const html=node('#entry').innerHTML;assert.equal((html.match(/data-source="zh"/g)||[]).length,1);assert.equal((html.match(/class="entry-variant"/g)||[]).length,4);
  assert.equal(calls.length,4);assert.equal(calls.filter(c=>c.record).length,1);
  for(const text of ['hello (when answering the phone)','to feed','to call out','hey'])assert.ok(html.includes(text));
});
test('quick popup uses one tab per source and keeps all definitions',()=>{
  const {context,node}=environment('popup.js');context.fixture=details;
  vm.runInContext(`state.snapshot={settings:{dictionaries:[{id:'zh',name:'汉语字典'}]}};state.groups=DictLiteEntries.groupSources(fixture);state.source='zh';state.word='喂';render();`,context);
  const html=node('#content').innerHTML;assert.equal((html.match(/data-source="zh"/g)||[]).length,1);assert.equal((html.match(/class="variant"/g)||[]).length,4);
  assert.ok(html.includes('to feed'));assert.ok(html.includes('hey'));assert.equal(node('#open-main').hidden,false);
});
test('settings put startup first and persist the quick lookup switch both ways',async()=>{
  const {context,node}=environment('app.js');
  let snapshot={settings:{general:{suggestions:true,history:true,launchAtStartup:false,closeToTray:true,releaseMinutes:5,quickLookup:true},dictionaries:[]},libraries:[],saved:[],history:[],data_directory:'test'};
  const saved=[];context.fixture=snapshot;
  context.window.__TAURI__.core.invoke=async(_,command)=>{if(command.action==='save_settings'){snapshot.settings=structuredClone(command.payload);saved.push(snapshot.settings.general.quickLookup);return true;}if(command.action==='snapshot')return snapshot;};
  vm.runInContext('state.snapshot=fixture;renderSettings()',context);
  let html=node('#settings-view').innerHTML;assert.ok(html.indexOf('启动与后台')<html.indexOf('>查词</h2>'));assert.ok(html.includes('data-setting="quickLookup"'));
  context.event={target:{closest:selector=>selector==='button,a'?{dataset:{setting:'quickLookup'},matches:()=>false}:null}};
  await vm.runInContext('click(event)',context);assert.deepEqual(saved,[false]);html=node('#settings-view').innerHTML;assert.match(html,/data-setting="quickLookup"[^>]*aria-checked="false"/);
  await vm.runInContext('click(event)',context);assert.deepEqual(saved,[false,true]);assert.match(node('#settings-view').innerHTML,/data-setting="quickLookup"[^>]*aria-checked="true"/);
});
test('quick lookup focuses after definitions load and toggles pin both ways',async()=>{
  const {context,node}=environment('popup.js');const calls=[];
  context.window.__TAURI__.core.invoke=async(_,command)=>{
    calls.push(command);
    switch(command.action){
      case 'snapshot':return {settings:{dictionaries:[{id:'zh',name:'汉语字典'}]},libraries:[{enabled:true,available:true}]};
      case 'search':return [{word:'喂',language:'zh'}];
      case 'lookup':return details.map(e=>({dictionary:e.dictionary,entry_id:e.id}));
      case 'detail':return details.find(e=>e.id===command.payload.entry_id);
      case 'set_popup_pinned':return command.payload.pinned;
      case 'popup_ready':assert.ok(node('#content').innerHTML.includes('to feed'));assert.ok(node('#content').innerHTML.includes('hey'));return true;
    }
  };
  await vm.runInContext(`lookup({query:'喂',revision:1,pinned:false})`,context);
  assert.equal(calls.at(-1).action,'popup_ready');assert.equal(calls.at(-1).payload.revision,1);
  await vm.runInContext('togglePin()',context);assert.equal(node('#pin').attributes['aria-pressed'],'true');assert.equal(node('#pin').title,'取消置顶');
  await vm.runInContext('togglePin()',context);assert.equal(node('#pin').attributes['aria-pressed'],'false');assert.deepEqual(calls.filter(c=>c.action==='set_popup_pinned').map(c=>c.payload.pinned),[true,false]);
});

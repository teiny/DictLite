(function(root) {
  function groupSources(entries) {
    const groups = new Map();
    for (const entry of entries) {
      if (!groups.has(entry.dictionary)) groups.set(entry.dictionary, {dictionary:entry.dictionary,entries:[]});
      const group = groups.get(entry.dictionary);
      if (!group.entries.some(item=>(item.entry_id??item.id)===(entry.entry_id??entry.id))) group.entries.push(entry);
    }
    return [...groups.values()];
  }
  function entryText(entries) {
    return entries.map(e=>[e.word,...e.pronunciations.map(p=>p.text),...(e.senses.length?e.senses.map(s=>`${s.pos} ${s.translation}${s.note?'\n'+s.note:''}`):[e.body])].join('\n')).join('\n\n');
  }
  const api = {groupSources,entryText};
  if (typeof module !== 'undefined' && module.exports) module.exports=api;
  else root.DictLiteEntries=api;
})(typeof globalThis !== 'undefined'?globalThis:this);

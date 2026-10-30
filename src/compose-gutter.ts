import {tr,msg,monaco,own,on} from './sdk';

export type ComposeTarget={line:number;service?:string};
export type ComposeRun=(file:string,action:'up'|'stop',service?:string)=>void;

/** Same naming convention the native side accepts: compose.yaml, docker-compose.override.yml… */
export const isComposeFile=(path:string)=>/^(?:docker-)?compose(?:[.-][^/]*)?\.ya?ml$/i.test(path.split('/').pop()??'');

/** Top-level `services:` and the keys directly below it (first indentation level). */
export function composeTargets(text:string):ComposeTarget[]{
 const out:ComposeTarget[]=[];let inServices=false;let indent=-1;
 text.split(/\r?\n/).forEach((raw,index)=>{
  if(!raw.trim()||/^\s*#/.test(raw))return;
  const lead=raw.length-raw.trimStart().length;
  if(lead===0){inServices=/^(["']?)services\1\s*:\s*(?:#.*)?$/.test(raw);indent=-1;if(inServices)out.push({line:index+1});return;}
  if(!inServices||raw.slice(0,lead).includes('\t'))return;
  if(indent<0)indent=lead;
  if(lead!==indent)return;
  const key=raw.slice(lead).match(/^(["']?)([A-Za-z0-9][A-Za-z0-9._-]*)\1\s*:(?:\s|$)/);
  if(key)out.push({line:index+1,service:key[2]});
 });
 return out;
}

/** Run icons in the editor gutter for Compose files, as in the reference IDE. */
export function mountComposeGutter(editor:any,activePath:()=>string|undefined,run:ComposeRun){
 // sdk.own only tracks functions; editor listeners are IDisposable and must not outlive the plugin generation.
 const owned=(d:{dispose():void})=>own(()=>d.dispose());
 const decorations=editor.createDecorationsCollection();let targets:ComposeTarget[]=[];let file:string|undefined;let timer:any;let menu:HTMLElement|undefined;
 const close=()=>{menu?.remove();menu=undefined;};
 function paint(){
  // The host sets the project-relative active path before switching models.
  const model=editor.getModel();const path=model?activePath():undefined;
  file=path&&isComposeFile(path)?path:undefined;targets=file?composeTargets(model.getValue()):[];
  decorations.set(targets.map(t=>({range:new monaco.Range(t.line,1,t.line,1),options:{glyphMarginClassName:t.service?'compose-run':'compose-run compose-run-all',glyphMarginHoverMessage:{value:t.service?msg`Executar o serviço ${t.service}`:tr('Executar todos os serviços do Compose')}}})));
 }
 const schedule=()=>{clearTimeout(timer);timer=setTimeout(paint,150);};
 owned(editor.onDidChangeModel(()=>{close();paint();}));owned(editor.onDidChangeModelContent(schedule));
 own(()=>{clearTimeout(timer);close();decorations.clear();});
 function open(target:ComposeTarget,x:number,y:number){
  close();const composeFile=file!;menu=document.createElement('div');menu.className='tool-context-menu compose-run-menu';menu.setAttribute('role','menu');
  const items:[string,'up'|'stop'][]=target.service?[[msg`Executar '${target.service}'`,'up'],[msg`Parar '${target.service}'`,'stop']]:[[tr('Executar todos os serviços'),'up'],[tr('Parar todos os serviços'),'stop']];
  for(const [label,action] of items){const item=document.createElement('button');item.type='button';item.setAttribute('role','menuitem');item.dataset.composeRun=action;item.textContent=label;item.onclick=()=>{close();editor.focus();run(composeFile,action,target.service);};menu.append(item);}
  document.body.append(menu);menu.style.left=Math.min(x,window.innerWidth-menu.offsetWidth-8)+'px';menu.style.top=Math.min(y,window.innerHeight-menu.offsetHeight-8)+'px';menu.querySelector('button')?.focus();
  on(menu,'keydown',(e:KeyboardEvent)=>{if(e.key!=='ArrowDown'&&e.key!=='ArrowUp')return;e.preventDefault();const buttons=[...menu!.querySelectorAll('button')];const i=buttons.indexOf(document.activeElement as HTMLButtonElement);buttons[(i+(e.key==='ArrowDown'?1:buttons.length-1))%buttons.length]?.focus();});
 }
 owned(editor.onMouseDown((e:any)=>{
  if(e.target.type!==monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN||!e.target.position||!file)return;
  const target=targets.find(t=>t.line===e.target.position.lineNumber);if(!target)return;
  e.event.preventDefault();open(target,e.event.browserEvent.clientX,e.event.browserEvent.clientY);
 }));
 on(document,'pointerdown',(e:Event)=>{if(menu&&!menu.contains(e.target as Node))close();});on(document,'keydown',(e:KeyboardEvent)=>{if(e.key==='Escape'&&menu){close();editor.focus();}});
 paint();
 return {refresh:paint,targets:()=>targets,file:()=>file};
}

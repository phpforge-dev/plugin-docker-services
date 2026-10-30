import {test} from 'node:test';import assert from 'node:assert/strict';import {Window} from 'happy-dom';
import {activate} from '../workbench.js';
const compose=`version: "3"
services:

  cache:
    image: redis:alpine
    networks:
      shosp:
        ipv4_address: 172.28.5.10
    ports:
      - '6379:6379'
  # comment: ignored
  cache.read1:
    image: redis:alpine
  "web":
    build: .
networks:
  shosp: {}
`;
function setup(path,api){
 const w=new Window();for(const key of ['window','document','MutationObserver','ResizeObserver','HTMLElement','HTMLDialogElement','localStorage','navigator'])Object.defineProperty(globalThis,key,{value:w[key],configurable:true,writable:true});
 globalThis.Option=function(text,value){const el=document.createElement('option');el.text=text;el.value=value;return el;};
 document.body.innerHTML='<section class="code"><div id="editor"></div></section><div class="left-rail"></div>';
 const listeners={mouse:[],model:[],content:[]};let decorations=[];const statuses=[];let focused=0;
 const model={uri:{fsPath:'/p/'+path},getValue:()=>compose};
 const editor={getModel:()=>model,focus:()=>focused++,createDecorationsCollection:()=>({set:d=>decorations=d,clear:()=>decorations=[]}),onMouseDown:fn=>(listeners.mouse.push(fn),{dispose(){}}),onDidChangeModel:fn=>(listeners.model.push(fn),{dispose(){}}),onDidChangeModelContent:fn=>(listeners.content.push(fn),{dispose(){}})};
 const cleanups=[];let contribution;const format=(parts,...args)=>typeof parts==='string'?parts:parts.reduce((v,s,i)=>v+s+(args[i]??''),'');
 const monaco={Range:class{constructor(a,b,c,d){Object.assign(this,{startLineNumber:a,startColumn:b,endLineNumber:c,endColumn:d});}},editor:{MouseTargetType:{GUTTER_GLYPH_MARGIN:2}}};
 const sdk={own:fn=>cleanups.push(typeof fn==='function'?fn:()=>fn.dispose()),provide:(name,value)=>contribution=value,tr:s=>s,msg:format,html:format,activeLanguage:()=>'pt-BR',dockWindow:()=>()=>{},resizeToolWindow:()=>{},toolIcon:()=>{},editorFont:{},fileIcon:()=>'',keymap:{},monaco,host:{editor,path:()=>path,status:s=>statuses.push(s)}};
 activate(sdk);sdk.api=api;contribution.mount(api,{docker:async()=>{}});
 const click=(line)=>{for(const fn of listeners.mouse)fn({target:{type:2,position:{lineNumber:line}},event:{preventDefault(){},browserEvent:{clientX:20,clientY:40}}});};
 return {w,decorations:()=>decorations,click,statuses,dispose:()=>{for(const fn of cleanups.reverse())fn();w.happyDOM.abort();}};
}
const fakeApi=(calls,fail)=>({onDockerProgress:()=>()=>{},dockerContexts:async()=>fail==='cli'?Promise.reject(Error('Docker CLI indisponível: not found')):[{name:'fixture-local',current:true,endpoint:'unix:///x'}],dockerSnapshot:async()=>fail==='daemon'?Promise.reject(Error('Docker: Cannot connect to the Docker daemon')):{version:'1',os:'linux',containers:[],images:[],volumes:[],networks:[]},dockerComposeFiles:async()=>['compose.yaml'],dockerCompose:async(...args)=>{calls.push(args);if(fail==='daemon')throw Error('Docker: Cannot connect to the Docker daemon');return 'Started';}});
const until=async(fn)=>{for(let i=0;i<100&&!fn();i++)await new Promise(r=>setTimeout(r,5));assert.ok(fn());};
test('compose files get ▶▶ on services: and ▶ on each service key',()=>{
 const t=setup('docker/compose.yaml',fakeApi([]));
 assert.deepEqual(t.decorations().map(d=>[d.range.startLineNumber,d.options.glyphMarginClassName]),[[2,'compose-run compose-run-all'],[4,'compose-run'],[12,'compose-run'],[14,'compose-run']]);
 t.dispose();
 const other=setup('config/app.yaml',fakeApi([]));assert.equal(other.decorations().length,0);other.dispose();
});
test('gutter menu runs up -d for all services or one service and reports in Services',async()=>{
 const calls=[];const t=setup('docker/compose.yaml',fakeApi(calls));
 t.click(12);const items=[...document.querySelectorAll('.compose-run-menu [role=menuitem]')];assert.deepEqual(items.map(i=>i.textContent),["Executar 'cache.read1'","Parar 'cache.read1'"]);
 items[0].click();await until(()=>calls.length===1);
 assert.deepEqual([calls[0][0],calls[0][1],calls[0][2],calls[0][4]],['fixture-local','docker/compose.yaml','up','cache.read1']);
 await until(()=>t.statuses.some(s=>s.includes('concluído')));assert.equal(document.getElementById('docker-panel').hidden,false);assert.equal(document.getElementById('docker-compose-file').value,'docker/compose.yaml');
 t.click(2);document.querySelector('[data-compose-run=stop]').click();await until(()=>calls.length===2);assert.deepEqual([calls[1][2],calls[1][4]],['stop',undefined]);
 t.click(3);assert.equal(document.querySelector('.compose-run-menu'),null);
 t.dispose();assert.equal(document.querySelector('.compose-run-menu,#docker-panel'),null);
});
test('missing CLI and stopped daemon are reported without leaving the panel busy',async()=>{
 const calls=[];const t=setup('compose.yaml',fakeApi(calls,'cli'));t.click(2);document.querySelector('[data-compose-run=up]').click();
 await until(()=>t.statuses.some(s=>s.includes('Docker CLI indisponível')));assert.equal(calls.length,0);t.dispose();
 const d=setup('compose.yaml',fakeApi(calls,'daemon'));d.click(4);document.querySelector('[data-compose-run=up]').click();
 await until(()=>d.statuses.some(s=>s.includes('daemon')));assert.match(document.getElementById('docker-message').textContent,/daemon/);assert.equal(document.getElementById('docker-cancel').hidden,true);d.dispose();
});

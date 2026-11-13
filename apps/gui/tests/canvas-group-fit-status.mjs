import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {illustrationFitStatus} from '../src/features/canvas/canvas-selection-context.ts';
assert.ok(process.env.VISUALIZATION_GROUP_FIT_FIXTURE,'core-generated group fit required');const input=JSON.parse(await readFile(process.env.VISUALIZATION_GROUP_FIT_FIXTURE,'utf8')),doc=input.document;
assert.equal(illustrationFitStatus(doc.elements,'anchor').status,'current_snapshot');
for(const [change,status]of [[d=>d.elements.find(e=>e.id==='part').fileId='changed','stale_file'],[d=>d.elements.find(e=>e.id==='part').x+=1,'stale_geometry'],[d=>d.elements.find(e=>e.id==='anchor').width+=1,'stale_geometry'],[d=>d.elements.find(e=>e.id==='anchor').customData.visualBadgeIds=[],'invalid_record'],[d=>d.elements.push({...structuredClone(d.elements.find(e=>e.id==='part')),id:'unlisted'}),'invalid_record']]){const d=structuredClone(doc);change(d);assert.equal(illustrationFitStatus(d.elements,'anchor').status,status);}
console.log('group fit status: all members and union geometry tracked; changed file/geometry/membership rejected');

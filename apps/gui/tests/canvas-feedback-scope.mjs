import assert from 'node:assert/strict';
import {candidateFeedbackScope} from '../src/features/canvas/canvas-feedback-scope.ts';
const encode=(focus,comparison)=>JSON.stringify(candidateFeedbackScope(focus,comparison));
const formal={kind:'formal_revision',revision:2},parent={kind:'parent_candidate',proposal_id:'p',document_sha256:'hash'};
assert.equal(encode('edge',parent),encode('edge',{document_sha256:'hash',proposal_id:'p',kind:'parent_candidate'}));
const keys=[encode(undefined,formal),encode('edge',formal),encode('other',formal),encode('edge',{...formal,revision:3}),encode('edge',parent),encode('edge',{...parent,proposal_id:'other'}),encode('edge',{...parent,document_sha256:'different'}),encode('edge:parent_candidate:p',formal)];assert.equal(new Set(keys).size,keys.length);
console.log('feedback draft scope: object, formal revision, parent ID/hash and structured separator isolation passed');

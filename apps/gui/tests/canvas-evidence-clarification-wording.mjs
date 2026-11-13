import assert from 'node:assert/strict';
import {checkEvidenceClarification} from './helpers/canvas-live-evidence.mjs';
const calls=[{name:'figure_evidence',input:{action:'search',query:'interferon'},is_error:false}];
const plan=question=>({status:'needs_clarification',document:{questions:[{id:'organism',question:'使用哪个物种？',answer:'',blocking:true},{id:'branch',question,answer:'',blocking:true}]}});
for(const question of ['刺激是 I 型干扰素（IFN-α/β）还是 II 型干扰素（IFN-γ）？','Should the stimulus be type I or type II interferon?','选择Ⅰ型干扰素还是Ⅱ型干扰素？'])checkEvidenceClarification(plan(question),calls);
for(const question of ['IFN-γ 刺激后是否加入药物？','是否比较 I 型细胞和 II 型细胞？','是否需要干扰素的配色？'])assert.throws(()=>checkEvidenceClarification(plan(question),calls));
const answered=plan('使用哪种干扰素亚型？');answered.document.questions[1].answer='IFN-γ';assert.throws(()=>checkEvidenceClarification(answered,calls));
console.log('clarification wording: explicit type-I/type-II choices accepted; assumed subtype, unrelated cell types and nonempty initial answers still reject');

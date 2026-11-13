// Bounded composition checks for the explicitly requested compact evaluation.
// These do not infer scientific grouping or certify publication quality.
import assert from 'node:assert/strict';
export function checkCompactComposition(plan,document){
 assert.ok(plan.panels?.length>=2,'compact evaluation requests explicit visual sections');
 const panels=new Set(plan.panels.map(panel=>panel.id));assert.ok(plan.nodes.every(node=>panels.has(node.panel)));
 const elements=document.elements.filter(e=>!e.isDeleted);const frames=elements.filter(e=>e.type==='frame');assert.equal(frames.length,plan.panels.length);
 const positions=elements.flatMap(e=>{assert.ok(['x','y','width','height'].every(key=>Number.isFinite(e[key])),'finite compact object geometry');assert.ok(!e.angle,'this composition check requires axis-aligned objects');return Array.isArray(e.points)?e.points.map(([x,y])=>[e.x+x,e.y+y]):[[e.x,e.y],[e.x+e.width,e.y+e.height]];});
 assert.ok(positions.length);const width=Math.max(...positions.map(p=>p[0]))-Math.min(...positions.map(p=>p[0])),height=Math.max(...positions.map(p=>p[1]))-Math.min(...positions.map(p=>p[1]));
 assert.ok(width>0&&height>0);const ratio=width/height;assert.ok(ratio>=.5&&ratio<=2.5,`compact evaluation bounds ratio ${ratio.toFixed(3)} must be within 0.5..2.5`);
 assert.ok(elements.filter(e=>e.type==='text').every(e=>e.fontSize>=16),'compactness must not be obtained with tiny text');
 return {width,height,aspect_ratio:ratio,panels:frames.length,scope:'explicit compact multi-panel case; bounding ratio and native text size only, scientific grouping and visual quality require review'};
}

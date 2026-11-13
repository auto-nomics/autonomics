import assert from 'node:assert/strict';
export function currentCandidateLeaf(proposals, revision) {
  const pending = proposals.filter(p => p.status === 'pending' && p.base_revision === revision);
  const leaves = pending.filter(p => !pending.some(child => child.parent_proposal_id === p.id));
  assert.equal(leaves.length, 1, 'one unambiguous candidate lineage at the current revision');
  const seen = new Set();
  let current = leaves[0];
  while (current) {
    assert.ok(!seen.has(current.id), 'candidate lineage cannot contain cycles');
    seen.add(current.id);
    if (!current.parent_proposal_id) break;
    current = pending.find(p => p.id === current.parent_proposal_id);
    assert.ok(current, 'current candidate ancestry must remain available at the same revision');
  }
  assert.equal(seen.size, pending.length, 'all current candidates belong to the same lineage');
  return leaves[0];
}

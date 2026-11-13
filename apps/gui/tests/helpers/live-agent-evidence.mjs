// Uses Node HTTP rather than the browser context, which may already be closed.
async function evidenceReader({ origin, token }) {
  const login = await fetch(`${origin}/api/session`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ token }), signal: AbortSignal.timeout(10000) });
  if (!login.ok) throw new Error(`evidence login HTTP ${login.status}`);
  const cookie = login.headers.get('set-cookie')?.split(';')[0];
  if (!cookie) throw new Error('evidence login missing session cookie');
  const get = async path => {
    const response = await fetch(`${origin}${path}`, { headers: { cookie }, signal: AbortSignal.timeout(10000) });
    if (!response.ok) throw new Error(`evidence HTTP ${response.status}`);
    return response.json();
  };
  return get;
}
export async function retainAgentEvidence({ origin, token, agent, persist }) {
  const get = await evidenceReader({ origin, token });
  const state = await get('/api/v1/state');
  const actor = state.agents.find(item => item.path === agent);
  const session = state.contexts?.[agent]?.id;
  await persist('failure-agent-state.json', { checked_at: new Date().toISOString(), agent, status: actor?.status, session_present: !!session });
  if (actor && session) {
    const history = await get(`/api/v1/agents/${encodeURIComponent(actor.agent_id)}/sessions/${encodeURIComponent(session)}/history`);
    await persist('failure-session-history.json', JSON.parse(JSON.stringify(history, (key, value) => key === 'data' && typeof value === 'string' ? '[IMAGE BYTES OMITTED]' : value)));
  }
  return actor?.status;
}


/** Retain the real scoped scene and candidates before an isolated host exits. */
export async function retainFigureEvidence({ origin, token, figurePath, persist }) {
  if (!/^\/api\/v1\/visualization\/projects\/[^/?#]+\/figures\/[^/?#]+$/.test(figurePath)) throw new Error('evidence requires an exact project figure path');
  const get = await evidenceReader({ origin, token });
  const report = { checked_at: new Date().toISOString(), figure_path: figurePath, saved: [], errors: [] };
  for (const [resource, suffix, name] of [['figure', '', 'failure-figure.json'], ['planning', '/planning', 'failure-planning.json'], ['candidates', '/proposals', 'failure-candidates.json']]) {
    try { await persist(name, await get(figurePath + suffix)); report.saved.push(resource); }
    catch (error) { report.errors.push({ resource, reason: String(error) }); }
  }
  await persist('failure-figure-evidence.json', report);
  return report;
}

import assert from 'node:assert/strict';
import { observeLiveAgent } from './helpers/live-agent-observer.mjs';
let clock = 0, polls = 0;
const windows = [], retries = [];
await observeLiveAgent({
  poll: async () => { polls++; if (polls === 2) throw new Error('fetch failed'); return polls === 8; },
  isAlive: () => true, windowMs: 20, intervalMs: 10,
  now: () => clock, sleep: async ms => { clock += ms; },
  onWindow: event => windows.push(event), onRetry: event => retries.push(event),
});
assert.equal(polls, 8);
assert.equal(windows.length, 3);
assert.equal(retries.length, 1);
assert.equal(windows.at(-1).elapsed_ms, 60);
await assert.rejects(observeLiveAgent({ poll: async () => false, isAlive: () => false, windowMs: 20 }), /host stopped/);
await assert.rejects(observeLiveAgent({ poll: async () => { throw new Error('agent ended with ERROR'); }, isAlive: () => true, windowMs: 20 }), /ended with ERROR/);
await assert.rejects(observeLiveAgent({ poll: async () => { assert.fail('candidate invariant'); }, isAlive: () => true, windowMs: 20 }), /candidate invariant/);
await assert.rejects(observeLiveAgent({ poll: async () => { assert.fail('fetch failed is an invariant message, not a network error'); }, isAlive: () => true, windowMs: 20 }), /invariant message/);
console.log('live-agent-observer: passed (window renewal, transient retry, host exit, terminal and invariant failure)');

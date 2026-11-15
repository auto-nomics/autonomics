/** Progress-window UI rendered inside the extension iframe injected by content.js. */
const nonce = location.hash.slice(1);
const el = id => document.getElementById(id);

function post(kind, payload = {}) { parent.postMessage({ nonce, kind, ...payload }, '*'); }

const rows = new Map();
function row(index, title) {
  if (rows.has(index)) return rows.get(index);
  const li = document.createElement('li');
  const name = document.createElement('span'); name.className = 'title'; name.textContent = title;
  const state = document.createElement('span'); state.className = 'state'; state.textContent = '…';
  li.append(name, state);
  el('items').append(li);
  rows.set(index, li);
  return li;
}

const events = {
  show({ headline }) {
    el('headline').textContent = headline || 'Saving to Autonomics';
    el('saving').classList.remove('hidden'); el('selector').classList.add('hidden');
  },
  itemProgress({ index, title, progress, done, warning }) {
    const li = row(index, title || `Item ${Number(index) + 1}`);
    li.querySelector('.state').textContent = progress || (done ? 'Saved' : '…');
    li.classList.toggle('done', Boolean(done) && !warning);
    li.classList.toggle('warning', Boolean(warning));
    li.title = warning || '';
  },
  select({ entries }) {
    el('saving').classList.add('hidden'); el('selector').classList.remove('hidden');
    el('entries').replaceChildren();
    for (const entry of entries || []) {
      const label = document.createElement('label');
      const box = document.createElement('input'); box.type = 'checkbox'; box.checked = true; box.value = String(entry.index);
      const text = document.createElement('span'); text.textContent = entry.title;
      label.append(box, text); el('entries').append(label);
    }
  },
  done() { setTimeout(() => post('close'), 600); },
  error({ message }) {
    el('saving').classList.remove('hidden'); el('selector').classList.add('hidden');
    el('error').textContent = message || 'Saving failed.';
    el('error').classList.remove('hidden');
  },
};

el('ok').onclick = () => {
  const indices = [...el('entries').querySelectorAll('input:checked')].map(input => Number(input.value));
  post('selection', { indices });
};
el('cancel').onclick = () => post('selection', { indices: [] });

window.addEventListener('message', event => {
  if (event.data?.nonce !== nonce) return;
  events[event.data.event]?.(event.data);
});

post('ready');

// Çıkış akışı davranış testi: frontend'in son <script> bloğunu izole çalıştırır.
// Doğrular: (1) Çıkış logout komutunu çağırır, (2) durum offline'a düşer,
// (3) "Bağlan" yeniden aktif olur, (4) QR penceresi yeniden QR bekler.
const fs = require('fs');
const assert = require('assert');

const html = fs.readFileSync(__dirname + '/index.html', 'utf8');

function el(id) {
  return {
    id,
    classList: {
      _s: new Set(['hidden']),
      add(c) { this._s.add(c); },
      remove(c) { this._s.delete(c); },
      replace(a, b) { this._s.delete(a); this._s.add(b); },
      toggle(c, on) { on ? this._s.add(c) : this._s.delete(c); },
      contains(c) { return this._s.has(c); },
    },
    textContent: '',
    innerHTML: '',
    value: '',
    disabled: false,
    checked: false,
    onclick: null,
    addEventListener() {},
  };
}
const nodes = new Proxy({}, { get: (t, k) => (t[k] ||= el(String(k))) });
const document = {
  getElementById(id) { return nodes[id]; },
  querySelector(sel) { return nodes[sel.replace(/^#/, '')]; },
  querySelectorAll() { return []; },
  addEventListener() {},
};

const calls = [];
let status = 'connected';
const listeners = {};
function invoke(cmd) {
  calls.push(cmd);
  if (cmd === 'get_wa_status') return Promise.resolve(status);
  if (cmd === 'logout_whatsapp') { status = 'offline'; return Promise.resolve(null); }
  if (cmd === 'disconnect_whatsapp') { status = 'offline'; return Promise.resolve(null); }
  return Promise.resolve([]);
}
function listen(name, cb) { (listeners[name] ||= []).push(cb); }
function emit(name, payload) { (listeners[name] || []).forEach((f) => f({ payload })); }

// Sayfa kendi __TAURI__ köprüsünü kurmaya çalışır; hazır olanı görüp atlar.
const window = {
  __TAURI__: {
    tauri: { invoke },
    dialog: { open: async () => null },
    event: { listen },
  },
};

const g = {
  document, window, console, listeners, emit,
  alert: (m) => { throw new Error('beklenmeyen alert: ' + m); },
  confirm: () => true,
  QRCode: function () {},
  Promise, Object, JSON, Number, String, Array, Math, Date, RegExp, Error, isNaN, parseInt, parseFloat,
  setInterval: () => 0, // 5 sn'lik tazelemeyi elle tetikleriz
  setTimeout: () => 0,
  clearTimeout: () => {},
  localStorage: { getItem: () => null, setItem: () => {} },
  fetch: async () => ({ ok: true, json: async () => ({}) }),
};

const all = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
const script = all[all.length - 1];
new Function(...Object.keys(g), script)(...Object.values(g));

(async () => {
  await new Promise((r) => setImmediate(r));

  assert.equal(typeof nodes['waLogoutBtn'].onclick, 'function', 'Çıkış düğmesi bağlı olmalı');

  nodes['qrModal'].classList.add('flex');
  nodes['qrStatus'].textContent = 'QR kodu telefonunuzdan okutun';

  await nodes['waLogoutBtn'].onclick();

  assert.ok(calls.includes('logout_whatsapp'), 'logout_whatsapp çağrılmalı');
  assert.equal(nodes['waConnectBtn'].disabled, false, 'Bağlan düğmesi aktif olmalı');
  assert.equal(nodes['waConnectBtn'].textContent, 'WhatsApp Bağla');
  assert.ok(
    /QR kod bekleniyor/.test(nodes['qrStatus'].textContent),
    'QR penceresi yeni QR beklemeli; bulunan: ' + nodes['qrStatus'].textContent
  );
  assert.ok(
    nodes['waDisconnectBtn'].classList.contains('hidden'),
    'bağlantı yokken "Kes" gizli olmalı'
  );

  await nodes['waConnectBtn'].onclick();
  assert.ok(calls.includes('connect_whatsapp'), 'Bağlan, connect_whatsapp çağırmalı');

  console.log('OK: çıkış akışı doğru —', calls.join(', '));
})().catch((e) => {
  console.error('FAIL:', e.message);
  process.exit(1);
});

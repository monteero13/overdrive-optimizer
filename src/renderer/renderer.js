'use strict';
// Si por lo que sea el puente (preload) no está, usamos un stub inofensivo para
// que la interfaz nunca se quede en blanco por un error al cargar.
const api = window.overdrive || new Proxy({}, { get: () => () => Promise.resolve(null) });
const $ = (s) => document.querySelector(s);
const $$ = (s) => [...document.querySelectorAll(s)];

const state = {
  analysis: null, hasSSD: false, gpuVendors: [],
  level: 'media', tweaks: [], selected: new Set(), applied: [],
  profileTweaks: null, benchBefore: null, benchAfter: null, activeProfileId: null,
};

// ----------------------------- utilidades -----------------------------
function toast(msg, isErr = false) {
  const t = $('#toast');
  t.textContent = msg; t.classList.toggle('err', isErr); t.hidden = false;
  requestAnimationFrame(() => t.classList.add('show'));
  clearTimeout(toast._t);
  toast._t = setTimeout(() => { t.classList.remove('show'); setTimeout(() => (t.hidden = true), 300); }, 3400);
}
function show(sel) { const el = $(sel); if (el) el.hidden = false; }
function detectVendors(gpus) {
  const s = new Set();
  for (const g of gpus || []) {
    const n = (g.name || '').toLowerCase();
    if (n.includes('nvidia') || n.includes('geforce')) s.add('nvidia');
    if (n.includes('amd') || n.includes('radeon')) s.add('amd');
    if (n.includes('intel')) s.add('intel');
  }
  return [...s];
}

// ----------------------------- navegación por pestañas -----------------------------
$('#nav').addEventListener('click', (e) => {
  const btn = e.target.closest('.nav-item');
  if (!btn) return;
  const tab = btn.dataset.tab;
  $$('.nav-item').forEach((n) => n.classList.toggle('is-active', n === btn));
  $$('.tab').forEach((s) => s.classList.toggle('is-active', s.dataset.tab === tab));
  if (tab === 'perfiles') loadProfiles();
});

// ----------------------------- análisis -----------------------------
$('#btnScan').addEventListener('click', runScan);

async function runScan() {
  $('#btnScan').disabled = true;
  $('#scanHint').hidden = true;
  $('#specs').hidden = true;
  $('#driversBox').hidden = true;
  show('#scanning');
  try {
    const data = await api.analyze();
    state.analysis = data;
    state.hasSSD = (data.disks || []).some((d) => /ssd/i.test(d.media || ''));
    state.gpuVendors = detectVendors(data.gpu);
    renderSpecs(data);
    if (!data.admin) $('#adminBadge').hidden = false;
    show('#panel-level'); await loadTweaks(); show('#panel-plan');
    scanDrivers();
  } catch (err) {
    toast('No se pudo analizar: ' + err.message, true);
  } finally {
    $('#scanning').hidden = true;
    $('#btnScan').disabled = false;
    $('#btnScan').querySelector('.btn-label').textContent = 'Volver a escanear';
  }
}

async function scanDrivers() {
  const box = $('#driversBox'), cont = $('#driversList');
  box.hidden = false;
  cont.innerHTML = '<div class="muted">Buscando drivers… (unos segundos)</div>';
  try { renderDrivers((await api.analyzeDrivers()).driversOutdated || []); }
  catch (_) { cont.innerHTML = '<div class="muted">No se pudieron comprobar los drivers aquí.</div>'; }
}

function spec(k, v, x, meter) {
  return `<div class="spec"><div class="k">${k}</div><div class="v">${v}</div>${x ? `<div class="x">${x}</div>` : ''}${meter != null ? `<div class="meter"><i style="width:${meter}%"></i></div>` : ''}</div>`;
}
function renderSpecs(d) {
  const g = (d.gpu && d.gpu[0]) || {};
  const ramPct = d.ram.totalGB ? Math.round(((d.ram.totalGB - d.ram.freeGB) / d.ram.totalGB) * 100) : 0;
  const vol = (d.volumes || []).find((v) => v.drive === 'C:') || (d.volumes || [])[0] || {};
  const diskPct = vol.totalGB ? Math.round(((vol.totalGB - vol.freeGB) / vol.totalGB) * 100) : 0;
  const media = (d.disks || []).map((x) => x.media).filter(Boolean).join(', ') || '—';
  $('#specs').innerHTML = [
    spec('CPU', d.cpu.name || '—', `${d.cpu.cores} núcleos · ${d.cpu.threads} hilos · ${(d.cpu.clockMHz / 1000).toFixed(1)} GHz`, d.cpu.load),
    spec('GPU', g.name || '—', `Driver ${g.driverVersion || '—'} · ${g.driverDate || 's/f'}`),
    spec('RAM', `${d.ram.totalGB} GB`, `${(d.ram.totalGB - d.ram.freeGB).toFixed(1)} GB en uso · ${d.ram.freeGB} libres`, ramPct),
    spec('Disco C:', `${vol.freeGB ?? '—'} GB libres`, `${media} · ${vol.totalGB ?? '—'} GB`, diskPct),
    spec('Sistema', d.os.name || 'Windows', `Build ${d.os.build} · ${d.os.arch}`),
    spec('Energía', d.powerPlan || '—', `Arranque: ${d.os.lastBoot || '—'}`),
  ].join('');
  $('#specs').hidden = false;
  requestAnimationFrame(() => $$('#specs .meter > i').forEach((el) => { const w = el.style.width; el.style.width = '0'; requestAnimationFrame(() => (el.style.width = w)); }));
}

function renderDrivers(list) {
  const cont = $('#driversList');
  if (!list.length) { cont.innerHTML = '<div class="muted">Los drivers relevantes están al día. 👍</div>'; return; }
  cont.innerHTML = list.slice(0, 12).map((d) => {
    const years = (d.ageDays / 365).toFixed(1);
    const sev = d.severity === 'muy-desactualizado' ? '<span class="sev muy">MUY ANTIGUO</span>' : '<span class="sev des">Antiguo</span>';
    return `<div class="driver"><div style="min-width:0"><div class="dn">${d.device}</div><div class="dm">${d.class} · v${d.version} · ${d.date} (${years} a)</div></div>
      <div style="display:flex;align-items:center;gap:10px;flex-shrink:0">${sev}<button class="btn btn-ghost btn-sm btn-driver" data-url="${d.vendorUrl}">Actualizar</button></div></div>`;
  }).join('');
  cont.querySelectorAll('.btn-driver').forEach((b) => b.addEventListener('click', () => { api.openExternal(b.dataset.url); toast('Abriendo la descarga oficial del driver…'); }));
}

$('#levelSelector').addEventListener('click', (e) => {
  const btn = e.target.closest('.level'); if (!btn) return;
  $$('#levelSelector .level').forEach((l) => l.classList.toggle('is-active', l === btn));
  state.level = btn.dataset.level; loadTweaks();
});

async function loadTweaks() {
  try {
    const list = await api.listTweaks(state.level, { hasSSD: state.hasSSD, gpuVendors: state.gpuVendors });
    state.tweaks = list;
    state.selected = new Set(list.filter((t) => t.state !== 'on' && !t.isInstall && !t.informational).map((t) => t.id));
    renderPlan();
  } catch (err) { toast('No se pudo cargar el plan: ' + err.message, true); }
}

function tweakTags(t) {
  const on = t.isInstall ? 'instalado' : 'activo';
  return [
    `<span class="tag risk-${t.risk}">${t.risk === 'low' ? 'bajo' : t.risk === 'medium' ? 'medio' : 'alto'}</span>`,
    t.isInstall ? '<span class="tag install">instalación</span>' : '',
    t.informational ? '<span class="tag install">abre app</span>' : '',
    t.state === 'on' ? `<span class="tag state-on">${on}</span>` : '',
    t.needsAdmin ? '<span class="tag admin">admin</span>' : '',
    t.needsReboot ? '<span class="tag reboot">reinicio</span>' : '',
  ].filter(Boolean).join(' ');
}
function renderPlan() {
  const cont = $('#planList');
  cont.innerHTML = state.tweaks.map((t) => `
    <div class="tweak ${state.selected.has(t.id) ? 'sel' : ''}" data-id="${t.id}">
      <div class="chk"></div>
      <div style="flex:1;min-width:0"><div class="tt">${t.title} ${tweakTags(t)}</div><div class="td">${t.desc}</div></div>
    </div>`).join('');
  cont.querySelectorAll('.tweak').forEach((el) => el.addEventListener('click', () => {
    const id = el.dataset.id;
    state.selected.has(id) ? state.selected.delete(id) : state.selected.add(id);
    el.classList.toggle('sel'); updateApplyBtn();
  }));
  updateApplyBtn();
}
function updateApplyBtn() {
  const btn = $('#btnApply'), n = state.selected.size;
  btn.disabled = n === 0;
  btn.textContent = n ? `Aplicar ${n}` : 'Aplicar';
}

// ----------------------------- aplicar / revertir -----------------------------
// Motor común de aplicación (usado por modo Experto y modo Fácil).
// Devuelve los resultados, o null si hubo que relanzar como admin.
async function runApply(ids, useRestore) {
  const needsAdmin = state.tweaks.some((t) => ids.includes(t.id) && t.needsAdmin);
  if (needsAdmin && state.analysis && !state.analysis.admin) {
    toast('Algunas optimizaciones requieren administrador. Acepta el aviso de Windows (UAC)…');
    const r = await api.relaunchAdmin();
    if (!r || !r.ok) toast('No se pudo reabrir como administrador. Cierra y abre con clic derecho → "Ejecutar como administrador".', true);
    return null;
  }
  const overlay = $('#overlay'), bar = $('#progressBar');
  overlay.hidden = false; bar.style.width = '0';
  if (useRestore) {
    bar.classList.add('indet'); let secs = 0;
    const fmt = () => `Creando punto de restauración… ${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`;
    $('#overlayText').textContent = fmt();
    $('#overlayNote').textContent = 'Es normal que tarde hasta 1 min. Está trabajando, no colgada.';
    const timer = setInterval(() => { secs++; $('#overlayText').textContent = fmt(); }, 1000);
    const rp = await api.restorePoint();
    clearInterval(timer); bar.classList.remove('indet'); $('#overlayNote').textContent = '';
    if (!rp.ok) toast('Se aplicará sin punto de restauración (' + (rp.message || 'Restaurar Sistema desactivado') + ').', true);
  }
  const off = api.onProgress((p) => {
    if (p.done) { bar.classList.remove('indet'); bar.style.width = '100%'; return; }
    const long = !!p.isInstall;
    $('#overlayText').textContent = (long ? 'Instalando ' : 'Aplicando ') + `(${p.index + 1}/${p.total}) ` + p.title;
    $('#overlayNote').textContent = long ? 'La instalación puede tardar varios minutos.' : '';
    bar.classList.toggle('indet', long);
    if (!long) bar.style.width = `${Math.round((p.index / p.total) * 100)}%`;
  });
  let results = [];
  try { results = await api.apply(ids); } catch (err) { toast('Error al aplicar: ' + err.message, true); }
  off(); overlay.hidden = true;
  state.applied = results.filter((r) => r.ok).map((r) => r.id);
  await loadTweaks();
  return results;
}

$('#btnApply').addEventListener('click', applyPlan);
async function applyPlan() {
  const ids = [...state.selected];
  if (!ids.length) return;
  const results = await runApply(ids, $('#chkRestore').checked);
  if (!results) return;
  renderResults(results); show('#panel-result');
  const okCount = results.filter((r) => r.ok).length;
  const reboot = results.some((r) => r.ok && r.needsReboot);
  toast(`${okCount}/${results.length} aplicadas${reboot ? ' · requiere reinicio' : ''}`);
}
function renderResults(results) {
  const byId = Object.fromEntries(state.tweaks.map((t) => [t.id, t.title]));
  $('#resultList').innerHTML = results.map((r) => `
    <div class="res ${r.ok ? 'ok' : 'err'}"><span class="ic">${r.ok ? '✓' : '!'}</span>
    <span>${byId[r.id] || r.id}${r.ok ? '' : ' — ' + (r.message || 'fallo')}${r.needsReboot && r.ok ? ' (reinicia para aplicar)' : ''}</span></div>`).join('');
}
$('#btnRevert').addEventListener('click', async () => {
  if (!state.applied.length) { toast('No hay cambios reversibles.'); return; }
  const overlay = $('#overlay'); $('#overlayText').textContent = 'Revirtiendo…'; $('#progressBar').style.width = '50%'; overlay.hidden = false;
  try { const res = await api.revert(state.applied); $('#progressBar').style.width = '100%'; toast(`${res.filter((r) => r.ok).length} cambios revertidos.`); state.applied = []; await loadTweaks(); }
  catch (err) { toast('Error al revertir: ' + err.message, true); } finally { overlay.hidden = true; }
});

// ----------------------------- Monitor -----------------------------
const mon = { timer: null, running: false, hist: { gpuTemp: [], gpuUtil: [], cpuLoad: [], ram: [] } };
$('#btnMonitor').addEventListener('click', () => (mon.running ? stopMonitor() : startMonitor()));
function startMonitor() {
  mon.running = true; const b = $('#btnMonitor'); b.textContent = 'Detener'; b.classList.add('live');
  $('#monitorGrid').hidden = false; $('#monitorHint').hidden = true; tickMonitor(); mon.timer = setInterval(tickMonitor, 1000);
}
function stopMonitor() { mon.running = false; clearInterval(mon.timer); const b = $('#btnMonitor'); b.textContent = 'Iniciar'; b.classList.remove('live'); }
async function tickMonitor() { let m; try { m = await api.monitorSample(); } catch (_) { return; } if (mon.running) renderMonitor(m); }
function setVal(id, text, cls) { const el = $('#m-' + id); if (el) { el.textContent = text; el.className = 'mval' + (cls ? ' ' + cls : ''); } }
function setBar(id, pct, color) { const el = $('#b-' + id); if (el) { el.style.width = Math.max(0, Math.min(100, pct)) + '%'; if (color) el.style.background = color; } }
function pushHist(k, v) { const a = mon.hist[k]; a.push(v); if (a.length > 60) a.shift(); }
function tClass(c) { if (c == null) return ''; if (c >= 85) return 'danger'; if (c >= 75) return 'warn'; if (c < 60) return 'ok'; return ''; }
function tColor(c) { if (c == null) return null; if (c >= 85) return 'var(--danger)'; if (c >= 75) return 'var(--warn)'; return 'linear-gradient(90deg,var(--cyan),var(--amber))'; }
function renderMonitor(m) {
  const g = m.gpu;
  if (g) {
    setVal('gpuTemp', g.tempC != null ? g.tempC + '°' : 'N/D', tClass(g.tempC)); setBar('gpuTemp', g.tempC || 0, tColor(g.tempC)); pushHist('gpuTemp', g.tempC || 0);
    setVal('gpuUtil', g.util != null ? g.util + '%' : 'N/D'); setBar('gpuUtil', g.util || 0); pushHist('gpuUtil', g.util || 0);
    setVal('gpuPower', g.powerW != null ? g.powerW.toFixed(0) + ' W' : 'N/D'); setBar('gpuPower', g.powerLimitW ? (g.powerW / g.powerLimitW) * 100 : 0);
    $('#m-gpuExtra').textContent = `${g.clockMHz ?? '—'} MHz · fan ${g.fanPct != null ? g.fanPct + '%' : '—'} · lím ${g.powerLimitW ?? '—'} W`;
  } else { ['gpuTemp', 'gpuUtil', 'gpuPower'].forEach((k) => setVal(k, 'N/D')); $('#m-gpuExtra').textContent = 'GPU NVIDIA no detectada'; }
  const c = m.cpu;
  if (c) {
    setVal('cpuLoad', c.load != null ? c.load + '%' : 'N/D'); setBar('cpuLoad', c.load || 0); pushHist('cpuLoad', c.load || 0);
    setVal('cpuClock', c.clock != null ? (c.clock / 1000).toFixed(2) + ' GHz' : 'N/D');
    $('#m-cpuExtra').textContent = c.temp != null ? `ACPI ${c.temp}° (zona placa)` : 'temp ACPI no disponible';
  }
  const r = m.ram;
  if (r && r.totalGB) { const pct = Math.round((r.usedGB / r.totalGB) * 100); setVal('ram', `${r.usedGB}/${r.totalGB}`); setBar('ram', pct); pushHist('ram', pct); }
  drawSpark('s-gpuTemp', mon.hist.gpuTemp); drawSpark('s-gpuUtil', mon.hist.gpuUtil); drawSpark('s-cpuLoad', mon.hist.cpuLoad); drawSpark('s-ram', mon.hist.ram);
}
function drawSpark(id, data) {
  const cv = document.getElementById(id); if (!cv) return;
  const ctx = cv.getContext('2d'), w = cv.width, h = cv.height; ctx.clearRect(0, 0, w, h);
  if (data.length < 2) return;
  const stepX = w / 59, max = 100;
  const grad = ctx.createLinearGradient(0, 0, w, 0); grad.addColorStop(0, '#58e0ff'); grad.addColorStop(1, '#ffa63d');
  ctx.beginPath(); data.forEach((v, i) => { const x = i * stepX, y = h - (Math.min(v, max) / max) * (h - 4) - 2; i ? ctx.lineTo(x, y) : ctx.moveTo(x, y); });
  ctx.lineTo((data.length - 1) * stepX, h); ctx.lineTo(0, h); ctx.closePath(); ctx.fillStyle = 'rgba(255,166,61,0.1)'; ctx.fill();
  ctx.beginPath(); data.forEach((v, i) => { const x = i * stepX, y = h - (Math.min(v, max) / max) * (h - 4) - 2; i ? ctx.lineTo(x, y) : ctx.moveTo(x, y); });
  ctx.strokeStyle = grad; ctx.lineWidth = 2; ctx.lineJoin = 'round'; ctx.stroke();
}

// ----------------------------- Perfiles -----------------------------
async function ensureProfileTweaks() {
  if (state.profileTweaks) return state.profileTweaks;
  const caps = { hasSSD: state.hasSSD, gpuVendors: state.gpuVendors.length ? state.gpuVendors : ['nvidia'] };
  const list = await api.listTweaks('agresiva', caps);
  state.profileTweaks = list.filter((t) => !t.isInstall && !t.informational && t.id !== 'nvidia-max-perf');
  return state.profileTweaks;
}
async function loadProfiles() {
  const cont = $('#profilesList');
  let list = [];
  try { list = await api.listProfiles(); } catch (_) {}
  if (!list.length) { cont.innerHTML = '<div class="empty">Aún no tienes perfiles. Crea uno para que se aplique solo al abrir tu juego.</div>'; return; }
  cont.innerHTML = list.map((p) => {
    const active = state.activeProfileId === p.id;
    const chips = [
      `<span class="autochip">${(p.tweaks || []).length} tweaks</span>`,
      p.gpuMax ? '<span class="autochip on">GPU máx</span>' : '',
      (p.suspend || []).length ? `<span class="autochip">${p.suspend.length} aligera</span>` : '',
      `<span class="autochip ${p.autoApply ? 'on' : ''}">${p.autoApply ? 'auto ON' : 'auto OFF'}</span>`,
    ].filter(Boolean).join('');
    return `<div class="pcard ${active ? 'active' : ''}" data-id="${p.id}">
      <div class="ph"><div><div class="pn">${p.name}</div><div class="pe">${p.exe || 'sin ejecutable'}</div></div></div>
      <div class="pmeta">${chips}</div>
      <div class="pact">
        <button class="btn btn-sm ${active ? 'btn-ghost' : 'btn-amber'}" data-act="toggle">${active ? 'Desactivar' : 'Activar ahora'}</button>
        <button class="btn btn-ghost btn-sm" data-act="edit">Editar</button>
        <button class="btn btn-ghost btn-sm" data-act="export">Exportar</button>
        <button class="btn btn-ghost btn-sm" data-act="delete">Borrar</button>
      </div></div>`;
  }).join('');
  cont.querySelectorAll('.pcard').forEach((card) => {
    const id = card.dataset.id;
    card.querySelectorAll('[data-act]').forEach((b) => b.addEventListener('click', () => profileAction(b.dataset.act, id)));
  });
}
async function profileAction(act, id) {
  const list = await api.listProfiles(); const p = list.find((x) => x.id === id);
  if (act === 'delete') { if (confirm(`¿Borrar el perfil "${p.name}"?`)) { await api.deleteProfile(id); loadProfiles(); } return; }
  if (act === 'edit') { openProfileEditor(p); return; }
  if (act === 'export') {
    const code = await api.exportProfile(id);
    try { await navigator.clipboard.writeText(code); toast('Código copiado al portapapeles ✓'); }
    catch (_) { toast('Código: ' + code); }
    return;
  }
  if (act === 'toggle') {
    if (state.activeProfileId === id) { await api.revertProfileNow(id); state.activeProfileId = null; hideBanner(); toast('Perfil desactivado.'); }
    else {
      const r = await api.applyProfileNow(id); state.activeProfileId = id;
      showBanner(`Perfil ${p.name} activo` + (r.freedMB ? ` · +${(r.freedMB / 1024).toFixed(1)} GB liberados` : ''));
      toast('Perfil aplicado.');
    }
    loadProfiles();
  }
}

// --- modal de perfil ---
$('#pmClose').addEventListener('click', closeModal);
$('#pmCancel').addEventListener('click', closeModal);
function closeModal() { $('#profileModal').hidden = true; }

$('#btnNewProfile').addEventListener('click', () => openProfileEditor(null));
async function openProfileEditor(profile) {
  const editing = !!profile;
  $('#pmTitle').textContent = editing ? 'Editar perfil' : 'Nuevo perfil';
  const body = $('#pmBody');
  body.innerHTML = '<div class="muted">Cargando juegos y opciones…</div>';
  $('#profileModal').hidden = false;

  const [tweaks, steam, running, cands] = await Promise.all([
    ensureProfileTweaks(), api.steamGames().catch(() => []), api.runningProcs().catch(() => []), api.suspendCandidates().catch(() => []),
  ]);
  const gameOpts = [
    '<option value="">— elige un juego detectado —</option>',
    ...steam.map((g) => `<option value="${g.exe}" data-name="${g.name.replace(/"/g, '')}">🎮 ${g.name}${g.exe ? '' : ' (sin exe)'}</option>`),
    ...running.map((r) => `<option value="${r.exe}" data-name="${(r.title || r.name).replace(/"/g, '')}">▶ ${r.name} (abierto)</option>`),
  ].join('');
  const selTweaks = new Set(profile ? profile.tweaks || [] : []);
  const selSusp = new Set(profile ? profile.suspend || [] : []);
  const suspNames = [...new Set([...(cands.map((c) => c.name)), ...selSusp])];

  body.innerHTML = `
    <div class="field"><label>Nombre del perfil</label><input id="pf-name" value="${profile ? profile.name.replace(/"/g, '&quot;') : ''}" placeholder="Ej: Valorant competitivo" /></div>
    <div class="field"><label>Juego detectado (rellena el ejecutable)</label><select id="pf-game">${gameOpts}</select></div>
    <div class="field"><label>Ejecutable (.exe) que dispara el perfil</label><input id="pf-exe" value="${profile ? (profile.exe || '') : ''}" placeholder="valorant.exe" /></div>
    <div class="field"><label>Optimizaciones a aplicar</label><div class="checklist" id="pf-tweaks">
      ${tweaks.map((t) => `<label class="citem"><input type="checkbox" value="${t.id}" ${selTweaks.has(t.id) ? 'checked' : ''}/> ${t.title}</label>`).join('')}
    </div></div>
    ${state.gpuVendors.includes('nvidia') ? `<label class="citem"><input type="checkbox" id="pf-gpu" ${profile && profile.gpuMax ? 'checked' : ''}/> Subir GPU NVIDIA al máximo (power limit)</label>` : ''}
    <div class="field"><label>Aligerar durante la partida (RAM y prioridad; se restauran al salir)</label><div class="checklist" id="pf-susp">
      ${suspNames.length ? suspNames.map((n) => { const c = cands.find((x) => x.name === n); return `<label class="citem"><input type="checkbox" value="${n}" ${selSusp.has(n) ? 'checked' : ''}/> ${n}${c ? ` · ${c.memMB} MB` : ''}</label>`; }).join('') : '<div class="muted">No hay apps de fondo típicas abiertas ahora.</div>'}
    </div></div>
    <label class="citem"><input type="checkbox" id="pf-auto" ${profile && profile.autoApply ? 'checked' : (editing ? '' : 'checked')}/> Aplicar automáticamente al abrir el juego</label>`;

  $('#pf-game').addEventListener('change', (e) => {
    const opt = e.target.selectedOptions[0];
    if (opt && opt.value) { $('#pf-exe').value = opt.value; if (!$('#pf-name').value) $('#pf-name').value = opt.dataset.name || ''; }
  });

  $('#pmSave').onclick = async () => {
    const name = $('#pf-name').value.trim();
    const exe = $('#pf-exe').value.trim();
    if (!name) { toast('Ponle un nombre al perfil.', true); return; }
    const payload = {
      ...(profile || {}), name, exe,
      tweaks: $$('#pf-tweaks input:checked').map((i) => i.value),
      gpuMax: !!$('#pf-gpu')?.checked,
      suspend: $$('#pf-susp input:checked').map((i) => i.value),
      autoApply: !!$('#pf-auto').checked,
    };
    await api.saveProfile(payload);
    closeModal(); loadProfiles(); toast('Perfil guardado ✓');
  };
}

// --- importar ---
$('#btnImport').addEventListener('click', () => {
  $('#pmTitle').textContent = 'Importar perfil';
  $('#pmBody').innerHTML = `<div class="field"><label>Pega el código del perfil (empieza por OVD-)</label><input id="pf-code" placeholder="OVD-..." /></div>`;
  $('#profileModal').hidden = false;
  $('#pmSave').onclick = async () => {
    const code = $('#pf-code').value.trim();
    try { const p = await api.importProfile(code); closeModal(); loadProfiles(); toast(`Perfil "${p.name}" importado ✓`); }
    catch (err) { toast(err.message || 'Código no válido', true); }
  };
});

// --- eventos del vigilante (juego abierto/cerrado) ---
api.onProfileEvent((ev) => {
  if (ev.type === 'applied') { state.activeProfileId = ev.id; showBanner(`Perfil ${ev.name} activo` + (ev.freedMB ? ` · +${(ev.freedMB / 1024).toFixed(1)} GB liberados` : '')); }
  else if (ev.type === 'reverted') { if (state.activeProfileId === ev.id) { state.activeProfileId = null; hideBanner(); } toast(`Perfil ${ev.name} revertido (juego cerrado).`); }
  if ($('.tab[data-tab="perfiles"]').classList.contains('is-active')) loadProfiles();
});
function showBanner(text) { $('#activeBannerText').textContent = text; $('#activeBanner').hidden = false; }
function hideBanner() { $('#activeBanner').hidden = true; }

// ----------------------------- Benchmark -----------------------------
$('#btnBenchBefore').addEventListener('click', () => runBench('before'));
$('#btnBenchAfter').addEventListener('click', () => runBench('after'));
async function runBench(which) {
  $('#benchRunning').hidden = false; $('#benchHint').hidden = true;
  $('#btnBenchBefore').disabled = true; $('#btnBenchAfter').disabled = true;
  try {
    const r = await api.runBenchmark();
    if (which === 'before') { state.benchBefore = r; state.benchAfter = null; }
    else state.benchAfter = r;
    renderBench();
  } catch (err) { toast('Error en el benchmark: ' + err.message, true); }
  finally {
    $('#benchRunning').hidden = true;
    $('#btnBenchBefore').disabled = false;
    $('#btnBenchAfter').disabled = !state.benchBefore;
  }
}
function renderBench() {
  const b = state.benchBefore, a = state.benchAfter;
  if (!b) return;
  const out = $('#benchResults'); out.hidden = false;
  const row = (k, before, after, unit, higherBetter) => {
    let delta = '';
    if (after != null && before != null && before !== 0) {
      const pct = ((after - before) / before) * 100;
      const good = higherBetter ? pct >= 0 : pct <= 0;
      delta = `<span class="bd ${good ? 'delta up' : 'delta down'}">${pct >= 0 ? '+' : ''}${pct.toFixed(1)}%</span>`;
    }
    return `<div class="brow"><div class="bk">${k}</div><div class="bv">${before ?? '—'}${after != null ? ` → ${after}` : ''} <span class="muted" style="font-size:12px">${unit}</span> ${delta}</div></div>`;
  };
  let scoreBlock = `<div class="bench-score"><div><div class="bk muted">Puntuación global</div><div class="big">${b.score}${a ? ' → ' + a.score : ''}</div></div>`;
  if (a) { const pct = ((a.score - b.score) / b.score) * 100; scoreBlock += `<div class="delta ${pct >= 0 ? 'up' : 'down'}">${pct >= 0 ? '+' : ''}${pct.toFixed(1)}% rendimiento</div>`; }
  scoreBlock += '</div>';
  out.innerHTML = scoreBlock + '<div class="bench-rows">' + [
    row('CPU 1 hilo (ms)', b.cpuSingleMs, a?.cpuSingleMs, 'ms', false),
    row('CPU multihilo (ms)', b.cpuMultiMs, a?.cpuMultiMs, `ms · ${b.cpuThreads}t`, false),
    row('Disco lectura', b.diskReadMBs, a?.diskReadMBs, 'MB/s', true),
    row('Disco escritura', b.diskWriteMBs, a?.diskWriteMBs, 'MB/s', true),
  ].join('') + '</div>';
}

// ----------------------------- Modo Fácil / Experto -----------------------------
function setMode(mode) {
  const easy = mode === 'easy';
  $$('.ms-btn').forEach((b) => b.classList.toggle('is-on', b.dataset.mode === mode));
  $('#easyView').hidden = !easy;
  $('#nav').hidden = easy;
  if (easy) {
    $$('.tab').forEach((t) => t.classList.remove('is-active'));
  } else if (!$('.tab.is-active')) {
    $('.nav-item[data-tab="analisis"]').classList.add('is-active');
    $('.tab[data-tab="analisis"]').classList.add('is-active');
  }
  try { localStorage.setItem('ovd-mode', mode); } catch (_) {}
}
$('#modeSwitch').addEventListener('click', (e) => { const b = e.target.closest('.ms-btn'); if (b) setMode(b.dataset.mode); });
(function initMode() { let m = 'easy'; try { m = localStorage.getItem('ovd-mode') || 'easy'; } catch (_) {} setMode(m); })();

// --- Optimizar en un clic ---
let easyBusy = false;
$('#btnEasyOptimize').addEventListener('click', easyOptimize);
async function easyOptimize() {
  if (easyBusy) return; easyBusy = true;
  const orb = $('#easyOrb'), btn = $('#btnEasyOptimize'), status = $('#easyStatus');
  btn.disabled = true; orb.classList.remove('done'); orb.classList.add('busy');
  status.textContent = 'Analizando tu equipo…';
  try {
    if (!state.analysis) {
      const d = await api.analyze();
      state.analysis = d; state.hasSSD = (d.disks || []).some((x) => /ssd/i.test(x.media || '')); state.gpuVendors = detectVendors(d.gpu);
      if (!d.admin) $('#adminBadge').hidden = false;
    }
    state.level = 'media';
    const list = await api.listTweaks('media', { hasSSD: state.hasSSD, gpuVendors: state.gpuVendors });
    state.tweaks = list;
    const ids = list.filter((t) => t.state !== 'on' && !t.isInstall && !t.informational).map((t) => t.id);
    if (!ids.length) { orb.classList.remove('busy'); orb.classList.add('done'); status.innerHTML = 'Tu PC ya estaba optimizado. <span class="gain">✓</span>'; return; }
    status.textContent = 'Aplicando mejoras…';
    const results = await runApply(ids, true);
    orb.classList.remove('busy');
    if (!results) { status.textContent = 'Necesita permisos de administrador. Acepta el aviso de Windows.'; return; }
    const ok = results.filter((r) => r.ok).length;
    orb.classList.add('done');
    status.innerHTML = `Listo. <span class="gain">${ok} mejoras</span> aplicadas. Tu PC está preparado para jugar.`;
    $('#btnEasyUndo').hidden = state.applied.length === 0;
  } catch (err) { orb.classList.remove('busy'); status.textContent = 'No se pudo optimizar: ' + err.message; }
  finally { btn.disabled = false; easyBusy = false; }
}
$('#btnEasyUndo').addEventListener('click', async () => {
  if (!state.applied.length) { toast('No hay nada que deshacer.'); return; }
  const overlay = $('#overlay'); $('#overlayText').textContent = 'Deshaciendo…'; $('#overlayNote').textContent = ''; $('#progressBar').style.width = '50%'; overlay.hidden = false;
  try {
    await api.revert(state.applied); $('#progressBar').style.width = '100%';
    state.applied = []; $('#btnEasyUndo').hidden = true; $('#easyOrb').classList.remove('done');
    $('#easyStatus').textContent = 'Optimización deshecha. Tu PC volvió al estado anterior.'; await loadTweaks();
  } catch (e) { toast('Error al deshacer: ' + e.message, true); } finally { overlay.hidden = true; }
});

// --- Modo Juego (aligerar apps de fondo: recortar RAM + bajar prioridad, sin congelarlas) ---
let gameModeSusp = null;
$('#btnGameMode').addEventListener('click', toggleGameMode);
// El estado vive en el proceso principal: si la ventana se recarga, el botón no debe "olvidarlo".
api.gameModeStatus().then((s) => {
  if (s && s.active) {
    gameModeSusp = [];
    const btn = $('#btnGameMode');
    btn.textContent = 'Desactivar Modo Juego'; btn.classList.add('live');
  }
}).catch(() => {});
async function toggleGameMode() {
  const btn = $('#btnGameMode'), freed = $('#gameModeFreed');
  if (gameModeSusp) {
    btn.disabled = true; btn.textContent = 'Restaurando…';
    await api.resumeProcs(gameModeSusp); gameModeSusp = null;
    btn.disabled = false; btn.textContent = 'Activar Modo Juego'; btn.classList.remove('live'); freed.hidden = true;
    toast('Apps de fondo restauradas.');
    return;
  }
  btn.disabled = true; btn.textContent = 'Buscando apps…';
  const cands = await api.suspendCandidates();
  const names = (cands || []).map((c) => c.name);
  if (!names.length) { btn.disabled = false; btn.textContent = 'Activar Modo Juego'; toast('No hay apps de fondo típicas abiertas ahora.'); return; }
  btn.textContent = 'Liberando RAM…';
  const r = await api.suspendProcs(names);
  btn.disabled = false; gameModeSusp = names;
  btn.textContent = 'Desactivar Modo Juego'; btn.classList.add('live');
  freed.hidden = false; freed.textContent = `✓ ${r.count} apps aligeradas · +${(r.freedMB / 1024).toFixed(1)} GB de RAM liberados`;
}

// --- Estado del equipo (vitales simples) ---
$('#btnEasyVitals').addEventListener('click', async () => {
  const el = $('#easyVitals'); el.textContent = 'Leyendo…';
  try {
    const m = await api.monitorSample(); const p = [];
    if (m.gpu && m.gpu.tempC != null) p.push(`GPU: <b>${m.gpu.tempC}°</b> · ${m.gpu.util ?? '—'}% de uso`);
    if (m.cpu) p.push(`CPU: <b>${m.cpu.load ?? '—'}%</b> de uso`);
    if (m.ram && m.ram.totalGB) p.push(`RAM: <b>${m.ram.usedGB} / ${m.ram.totalGB} GB</b>`);
    el.innerHTML = p.join('<br>') || 'No se pudo leer el estado.';
  } catch (_) { el.textContent = 'No se pudo leer el estado.'; }
});

// ----------------------------- Auto-updater -----------------------------
$('#btnCheckUpdate').addEventListener('click', () => { $('#updateStatus').textContent = 'Comprobando…'; api.checkUpdates(); });
$('#btnInstallUpdate').addEventListener('click', () => api.installUpdate());
api.onUpdateStatus((d) => {
  const st = $('#updateStatus'), prog = $('#updateProgress'), ib = $('#btnInstallUpdate');
  switch (d.status) {
    case 'checking': st.textContent = 'Comprobando…'; break;
    case 'available': st.textContent = `Descargando v${d.version}…`; prog.hidden = false; break;
    case 'none': st.textContent = 'Estás al día ✓'; break;
    case 'downloading': st.textContent = `Descargando ${d.percent}%`; prog.hidden = false; $('#updateBar').style.width = d.percent + '%'; break;
    case 'downloaded': st.textContent = `v${d.version} lista`; ib.hidden = false; prog.hidden = true; break;
    case 'dev': st.textContent = 'Updates solo en el .exe'; break;
    case 'error': st.textContent = 'Sin conexión de updates'; break;
    default: break;
  }
});

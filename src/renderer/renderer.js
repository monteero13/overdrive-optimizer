'use strict';
const api = window.overdrive;

const state = {
  analysis: null,
  hasSSD: false,
  gpuVendors: [],
  level: 'media',
  tweaks: [],
  selected: new Set(),
  applied: [],
};

// Deduce los fabricantes de GPU presentes (nvidia / amd / intel).
function detectVendors(gpus) {
  const set = new Set();
  for (const g of gpus || []) {
    const n = (g.name || '').toLowerCase();
    if (n.includes('nvidia') || n.includes('geforce')) set.add('nvidia');
    if (n.includes('amd') || n.includes('radeon')) set.add('amd');
    if (n.includes('intel')) set.add('intel');
  }
  return [...set];
}

const $ = (sel) => document.querySelector(sel);

// ----------------------------- Utilidades UI -----------------------------
function toast(msg, isErr = false) {
  const t = $('#toast');
  t.textContent = msg;
  t.classList.toggle('err', isErr);
  t.hidden = false;
  requestAnimationFrame(() => t.classList.add('show'));
  clearTimeout(toast._t);
  toast._t = setTimeout(() => {
    t.classList.remove('show');
    setTimeout(() => (t.hidden = true), 300);
  }, 3200);
}

function show(sel) { const el = $(sel); if (el) el.hidden = false; }

// ----------------------------- Analisis -----------------------------
$('#btnScan').addEventListener('click', runScan);

async function runScan() {
  $('#btnScan').disabled = true;
  $('#scanHint').hidden = true;
  $('#specs').hidden = true;
  $('#driversBox').hidden = true;
  show('#scanning');
  try {
    const data = await api.analyze(); // solo hardware: rapido
    state.analysis = data;
    state.hasSSD = (data.disks || []).some((d) => /ssd/i.test(d.media || ''));
    state.gpuVendors = detectVendors(data.gpu);
    renderSpecs(data);
    if (!data.admin) $('#adminBadge').hidden = false;
    show('#panel-level');
    await loadTweaks();
    show('#panel-plan');
    // El escaneo de drivers es lento: va aparte y NO bloquea el plan.
    scanDrivers();
  } catch (err) {
    toast('Error al analizar: ' + err.message, true);
  } finally {
    $('#scanning').hidden = true;
    $('#btnScan').disabled = false;
    $('#btnScan').querySelector('.btn-label').textContent = 'Volver a escanear';
  }
}

// Escaneo de drivers en segundo plano (no bloquea la interfaz).
async function scanDrivers() {
  const box = $('#driversBox');
  const cont = $('#driversList');
  box.hidden = false;
  cont.innerHTML = '<div class="hint">Buscando drivers… (puede tardar unos segundos)</div>';
  try {
    const drv = await api.analyzeDrivers();
    renderDrivers(drv.driversOutdated || []);
  } catch (err) {
    cont.innerHTML = '<div class="hint">No se pudieron comprobar los drivers en este equipo.</div>';
  }
}

function specCard(label, value, extra, barPct) {
  const bar = barPct != null ? `<div class="bar"><i style="width:${barPct}%"></i></div>` : '';
  return `<div class="spec">
    <div class="spec-label">${label}</div>
    <div class="spec-value">${value}</div>
    ${extra ? `<div class="spec-extra">${extra}</div>` : ''}
    ${bar}
  </div>`;
}

function renderSpecs(d) {
  const grid = $('#specs');
  const gpu = (d.gpu && d.gpu[0]) || {};
  const ramPct = d.ram.totalGB ? Math.round(((d.ram.totalGB - d.ram.freeGB) / d.ram.totalGB) * 100) : 0;
  const vol = (d.volumes || []).find((v) => v.drive === 'C:') || (d.volumes || [])[0] || {};
  const diskPct = vol.totalGB ? Math.round(((vol.totalGB - vol.freeGB) / vol.totalGB) * 100) : 0;
  const ssd = (d.disks || []).map((x) => x.media).filter(Boolean).join(', ') || '—';

  grid.innerHTML = [
    specCard('CPU', d.cpu.name || '—', `${d.cpu.cores} nucleos · ${d.cpu.threads} hilos · ${(d.cpu.clockMHz/1000).toFixed(1)} GHz`, d.cpu.load),
    specCard('GPU', gpu.name || '—', `Driver ${gpu.driverVersion || '—'} · ${gpu.driverDate || 'fecha desconocida'}`),
    specCard('RAM', `${d.ram.totalGB} GB`, `${(d.ram.totalGB - d.ram.freeGB).toFixed(1)} GB en uso · ${d.ram.freeGB} GB libres`, ramPct),
    specCard('Disco C:', `${vol.freeGB ?? '—'} GB libres`, `${ssd} · ${vol.totalGB ?? '—'} GB totales`, diskPct),
    specCard('Sistema', d.os.name || 'Windows', `Build ${d.os.build} · ${d.os.arch}`),
    specCard('Energia', d.powerPlan || '—', `Ultimo arranque: ${d.os.lastBoot}`),
  ].join('');
  grid.hidden = false;
  // Animar barras
  requestAnimationFrame(() => {
    grid.querySelectorAll('.bar > i').forEach((el) => {
      const w = el.style.width; el.style.width = '0'; requestAnimationFrame(() => (el.style.width = w));
    });
  });
}

function renderDrivers(list) {
  const box = $('#driversBox');
  const cont = $('#driversList');
  if (!list.length) {
    cont.innerHTML = `<div class="hint">Todos los drivers relevantes estan razonablemente actualizados. 👍</div>`;
    box.hidden = false;
    return;
  }
  cont.innerHTML = list.slice(0, 12).map((d) => {
    const years = (d.ageDays / 365).toFixed(1);
    const sev = d.severity === 'muy-desactualizado'
      ? '<span class="sev muy">MUY DESACTUALIZADO</span>'
      : '<span class="sev des">Desactualizado</span>';
    return `<div class="driver">
      <div class="d-main">
        <div class="d-name">${d.device}</div>
        <div class="d-meta">${d.class} · v${d.version} · ${d.date} (${years} años)</div>
      </div>
      <div style="display:flex;align-items:center;gap:10px;flex-shrink:0">
        ${sev}
        <button class="btn btn-ghost btn-driver" data-url="${d.vendorUrl}">Actualizar</button>
      </div>
    </div>`;
  }).join('');
  cont.querySelectorAll('.btn-driver').forEach((b) => {
    b.addEventListener('click', () => {
      api.openExternal(b.dataset.url);
      toast('Abriendo la pagina de descarga oficial del driver…');
    });
  });
  box.hidden = false;
}

// ----------------------------- Nivel -----------------------------
$('#levelSelector').addEventListener('click', (e) => {
  const btn = e.target.closest('.level');
  if (!btn) return;
  $('#levelSelector').querySelectorAll('.level').forEach((l) => l.classList.remove('is-active'));
  btn.classList.add('is-active');
  state.level = btn.dataset.level;
  loadTweaks();
});

// ----------------------------- Plan -----------------------------
async function loadTweaks() {
  try {
    const list = await api.listTweaks(state.level, { hasSSD: state.hasSSD, gpuVendors: state.gpuVendors });
    state.tweaks = list;
    // Por defecto seleccionamos los que estan 'off' y no son instalaciones/informativos.
    state.selected = new Set(
      list.filter((t) => t.state !== 'on' && !t.isInstall && !t.informational).map((t) => t.id)
    );
    renderPlan();
  } catch (err) {
    toast('No se pudo cargar el plan: ' + err.message, true);
  }
}

function renderPlan() {
  const cont = $('#planList');
  cont.innerHTML = state.tweaks.map((t) => {
    const sel = state.selected.has(t.id) ? 'sel' : '';
    const onLabel = t.isInstall ? 'instalado' : 'ya activo';
    const tags = [
      `<span class="tag risk-${t.risk}">${t.risk === 'low' ? 'riesgo bajo' : t.risk === 'medium' ? 'riesgo medio' : 'riesgo alto'}</span>`,
      t.isInstall ? '<span class="tag install">instalacion</span>' : '',
      t.informational ? '<span class="tag install">abre app</span>' : '',
      t.state === 'on' ? `<span class="tag state-on">${onLabel}</span>` : '',
      t.needsAdmin ? '<span class="tag admin">admin</span>' : '',
      t.needsReboot ? '<span class="tag reboot">reinicio</span>' : '',
    ].filter(Boolean).join(' ');
    return `<div class="tweak ${sel}" data-id="${t.id}">
      <div class="chk"></div>
      <div class="t-body">
        <div class="t-title">${t.title} ${tags}</div>
        <div class="t-desc">${t.desc}</div>
      </div>
    </div>`;
  }).join('');
  cont.querySelectorAll('.tweak').forEach((el) => {
    el.addEventListener('click', () => {
      const id = el.dataset.id;
      if (state.selected.has(id)) state.selected.delete(id);
      else state.selected.add(id);
      el.classList.toggle('sel');
      updateApplyBtn();
    });
  });
  updateApplyBtn();
}

function updateApplyBtn() {
  const btn = $('#btnApply');
  const n = state.selected.size;
  btn.disabled = n === 0;
  btn.textContent = n ? `Aplicar ${n} optimizacion${n > 1 ? 'es' : ''}` : 'Aplicar seleccionados';
}

// ----------------------------- Aplicar -----------------------------
$('#btnApply').addEventListener('click', applyPlan);

async function applyPlan() {
  const ids = [...state.selected];
  if (!ids.length) return;

  // Aviso si hay tweaks que requieren admin y no lo somos.
  const needsAdmin = state.tweaks.some((t) => ids.includes(t.id) && t.needsAdmin);
  if (needsAdmin && state.analysis && !state.analysis.admin) {
    toast('Algunas optimizaciones requieren administrador. Relanzando con permisos…', true);
    await api.relaunchAdmin();
    return;
  }

  const overlay = $('#overlay');
  const bar = $('#progressBar');
  overlay.hidden = false;
  bar.style.width = '0';

  // Punto de restauracion
  if ($('#chkRestore').checked) {
    $('#overlayText').textContent = 'Creando punto de restauración… (puede tardar hasta 1 min)';
    bar.classList.add('indet');
    const rp = await api.restorePoint();
    bar.classList.remove('indet');
    if (!rp.ok) {
      toast('No se pudo crear el punto de restauración: ' + (rp.message || ''), true);
      // Continuamos igualmente, pero avisado.
    }
  }

  const off = api.onProgress((p) => {
    if (p.done) { bar.classList.remove('indet'); bar.style.width = '100%'; return; }
    const long = !!p.isInstall;
    $('#overlayText').textContent = (long ? 'Instalando: ' : 'Aplicando: ') + p.title + (long ? ' (puede tardar varios minutos)' : '');
    bar.classList.toggle('indet', long);
    if (!long) bar.style.width = `${Math.round((p.index / p.total) * 100)}%`;
  });

  let results = [];
  try {
    results = await api.apply(ids);
  } catch (err) {
    toast('Error al aplicar: ' + err.message, true);
  }
  off();
  overlay.hidden = true;

  state.applied = results.filter((r) => r.ok).map((r) => r.id);
  renderResults(results);
  show('#panel-result');
  $('#panel-result').scrollIntoView({ behavior: 'smooth' });

  const okCount = results.filter((r) => r.ok).length;
  const reboot = results.some((r) => r.ok && r.needsReboot);
  toast(`${okCount}/${results.length} aplicadas correctamente${reboot ? ' · requiere reinicio' : ''}`);

  // Refrescar estados
  await loadTweaks();
}

function renderResults(results) {
  const cont = $('#resultList');
  const byId = Object.fromEntries(state.tweaks.map((t) => [t.id, t.title]));
  cont.innerHTML = results.map((r) => {
    const title = byId[r.id] || r.id;
    return `<div class="result-row ${r.ok ? 'ok' : 'err'}">
      <span class="ic">${r.ok ? '✓' : '!'}</span>
      <span>${title}${r.ok ? '' : ' — ' + (r.message || 'fallo')}${r.needsReboot && r.ok ? ' (reinicia para aplicar)' : ''}</span>
    </div>`;
  }).join('');
}

// ----------------------------- Monitor en tiempo real -----------------------------
const mon = { timer: null, running: false, hist: {} };
['gpuTemp', 'gpuUtil', 'cpuLoad', 'ram'].forEach((k) => (mon.hist[k] = []));

$('#btnMonitor').addEventListener('click', () => (mon.running ? stopMonitor() : startMonitor()));

function startMonitor() {
  mon.running = true;
  const b = $('#btnMonitor');
  b.textContent = 'Detener monitor';
  b.classList.add('live');
  $('#monitorGrid').hidden = false;
  $('#monitorHint').hidden = true;
  tickMonitor();
  mon.timer = setInterval(tickMonitor, 1000);
}
function stopMonitor() {
  mon.running = false;
  clearInterval(mon.timer);
  const b = $('#btnMonitor');
  b.textContent = 'Iniciar monitor';
  b.classList.remove('live');
}

async function tickMonitor() {
  let m;
  try { m = await api.monitorSample(); } catch (_) { return; }
  if (!mon.running) return;
  renderMonitor(m);
}

function setVal(id, text, cls) {
  const el = $('#m-' + id);
  if (!el) return;
  el.textContent = text;
  el.className = 'mval' + (cls ? ' ' + cls : '');
}
function setBar(id, pct, color) {
  const el = $('#b-' + id);
  if (!el) return;
  el.style.width = Math.max(0, Math.min(100, pct)) + '%';
  if (color) el.style.background = color;
}
function pushHist(key, v) {
  const a = mon.hist[key];
  a.push(v);
  if (a.length > 60) a.shift();
}
function tempClass(c) {
  if (c == null) return '';
  if (c >= 85) return 'danger';
  if (c >= 75) return 'warn';
  if (c < 60) return 'ok';
  return '';
}
function tempColor(c) {
  if (c == null) return null;
  if (c >= 85) return 'var(--danger)';
  if (c >= 75) return 'var(--warn)';
  return 'var(--accent-grad)';
}

function renderMonitor(m) {
  const g = m.gpu;
  if (g) {
    setVal('gpuTemp', g.tempC != null ? g.tempC + '°C' : 'N/D', tempClass(g.tempC));
    setBar('gpuTemp', g.tempC != null ? g.tempC : 0, tempColor(g.tempC));
    pushHist('gpuTemp', g.tempC || 0);

    setVal('gpuUtil', g.util != null ? g.util + '%' : 'N/D');
    setBar('gpuUtil', g.util || 0);
    pushHist('gpuUtil', g.util || 0);

    const pPct = g.powerLimitW ? (g.powerW / g.powerLimitW) * 100 : 0;
    setVal('gpuPower', g.powerW != null ? g.powerW.toFixed(0) + ' W' : 'N/D');
    setBar('gpuPower', pPct);
    $('#m-gpuExtra').textContent =
      `reloj ${g.clockMHz ?? '—'} MHz · fan ${g.fanPct != null ? g.fanPct + '%' : '—'} · límite ${g.powerLimitW ?? '—'} W`;
  } else {
    ['gpuTemp', 'gpuUtil', 'gpuPower'].forEach((k) => setVal(k, 'N/D'));
    $('#m-gpuExtra').textContent = 'GPU NVIDIA no detectada';
  }

  const c = m.cpu;
  if (c) {
    setVal('cpuLoad', c.load != null ? c.load + '%' : 'N/D');
    setBar('cpuLoad', c.load || 0);
    pushHist('cpuLoad', c.load || 0);
    setVal('cpuClock', c.clock != null ? (c.clock / 1000).toFixed(2) + ' GHz' : 'N/D');
    $('#m-cpuExtra').textContent = c.temp != null ? `temp ACPI ${c.temp}°C (zona placa)` : 'temp ACPI no disponible';
  }

  const r = m.ram;
  if (r && r.totalGB) {
    const pct = Math.round((r.usedGB / r.totalGB) * 100);
    setVal('ram', `${r.usedGB} / ${r.totalGB} GB`);
    setBar('ram', pct);
    pushHist('ram', pct);
  }

  drawSpark('s-gpuTemp', mon.hist.gpuTemp, 100);
  drawSpark('s-gpuUtil', mon.hist.gpuUtil, 100);
  drawSpark('s-cpuLoad', mon.hist.cpuLoad, 100);
  drawSpark('s-ram', mon.hist.ram, 100);
}

function drawSpark(id, data, max) {
  const cv = document.getElementById(id);
  if (!cv) return;
  const ctx = cv.getContext('2d');
  const w = cv.width, h = cv.height;
  ctx.clearRect(0, 0, w, h);
  if (data.length < 2) return;
  const n = data.length;
  const stepX = w / (60 - 1);
  const grad = ctx.createLinearGradient(0, 0, w, 0);
  grad.addColorStop(0, '#6c7bff');
  grad.addColorStop(1, '#37e2d5');
  // área
  ctx.beginPath();
  data.forEach((v, i) => {
    const x = i * stepX;
    const y = h - (Math.min(v, max) / max) * (h - 4) - 2;
    i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y);
  });
  ctx.lineTo((n - 1) * stepX, h);
  ctx.lineTo(0, h);
  ctx.closePath();
  ctx.fillStyle = 'rgba(108,123,255,0.12)';
  ctx.fill();
  // línea
  ctx.beginPath();
  data.forEach((v, i) => {
    const x = i * stepX;
    const y = h - (Math.min(v, max) / max) * (h - 4) - 2;
    i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y);
  });
  ctx.strokeStyle = grad;
  ctx.lineWidth = 2;
  ctx.lineJoin = 'round';
  ctx.stroke();
}

// ----------------------------- Auto-updater -----------------------------
$('#btnCheckUpdate').addEventListener('click', () => {
  $('#updateStatus').textContent = 'Comprobando actualizaciones…';
  api.checkUpdates();
});
$('#btnInstallUpdate').addEventListener('click', () => api.installUpdate());

api.onUpdateStatus((d) => {
  const st = $('#updateStatus');
  const prog = $('#updateProgress');
  const installBtn = $('#btnInstallUpdate');
  switch (d.status) {
    case 'checking': st.textContent = 'Comprobando actualizaciones…'; break;
    case 'available': st.textContent = `Nueva versión ${d.version} disponible, descargando…`; prog.hidden = false; break;
    case 'none': st.textContent = 'Estás en la última versión. ✓'; break;
    case 'downloading': st.textContent = `Descargando actualización… ${d.percent}%`; prog.hidden = false; $('#updateBar').style.width = d.percent + '%'; break;
    case 'downloaded': st.textContent = `Versión ${d.version} lista para instalar.`; installBtn.hidden = false; prog.hidden = true; break;
    case 'dev': st.textContent = 'Las actualizaciones solo funcionan en la versión instalada (.exe).'; break;
    case 'error': st.textContent = 'No se pudo comprobar actualizaciones: ' + (d.message || ''); break;
    default: break;
  }
});

// ----------------------------- Revertir -----------------------------
$('#btnRevert').addEventListener('click', async () => {
  if (!state.applied.length) { toast('No hay cambios reversibles que deshacer.'); return; }
  const overlay = $('#overlay');
  $('#overlayText').textContent = 'Revirtiendo cambios…';
  $('#progressBar').style.width = '50%';
  overlay.hidden = false;
  try {
    const res = await api.revert(state.applied);
    $('#progressBar').style.width = '100%';
    const ok = res.filter((r) => r.ok).length;
    toast(`${ok} cambios revertidos.`);
    state.applied = [];
    await loadTweaks();
  } catch (err) {
    toast('Error al revertir: ' + err.message, true);
  } finally {
    overlay.hidden = true;
  }
});

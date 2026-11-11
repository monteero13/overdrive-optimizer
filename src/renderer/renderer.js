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
    const data = await api.analyze();
    state.analysis = data;
    state.hasSSD = (data.disks || []).some((d) => /ssd/i.test(d.media || ''));
    state.gpuVendors = detectVendors(data.gpu);
    renderSpecs(data);
    renderDrivers(data.driversOutdated || []);
    if (!data.admin) $('#adminBadge').hidden = false;
    show('#panel-level');
    await loadTweaks();
    show('#panel-plan');
  } catch (err) {
    toast('Error al analizar: ' + err.message, true);
  } finally {
    $('#scanning').hidden = true;
    $('#btnScan').disabled = false;
    $('#btnScan').querySelector('.btn-label').textContent = 'Volver a escanear';
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
    $('#overlayText').textContent = 'Creando punto de restauracion…';
    const rp = await api.restorePoint();
    if (!rp.ok) {
      toast('No se pudo crear el punto de restauracion: ' + (rp.message || ''), true);
      // Continuamos igualmente, pero avisado.
    }
  }

  const off = api.onProgress((p) => {
    if (p.done) { bar.style.width = '100%'; return; }
    $('#overlayText').textContent = `Aplicando: ${p.title}`;
    bar.style.width = `${Math.round((p.index / p.total) * 100)}%`;
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

'use strict';
// Micro-benchmark para medir el efecto real de las optimizaciones (antes/después).
// CPU monohilo y multihilo (worker_threads) + disco. Trabajo FIJO, se mide el tiempo:
// menos ms = mejor. Se deriva un "score" legible (más alto = mejor).

const { Worker } = require('worker_threads');
const os = require('os');
const fs = require('fs');
const path = require('path');

// Worker: ejecuta una cantidad fija de cómputo y devuelve el tiempo.
const WORKER_SRC = `
const { parentPort, workerData } = require('worker_threads');
let x = 0;
const t = Date.now();
for (let i = 0; i < workerData.iter; i++) { x += Math.sqrt(i * 2.1) + Math.sin(i * 0.5); }
parentPort.postMessage({ ms: Date.now() - t, x });
`;

const ITER = 6_000_000;

function runWorker() {
  return new Promise((resolve, reject) => {
    const w = new Worker(WORKER_SRC, { eval: true, workerData: { iter: ITER } });
    w.once('message', (m) => { w.terminate(); resolve(m.ms); });
    w.once('error', reject);
  });
}

async function cpuSingle() {
  // mejor de 2 pasadas para reducir ruido
  const a = await runWorker();
  const b = await runWorker();
  return Math.min(a, b);
}

async function cpuMulti() {
  const threads = Math.min(os.cpus().length || 4, 16);
  const t = Date.now();
  await Promise.all(Array.from({ length: threads }, () => runWorker()));
  return { ms: Date.now() - t, threads };
}

function disk() {
  const tmp = path.join(os.tmpdir(), 'ovd_bench_' + Date.now() + '.bin');
  const mb = 64;
  const buf = Buffer.alloc(mb * 1024 * 1024, 7);
  let writeMs = 0, readMs = 0;
  try {
    let t = Date.now();
    const fd = fs.openSync(tmp, 'w');
    fs.writeSync(fd, buf);
    fs.fsyncSync(fd);
    fs.closeSync(fd);
    writeMs = Date.now() - t;
    t = Date.now();
    fs.readFileSync(tmp);
    readMs = Date.now() - t;
  } catch (_) {
    /* disco no medible */
  } finally {
    try { fs.unlinkSync(tmp); } catch (_) {}
  }
  return {
    writeMBs: writeMs ? Math.round(mb / (writeMs / 1000)) : null,
    readMBs: readMs ? Math.round(mb / (readMs / 1000)) : null,
  };
}

async function run() {
  const single = await cpuSingle();
  const multi = await cpuMulti();
  const d = disk();
  const score = Math.round(200000 / single + 600000 / multi.ms);
  return {
    t: Date.now(),
    cpuSingleMs: single,
    cpuMultiMs: multi.ms,
    cpuThreads: multi.threads,
    diskWriteMBs: d.writeMBs,
    diskReadMBs: d.readMBs,
    score,
  };
}

module.exports = { run };

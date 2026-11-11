'use strict';
// Helper central para ejecutar PowerShell de forma segura desde el proceso principal.
// Usamos -EncodedCommand (UTF-16LE + base64) para evitar por completo los problemas
// de comillas/escape al pasar scripts multilinea desde JavaScript a PowerShell.

const { execFile } = require('child_process');

const PS_EXE = 'powershell.exe';
const BASE_ARGS = ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass'];

/**
 * Ejecuta un script de PowerShell y devuelve su stdout como texto.
 * @param {string} script  Script PowerShell (puede ser multilinea).
 * @param {object} [opts]
 * @param {number} [opts.timeout=120000]
 * @returns {Promise<string>}
 */
function psRun(script, opts = {}) {
  // Forzamos salida en UTF-8 para que los acentos lleguen correctos a Node.
  const prelude = '[Console]::OutputEncoding = [System.Text.Encoding]::UTF8;\n';
  const encoded = Buffer.from(prelude + script, 'utf16le').toString('base64');
  const args = [...BASE_ARGS, '-EncodedCommand', encoded];
  return new Promise((resolve, reject) => {
    execFile(
      PS_EXE,
      args,
      {
        windowsHide: true,
        maxBuffer: 1024 * 1024 * 32,
        timeout: opts.timeout || 120000,
      },
      (err, stdout, stderr) => {
        if (err) {
          const msg = (stderr && stderr.trim()) || err.message || 'Error de PowerShell';
          return reject(new Error(msg));
        }
        resolve(stdout);
      }
    );
  });
}

/**
 * Ejecuta un script que termina produciendo objetos y parsea el JSON resultante.
 * El script debe NO incluir ConvertTo-Json; se adjunta automaticamente.
 * @param {string} script
 * @param {number} [depth=6]
 * @returns {Promise<any>}
 */
async function psJson(script, depth = 6) {
  const full = `${script}\n| ConvertTo-Json -Depth ${depth} -Compress`;
  const out = (await psRun(full)).trim();
  if (!out) return null;
  try {
    return JSON.parse(out);
  } catch (e) {
    throw new Error('No se pudo parsear la salida de PowerShell como JSON: ' + e.message);
  }
}

module.exports = { psRun, psJson };

'use strict';
// Perfiles por juego: se guardan en userData/profiles.json.
// Un perfil = { id, name, exe, tweaks:[ids], gpuMax, suspend:[procNames], autoApply }
// Export/import como código corto "OVD-..." (JSON comprimido en base64url).

const { app } = require('electron');
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');

function file() {
  return path.join(app.getPath('userData'), 'profiles.json');
}

function load() {
  try {
    return JSON.parse(fs.readFileSync(file(), 'utf8'));
  } catch (_) {
    return [];
  }
}

function save(list) {
  try {
    fs.writeFileSync(file(), JSON.stringify(list, null, 2));
  } catch (_) {
    /* noop */
  }
}

function all() {
  return load();
}

function get(id) {
  return load().find((p) => p.id === id) || null;
}

function upsert(profile) {
  const list = load();
  if (!profile.id) {
    profile.id = 'p_' + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
    profile.createdAt = new Date().toISOString();
  }
  const i = list.findIndex((p) => p.id === profile.id);
  if (i >= 0) list[i] = { ...list[i], ...profile };
  else list.push(profile);
  save(list);
  return profile.id;
}

function remove(id) {
  save(load().filter((p) => p.id !== id));
}

function exportCode(id) {
  const p = get(id);
  if (!p) return null;
  const payload = { n: p.name, e: p.exe, t: p.tweaks || [], g: !!p.gpuMax, s: p.suspend || [] };
  const b64 = zlib.deflateRawSync(JSON.stringify(payload)).toString('base64url');
  return 'OVD-' + b64;
}

function importCode(code) {
  if (!code || typeof code !== 'string' || !code.trim().startsWith('OVD-')) {
    throw new Error('El código no es válido (debe empezar por OVD-).');
  }
  const b64 = code.trim().slice(4);
  let payload;
  try {
    payload = JSON.parse(zlib.inflateRawSync(Buffer.from(b64, 'base64url')).toString('utf8'));
  } catch (_) {
    throw new Error('El código está corrupto o incompleto.');
  }
  const profile = {
    name: payload.n || 'Perfil importado',
    exe: payload.e || '',
    tweaks: Array.isArray(payload.t) ? payload.t : [],
    gpuMax: !!payload.g,
    suspend: Array.isArray(payload.s) ? payload.s : [],
    autoApply: false,
  };
  const id = upsert(profile);
  return get(id);
}

module.exports = { all, get, upsert, remove, exportCode, importCode };

// Portable startup assets use the same config hierarchy as native loose files.
// This namespace contains display resources only; stock compatibility is untouched.
import fs from 'node:fs';
import path from 'node:path';

export function collectGraphicsPack(root) {
  const canonicalRoot = fs.realpathSync(root);
  const entries = new Map();
  function add(name, mime) {
    if (typeof name !== 'string' || /[\\:\0]/.test(name)
        || name.split('/').some(p => !p || p === '.' || p === '..')) {
      throw new Error(`Invalid graphics resource path: ${name}`);
    }
    // MPK names are case insensitive. Reject ambiguous authoring instead of
    // producing a pack that behaves differently from native loose files.
    const key = `graphics/${name.toLowerCase()}`;
    if (entries.has(key)) {
      if (entries.get(key).source !== name) throw new Error(`Graphics path case collision: ${name}`);
      return entries.get(key).data;
    }
    const file = fs.realpathSync(path.join(root, name));
    if (!file.startsWith(canonicalRoot + path.sep)) throw new Error(`Graphics symlink escapes config root: ${name}`);
    if (fs.statSync(file).size > 16 * 1024 * 1024) throw new Error(`Graphics file exceeds 16 MiB: ${name}`);
    const data = fs.readFileSync(file);
    entries.set(key, {name: key, data, mime, source: name});
    return data;
  }
  function addPng(resource) {
    const data = add(resource, 'image/png');
    if (data.length < 24 || !data.subarray(0, 8).equals(Buffer.from([137,80,78,71,13,10,26,10]))
        || data.readUInt32BE(16) < 1 || data.readUInt32BE(16) > 4096
        || data.readUInt32BE(20) < 1 || data.readUInt32BE(20) > 4096) {
      throw new Error(`Invalid graphics PNG: ${resource}`);
    }
  }
  const index = JSON.parse(add('graphics-enhancement.json', 'application/json'));
  if (index.schema_version !== 1 || typeof index.enabled !== 'boolean' || !Array.isArray(index.packs)
      || index.packs.length > 16 || new Set(index.packs).size !== index.packs.length) {
    throw new Error('Invalid graphics index');
  }
  for (const name of index.packs) {
    const pack = JSON.parse(add(name, 'application/json'));
    if (![1, 2].includes(pack.schema_version) || typeof pack.id !== 'string') throw new Error(`Invalid graphics pack: ${name}`);
    for (const resource of Object.values(pack.cloth_meshes ?? {})) {
      if (typeof resource !== 'string' || /[\\:\0]/.test(resource)
          || resource.split('/').some(p => !p || p === '.' || p === '..')
          || !resource.toLowerCase().endsWith('.obj')) {
        throw new Error(`Invalid cloth OBJ path: ${resource}`);
      }
      const data = add(path.posix.join(path.posix.dirname(name), resource), 'text/plain');
      if (data.length === 0 || data.length > 1024 * 1024) throw new Error(`Invalid cloth OBJ size: ${resource}`);
    }
    if (pack.water) {
      const relative = resource => {
        const directory = path.posix.dirname(name);
        if (typeof resource !== 'string') throw new Error(`Invalid water path: ${resource}`);
        return directory === '.' ? resource : `${directory}/${resource}`;
      };
      for (const field of ['config', 'deep', 'shallow']) {
        JSON.parse(add(relative(pack.water[field]), 'application/json'));
      }
      addPng(relative(pack.water.ripples));
      if (pack.water.baked != null) {
        const catalog = JSON.parse(add(relative(pack.water.baked), 'application/json'));
        for (const surface of catalog) {
          for (const resource of [surface.reflection, surface.depth, surface.sky_visibility?.file]) {
            if (resource != null) addPng(relative(resource));
          }
        }
      }
    }
    // Desktop replacement geometry is staged by graphics_packaging.py and
    // loaded on stage entry. Do not force it into the mandatory portable MPK.
    for (const resource of Object.values(pack.resources ?? {})) {
      // Validate the original resource path before joining (join normalizes ..).
      if (typeof resource !== 'string' || /[\\:\0]/.test(resource)
          || resource.split('/').some(p => !p || p === '.' || p === '..')) {
        throw new Error(`Invalid graphics resource: ${resource}`);
      }
      addPng(path.posix.join(path.posix.dirname(name), resource));
    }
  }
  return [...entries.values()].map(({source, ...entry}) => entry);
}

import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {collectGraphicsPack} from '../lib/graphicsPack.mjs';
import {encodePack, decodePack} from '../lib/packFormat.mjs';
const root = fileURLToPath(new URL('../../meteor_rust/config/', import.meta.url));

test('graphics resources survive MPK round trip without gameplay namespaces', () => {
  const entries = collectGraphicsPack(root);
  assert.ok(entries.some(e => e.name === 'graphics/graphics-packs/sn04-interactive-snow/pack.json'));
  assert.ok(entries.some(e => e.name === 'graphics/graphics-packs/starter/models/red-cape.obj'));
  for (const name of ['models/p0-split-coat-sim.obj', 'models/p0-split-coat-render.obj', 'textures/p0-split-coat-brocade.png']) {
    assert.ok(entries.some(e => e.name === `graphics/graphics-packs/starter/${name}`), name);
  }
  const prefix = 'graphics/graphics-packs/starter/';
  const starter = JSON.parse(entries.find(entry => entry.name === prefix + 'pack.json').data);
  const wardrobe = starter.scenes.find(scene => scene.stage === '*').character_cloth
    .filter(cloth => cloth.character === 'p1');
  assert.deepEqual(wardrobe.map(cloth => cloth.slot).sort(), ['pink_skirt', 'veil_hat']);
  for (const cloth of wardrobe) {
    for (const id of [cloth.mesh.id, cloth.mesh.render_id, cloth.rigid_mesh?.id].filter(Boolean)) {
      assert.ok(entries.some(entry => entry.name === prefix + starter.cloth_meshes[id]), id);
    }
    for (const id of [cloth.material, cloth.rigid_mesh?.material].filter(Boolean)) {
      const texture = starter.resources[starter.materials[id].base_color];
      assert.ok(entries.some(entry => entry.name === prefix + texture), id);
    }
  }
  const waterPrefix = 'graphics/graphics-packs/water/';
  const byName = new Map(entries.map(entry => [entry.name, entry]));
  const index = JSON.parse(byName.get('graphics/graphics-enhancement.json').data);
  const normalPackPath = 'graphics-packs/scene-normals/pack.json';
  assert.equal(index.packs[0], normalPackPath);
  const normals = JSON.parse(byName.get('graphics/' + normalPackPath).data);
  const waterNormals = new Set(['sn06/sn06w.jpg', 'sn09/sn914.jpg', 'sn13/sn13w01.jpg']);
  const assigned = new Map(normals.scenes.flatMap(scene => scene.surfaces.map(surface =>
    [scene.stage + '/' + surface.texture, surface.material],
  )));
  const normalCount = Object.keys(normals.resources).length;
  assert.ok(normalCount > 0);
  assert.equal(Object.keys(normals.materials).length, normalCount - waterNormals.size);
  assert.equal(assigned.size, normalCount - waterNormals.size);
  for (const [key, resource] of Object.entries(normals.resources)) {
    assert.equal(resource, `textures/${key}.normal.png`);
    const entry = byName.get('graphics/' + path.posix.dirname(normalPackPath) + '/' + resource.toLowerCase());
    assert.equal(entry.mime, 'image/png');
    if (waterNormals.has(key)) {
      assert.equal(assigned.has(key), false);
    } else {
      assert.equal(normals.materials[assigned.get(key)].normal, key);
    }
  }
  const {water} = JSON.parse(byName.get(waterPrefix + 'pack.json').data);
  for (const resource of Object.values(water)) {
    assert.ok(byName.has(waterPrefix + resource.toLowerCase()), resource);
  }
  const catalog = JSON.parse(byName.get(waterPrefix + water.baked).data);
  assert.ok(catalog.length > 0);
  for (const surface of catalog) {
    for (const resource of [surface.reflection, surface.depth, surface.sky_visibility?.file]) {
      if (resource != null) assert.ok(byName.has(waterPrefix + resource.toLowerCase()), resource);
    }
  }
  assert.ok(entries.every(e => e.name.startsWith('graphics/')));
  const pack = decodePack(encodePack({packId:'graphics-test', kind:'startup', entries}));
  for (const entry of entries) assert.deepEqual(Buffer.from(pack.get(entry.name)), entry.data);
});
test('graphics collector rejects traversal', () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'meteor-graphics-'));
  try {
    fs.writeFileSync(path.join(temp, 'graphics-enhancement.json'), JSON.stringify({schema_version:1,enabled:true,packs:['../pack.json']}));
    assert.throws(() => collectGraphicsPack(temp), /Invalid graphics resource path/);
  } finally { fs.rmSync(temp, {recursive:true,force:true}); }
});
test('graphics collector rejects escaping symlinks', {skip:process.platform === 'win32'}, () => {
  // Windows symlink creation needs privileges not required by the game.
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'meteor-graphics-'));
  try {
    fs.writeFileSync(path.join(temp, 'graphics-enhancement.json'), JSON.stringify({schema_version:1,enabled:true,packs:['pack.json']}));
    fs.symlinkSync(path.join(root, 'graphics-packs/starter/pack.json'), path.join(temp, 'pack.json'));
    assert.throws(() => collectGraphicsPack(temp), /escapes config root/);
  } finally { fs.rmSync(temp, {recursive:true,force:true}); }
});

test('v2 manifests can share portable PNGs without putting desktop models into startup', () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'meteor-meshlet-pack-'));
  try {
    fs.writeFileSync(path.join(temp, 'graphics-enhancement.json'), JSON.stringify({
      schema_version:1, enabled:true, packs:['pack.json'],
    }));
    fs.copyFileSync(path.join(root, 'graphics-packs/meshlet-demo/pack.json'), path.join(temp, 'pack.json'));
    const entries = collectGraphicsPack(temp);
    assert.deepEqual(entries.map(e => e.name), ['graphics/graphics-enhancement.json', 'graphics/pack.json']);
  } finally { fs.rmSync(temp, {recursive:true,force:true}); }
});

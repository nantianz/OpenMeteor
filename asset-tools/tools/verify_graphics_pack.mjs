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

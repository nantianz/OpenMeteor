// MPK1 — Meteor 资源包容器格式。
//
// 为什么不用 ZIP：原版 .pak 就是 ZIP，但 ZIP 逐条压缩，拿不到跨文件的冗余收益。
// 实测 20 个 P*.POS 逐条 brotli 相加 528 KB，整体压缩只要 27 KB（19x 差距），
// 因为各角色的姿态表内容几乎相同。所以容器本身不压缩，交给 HTTP 的
// Content-Encoding: br 整体压——这样跨文件冗余才能被压缩器看到。
//
// 布局（小端）：
//   0                magic "MPK1"        4 字节
//   4                manifestLen         4 字节 uint32
//   8                manifest JSON UTF-8 manifestLen 字节
//   8+manifestLen    blob 区             各条目原始字节按 manifest 顺序拼接
//
// 包的身份是整个容器字节的 sha256，manifest 内部不含自身哈希（避免循环依赖）。
// 校验方式：取 fetch 到的完整字节算 sha256，与索引里声明的值比对。

export const PACK_MAGIC = "MPK1";
export const PACK_FORMAT_VERSION = 1;

const MAGIC_BYTES = new Uint8Array([0x4d, 0x50, 0x4b, 0x31]); // "MPK1"
const HEADER_SIZE = 8;
const MAX_MANIFEST_BYTES = 32 * 1024 * 1024;
const MAX_ENTRY_COUNT = 100000;
const PACK_KINDS = new Set([
  "startup", "common", "textures", "skeleton", "character", "stage", "sound", "music",
]);
// 包内条目只会交给现有的数据解析器或浏览器媒体解码器。这里使用允许列表，
// 而不是只拦 `.js`：否则 `.html` / `.wasm` / 新的脚本扩展名很容易漏网。
const DATA_ENTRY_EXTENSIONS = new Set([
  ".act", ".amb", ".bmp", ".bnc", ".cob", ".dds", ".des", ".ef",
  ".fmc", ".gif", ".gmb", ".gmc", ".h", ".ifl", ".ion", ".jpeg",
  ".jpg", ".json", ".lst", ".mp3", ".ogg", ".png", ".pos", ".pst",
  ".ttf", ".res", ".skc", ".tga", ".wav", ".webp", ".wp",
]);
const SAFE_MIME_TYPES = new Set([
  "application/json",
  "application/octet-stream",
  "audio/mpeg",
  "audio/ogg",
  "audio/wav",
  "image/bmp",
  "image/gif",
  "image/jpeg",
  "image/png",
  "image/webp",
  "text/plain",
]);
const DANGEROUS_OBJECT_KEYS = new Set(["__proto__", "constructor", "prototype"]);

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder("utf-8");

function toUint8(data) {
  // Node Buffer 是 Uint8Array 子类，但它的 `.slice()` 是零拷贝视图；若原样
  // 保留，调用方为 DataView 复制条目时可能意外拿到整个 MPK 缓冲。统一重建为
  // 原生 Uint8Array 视图，浏览器与 Node 的条目语义才一致。
  if (data instanceof Uint8Array) {
    return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  }
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) {
    return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  }
  throw new TypeError("Pack entry data must be a Uint8Array/ArrayBuffer");
}

// 条目名统一小写并把反斜杠换成正斜杠。原版资源大小写混用
// （SFX 里同时有 pos305.ef 和 Pos289.ef），查表必须大小写无关。
export function normalizeEntryName(name) {
  return String(name || "").replace(/\\/g, "/").trim().toLowerCase();
}

function assertPlainData(value, label, depth = 0) {
  if (depth > 64) throw new Error(`${label}: data nesting is too deep`);
  if (
    value == null
    || typeof value === "string"
    || typeof value === "boolean"
    || (typeof value === "number" && Number.isFinite(value))
  ) {
    return;
  }
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i += 1) {
      assertPlainData(value[i], `${label}[${i}]`, depth + 1);
    }
    return;
  }
  if (typeof value !== "object") {
    throw new Error(`${label}: only JSON data is allowed`);
  }
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) {
    throw new Error(`${label}: only plain JSON objects are allowed`);
  }
  for (const key of Object.keys(value)) {
    if (DANGEROUS_OBJECT_KEYS.has(key)) {
      throw new Error(`${label}: unsafe object key ${key}`);
    }
    assertPlainData(value[key], `${label}.${key}`, depth + 1);
  }
}

function assertPackId(packId) {
  if (
    typeof packId !== "string"
    || !/^[a-z0-9][a-z0-9._/-]{0,127}$/i.test(packId)
    || packId.startsWith("/")
    || packId.endsWith("/")
    || packId.split("/").some((part) => part === "." || part === "..")
  ) {
    throw new Error(`invalid packId: ${String(packId)}`);
  }
}

/**
 * 包条目名的安全边界。条目永远只在内存中查表，不落盘；仍禁止绝对路径、
 * `..` 和非数据扩展名，确保以后新增 loader 时也不能把包变成代码载体。
 */
export function assertSafePackEntryName(name) {
  const normalized = normalizeEntryName(name);
  const parts = normalized.split("/");
  if (
    !normalized
    || normalized.length > 512
    || normalized.startsWith("/")
    || normalized.includes("\0")
    || parts.some((part) => !part || part === "." || part === "..")
  ) {
    throw new Error(`unsafe pack entry path: ${String(name)}`);
  }
  const slash = normalized.lastIndexOf("/");
  const base = slash >= 0 ? normalized.slice(slash + 1) : normalized;
  const dot = base.lastIndexOf(".");
  const extension = dot >= 0 ? base.slice(dot) : "";
  const clothObj = extension === ".obj" && normalized.startsWith("graphics/");
  const stageText = /^stage\/sn\d{2}\.(?:name\.txt|description\.(?:txt|md))$/.test(normalized);
  if (!DATA_ENTRY_EXTENSIONS.has(extension) && !clothObj && !stageText) {
    throw new Error(`pack entry is not an allowed data file: ${normalized}`);
  }
  return normalized;
}

/**
 * 校验 MPK manifest。encode/decode 共用同一条规则，所以“只允许数据”不是
 * CI 约定，而是客户端实际启用包之前必经的运行时检查。
 */
export function validatePackManifest(
  manifest,
  { blobByteLength = null } = {},
) {
  if (!manifest || typeof manifest !== "object" || Array.isArray(manifest)) {
    throw new Error("pack manifest must be an object");
  }
  if (manifest.formatVersion !== PACK_FORMAT_VERSION) {
    throw new Error(
      `unsupported formatVersion ${manifest.formatVersion} (expected ${PACK_FORMAT_VERSION})`,
    );
  }
  assertPackId(manifest.packId);
  if (!PACK_KINDS.has(manifest.kind)) {
    throw new Error(`unsupported pack kind: ${String(manifest.kind)}`);
  }
  if (!Array.isArray(manifest.requires) || !Array.isArray(manifest.covers)) {
    throw new Error("pack requires/covers must be arrays");
  }
  const required = new Set();
  for (const dependency of manifest.requires) {
    assertPackId(dependency);
    if (dependency === manifest.packId) {
      throw new Error(`pack ${manifest.packId} cannot depend on itself`);
    }
    if (required.has(dependency)) {
      throw new Error(`duplicate pack dependency: ${dependency}`);
    }
    required.add(dependency);
  }
  for (const prefix of manifest.covers) {
    const normalized = normalizeEntryName(prefix);
    if (
      typeof prefix !== "string"
      || !normalized
      || prefix !== normalized
      || normalized.startsWith("/")
      || normalized.includes("\0")
      || normalized.split("/").some((part) => part === "." || part === "..")
    ) {
      throw new Error(`unsafe pack cover prefix: ${String(prefix)}`);
    }
  }
  if (!Array.isArray(manifest.entries) || manifest.entries.length > MAX_ENTRY_COUNT) {
    throw new Error(`pack entries must be an array of at most ${MAX_ENTRY_COUNT} items`);
  }

  const seen = new Set();
  const ranges = [];
  for (const record of manifest.entries) {
    if (!record || typeof record !== "object" || Array.isArray(record)) {
      throw new Error("pack entry record must be an object");
    }
    const normalized = assertSafePackEntryName(record.name);
    if (record.name !== normalized) {
      throw new Error(`pack entry name is not normalized: ${String(record.name)}`);
    }
    if (seen.has(normalized)) throw new Error(`duplicate entry: ${normalized}`);
    seen.add(normalized);
    if (
      !Number.isSafeInteger(record.off)
      || record.off < 0
      || !Number.isSafeInteger(record.len)
      || record.len < 0
    ) {
      throw new Error(`invalid byte range for ${normalized}`);
    }
    if (record.mime != null && !SAFE_MIME_TYPES.has(record.mime)) {
      throw new Error(`unsafe MIME type for ${normalized}: ${String(record.mime)}`);
    }
    if (
      record.sourceSize != null
      && (!Number.isSafeInteger(record.sourceSize) || record.sourceSize < 0)
    ) {
      throw new Error(`invalid sourceSize for ${normalized}`);
    }
    const end = record.off + record.len;
    if (!Number.isSafeInteger(end)) throw new Error(`byte range overflow for ${normalized}`);
    if (blobByteLength != null && end > blobByteLength) {
      throw new Error(`entry ${normalized} out of range`);
    }
    ranges.push({ start: record.off, end, name: normalized });
  }
  ranges.sort((a, b) => a.start - b.start || a.end - b.end);
  let previousEnd = 0;
  for (const range of ranges) {
    if (range.start < previousEnd && range.end > range.start) {
      throw new Error(`overlapping byte range for ${range.name}`);
    }
    previousEnd = Math.max(previousEnd, range.end);
  }

  if (manifest.authority != null) assertPlainData(manifest.authority, "authority");
  if (manifest.meta != null) assertPlainData(manifest.meta, "meta");
  return manifest;
}

/**
 * 编码一个资源包。
 * @param {object} spec
 * @param {string} spec.packId      包标识，如 "stage/sn01"
 * @param {string} spec.kind        包类别：common | textures | skeleton | character | stage | sound
 * @param {string[]} [spec.requires] 依赖的 packId 列表
 * @param {string[]} [spec.covers]  本包"权威覆盖"的条目名前缀。落在这些前缀下
 *   却不在包里的条目，视为该资源本来就不存在，客户端直接跳过 HTTP 请求。
 *   原版有大量可选伴生文件（.fmc 刚体动画、.pos 姿态），逐个探测会打出成片
 *   404；有了这个声明才能把这些空转请求彻底去掉。
 * @param {Array<{name:string,data:Uint8Array,mime?:string,sourceSize?:number}>} spec.entries
 * @param {object} [spec.authority] 权威侧判定数据（招式表 / 场景碰撞），随包分发
 * @param {object} [spec.meta]      附加元数据
 * @returns {Uint8Array}
 */
export function encodePack(spec) {
  const {
    packId, kind, requires = [], covers = [], entries = [],
    authority = null, meta = null,
  } = spec;
  if (!packId) throw new Error("encodePack: packId is required");
  if (!kind) throw new Error("encodePack: kind is required");

  const records = [];
  const blobs = [];
  let offset = 0;
  const seen = new Set();
  for (const entry of entries) {
    const name = assertSafePackEntryName(entry.name);
    if (!name) throw new Error(`encodePack: empty entry name in ${packId}`);
    if (seen.has(name)) throw new Error(`encodePack: duplicate entry ${name} in ${packId}`);
    seen.add(name);
    const data = toUint8(entry.data);
    const record = { name, off: offset, len: data.byteLength };
    if (entry.mime) record.mime = entry.mime;
    if (Number.isSafeInteger(entry.sourceSize) && entry.sourceSize >= 0) {
      record.sourceSize = entry.sourceSize;
    }
    records.push(record);
    blobs.push(data);
    offset += data.byteLength;
  }

  const manifest = {
    formatVersion: PACK_FORMAT_VERSION,
    packId,
    kind,
    requires,
    covers: covers.map((p) => normalizeEntryName(p)),
    entries: records,
  };
  if (authority) manifest.authority = authority;
  if (meta) manifest.meta = meta;
  validatePackManifest(manifest, { blobByteLength: offset });

  const manifestBytes = textEncoder.encode(JSON.stringify(manifest));
  if (manifestBytes.byteLength > MAX_MANIFEST_BYTES) {
    throw new Error(`encodePack: manifest exceeds ${MAX_MANIFEST_BYTES} bytes`);
  }
  const total = HEADER_SIZE + manifestBytes.byteLength + offset;
  const out = new Uint8Array(total);
  out.set(MAGIC_BYTES, 0);
  new DataView(out.buffer).setUint32(4, manifestBytes.byteLength, true);
  out.set(manifestBytes, HEADER_SIZE);
  let cursor = HEADER_SIZE + manifestBytes.byteLength;
  for (const blob of blobs) {
    out.set(blob, cursor);
    cursor += blob.byteLength;
  }
  return out;
}

/**
 * 解码一个资源包。返回的 get() 是零拷贝子视图，不复制 blob 区。
 * @param {ArrayBuffer|Uint8Array} buffer
 * @returns {{manifest:object, packId:string, kind:string, has:(n:string)=>boolean,
 *            get:(n:string)=>Uint8Array|null, names:()=>string[], byteLength:number}}
 */
export function decodePack(buffer) {
  const bytes = toUint8(buffer);
  if (bytes.byteLength < HEADER_SIZE) throw new Error("decodePack: buffer too small");
  for (let i = 0; i < MAGIC_BYTES.length; i += 1) {
    if (bytes[i] !== MAGIC_BYTES[i]) throw new Error("decodePack: bad magic (not an MPK1 pack)");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const manifestLen = view.getUint32(4, true);
  if (manifestLen > MAX_MANIFEST_BYTES) throw new Error("decodePack: manifest too large");
  const manifestEnd = HEADER_SIZE + manifestLen;
  if (manifestEnd > bytes.byteLength) throw new Error("decodePack: truncated manifest");

  const manifest = JSON.parse(textDecoder.decode(bytes.subarray(HEADER_SIZE, manifestEnd)));
  validatePackManifest(manifest, { blobByteLength: bytes.byteLength - manifestEnd });

  const index = new Map();
  for (const record of manifest.entries) {
    index.set(record.name, record);
  }

  const blobBase = manifestEnd;
  const get = (name) => {
    const record = index.get(normalizeEntryName(name));
    if (!record) return null;
    const start = blobBase + record.off;
    const end = start + record.len;
    return bytes.subarray(start, end);
  };

  return {
    manifest,
    packId: manifest.packId,
    kind: manifest.kind,
    requires: manifest.requires || [],
    covers: manifest.covers || [],
    authority: manifest.authority || null,
    byteLength: bytes.byteLength,
    has: (name) => index.has(normalizeEntryName(name)),
    get,
    entry: (name) => index.get(normalizeEntryName(name)) || null,
    names: () => [...index.keys()],
  };
}

// sha256 内容寻址。crypto.subtle 在浏览器和 Node 18+ 都是全局可用的，
// 所以打包工具、服务端和客户端算出来的是同一个值。
export async function packHash(buffer) {
  const bytes = toUint8(buffer);
  // subtle.digest 要求独立的 ArrayBuffer 视图，子视图需要复制一份再送进去。
  const source = bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength
    ? bytes.buffer
    : bytes.slice().buffer;
  const digest = await crypto.subtle.digest("SHA-256", source);
  const hex = [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
  return `sha256:${hex}`;
}

// 索引与文件名里用的短哈希；碰撞概率对本工程的规模足够低，且便于阅读。
export function shortHash(hash) {
  return String(hash || "").replace(/^sha256:/, "").slice(0, 12);
}

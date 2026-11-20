import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import struct
import zipfile

from windows_font import verify_font


ROOT = Path(__file__).resolve().parents[2]
UI_IMAGES = (
    "studio-logo.png",
    "cinematic-title-reborn-matte.png",
    "cinematic-fortress.png",
    "campaign-map.png",
    "campaign-halo.png",
)
INSTRUCTIONS = """OpenMeteor Windows

Extract the complete ZIP into a compatible original game directory, keeping
config/ and fonts/ beside OpenMeteor.exe, then double-click OpenMeteor.exe.
No command-line arguments, launcher script, or MPK packs are required.
This package does not include original game resources.
For shortcuts, set the working directory to the original game directory.
The Chinese font is included as fonts/ui.ttf, with its license in licenses/.
The startup logo, menu title and background, campaign map, and halo are embedded
in OpenMeteor.exe. No separate UI image download is needed.
See GRAPHICS-README.md for stage wind, interactive grass, day-night modes, P0 and P1 clothing, and water settings.
The package does not enable optional DLSS or ANGLE backends.

将 ZIP 完整解压到原版游戏目录，保留 config/ 和 fonts/ 的目录结构，
直接双击 OpenMeteor.exe，无需启动参数、启动脚本或 MPK 资源包。
本包不包含原版游戏资源；原版程序和资源文件可以继续保留。
使用快捷方式时，将“起始位置”设为原版游戏目录。
包内已附带 fonts/ui.ttf 中文字体及 licenses/ 中的字体许可。
启动 Logo、主界面标题与背景、战役地图和光晕已内置于 OpenMeteor.exe，
不需要另行下载界面图片。
随包附带关卡风、交互草地、昼夜时钟、P0 和 P1 布料以及深浅水的配置，请查阅 GRAPHICS-README.md。
"""


def verify_embedded_ui(binary):
    data = binary.read_bytes()
    for name in UI_IMAGES:
        path = ROOT / "meteor_rust/crates/meteor-app/assets/ui" / name
        if not path.is_file() or path.is_symlink():
            raise ValueError(f"missing or invalid UI image: {name}")
        image = path.read_bytes()
        if (len(image) < 33 or image[:8] != b"\x89PNG\r\n\x1a\n"
                or image[12:16] != b"IHDR"
                or min(struct.unpack_from(">II", image, 16)) <= 1):
            raise ValueError(f"invalid or placeholder UI image: {name}")
        if image not in data:
            raise ValueError(f"UI image is not embedded in the executable: {name}")


def graphics_files():
    config = (ROOT / "meteor_rust/config").resolve()
    files = {"GRAPHICS-README.md": ROOT / "docs/guides/graphics-enhancement-packs.md"}

    def add(name, parent=PurePosixPath()):
        if (not isinstance(name, str) or any(c in name for c in "\\:\0")
                or any(part in ("", ".", "..") for part in name.split("/"))):
            raise ValueError(f"invalid graphics resource path: {name}")
        relative = parent / name
        path = config / relative
        if not path.resolve(strict=True).is_relative_to(config):
            raise ValueError(f"graphics resource escapes config directory: {relative}")
        files["config/" + relative.as_posix()] = path
        return path

    index = json.loads(add("graphics-enhancement.json").read_text(encoding="utf-8"))
    required = {
        "graphics-packs/sn06-moss-valley/pack.json",
        "graphics-packs/sn06-nextgen-sample/pack.json",
        "graphics-packs/longsword/pack.json",
        "graphics-packs/sn06-color-pipeline/pack.json",
    }
    if index.get("enabled") is not True or not required.issubset(index.get("packs", [])):
        raise ValueError("default graphics enhancements are disabled or incomplete")
    for preset in ("water-deep.json", "water-shallow.json"):
        json.loads(add(preset).read_text(encoding="utf-8"))
    demo = "graphics-packs/meshlet-demo/pack.json"
    optional_packs = []
    for preset in ("graphics-enhancement-sn03.json", "graphics-enhancement-sn06.json",
                   "graphics-enhancement-sn06-nextgen.json", "graphics-enhancement-sn06-color-pipeline.json",
                   "graphics-enhancement-longsword-preview.json", "graphics-enhancement-sn06-waterfall-experiment.json"):
        optional_packs.extend(json.loads(add(preset).read_text(encoding="utf-8"))["packs"])
    for name in dict.fromkeys([*index["packs"], demo, *optional_packs]):
        manifest = json.loads(add(name).read_text(encoding="utf-8"))
        parent = PurePosixPath(name).parent
        for resource in manifest.get("cloth_meshes", {}).values():
            source = add(resource, parent)
            if source.suffix.lower() != ".obj" or not 0 < source.stat().st_size <= 1024 * 1024:
                raise ValueError(f"invalid cloth OBJ file: {resource}")
        for resource in manifest.get("resources", {}).values():
            add(resource, parent)
        for model in manifest.get("meshlet_models", {}).values():
            source = add(model["file"], parent)
            if source.suffix != ".mlmesh" or not 44 <= source.stat().st_size <= 16 * 1024 * 1024:
                raise ValueError(f"invalid Meshlet file: {model['file']}")
            payload = source.read_bytes()
            if payload[:8] != b"MLMESH01" or payload[8:40] != hashlib.sha256(payload[40:]).digest():
                raise ValueError(f"invalid Meshlet version or checksum: {model['file']}")
    return files


def package(binary, architecture, version, commit, font, output):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,79}", version):
        raise ValueError("unsupported version")
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("invalid source commit")
    verify_font(font)
    machine, magic = {"x64": (0x8664, 0x20B), "x86": (0x14C, 0x10B)}[architecture]
    with binary.open("rb") as stream:
        if stream.read(2) != b"MZ":
            raise ValueError("not a Windows executable")
        stream.seek(0x3C)
        stream.seek(struct.unpack("<I", stream.read(4))[0])
        header = stream.read(26)
    if (len(header) != 26 or header[:4] != b"PE\0\0"
            or struct.unpack_from("<H", header, 4)[0] != machine
            or struct.unpack_from("<H", header, 24)[0] != magic):
        raise ValueError("executable architecture mismatch")
    verify_embedded_ui(binary)
    files = {
        "OpenMeteor.exe": binary,
        "LICENSE": ROOT / "LICENSE",
        "THIRD_PARTY_NOTICES.md": ROOT / "THIRD_PARTY_NOTICES.md",
        "fonts/ui.ttf": font,
    }
    for name in ("balance-rules.json", "water-enhancement.json"):
        path = ROOT / "meteor_rust/config" / name
        json.loads(path.read_text(encoding="utf-8"))
        files["config/" + name] = path
    files.update(graphics_files())
    for path in sorted((ROOT / "licenses").rglob("*")):
        if path.is_file():
            files[path.relative_to(ROOT).as_posix()] = path
    for path in sorted((ROOT / "meteor_rust/vendor").glob("*/LICENSE*")):
        if path.is_file():
            files[f"licenses/{path.parent.name}/{path.name}"] = path
    for path in files.values():
        if not path.is_file() or path.is_symlink() or not path.stat().st_size:
            raise ValueError("missing or invalid package input")
    output.mkdir(parents=True, exist_ok=True)
    archive_path = output / f"OpenMeteor-{version}-windows-{architecture}.zip"
    metadata = {"version": version, "commit": commit, "architecture": architecture}
    with zipfile.ZipFile(archive_path, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for name, path in files.items():
            archive.write(path, name)
        archive.writestr("README.txt", INSTRUCTIONS.replace("\n", "\r\n"))
        archive.writestr("version.json", json.dumps(metadata, indent=2) + "\n")
    with zipfile.ZipFile(archive_path) as archive:
        if archive.testzip() is not None:
            raise ValueError("ZIP integrity check failed")
        if set(archive.namelist()) != set(files) | {"README.txt", "version.json"}:
            raise ValueError("unexpected ZIP contents")
    digest = hashlib.sha256(archive_path.read_bytes()).hexdigest()
    archive_path.with_suffix(".zip.sha256").write_text(
        f"{digest}  {archive_path.name}\n", encoding="utf-8")
    print(archive_path.name)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--architecture", required=True, choices=("x64", "x86"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--font", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    package(args.binary, args.architecture, args.version, args.commit, args.font, args.output)

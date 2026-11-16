import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import zipfile

from windows_font import verify_font


ROOT = Path(__file__).resolve().parents[2]
INSTRUCTIONS = """OpenMeteor Windows

Extract the complete ZIP into a compatible original game directory, keeping
config/ and fonts/ beside OpenMeteor.exe, then double-click OpenMeteor.exe.
No command-line arguments, launcher script, or MPK packs are required.
This package does not include original game resources.
For shortcuts, set the working directory to the original game directory.
The Chinese font is included as fonts/ui.ttf, with its license in licenses/.
The package does not enable optional DLSS or ANGLE backends.

将 ZIP 完整解压到原版游戏目录，保留 config/ 和 fonts/ 的目录结构，
直接双击 OpenMeteor.exe，无需启动参数、启动脚本或 MPK 资源包。
本包不包含原版游戏资源；原版程序和资源文件可以继续保留。
使用快捷方式时，将“起始位置”设为原版游戏目录。
包内已附带 fonts/ui.ttf 中文字体及 licenses/ 中的字体许可。
"""


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

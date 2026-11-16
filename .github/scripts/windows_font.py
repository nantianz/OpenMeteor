import hashlib
from pathlib import Path
import sys
import urllib.request


FONT_URL = "https://raw.githubusercontent.com/google/fonts/a85815a42757630ce188fdad368c2dfc444d4773/ofl/notosanssc/NotoSansSC%5Bwght%5D.ttf"
FONT_SHA256 = "a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da"


def verify_font(path):
    if not path.is_file() or path.is_symlink():
        raise ValueError("missing or invalid Chinese font")
    if hashlib.sha256(path.read_bytes()).hexdigest() != FONT_SHA256:
        raise ValueError("Chinese font checksum mismatch")


def download_font(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(FONT_URL, timeout=60) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != FONT_SHA256:
        raise ValueError("Chinese font download checksum mismatch")
    path.write_bytes(data)
    verify_font(path)
    print("Chinese font verified.")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: windows_font.py <output-font>")
    download_font(Path(sys.argv[1]))

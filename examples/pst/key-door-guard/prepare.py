"""Build an isolated original-directory installation of the PST example."""

import argparse
from pathlib import Path
import re
import shutil
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="Original asset directory")
    parser.add_argument("output", type=Path, help="New directory for the demo")
    args = parser.parse_args()
    source = args.source.resolve()
    output = args.output.resolve()
    sample = Path(__file__).resolve().parent
    if output.is_relative_to(source) or output.exists():
        parser.error("Choose a new output directory outside the original assets.")

    descriptor = source / "sn01" / "sn01.des"
    original = descriptor.read_bytes()
    for required in ("Meteor.res", "sn01/sn01.gmb", "Level/sn01.wp",
                     "cmodel/IPItem01.gmb", "cmodel/Pdoor02.gmb",
                     "cmodel/w0_0.GMC", "music/s8.mp3"):
        if not (source / required).is_file():
            parser.error(f"Missing demo asset: {required}")
    if b"Object D_DemoKey" in original or b"Object D_DemoDoor" in original:
        parser.error("The source descriptor already contains demo objects.")
    objects = (sample / "props.des").read_bytes().split(b"\n", 1)[1]
    count = len(re.findall(rb"^Object ", objects, re.MULTILINE))
    patched, replaced = re.subn(
        rb"(SceneObjects\s+\d+\s+DummeyObjects\s+)(\d+)",
        lambda match: match[1] + str(int(match[2]) + count).encode("ascii"),
        original,
        count=1,
    )
    if replaced != 1 or count != 2:
        parser.error("Unexpected DES header or demo object count.")
    newline = b"\r\n" if b"\r\n" in original else b"\n"
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".pst-demo-", dir=output.parent) as temporary:
        installation = Path(temporary) / "installation"
        shutil.copytree(source, installation)
        (installation / "sn01/sn01.des").write_bytes(
            patched.rstrip(b"\r\n") + newline + objects.replace(b"\n", newline)
        )
        for src, dst in (
            ("scene.pst", "sn01/sn01.pst"),
            ("mission.pst", "Level/sn01_.pst"),
            ("guard.pst", "Level/pst_demo_guard.pst"),
        ):
            shutil.copyfile(sample / src, installation / dst)
        if output.exists():
            parser.error("The demo output directory was created while copying assets.")
        installation.rename(output)
    print(output)


if __name__ == "__main__":
    main()

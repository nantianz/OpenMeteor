"""Build an original-directory installation of the PST entertainment menu."""

import argparse
from array import array
import math
from pathlib import Path
import shutil
import sys
import tempfile
import wave


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
    for required in ("Meteor.res", "sn01/sn01.gmb", "Level/sn01.wp"):
        if not (source / required).is_file():
            parser.error(f"Missing demo asset: {required}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".pst-menu-", dir=output.parent) as directory:
        installation = Path(directory) / "installation"
        shutil.copytree(source, installation)
        startup = installation / "PScriptLib/Startup"
        startup.mkdir(parents=True, exist_ok=True)
        index = startup / "index.txt"
        prior = index.read_text(encoding="utf-8") if index.exists() else ""
        index.write_text(prior.rstrip() + "\nmenu.pst\n", encoding="utf-8")
        pictures = installation / "Image/PstUi"
        pictures.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(sample / "menu.pst", startup / "menu.pst")
        shutil.copyfile(sample / "mission.pst", installation / "Level/sn01_.pst")
        shutil.copyfile(sample.parent / "key-door-guard/guard.pst", installation / "Level/pst_demo_guard.pst")
        repo = sample.parents[2]
        shutil.copyfile(repo / "meteor_rust/icons/meteor-512.png", pictures / "menu.png")
        with wave.open(str(pictures / "hint.wav"), "wb") as audio:
            sample_rate = 24000
            count = sample_rate // 3
            frames = array("h", (int(3000 * (1 - i / count) * math.sin(2 * math.pi * 440 * i / sample_rate))
                                 for i in range(count)))
            if sys.byteorder != "little":
                frames.byteswap()
            audio.setparams((1, 2, sample_rate, count, "NONE", "not compressed"))
            audio.writeframes(frames.tobytes())
        installation.rename(output)
    print(output)


if __name__ == "__main__":
    main()

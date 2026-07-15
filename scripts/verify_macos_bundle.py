#!/usr/bin/env python3
"""Verify that a FreeWheeling bundle is self-contained and distributable."""

import argparse
import pathlib
import plistlib
import shutil
import subprocess
import sys
import tempfile

SYSTEM_PREFIXES = ("/System/Library/", "/usr/lib/")
VERA_MARKERS = ("Bitstream Vera", "Permission is hereby granted", "Font Software")


def run(*command: str) -> str:
    result = subprocess.run(command, text=True, capture_output=True)
    if result.returncode:
        raise ValueError(f"{' '.join(command)} failed: {result.stderr.strip()}")
    return result.stdout


def linked_libraries(binary: pathlib.Path) -> list[str]:
    lines = run("otool", "-L", str(binary)).splitlines()[1:]
    return [line.strip().split(" (compatibility", 1)[0] for line in lines if line.strip()]


def verify_macho(binary: pathlib.Path, frameworks: pathlib.Path) -> None:
    architectures = run("lipo", "-archs", str(binary)).split()
    if architectures != ["arm64"]:
        raise ValueError(f"Mach-O must be arm64-only: {binary} ({' '.join(architectures)})")
    for dependency in linked_libraries(binary):
        if dependency.startswith(SYSTEM_PREFIXES):
            continue
        if dependency.startswith("@rpath/"):
            bundled = frameworks / dependency.removeprefix("@rpath/")
        elif dependency.startswith("@loader_path/../Frameworks/"):
            bundled = frameworks / dependency.rsplit("/", 1)[-1]
        else:
            raise ValueError(f"unbundled or non-relocatable dependency in {binary}: {dependency}")
        if not bundled.is_file():
            raise ValueError(f"referenced bundled dependency is missing: {bundled}")


def verify_signature(bundle: pathlib.Path, contents: pathlib.Path, executable: pathlib.Path,
                     plist: dict) -> None:
    """Verify a complete app seal, or the code seal of a minimal test fixture."""
    code_resources = contents / "_CodeSignature" / "CodeResources"
    if plist.get("CFBundlePackageType") == "APPL":
        if not code_resources.is_file():
            raise ValueError(f"signed application has no resource seal: {code_resources}")
        run("codesign", "--verify", "--deep", "--strict", str(bundle))
    else:
        # Unit-test fixtures intentionally contain only the fields exercised by
        # this verifier.  They are not distributable APPL bundles, but their
        # copied Mach-O executable must still retain a valid code signature.
        # Verify an identical copy outside the .app path so codesign does not
        # incorrectly interpret the fixture directory as its resource envelope.
        with tempfile.TemporaryDirectory() as temporary:
            standalone = pathlib.Path(temporary) / executable.name
            shutil.copy2(executable, standalone)
            run("codesign", "--verify", "--strict", str(standalone))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("bundle", type=pathlib.Path)
    args = parser.parse_args()
    try:
        bundle = args.bundle.resolve()
        contents = bundle / "Contents"
        plist_path = contents / "Info.plist"
        with plist_path.open("rb") as source:
            plist = plistlib.load(source)
        executable_name = plist.get("CFBundleExecutable")
        if not executable_name:
            raise ValueError("Info.plist has no CFBundleExecutable")
        executable = contents / "MacOS" / executable_name
        resources = contents / "Resources"
        required = [
            executable,
            resources / "data/fweelin.xml",
            resources / "data/Vera.ttf",
            resources / "data/VeraBd.ttf",
            resources / "data/basic.sf2",
            resources / "licenses/COPYING",
            resources / "licenses/Bitstream-Vera-NOTICE.txt",
        ]
        missing = [str(path) for path in required if not path.is_file()]
        if missing:
            raise ValueError("missing required bundle files: " + ", ".join(missing))
        usage = plist.get("NSMicrophoneUsageDescription", "")
        if not isinstance(usage, str) or not usage.strip():
            raise ValueError("Info.plist has no microphone usage text")
        document_types = plist.get("CFBundleDocumentTypes", [])
        if not document_types:
            raise ValueError("Info.plist has no Finder document declarations")
        vera_notice = (resources / "licenses/Bitstream-Vera-NOTICE.txt").read_text()
        if not all(marker in vera_notice for marker in VERA_MARKERS):
            raise ValueError("Bitstream Vera notice is incomplete")
        if sys.platform == "darwin":
            frameworks = contents / "Frameworks"
            verify_macho(executable, frameworks)
            for dylib in frameworks.glob("*.dylib"):
                verify_macho(dylib, frameworks)
            verify_signature(bundle, contents, executable, plist)

        # Keep redistribution authorization separate and last: an unlicensed
        # SoundFont must not conceal failures in architecture, dependencies,
        # resources, plist metadata, or signing.
        sf2_license_path = resources / "licenses/basic.sf2-LICENSE.txt"
        if not sf2_license_path.is_file():
            raise ValueError(
                "basic.sf2 is the sole distribution blocker: reviewed license evidence is missing"
            )
        sf2_license = sf2_license_path.read_text().strip()
        if len(sf2_license) < 40:
            raise ValueError(
                "basic.sf2 is the sole distribution blocker: reviewed license evidence is inadequate"
            )
        print(f"bundle verified: {bundle}")
        return 0
    except (OSError, ValueError, plistlib.InvalidFileException) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

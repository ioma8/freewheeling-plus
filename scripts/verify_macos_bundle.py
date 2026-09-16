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
    # Universal binaries print an additional "(architecture <name>):" header
    # before each slice. It is not a linked library.
    return [
        line.strip().split(" (compatibility", 1)[0]
        for line in lines
        if line.strip() and " (architecture " not in line
    ]


def verify_macho(binary: pathlib.Path, frameworks: pathlib.Path, expected_architectures: set[str]) -> None:
    architectures = run("lipo", "-archs", str(binary)).split()
    if set(architectures) != expected_architectures:
        raise ValueError(
            f"Mach-O architectures must be {' '.join(sorted(expected_architectures))}: "
            f"{binary} ({' '.join(architectures)})"
        )
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
                     plist: dict, fixture: bool) -> None:
    """Verify a complete app seal, or the code seal of a minimal test fixture.

    `fixture` is an explicit command-line choice: deriving it from the bundle's
    own `CFBundlePackageType` let a tampered bundle downgrade its own
    verification. The resource seal is checked whenever it is present, whatever
    mode was requested.
    """
    code_resources = contents / "_CodeSignature" / "CodeResources"
    if code_resources.is_file():
        run("codesign", "--verify", "--deep", "--strict", str(bundle))
        return
    if not fixture:
        # No seal at all: only a fixture may be missing one, and the caller
        # asked for a real bundle.
        raise ValueError(f"bundle has no resource seal: {code_resources}")
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
    parser.add_argument("--architectures", nargs="+", default=["arm64"])
    parser.add_argument(
        "--fixture",
        action="store_true",
        help="verify a minimal test fixture (no APPL seal) instead of a distributable bundle",
    )
    args = parser.parse_args()
    try:
        bundle = args.bundle.resolve()
        contents = bundle / "Contents"
        plist_path = contents / "Info.plist"
        with plist_path.open("rb") as source:
            plist = plistlib.load(source)
        if not isinstance(plist, dict):
            raise ValueError("Info.plist root is not a dictionary")
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
        # Explicit encoding: the locale default (often ASCII under LANG=C on
        # CI) would make a non-ASCII byte fail on some machines only.
        vera_notice = (resources / "licenses/Bitstream-Vera-NOTICE.txt").read_text(
            encoding="utf-8"
        )
        if not all(marker in vera_notice for marker in VERA_MARKERS):
            raise ValueError("Bitstream Vera notice is incomplete")
        if sys.platform == "darwin":
            frameworks = contents / "Frameworks"
            expected_architectures = set(args.architectures)
            verify_macho(executable, frameworks, expected_architectures)
            for dylib in frameworks.glob("*.dylib"):
                verify_macho(dylib, frameworks, expected_architectures)
            verify_signature(bundle, contents, executable, plist, args.fixture)

        if sys.platform != "darwin":
            print(
                "warning: Mach-O architecture, bundled-dylib linkage and "
                "code-signature checks were skipped on this platform",
                file=sys.stderr,
            )
        print(f"bundle verified: {bundle}")
        return 0
    except (OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Verify a locally generated Cargo archive and a standalone user installation."""
# SPDX-License-Identifier: GPL-3.0-or-later
import os
from pathlib import Path, PurePosixPath
import re
import struct
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def verify():
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]
    label = f"{manifest['name']}-{manifest['version']}"
    archive = ROOT / ".work/target/package" / f"{label}.crate"
    required = {
        "Cargo.toml", "Cargo.lock", "LICENSE", "NOTICE", "CHANGELOG.md", "README.md",
        "src/main.rs", "src/service.rs", "contrib/systemd/turing-screen-monitor.service",
        "docs/installation.md", "docs/metrics.md", "docs/releasing.md",
    }
    allowed_files = required | {
        "rust-toolchain.toml", "Cargo.toml.orig", ".cargo_vcs_info.json",
        "assets/panel-preview.png",
        "tools/bootstrap-rust", "tools/cargo-local", "tools/check-release",
        "tools/check-package.py", "tools/profile-monitor.py",
    }

    def allowed(path):
        return str(path) in allowed_files or (
            not any(part.startswith(".") or part == "__pycache__" for part in path.parts)
            and ((path.parts[0] == "src" and path.suffix == ".rs")
                 or (len(path.parts) == 2 and path.parts[0] == "docs" and path.suffix == ".md")
                 or (len(path.parts) == 2 and path.parts[0] == "tests" and path.suffix in {".py", ".c"}))
        )

    with tempfile.TemporaryDirectory(prefix="release-check-", dir=ROOT / ".work/tmp") as temporary:
        destination = Path(temporary)
        with tarfile.open(archive) as package:
            members = {}
            for entry in package:
                path = PurePosixPath(entry.name)
                if path.is_absolute() or ".." in path.parts or path.parts[0] != label:
                    raise RuntimeError("Unsafe archive path")
                relative = PurePosixPath(*path.parts[1:])
                if entry.isdir():
                    continue
                if not allowed(relative):
                    raise RuntimeError(f"Unexpected package path: {relative}")
                if not entry.isfile():
                    raise RuntimeError(f"Unexpected non-file archive member: {relative}")
                content = package.extractfile(entry).read()
                members[str(relative)] = content
                if re.search(rb"/(?:home|Users)/[A-Za-z0-9_.-]+/", content):
                    raise RuntimeError(f"Personal absolute path included: {relative}")
                target = destination.joinpath(*path.parts)
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(content)
                target.chmod(entry.mode & 0o777)
        if missing := required - members.keys():
            raise RuntimeError(f"Missing package files: {sorted(missing)}")
        for target in re.findall(r"\[[^\]]*\]\(([^)]+)\)", members["README.md"].decode()):
            if "://" not in target and not target.startswith("#") and target.split("#", 1)[0] not in members:
                raise RuntimeError(f"Broken packaged README link: {target}")
        installed = destination / "install"
        subprocess.run(
            ["bash", str(ROOT / "tools/cargo-local"), "install", "--path",
             str(destination / label), "--locked", "--offline", "--root", str(installed)],
            cwd=ROOT, check=True,
        )
        binary = installed / "bin" / manifest["name"]
        working = destination / "empty"
        working.mkdir()
        env = os.environ.copy()
        env["PATH"] = ""
        for args in [
            ["--version"], ["service", "install", "--dry-run", "--enable", "--start"],
            ["service", "uninstall", "--dry-run"],
        ]:
            result = subprocess.run([str(binary), *args], cwd=working, env=env,
                                    capture_output=True, text=True, timeout=5, check=True)
            if args == ["--version"] and result.stdout.strip() != f"{manifest['name']} {manifest['version']}":
                raise RuntimeError("Installed version does not match manifest")
            if "CPU:" in result.stderr or list(working.iterdir()):
                raise RuntimeError("Informational command sampled metrics or created files")
        preview = working / "preview.png"
        subprocess.run([str(binary), "--preview", str(preview), "--demo"], cwd=working, env=env,
                       capture_output=True, timeout=5, check=True)
        first_preview = preview.read_bytes()
        subprocess.run([str(binary), "--preview", str(preview), "--demo"], cwd=working, env=env,
                       capture_output=True, timeout=5, check=True)
        if preview.read_bytes() != first_preview:
            raise RuntimeError("Demo preview is not deterministic")
        if first_preview != members["assets/panel-preview.png"]:
            raise RuntimeError("Packaged preview differs from the deterministic demo")
        png = preview.read_bytes()
        if png[:8] != b"\x89PNG\r\n\x1a\n" or struct.unpack(">II", png[16:24]) != (480, 320):
            raise RuntimeError("Installed preview has invalid format or dimensions")
        print(f"Verified {len(members)} package files and standalone installation; {archive.stat().st_size} bytes.")


if __name__ == "__main__":
    verify()

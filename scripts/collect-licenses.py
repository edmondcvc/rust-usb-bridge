#!/usr/bin/env python3
"""Collect actual cached Cargo license texts without copying dependencies or state."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import tomllib
from urllib.request import Request, urlopen


def c_path(value: str) -> Path:
    path = Path(value).resolve()
    if os.name == "nt" and path.drive.upper() != "C:":
        raise ValueError(f"Only C: paths are supported: {path}")
    return path


def inside(path: Path, root: Path) -> Path:
    full = path.resolve()
    if not full.is_relative_to(root.resolve()):
        raise ValueError("License path escapes its source or destination directory")
    return full


def license_candidate(name: str) -> bool:
    return bool(re.search(r"(^|[-_.])(LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE|OFL|UFL)([-_.]|$)", name, re.IGNORECASE))


HTTP_CACHE: dict[str, bytes] = {}


def fetch(url: str) -> bytes:
    if url not in HTTP_CACHE:
        request = Request(url, headers={"User-Agent": "RustUsbBridge-LicenseCollector"})
        with urlopen(request, timeout=30) as response:
            HTTP_CACHE[url] = response.read()
    return HTTP_CACHE[url]


def upstream_texts(entry: dict, crate: Path, target: Path) -> None:
    """Fill missing monorepo root licenses from the declared official repository."""
    match = re.match(r"https://github\.com/([^/]+)/([^/#?]+)", entry.get("repository") or "")
    if not match:
        return
    owner, repository = match.groups()
    repository = repository.removesuffix(".git")
    vcs = crate / ".cargo_vcs_info.json"
    revision = json.loads(vcs.read_text(encoding="utf-8")).get("git", {}).get("sha1") if vcs.exists() else None
    exact = bool(revision)
    if not revision:
        revision = json.loads(fetch(f"https://api.github.com/repos/{owner}/{repository}/commits/HEAD"))["sha"]
    tree = json.loads(fetch(f"https://api.github.com/repos/{owner}/{repository}/git/trees/{revision}"))
    for item in tree.get("tree", []):
        if item["type"] != "blob" or not license_candidate(item["path"]):
            continue
        filename = item["path"]
        if "/" in filename:
            continue
        source_url = f"https://raw.githubusercontent.com/{owner}/{repository}/{revision}/{filename}"
        content = fetch(source_url)
        if b"\0" in content:
            continue
        destination = inside(target / f"{entry['crate']}-{entry['version']}" / filename, target)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(content)
        entry["texts"].append({
            "path": destination.relative_to(target).as_posix(),
            "sha256": hashlib.sha256(content).hexdigest(),
            "source_url": source_url,
            "revision_basis": "published crate VCS revision" if exact else "official repository snapshot; crate declares matching license",
        })


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project-dir", required=True)
    parser.add_argument("--registry-dir", required=True)
    args = parser.parse_args()
    project = c_path(args.project_dir)
    registry = c_path(args.registry_dir)
    target = inside(project / "THIRD_PARTY_LICENSES", project)
    if target != project / "THIRD_PARTY_LICENSES":
        raise ValueError("Unexpected license destination")
    if target.is_symlink():
        raise ValueError("License destination must not be a symlink")
    target.mkdir(exist_ok=True)
    with (project / "Cargo.lock").open("rb") as file:
        lock = tomllib.load(file)
    records = []
    copied = 0
    missing = []
    for package in sorted(lock["package"], key=lambda p: (p["name"], p["version"])):
        if not package.get("source", "").startswith("registry+"):
            continue
        name, version = package["name"], package["version"]
        crate = inside(registry / f"{name}-{version}", registry)
        entry = {
            "crate": name,
            "version": version,
            "checksum": package.get("checksum"),
            "metadata_url": f"https://crates.io/crates/{name}/{version}",
            "cached": crate.is_dir(),
            "license": None,
            "repository": None,
            "texts": [],
        }
        if not crate.is_dir():
            records.append(entry)
            continue
        with (crate / "Cargo.toml").open("rb") as file:
            metadata = tomllib.load(file)["package"]
        entry["license"] = metadata.get("license")
        entry["repository"] = metadata.get("repository")
        entry["authors"] = metadata.get("authors", [])
        candidates: set[Path] = set()
        for directory, subdirectories, filenames in os.walk(crate, followlinks=False):
            subdirectories[:] = [d for d in subdirectories if not (Path(directory) / d).is_symlink()]
            for filename in filenames:
                if license_candidate(filename):
                    candidate = Path(directory) / filename
                    if not candidate.is_symlink():
                        candidates.add(inside(candidate, crate))
        if metadata.get("license-file"):
            candidates.add(inside(crate / metadata["license-file"], crate))
        for candidate in sorted(candidates):
            if not candidate.is_file():
                continue
            content = candidate.read_bytes()
            if b"\0" in content:
                continue
            relative = candidate.relative_to(crate)
            destination = inside(target / f"{name}-{version}" / relative, target)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(candidate, destination)
            entry["texts"].append({
                "path": destination.relative_to(target).as_posix(),
                "sha256": hashlib.sha256(content).hexdigest(),
            })
            copied += 1
        # egui crates and embedded fonts publish their Rust code license at monorepo root.
        needs_upstream = not entry["texts"] or "github.com/emilk/egui" in (entry["repository"] or "")
        if needs_upstream:
            try:
                before = len(entry["texts"])
                upstream_texts(entry, crate, target)
                copied += len(entry["texts"]) - before
            except Exception as error:
                entry["upstream_lookup_error"] = str(error)
        if not entry["texts"]:
            missing.append(f"{name}-{version}")
        records.append(entry)
    index = {
        "generated_by": "RustUsbBridge-CargoLicenses-v1",
        "cargo_lock_sha256": hashlib.sha256((project / "Cargo.lock").read_bytes()).hexdigest(),
        "scope": "Cached Cargo.lock registry packages; missing monorepo root texts supplemented from their declared official GitHub repository, with revision/source URL recorded. May include unused platform and development dependencies.",
        "packages": records,
        "cached_packages_without_text": missing,
    }
    (target / "INDEX.json").write_text(json.dumps(index, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    lines = [
        "# Cargo third-party license texts",
        "",
        "The Rust USB Bridge application is licensed under its own MIT LICENSE.",
        "These unchanged dependency and bundled font license texts remain applicable to their respective code/assets.",
        "INDEX.json identifies exact locked versions, crate metadata URLs, declared SPDX licenses, and text hashes.",
        "The directory may include licenses for cached platform-specific or development crates not present in a given binary.",
        "Missing monorepo root license texts are copied from each crate's declared official repository; the index records the exact source and revision basis.",
        "No crate source, credentials, keys, package cache, fonts, or executables are copied here.",
        "",
        f"Copied {copied} license/notice texts from {sum(p['cached'] for p in records)} cached locked crates.",
    ]
    if missing:
        lines.extend(["", "Cached packages without a standalone license text:", "", *[f"- {name}" for name in missing]])
    (target / "NOTICE.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps({"packages": len(records), "cached": sum(p["cached"] for p in records), "texts": copied, "without_text": missing}, ensure_ascii=False))


if __name__ == "__main__":
    main()

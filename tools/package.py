#!/usr/bin/env python3
"""Package the built wasm into an .aix and regenerate the source list.

Aidoku's .aix is a plain zip whose only top-level entry is ``Payload/``
containing main.wasm, source.json and icon.png, using forward slashes.
Ubuntu runners ship GNU tar (no --format zip), so zipfile does the packing.
"""
import json
import os
import shutil
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "src", "zh.hipmh7")
WASM = os.path.join(SRC, "target", "wasm32-unknown-unknown", "release", "hipmh.wasm")
DIST = os.path.join(ROOT, "dist")


def build_aix():
    stage = os.path.join(os.environ.get("RUNNER_TEMP", "/tmp"), "aix-stage")
    payload = os.path.join(stage, "Payload")
    shutil.rmtree(stage, ignore_errors=True)
    os.makedirs(payload, exist_ok=True)

    shutil.copy2(WASM, os.path.join(payload, "main.wasm"))
    shutil.copy2(os.path.join(SRC, "res", "source.json"), os.path.join(payload, "source.json"))
    shutil.copy2(os.path.join(SRC, "res", "icon.png"), os.path.join(payload, "icon.png"))

    out = os.path.join(stage, "package.aix")
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for root, _, files in os.walk(payload):
            for name in files:
                full = os.path.join(root, name)
                arc = os.path.relpath(full, stage).replace(os.sep, "/")
                z.write(full, arc)

    with zipfile.ZipFile(out) as z:
        entries = z.namelist()
    expected = {"Payload/main.wasm", "Payload/source.json", "Payload/icon.png"}
    if not expected.issubset(set(entries)):
        sys.exit("aix is missing entries: %r" % (expected - set(entries),))
    print("aix entries:", entries)
    return out


def write_dist(aix_path):
    info = json.load(open(os.path.join(SRC, "res", "source.json"), encoding="utf-8"))["info"]
    version = int(info["version"])
    os.makedirs(DIST, exist_ok=True)

    versioned = "zh.hipmh-v%d.aix" % version
    shutil.copy2(aix_path, os.path.join(DIST, versioned))
    shutil.copy2(aix_path, os.path.join(DIST, "zh.hipmh.aix"))
    shutil.copy2(os.path.join(SRC, "res", "icon.png"), os.path.join(DIST, "icon.png"))

    entry = {
        "id": info["id"],
        "name": info["name"],
        "version": version,
        # Relative URLs: Aidoku resolves them against the list.json URL, so
        # any mirror/cdn prefix in front of list.json keeps working.
        "downloadURL": versioned,
        "iconURL": "icon.png",
        "languages": info.get("languages", ["zh"]),
        "contentRating": info.get("contentRating", 0),
        "altNames": ["hipmh", "Hippi Comics"],
        "minAppVersion": "0.7",
    }
    source_list = {"name": "嬉皮漫画 图源列表", "sources": [entry]}
    with open(os.path.join(DIST, "list.json"), "w", encoding="utf-8") as f:
        json.dump(source_list, f, ensure_ascii=False, indent=2)
    # flat index kept for compatibility with legacy loaders
    with open(os.path.join(DIST, "index.min.json"), "w", encoding="utf-8") as f:
        json.dump([entry], f, ensure_ascii=False, separators=(",", ":"))

    # Same payload under a second filename: jsdelivr caches /list.json very
    # aggressively (it kept serving an old version after a purge), while the
    # versioned .aix filenames are always fresh. The app only needs *a* URL.
    shutil.copyfile(os.path.join(DIST, "list.json"), os.path.join(DIST, "sources.json"))

    with open(os.path.join(DIST, "README.md"), "w", encoding="utf-8") as f:
        f.write(
            "# 嬉皮漫画 Aidoku 图源（zh.hipmh）\n\n"
            "当前版本：v%d\n\n"
            "Aidoku「浏览 → + → 图源列表」粘贴本目录的 list.json 地址即可；"
            "以后在「更新」区点一下就能升级。\n" % version
        )
    print("dist written for v%d" % version)
    return version


def main():
    if not os.path.exists(WASM):
        sys.exit("wasm not built: %s" % WASM)
    aix = build_aix()
    version = write_dist(aix)
    env_file = os.environ.get("GITHUB_ENV")
    if env_file:
        with open(env_file, "a", encoding="utf-8") as f:
            f.write("VERSION=%d\n" % version)


if __name__ == "__main__":
    main()


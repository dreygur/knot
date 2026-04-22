#!/usr/bin/env python3
"""Read version and description from Cargo.toml and stamp all plugin manifests."""
import json, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).parent.parent

def cargo_metadata():
    raw = subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        text=True, cwd=ROOT,
    )
    pkgs = json.loads(raw)["packages"]
    return next(p for p in pkgs if p["name"] == "knot")

pkg = cargo_metadata()
version     = pkg["version"]
description = pkg["description"]
short_desc  = pkg["metadata"]["knot"]["short_description"]

def update(path, fn):
    p = ROOT / path
    d = json.loads(p.read_text())
    fn(d)
    p.write_text(json.dumps(d, indent=2) + "\n")
    print(f"  {path}")

print(f"[sync] v{version}")

update("plugin.json", lambda d: d.update({"version": version, "description": description}))

update(".claude-plugin/plugin.json", lambda d: d.update({"version": version, "description": description}))

def sync_marketplace(d):
    d["metadata"]["description"] = short_desc
    for p in d["plugins"]:
        if p["name"] == "knot":
            p["version"] = version
            p["description"] = description

update(".claude-plugin/marketplace.json", sync_marketplace)

print("[sync] done")

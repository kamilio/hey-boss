#!/usr/bin/env python3
"""Embed a locally installed axe-core into the Playwright accessibility checks."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--axe", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--template", type=Path, default=Path(__file__).with_name("issues_web_accessibility.js"))
args = parser.parse_args()
template = args.template.read_text()
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(template.replace("/* AXE_SOURCE */", json.dumps(args.axe.read_text())))

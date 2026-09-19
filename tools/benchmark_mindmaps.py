#!/usr/bin/env python3
"""Measure the real CLI's projection over an isolated synthetic map.

Usage: python3 tools/benchmark_mindmaps.py --nodes 2500 --links-per-node 2
No installation or live hey-boss database/Inbox is used.
"""
import argparse
import json
import os
import pathlib
import sqlite3
import statistics
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, default=pathlib.Path("target/debug/hey-boss"))
    parser.add_argument("--nodes", type=int, default=2500)
    parser.add_argument("--links-per-node", type=int, default=2)
    parser.add_argument("--samples", type=int, default=3)
    args = parser.parse_args()
    if not 2 <= args.nodes <= 10000 or not 0 <= args.links_per_node < args.nodes or not 1 <= args.samples <= 20:
        parser.error("Use 2..10000 nodes, fewer links-per-node than nodes, and 1..20 samples")
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="hey-boss-mm-benchmark-") as temporary:
        root = pathlib.Path(temporary)
        env = dict(os.environ, HEY_BOSS_ISSUE_DB=str(root / "issues.db"), HEY_BOSS_INBOX_SOCKET=str(root / "absent.sock"))
        env.pop("HEY_BOSS_ISSUE_HOST", None)
        env.pop("HEY_BOSS_ISSUE_PROJECT", None)
        command = [str(binary), "mm", "--project", "Scale", "--json"]
        subprocess.run(command, env=env, stdout=subprocess.DEVNULL, check=True)
        with sqlite3.connect(root / "issues.db") as db:
            roots = min(25, args.nodes)
            db.executemany("INSERT INTO mindmap_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?,?)", (
                (f"n-scale-{i}", "named:Scale", f"topic-{i}", None if i < roots else f"n-scale-{i % roots}", i, "text", f"Planning topic {i}", "", None, None, 1, 1)
                for i in range(args.nodes)
            ))
            db.executemany("INSERT INTO mindmap_links VALUES(?,?,?,?,?)", (
                (f"n-scale-{i}", f"n-scale-{(i + offset + 1) % args.nodes}", "depends-on", "Planning dependency", 1)
                for i in range(args.nodes) for offset in range(args.links_per_node)
            ))
        samples = []
        for _ in range(args.samples):
            start = time.monotonic()
            reply = subprocess.run(command, env=env, capture_output=True, check=True)
            samples.append(time.monotonic() - start)
            graph = json.loads(reply.stdout)
            assert len(graph["nodes"]) == args.nodes
            assert len(graph["links"]) == args.nodes * args.links_per_node
        print(json.dumps({"nodes": args.nodes, "links": len(graph["links"]), "response_bytes": len(reply.stdout), "show_seconds": samples, "median_show_seconds": statistics.median(samples)}, indent=2))


if __name__ == "__main__":
    main()

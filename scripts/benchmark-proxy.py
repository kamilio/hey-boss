#!/usr/bin/env python3
"""Repeatable offline startup/forwarding benchmark; all credentials/data are synthetic.

uv run --with aiohttp --with psutil scripts/benchmark-proxy.py \
  --binary target/release/hey-proxy --output /tmp/proxy-benchmark.json

Use --history to seed a realistic archived request count, --stream-events to
exercise SSE inspection, and --poll-path to add dashboard traffic under load.
Reports observations rather than machine-dependent pass/fail thresholds.
"""
import argparse
import asyncio
import contextlib
import datetime
import json
import math
import os
import pathlib
import signal
import random
import socket
import sqlite3
import statistics
import subprocess
import tempfile
import time
import urllib.parse
from zoneinfo import ZoneInfo

import aiohttp
from aiohttp import web
import psutil


class Reservoir:
    def __init__(self, limit):
        self.samples, self.count, self.limit = [], 0, limit
        self.random = random.Random(0)

    def append(self, value):
        self.count += 1
        if len(self.samples) < self.limit:
            self.samples.append(value)
        else:
            index = self.random.randrange(self.count)
            if index < self.limit:
                self.samples[index] = value

    def __len__(self):
        return self.count


def percentile(values, percent):
    values = sorted(values.samples if isinstance(values, Reservoir) else values)
    return values[max(0, math.ceil(len(values) * percent / 100) - 1)] if values else None


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def seed_history(path, count, legacy=False):
    if not count:
        return
    now = int(time.time() * 1000)
    with sqlite3.connect(path) as connection:
        connection.execute("PRAGMA journal_mode=WAL")
        if legacy:
            connection.executescript("""DROP TRIGGER IF EXISTS dashboard_insert_v1;
                DROP TRIGGER IF EXISTS dashboard_update_v1; DROP TRIGGER IF EXISTS dashboard_delete_v1;
                DROP TABLE IF EXISTS dashboard_totals;
                DELETE FROM metadata WHERE key='dashboard_minutes_v1';""")
        connection.execute("""WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<?)
            INSERT INTO requests(request_id,session_id,timestamp_ms,updated_ms,ended_ms,
              requested_model,routed_model,project,path,method,transport,mode,state,status,
              retries,input_tokens,output_tokens,cost_nano_usd,record)
            SELECT 'fixture-'||i,'fixture',?-(i%604800)*1000,?-(i%604800)*1000,?-(i%604800)*1000,
              'gpt-4.1','gpt-4.1','default','/v1/responses','POST','HTTP','standalone','succeeded',200,
              0,1000,100,2800000,
              json_object('id',i,'request_id','fixture-'||i,'session_id','fixture',
                'timestamp_ms',?-(i%604800)*1000,'updated_ms',?-(i%604800)*1000,
                'ended_ms',?-(i%604800)*1000,'requested_model','gpt-4.1','routed_model','gpt-4.1',
                'path','/v1/responses','method','POST','transport','HTTP','mode','standalone',
                'state','succeeded','status',200,'retries',0,'input_tokens',1000,'output_tokens',100,
                'estimated_cost_usd',0.0028,'cost_nano_usd',2800000)
            FROM n""", (count, now, now, now, now, now, now))
        connection.commit()
        connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")


async def start_proxy(binary, config, session):
    started = time.perf_counter()
    process = subprocess.Popen([str(binary), "--config", str(config)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    port = json.loads(config.read_text())["listen"].split(":")[-1]
    root = f"http://127.0.0.1:{port}"
    while time.perf_counter() - started < 30:
        if process.poll() is not None:
            raise RuntimeError("Synthetic proxy exited: " + process.stderr.read().decode()[-2000:])
        try:
            async with session.get(root + "/overview/api", timeout=aiohttp.ClientTimeout(total=0.25)) as response:
                await response.read()
                if response.status == 200:
                    return process, root, (time.perf_counter() - started) * 1000
        except (OSError, asyncio.TimeoutError):
            pass
        await asyncio.sleep(0.002)
    process.kill()
    process.wait()
    diagnostic = process.stderr.read().decode()[-2000:]
    process.stderr.close()
    raise RuntimeError("Proxy startup timed out: " + diagnostic)


async def stop_proxy(process):
    process.send_signal(signal.SIGTERM)
    try:
        await asyncio.to_thread(process.wait, 15)
    except subprocess.TimeoutExpired:
        process.kill()
        await asyncio.to_thread(process.wait)
    if process.returncode not in (0, -signal.SIGTERM):
        raise RuntimeError("Proxy shutdown failed: " + process.stderr.read().decode()[-2000:])
    process.stderr.close()


async def wait_database(root, session):
    for _ in range(2000):
        async with session.get(root + "/logs/api/health") as response:
            health = await response.json()
        if health.get("status") in ("healthy", "disabled"):
            return
        if health.get("status") == "error":
            raise RuntimeError("Synthetic database initialization failed")
        await asyncio.sleep(0.005)
    raise RuntimeError("Database initialization timed out")


async def run(args):
    response_value = {"id": "resp_benchmark", "object": "response", "status": "completed", "model": "gpt-4.1",
                      "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "ok"}]}],
                      "usage": {"input_tokens": 1000, "output_tokens": 100}}
    if args.claude:
        response_value = {"type":"message", "id":"msg_benchmark", "role":"assistant", "model":"claude-sonnet-4-6",
                          "content":[{"type":"text", "text":"ok"}], "stop_reason":"end_turn",
                          "usage":{"input_tokens":1000,"output_tokens":100}}
    stream_bytes = []
    if args.claude and args.stream_events:
        events = [{"type":"message_start", "message": response_value | {"content":[], "stop_reason":None, "usage":{"input_tokens":1000,"output_tokens":0}}}]
        events += [{"type":"content_block_delta", "index":0, "delta":{"type":"text_delta","text":"a" * args.delta_bytes}} for _ in range(args.stream_events)]
        events += [{"type":"message_delta", "delta":{"stop_reason":"end_turn"}, "usage":{"output_tokens":100}}, {"type":"message_stop"}]
        stream_bytes = [(f"event: {event['type']}\ndata: " + json.dumps(event) + "\n\n").encode() for event in events]
    elif args.stream_events:
        stream_bytes.append(b'event: response.created\ndata: {"type":"response.created","response":{"id":"resp_benchmark"}}\n\n')
        for _ in range(args.stream_events):
            event = {"type": "response.output_text.delta", "delta": "a" * args.delta_bytes}
            stream_bytes.append(("event: response.output_text.delta\ndata: " + json.dumps(event) + "\n\n").encode())
        stream_bytes.append(("event: response.completed\ndata: " + json.dumps({"type": "response.completed", "response": response_value}) + "\n\n").encode())

    accepted_tokens = {"sk-ant-oat01-synthetic-benchmark" if args.claude else "synthetic"}
    seen_tokens = set()

    async def upstream(request):
        authorization = request.headers.get("Authorization", "")
        assert authorization.startswith("Bearer ") and authorization[7:] in accepted_tokens
        seen_tokens.add(authorization[7:])
        await request.read()
        if stream_bytes:
            response = web.StreamResponse(headers={"Content-Type": "text/event-stream"})
            await response.prepare(request)
            for chunk in stream_bytes:
                await response.write(chunk)
            await response.write_eof()
            return response
        return web.json_response(response_value)

    app = web.Application(client_max_size=max(10 * 1024 * 1024, args.payload_bytes * 2))
    app.router.add_route("*", "/{path:.*}", upstream)
    runner = web.AppRunner(app, access_log=None)
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", 0)
    await site.start()
    upstream_port = site._server.sockets[0].getsockname()[1]
    payload = json.dumps({"model": "gpt-4.1", "input": "a" * args.payload_bytes, "stream": bool(args.stream_events)}).encode()
    if args.claude:
        payload = json.dumps({"model":"claude-sonnet-4-6", "max_tokens":200, "messages":[{"role":"user", "content":"a" * args.payload_bytes}], "stream":bool(args.stream_events)}).encode()
    request_path = "/v1/messages" if args.claude else "/v1/responses"
    samples = []
    async with aiohttp.ClientSession(connector=aiohttp.TCPConnector(limit=args.concurrency + 10), timeout=aiohttp.ClientTimeout(total=30)) as session:
        for repeat in range(args.repeats):
            binary = args.compare_binary if args.compare_binary and repeat % 2 else args.binary
            seen_tokens.clear()
            with tempfile.TemporaryDirectory(prefix="hey-proxy-perf-") as temporary:
                directory = pathlib.Path(temporary)
                config = directory / "config.json"
                providers = {"openai": {"upstream_url": f"http://127.0.0.1:{upstream_port}", "api_keys": {"default":"synthetic"}}}
                if args.claude:
                    import base64
                    from cryptography.hazmat.primitives.ciphers.aead import AESGCMSIV
                    key, nonce = os.urandom(32), os.urandom(12)
                    tokens = json.dumps({"access_token":"sk-ant-oat01-synthetic-benchmark", "refresh_token":"synthetic-refresh", "expires_at":int(time.time()) + 86400}).encode()
                    encrypted = AESGCMSIV(key).encrypt(nonce, tokens, b"hey-proxy Claude OAuth v1")
                    for name, data in [("config.claude.key", key), ("config.claude.json", json.dumps({"encrypted":"v1:" + base64.b64encode(nonce + encrypted).decode()}).encode())]:
                        (directory / name).write_bytes(data)
                        (directory / name).chmod(0o600)
                    providers = {"claude":{"upstream_url": f"http://127.0.0.1:{upstream_port}"}}
                config.write_text(json.dumps({"listen": f"127.0.0.1:{free_port()}",
                    "providers": providers, "retry": {"max_retries": 0}, "logging": {"enabled": not args.memory_only}}))
                process, root, startup_ms = await start_proxy(binary, config, session)
                try:
                    await wait_database(root, session)
                    if args.history and not args.memory_only:
                        await stop_proxy(process)
                        seed_history(directory / "requests.sqlite3", args.history, args.legacy_history)
                        process, root, startup_ms = await start_proxy(binary, config, session)
                        if not args.load_during_init:
                            await wait_database(root, session)
                    # Warm the forwarding stack independently from measured requests.
                    for warmup in range(50):
                        warmup_started = time.perf_counter()
                        async with session.post(root + request_path, data=payload, headers={"Content-Type": "application/json"}) as response:
                            await response.read()
                            assert response.status == 200
                        if warmup == 0:
                            cold_request_ms = (time.perf_counter() - warmup_started) * 1000
                    measured = psutil.Process(process.pid)
                    before = measured.cpu_times()
                    latencies, first_bytes = Reservoir(args.sample_limit), Reservoir(args.sample_limit)
                    statuses, poll_latencies, poll_bytes, progress, calendar_windows, rotations = {}, [], [], [], [], []
                    rss_peak = measured.memory_info().rss
                    started = time.perf_counter()
                    deadline = started + args.duration
                    next_slot = started

                    async def worker():
                        nonlocal next_slot
                        while time.perf_counter() < deadline:
                            if args.rate:
                                scheduled = max(next_slot, time.perf_counter())
                                next_slot = scheduled + 1 / args.rate
                                if scheduled >= deadline:
                                    return
                                await asyncio.sleep(max(0, scheduled - time.perf_counter()))
                            request_start = time.perf_counter()
                            async with session.post(root + request_path, data=payload, headers={"Content-Type": "application/json"}) as response:
                                status = response.status
                                first = await response.content.readany()
                                first_bytes.append((time.perf_counter() - request_start) * 1000)
                                await response.read()
                                assert first or response.content.at_eof()
                            latencies.append((time.perf_counter() - request_start) * 1000)
                            statuses[status] = statuses.get(status, 0) + 1

                    async def poller():
                        if not args.poll_path:
                            return
                        while time.perf_counter() < deadline:
                            poll_start = time.perf_counter()
                            poll_path = args.poll_path
                            if args.calendar_zone:
                                day = datetime.datetime.now(ZoneInfo(args.calendar_zone)).replace(hour=0, minute=0, second=0, microsecond=0)
                                week = day - datetime.timedelta(days=day.weekday())
                                poll_path += "?" + urllib.parse.urlencode({"day_start_ms":int(day.timestamp() * 1000), "week_start_ms":int(week.timestamp() * 1000)})
                            async with session.get(root + poll_path) as response:
                                data = await response.read()
                                poll_bytes.append(len(data))
                                if response.status != 200:
                                    raise RuntimeError(f"Dashboard polling returned HTTP {response.status}")
                            poll_latencies.append((time.perf_counter() - poll_start) * 1000)
                            if args.calendar_zone:
                                spend = json.loads(data).get("spend")
                                if spend and (not calendar_windows or calendar_windows[-1]["day_start_ms"] != spend["day_start_ms"]):
                                    calendar_windows.append(spend)
                                    print(json.dumps({"calendar_boundary":spend}), flush=True)
                            await asyncio.sleep(min(args.poll_interval, max(0, deadline - time.perf_counter())))

                    async def memory():
                        nonlocal rss_peak
                        next_progress = started + args.progress_every if args.progress_every else math.inf
                        while time.perf_counter() < deadline:
                            rss = measured.memory_info().rss
                            rss_peak = max(rss_peak, rss)
                            if time.perf_counter() >= next_progress:
                                async with session.get(root + "/logs/api/health") as response:
                                    health = await response.json()
                                update = {"progress_seconds":round(time.perf_counter() - started), "requests":len(latencies),
                                          "rss_bytes":rss, "logging":health}
                                progress.append(update)
                                print(json.dumps(update), flush=True)
                                next_progress += args.progress_every
                            await asyncio.sleep(min(0.2, max(0, deadline - time.perf_counter())))

                    async def rotate_credentials():
                        if not args.rotate_every:
                            return
                        version = 0
                        while time.perf_counter() + args.rotate_every + 5 < deadline:
                            await asyncio.sleep(args.rotate_every)
                            version += 1
                            token = f"sk-ant-oat01-synthetic-benchmark-{repeat}-{version}"
                            accepted_tokens.add(token)
                            nonce = os.urandom(12)
                            tokens = json.dumps({"access_token":token, "refresh_token":"synthetic-refresh", "expires_at":int(time.time()) + 86400}).encode()
                            encrypted = AESGCMSIV(key).encrypt(nonce, tokens, b"hey-proxy Claude OAuth v1")
                            data = json.dumps({"encrypted":"v1:" + base64.b64encode(nonce + encrypted).decode()}).encode()
                            temporary_path = directory / "config.claude.next"
                            descriptor = os.open(temporary_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                            with os.fdopen(descriptor, "wb") as target:
                                target.write(data)
                                target.flush()
                                os.fsync(target.fileno())
                            changed = time.perf_counter()
                            temporary_path.replace(directory / "config.claude.json")
                            while token not in seen_tokens and time.perf_counter() - changed < 3:
                                await asyncio.sleep(0.02)
                            assert token in seen_tokens, "Proxy did not adopt rotated synthetic credentials"
                            rotations.append({"version":version, "adoption_ms":(time.perf_counter() - changed) * 1000})

                    await asyncio.gather(*(worker() for _ in range(args.concurrency)), poller(), memory(), rotate_credentials())
                    elapsed = time.perf_counter() - started
                    under_load = measured.cpu_times()
                    drain_started = time.perf_counter()
                    while True:
                        async with session.get(root + "/logs/api/health") as response:
                            health = await response.json()
                        if not health.get("pending_events") or time.perf_counter() - drain_started > 30:
                            break
                        rss_peak = max(rss_peak, measured.memory_info().rss)
                        await asyncio.sleep(0.05)
                    drain_seconds = time.perf_counter() - drain_started
                    after = measured.cpu_times()
                    cpu = after.user + after.system - before.user - before.system
                    load_cpu = under_load.user + under_load.system - before.user - before.system
                    sample = {"repeat": repeat + 1, "binary":str(binary), "startup_ms": startup_ms, "requests": len(latencies),
                        "elapsed_seconds": elapsed, "requests_per_second": len(latencies) / elapsed, "cold_request_ms":cold_request_ms,
                        "latency_samples": len(latencies.samples), "progress": progress, "calendar_windows":calendar_windows, "credential_rotations":rotations,
                        "latency_ms": {"p50": percentile(latencies, 50), "p95": percentile(latencies, 95), "p99": percentile(latencies, 99)},
                        "first_byte_ms": {"p50": percentile(first_bytes, 50), "p95": percentile(first_bytes, 95), "p99": percentile(first_bytes, 99)},
                        "proxy_cpu_seconds": cpu, "proxy_cpu_us_per_request": cpu * 1e6 / len(latencies),
                        "proxy_cpu_us_per_request_under_load": load_cpu * 1e6 / len(latencies),
                        "accounting_drain_seconds": drain_seconds,
                        "proxy_peak_rss_bytes": rss_peak, "status_counts": statuses,
                        "polls": len(poll_latencies), "poll_p95_ms": percentile(poll_latencies, 95), "poll_max_bytes": max(poll_bytes, default=0),
                        "logging": health}
                    samples.append(sample)
                    print(json.dumps(sample), flush=True)
                finally:
                    if process.poll() is None:
                        await stop_proxy(process)
                sample["database_bytes"] = sum(p.stat().st_size for p in directory.glob("requests.sqlite3*"))
                if args.verify_accounting:
                    assert statuses == {200: len(latencies)}, statuses
                    assert not health.get("dropped_events") and not health.get("pending_events"), health
                    assert not health.get("write_errors"), health
                    if not args.memory_only:
                        with sqlite3.connect(directory / "requests.sqlite3") as connection:
                            assert connection.execute("PRAGMA quick_check").fetchone()[0] == "ok"
                            recorded, cost = connection.execute("SELECT count(*), coalesce(sum(cost_nano_usd),0) FROM requests").fetchone()
                            expected = args.history + 50 + len(latencies)
                            assert recorded == expected, (recorded, expected)
                            sample["accounting_verified"] = {"requests":recorded, "cost_nano_usd":cost}
                            if connection.execute("SELECT count(*) FROM sqlite_master WHERE name='dashboard_totals'").fetchone()[0]:
                                totals = connection.execute("SELECT requests, cost_nano_usd FROM dashboard_totals WHERE bucket=-1").fetchone()
                                assert totals == (recorded, cost), (totals, recorded, cost)
    await runner.cleanup()
    report = {"binary": str(args.binary), "configuration": vars(args) | {"binary": str(args.binary), "output": str(args.output), "compare_binary": str(args.compare_binary) if args.compare_binary else None},
              "samples": samples, "median_requests_per_second": statistics.median(s["requests_per_second"] for s in samples),
              "median_cpu_us_per_request": statistics.median(s["proxy_cpu_us_per_request"] for s in samples),
              "median_startup_ms": statistics.median(s["startup_ms"] for s in samples)}
    report["by_binary"] = {}
    for binary in {sample["binary"] for sample in samples}:
        selected = [sample for sample in samples if sample["binary"] == binary]
        report["by_binary"][binary] = {
            "runs":len(selected),
            "median_cpu_us_per_request":statistics.median(s["proxy_cpu_us_per_request"] for s in selected),
            "median_requests_per_second":statistics.median(s["requests_per_second"] for s in selected),
            "median_p95_ms":statistics.median(s["latency_ms"]["p95"] for s in selected),
            "median_first_byte_p95_ms":statistics.median(s["first_byte_ms"]["p95"] for s in selected)}
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Saved {args.output}", flush=True)


async def startups(args):
    samples = []
    async with aiohttp.ClientSession() as session:
        for trial in range(args.startup_trials):
            binary = args.compare_binary if args.compare_binary and trial % 2 else args.binary
            with tempfile.TemporaryDirectory(prefix="hey-proxy-startup-") as temporary:
                directory = pathlib.Path(temporary)
                config = directory / "config.json"
                config.write_text(json.dumps({"listen":f"127.0.0.1:{free_port()}",
                    "providers":{"openai":{"upstream_url":"http://127.0.0.1:9", "api_keys":{"default":"synthetic"}}},
                    "logging":{"enabled":not args.memory_only}}))
                process = None
                try:
                    process, root, elapsed = await start_proxy(binary, config, session)
                    sample = {"trial":trial + 1, "binary":str(binary), "startup_ms":elapsed}
                except Exception as error:
                    sample = {"trial":trial + 1, "binary":str(binary), "error":str(error)}
                finally:
                    if process is not None and process.poll() is None:
                        await stop_proxy(process)
                samples.append(sample)
                print(json.dumps(sample), flush=True)
    values = [sample["startup_ms"] for sample in samples if "startup_ms" in sample]
    errors = len(samples) - len(values)
    report = {"binary":str(args.binary), "samples":samples, "errors":errors,
              "startup_ms":{"p50":percentile(values,50), "p95":percentile(values,95), "p99":percentile(values,99), "max":max(values,default=None)}}
    report["by_binary"] = {}
    for binary in {sample["binary"] for sample in samples}:
        times = [sample["startup_ms"] for sample in samples if sample["binary"] == binary and "startup_ms" in sample]
        report["by_binary"][binary] = {"trials":len(times), "p50":percentile(times,50), "p95":percentile(times,95), "max":max(times,default=None)}
    args.output.write_text(json.dumps(report,indent=2) + "\n")
    print(f"Saved {args.output}", flush=True)
    if errors:
        raise RuntimeError(f"{errors} startup trials failed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--startup-trials", type=int, default=0)
    parser.add_argument("--compare-binary", type=pathlib.Path, help="Alternate startup trials or workload repeats with this binary")
    parser.add_argument("--rate", type=float, default=0, help="Target total requests/second; zero means saturation")
    parser.add_argument("--sample-limit", type=int, default=1_000_000, help="Bound latency samples during long soaks")
    parser.add_argument("--progress-every", type=float, default=0, help="Emit health/memory progress every N seconds")
    parser.add_argument("--duration", type=float, default=20)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--concurrency", type=int, default=16)
    parser.add_argument("--payload-bytes", type=int, default=4096)
    parser.add_argument("--stream-events", type=int, default=0)
    parser.add_argument("--delta-bytes", type=int, default=32)
    parser.add_argument("--history", type=int, default=0)
    parser.add_argument("--legacy-history", action="store_true", help="Seed an older archive without dashboard aggregates to exercise migration")
    parser.add_argument("--load-during-init", action="store_true", help="Start forwarding while seeded-history initialization is still running")
    parser.add_argument("--memory-only", action="store_true")
    parser.add_argument("--verify-accounting", action="store_true", help="Verify successful responses, durable counts, aggregate costs and SQLite integrity after each run")
    parser.add_argument("--claude", action="store_true", help="Benchmark native Messages using synthetic encrypted OAuth credentials; requires cryptography")
    parser.add_argument("--poll-path", default="")
    parser.add_argument("--poll-interval", type=float, default=5)
    parser.add_argument("--calendar-zone", help="Poll compact dashboard with local calendar boundaries in this IANA time zone")
    parser.add_argument("--rotate-every", type=float, default=0, help="Rotate synthetic Claude credential files every N seconds and verify adoption")
    args = parser.parse_args()
    if args.rotate_every and (not args.claude or args.rotate_every < 1 or 0 < args.rate < 1):
        parser.error("rotate-every requires Claude, at least one second and rate >= 1 (or saturation)")
    if args.calendar_zone:
        if args.poll_path != "/logs/api/dashboard":
            parser.error("calendar-zone requires --poll-path /logs/api/dashboard")
        try:
            ZoneInfo(args.calendar_zone)
        except (KeyError, ValueError):
            parser.error("unknown calendar-zone")
    args.binary = args.binary.resolve()
    if args.compare_binary:
        args.compare_binary = args.compare_binary.resolve()
    if args.duration <= 0 or args.concurrency <= 0 or args.repeats <= 0:
        parser.error("duration, concurrency and repeats must be positive")
    if args.rate < 0 or args.sample_limit <= 0 or args.startup_trials < 0 or args.progress_every < 0:
        parser.error("rate, progress and startup trials must be nonnegative; sample-limit must be positive")
    asyncio.run(startups(args) if args.startup_trials else run(args))

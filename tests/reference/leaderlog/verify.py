#!/usr/bin/env python3
"""Validate untouched C goldens and regenerate fixtures using the real node oracle.

Requires cc/g++, GMP, and an already-built jormanager-leaderlog-oracle.
No Kotlin numerical implementation is used to create expected values.
Optional --cncli replays all independently checked fixtures through the real CLI,
including SQLite persistence. Add --benchmark for verified long-fixture timings.
"""
import argparse
from datetime import datetime, timezone
from fractions import Fraction
import gzip
import hashlib
import itertools
import json
import math
import os
import re
import shutil
import signal
import sqlite3
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
RESOURCES = ROOT / "tests/data/leaderlog"
SCALE = 10**34
ROWS = 100000
HASHES = {
    "golden_tests.txt": "54da107c827bf9d21484f88bbbe8dc140071d44aa2a6d665113a1dfd26dcad9a",
    "golden_tests_result.txt": "1918c67d0043dc4b03a430eac7c989c5e753c405b7d7e2d4d7cac177da7b1a69",
}
DECIMAL = re.compile(r"-?\d+\.\d{34}\Z")
INTEGER = re.compile(r"-?\d+\Z")
CLASSES = {"ABOVE", "BELOW", "MAX_REACHED"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def compressed(path, payload):
    path.write_bytes(gzip.compress(payload, mtime=0))


def lines(data, count, fields, label):
    require(data.endswith(b"\n"), f"{label}: truncated final row")
    result = [row.split() for row in data.decode("ascii").splitlines()]
    require(len(result) == count, f"{label}: expected {count} rows, got {len(result)}")
    require(all(len(row) == fields for row in result), f"{label}: malformed field count")
    return result


def validate_corpus(source):
    reference = source / "libs/non-integral/reference"
    payloads = {}
    for name, digest in HASHES.items():
        data = (reference / name).read_bytes()
        require(hashlib.sha256(data).hexdigest() == digest, f"{name}: wrong upstream SHA-256")
        require(gzip.decompress((RESOURCES / (name + ".gz")).read_bytes()) == data,
                f"{name}: committed compressed corpus differs")
        payloads[name] = data
    inputs = lines(payloads["golden_tests.txt"], ROWS, 3, "C inputs")
    outputs = lines(payloads["golden_tests_result.txt"], ROWS, 6, "C outputs")
    require(all(all(INTEGER.fullmatch(v) for v in row) for row in inputs), "malformed C input")
    require(all(all(DECIMAL.fullmatch(v) for v in row[:4]) and row[4] in {"LT", "GT"}
                and INTEGER.fullmatch(row[5]) and 0 < int(row[5]) <= 1000 for row in outputs),
            "malformed C output")
    with tempfile.TemporaryDirectory(prefix="leaderlog-c-") as directory:
        work = Path(directory)
        # Keep upstream source untouched, including headers. Build products stay temporary.
        for name in ("non_integral.c", "non_integral.h", "non_integral.hpp", "non_integral_test.cpp"):
            shutil.copyfile(reference / name, work / name)
        subprocess.run(["cc", "-O2", "-fPIC", "-c", "non_integral.c", "-o", "non_integral.o"], cwd=work, check=True)
        subprocess.run(["g++", "-O2", "non_integral_test.cpp", "non_integral.o", "-lgmp", "-lgmpxx", "-o", "runner"], cwd=work, check=True)
        with (reference / "golden_tests.txt").open("rb") as stdin, (work / "result").open("wb") as stdout:
            subprocess.run([str(work / "runner")], stdin=stdin, stdout=stdout, check=True)
        actual = (work / "result").read_bytes()
        lines(actual, ROWS, 6, "C regenerated output")
        require(actual == payloads["golden_tests_result.txt"], "C regenerated stdout differs byte-for-byte")
    return payloads, inputs, outputs


def oracle_rows(oracle, mode, rows, expected_fields):
    payload = "".join(" ".join(map(str, row)) + "\n" for row in rows).encode("ascii")
    result = subprocess.run([oracle, mode], input=payload, stdout=subprocess.PIPE, check=True).stdout
    parsed = lines(result, len(rows), expected_fields, f"node {mode}")
    return result, parsed


def value_rows(oracle, rows):
    _, result = oracle_rows(oracle, "values", rows, 5)
    for row, output in zip(rows, result):
        require(output[0] in {"true", "false"}, "invalid node boolean")
        require(all(INTEGER.fullmatch(output[i]) for i in (1, 3)), "invalid node sigma/q")
        require(all(INTEGER.fullmatch(output[i]) for i in (2, 4)) if row[4] != row[5]
                else output[2] == output[4] == "none", "invalid node c/x")
    return result


def generate(oracle, directory, inputs, c_outputs, specs):
    arithmetic, node = oracle_rows(oracle, "arithmetic", inputs, 5)
    require(all(all(DECIMAL.fullmatch(v) for v in row[:3]) and row[3] in CLASSES
                and row[4].isdigit() and 0 < int(row[4]) <= 1000 for row in node), "malformed node arithmetic")
    compressed(directory / "node-arithmetic.tsv.gz", arithmetic)
    divergences = []
    for index, (inp, c_row, node_row) in enumerate(zip(inputs, c_outputs, node)):
        for field, c_result, node_result in zip(("exp", "negativeLn", "threshold"), c_row[:3], node_row[:3]):
            if c_result != node_result:
                divergences.append(dict(row=index, field=field, input=" ".join(inp), cResult=c_result, nodeResult=node_result))
    write_json(directory / "oracle-divergences.json", divergences)

    primitive_cases = {
        "multiply": [(-1, 1), (1, -1), (-1, -1), (1, 1),
                     (-SCALE, SCALE), (SCALE, -SCALE), (-SCALE, -SCALE), (0, -1)],
        "divide": [(a, b) for a in (-7, -6, 0, 6, 7)
                   for b in (-3 * SCALE, 3 * SCALE)] + [(-1, 2 * SCALE), (1, -2 * SCALE)],
        "ratio": [(a, b) for a in (-3, -1, 0, 1, 3) for b in (-3, 3)]
                 + [(-1, SCALE * 2), (1, -SCALE * 2)],
    }
    primitive_rows = []
    for operation, arguments in primitive_cases.items():
        _, results = oracle_rows(oracle, operation, arguments, 1)
        require(all(INTEGER.fullmatch(row[0]) for row in results), "invalid primitive output")
        primitive_rows.extend("\t".join(map(str, [operation, *args, output[0]])) + "\n"
                              for args, output in zip(arguments, results))
    (directory / "node-primitives.tsv").write_text("".join(primitive_rows), encoding="ascii")

    extra_inputs = list(itertools.product(
        (-SCALE, -1, 0, 1, SCALE, 2 * SCALE, 3 * SCALE),
        (1, 10**15, SCALE - 1, SCALE, 2 * SCALE),
        (0, 1, SCALE // 3, SCALE - 1)))
    regular = [args for args in extra_inputs if args[1] != SCALE]
    _, extra_outputs = oracle_rows(oracle, "arithmetic", regular, 5)
    results = dict(zip(regular, extra_outputs))
    for args in extra_inputs:
        if args[1] == SCALE:
            failure = subprocess.run([oracle, "arithmetic"],
                input=(" ".join(map(str, args)) + "\n").encode("ascii"),
                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            require(failure.returncode != 0 and not failure.stdout
                    and b"divide by zero" in failure.stderr,
                    f"singular arithmetic row did not produce the expected oracle error: {args}")
            results[args] = ["ERROR", "divide-by-zero"]
    (directory / "node-extra-arithmetic.tsv").write_text(
        "".join("\t".join(map(str, [*args, *results[args]])) + "\n"
                for args in extra_inputs), encoding="ascii")

    rows = []
    for width, (pool, active), (fn, fd) in itertools.product(specs["widths"], specs["stakes"], specs["coefficients"]):
        maximum = 1 << (8 * width)
        rows.extend([width, cert, pool, active, fn, fd] for cert in (0, 1, maximum - 1, maximum))
    # Batch each bisection round across all boundaries to amortize oracle startup.
    boundaries = []
    for width, (pool, active), (fn, fd) in itertools.product(specs["widths"], specs["boundaryStakes"], specs["boundaryCoefficients"]):
        boundaries.append([width, pool, active, fn, fd, 0, 1 << (8 * width)])
    endpoint_rows = [[w, cert, p, a, fn, fd] for w, p, a, fn, fd, lo, hi in boundaries for cert in (lo, hi)]
    endpoints = value_rows(oracle, endpoint_rows)
    require(all(output[0] == ("true" if index % 2 == 0 else "false") for index, output in enumerate(endpoints)),
            "invalid boundary endpoints")
    while True:
        pending = [boundary for boundary in boundaries if boundary[6] - boundary[5] > 1]
        if not pending:
            break
        queries = [[w, (lo + hi) // 2, p, a, fn, fd] for w, p, a, fn, fd, lo, hi in pending]
        for boundary, query, output in zip(pending, queries, value_rows(oracle, queries)):
            boundary[5 if output[0] == "true" else 6] = query[1]
    for width, pool, active, fn, fd, low, high in boundaries:
        rows.extend([width, cert, pool, active, fn, fd] for cert in (low - 1, low, low + 1) if cert >= 0)
    # Preserve first occurrence/order, including diagnostic large integer inputs.
    rows = [list(row) for row in dict.fromkeys(map(tuple, rows))]
    outputs = value_rows(oracle, rows)
    (directory / "node-vectors.tsv").write_text("".join("\t".join(map(str, row + output)) + "\n" for row, output in zip(rows, outputs)), encoding="ascii")

    comparisons = specs["comparisons"]
    _, outputs = oracle_rows(oracle, "comparison", comparisons, 2)
    require(all(row[0] in CLASSES and row[1].isdigit() and 0 < int(row[1]) <= 1000 for row in outputs), "invalid node comparison")
    (directory / "node-comparisons.tsv").write_text("".join("\t".join(map(str, row + output)) + "\n" for row, output in zip(comparisons, outputs)), encoding="ascii")

    cases = []
    for case in specs["schedules"]:
        keys = ("mode", "firstSlot", "slotCount", "poolStake", "activeStake", "fNumerator", "fDenominator", "dNumerator", "dDenominator", "nonceHex")
        result = subprocess.run([oracle, "schedule"] + [str(case[key]) for key in keys], stdout=subprocess.PIPE, check=True).stdout
        require(result.endswith(b"\n"), "truncated schedule output")
        output = result.decode("ascii").splitlines()
        require(len(output) > 0 and re.fullmatch(r"[0-9a-f]{128}", output[0]), "invalid synthetic VRF key")
        require(all(INTEGER.fullmatch(slot) for slot in output[1:]), "invalid schedule slot")
        slots = list(map(int, output[1:]))
        require(slots == sorted(set(slots)), "schedule not sorted/unique")
        require(all(case["firstSlot"] <= slot < case["firstSlot"] + case["slotCount"] for slot in slots), "schedule slot outside range")
        cases.append(dict(case, vrfSkeyHex=output[0], assignedSlots=slots))
    freeze = Path(__file__).with_name("cabal.project.freeze")
    require(freeze.is_file(), "record resolved transitive pins first: cabal freeze --project-file=src/test/reference/leaderlog/cabal.project")
    metadata = dict(specs["metadata"], freezeSha256=hashlib.sha256(freeze.read_bytes()).hexdigest())
    write_json(directory / "node-schedules.json", dict(metadata=metadata, cases=cases))


def ratio_text(numerator, denominator):
    """Use a lossless decimal when terminating, otherwise the exact fraction."""
    ratio = Fraction(int(numerator), int(denominator))
    remaining = ratio.denominator
    twos = fives = 0
    while remaining % 2 == 0:
        remaining //= 2
        twos += 1
    while remaining % 5 == 0:
        remaining //= 5
        fives += 1
    if remaining != 1:
        return f"{ratio.numerator}/{ratio.denominator}"
    digits = max(twos, fives)
    scaled = ratio.numerator * (10**digits // ratio.denominator)
    if not digits:
        return str(scaled)
    integer, fraction = divmod(scaled, 10**digits)
    return f"{integer}.{fraction:0{digits}d}".rstrip("0").rstrip(".")


def fixed_float(raw):
    return float(f"{raw // SCALE}.{raw % SCALE:034d}")


def display_round(value):
    # Rust's positive f64::round rounds halfway away from zero, unlike Python.
    return math.floor(value * 100.0 + 0.5) / 100.0


def run_cli_case(cncli, case, consensus, d_text, measured=False):
    label = (f"{consensus}, first={case['firstSlot']}, count={case['slotCount']}, "
             f"stake={case['poolStake']}/{case['activeStake']}, "
             f"f={case['fNumerator']}/{case['fDenominator']}, d={d_text}")
    first, count = case["firstSlot"], case["slotCount"]
    require(first > count and (first - count) % 10 == 0, f"{label}: unsupported epoch mapping")
    with tempfile.TemporaryDirectory(prefix="leaderlog-cli-") as temporary:
        directory = Path(temporary)
        database = directory / "chain.db"
        with sqlite3.connect(database) as connection:
            connection.execute("VACUUM")
        start = int(time.time()) - (first + 2 * count + 300)
        write_json(directory / "byron.json", {
            "startTime": start,
            "protocolConsts": {"k": (first - count) // 10},
            "blockVersionData": {"slotDuration": "1000"},
        })
        f_text = ratio_text(case["fNumerator"], case["fDenominator"])
        coefficient = (json.dumps({"numerator": int(case["fNumerator"]),
                                   "denominator": int(case["fDenominator"])})
                       if "/" in f_text else f_text)
        (directory / "shelley.json").write_text(
            f'{{"activeSlotsCoeff":{coefficient},"networkMagic":42,'
            f'"slotLength":1,"epochLength":{count}}}\n', encoding="utf-8")
        write_json(directory / "pool.vrf.skey", {
            "type": "VrfSigningKey_PraosVRF",
            "description": "public zero-seed node oracle fixture",
            "cborHex": "5840" + case["vrfSkeyHex"],
        })
        command = [
            str(cncli), "leaderlog", "--db", str(database),
            "--byron-genesis", str(directory / "byron.json"),
            "--shelley-genesis", str(directory / "shelley.json"),
            "--pool-stake", str(case["poolStake"]),
            "--active-stake", str(case["activeStake"]), f"--d={d_text}",
            "--pool-id", "node-parity", "--pool-vrf-skey", str(directory / "pool.vrf.skey"),
            "--tz", "UTC", "--consensus", consensus, "--shelley-transition-epoch", "1",
            "--nonce", case["nonceHex"], "--epoch", "2",
        ]
        metrics = directory / "time.txt"
        if measured:
            command = ["/usr/bin/time", "-v", "-o", str(metrics)] + command
        began = time.perf_counter()
        child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 start_new_session=True)
        try:
            stdout, stderr = child.communicate(timeout=600)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate()
            raise ValueError(f"{label}: CLI exceeded 600-second deadline")
        elapsed = time.perf_counter() - began
        require(child.returncode == 0,
                f"{label}: CLI exit {child.returncode}: {stderr.decode(errors='replace')}")
        try:
            body = json.loads(stdout)
        except (ValueError, UnicodeDecodeError) as error:
            raise ValueError(f"{label}: invalid CLI JSON: {stdout!r}; stderr: {stderr!r}") from error
        slots = case["assignedSlots"]
        require(slots == sorted(set(slots)), f"{label}: oracle slots not ordered/unique")
        sigma_raw = int(case["poolStake"]) * SCALE // int(case["activeStake"])
        f_raw = int(case["fNumerator"]) * SCALE // int(case["fDenominator"])
        d = Fraction(d_text)
        multiplier = ((d.denominator - d.numerator) * SCALE // d.denominator
                      if consensus == "tpraos" else SCALE)
        ideal_raw = sigma_raw * (count * f_raw) // SCALE
        ideal_raw = ideal_raw * multiplier // SCALE
        ideal = display_round(fixed_float(ideal_raw))
        expected = {
            "status": "ok", "epoch": 2, "epochNonce": case["nonceHex"],
            "consensus": consensus, "epochSlots": len(slots), "epochSlotsIdeal": ideal,
            "maxPerformance": math.floor(len(slots) / ideal * 10000.0 + 0.5) / 100.0 if ideal else None,
            "poolId": "node-parity", "sigma": fixed_float(sigma_raw),
            "activeStake": int(case["poolStake"]), "totalActiveStake": int(case["activeStake"]),
            "f": fixed_float(f_raw), "d": fixed_float(d.numerator * SCALE // d.denominator),
            "assignedSlots": [
                {"no": index + 1, "slot": slot, "slotInEpoch": slot - first,
                 "at": datetime.fromtimestamp(start + slot, timezone.utc).isoformat()}
                for index, slot in enumerate(slots)
            ],
        }
        require(body == expected, f"{label}: full CLI JSON differs from oracle schedule/display contract")
        compact = json.dumps(slots, separators=(",", ":"))
        with sqlite3.connect(database) as connection:
            rows = connection.execute(
                "SELECT slot_qty,slots,hash FROM slots WHERE epoch=2 AND pool_id='node-parity'"
            ).fetchall()
            total = connection.execute("SELECT COUNT(*) FROM slots").fetchone()[0]
        require(total == 1 and len(rows) == 1, f"{label}: missing or extra stored schedules")
        quantity, stored, digest = rows[0]
        require(quantity == len(slots) and json.loads(stored) == slots and stored == compact,
                f"{label}: stored ordered slots differ")
        require(digest == hashlib.blake2b(compact.encode("ascii"), digest_size=32).hexdigest(),
                f"{label}: stored slot hash differs")
        if not measured:
            return None
        rss = re.findall(r"^\s*Maximum resident set size \(kbytes\):\s*(\d+)\s*$",
                         metrics.read_text(), re.MULTILINE)
        require(len(rss) == 1, f"{label}: missing/ambiguous child peak RSS")
        return {"wallSeconds": elapsed, "peakRssKiB": int(rss[0])}


def verify_cli(cncli, benchmark):
    require(cncli.is_file() and os.access(cncli, os.X_OK), f"CNCLI is not executable: {cncli}")
    cases = json.loads((RESOURCES / "node-schedules.json").read_text())["cases"]
    runs = 0
    for case in cases:
        d_text = ratio_text(case["dNumerator"], case["dDenominator"])
        modes = ("praos", "cpraos") if case["mode"] == "praos" else ("tpraos",)
        for consensus in modes:
            run_cli_case(cncli, case, consensus, d_text)
            runs += 1
            if case["mode"] == "praos":
                for nonzero_d in ("0.0004", "1/3", "1"):
                    run_cli_case(cncli, case, consensus, nonzero_d)
                    runs += 1
    print(f"Validated {runs} actual CLI schedules, full JSON, stored slots and Blake2b hashes")
    if benchmark:
        require(Path("/usr/bin/time").is_file(), "benchmark requires /usr/bin/time")
        long_cases = [case for case in cases if case["slotCount"] == 432000]
        require(len(long_cases) == 2 and {case["mode"] for case in long_cases} == {"tpraos", "praos"},
                "benchmark requires the two original 432000-slot fixtures")
        summaries = []
        for case in long_cases:
            d_text = ratio_text(case["dNumerator"], case["dDenominator"])
            run_cli_case(cncli, case, case["mode"], d_text)  # One verified warmup.
            samples = [run_cli_case(cncli, case, case["mode"], d_text, measured=True)
                       for _ in range(5)]
            summaries.append({
                "consensus": case["mode"], "firstSlot": case["firstSlot"], "slotCount": case["slotCount"],
                "assignedSlots": len(case["assignedSlots"]), "samples": samples,
                "medianWallSeconds": statistics.median(sample["wallSeconds"] for sample in samples),
                "medianPeakRssKiB": statistics.median(sample["peakRssKiB"] for sample in samples),
            })
        print(json.dumps({"benchmark": summaries, "rayonNumThreads": os.environ.get("RAYON_NUM_THREADS")},
                         indent=2, sort_keys=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--refresh", action="store_true")
    mode.add_argument("--check", action="store_true")
    parser.add_argument("--ledger-source", type=Path, required=True)
    parser.add_argument("--oracle", required=True)
    parser.add_argument("--cncli", type=Path,
                        help="after independent --check, replay every schedule through this actual executable")
    parser.add_argument("--benchmark", action="store_true",
                        help="with --cncli, verify one warmup and five timed runs of each long fixture")
    args = parser.parse_args()
    if args.cncli and not args.check:
        parser.error("--cncli requires --check so independent oracle checks precede CLI replay")
    if args.benchmark and not args.cncli:
        parser.error("--benchmark requires --cncli")
    specs = json.loads((RESOURCES / "fixture-specifications.json").read_text())
    _, inputs, outputs = validate_corpus(args.ledger_source)
    with tempfile.TemporaryDirectory(prefix="leaderlog-node-") as temporary:
        target = Path(temporary)
        generate(str(Path(args.oracle).resolve()), target, inputs, outputs, specs)
        for generated in sorted(target.iterdir()):
            committed = RESOURCES / generated.name
            if args.refresh:
                shutil.copyfile(generated, committed)
            else:
                require(committed.is_file(), f"missing committed fixture: {committed}")
                require(generated.read_bytes() == committed.read_bytes(), f"node fixture differs: {committed}")
    print(f"Validated all {ROWS} C rows and {'refreshed' if args.refresh else 'checked'} every node fixture")
    if args.cncli:
        verify_cli(args.cncli.resolve(), args.benchmark)


if __name__ == "__main__":
    main()

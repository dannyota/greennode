#!/usr/bin/env -S python3 -I
# Usage: python3 -I scripts/test/check-crate-age-test.py
import datetime
import importlib.util
import sys
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-crate-age.py"
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("check_crate_age", SCRIPT)
check_crate_age = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check_crate_age)

NOW = datetime.datetime(2026, 11, 18, 12, 0, tzinfo=datetime.timezone.utc)
REGISTRY = check_crate_age.CRATES_IO


def lock(*packages):
    blocks = []
    for name, version, source in packages:
        block = f'[[package]]\nname = "{name}"\nversion = "{version}"\n'
        if source:
            block += f'source = "{source}"\n'
        blocks.append(block)
    return "version = 4\n\n" + "\n".join(blocks)


def fetcher(ages):
    calls = []

    def fetch(name, version):
        calls.append((name, version))
        return NOW - datetime.timedelta(hours=ages[(name, version)])

    return fetch, calls


class CrateAge(unittest.TestCase):
    def test_only_changed_crates_io_versions_are_fetched(self):
        base = lock(("serde", "1.0.0", REGISTRY), ("tokio", "1.0.0", REGISTRY))
        head = lock(
            ("serde", "1.0.0", REGISTRY),
            ("tokio", "1.1.0", REGISTRY),
            ("greennode", "0.1.0", None),
            ("forked", "0.1.0", "git+https://example.invalid/forked"),
        )
        fetch, calls = fetcher({("tokio", "1.1.0"): 100})
        report, failures = check_crate_age.check(base, head, {}, fetch, NOW)
        self.assertEqual(calls, [("tokio", "1.1.0")])
        self.assertEqual((report, failures), (["ok: tokio@1.1.0 published 100h ago"], 0))

    def test_boundary_at_24_hours(self):
        head = lock(("old", "1.0.0", REGISTRY), ("new", "1.0.0", REGISTRY))
        fetch, _ = fetcher({("old", "1.0.0"): 24, ("new", "1.0.0"): 23.9})
        report, failures = check_crate_age.check("", head, {}, fetch, NOW)
        self.assertEqual(failures, 1)
        self.assertEqual(report, [
            "too new: new@1.0.0 published 23h ago, under 24h",
            "ok: old@1.0.0 published 24h ago",
        ])

    def test_exempt_version_passes_with_reason(self):
        head = lock(("rustls", "0.23.99", REGISTRY))
        fetch, _ = fetcher({("rustls", "0.23.99"): 2})
        exempt = check_crate_age.exemptions("rustls@0.23.99 fixes RUSTSEC-2026-0001\n")
        report, failures = check_crate_age.check("", head, exempt, fetch, NOW)
        self.assertEqual((report, failures), (
            ["exempt: rustls@0.23.99 published 2h ago: fixes RUSTSEC-2026-0001"], 0,
        ))


if __name__ == "__main__":
    unittest.main()

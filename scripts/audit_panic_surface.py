#!/usr/bin/env python3
"""Audit the panic surface of the pactree engine.

Counts production `unwrap()`, `expect(...)`, and `panic!(...)` call sites and
separates them from test-only call sites. This is the *official* ratchet
source for the hardening plan — raw `rg` is only a discovery helper because it
cannot tell a production panic point from one inside `#[cfg(test)] mod tests`.

What counts as test-only (excluded from the production gate):
  * any file whose path contains `test` (e.g. `*/tests.rs`, `*_test.rs`)
  * any line inside an inline `#[cfg(test)]` module, tracked by brace depth

What is skipped entirely (neither production nor test):
  * generated/data lexpack files (path contains `lexpack`)
  * `//` line comments (stripped before matching)

Usage:
    python scripts/audit_panic_surface.py [--src SRC_DIR] [--json]

Exit code is 0 only when the production panic surface is empty. Test-only
assertions remain inventoried but do not affect the gate.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

# Patterns for the three panic-producing constructs. `.unwrap()` is matched
# exactly so `.unwrap_or(...)`, `.unwrap_or_else(...)`, `.unwrap_err()` etc. do
# NOT count — those do not panic.
PATTERNS = {
    "unwrap": re.compile(r"\.unwrap\(\)"),
    "expect": re.compile(r"\.expect\("),
    "panic": re.compile(r"\bpanic!\("),
}


@dataclass
class Hit:
    path: str
    line_no: int
    kind: str
    text: str


@dataclass
class Counts:
    unwrap: int = 0
    expect: int = 0
    panic: int = 0

    def add(self, kind: str) -> None:
        setattr(self, kind, getattr(self, kind) + 1)

    @property
    def total(self) -> int:
        return self.unwrap + self.expect + self.panic


@dataclass
class Report:
    production: Counts = field(default_factory=Counts)
    test: Counts = field(default_factory=Counts)
    production_hits: list[Hit] = field(default_factory=list)


def strip_line_comment(line: str) -> str:
    """Remove a trailing `//` comment.

    Naive but adequate: a `//` inside a string literal is rare in this
    codebase and only risks under-counting comment-shaped matches, never
    inflating the production count. Block comments are not stripped; the
    constructs we match do not appear in this repo's block comments.
    """
    idx = line.find("//")
    return line[:idx] if idx != -1 else line


def is_test_file(path: Path) -> bool:
    return "test" in path.as_posix().lower()


def is_skipped_file(path: Path) -> bool:
    return "lexpack" in path.as_posix().lower()


def scan_file(path: Path, rel: str, report: Report) -> None:
    """Scan one production-eligible `.rs` file.

    Tracks `#[cfg(test)]` module bodies by brace depth so their contents are
    bucketed as test, not production.
    """
    in_test_mod = False
    test_brace_depth = 0
    pending_cfg_test = False

    for line_no, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        code = strip_line_comment(raw)

        # Detect the `#[cfg(test)]` attribute; the module/fn it guards starts
        # at the next opening brace.
        if not in_test_mod and re.search(r"#\[cfg\(test\)\]", code):
            pending_cfg_test = True

        opens = code.count("{")
        closes = code.count("}")

        if in_test_mod:
            # Count matches on this line as test before adjusting depth.
            _tally(code, report.test, rel, line_no, report, production=False)
            test_brace_depth += opens - closes
            if test_brace_depth <= 0:
                in_test_mod = False
                test_brace_depth = 0
            continue

        if pending_cfg_test and opens > 0:
            # Entering the cfg(test) block on this line. Anything before the
            # brace is negligible; treat the whole line as test.
            pending_cfg_test = False
            in_test_mod = True
            _tally(code, report.test, rel, line_no, report, production=False)
            test_brace_depth = opens - closes
            if test_brace_depth <= 0:
                in_test_mod = False
                test_brace_depth = 0
            continue

        # Production line.
        _tally(code, report.production, rel, line_no, report, production=True)


def _tally(
    code: str,
    counts: Counts,
    rel: str,
    line_no: int,
    report: Report,
    *,
    production: bool,
) -> None:
    for kind, pat in PATTERNS.items():
        for _ in pat.finditer(code):
            counts.add(kind)
            if production:
                report.production_hits.append(
                    Hit(path=rel, line_no=line_no, kind=kind, text=code.strip())
                )


def build_report(src_dir: Path) -> Report:
    report = Report()
    for path in sorted(src_dir.rglob("*.rs")):
        rel = path.relative_to(src_dir.parent).as_posix()
        if is_skipped_file(path):
            continue
        if is_test_file(path):
            # Whole file is test-only; tally everything into the test bucket.
            for line_no, raw in enumerate(
                path.read_text(encoding="utf-8").splitlines(), 1
            ):
                _tally(
                    strip_line_comment(raw),
                    report.test,
                    rel,
                    line_no,
                    report,
                    production=False,
                )
            continue
        scan_file(path, rel, report)
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--src",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "src",
        help="source directory to scan (default: ../src relative to script)",
    )
    parser.add_argument(
        "--json", action="store_true", help="emit machine-readable JSON"
    )
    parser.add_argument(
        "--inventory",
        action="store_true",
        help="print the full production file/line inventory",
    )
    args = parser.parse_args()

    src_dir = args.src.resolve()
    if not src_dir.is_dir():
        print(f"error: source dir not found: {src_dir}", file=sys.stderr)
        return 2

    report = build_report(src_dir)

    if args.json:
        print(
            json.dumps(
                {
                    "production": {
                        "unwrap": report.production.unwrap,
                        "expect": report.production.expect,
                        "panic": report.production.panic,
                        "total": report.production.total,
                    },
                    "test": {
                        "unwrap": report.test.unwrap,
                        "expect": report.test.expect,
                        "panic": report.test.panic,
                        "total": report.test.total,
                    },
                    "production_hits": [
                        {
                            "path": h.path,
                            "line": h.line_no,
                            "kind": h.kind,
                            "text": h.text,
                        }
                        for h in report.production_hits
                    ],
                },
                indent=2,
            )
        )
        return int(report.production.total != 0)

    prod, test = report.production, report.test
    print("pactree panic-surface audit")
    print("=" * 40)
    print(f"source: {src_dir}")
    print()
    print("PRODUCTION (gated):")
    print(f"  panic!()   : {prod.panic}")
    print(f"  .expect()  : {prod.expect}")
    print(f"  .unwrap()  : {prod.unwrap}")
    print(f"  total      : {prod.total}")
    print()
    print("TEST-ONLY (not gated):")
    print(f"  panic!()   : {test.panic}")
    print(f"  .expect()  : {test.expect}")
    print(f"  .unwrap()  : {test.unwrap}")
    print(f"  total      : {test.total}")
    print()

    if args.inventory or report.production_hits:
        print("PRODUCTION INVENTORY:")
        if not report.production_hits:
            print("  (none — clean)")
        else:
            for h in report.production_hits:
                print(f"  {h.path}:{h.line_no}: [{h.kind}] {h.text}")

    return int(prod.total != 0)


if __name__ == "__main__":
    raise SystemExit(main())

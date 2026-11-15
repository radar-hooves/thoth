#!/usr/bin/env python3
"""Headless WebKit layout check for a Tauri front end: the one layout check that
needs no WKWebView, since WKWebView has no external WebDriver.

  nix shell --impure --expr 'with import (builtins.getFlake "nixpkgs") {}; python3.withPackages (p: [p.playwright])' \\
    --command env PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS=1 \\
    python scripts/webkit-layout/run.py http://localhost:5173 --shot /tmp/home.png --eval check.js

Serve the app first (`pnpm dev`). Each --emit "event=json" fires after load, then
--eval's JavaScript file runs in the page and its JSON result is printed.
--invoke "command=json" answers a Tauri command. Exits 1 when the result is false
or an object with "ok": false, so a check can gate.

Reproduce a known failure before trusting a pass: break the layout on purpose,
or check out the commit that had the bug, and see this report it.
"""

import argparse
import json
import sys
from pathlib import Path

from playwright.sync_api import sync_playwright

MOCK = Path(__file__).with_name("tauri-mock.js").read_text()


def pair(text: str) -> tuple[str, object]:
    name, _, value = text.partition("=")
    return name, json.loads(value or "null")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("url")
    ap.add_argument("--width", type=int, default=1200)
    ap.add_argument("--height", type=int, default=800)
    ap.add_argument("--shot", help="screenshot path")
    ap.add_argument("--eval", dest="script", help="JavaScript file; its value is the result")
    ap.add_argument("--invoke", action="append", default=[], metavar="CMD=JSON")
    ap.add_argument("--emit", action="append", default=[], metavar="EVENT=JSON")
    args = ap.parse_args()

    with sync_playwright() as p:
        browser = p.webkit.launch()
        page = browser.new_page(viewport={"width": args.width, "height": args.height})
        page.on("pageerror", lambda e: print(f"pageerror: {e}", file=sys.stderr))
        page.add_init_script(f"window.__MOCK_INVOKE__ = {json.dumps(dict(pair(i) for i in args.invoke))};\n{MOCK}")
        response = page.goto(args.url, wait_until="networkidle")
        if response is None or not response.ok:
            sys.exit(f"{args.url} answered {response and response.status}: serve the app first")
        for emit in args.emit:
            event, payload = pair(emit)
            page.evaluate("([e, p]) => window.__tauriEmit(e, p)", [event, payload])
            page.wait_for_timeout(100)
        result = page.evaluate(Path(args.script).read_text()) if args.script else None
        if args.shot:
            page.screenshot(path=args.shot)
        browser.close()

    print(json.dumps(result, indent=2))
    return 1 if result is False or (isinstance(result, dict) and result.get("ok") is False) else 0


if __name__ == "__main__":
    sys.exit(main())

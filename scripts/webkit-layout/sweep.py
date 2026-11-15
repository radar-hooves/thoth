#!/usr/bin/env python3
"""Layout sweep for a Tauri front end: every view at every window size and UI zoom,
checked for overlap, overflow and clipping by detect.js, in headless WebKit with the
Tauri runtime mocked (tauri-mock.js).

  nix shell --impure --expr 'with import (builtins.getFlake "nixpkgs") {}; python3.withPackages (p: [p.playwright])' \\
    --command env PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS=1 \\
    python scripts/webkit-layout/sweep.py --app scripts/layout-sweep.py --serve "pnpm exec vite dev --port {port} --strictPort"

--app is the app's Python file (below). Give --url for a server already running, or
--serve to start one ({port} is a free port; it is stopped at the end). A view with
findings is screenshotted with the offenders outlined (red = first, yellow = second).
Exits 1 when anything is found. A view whose action raises is reported `unreachable`:
a control a person could not click is itself a finding.

  --sizes 900x600,1190x1880   only these windows (default: the app's SIZES, else DEFAULT_SIZES)
  --zoom 100,130              UI zoom percentages (default: 100,130)
  --views settings            only views whose name contains this
  --all-shots                 screenshot every view, not just the failing ones
  --out DIR                   findings.json and screenshots (default: /tmp/layout-sweep)

The app file is a module; every name but `views` is optional:

  VARIANTS = ["grid", "list"]     states the app is swept in; each gets a fresh page. Default [""].
  SIZES = [(520, 400), ...]       the app's window sizes, its tauri.conf.json minimum first.
  IGNORE = ".search-popup"        CSS selectors of deliberate overlays and bleeds, skipped.
                                  An element can also carry data-sweep-ignore.
  INVOKE = {"get_x": value|fn}    Tauri commands the app answers (tauri-mock.js).
  def init_script(variant, zoom) -> str    JS run before the app loads (seed local storage, data).
  def ready(page, variant)                 after load: emit events, await first paint.
  def views(variant) -> [(name, action)]   action(page) drives the app to that view from any state.
  def set_zoom(page, zoom)                 apply zoom the app's own way; default is CSS zoom on <html>.

Zoom is CSS zoom on the root, which WebKit does not reflect in media or container queries
written in px (they compare against the unzoomed window): write breakpoints in rem so they
follow zoom. detect.js divides the window by the zoom itself.

Reproduce a known failure before trusting a pass: break a layout on purpose and see it flagged.
"""

import argparse
import collections
import importlib.util
import json
import socket
import subprocess
import sys
import time
from pathlib import Path

from playwright.sync_api import sync_playwright

HERE = Path(__file__).resolve().parent
MOCK = (HERE / "tauri-mock.js").read_text()
DETECT = (HERE / "detect.js").read_text()

DEFAULT_SIZES = [(520, 400), (900, 600), (1024, 640), (1200, 630), (1440, 900), (1920, 1080)]
MARK = "window.__sweepMark()"
UNMARK = "window.__sweepEls.flat().forEach(e => { e.style.outline = ''; e.style.outlineOffset = ''; })"


def load_app(path: str):
    spec = importlib.util.spec_from_file_location("layout_sweep_app", path)
    app = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(app)
    return app


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def new_page(browser, app, url, size, variant, zoom):
    ctx = browser.new_context(viewport={"width": size[0], "height": size[1]})
    ctx.add_init_script(f"window.__MOCK_INVOKE__ = {json.dumps(getattr(app, 'INVOKE', {}))};\n{MOCK}")
    ctx.add_init_script(f"window.__SWEEP_IGNORE__ = {json.dumps(getattr(app, 'IGNORE', ''))};")
    if hasattr(app, "init_script"):
        ctx.add_init_script(app.init_script(variant, zoom))
    page = ctx.new_page()
    page.on("pageerror", lambda e: print(f"pageerror: {str(e)[:200]}", file=sys.stderr))
    response = page.goto(url, wait_until="networkidle")
    if response is None or not response.ok:
        sys.exit(f"{url} answered {response and response.status}: serve the app first")
    if hasattr(app, "ready"):
        app.ready(page, variant)
    if hasattr(app, "set_zoom"):
        app.set_zoom(page, zoom)
    elif zoom != 100:
        page.evaluate("z => { document.documentElement.style.zoom = z / 100 }", zoom)
    page.set_default_timeout(4000)
    return ctx, page


def check(page, size, zoom, view, out, all_shots):
    findings = page.evaluate(DETECT)
    shot = None
    if findings or all_shots:
        folder = out / f"{size[0]}x{size[1]}" / f"z{zoom}"
        folder.mkdir(parents=True, exist_ok=True)
        shot = folder / f"{view}.png"
        if findings:
            page.evaluate(MARK)
            page.evaluate("window.__sweepEls[0][0].scrollIntoView({ block: 'center', inline: 'center' })")
        page.screenshot(path=str(shot))
        if findings:
            page.evaluate(UNMARK)
    return [
        dict(f, view=view, size=f"{size[0]}x{size[1]}", zoom=zoom, screenshot=str(shot) if shot else None)
        for f in findings
    ]


def sweep(browser, app, url, args):
    results = []
    for size in args.sizes:
        for zoom in args.zoom:
            for variant in getattr(app, "VARIANTS", [""]):
                views = [(n, a) for n, a in app.views(variant) if args.views in n]
                if not views:
                    continue
                ctx, page = new_page(browser, app, url, size, variant, zoom)
                for name, action in views:
                    try:
                        action(page)
                        page.evaluate("document.fonts.ready")
                        page.wait_for_timeout(250)
                    except Exception as e:
                        results.append(
                            {
                                "kind": "unreachable",
                                "view": name,
                                "size": f"{size[0]}x{size[1]}",
                                "zoom": zoom,
                                "container": "window",
                                "a": {"sel": name, "text": ""},
                                "detail": str(e).splitlines()[0][:120],
                                "screenshot": None,
                            }
                        )
                        continue
                    results += check(page, size, zoom, name, args.out, args.all_shots)
                ctx.close()
            n = sum(1 for r in results if r["size"] == f"{size[0]}x{size[1]}" and r["zoom"] == zoom)
            print(f"{size[0]}x{size[1]} z{zoom}: {n} findings", flush=True)
    return results


def signature(f):
    return (f["kind"], f["view"], f["container"], f["a"]["sel"], f.get("b", {}).get("sel", ""))


def report(results, out):
    (out / "findings.json").write_text(json.dumps(results, indent=1, ensure_ascii=False))
    by_view = collections.Counter(r["view"] for r in results)
    print(f"\n{len(results)} findings in {len(by_view)} views -> {out}/findings.json")
    for view, n in sorted(by_view.items()):
        print(f"  {view}: {n}")
    seen = set()
    for r in results:
        sig = signature(r)
        if sig in seen:
            continue
        seen.add(sig)
        sizes = sorted({f"{x['size']}@{x['zoom']}" for x in results if signature(x) == sig})
        b = f" x {r['b']['sel']} [{r['b']['text']}]" if "b" in r else ""
        more = "..." if len(sizes) > 4 else ""
        print(
            f"- {r['kind']} in {r['view']}: {r['a']['sel']} [{r['a']['text']}]{b} ({r['detail']}) at {', '.join(sizes[:4])}{more}"
        )
        print(f"    {r['screenshot']}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--app", required=True, help="the app's Python file")
    ap.add_argument("--url", help="a running dev server")
    ap.add_argument(
        "--serve", help='command that serves the app on {port}, e.g. "pnpm exec vite dev --port {port} --strictPort"'
    )
    ap.add_argument("--out", type=Path, default=Path("/tmp/layout-sweep"))
    ap.add_argument("--sizes")
    ap.add_argument("--zoom", default="100,130")
    ap.add_argument("--views", default="")
    ap.add_argument("--all-shots", action="store_true")
    args = ap.parse_args()
    if bool(args.url) == bool(args.serve):
        ap.error("give one of --url or --serve")
    app = load_app(args.app)
    sizes = args.sizes.split(",") if args.sizes else None
    args.sizes = [tuple(int(n) for n in s.split("x")) for s in sizes] if sizes else getattr(app, "SIZES", DEFAULT_SIZES)
    args.zoom = [int(z) for z in args.zoom.split(",")]
    args.out.mkdir(parents=True, exist_ok=True)

    server = None
    url = args.url
    if args.serve:
        port = free_port()
        url = f"http://localhost:{port}/"
        server = subprocess.Popen(
            args.serve.format(port=port),
            shell=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
    try:
        if server:
            for _ in range(120):
                try:
                    socket.create_connection(("localhost", port), 1).close()
                    break
                except OSError:
                    time.sleep(0.5)
            else:
                sys.exit(f"--serve did not listen on {port} in 60s")
        with sync_playwright() as p:
            browser = p.webkit.launch()
            results = sweep(browser, app, url, args)
            browser.close()
    finally:
        if server:
            import os
            import signal

            os.killpg(server.pid, signal.SIGTERM)
            server.wait()
    report(results, args.out)
    return 1 if results else 0


if __name__ == "__main__":
    sys.exit(main())

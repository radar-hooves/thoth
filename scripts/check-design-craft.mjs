#!/usr/bin/env node
/**
 * Fail when the frontend carries a craft defect none of the other gates can see.
 *
 * ESLint, svelte-check, stylelint and the type-checker all pass on UI that is
 * still wrong once a human looks at the rendered page: a card nested inside a
 * card, a side-tab accent border, gradient-text on a heading, decorative
 * grid/glow slop, text a user cannot read against its background, content that
 * overflows its box. Its sibling gate (check-ui-drift.mjs) catches UI that
 * hand-rolls what @poodle64/ui already ships; this one catches craft defects in
 * what the app DID compose.
 *
 * It shells out to `impeccable` (github.com/pbakaus/impeccable), an offline
 * anti-pattern detector — no LLM, no API key, no network — built for exactly
 * this: a fixed catalogue of tells mined from what AI-generated UI reliably
 * gets wrong.
 *
 * TWO MODES. Only STATIC is binding (`canonical-app-shape.md`); it is the
 * pre-commit gate. Live mode drives the running app in a real browser and
 * catches contrast, overflow and layout defects that only exist once actual
 * content is laid out — run it deliberately, never from a hook.
 *
 *   pnpm lint:design        static, fast, no browser — the pre-commit gate.
 *   pnpm lint:design:live   drives the app at real viewports. Needs a dev
 *                           server; NOT suitable for pre-commit.
 *
 * Usage:  node scripts/check-design-craft.mjs [--live] [--json] [--baseline]
 * Exit:   0 clean · 1 new finding(s) · 2 could not run
 */

import {
	existsSync,
	readFileSync,
	writeFileSync,
	readdirSync,
	statSync,
	accessSync,
	constants as fsConstants
} from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import os from 'node:os';

// Anchor to the frontend package, not the process cwd. pre-commit runs its
// hooks from the repo root, but `pnpm lint:design` runs from the frontend — the
// script must resolve the same paths either way. The script lives at
// <frontend>/scripts/, so the frontend root is its parent.
//
// INSTALL_DIR is derived the same way its sibling gate derives it, and for the
// same reason: a full-stack app nests the frontend at `frontend/`, a Tauri
// desktop app's SvelteKit app IS the repo root, and a hardcoded nested path
// makes every desktop app hand-edit a factory file over one constant.
const FRONTEND_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const INSTALL_DIR = path.basename(FRONTEND_ROOT) === 'frontend' ? 'frontend' : '.';
const IMPECCABLE = path.join(FRONTEND_ROOT, 'node_modules/.bin/impeccable');
const STATIC_TARGET = path.join(FRONTEND_ROOT, 'src');
const BASELINE = path.join(FRONTEND_ROOT, '.design-craft-baseline.json');
const BASE_URL = process.env.THOTH_URL || 'http://localhost:1422';

// Kept as a plain array, not derived from the route tree, so a new page can be
// added to the live sweep without the script having to know how to walk
// SvelteKit's routing — easy to edit, easy to see what is and is not covered.
const LIVE_ROUTES = ['/'];

// A desktop app's smallest usable window and a large one; it has no phone width.
const LIVE_VIEWPORTS = ['980x700', '1400x1000'];

// ---------------------------------------------------------------------------
// Advisory rules: reported below, but never failing the build and never
// entering the baseline. Roughly two thirds of what such a detector reports on
// a real app is noise, so a version without a hard/advisory split gets
// `--no-verify`'d within a week and then catches nothing — the split is what
// lets this gate survive contact with a real codebase.
//
// Two sources make a finding advisory:
//
//   1. impeccable's OWN `severity: "advisory"` flag (handled below) — the
//      detector already classes some tells as opt-in noise rather than
//      failures. That classification is honoured directly.
//
//   2. This set. It ships with the household-wide prose calls and nothing
//      else. The three below are conventions the household writes to on
//      purpose (dictated prose reaches the UI as-is), so they are a detector
//      disagreement with house style, not a UI defect — the same call every
//      app would otherwise re-derive alone.
//
// An app-measured false positive is added here with its reason and whether it
// is a taste call this app owns or a measured detector failure — a future
// editor needs to know which before deciding whether the entry still belongs.
// Do NOT copy another app's suppressions: a suppression argued on another
// app's measurements is an assumption here.
// ---------------------------------------------------------------------------
const ADVISORY_RULES = new Set(['em-dash-overuse', 'marketing-buzzword', 'aphoristic-cadence']);

if (!existsSync(IMPECCABLE)) {
	console.error(
		`impeccable not installed at ${path.relative(FRONTEND_ROOT, IMPECCABLE)} — run pnpm install in ${INSTALL_DIR} first.`
	);
	process.exit(2);
}

const args = process.argv.slice(2);
const mode = args.includes('--live') ? 'live' : 'static';
const asJson = args.includes('--json');
const writeBaseline = args.includes('--baseline');

// ---------------------------------------------------------------------------
// Browser resolution (live mode only).
//
// impeccable's live scan runs on puppeteer, but this repo deliberately never
// runs puppeteer's own Chromium download — that would be a second copy of the
// browser Playwright already fetched for the frontend's E2E suite. Instead find
// that copy and point PUPPETEER_EXECUTABLE_PATH at it. The exact subpath varies
// by platform and CPU architecture, and the highest-numbered build changes on
// every Playwright update, so walk rather than hardcode.
// ---------------------------------------------------------------------------

function isExecutable(candidate) {
	try {
		accessSync(candidate, fsConstants.X_OK);
		return statSync(candidate).isFile();
	} catch {
		return false;
	}
}

const EXECUTABLE_NAMES = new Set(['Google Chrome for Testing', 'chrome', 'chromium']);

function findExecutableUnder(dir, depth = 0) {
	if (depth > 5) return null;
	let entries;
	try {
		entries = readdirSync(dir, { withFileTypes: true });
	} catch {
		return null;
	}
	for (const entry of entries) {
		const full = path.join(dir, entry.name);
		if (entry.isDirectory()) {
			const found = findExecutableUnder(full, depth + 1);
			if (found) return found;
		} else if (EXECUTABLE_NAMES.has(entry.name) && isExecutable(full)) {
			return full;
		}
	}
	return null;
}

function resolveBrowserExecutable() {
	const configured = process.env.PUPPETEER_EXECUTABLE_PATH;
	if (configured && isExecutable(configured)) return configured;

	const browsersDir = [
		process.env.PLAYWRIGHT_BROWSERS_PATH,
		path.join(os.homedir(), 'Library', 'Caches', 'ms-playwright'),
		path.join(os.homedir(), '.cache', 'ms-playwright')
	].find((dir) => dir && existsSync(dir));
	if (!browsersDir) return null;

	const chromiumBuilds = readdirSync(browsersDir)
		.filter((d) => /^chromium-\d+$/.test(d))
		.sort((a, b) => Number(b.split('-')[1]) - Number(a.split('-')[1]));

	for (const build of chromiumBuilds) {
		const found = findExecutableUnder(path.join(browsersDir, build));
		if (found) return found;
	}
	return null;
}

// ---------------------------------------------------------------------------
// Run impeccable and normalise its result.
//
// impeccable exits non-zero whenever it finds ANYTHING (advisory findings
// included) and 0 only on a clean scan — the opposite of the usual convention,
// and not a signal about whether the RUN succeeded. It also degrades silently:
// point it at an unreachable target and it can print an `Error:` line to
// stderr, exit 0, and hand back `[]` — exactly the shape of a genuinely clean
// scan. So the run's own exit code is not trusted at all; success is: the
// process spawned, stderr is empty, and stdout parses as a JSON array. Anything
// else is "could not run", never silently reported as clean.
// ---------------------------------------------------------------------------

function runImpeccable(cliArgs, envOverrides = {}) {
	const result = spawnSync(IMPECCABLE, cliArgs, {
		encoding: 'utf8',
		env: { ...process.env, ...envOverrides },
		maxBuffer: 1024 * 1024 * 20
	});
	if (result.error) {
		return { ok: false, message: `could not run impeccable: ${result.error.message}` };
	}
	const stderr = (result.stderr || '').trim();
	if (stderr) {
		return { ok: false, message: stderr };
	}
	let parsed;
	try {
		parsed = JSON.parse(result.stdout);
	} catch (err) {
		return { ok: false, message: `impeccable produced invalid JSON: ${err.message}` };
	}
	if (!Array.isArray(parsed)) {
		return {
			ok: false,
			message: 'impeccable produced an unexpected JSON shape (expected an array)'
		};
	}
	return { ok: true, findings: parsed };
}

const oneLine = (value, max = 180) => {
	const collapsed = String(value ?? '')
		.replace(/\s+/g, ' ')
		.trim();
	return collapsed.length > max ? `${collapsed.slice(0, max - 1)}…` : collapsed;
};

// ---------------------------------------------------------------------------
// Collect raw findings for the selected mode.
// ---------------------------------------------------------------------------

let raw = [];

if (mode === 'static') {
	const res = runImpeccable(['detect', STATIC_TARGET, '--json']);
	if (!res.ok) {
		console.error(`Static scan failed: ${res.message}`);
		process.exit(2);
	}
	raw = res.findings.map((f) => ({ ...f, target: path.relative(FRONTEND_ROOT, f.file) }));
} else {
	// Fail loud on an unreachable app rather than let a `[]` from a refused
	// connection read as a clean pass — see the runImpeccable comment above.
	try {
		await fetch(BASE_URL, { signal: AbortSignal.timeout(5000) });
	} catch (err) {
		console.error(
			`Cannot reach ${BASE_URL} — is the dev server running? (pnpm dev)\n${err.message}`
		);
		process.exit(2);
	}

	const execPath = resolveBrowserExecutable();
	if (!execPath) {
		console.error(
			'No Chromium executable found for impeccable’s live scan (checked PUPPETEER_EXECUTABLE_PATH and the Playwright cache).\n' +
				`Run: pnpm exec playwright install chromium (from ${INSTALL_DIR})`
		);
		process.exit(2);
	}

	for (const route of LIVE_ROUTES) {
		const url = new URL(route, BASE_URL).toString();
		for (const viewport of LIVE_VIEWPORTS) {
			const res = runImpeccable(['detect', url, '--viewport', viewport, '--json'], {
				PUPPETEER_EXECUTABLE_PATH: execPath
			});
			if (!res.ok) {
				console.error(`Live scan failed at ${route} (${viewport}): ${res.message}`);
				process.exit(2);
			}
			// target is the route we asked for, not impeccable's own `file` (the full
			// scanned URL) — a defect present at one viewport is the same defect at
			// the other, and the baseline key must not care which one found it.
			for (const f of res.findings) raw.push({ ...f, target: route });
		}
	}
}

const normalised = raw.map((f) => ({
	rule: f.antipattern,
	target: f.target,
	detail: oneLine(f.snippet ? `${f.name} — ${f.snippet}` : f.name || f.description),
	// A finding is advisory if impeccable itself marks it so, or if this app has
	// argued the rule down (ADVISORY_RULES). Advisory findings are reported,
	// never fail, never baselined.
	advisory: f.severity === 'advisory' || ADVISORY_RULES.has(f.antipattern)
}));

const findings = normalised.filter((f) => !f.advisory);
const advisory = normalised.filter((f) => f.advisory);

// ---------------------------------------------------------------------------
// Baseline: gate on NEW findings, not on the backlog — same discipline as
// check-ui-drift.mjs, for the same reason. A gate that fails on the day it
// lands gets disabled within a week and then catches nothing forever.
//
// The key is `${rule}:${target}` — deliberately NOT line number or selector. A
// line number shifts on every unrelated edit above it; a selector churns on a
// class-name refactor. Either would make the gate re-flag a finding that never
// actually changed, which is how a baseline gate loses trust and gets
// `--no-verify`'d.
//
// Static and live findings share ONE baseline file, kept apart by a `live:`
// prefix so the two namespaces cannot collide. `--baseline` rewrites only the
// keys for the mode being run and leaves the other mode's entries untouched:
// running `--baseline` in static mode must not erase live entries banked from
// a previous live run just because this run never looked at live mode.
// ---------------------------------------------------------------------------

// Granularity limit, known and accepted rather than an oversight: this key is
// target-level, not instance-level, so a target already on the register for
// `nested-cards` can accumulate more instances of it invisibly. An
// instance-level key (selector, snippet) would churn on every cosmetic edit
// near a flagged element, which is exactly the noise a baseline exists to
// avoid. The trade is accepted because a craft defect class, unlike a missing
// composed component, is rarely fixed one instance at a time — it is fixed by
// revisiting the whole target. Re-open this if that stops being true.
function key(finding) {
	return mode === 'live'
		? `live:${finding.rule}:${finding.target}`
		: `${finding.rule}:${finding.target}`;
}

function readBaselineKeys() {
	if (!existsSync(BASELINE)) return new Set();
	let parsed;
	try {
		parsed = JSON.parse(readFileSync(BASELINE, 'utf8'));
	} catch (err) {
		console.error(`${path.relative(FRONTEND_ROOT, BASELINE)} is not valid JSON: ${err.message}`);
		process.exit(2);
	}
	if (!Array.isArray(parsed)) {
		console.error(`${path.relative(FRONTEND_ROOT, BASELINE)} does not contain a JSON array.`);
		process.exit(2);
	}
	return new Set(parsed);
}

const isLiveKey = (k) => k.startsWith('live:');
const allBaseline = readBaselineKeys();
const modeKnown = new Set([...allBaseline].filter((k) => isLiveKey(k) === (mode === 'live')));

if (writeBaseline) {
	const otherModeKeys = [...allBaseline].filter((k) => isLiveKey(k) !== (mode === 'live'));
	const thisModeKeys = [...new Set(findings.map(key))];
	const combined = [...new Set([...otherModeKeys, ...thisModeKeys])].sort();
	writeFileSync(BASELINE, `${JSON.stringify(combined, null, 2)}\n`);
	const otherLabel = mode === 'live' ? 'static' : 'live';
	console.log(
		`Baseline written (${mode} mode): ${thisModeKeys.length} known ${mode} finding(s); ` +
			`${otherModeKeys.length} ${otherLabel} finding(s) preserved untouched. ` +
			`${combined.length} total in ${path.relative(FRONTEND_ROOT, BASELINE)}`
	);
	process.exit(0);
}

const fresh = findings.filter((f) => !modeKnown.has(key(f)));
const fixed = [...modeKnown].filter((k) => !findings.some((f) => key(f) === k));

if (asJson) {
	console.log(
		JSON.stringify(
			{ mode, fresh, grandfathered: findings.length - fresh.length, fixed, advisory },
			null,
			2
		)
	);
	process.exit(fresh.length ? 1 : 0);
}

for (const f of fresh) console.error(`${f.target}\n  [${f.rule}] ${f.detail}`);
if (fresh.length) {
	console.error(`\n${fresh.length} NEW design-craft finding(s) (${mode} mode).`);
	console.error(
		'Fix it, or if it is a false positive for this app, argue the case and add it to ADVISORY_RULES with a reason.'
	);
} else {
	console.log(
		`No new design-craft findings in ${mode} mode. (${modeKnown.size} known, grandfathered.)`
	);
}
if (fixed.length) {
	console.log(`\n${fixed.length} baseline finding(s) fixed — rerun with --baseline to bank it:`);
	for (const k of fixed) console.log(`  ${k}`);
}
if (advisory.length) {
	console.log(
		`\nFYI — ${advisory.length} advisory finding(s) (reported, never failing, never baselined):`
	);
	for (const f of advisory) console.log(`  ${f.target} [${f.rule}] ${f.detail}`);
}

process.exit(fresh.length ? 1 : 0);

#!/usr/bin/env node
/**
 * Fail when the app hand-rolls something the shared design system already ships.
 *
 * This catches what no other gate can see. A hand-written card compiles,
 * renders, type-checks and passes its tests; it is only wrong once a human
 * opens it on a real display and sees that it does not match the rest of the
 * app. Its sibling gate (check-design-craft.mjs) catches craft defects in what
 * the app DID compose; this one catches the app composing nothing at all where
 * the package ships the answer.
 *
 * `canonical-app-shape.md` makes this gate binding on every full-stack app.
 *
 * FOUR RULES:
 *   1. vendored-copy — a local component whose name matches one @poodle64/ui
 *      ships, and which does not delegate to it; or a `components/ui/`
 *      primitive the package has since shipped.
 *   2. hand-rolled-page-title — a route drawing its own title (an <h1> or
 *      PageHeader) over the rail's, which names the page.
 *   3. surface-brief-divergence — a route composing a package component its
 *      surface brief does not name. Inert until the app grows
 *      `docs/product/surfaces/`; surface briefs are optional, so an app
 *      without them never sees this rule fire.
 *   4. viewport-variants — the app's count of viewport breakpoint variants
 *      (`hidden md:flex`, `lg:grid-cols-3`) rising above its banked count.
 *
 * Usage:  node scripts/check-ui-drift.mjs [--json] [--baseline]
 * Exit:   0 clean · 1 new drift · 2 could not run
 */

import { readdirSync, readFileSync, existsSync, statSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

// Anchor to the frontend package, not the process cwd. pre-commit runs its
// hooks from the repo root, but `pnpm lint:drift` runs from the frontend — the
// script must resolve the same paths either way. The script lives at
// <frontend>/scripts/, so the frontend root is its parent.
//
// Where that frontend sits is the one thing that differs by archetype, so it is
// DERIVED rather than assumed. A full-stack app nests it at `frontend/`, a name
// the canonical shape mandates, so the basename is a reliable signal. A Tauri
// desktop app has no server to sit beside: its SvelteKit app IS the repo root,
// and there is no enclosing directory to step up into. Hardcoding the nested
// case made every desktop app hand-edit this line, which forks a factory file
// over one constant — exactly what an extension point exists to prevent.
const FRONTEND_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const REPO_ROOT =
	path.basename(FRONTEND_ROOT) === 'frontend' ? path.resolve(FRONTEND_ROOT, '..') : FRONTEND_ROOT;
// Where to tell someone to run pnpm: `frontend` when nested, `.` when the
// frontend is the repo root.
const INSTALL_DIR = path.relative(REPO_ROOT, FRONTEND_ROOT) || '.';
const UI_DIST = path.join(FRONTEND_ROOT, 'node_modules/@poodle64/ui/dist/components/ui');
const SRC = path.join(FRONTEND_ROOT, 'src');
const ROUTES = path.join(SRC, 'routes');
const BASELINE = path.join(FRONTEND_ROOT, '.ui-drift-baseline.json');
const SURFACES_DIR = path.join(REPO_ROOT, 'docs/product/surfaces');

if (!existsSync(UI_DIST)) {
	console.error(
		`@poodle64/ui not installed at ${path.relative(FRONTEND_ROOT, UI_DIST)} — run pnpm install in ${INSTALL_DIR} first.`
	);
	process.exit(2);
}

const shipped = new Set(
	readdirSync(UI_DIST).filter((d) => statSync(path.join(UI_DIST, d)).isDirectory())
);

/** kebab-case a PascalCase component name, to compare against a package subpath. */
const kebab = (name) => name.replace(/([a-z0-9])([A-Z])/g, '$1-$2').toLowerCase();

const kebabToPascal = (k) =>
	k
		.split('-')
		.map((s) => s[0].toUpperCase() + s.slice(1))
		.join('');

function walk(dir, out = []) {
	if (!existsSync(dir)) return out;
	for (const entry of readdirSync(dir, { withFileTypes: true })) {
		const full = path.join(dir, entry.name);
		if (entry.isDirectory()) walk(full, out);
		else out.push(full);
	}
	return out;
}

const files = walk(SRC);
const rel = (f) => path.relative(FRONTEND_ROOT, f);
const findings = [];

// ---------------------------------------------------------------------------
// 1. A local component with the same name as one the package ships.
//
// `sveltekit-frontend.md` forbids this outright, and the reason is not
// tidiness: a vendored copy cannot receive an upstream fix. It also drifts —
// the copy grows a prop, the shared one grows a different one, and the two
// diverge silently because nothing compares them.
//
// A local file that IMPORTS the shipped component of the same name is excluded,
// and that distinction is the rule rather than a hole in it: a DELEGATING
// composition is the opposite of a fork — a thin wrapper that adapts the
// package's API to a call shape this app repeats. A genuine fork cannot pass
// the test, because it does not import the thing it forked.
//
// `components/ui/<name>/` is judged as a whole instead: it holds a shadcn
// primitive this app vendored because the package did not ship <name>, and
// that reason expires the day the package does. A primitive is the package's
// to own, so there is no delegating exception; a part the local copy has and
// the package lacks is a need filed with the package. Measured 23/09/2026: four
// apps still carried `form` and a fifth `dropdown-menu`, `popover` and
// `switch`, all shipped since, while this rule skipped the directory entirely.
// ---------------------------------------------------------------------------
const UI_DIR = `${path.sep}components${path.sep}ui${path.sep}`;
const vendoredPrimitives = new Set();
for (const f of files) {
	const at = f.indexOf(UI_DIR);
	if (at === -1) continue;
	const [name, ...rest] = f.slice(at + UI_DIR.length).split(path.sep);
	if (rest.length && shipped.has(name))
		vendoredPrimitives.add(f.slice(0, at + UI_DIR.length) + name);
}
for (const dir of vendoredPrimitives) {
	const name = path.basename(dir);
	findings.push({
		rule: 'vendored-copy',
		file: rel(dir),
		detail: `local ${name} primitive duplicates @poodle64/ui/${name}, which ships it now; import the shipped one`
	});
}

for (const f of files.filter((f) => f.endsWith('.svelte'))) {
	if (f.includes(UI_DIR)) continue;
	const name = path.basename(f, '.svelte');
	if (!shipped.has(kebab(name))) continue;
	const src = readFileSync(f, 'utf8');
	if (new RegExp(`from\\s+['"]@poodle64/ui/${kebab(name)}['"]`).test(src)) continue;
	findings.push({
		rule: 'vendored-copy',
		file: rel(f),
		detail: `local ${name} duplicates @poodle64/ui/${kebab(name)}; import the shipped one`
	});
}

// ---------------------------------------------------------------------------
// 2. A route page that draws its own title.
//
// Under the one shell the rail names the
// page: the breadcrumbs' last crumb is the title and the shell writes the one
// hidden <h1>. A page that writes an <h1>, or composes PageHeader (which draws
// one, and a trail, over the rail's), draws a second title. A section heading
// (<h2>, <h3>) is the page's own content and is right; the lead sentence is
// PageLead's. Children are read one level deep, as rule 3 reads them.
// ---------------------------------------------------------------------------
const H1 = /<h1[\s>]/;
const PAGE_HEADER = /<PageHeader[\s>/]/;

for (const f of files.filter((f) => path.basename(f) === '+page.svelte')) {
	const src = readFileSync(f, 'utf8');
	const drawn = [src, ...childComponentFiles(src).map((c) => readFileSync(c, 'utf8'))];
	if (drawn.some((s) => H1.test(s) || PAGE_HEADER.test(s))) {
		findings.push({
			rule: 'hand-rolled-page-title',
			file: rel(f),
			detail:
				'draws its own title (an <h1> or PageHeader); the rail names the page from `title` in the route load data. Lead with PageLead, put actions in ShellActions'
		});
	}
}

// ---------------------------------------------------------------------------
// 3. A route composing a package component its surface brief does not name.
//
// A surface brief records the presentation decision for one route, naming the
// composed vocabulary it reasoned its way to. Without this check a brief can be
// edited, or a route can quietly grow a new component, and the two drift with
// nothing noticing.
//
// Inert until the app grows `docs/product/surfaces/`. Surface briefs are
// optional, so an app that does not keep them never sees this rule fire — but
// an app that adopts them gets the gate without re-deriving it.
//
// Primitives are free: a brief is a presentation decision, and reaching for a
// Button is not one. Only a COMPOSED component counts.
// ---------------------------------------------------------------------------
const PRIMITIVES = new Set([
	'alert',
	'alert-dialog',
	'avatar',
	'badge',
	'button',
	'card',
	'checkbox',
	'command',
	'data-table',
	'dialog',
	'dropdown-menu',
	'input',
	'input-group',
	'label',
	'password-input',
	'popover',
	'progress',
	'select',
	'separator',
	'sheet',
	'skeleton',
	'sonner',
	'switch',
	'table',
	'tabs',
	'textarea',
	'tooltip'
]);

const surfaceFiles = existsSync(SURFACES_DIR)
	? readdirSync(SURFACES_DIR).filter((f) => f.endsWith('.md') && f !== 'README.md')
	: [];

// route -> +page.svelte file. A route group is a parenthesised directory name
// (`(protected)`, `(admin)`) that organises files on disk without appearing in
// the URL, so it has to be stripped before a brief's `route:` (which names the
// URL) can be matched against a filesystem path.
const routeToPageFile = new Map();
for (const f of files.filter((f) => path.basename(f) === '+page.svelte')) {
	const relDir = path.relative(ROUTES, path.dirname(f));
	const segments = relDir.split(path.sep).filter((s) => s && !/^\(.*\)$/.test(s));
	routeToPageFile.set('/' + segments.join('/'), f);
}

/** Pull the named `## Heading` section's body out of a brief's markdown. */
function briefSection(md, heading) {
	const lines = md.split('\n');
	const start = lines.findIndex((l) => l.trim() === `## ${heading}`);
	if (start === -1) return '';
	let end = lines.findIndex((l, idx) => idx > start && /^## /.test(l));
	if (end === -1) end = lines.length;
	return lines.slice(start + 1, end).join('\n');
}

/**
 * The brief's declared vocabulary: shipped component names named anywhere in
 * `## The decision`, not only in a closing "Composed vocabulary:" line. A brief
 * reasons about a component inline well before it reaches that summary
 * sentence, and relying on the sentence alone would miss a brief that names a
 * component only in prose.
 */
function declaredVocabulary(decisionSection) {
	const declared = new Set();
	const backtickRe = /`([^`]+)`/g;
	let m;
	while ((m = backtickRe.exec(decisionSection))) {
		for (const candidate of m[1].split(',')) {
			const k = kebab(candidate.trim());
			if (shipped.has(k)) declared.add(k);
		}
	}
	return declared;
}

/**
 * Every COMPOSED `@poodle64/ui` component a source file imports. Skips
 * `import type` — a type import composes nothing on screen, and several shipped
 * directories export a type of the same name.
 */
function composedImports(src) {
	const found = new Set();
	const importRe = /^[ \t]*import\s+(type\s+)?[^\n]*?from\s+['"]@poodle64\/ui\/([a-z0-9-]+)['"]/gm;
	let m;
	while ((m = importRe.exec(src))) {
		if (m[1]) continue;
		const component = m[2];
		if (shipped.has(component) && !PRIMITIVES.has(component)) found.add(component);
	}
	return found;
}

/**
 * A page's own presentation frequently lives one level down, not in the route
 * file itself. Only direct children (`#lib` imports resolving to
 * a `.svelte` file) are followed; a grandchild is out of scope, matching "one
 * level deep" in the brief contract.
 */
function childComponentFiles(pageSrc) {
	const out = [];
	const importRe = /from\s+['"](#lib\/[^'"]+)['"]/g;
	let m;
	while ((m = importRe.exec(pageSrc))) {
		const spec = m[1];
		if (!spec.endsWith('.svelte')) continue;
		const relPath = spec.replace('#lib/', 'lib/');
		const full = path.join(SRC, relPath);
		if (existsSync(full)) out.push(full);
	}
	return out;
}

const staleBriefs = [];

for (const surfaceFile of surfaceFiles) {
	const md = readFileSync(path.join(SURFACES_DIR, surfaceFile), 'utf8');
	const routeMatch = md.match(/^route:\s*(\S+)/m);
	if (!routeMatch) continue;
	const route = routeMatch[1];
	const pageFile = routeToPageFile.get(route);

	if (!pageFile) {
		staleBriefs.push(`docs/product/surfaces/${surfaceFile} (route: ${route})`);
		continue;
	}

	const declared = declaredVocabulary(briefSection(md, 'The decision'));
	const pageSrc = readFileSync(pageFile, 'utf8');
	const sources = [pageSrc, ...childComponentFiles(pageSrc).map((f) => readFileSync(f, 'utf8'))];

	const composed = new Set();
	for (const src of sources) {
		for (const c of composedImports(src)) composed.add(c);
	}

	for (const c of [...composed].sort()) {
		if (declared.has(c)) continue;
		findings.push({
			rule: 'surface-brief-divergence',
			file: rel(pageFile),
			// Carried as its own field, not just baked into `detail`, because the
			// baseline key needs it too — see the key() comment below.
			component: kebabToPascal(c),
			detail: `composes ${kebabToPascal(c)}, which docs/product/surfaces/${surfaceFile} does not name; update the brief first if the presentation decision has changed`
		});
	}
}

// ---------------------------------------------------------------------------
// 4. Viewport variants: one count per app, which only falls.
//
// A route renders inside the shell's content panel, which beside a sidebar or
// a second column runs far narrower than the window. A viewport breakpoint
// (`hidden lg:table-cell`, `md:grid-cols-3`) fires on the window, so it reveals
// or splits content in a panel a third that size. A layout inside the shell
// gates on its panel with a container query instead (`@container` on the
// wrapper, `@lg:` on the cell), which is not counted. The first run banks the
// app's count as `viewport-variants:<n>`; from then on it may fall, and each
// fall is banked, but it never rises.
// ---------------------------------------------------------------------------
const VIEWPORT_VARIANT =
	/(?<![\w@-])(?:(?:max-|min-)?(?:sm|md|lg|xl|2xl)|(?:max|min)-\[[^\]\s]+\]):(?=[!\w[-])/g;
const viewportVariants = files
	.filter((f) => f.endsWith('.svelte'))
	.reduce((n, f) => n + (readFileSync(f, 'utf8').match(VIEWPORT_VARIANT)?.length ?? 0), 0);

// ---------------------------------------------------------------------------
// Informational: shipped components this app never imports.
//
// NOT a failure — plenty are legitimately unneeded. It is printed so the list
// can be read the way an audit reads it: for each one, "do we hand-roll that?"
// ---------------------------------------------------------------------------
const allSource = files
	.filter((f) => f.endsWith('.svelte') || f.endsWith('.ts'))
	.map((f) => readFileSync(f, 'utf8'))
	.join('\n');
const unused = [...shipped]
	.filter((c) => !new RegExp(`@poodle64/ui/${c}\\b`).test(allSource))
	.sort();

// ---------------------------------------------------------------------------
// Baseline: gate on NEW drift, not on the backlog.
//
// A gate that fails on the day it lands gets disabled within a week, and then
// catches nothing forever. Grandfathering what already exists and failing only
// on what a change ADDS is what makes it survivable. A freshly stamped app
// ships this baseline EMPTY: the scaffold composes what the package ships, so
// there is nothing to grandfather. It is in copier's `_skip_if_exists`, so
// `copier update` never wipes the debt an app has since banked.
//
// The baseline is a debt register, not an amnesty, and it only shrinks:
// `--baseline` drops what has been fixed and banks a fall in the viewport
// count, but never banks a fresh finding or a rise. A finding the app must keep
// is added by hand, a diff a reviewer reads. Only an app with no baseline file
// at all banks its whole backlog with `--baseline`, once.
// ---------------------------------------------------------------------------
//
// The key is `${rule}:${file}` — deliberately NOT a line number or a selector,
// either of which churns on unrelated edits and re-flags a finding that never
// changed, which is how a baseline gate loses trust and gets bypassed.
//
// The granularity is deliberately NOT uniform across rules; do not "tidy" it
// back. `vendored-copy` and `hand-rolled-page-title` are one-finding-per-file
// by construction, so `${rule}:${file}` uniquely identifies each. But one page
// can compose several undeclared components at once, so keying
// `surface-brief-divergence` the same way collapses them into one entry:
// banking any one silently banks the rest, and a FURTHER divergence on an
// already-flagged page produces no fresh finding at all. That is a fail-open —
// the gate goes quiet exactly when a new divergence lands. Its key therefore
// carries the component name.
const key = (f) =>
	f.rule === 'surface-brief-divergence'
		? `${f.rule}:${f.file}:${f.component}`
		: `${f.rule}:${f.file}`;

const VIEWPORT_KEY = 'viewport-variants:';
const BASELINE_NAME = path.relative(FRONTEND_ROOT, BASELINE);
const hasBaseline = existsSync(BASELINE);
const known = new Set(hasBaseline ? JSON.parse(readFileSync(BASELINE, 'utf8')) : []);
const viewportEntry = [...known].find((k) => k.startsWith(VIEWPORT_KEY));
const viewportBanked =
	viewportEntry === undefined ? undefined : Number(viewportEntry.slice(VIEWPORT_KEY.length));
known.delete(viewportEntry);
const fresh = findings.filter((f) => !known.has(key(f)));
const fixed = [...known].filter((k) => !findings.some((f) => key(f) === k));

const writeBaseline = (keys, viewport) =>
	writeFileSync(
		BASELINE,
		JSON.stringify([...new Set(keys), `${VIEWPORT_KEY}${viewport}`].sort(), null, 2) + '\n'
	);

if (process.argv.includes('--baseline')) {
	const keep = findings.map(key).filter((k) => !hasBaseline || known.has(k));
	const viewport = Math.min(viewportVariants, viewportBanked ?? viewportVariants);
	writeBaseline(keep, viewport);
	console.log(
		`Baseline written: ${new Set(keep).size} known finding(s) and ${viewport} viewport variant(s) in ${BASELINE_NAME}`
	);
	const unbanked = [
		...(hasBaseline ? fresh : []).map((f) => `${f.file} [${f.rule}]`),
		...(viewportVariants > viewport ? [`${viewportVariants - viewport} viewport variant(s)`] : [])
	];
	for (const u of unbanked) console.error(`Not banked, fix it: ${u}`);
	process.exit(unbanked.length ? 1 : 0);
}

const viewportProblem =
	viewportBanked === undefined || viewportVariants === viewportBanked
		? null
		: viewportVariants > viewportBanked
			? `${viewportVariants} viewport variants against ${viewportBanked} banked; inside the shell, gate a layout on its panel with a container query (@container, @lg:), not a viewport breakpoint`
			: `${viewportVariants} viewport variants against ${viewportBanked} banked; rerun with --baseline to bank the fall`;
const failed = fresh.length > 0 || viewportProblem !== null;

if (process.argv.includes('--json')) {
	console.log(
		JSON.stringify(
			{
				fresh,
				grandfathered: findings.length - fresh.length,
				fixed,
				viewportVariants: { count: viewportVariants, banked: viewportBanked ?? null },
				unusedShippedComponents: unused,
				staleBriefs
			},
			null,
			2
		)
	);
	process.exit(failed ? 1 : 0);
}

if (viewportBanked === undefined) {
	writeBaseline(known, viewportVariants);
	console.log(
		`Banked ${viewportVariants} viewport variant(s) in ${BASELINE_NAME} on this first run; commit it.\n`
	);
}

for (const f of fresh) console.error(`${f.file}\n  [${f.rule}] ${f.detail}`);
if (viewportProblem) console.error(`${rel(SRC)}\n  [viewport-variants] ${viewportProblem}`);
if (fresh.length) {
	console.error(`\n${fresh.length} NEW design-system drift finding(s).`);
	console.error('Compose what the package ships; a local copy cannot receive an upstream fix.');
} else if (!viewportProblem) {
	console.log(
		`No new design-system drift. (${known.size} known, grandfathered; ${viewportVariants} viewport variant(s).)`
	);
}
if (fixed.length) {
	console.log(`\n${fixed.length} baseline finding(s) fixed — rerun with --baseline to bank it:`);
	for (const k of fixed) console.log(`  ${k}`);
}
if (unused.length) {
	console.log(`\nFYI — ${unused.length} shipped components this app never imports:`);
	console.log(`  ${unused.join(', ')}`);
	console.log('  Worth a glance: is any of them something a page here hand-rolls?');
}
if (staleBriefs.length) {
	console.log(`\nFYI — ${staleBriefs.length} surface brief(s) whose route no longer exists:`);
	for (const s of staleBriefs) console.log(`  ${s}`);
	console.log('  A brief for a route that has gone is a decision nobody is reading any more.');
}

process.exit(failed ? 1 : 0);

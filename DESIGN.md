---
name: Thoth
description: "Private, offline voice transcription for macOS and Linux: press a key, speak, and the text lands at your cursor"
version: 2026.8.1
status: "on @poodle64/design-tokens and @poodle64/ui, with the drift and craft gates holding it"
---

## Current state

Thoth uses a dark-only theme called **Scribe Amber**, wired onto `@poodle64/design-tokens` + `@poodle64/ui` in `src/app.css`. The palette is built around a warm amber primary (`oklch(0.693 0.124 65.9)`), dark brown backgrounds (deepest at `oklch(0.223 0.002 67.7)`), and muted stone neutrals — all still expressed in OKLCH, but now as a **full surface-ladder override** of the shared `--ds-color-*` tokens rather than a hand-rolled shadcn alias layer (see the block comment at the top of `app.css` for the rationale and the one visible drift this introduced: `--accent` moved from the old muted tier to the card tier). The radius base now resolves to `var(--ds-radius-lg)` (0.625rem, up from the previous hand-rolled `0.5rem`) via `@poodle64/ui/styles.css`. Thoth has no light mode (`ModeWatcher` defaults to dark, no toggle exists), so `:root` and `.dark` carry the identical override.

### Typography (corrected 27/08/2026)

This section previously said the shared faces were "not yet applied". That was wrong for two of the three, and the correction is recorded here rather than quietly swapped, because it was read as outstanding work for a month.

- **Body — applied, and it always was.** `tokens.tw.css` maps `--font-sans` to `--ds-font-body`, and Tailwind v4's preflight resolves `html { font-family: var(--default-font-family) }` through it. The body face therefore arrived with the token adoption itself, with nothing to switch on. Verified in the built stylesheet, not inferred: `--default-font-family: var(--font-sans)` → `--font-sans: var(--ds-font-body)` → `"Avenir Next", "Hanken Grotesk", …`.
- **Code — applied.** Tailwind's `font-mono` resolves to `--ds-font-code` and is used across seven files. Four rules had hand-written their own monospace stack instead, in three mutually disagreeing spellings; all four now read `var(--ds-font-code)`.
- **Display (Fraunces) — genuinely unapplied.** Nothing in Thoth uses `font-display` or `font-serif`: the app has no display typography to put it on. This is the one real gap, and it is a design decision (what earns a display face in a settings workbench) rather than a wiring task.

None of the three needs a bundled font file, which matters for an app that must work with no network: every stack degrades through a system face.

## Shared baseline

The household design language and binding constants live in two places:

- `docs/master/reference/guide-shared-design-language.md` -- canonical source for colour space (OKLCH), radius (`--ds-radius-lg: 0.625rem`), typography (Fraunces display, Hanken Grotesk body, JetBrains Mono code), status vocabulary (success / warning / error / info / neutral), spacing, and namespace (`--ds-*`).
- `@poodle64/design-tokens` (public npm) -- the compiled CSS custom-property package that projects consume. Provides the `--ds-*` token set and the wiring snippet for `app.css`.
- `@poodle64/ui` (public npm) -- the shared shadcn-svelte component layer. Provides the shadcn semantic surface registration (`--card`, `--popover`, `--muted`, …) plus the composed page-chrome components.

## Component adoption

Thoth's vendored `src/lib/components/ui/` primitives with a `@poodle64/ui` equivalent now import from the package. Three primitives stay local because the package genuinely does not ship them:

- **`form`** -- deliberately excluded upstream (a Formsnap/Superforms binding layer is an application-architecture choice, not a design one; see `@poodle64/ui`'s README).
- **`radio-group`**, **`context-menu`** -- not yet added to the shared package (no app has migrated them upstream yet); Thoth's local copies are not drift, per `20-sveltekit-frontend.md`'s "genuinely missing" carve-out.

`scroll-area` was vendored but had zero importers in Thoth's codebase and was deleted outright rather than migrated.

`EmptyState`, `ErrorState` and `LoadingState` were local copies until 27/08/2026 and now come from the package via `src/lib/components/common/index.ts`. They had already rotted as copies: all three styled themselves with `text-text-primary`, `text-text-secondary`, `text-text-tertiary` and `text-error`, none of which this app defines, so their titles and messages had been rendering uncoloured.

## What holds this in place

Two gates, both on checked-in baselines so they fail only on NEW findings, and both currently baselined EMPTY — there is no grandfathered backlog, so anything either reports is new:

- `pnpm lint:drift` — hand-rolling what `@poodle64/ui` already ships.
- `pnpm lint:design` — craft defects no other gate can see.

They run in pre-commit and in CI's Frontend job, alongside `pnpm check`, `pnpm lint:css` and the production build. Each was proven to fail on a planted violation before being trusted.

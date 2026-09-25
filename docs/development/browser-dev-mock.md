# Browser Dev Mock

How Thoth's Svelte frontend renders in a plain browser (Vite dev server, no Rust backend) for Playwright-driven visual verification.

`src/lib/dev/tauri-mock.ts` installs Tauri's own `mockIPC`/`mockWindows` (`@tauri-apps/api/mocks`) so `invoke()`/`listen()`/`once()`/`emit()` work without `window.__TAURI_INTERNALS__`, which the real Tauri runtime injects before app code runs and which a browser-only Vite server never sets. It is app-agnostic — copy it as-is to another Tauri project.

`src/lib/dev/thoth-mock-data.ts` is Thoth's own command/event map and seed data — the part each app rewrites.

Wired in `src/routes/+layout.svelte`, guarded by `import.meta.env.DEV && !('__TAURI_INTERNALS__' in window)` so it never ships in a production build and never runs against the real Tauri runtime.

## Driving it

`pnpm run dev`, then open `http://localhost:1422/{,history,recording-indicator}` with the Playwright MCP. A command-map entry's key must match the real `invoke()` string the frontend actually calls — casing varies per command (config is snake_case, history rows camelCase) — so if a mocked call goes unanswered, check the calling store, not this doc.

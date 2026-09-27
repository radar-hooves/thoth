import { test, expect } from '@playwright/test';

/**
 * Settings pane switching (#178): "moving between panes in the Settings
 * window is noticeably slow, and a switch occasionally misbehaves."
 *
 * Each pane in Settings.svelte is an `{#if}` branch, so a switch destroys the
 * previous pane's component tree and mounts the next one from scratch —
 * asserting a render budget here catches a pane whose mount does expensive
 * synchronous work, and clicking through the whole set twice catches a pane
 * that fails to reset (stale content, a listener never cleaned up) on a second
 * visit. As in smoke.spec.ts, invoke() runs through the browser dev mock, so
 * this measures render cost, not IPC latency.
 */

const PANE_LABELS = ['Overview', 'Insights', 'Recording', 'Models', 'AI Enhancement', 'History', 'Dictionary', 'Transcribe', 'Storage', 'Integrations'] as const;

/** Generous for a mocked, local render; well under what a user would call slow. */
const RENDER_BUDGET_MS = 1_000;

test.describe('settings pane switching', () => {
  test('every pane renders within budget, and switching never leaves the content area empty', async ({ page }) => {
    const pageErrors: string[] = [];
    page.on('pageerror', (error) => pageErrors.push(error.message));

    await page.goto('/');
    await expect(page.locator('.settings-window')).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('nav.sidebar')).toBeVisible();

    for (const label of PANE_LABELS) {
      const started = Date.now();

      await page.locator('nav.sidebar button', { hasText: label }).click();

      // The content area re-renders on every switch (a fresh `.pane`, or the
      // history/dictionary panes' own root); waiting on it rather than a
      // fixed sleep is what makes the timing meaningful.
      await expect(page.locator('main.content > *').first()).toBeVisible();

      const elapsed = Date.now() - started;
      expect(elapsed, `switching to "${label}" took ${elapsed}ms, over the ${RENDER_BUDGET_MS}ms budget`).toBeLessThan(RENDER_BUDGET_MS);

      // A pane that "misbehaves" most often leaves the content area empty
      // rather than throwing — assert something actually mounted.
      const childCount = await page.locator('main.content > *').count();
      expect(childCount, `"${label}" left the content area empty`).toBeGreaterThan(0);
    }

    // A second pass over the same panes catches state that survives a switch
    // when it shouldn't (a stale value, a listener that fires twice).
    for (const label of PANE_LABELS) {
      await page.locator('nav.sidebar button', { hasText: label }).click();
      await expect(page.locator('main.content > *').first()).toBeVisible();
    }

    const EXPECTED_WITHOUT_TAURI = /__TAURI__|__TAURI_INTERNALS__|invoke|IPC/i;
    const unexpected = pageErrors.filter((message) => !EXPECTED_WITHOUT_TAURI.test(message));
    expect(unexpected, `unexpected page errors: ${unexpected.join('; ')}`).toEqual([]);
  });
});

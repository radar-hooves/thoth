import { test, expect } from '@playwright/test';

/**
 * Permanent delete from Insights' Trash asks first (UX audit 08/10/2026, §4):
 * the reversible "Move to Trash" had a confirmation while the irreversible
 * purge beside it fired on one click. Runs against the browser dev mock, whose
 * cruft candidates move into a mock trash.
 */
test('deleting a trashed recording permanently needs a confirmation', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('.settings-window')).toBeVisible({ timeout: 15_000 });
  await page.locator('nav.sidebar button', { hasText: 'Insights' }).click();

  // Put one recording in the trash through the reversible path.
  await page.getByRole('button', { name: 'Scan for cruft' }).click();
  await page.getByText('Thank you.').click();
  await page.getByRole('button', { name: 'Move 1 to Trash' }).click();
  await page.getByRole('alertdialog').getByRole('button', { name: 'Move to Trash' }).click();
  await page.getByRole('button', { name: 'Trash (reversible)' }).click();

  const purge = page.getByTitle('Delete permanently').first();
  const entries = page.getByTitle('Delete permanently');
  const before = await entries.count();
  expect(before).toBeGreaterThan(0);

  // One click only asks; cancelling keeps the entry.
  await purge.click();
  const dialog = page.getByRole('alertdialog');
  await expect(dialog).toContainText('Delete this recording permanently?');
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(entries).toHaveCount(before);

  // Confirming deletes it.
  await purge.click();
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete permanently' }).click();
  await expect(entries).toHaveCount(before - 1);
});

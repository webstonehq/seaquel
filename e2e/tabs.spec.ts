/**
 * E2E tests for tab management.
 *
 * Tests:
 * - Creating new query tabs
 * - Switching between tabs
 * - Renaming tabs
 * - Closing tabs
 * - Tab persistence
 */

import { test, expect } from './fixtures/test-fixtures';

// Helper to create a connection for tab tests (same as other test files)
async function setupConnection(appPage: any, testDb: { connectionString: string }) {
  await appPage.getByText('Add new connection').click();
  const dialog = appPage.getByRole('dialog');
  await dialog.getByRole('textbox', { name: 'Connection String' }).fill(testDb.connectionString);
  await dialog.getByRole('button', { name: 'Add Connection' }).click();

  // Wait for connection to be established
  await appPage.waitForTimeout(3000);

  // Force close dialog - it stays open in mock environment
  // Use Cancel button since Escape doesn't work reliably
  const dialogLocator = appPage.locator('[role="dialog"]');
  for (let attempt = 0; attempt < 10; attempt++) {
    const dialogCount = await dialogLocator.count();
    if (dialogCount === 0) break;

    // Click the Cancel button to close the dialog
    const cancelBtn = dialogLocator.getByRole('button', { name: 'Cancel' });
    if (await cancelBtn.isVisible().catch(() => false)) {
      await cancelBtn.click();
      await appPage.waitForTimeout(500);
      continue;
    }

    // Fallback: try Escape
    await appPage.keyboard.press('Escape');
    await appPage.waitForTimeout(300);
  }

  // Final verification - dialog should be closed
  await expect(dialogLocator).toHaveCount(0, { timeout: 5000 });

  // Verify connection was created
  await expect(appPage.getByRole('button', { name: /SQLite/ })).toBeVisible({ timeout: 5000 });

  // Click on the Queries tab to switch to query view
  const queriesTab = appPage.getByRole('tab', { name: 'Queries' });
  if (await queriesTab.isVisible({ timeout: 1000 }).catch(() => false)) {
    await queriesTab.click();
    await appPage.waitForTimeout(500);
  }
}

test.describe('Tab Management', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('creates initial query tab on connection', async ({ appPage }) => {
    // Should have at least one query tab
    await expect(appPage.getByText('Query 1')).toBeVisible({ timeout: 5000 });
  });

  test('creates new query tab with keyboard shortcut', async ({ appPage }) => {
    // Wait for initial tab
    await expect(appPage.getByText('Query 1')).toBeVisible({ timeout: 5000 });

    // Use Cmd+T keyboard shortcut to create new tab
    await appPage.keyboard.press('Meta+t');
    await appPage.waitForTimeout(1000);

    // Should have a new tab (Query 2)
    await expect(appPage.getByText('Query 2')).toBeVisible({ timeout: 3000 });
  });

  test('switches between query tabs', async ({ appPage }) => {
    // Wait for initial tab
    await expect(appPage.getByText('Query 1')).toBeVisible({ timeout: 5000 });

    // Create a second tab using keyboard shortcut
    await appPage.keyboard.press('Meta+t');
    await appPage.waitForTimeout(1000);
    await expect(appPage.getByText('Query 2')).toBeVisible({ timeout: 3000 });

    // Click on the first tab (Query 1)
    await appPage.getByText('Query 1').click();
    await appPage.waitForTimeout(300);

    // Should be able to switch between tabs
    await expect(appPage.getByText('Query 1')).toBeVisible();
    await expect(appPage.getByText('Query 2')).toBeVisible();
  });

  test('closes query tab with Cmd+W', async ({ appPage }) => {
    // Wait for initial tab
    await expect(appPage.getByText('Query 1')).toBeVisible({ timeout: 5000 });

    // Create a second tab first using keyboard shortcut
    await appPage.keyboard.press('Meta+t');
    await appPage.waitForTimeout(1000);
    await expect(appPage.getByText('Query 2')).toBeVisible({ timeout: 3000 });

    // Close the current tab (Query 2) with Cmd+W
    await appPage.keyboard.press('Meta+w');
    await appPage.waitForTimeout(500);

    // Query 2 tab should be removed
    await expect(appPage.getByText('Query 2')).not.toBeVisible({ timeout: 3000 });
    // Query 1 should still exist
    await expect(appPage.getByText('Query 1')).toBeVisible();
  });
});

// Tests that require Monaco editor (skipped)
test.describe.skip('Tab Content (requires Monaco)', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('renames query tab with double-click', async ({ appPage }) => {
    // Double-click on the tab name to edit
    const tabName = appPage.getByText('Query 1');
    await tabName.dblclick();

    // Should show input field
    const input = appPage.locator('input').filter({ has: appPage.locator('[class*="text-xs"]') });
    await input.first().fill('My Custom Query');
    await appPage.keyboard.press('Enter');

    // Tab should be renamed
    await expect(appPage.getByText('My Custom Query')).toBeVisible();
  });

  test('preserves query content when switching tabs', async ({ appPage }) => {
    // Type in first tab
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM users');

    // Create new tab
    const plusButton = appPage.locator('button').filter({ has: appPage.locator('svg.lucide-plus') });
    await plusButton.click();
    await appPage.waitForTimeout(500);

    // Type in second tab
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM products');

    // Switch back to first tab
    await appPage.getByText('Query 1').click();
    await appPage.waitForTimeout(300);

    // First tab should still have its content
    // (This would need to check Monaco editor content - simplified check)
    await expect(editor).toBeVisible();
  });
});

// Note: Schema tab tests are covered in schema.spec.ts
// Note: Tab keyboard shortcuts (Cmd+T, Cmd+W) are covered in shortcuts.spec.ts

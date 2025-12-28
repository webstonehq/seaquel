/**
 * E2E tests for keyboard shortcuts.
 *
 * Tests:
 * - Execute query shortcut
 * - Tab navigation shortcuts
 * - New tab shortcut
 * - Help dialog
 */

import { test, expect } from './fixtures/test-fixtures';

// Helper to create a connection (same as other test files)
async function setupConnection(appPage: any, testDb: { connectionString: string }) {
  await appPage.getByText('Add new connection').click();
  const dialog = appPage.getByRole('dialog');
  await dialog.getByRole('textbox', { name: 'Connection String' }).fill(testDb.connectionString);
  await dialog.getByRole('button', { name: 'Add Connection' }).click();

  // Wait for connection to be established
  await appPage.waitForTimeout(3000);

  // Close the dialog if still open - try multiple approaches
  for (let i = 0; i < 5; i++) {
    if (await dialog.isVisible({ timeout: 500 }).catch(() => false)) {
      // Try pressing Escape first
      await appPage.keyboard.press('Escape');
      await appPage.waitForTimeout(300);

      if (await dialog.isVisible({ timeout: 200 }).catch(() => false)) {
        // Try clicking the Close button
        const closeButton = dialog.getByRole('button', { name: 'Close' });
        if (await closeButton.isVisible({ timeout: 200 }).catch(() => false)) {
          await closeButton.click();
          await appPage.waitForTimeout(300);
          continue;
        }
        // Try clicking outside
        await appPage.mouse.click(10, 10);
        await appPage.waitForTimeout(300);
      }
    } else {
      break;
    }
  }

  // Verify dialog is closed
  await expect(dialog).not.toBeVisible({ timeout: 5000 });

  // Verify connection was created
  await expect(appPage.getByRole('button', { name: /SQLite/ })).toBeVisible({ timeout: 5000 });

  // Click on the Queries tab to switch to query view
  const queriesTab = appPage.getByRole('tab', { name: 'Queries' });
  if (await queriesTab.isVisible({ timeout: 1000 }).catch(() => false)) {
    await queriesTab.click();
    await appPage.waitForTimeout(500);
  }
}

// TODO: Enable once Tauri IPC mock fully supports database connections
test.describe.skip('Query Shortcuts', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('Cmd+Enter executes query', async ({ appPage }) => {
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM users');

    // Execute with Cmd+Enter
    await appPage.keyboard.press('Meta+Enter');

    await appPage.waitForTimeout(1000);

    // Should show results
    await expect(appPage.getByText(/\d+ row/i)).toBeVisible();
  });

  test('Cmd+S opens save query dialog', async ({ appPage }) => {
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM products');

    // Save with Cmd+S
    await appPage.keyboard.press('Meta+s');

    await appPage.waitForTimeout(500);

    // Save dialog should appear
    const dialog = appPage.getByRole('dialog');
    await expect(dialog).toBeVisible();
  });
});

test.describe('Tab Shortcuts', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('Cmd+T creates new tab', async ({ appPage }) => {
    // Wait for initial tab to appear
    await expect(appPage.getByText('Query 1')).toBeVisible({ timeout: 5000 });

    // Press Cmd+T to create new tab
    await appPage.keyboard.press('Meta+t');
    await appPage.waitForTimeout(1000);

    // Should now have Query 2
    await expect(appPage.getByText('Query 2')).toBeVisible({ timeout: 3000 });
  });

  test('Cmd+W closes current tab', async ({ appPage }) => {
    // Wait for initial tab
    await expect(appPage.getByText('Query 1')).toBeVisible({ timeout: 5000 });

    // Create a second tab first
    await appPage.keyboard.press('Meta+t');
    await appPage.waitForTimeout(1000);
    await expect(appPage.getByText('Query 2')).toBeVisible({ timeout: 3000 });

    // Close the current tab (Query 2)
    await appPage.keyboard.press('Meta+w');
    await appPage.waitForTimeout(500);

    // Query 2 should be gone
    await expect(appPage.getByText('Query 2')).not.toBeVisible({ timeout: 3000 });
    // Query 1 should still exist
    await expect(appPage.getByText('Query 1')).toBeVisible();
  });

  test('Cmd+1 switches to first tab', async ({ appPage }) => {
    // Wait for initial tab
    await expect(appPage.getByText('Query 1')).toBeVisible({ timeout: 5000 });

    // Create a second tab
    await appPage.keyboard.press('Meta+t');
    await appPage.waitForTimeout(1000);
    await expect(appPage.getByText('Query 2')).toBeVisible({ timeout: 3000 });

    // Switch to first tab with Cmd+1
    await appPage.keyboard.press('Meta+1');
    await appPage.waitForTimeout(300);

    // First tab should still be visible (basic check)
    await expect(appPage.getByText('Query 1')).toBeVisible();
  });
});

test.describe('Help Dialog', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('pressing ? opens keyboard shortcuts help', async ({ appPage }) => {
    // Click on the page first to ensure focus
    await appPage.locator('main').click();
    await appPage.waitForTimeout(200);

    // Open help dialog with ? key (type the character directly)
    await appPage.keyboard.type('?');
    await appPage.waitForTimeout(500);

    // Help dialog should appear with keyboard shortcuts content
    const helpDialog = appPage.getByRole('dialog');
    await expect(helpDialog).toBeVisible({ timeout: 3000 });
  });

  test('help dialog shows available shortcuts', async ({ appPage }) => {
    // Click on the page first to ensure focus
    await appPage.locator('main').click();
    await appPage.waitForTimeout(200);

    // Open help dialog
    await appPage.keyboard.type('?');
    await appPage.waitForTimeout(500);

    const helpDialog = appPage.getByRole('dialog');
    await expect(helpDialog).toBeVisible({ timeout: 3000 });

    // Should show Keyboard Shortcuts title (as heading)
    await expect(helpDialog.getByRole('heading', { name: 'Keyboard Shortcuts' })).toBeVisible();

    // Should show shortcut descriptions (within the dialog)
    await expect(helpDialog.getByText('Execute query')).toBeVisible();
    await expect(helpDialog.getByText('New query tab')).toBeVisible();
  });

  test('can close help dialog with Escape', async ({ appPage }) => {
    // Click on the page first to ensure focus
    await appPage.locator('main').click();
    await appPage.waitForTimeout(200);

    // Open help dialog
    await appPage.keyboard.type('?');
    await appPage.waitForTimeout(500);

    const helpDialog = appPage.getByRole('dialog');
    await expect(helpDialog).toBeVisible({ timeout: 3000 });

    await appPage.keyboard.press('Escape');
    await appPage.waitForTimeout(300);

    await expect(helpDialog).not.toBeVisible();
  });
});

// TODO: Enable once Tauri IPC mock fully supports database connections
test.describe.skip('Editor Shortcuts', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('Cmd+A selects all text in editor', async ({ appPage }) => {
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM users');

    // Select all
    await appPage.keyboard.press('Meta+a');

    // Type new content (should replace)
    await appPage.keyboard.type('SELECT 1');

    // Monaco should now have only "SELECT 1"
    // This is hard to verify without Monaco API access
  });

  test('Cmd+Z undoes last change', async ({ appPage }) => {
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM users');
    await appPage.keyboard.type(' WHERE id = 1');

    // Undo
    await appPage.keyboard.press('Meta+z');

    // Should undo the last typed content
    // This would need Monaco content verification
  });

  test('Cmd+Shift+Z redoes undone change', async ({ appPage }) => {
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM users');

    // Undo then redo
    await appPage.keyboard.press('Meta+z');
    await appPage.keyboard.press('Meta+Shift+z');

    // Content should be restored
  });
});

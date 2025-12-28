/**
 * E2E tests for EXPLAIN/ANALYZE functionality.
 *
 * Tests:
 * - Executing EXPLAIN on queries
 * - Executing ANALYZE
 * - Query plan visualization
 */

import { test, expect } from './fixtures/test-fixtures';

// Helper to create a connection
async function setupConnection(appPage: any, testDb: { connectionString: string }) {
  await appPage.getByText('Add new connection').click();
  const dialog = appPage.getByRole('dialog');
  await dialog.getByLabel('Connection String').fill(testDb.connectionString);
  await dialog.getByRole('button', { name: 'Add Connection' }).click();
  await expect(dialog).not.toBeVisible({ timeout: 5000 });
  await appPage.waitForTimeout(1000);
}

// TODO: Enable once Tauri IPC mock fully supports database connections
test.describe.skip('EXPLAIN Queries', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('can execute EXPLAIN on a query', async ({ appPage }) => {
    // Type a SELECT query
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM users');

    // Look for EXPLAIN button or menu option
    const explainButton = appPage.getByRole('button', { name: /explain/i });
    if (await explainButton.isVisible()) {
      await explainButton.click();
    } else {
      // Try keyboard shortcut or context menu
      await appPage.keyboard.press('Alt+e');
    }

    await appPage.waitForTimeout(1000);

    // Should show explain tab or results
    const explainTab = appPage.locator('div').filter({ hasText: /Explain:|Analyze:/i });
    await expect(explainTab.first().or(appPage.getByText(/SCAN|SEARCH|Query Plan/i))).toBeVisible();
  });

  test('shows query plan nodes for EXPLAIN', async ({ appPage }) => {
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM users WHERE id = 1');

    const explainButton = appPage.getByRole('button', { name: /explain/i });
    if (await explainButton.isVisible()) {
      await explainButton.click();
    }

    await appPage.waitForTimeout(1000);

    // Should show query plan information
    // SQLite typically shows SCAN or SEARCH
    await expect(
      appPage.getByText(/SCAN|SEARCH|Seq Scan|Index Scan|Query Plan/i)
    ).toBeVisible();
  });

  test('creates explain tab for query', async ({ appPage }) => {
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM products');

    const explainButton = appPage.getByRole('button', { name: /explain/i });
    if (await explainButton.isVisible()) {
      await explainButton.click();
    }

    await appPage.waitForTimeout(1000);

    // Should create an explain tab (with Activity icon)
    const explainTabs = appPage.locator('div').filter({ has: appPage.locator('svg.lucide-activity') });
    await expect(explainTabs.first()).toBeVisible();
  });
});

// TODO: Enable once Tauri IPC mock fully supports database connections
test.describe.skip('ANALYZE Queries', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('can execute ANALYZE on a query', async ({ appPage }) => {
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM users');

    // Look for ANALYZE button
    const analyzeButton = appPage.getByRole('button', { name: /analyze/i });
    if (await analyzeButton.isVisible()) {
      await analyzeButton.click();
    }

    await appPage.waitForTimeout(1000);

    // Should show analyze results with timing
    // The tab name includes "Analyze:"
    const analyzeTab = appPage.locator('div').filter({ hasText: /Analyze:/i });
    await expect(analyzeTab.first().or(appPage.getByText(/actual|time|rows/i))).toBeVisible();
  });

  test('ANALYZE shows actual execution metrics', async ({ appPage }) => {
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM orders JOIN users ON orders.user_id = users.id');

    const analyzeButton = appPage.getByRole('button', { name: /analyze/i });
    if (await analyzeButton.isVisible()) {
      await analyzeButton.click();
    }

    await appPage.waitForTimeout(1000);

    // ANALYZE should show actual row counts or timing
    // Look for numeric metrics
    await expect(
      appPage.getByText(/\d+\s*(row|ms|time)/i).or(appPage.getByText(/actual/i))
    ).toBeVisible();
  });
});

// TODO: Enable once Tauri IPC mock fully supports database connections
test.describe.skip('Explain Tab Management', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('can close explain tab', async ({ appPage }) => {
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM users');

    const explainButton = appPage.getByRole('button', { name: /explain/i });
    if (await explainButton.isVisible()) {
      await explainButton.click();
    }

    await appPage.waitForTimeout(1000);

    // Find and close the explain tab
    const explainTab = appPage.locator('div').filter({ has: appPage.locator('svg.lucide-activity') }).first();
    await explainTab.hover();

    const closeButton = explainTab.locator('button').filter({ has: appPage.locator('svg.lucide-x') });
    await closeButton.click();

    await appPage.waitForTimeout(300);

    // Explain tab should be closed, back to query view
    await expect(appPage.locator('.monaco-editor')).toBeVisible();
  });

  test('can switch between query and explain tabs', async ({ appPage }) => {
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM users');

    const explainButton = appPage.getByRole('button', { name: /explain/i });
    if (await explainButton.isVisible()) {
      await explainButton.click();
    }

    await appPage.waitForTimeout(1000);

    // Click on query tab
    await appPage.getByText('Query 1').click();
    await appPage.waitForTimeout(300);

    // Should show query editor
    await expect(appPage.locator('.monaco-editor')).toBeVisible();

    // Click back on explain tab
    const explainTabs = appPage.locator('div').filter({ has: appPage.locator('svg.lucide-activity') });
    await explainTabs.first().click();

    await appPage.waitForTimeout(300);

    // Should show explain view
    await expect(appPage.getByText(/SCAN|SEARCH|Query Plan/i)).toBeVisible();
  });
});

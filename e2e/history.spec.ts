/**
 * E2E tests for query history and saved queries.
 *
 * Tests:
 * - Query history population
 * - Favorite queries
 * - Saved queries CRUD
 * - Loading queries from history
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

// Helper to execute a query
async function executeQuery(appPage: any, query: string) {
  const editor = appPage.locator('.monaco-editor');
  await editor.click();
  await appPage.keyboard.press('Meta+a');
  await appPage.keyboard.type(query);
  await appPage.keyboard.press('Meta+Enter');
  await appPage.waitForTimeout(1000);
}

// TODO: Enable once Tauri IPC mock fully supports database connections
test.describe.skip('Query History', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('shows Queries tab in sidebar', async ({ appPage }) => {
    await expect(appPage.getByRole('tab', { name: 'Queries' })).toBeVisible();
  });

  test('adds executed query to history', async ({ appPage }) => {
    // Execute a query
    await executeQuery(appPage, 'SELECT * FROM users');

    // Switch to Queries tab in sidebar
    await appPage.getByRole('tab', { name: 'Queries' }).click();
    await appPage.waitForTimeout(300);

    // History should show the query
    await expect(appPage.getByText('SELECT * FROM users')).toBeVisible();
  });

  test('shows execution time in history', async ({ appPage }) => {
    await executeQuery(appPage, 'SELECT * FROM users');

    await appPage.getByRole('tab', { name: 'Queries' }).click();
    await appPage.waitForTimeout(300);

    // Should show execution time
    await expect(appPage.getByText(/\d+(\.\d+)?\s*ms/)).toBeVisible();
  });

  test('shows row count in history', async ({ appPage }) => {
    await executeQuery(appPage, 'SELECT * FROM users');

    await appPage.getByRole('tab', { name: 'Queries' }).click();
    await appPage.waitForTimeout(300);

    // Should show row count (3 users)
    await expect(appPage.getByText(/3\s*row/i)).toBeVisible();
  });

  test('can toggle query as favorite', async ({ appPage }) => {
    await executeQuery(appPage, 'SELECT * FROM users');

    await appPage.getByRole('tab', { name: 'Queries' }).click();
    await appPage.waitForTimeout(300);

    // Find and click the star/favorite button
    const starButton = appPage.locator('button').filter({ has: appPage.locator('svg.lucide-star') }).first();
    await starButton.click();

    await appPage.waitForTimeout(300);

    // Star should be filled/active
    await expect(appPage.locator('.lucide-star.text-yellow-500, .lucide-star[fill]')).toBeVisible();
  });

  test('loads query from history to new tab', async ({ appPage }) => {
    await executeQuery(appPage, 'SELECT * FROM products');

    await appPage.getByRole('tab', { name: 'Queries' }).click();
    await appPage.waitForTimeout(300);

    // Click on the history item to load it
    await appPage.getByText('SELECT * FROM products').click();

    await appPage.waitForTimeout(300);

    // Should create a new tab with the query
    // Check that we're on a query tab (not schema/explain)
    await expect(appPage.locator('.monaco-editor')).toBeVisible();
  });

  test('filters history with search', async ({ appPage }) => {
    // Execute multiple queries
    await executeQuery(appPage, 'SELECT * FROM users');
    await executeQuery(appPage, 'SELECT * FROM products');

    await appPage.getByRole('tab', { name: 'Queries' }).click();

    // Search for 'users'
    const searchInput = appPage.getByPlaceholder(/search/i);
    await searchInput.fill('users');

    await appPage.waitForTimeout(300);

    // Should only show users query
    await expect(appPage.getByText('SELECT * FROM users')).toBeVisible();
    await expect(appPage.getByText('SELECT * FROM products')).not.toBeVisible();
  });
});

// TODO: Enable once Tauri IPC mock fully supports database connections
test.describe.skip('Saved Queries', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('shows saved queries section', async ({ appPage }) => {
    await appPage.getByRole('tab', { name: 'Queries' }).click();
    await appPage.waitForTimeout(300);

    // Should show Saved section
    await expect(appPage.getByText(/Saved|Bookmarks/i)).toBeVisible();
  });

  test('can save current query', async ({ appPage }) => {
    // Type a query
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM users WHERE id = 1');

    // Look for save button (Cmd+S or save icon)
    await appPage.keyboard.press('Meta+s');

    await appPage.waitForTimeout(300);

    // Save dialog should appear
    const saveDialog = appPage.getByRole('dialog');
    if (await saveDialog.isVisible()) {
      await saveDialog.getByLabel(/name/i).fill('Get User By ID');
      await saveDialog.getByRole('button', { name: /save/i }).click();
    }

    await appPage.waitForTimeout(300);

    // Query should appear in saved section
    await appPage.getByRole('tab', { name: 'Queries' }).click();
    await expect(appPage.getByText('Get User By ID')).toBeVisible();
  });

  test('loads saved query into new tab', async ({ appPage }) => {
    // First save a query
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT * FROM products');
    await appPage.keyboard.press('Meta+s');

    await appPage.waitForTimeout(300);

    const saveDialog = appPage.getByRole('dialog');
    if (await saveDialog.isVisible()) {
      await saveDialog.getByLabel(/name/i).fill('All Products');
      await saveDialog.getByRole('button', { name: /save/i }).click();
    }

    await appPage.waitForTimeout(300);

    // Create a new tab
    await appPage.keyboard.press('Meta+t');
    await appPage.waitForTimeout(300);

    // Go to Queries tab and click on saved query
    await appPage.getByRole('tab', { name: 'Queries' }).click();
    await appPage.getByText('All Products').click();

    await appPage.waitForTimeout(300);

    // Should load the query
    await expect(appPage.locator('.monaco-editor')).toBeVisible();
  });

  test('can delete saved query', async ({ appPage }) => {
    // First save a query
    const editor = appPage.locator('.monaco-editor');
    await editor.click();
    await appPage.keyboard.type('SELECT 1');
    await appPage.keyboard.press('Meta+s');

    await appPage.waitForTimeout(300);

    const saveDialog = appPage.getByRole('dialog');
    if (await saveDialog.isVisible()) {
      await saveDialog.getByLabel(/name/i).fill('Test Query');
      await saveDialog.getByRole('button', { name: /save/i }).click();
    }

    await appPage.waitForTimeout(300);

    // Go to Queries tab
    await appPage.getByRole('tab', { name: 'Queries' }).click();

    // Find the saved query and delete it
    const savedQuery = appPage.getByText('Test Query');
    await savedQuery.hover();

    const deleteButton = appPage.locator('button').filter({ has: appPage.locator('svg.lucide-trash') }).first();
    await deleteButton.click();

    await appPage.waitForTimeout(300);

    // Query should be removed
    await expect(appPage.getByText('Test Query')).not.toBeVisible();
  });
});

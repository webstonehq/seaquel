/**
 * E2E tests for schema browser functionality.
 *
 * Tests:
 * - Viewing tables list
 * - Table columns display
 * - Index information
 * - Empty schema handling
 */

import { test, expect } from './fixtures/test-fixtures';

// Helper to create a connection (same as query.spec.ts)
async function setupConnection(appPage: any, testDb: { connectionString: string }) {
  await appPage.getByText('Add new connection').click();
  const dialog = appPage.getByRole('dialog');
  await dialog.getByRole('textbox', { name: 'Connection String' }).fill(testDb.connectionString);
  await dialog.getByRole('button', { name: 'Add Connection' }).click();

  // Wait for connection to be established
  await appPage.waitForTimeout(3000);

  // Close the dialog if still open
  for (let i = 0; i < 3; i++) {
    if (await dialog.isVisible({ timeout: 500 }).catch(() => false)) {
      const closeButton = dialog.locator('button:has(svg.lucide-x)').first();
      if (await closeButton.isVisible({ timeout: 200 }).catch(() => false)) {
        await closeButton.click();
        await appPage.waitForTimeout(500);
        continue;
      }
      await appPage.mouse.click(10, 10);
      await appPage.waitForTimeout(500);
    } else {
      break;
    }
  }

  // Verify dialog is closed
  await expect(dialog).not.toBeVisible({ timeout: 5000 });

  // Verify connection was created
  await expect(appPage.getByRole('button', { name: /SQLite/ })).toBeVisible({ timeout: 5000 });
}

// Helper to expand the schema folder
async function expandSchemaFolder(appPage: any) {
  const mainFolder = appPage.getByText('main').first();
  if (await mainFolder.isVisible({ timeout: 2000 }).catch(() => false)) {
    await mainFolder.click();
    await appPage.waitForTimeout(500);
  }
}

test.describe('Schema Browser', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('shows tables in sidebar', async ({ appPage }) => {
    // Sidebar should show Schema tab
    await expect(appPage.getByRole('tab', { name: 'Schema' })).toBeVisible();

    // Expand the schema folder
    await expandSchemaFolder(appPage);

    // Should show table names from test database
    await expect(appPage.getByText('users')).toBeVisible({ timeout: 5000 });
    await expect(appPage.getByText('products')).toBeVisible();
    await expect(appPage.getByText('orders')).toBeVisible();
  });

  test('expands schema folder to show tables', async ({ appPage }) => {
    // Schema folder should be visible with count
    await expect(appPage.getByText('main')).toBeVisible();

    // Click to expand
    await expandSchemaFolder(appPage);

    // Tables should now be visible
    await expect(appPage.getByText('users')).toBeVisible({ timeout: 3000 });
    await expect(appPage.getByText('orders')).toBeVisible();
    await expect(appPage.getByText('products')).toBeVisible();
  });

  test('shows row count for tables', async ({ appPage }) => {
    // Expand schema
    await expandSchemaFolder(appPage);

    // Should show row count badge (3 for the main schema, or individual table counts)
    await expect(appPage.getByText('3').first()).toBeVisible({ timeout: 3000 });
  });

  test('searches tables in sidebar', async ({ appPage }) => {
    // Expand schema folder first
    await expandSchemaFolder(appPage);

    // Find search input
    const searchInput = appPage.getByPlaceholder('Search tables...');
    await searchInput.fill('user');

    await appPage.waitForTimeout(500);

    // Should filter to show only matching tables
    await expect(appPage.getByText('users')).toBeVisible({ timeout: 3000 });
  });

  test('clears search to show all tables', async ({ appPage }) => {
    // Expand schema folder first
    await expandSchemaFolder(appPage);

    const searchInput = appPage.getByPlaceholder('Search tables...');
    await searchInput.fill('user');
    await appPage.waitForTimeout(300);

    // Clear search
    await searchInput.fill('');
    await appPage.waitForTimeout(500);

    // All tables should be visible again
    await expect(appPage.getByText('users')).toBeVisible({ timeout: 3000 });
    await expect(appPage.getByText('products')).toBeVisible();
  });
});

test.describe('Table Details', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('opens table details when clicking table', async ({ appPage }) => {
    // Expand schema folder
    await expandSchemaFolder(appPage);
    await appPage.waitForTimeout(300);

    // Double-click on users table to open table view
    const usersTable = appPage.getByText('users').first();
    await usersTable.dblclick();
    await appPage.waitForTimeout(1000);

    // Should show table viewer with Columns header
    await expect(appPage.getByText('Columns').first()).toBeVisible({ timeout: 5000 });
  });

  test('shows column details for table', async ({ appPage }) => {
    // Expand schema folder
    await expandSchemaFolder(appPage);
    await appPage.waitForTimeout(300);

    // Open users table
    const usersTable = appPage.getByText('users').first();
    await usersTable.dblclick();
    await appPage.waitForTimeout(1000);

    // Should show column information
    await expect(appPage.getByText('id').first()).toBeVisible({ timeout: 3000 });
    await expect(appPage.getByText(/INTEGER/i).first()).toBeVisible();
  });

  test('shows primary key indicator', async ({ appPage }) => {
    // Expand schema folder
    await expandSchemaFolder(appPage);
    await appPage.waitForTimeout(300);

    // Open users table
    const usersTable = appPage.getByText('users').first();
    await usersTable.dblclick();
    await appPage.waitForTimeout(1000);

    // Should show primary key indicator
    await expect(appPage.getByText(/PK/i).first()).toBeVisible({ timeout: 3000 });
  });

  test('shows nullable indicator', async ({ appPage }) => {
    // Expand schema folder
    await expandSchemaFolder(appPage);
    await appPage.waitForTimeout(300);

    // Open users table
    const usersTable = appPage.getByText('users').first();
    await usersTable.dblclick();
    await appPage.waitForTimeout(1000);

    // Look for NOT NULL indicator (name column is NOT NULL)
    await expect(appPage.getByText(/NOT NULL/i).first()).toBeVisible({ timeout: 3000 });
  });
});

test.describe('Table Indexes', () => {
  test.beforeEach(async ({ appPage, testDb }) => {
    await appPage.waitForLoadState('networkidle');
    await setupConnection(appPage, testDb);
  });

  test('shows indexes section for table', async ({ appPage }) => {
    // Expand schema folder
    await expandSchemaFolder(appPage);
    await appPage.waitForTimeout(300);

    // Open users table
    const usersTable = appPage.getByText('users').first();
    await usersTable.dblclick();
    await appPage.waitForTimeout(1000);

    // Should show Indexes section
    await expect(appPage.getByText('Indexes').first()).toBeVisible({ timeout: 3000 });
  });

  test('shows unique constraint on email', async ({ appPage }) => {
    // Expand schema folder
    await expandSchemaFolder(appPage);
    await appPage.waitForTimeout(300);

    // Open users table
    const usersTable = appPage.getByText('users').first();
    await usersTable.dblclick();
    await appPage.waitForTimeout(1000);

    // Email has UNIQUE constraint - look for unique indicator
    await expect(appPage.getByText(/UNIQUE/i).first()).toBeVisible({ timeout: 3000 });
  });
});

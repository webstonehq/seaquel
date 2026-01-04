import { browser } from "@wdio/globals";
import { setupBrowser } from "@testing-library/webdriverio";
import { waitForAppReady } from "../helpers/queries.js";

// Helper to get Testing Library screen
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function getScreen() {
	return setupBrowser(browser as any);
}

// State is set up in wdio.conf.ts beforeSession hook (empty state for this spec)
describe("Welcome Screen (No Connections)", () => {
	it("should display the welcome heading", async () => {
		await waitForAppReady(browser);
		const screen = getScreen();

		const heading = await screen.getByRole("heading", {
			name: /welcome to seaquel/i,
		});
		await expect(heading).toBeDisplayed();
	});

	it('should display "Add Your First Connection" button', async () => {
		await waitForAppReady(browser);
		const screen = getScreen();

		const addButton = await screen.getByRole("button", {
			name: /add your first connection/i,
		});
		await expect(addButton).toBeDisplayed();
	});

	it("should display 4 feature cards", async () => {
		await waitForAppReady(browser);
		const screen = getScreen();

		// Verify all feature cards are visible by their titles
		const queryEditor = await screen.getByText("Query Editor");
		const schemaBrowser = await screen.getByText("Schema Browser");
		const erdViewer = await screen.getByText("ERD Viewer");
		const aiAssistant = await screen.getByText("AI Assistant");

		await expect(queryEditor).toBeDisplayed();
		await expect(schemaBrowser).toBeDisplayed();
		await expect(erdViewer).toBeDisplayed();
		await expect(aiAssistant).toBeDisplayed();
	});

	it("should open connection dialog when clicking add button", async () => {
		await waitForAppReady(browser);
		const screen = getScreen();

		const addButton = await screen.getByRole("button", {
			name: /add your first connection/i,
		});
		await addButton.click();

		// Wait for dialog to appear
		await browser.pause(300);

		// Dialog should show "New Database Connection" title
		const dialogTitle = await screen.getByRole("heading", {
			name: /new database connection/i,
		});
		await expect(dialogTitle).toBeDisplayed();
	});
});

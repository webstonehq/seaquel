import { browser } from "@wdio/globals";
import { setupBrowser } from "@testing-library/webdriverio";
import { waitForAppReady } from "../helpers/queries.js";

// Helper to get Testing Library screen
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function getScreen() {
	return setupBrowser(browser as any);
}

// State is set up in wdio.conf.ts beforeSession hook
// This spec gets a seeded test connection with:
// - id: "test-conn-1", name: "Test PostgreSQL", type: "postgres"
// - host: "localhost", port: 5432, databaseName: "testdb", username: "testuser"
describe("Connections Grid (With Existing Connection)", () => {
	it('should display "Select a connection" heading', async () => {
		await waitForAppReady(browser);
		const screen = getScreen();

		const heading = await screen.getByRole("heading", {
			name: /select a connection/i,
		});
		await expect(heading).toBeDisplayed();
	});

	it("should display add connection card", async () => {
		await waitForAppReady(browser);
		const screen = getScreen();

		const addButton = await screen.getByText(/add connection/i);
		await expect(addButton).toBeDisplayed();
	});

	it("should display the existing connection card", async () => {
		await waitForAppReady(browser);
		const screen = getScreen();

		const connectionName = await screen.getByText("Test PostgreSQL");
		await expect(connectionName).toBeDisplayed();
	});

	it("should display connection type badge", async () => {
		await waitForAppReady(browser);
		const screen = getScreen();

		const typeBadge = await screen.getByText("PostgreSQL");
		await expect(typeBadge).toBeDisplayed();
	});

	it("should open connection dialog when clicking add card", async () => {
		await waitForAppReady(browser);
		const screen = getScreen();

		const addButton = await screen.getByText(/add connection/i);
		await addButton.click();
		await browser.pause(300);

		// Dialog should show "New Database Connection" title
		const dialogTitle = await screen.getByRole("heading", {
			name: /new database connection/i,
		});
		await expect(dialogTitle).toBeDisplayed();
	});

	it("should open reconnect dialog when clicking existing connection", async () => {
		await waitForAppReady(browser);
		const screen = getScreen();

		// Click on the connection card
		const connectionCard = await screen.getByText("Test PostgreSQL");
		await connectionCard.click();
		await browser.pause(300);

		// Dialog should show "Reconnect" title
		const dialogTitle = await screen.getByRole("heading", {
			name: /reconnect to database/i,
		});
		await expect(dialogTitle).toBeDisplayed();
	});
});

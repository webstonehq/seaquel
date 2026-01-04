import { browser } from "@wdio/globals";
import { setupBrowser } from "@testing-library/webdriverio";
import { waitForAppReady } from "../helpers/queries.js";

// Helper to get Testing Library screen
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function getScreen() {
	return setupBrowser(browser as any);
}

// State is set up in wdio.conf.ts beforeSession hook (empty state for this spec)
describe("Connection Dialog", () => {
	// Open the connection dialog before running tests
	before(async () => {
		await waitForAppReady(browser);

		// Open the connection dialog
		const screen = getScreen();
		const addButton = await screen.getByRole("button", {
			name: /add your first connection/i,
		});
		await addButton.click();
		await browser.pause(300);
	});

	describe("Tab Navigation", () => {
		it("should have Connection String and Manual tabs", async () => {
			const screen = getScreen();

			const connectionStringTab = await screen.getByRole("tab", {
				name: /connection string/i,
			});
			const manualTab = await screen.getByRole("tab", { name: /manual/i });

			await expect(connectionStringTab).toBeDisplayed();
			await expect(manualTab).toBeDisplayed();
		});

		it("should show Connection String tab content by default", async () => {
			const screen = getScreen();

			// Connection String tab should show textarea
			const textarea = await screen.getByRole("textbox");
			await expect(textarea).toBeDisplayed();

			// Parse button should be visible
			const parseButton = await screen.getByRole("button", {
				name: /parse connection string/i,
			});
			await expect(parseButton).toBeDisplayed();
		});

		it("should switch to Manual tab when clicked", async () => {
			const screen = getScreen();

			const manualTab = await screen.getByRole("tab", { name: /manual/i });
			await manualTab.click();
			await browser.pause(200);

			// Connection Name field should be visible
			const nameInput = await screen.getByLabelText(/connection name/i);
			await expect(nameInput).toBeDisplayed();
		});
	});

	describe("Connection String Tab", () => {
		it("should display Parse button", async () => {
			const screen = getScreen();

			const parseButton = await screen.getByRole("button", {
				name: /parse connection string/i,
			});
			await expect(parseButton).toBeDisplayed();
		});
	});

	describe("Manual Tab Fields", () => {
		beforeEach(async () => {
			const screen = getScreen();
			const manualTab = await screen.getByRole("tab", { name: /manual/i });
			await manualTab.click();
			await browser.pause(200);
		});

		it("should display Connection Name field", async () => {
			const screen = getScreen();
			const nameInput = await screen.getByLabelText(/connection name/i);
			await expect(nameInput).toBeDisplayed();
		});

		it("should display Database Type field", async () => {
			const screen = getScreen();
			const typeLabel = await screen.getByText(/database type/i);
			await expect(typeLabel).toBeDisplayed();
		});

		it("should display Host field with default value", async () => {
			const screen = getScreen();
			const hostInput = await screen.getByLabelText(/^host$/i);
			await expect(hostInput).toBeDisplayed();
			await expect(hostInput).toHaveValue("localhost");
		});

		it("should display Port field with default value for PostgreSQL", async () => {
			const screen = getScreen();
			const portInput = await screen.getByLabelText(/^port$/i);
			await expect(portInput).toBeDisplayed();
			await expect(portInput).toHaveValue("5432");
		});

		it("should display Database field", async () => {
			const screen = getScreen();
			const databaseInput = await screen.getByLabelText(/^database$/i);
			await expect(databaseInput).toBeDisplayed();
		});

		it("should display Username field", async () => {
			const screen = getScreen();
			const usernameInput = await screen.getByLabelText(/username/i);
			await expect(usernameInput).toBeDisplayed();
		});

		it("should display Password field", async () => {
			const screen = getScreen();
			const passwordInput = await screen.getByLabelText(/^password$/i);
			await expect(passwordInput).toBeDisplayed();
		});

		it("should display SSL Mode field", async () => {
			const screen = getScreen();
			const sslLabel = await screen.getByText(/ssl mode/i);
			await expect(sslLabel).toBeDisplayed();
		});
	});

	describe("SSH Tunnel Section", () => {
		beforeEach(async () => {
			const screen = getScreen();
			const manualTab = await screen.getByRole("tab", { name: /manual/i });
			await manualTab.click();
			await browser.pause(200);
		});

		it("should have SSH tunnel checkbox", async () => {
			const screen = getScreen();
			const sshCheckbox = await screen.getByLabelText(
				/connect via ssh tunnel/i
			);
			await expect(sshCheckbox).toBeDisplayed();
		});

		it("should show SSH fields when enabled", async () => {
			const screen = getScreen();

			// Enable SSH tunnel
			const sshCheckbox = await screen.getByLabelText(
				/connect via ssh tunnel/i
			);
			await sshCheckbox.click();
			await browser.pause(200);

			// SSH fields should now be visible
			const sshHost = await screen.getByLabelText(/ssh host/i);
			const sshPort = await screen.getByLabelText(/ssh port/i);
			const sshUsername = await screen.getByLabelText(/ssh username/i);

			await expect(sshHost).toBeDisplayed();
			await expect(sshPort).toBeDisplayed();
			await expect(sshUsername).toBeDisplayed();
		});
	});

	describe("Validation", () => {
		beforeEach(async () => {
			const screen = getScreen();
			const manualTab = await screen.getByRole("tab", { name: /manual/i });
			await manualTab.click();
			await browser.pause(200);
		});

		it("should show error when name is empty", async () => {
			const screen = getScreen();

			// Click submit without filling anything
			const submitButton = await screen.getByRole("button", {
				name: /add connection/i,
			});
			await submitButton.click();
			await browser.pause(200);

			// Error should be shown
			const error = await screen.getByText(/please enter a connection name/i);
			await expect(error).toBeDisplayed();
		});

		it("should show error when database is empty", async () => {
			const screen = getScreen();

			// Fill name only
			const nameInput = await screen.getByLabelText(/connection name/i);
			await nameInput.setValue("Test Connection");

			// Click submit
			const submitButton = await screen.getByRole("button", {
				name: /add connection/i,
			});
			await submitButton.click();
			await browser.pause(200);

			// Error should be shown for database
			const error = await screen.getByText(/please enter a database name/i);
			await expect(error).toBeDisplayed();
		});
	});

	describe("Dialog Actions", () => {
		it("should close dialog when Cancel is clicked", async () => {
			const screen = getScreen();

			const cancelButton = await screen.getByRole("button", { name: /cancel/i });
			await cancelButton.click();
			await browser.pause(300);

			// Dialog should be closed - welcome screen should be visible again
			const welcomeHeading = await screen.getByRole("heading", {
				name: /welcome to seaquel/i,
			});
			await expect(welcomeHeading).toBeDisplayed();
		});

		it("should have Test Connection button", async () => {
			const screen = getScreen();

			const testButton = await screen.getByRole("button", {
				name: /test connection/i,
			});
			await expect(testButton).toBeDisplayed();
		});

		it("should have Add Connection button", async () => {
			const screen = getScreen();

			const addButton = await screen.getByRole("button", {
				name: /add connection/i,
			});
			await expect(addButton).toBeDisplayed();
		});
	});
});

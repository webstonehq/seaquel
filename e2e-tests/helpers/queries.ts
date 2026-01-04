import { setupBrowser } from "@testing-library/webdriverio";

/**
 * Sets up Testing Library queries bound to the browser.
 * Call this at the beginning of each test to get semantic queries.
 *
 * @example
 * const screen = setupQueries(browser);
 * const button = await screen.getByRole('button', { name: /submit/i });
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function setupQueries(browserInstance: any) {
	// Cast to any to work around type incompatibility between WDIO v9 and testing-library
	return setupBrowser(browserInstance);
}

/**
 * Wait for the app to be ready (main content visible).
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export async function waitForAppReady(browserInstance: any) {
	await browserInstance.waitUntil(
		async () => {
			const body = await browserInstance.$("body");
			return body.isDisplayed();
		},
		{
			timeout: 10000,
			timeoutMsg: "App did not become ready within 10 seconds",
		}
	);
	// Give the app a moment to fully render
	await browserInstance.pause(500);
}

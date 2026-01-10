import * as monaco from "monaco-editor";
import { setupLanguageFeatures, LanguageIdEnum } from "monaco-sql-languages";

let isInitialized = false;

export async function initMonaco(): Promise<void> {
	if (isInitialized) return;

	// Configure Monaco workers for Vite ESM
	self.MonacoEnvironment = {
		getWorker(_workerId: string, _label: string) {
			const workerUrl = new URL(
				"monaco-editor/esm/vs/editor/editor.worker.js",
				import.meta.url
			);
			const worker = new Worker(workerUrl, { type: "module" });
			// Suppress worker errors (Monaco still works, just without some features)
			worker.onerror = (e) => {
				e.preventDefault();
				console.debug("[Monaco] Worker failed to load, some features may be limited");
			};
			return worker;
		}
	};

	// Setup PostgreSQL language features from monaco-sql-languages
	// Disable built-in completions since we provide our own schema-aware completions
	setupLanguageFeatures(LanguageIdEnum.PG, {
		completionItems: {
			enable: false
		}
	});

	isInitialized = true;
}

export { monaco };

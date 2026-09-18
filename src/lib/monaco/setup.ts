// Monaco loads lazily because it's strictly browser-only — `window`/`document`
// are referenced at module top level. Importing it statically would pull the
// CSS-laden source tree into the adapter-node SSR bundle, where Rollup has no
// CSS plugin and dies on `import './codicon-modifiers.css'`. Keeping it behind
// a dynamic `import()` means the import only resolves in the client chunk, so
// `monaco-editor` (and `monaco-sql-languages`) can stay in devDependencies.
//
// `import type * as MonacoNS` is type-only and erased by the TS preprocessor —
// no runtime reference, nothing for Rollup to follow.
import type * as MonacoNS from "monaco-editor";

export type Monaco = typeof MonacoNS;

let initPromise: Promise<Monaco> | null = null;

/**
 * Lazy-load Monaco and configure workers, language features, and custom themes.
 * Idempotent: concurrent and repeat calls share the same in-flight promise.
 */
export function initMonaco(): Promise<Monaco> {
  if (initPromise) return initPromise;

  initPromise = (async () => {
    const monaco = await import("monaco-editor");
    const { setupLanguageFeatures, LanguageIdEnum } = await import("monaco-sql-languages");
    const { default: EditorWorker } =
      await import("monaco-editor/esm/vs/editor/editor.worker?worker");

    // Configure Monaco workers for Vite ESM. Using Vite's ?worker import
    // ensures the worker is properly bundled in production (plain `new URL()`
    // creates a data: URL with unresolved imports).
    self.MonacoEnvironment = {
      getWorker() {
        return new EditorWorker();
      },
    };

    // Setup PostgreSQL language features from monaco-sql-languages.
    // Disable built-in completions since we provide our own schema-aware ones.
    setupLanguageFeatures(LanguageIdEnum.PG, {
      completionItems: {
        enable: false,
      },
    });

    defineCustomThemes(monaco);

    return monaco;
  })();

  return initPromise;
}

/**
 * Define custom Monaco themes that extend the default themes with styling for
 * template variables ({{var}}).
 */
function defineCustomThemes(monaco: Monaco): void {
  // Light theme extending vs
  monaco.editor.defineTheme("seaquel-light", {
    base: "vs",
    inherit: true,
    rules: [{ token: "template-variable", foreground: "9333ea", fontStyle: "bold" }],
    colors: {},
  });

  // Dark theme extending vs-dark
  monaco.editor.defineTheme("seaquel-dark", {
    base: "vs-dark",
    inherit: true,
    rules: [{ token: "template-variable", foreground: "c084fc", fontStyle: "bold" }],
    colors: {},
  });
}

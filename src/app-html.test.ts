/**
 * `src/app.html`'s inline script runs before anything else, in every build.
 * With site data blocked (Firefox with storage off, some private modes),
 * reading `localStorage` throws `SecurityError`; the script must carry on, or
 * the demo shows a bare error instead of reaching its "nothing is kept"
 * notice (phase 8 Task 7 probe, item 6).
 */
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";

const file = process.env.SEAQUEL_APP_HTML ?? new URL("./app.html", import.meta.url).pathname;

function inlineScript(): string {
  const html = readFileSync(file, "utf8");
  const script = /<script>([\s\S]*?)<\/script>/.exec(html)?.[1];
  if (!script) throw new Error("no inline script in app.html");
  return script;
}

function page(storage: () => Storage) {
  const root = {
    lang: "",
    dir: "",
    classList: { add: () => {} },
    style: { colorScheme: "", setProperty: () => {} },
  };
  const window = {
    get localStorage() {
      return storage();
    },
    matchMedia: () => ({ matches: false }),
    get sessionStorage(): Storage {
      return storage();
    },
  };
  return {
    root,
    context: {
      window,
      document: { documentElement: root },
      get localStorage() {
        return window.localStorage;
      },
      JSON,
    },
  };
}

describe("app.html's inline script", () => {
  it("runs when localStorage throws on access", () => {
    const { context } = page(() => {
      throw new DOMException("The operation is insecure.", "SecurityError");
    });
    expect(() => runInNewContext(inlineScript(), context)).not.toThrow();
  });

  it("gives the page a working in-memory localStorage when the real one throws", () => {
    const { context } = page(() => {
      throw new DOMException("The operation is insecure.", "SecurityError");
    });
    runInNewContext(inlineScript(), context);
    // What paraglide, mode-watcher and runed do at startup, unguarded.
    const storage = context.window.localStorage;
    expect(storage.getItem("PARAGLIDE_LOCALE")).toBeNull();
    storage.setItem("PARAGLIDE_LOCALE", "de");
    expect(storage.getItem("PARAGLIDE_LOCALE")).toBe("de");
    expect(storage.length).toBe(1);
    expect(storage.key(0)).toBe("PARAGLIDE_LOCALE");
    storage.removeItem("PARAGLIDE_LOCALE");
    expect(storage.getItem("PARAGLIDE_LOCALE")).toBeNull();
    expect(context.window.sessionStorage.getItem("x")).toBeNull();
  });

  it("still reads the locale when storage works", () => {
    const store = new Map([["PARAGLIDE_LOCALE", "ar"]]);
    const { root, context } = page(
      () => ({ getItem: (k: string) => store.get(k) ?? null }) as Storage,
    );
    runInNewContext(inlineScript(), context);
    expect([root.lang, root.dir]).toEqual(["ar", "rtl"]);
  });
});

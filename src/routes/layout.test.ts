/**
 * The root layout in a build that isn't the desktop app, the web build or
 * the demo (Task 6 review, I2), e.g. `npm run dev` opened in Chrome: an
 * error page saying how to run the demo, and nothing of the app. No module
 * loads, no Core opens and nothing calls a transport, so no socket starts
 * its retry loop.
 */
import { describe, expect, it, vi } from "vitest";
import { createRawSnippet } from "svelte";
import { render } from "svelte/server";

const env = vi.hoisted(() => ({ supported: false }));
const initSeaquelWasm = vi.hoisted(() => vi.fn(async () => {}));
const getCoreClient = vi.hoisted(() => vi.fn());

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => false,
  isWeb: () => false,
  isDemo: () => false,
  isSupportedBuild: () => env.supported,
}));
vi.mock("$lib/wasm", () => ({ initSeaquelWasm }));
vi.mock("$lib/core", () => ({ getCoreClient }));

const { load } = await import("./+layout");
const { default: Layout } = await import("./+layout.svelte");
const { m } = await import("$lib/paraglide/messages.js");

const app = createRawSnippet(() => ({ render: () => "<p>THE APP</p>" }));

describe("a build that only runs inside the Seaquel app", () => {
  it("loads nothing and says how to run the demo", async () => {
    const data = await load({ fetch } as never);
    expect(data).toMatchObject({ unsupportedBuild: true });
    expect(initSeaquelWasm).not.toHaveBeenCalled();
    expect(getCoreClient).not.toHaveBeenCalled();

    const { body } = render(Layout, { props: { data, children: app } as never });
    expect(body).toContain(m.unsupported_build_message());
    expect(body).not.toContain("THE APP");
  });

  it("a supported build loads as before and renders the app", async () => {
    env.supported = true;
    const data = await load({ fetch } as never);
    expect(data).toMatchObject({ unsupportedBuild: false, wasmError: null });
    expect(initSeaquelWasm).toHaveBeenCalledOnce();
    const { body } = render(Layout, { props: { data, children: app } as never });
    expect(body).toContain("THE APP");
  });
});

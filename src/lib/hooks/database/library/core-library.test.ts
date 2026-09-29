import { describe, expect, it } from "vitest";
import { CoreCallError, RustStorageClient, type CoreTransport } from "$lib/storage/rust-client";
import { CoreLibrary } from "./core-library";
import { takenByOf } from "./types";

const seq = { epoch: "e1", n: 3 };

/** A transport that records each request's text and answers from `reply`. */
function transport(reply: (request: { method: string; params: { method: string } }) => unknown) {
  const sent: string[] = [];
  const release: Array<() => void> = [];
  let hold = false;
  const t: CoreTransport = async (body) => {
    const text = new TextDecoder().decode(body);
    sent.push(text);
    if (hold) await new Promise<void>((r) => release.push(r));
    const answer = reply(JSON.parse(text));
    if (answer instanceof Error) throw answer;
    return answer;
  };
  return {
    t,
    sent,
    hold: (on: boolean) => (hold = on),
    releaseAll: () => release.splice(0).forEach((r) => r()),
  };
}

const ok = (request: { method: string; params: { method: string } }) => ({
  method: request.method,
  result: {
    method: request.params.method,
    result: request.method === "library" ? { value: null, seq } : null,
  },
});

describe("CoreLibrary", () => {
  it("sends each call as a library request, method before params, without empty secrets", async () => {
    const wire = transport(ok);
    const library = new CoreLibrary(() => new RustStorageClient(wire.t));
    await library.listConnections();
    await library.updateConnection("c1", { name: "N" }, {});
    await library.updateConnection("c1", { port: 1 }, { db: "pw" });
    await library.ensureDefaultProject();
    expect(wire.sent).toEqual([
      '{"method":"library","params":{"method":"connectionsList"}}',
      '{"method":"library","params":{"method":"connectionUpdate","params":{"id":"c1","patch":{"name":"N"}}}}',
      '{"method":"library","params":{"method":"connectionUpdate","params":{"id":"c1","patch":{"port":1},"secrets":{"db":"pw"}}}}',
      '{"method":"library","params":{"method":"projectEnsureDefault"}}',
    ]);
  });

  it("returns the value and its seq", async () => {
    const wire = transport(ok);
    const library = new CoreLibrary(() => new RustStorageClient(wire.t));
    await expect(library.removeSavedQuery("q1")).resolves.toEqual({ value: null, seq });
  });

  it("library writes share the storage writes' queue; lists don't wait", async () => {
    const wire = transport(ok);
    const client = new RustStorageClient(wire.t);
    const library = new CoreLibrary(() => client);
    wire.hold(true);
    const storageWrite = client.appState.set("k", "v");
    const libraryWrite = library.createProject({ name: "P" });
    const list = library.listProjects();
    await Promise.resolve();
    await Promise.resolve();
    // The list went out at once; the library write waits for the storage write.
    expect(new Set(wire.sent.map((s) => JSON.parse(s).params.method))).toEqual(
      new Set(["appStateSet", "projectsList"]),
    );
    wire.hold(false);
    wire.releaseAll();
    await Promise.all([storageWrite, list, libraryWrite]);
    expect(wire.sent.map((s) => JSON.parse(s).params.method).at(-1)).toBe("projectCreate");
  });

  it("a refusal keeps Core's code and the row NAME_TAKEN names", async () => {
    const wire = transport(() => ({
      code: "NAME_TAKEN",
      message: "Another project here already has this name.",
      takenBy: "project-1",
    }));
    const library = new CoreLibrary(
      () =>
        new RustStorageClient(async (body) => {
          const answer = await wire.t(body);
          throw new CoreCallError(answer as never);
        }),
    );
    const error = await library.createProject({ name: "P" }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreCallError);
    expect((error as CoreCallError).code).toBe("NAME_TAKEN");
    expect(takenByOf(error)).toBe("project-1");
  });
});

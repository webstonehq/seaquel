/**
 * `withHostKeyPrompt`: an `UNKNOWN_HOST_KEY` prompts with the fingerprint
 * from the message and, if trusted, retries once with `trustHostKey`.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CoreCallError } from "$lib/storage/rust-client";

const prompt = vi.fn<(host: string, port: number, fingerprint: string) => Promise<boolean>>();
vi.mock("$lib/stores/ssh-host-key-prompt.svelte", () => ({
  sshHostKeyPromptStore: {
    prompt: (host: string, port: number, fp: string) => prompt(host, port, fp),
  },
}));

const { withHostKeyPrompt } = await import("./host-key");

const FP = "SHA256:abcDEF123+/=";
const unknownKey = () =>
  new CoreCallError({
    code: "UNKNOWN_HOST_KEY",
    message: `bastion:2222 isn't in known_hosts; its key is ${FP}`,
  });
const server = { host: "bastion", port: 2222 };

beforeEach(() => {
  prompt.mockReset();
});

describe("withHostKeyPrompt", () => {
  it("retries with the fingerprint the user trusted", async () => {
    prompt.mockResolvedValue(true);
    const attempt = vi
      .fn<(trust?: string) => Promise<string>>()
      .mockRejectedValueOnce(unknownKey())
      .mockResolvedValueOnce("c-1");
    await expect(withHostKeyPrompt(attempt, server)).resolves.toBe("c-1");
    expect(prompt).toHaveBeenCalledWith("bastion", 2222, FP);
    expect(attempt.mock.calls).toEqual([[], [FP]]);
  });

  it("rethrows when the user says no", async () => {
    prompt.mockResolvedValue(false);
    const attempt = vi.fn().mockRejectedValue(unknownKey());
    await expect(withHostKeyPrompt(attempt, server)).rejects.toThrow("UNKNOWN_HOST_KEY");
    expect(attempt).toHaveBeenCalledOnce();
  });

  it("never prompts for another error, a changed key included", async () => {
    const attempt = vi
      .fn()
      .mockRejectedValue(new CoreCallError({ code: "HOST_KEY_MISMATCH", message: FP }));
    await expect(withHostKeyPrompt(attempt, server)).rejects.toThrow("HOST_KEY_MISMATCH");
    expect(prompt).not.toHaveBeenCalled();
  });

  it("doesn't prompt without a fingerprint", async () => {
    const attempt = vi
      .fn()
      .mockRejectedValue(new CoreCallError({ code: "UNKNOWN_HOST_KEY", message: "no key" }));
    await expect(withHostKeyPrompt(attempt, server)).rejects.toThrow("UNKNOWN_HOST_KEY");
    expect(prompt).not.toHaveBeenCalled();
  });

  it("a second unknown key after trusting fails instead of prompting again", async () => {
    prompt.mockResolvedValue(true);
    const attempt = vi.fn().mockRejectedValue(unknownKey());
    await expect(withHostKeyPrompt(attempt, server)).rejects.toThrow("UNKNOWN_HOST_KEY");
    expect(prompt).toHaveBeenCalledOnce();
    expect(attempt).toHaveBeenCalledTimes(2);
  });
});

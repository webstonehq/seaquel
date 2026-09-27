import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  claudeCodeCommand,
  claudeDesktopConfig,
  commandLine,
  mcpArgs,
  posixQuote,
  powershellQuote,
} from "./mcp-snippets";

const AWKWARD = [
  "/Applications/My Apps/Seaquel.app/Contents/MacOS/seaquel-cli",
  'Seaquel\'s "beta" $HOME `id` \\ app',
  "it's",
  "'",
  "$(rm -rf /)",
  "a b\tc",
  "=cmd",
  "~/bin",
  "*",
  "!!",
  "",
  "ünïcødé ‘curly’",
];

const selection = { projectIds: ["p-1"], connectionIds: ["c-1", "c 2"] };

describe("mcpArgs", () => {
  it("lists projects, then connections, by id", () => {
    expect(mcpArgs(selection)).toEqual([
      "mcp",
      "--project",
      "p-1",
      "--connection",
      "c-1",
      "--connection",
      "c 2",
    ]);
  });

  it("is just `mcp` with nothing chosen", () => {
    expect(mcpArgs({ projectIds: [], connectionIds: [] })).toEqual(["mcp"]);
  });
});

describe("posixQuote", () => {
  it("leaves plain words bare", () => {
    expect(posixQuote("/usr/local/bin/seaquel-cli")).toBe("/usr/local/bin/seaquel-cli");
    expect(posixQuote("--connection")).toBe("--connection");
    expect(posixQuote("--")).toBe("--");
    expect(posixQuote("0f1e2d3c-aaaa-bbbb-cccc-1234567890ab")).toBe(
      "0f1e2d3c-aaaa-bbbb-cccc-1234567890ab",
    );
  });

  it("single-quotes everything else", () => {
    expect(posixQuote("a b")).toBe("'a b'");
    expect(posixQuote("it's")).toBe("'it'\\''s'");
    expect(posixQuote("$HOME")).toBe("'$HOME'");
    expect(posixQuote("=cmd")).toBe("'=cmd'");
    expect(posixQuote("")).toBe("''");
  });

  it.runIf(existsSync("/bin/sh"))("round-trips through /bin/sh", () => {
    const script = `printf '%s\\0' ${AWKWARD.map(posixQuote).join(" ")}`;
    const out = execFileSync("/bin/sh", ["-c", script], { encoding: "utf8" });
    expect(out.split("\0").slice(0, -1)).toEqual(AWKWARD);
  });

  it.runIf(existsSync("/bin/zsh"))("round-trips through zsh", () => {
    const script = `printf '%s\\0' ${AWKWARD.map(posixQuote).join(" ")}`;
    const out = execFileSync("/bin/zsh", ["-f", "-c", script], { encoding: "utf8" });
    expect(out.split("\0").slice(0, -1)).toEqual(AWKWARD);
  });
});

describe("powershellQuote", () => {
  it("leaves plain words and flags bare", () => {
    expect(powershellQuote("C:\\Program\\seaquel-cli.exe")).toBe("C:\\Program\\seaquel-cli.exe");
    expect(powershellQuote("--connection")).toBe("--connection");
    expect(powershellQuote("mcp")).toBe("mcp");
  });

  it("quotes `--`, a leading dash and anything PowerShell would read", () => {
    expect(powershellQuote("--")).toBe("'--'");
    expect(powershellQuote("-x")).toBe("'-x'");
    expect(powershellQuote("a,b")).toBe("'a,b'");
    expect(powershellQuote("@id")).toBe("'@id'");
    expect(powershellQuote("$env:HOME")).toBe("'$env:HOME'");
    expect(powershellQuote("C:\\Program Files\\Seaquel\\seaquel-cli.exe")).toBe(
      "'C:\\Program Files\\Seaquel\\seaquel-cli.exe'",
    );
    expect(powershellQuote('say "hi" `n')).toBe("'say \"hi\" `n'");
  });

  it("doubles straight and typographic single quotes", () => {
    expect(powershellQuote("it's")).toBe("'it''s'");
    expect(powershellQuote("it\u2019s \u2018x\u2019")).toBe(
      "'it\u2019\u2019s \u2018\u2018x\u2019\u2019'",
    );
  });
});

describe("commandLine", () => {
  it("quotes the path and ids for a POSIX shell", () => {
    expect(commandLine("/Apps/Sea quel.app/seaquel-cli", selection, "posix")).toBe(
      "'/Apps/Sea quel.app/seaquel-cli' mcp --project p-1 --connection c-1 --connection 'c 2'",
    );
  });

  it("calls a quoted path with & in PowerShell", () => {
    expect(
      commandLine("C:\\Program Files\\Seaquel\\seaquel-cli.exe", selection, "powershell"),
    ).toBe(
      "& 'C:\\Program Files\\Seaquel\\seaquel-cli.exe' mcp --project p-1 --connection c-1 --connection 'c 2'",
    );
    expect(
      commandLine(
        "C:\\Seaquel\\seaquel-cli.exe",
        { projectIds: [], connectionIds: [] },
        "powershell",
      ),
    ).toBe("C:\\Seaquel\\seaquel-cli.exe mcp");
  });
});

describe("claudeCodeCommand", () => {
  it("builds `claude mcp add seaquel -- <path> mcp …`", () => {
    expect(claudeCodeCommand("/Apps/It's $x/seaquel-cli", selection, "posix")).toBe(
      "claude mcp add seaquel -- '/Apps/It'\\''s $x/seaquel-cli' mcp --project p-1 --connection c-1 --connection 'c 2'",
    );
  });

  it("quotes `--` for PowerShell", () => {
    expect(
      claudeCodeCommand("C:\\Program Files\\Seaquel\\seaquel-cli.exe", selection, "powershell"),
    ).toBe(
      "claude mcp add seaquel '--' 'C:\\Program Files\\Seaquel\\seaquel-cli.exe' mcp --project p-1 --connection c-1 --connection 'c 2'",
    );
  });

  it.runIf(existsSync("/bin/sh"))("hands every word back unchanged through /bin/sh", () => {
    const binary = AWKWARD[1];
    const sel = { projectIds: [AWKWARD[3]], connectionIds: [AWKWARD[4], AWKWARD[11]] };
    const line = claudeCodeCommand(binary, sel, "posix");
    const out = execFileSync("/bin/sh", ["-c", `printf '%s\\0' ${line.slice("claude ".length)}`], {
      encoding: "utf8",
    });
    expect(out.split("\0").slice(0, -1)).toEqual([
      "mcp",
      "add",
      "seaquel",
      "--",
      binary,
      ...mcpArgs(sel),
    ]);
  });
});

describe("claudeDesktopConfig", () => {
  it("is valid JSON with the absolute path as command and the args", () => {
    const binary = AWKWARD[1];
    const sel = { projectIds: [], connectionIds: ['id "with" \\ quotes\n'] };
    const parsed = JSON.parse(claudeDesktopConfig(binary, sel));
    expect(parsed).toEqual({
      mcpServers: {
        seaquel: { command: binary, args: ["mcp", "--connection", 'id "with" \\ quotes\n'] },
      },
    });
  });
});

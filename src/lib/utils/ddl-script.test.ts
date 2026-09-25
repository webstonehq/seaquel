import { describe, expect, it } from "vitest";
import { splitDdlScript } from "./ddl-script";

describe("splitDdlScript", () => {
  it("splits statements on ;\\n and adds the final semicolon", () => {
    expect(
      splitDdlScript('CREATE TABLE "t" (\n  "a" TEXT\n);\n\nCREATE INDEX "i" ON "t" ("a");'),
    ).toEqual({
      statements: ['CREATE TABLE "t" (\n  "a" TEXT\n);', 'CREATE INDEX "i" ON "t" ("a");'],
      notes: [],
    });
  });

  it("returns the note lines after the statements (SQLite)", () => {
    const sql =
      'ALTER TABLE "main"."t" RENAME COLUMN "a" TO "b";\n' +
      '-- SQLite can\'t alter column "b" (type); recreate the table to change it\n' +
      '-- SQLite can\'t add a foreign key to an existing table: ("c") REFERENCES "u" ("id"); recreate the table to add it';
    expect(splitDdlScript(sql)).toEqual({
      statements: ['ALTER TABLE "main"."t" RENAME COLUMN "a" TO "b";'],
      notes: [
        'SQLite can\'t alter column "b" (type); recreate the table to change it',
        'SQLite can\'t add a foreign key to an existing table: ("c") REFERENCES "u" ("id"); recreate the table to add it',
      ],
    });
  });

  it("has no statements when every edit is a note", () => {
    expect(
      splitDdlScript(
        '-- SQLite can\'t alter column "status" (default); recreate the table to change it',
      ),
    ).toEqual({
      statements: [],
      notes: ['SQLite can\'t alter column "status" (default); recreate the table to change it'],
    });
  });

  it("keeps a statement whose quoted name holds a line starting with --", () => {
    expect(splitDdlScript('ALTER TABLE "t" RENAME COLUMN "a" TO "x\n-- y";')).toEqual({
      statements: ['ALTER TABLE "t" RENAME COLUMN "a" TO "x\n-- y";'],
      notes: [],
    });
  });
});

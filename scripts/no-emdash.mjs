#!/usr/bin/env node
// Em dash guard (CD-44 Stage B, D-0064). Binding repo rule: the em dash
// (U+2014) and the en dash (U+2013), plus their HTML entities, are forbidden
// in user-facing copy and in every repo document, because copy that instantly
// reads as machine-written undermines a product built on trust.
//
// Standing: run before every push, alongside the secret/IP grep (CLAUDE.md).
// `cargo test` carries a second tripwire over the UI and doc files
// (src/main.rs, `no_em_dashes_in_ui_and_docs`) so a plain test run catches a
// reintroduction while you are still editing.
//
// Scope: the INDEX (git grep --cached), i.e. exactly the content that is
// about to be committed. That is the right unit for a pre-push guard: an
// unrelated uncommitted WIP in someone else's file cannot block your push,
// and whoever stages it gets flagged then. Cargo.lock is generated and
// LICENSE is verbatim upstream text, so both are out of scope.

import { execFileSync } from "node:child_process";

// The two characters, plus their HTML entities. The entity patterns are
// assembled from pieces so this guard does not match its own source (the
// rule text in CLAUDE.md and D-0064 names them the same careful way).
const AMP = "&";
const PATTERN = `\\x{2014}|\\x{2013}|${AMP}mdash;|${AMP}ndash;`;
const PATHSPEC = [".", ":!Cargo.lock", ":!LICENSE"];

let out = "";
try {
  out = execFileSync(
    "git",
    ["grep", "-n", "--cached", "-P", PATTERN, "--", ...PATHSPEC],
    { encoding: "utf8" }
  );
} catch (e) {
  // git grep exits 1 with no output when nothing matches: that is the pass.
  if (e.status === 1 && !e.stdout) {
    console.log("ok  no em/en dashes in the staged tree");
    process.exit(0);
  }
  if (e.status !== 1) {
    console.error(`em dash guard could not run: ${e.message}`);
    process.exit(2);
  }
  out = e.stdout || "";
}

const lines = out.split("\n").filter(Boolean);
for (const line of lines) console.error(line);
console.error(
  `\nFAIL: ${lines.length} line${lines.length === 1 ? "" : "s"} with a ` +
    "forbidden dash.\nUse a regular hyphen, a comma, a colon, or rewrite the " +
    "sentence (CLAUDE.md, D-0064)."
);
process.exit(1);

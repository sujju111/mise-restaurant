import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
execFileSync(
  "cargo",
  [
    "build",
    "-p",
    "mise-core",
    "--release",
    "--target",
    "wasm32-unknown-unknown",
  ],
  { stdio: "inherit" },
);
mkdirSync("public/engine", { recursive: true });
copyFileSync(
  "target/wasm32-unknown-unknown/release/mise_core.wasm",
  "public/engine/mise_core.wasm",
);

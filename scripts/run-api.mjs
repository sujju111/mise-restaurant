import { spawn } from "node:child_process";
const child = spawn("cargo", ["run", "-p", "mise-server"], {
  stdio: "inherit",
  env: process.env,
});
child.on("exit", (code) => process.exit(code ?? 1));

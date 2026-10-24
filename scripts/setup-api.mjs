import { randomBytes } from "node:crypto";
import { writeFileSync, existsSync } from "node:fs";
if (existsSync(".env.local"))
  throw new Error(
    ".env.local already exists; keep or explicitly rename it first.",
  );
const accounts = ["owner", "waiter", "kitchen", "cashier"].map((role) => ({
  role,
  name: role,
  tenant: "juniper",
  token: randomBytes(32).toString("hex"),
}));
accounts.push({
  role: "owner",
  name: "Ember owner",
  tenant: "ember",
  token: randomBytes(32).toString("hex"),
});
writeFileSync(
  ".env.local",
  `MISE_ACCOUNTS='${JSON.stringify(accounts)}'\nMISE_DATABASE=mise.sqlite\nMISE_BIND=127.0.0.1:8080\n`,
  { mode: 0o600 },
);
console.log(
  "Created private .env.local. Open it locally to copy your development staff token. Start the server with npm run api.",
);

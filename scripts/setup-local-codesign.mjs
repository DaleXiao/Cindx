#!/usr/bin/env node
// One-time local code-signing setup for Cindx development builds.
//
// Why: local builds were ad-hoc signed (`codesign --sign -`), whose designated
// requirement is a per-build cdhash. Every rebuild therefore invalidated the
// login keychain ACL entry for the provider API key, and macOS re-armed the
// "Cindx wants to use your confidential information" authorization prompt after
// each build. A stable self-signed code-signing identity gives every build the
// same designated-requirement anchor (`anchor = <cert hash> and identifier =
// app.cindx.desktop`), so one "Always Allow" click survives all future
// rebuilds and unattended runs (evaluations, scheduled tasks) never block on a
// password prompt.
//
// This script is idempotent: it exits early when the identity already exists.
// It creates a 10-year self-signed certificate ("Cindx Local Dev") with the
// codeSigning EKU in the login keychain, grants /usr/bin/codesign non-interactive
// access to its private key, and adds a user-domain trust setting for code
// signing. Expect exactly ONE GUI authorization prompt (the trust setting);
// everything else is non-interactive.
//
// Usage: node scripts/setup-local-codesign.mjs [--force]

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

const IDENTITY_NAME = "Cindx Local Dev";

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: "utf8", ...options });
  if (result.status !== 0) {
    process.stderr.write(result.stdout ?? "");
    process.stderr.write(result.stderr ?? "");
    throw new Error(`${command} ${args.join(" ")} failed with status ${result.status}`);
  }
  return result.stdout ?? "";
}

function identityExists() {
  const out = spawnSync("security", ["find-identity", "-v", "-p", "codesigning"], {
    encoding: "utf8"
  });
  return (out.stdout ?? "").includes(IDENTITY_NAME);
}

if (identityExists() && !process.argv.includes("--force")) {
  console.log(`Code-signing identity "${IDENTITY_NAME}" already exists; nothing to do.`);
  console.log("Local builds will sign with it automatically.");
  process.exit(0);
}

if (process.platform !== "darwin") {
  throw new Error("Local code signing setup is macOS-only.");
}

// LibreSSL's PKCS#12 output (3DES/SHA-1) is what `security import` accepts
// everywhere; Homebrew OpenSSL 3 defaults to AES-256 PBES2, which older
// import paths reject. Prefer the system binary, fall back to PATH openssl.
const openssl = fs.existsSync("/usr/bin/openssl") ? "/usr/bin/openssl" : "openssl";

const temporaryRoot = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-codesign-"));
try {
  const configPath = path.join(temporaryRoot, "openssl.cnf");
  const keyPath = path.join(temporaryRoot, "identity.key");
  const certPath = path.join(temporaryRoot, "identity.crt");
  const p12Path = path.join(temporaryRoot, "identity.p12");
  // Transport-only passphrase for the seconds between export and import; the
  // p12 never leaves the 0700 temporary directory and is shredded below.
  const p12Passphrase = `cindx-${process.pid}-${Date.now()}`;

  fs.writeFileSync(
    configPath,
    [
      "[req]",
      "distinguished_name = dn",
      "x509_extensions = v3",
      "prompt = no",
      "[dn]",
      `CN = ${IDENTITY_NAME}`,
      "[v3]",
      "basicConstraints = critical, CA:false",
      "keyUsage = critical, digitalSignature",
      "extendedKeyUsage = critical, codeSigning"
    ].join("\n")
  );

  run(openssl, [
    "req", "-x509", "-newkey", "rsa:2048",
    "-keyout", keyPath, "-out", certPath,
    "-days", "3650", "-nodes", "-config", configPath
  ]);

  const p12Args = ["pkcs12", "-export", "-inkey", keyPath, "-in", certPath,
    "-out", p12Path, "-passout", `pass:${p12Passphrase}`];
  const p12 = spawnSync(openssl, p12Args, { encoding: "utf8" });
  if (p12.status !== 0) {
    // Homebrew OpenSSL 3 needs -legacy for an import-compatible p12.
    run(openssl, [...p12Args, "-legacy"]);
  }

  const loginKeychain = path.join(os.homedir(), "Library", "Keychains", "login.keychain-db");
  // -T grants these binaries non-interactive access to the private key, so
  // `codesign` during builds never prompts.
  run("security", [
    "import", p12Path, "-k", loginKeychain, "-P", p12Passphrase,
    "-T", "/usr/bin/codesign", "-T", "/usr/bin/security"
  ]);

  console.log("");
  console.log("One macOS authorization prompt will appear now (adding the code-signing");
  console.log(`trust setting for "${IDENTITY_NAME}"). Approve it — this is the last`);
  console.log("password this setup ever asks for.");
  run("security", [
    "add-trusted-cert", "-r", "trustRoot", "-p", "codeSign",
    "-k", loginKeychain, certPath
  ]);

  if (!identityExists()) {
    throw new Error(
      `identity "${IDENTITY_NAME}" was created but is not listed for code signing; ` +
      "check `security find-identity -v -p codesigning`"
    );
  }
  console.log(`Code-signing identity "${IDENTITY_NAME}" is ready.`);
} finally {
  // Shred the plaintext key material.
  try {
    for (const entry of fs.readdirSync(temporaryRoot)) {
      const file = path.join(temporaryRoot, entry);
      if (entry.endsWith(".key") || entry.endsWith(".p12")) {
        const size = fs.statSync(file).size;
        const handle = fs.openSync(file, "r+");
        try {
          fs.writeSync(handle, Buffer.alloc(size, 0));
        } finally {
          fs.closeSync(handle);
        }
      }
    }
  } catch {
    // best effort
  }
  fs.rmSync(temporaryRoot, { recursive: true, force: true });
}

console.log("");
console.log("Next steps:");
console.log("  1. Rebuild and reinstall: node scripts/build-local-app.mjs");
console.log("  2. On first launch the keychain prompt appears ONE last time —");
console.log('     click "Always Allow". Future rebuilds never prompt again.');

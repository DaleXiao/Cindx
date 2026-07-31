import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);

function option(name, fallback) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
}

const contractPath = path.resolve(
  repoRoot,
  option("--contract", "apps/desktop/contracts/tauri-dto-v1.json")
);
const typescriptOnly = args.includes("--typescript-only");
const contract = JSON.parse(fs.readFileSync(contractPath, "utf8"));

if (contract.schema !== "cindx.desktop-dto-contract.v1") {
  throw new Error(`unexpected desktop DTO contract schema: ${contract.schema}`);
}
if (!Array.isArray(contract.cases) || contract.cases.length === 0) {
  throw new Error("desktop DTO contract must contain cases");
}

const identifiers = /^[A-Za-z_$][A-Za-z0-9_$]*$/;
const names = new Set();
const tsTypesByModule = new Map([
  ["tauriTypes", new Set()],
  ["tauriNativeTypes", new Set()]
]);
for (const entry of contract.cases) {
  if (!entry || !identifiers.test(entry.name) || !identifiers.test(entry.tsType)) {
    throw new Error("desktop DTO case names and TypeScript types must be identifiers");
  }
  if (names.has(entry.name)) throw new Error(`duplicate desktop DTO case: ${entry.name}`);
  names.add(entry.name);
  const tsModule = entry.tsModule ?? "tauriTypes";
  if (!tsTypesByModule.has(tsModule)) {
    throw new Error(`unsupported TypeScript module for ${entry.name}: ${tsModule}`);
  }
  tsTypesByModule.get(tsModule).add(entry.tsType);
  if (!["rust_to_ts", "ts_to_rust"].includes(entry.direction)) {
    throw new Error(`unsupported desktop DTO direction for ${entry.name}`);
  }
  if (!Array.isArray(entry.values) || entry.values.length === 0) {
    throw new Error(`desktop DTO case ${entry.name} must contain values`);
  }
}

function run(command, commandArgs, label, env = process.env) {
  const result = spawnSync(command, commandArgs, {
    cwd: repoRoot,
    env,
    encoding: "utf8"
  });
  if (result.status !== 0) {
    process.stderr.write(result.stdout ?? "");
    process.stderr.write(result.stderr ?? "");
    throw new Error(`${label} failed with status ${result.status}`);
  }
}

if (!typescriptOnly) {
  run(
    process.env.CARGO ?? "cargo",
    [
      "test",
      "--manifest-path",
      "apps/desktop/src-tauri/Cargo.toml",
      "--locked",
      "view_model_contract_tests::desktop_dto_contract_matches_committed_wire_values",
      "--",
      "--exact"
    ],
    "Rust desktop DTO contract"
  );
}

const temporaryRoot = fs.mkdtempSync(path.join(os.tmpdir(), "cindx-dto-contract-"));
try {
  const checkPath = path.join(temporaryRoot, "contract-check.ts");
  const tsconfigPath = path.join(temporaryRoot, "tsconfig.json");
  const typeImports = [...tsTypesByModule.entries()]
    .filter(([, types]) => types.size > 0)
    .map(([moduleName, types]) => {
      let typeModule = path.relative(
        temporaryRoot,
        path.join(repoRoot, `apps/desktop/src/${moduleName}.ts`)
      );
      if (!typeModule.startsWith(".")) typeModule = `./${typeModule}`;
      return `import type { ${[...types].sort().join(", ")} } from ${JSON.stringify(typeModule)};`;
    })
    .join("\n");

  const checks = contract.cases
    .map((entry, index) => {
      const variable = `case${index}`;
      return [
        `const ${variable} = ${JSON.stringify(entry.values, null, 2)} satisfies ${entry.tsType}[];`,
        `type ${entry.name}Wire = (typeof ${variable})[number];`,
        `type ${entry.name}Contract = Assert<ContractMatches<${entry.tsType}, ${entry.name}Wire>>;`
      ].join("\n");
    })
    .join("\n\n");

  const source = `${typeImports}

type Equal<Left, Right> =
  (<Value>() => Value extends Left ? 1 : 2) extends
  (<Value>() => Value extends Right ? 1 : 2) ? true : false;
type Assert<Value extends true> = Value;
type OptionalKeys<Value> = {
  [Key in keyof Value]-?: {} extends Pick<Value, Key> ? Key : never
}[keyof Value];
type NonNull<Value> = Exclude<Value, null | undefined>;
type UnionKeys<Value> = Value extends unknown ? keyof Value : never;
type WireValue<Value, Key extends PropertyKey> =
  Value extends unknown ? Key extends keyof Value ? Value[Key] : never : never;
type WireOptionalKeys<Value> = {
  [Key in UnionKeys<Value>]: [Value] extends [Record<Key, unknown>] ? never : Key
}[UnionKeys<Value>];
type ShapeKind<Value> =
  NonNull<Value> extends readonly unknown[] ? "array" :
  NonNull<Value> extends string ? "string" :
  NonNull<Value> extends number ? "number" :
  NonNull<Value> extends boolean ? "boolean" :
  NonNull<Value> extends object ? "object" : "unknown";
type ArrayItem<Value> = NonNull<Value> extends readonly (infer Item)[] ? Item : never;
type MismatchedFields<Declared, Wire> = {
  [Key in keyof Declared]-?:
    ContractMatches<Declared[Key], WireValue<Wire, Key>> extends true ? never : Key
}[keyof Declared];
type ObjectContractMatches<Declared, Wire> =
  Equal<keyof Declared, UnionKeys<Wire>> extends true ?
  Equal<OptionalKeys<Declared>, WireOptionalKeys<Wire>> extends true ?
  [MismatchedFields<Declared, Wire>] extends [never] ? true : false
  : false : false;
type ContractMatches<Declared, Wire> =
  [Wire] extends [never] ? true :
  Equal<null extends Declared ? true : false, null extends Wire ? true : false> extends true ?
  Equal<ShapeKind<Declared>, ShapeKind<Wire>> extends true ?
  ShapeKind<Declared> extends "array" ? ContractMatches<ArrayItem<Declared>, ArrayItem<Wire>> :
  ShapeKind<Declared> extends "object" ? ObjectContractMatches<NonNull<Declared>, NonNull<Wire>> : true
  : false : false;

${checks}
`;
  fs.writeFileSync(checkPath, source);
  fs.writeFileSync(
    tsconfigPath,
    JSON.stringify(
      {
        extends: path.join(repoRoot, "apps/desktop/tsconfig.json"),
        compilerOptions: { noEmit: true },
        files: [checkPath]
      },
      null,
      2
    )
  );

  run(
    process.execPath,
    [path.join(repoRoot, "apps/desktop/node_modules/typescript/bin/tsc"), "-p", tsconfigPath],
    "TypeScript desktop DTO contract"
  );
} finally {
  fs.rmSync(temporaryRoot, { recursive: true, force: true });
}

console.log(
  JSON.stringify({
    schema: contract.schema,
    cases: contract.cases.length,
    rust_checked: !typescriptOnly,
    typescript_checked: true
  })
);

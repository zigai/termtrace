import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import Ajv2020 from "ajv/dist/2020.js";
import standaloneCode from "ajv/dist/standalone/index.js";
import { build } from "esbuild";
import { compileFromFile } from "json-schema-to-typescript";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = resolve(packageRoot, "../..");
const outputDirectory = resolve(packageRoot, "src/generated");
const contracts = ["driver-request", "driver-response"];

await mkdir(outputDirectory, { recursive: true });

for (const contract of contracts) {
  const schemaName = `${contract}.schema.json`;
  const schemaPath = resolve(repositoryRoot, "schemas", schemaName);
  const typesPath = resolve(outputDirectory, `${contract}.ts`);
  const validatorPath = resolve(outputDirectory, `validate-${contract}.ts`);
  const types = await compileFromFile(schemaPath, {
    bannerComment: `/* Generated from schemas/${schemaName}. DO NOT EDIT. */`,
    cwd: repositoryRoot,
    style: { semi: true, singleQuote: false, tabWidth: 2, trailingComma: "all" },
    unknownAny: true,
  });
  await writeFile(typesPath, types);

  const schema = JSON.parse(await readFile(schemaPath, "utf8"));
  const ajv = new Ajv2020({
    allErrors: false,
    code: { esm: true, lines: true, source: true },
    strict: true,
    validateFormats: false,
  });
  const validate = ajv.compile(schema);
  const validator = standaloneCode(ajv, validate);
  const bundled = await build({
    bundle: true,
    format: "esm",
    legalComments: "none",
    minify: true,
    platform: "node",
    stdin: {
      contents: validator,
      loader: "js",
      resolveDir: packageRoot,
      sourcefile: `validate-${contract}.js`,
    },
    write: false,
  });
  const output = bundled.outputFiles[0];
  if (output === undefined) throw new Error(`esbuild produced no ${contract} validator`);
  await writeFile(
    validatorPath,
    `/* Generated from schemas/${schemaName}. DO NOT EDIT. */\n// @ts-nocheck -- generated and bundled Ajv validator code\n${output.text}`,
  );
}

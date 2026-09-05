import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import Ajv2020 from "ajv/dist/2020.js";
import standaloneCode from "ajv/dist/standalone/index.js";
import { build } from "esbuild";
import { compileFromFile } from "json-schema-to-typescript";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = resolve(packageRoot, "../..");
const outputDirectory = resolve(packageRoot, "src/transport/generated");
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

    if (contract === "driver-response") {
        const boundaryDefinitions = [
            ["control-data", "ControlData"],
            ["screen-frame", "ScreenFrame"],
        ];
        for (const [boundaryName, definitionName] of boundaryDefinitions) {
            const boundarySchema = {
                $schema: schema.$schema,
                $ref: `#/$defs/${definitionName}`,
                $defs: schema.$defs,
            };
            const validateBoundary = ajv.compile(boundarySchema);
            const boundaryValidator = standaloneCode(ajv, validateBoundary);
            const boundaryBundle = await build({
                bundle: true,
                format: "esm",
                legalComments: "none",
                minify: true,
                platform: "node",
                stdin: {
                    contents: boundaryValidator,
                    loader: "js",
                    resolveDir: packageRoot,
                    sourcefile: `validate-${boundaryName}.js`,
                },
                write: false,
            });
            const boundaryOutput = boundaryBundle.outputFiles[0];
            if (boundaryOutput === undefined)
                throw new Error(`esbuild produced no ${boundaryName} validator`);
            await writeFile(
                resolve(outputDirectory, `validate-${boundaryName}.ts`),
                `/* Generated from schemas/${schemaName}#/$defs/${definitionName}. DO NOT EDIT. */\n// @ts-nocheck -- generated and bundled Ajv validator code\n${boundaryOutput.text}`,
            );
        }
    }
}

const styleSchemaPath = resolve(packageRoot, "src/screen/style-filter.schema.json");
const styleSchema = JSON.parse(await readFile(styleSchemaPath, "utf8"));
const styleAjv = new Ajv2020({
    allErrors: false,
    code: { esm: true, lines: true, source: true },
    strict: true,
    validateFormats: false,
});
const validateStyle = styleAjv.compile(styleSchema);
const styleBundle = await build({
    bundle: true,
    format: "esm",
    legalComments: "none",
    minify: true,
    platform: "node",
    stdin: {
        contents: standaloneCode(styleAjv, validateStyle),
        loader: "js",
        resolveDir: packageRoot,
        sourcefile: "validate-style-filter.js",
    },
    write: false,
});
const styleOutput = styleBundle.outputFiles[0];
if (styleOutput === undefined) throw new Error("esbuild produced no style-filter validator");
await writeFile(
    resolve(outputDirectory, "validate-style-filter.ts"),
    `/* Generated from src/screen/style-filter.schema.json. DO NOT EDIT. */\n// @ts-nocheck -- generated and bundled Ajv validator code\n${styleOutput.text}`,
);

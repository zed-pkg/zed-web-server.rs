#!/usr/bin/env node

import { constants, brotliCompressSync, brotliDecompressSync } from "node:zlib";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const checkOnly = process.argv.slice(2).includes("--check");

const assets = [
  ["assets/dependency-graph.js", "static/dependency-graph.js.br"],
  ["assets/dependency-graph.css", "static/dependency-graph.css.br"],
];

let invalid = false;
for (const [sourceName, outputName] of assets) {
  const sourcePath = path.join(repositoryRoot, sourceName);
  const outputPath = path.join(repositoryRoot, outputName);
  const source = readFileSync(sourcePath);

  if (checkOnly) {
    const committed = readFileSync(outputPath);
    let decoded;
    try {
      decoded = brotliDecompressSync(committed);
    } catch (error) {
      console.error(`${outputName} is not valid Brotli: ${error.message}`);
      invalid = true;
      continue;
    }
    if (!decoded.equals(source)) {
      console.error(`${outputName} does not decode to ${sourceName}`);
      invalid = true;
    }
  } else {
    const encoded = brotliCompressSync(source, {
      params: {
        [constants.BROTLI_PARAM_MODE]: constants.BROTLI_MODE_TEXT,
        [constants.BROTLI_PARAM_QUALITY]: 11,
        [constants.BROTLI_PARAM_SIZE_HINT]: source.byteLength,
      },
    });
    writeFileSync(outputPath, encoded);
    console.log(`${sourceName} -> ${outputName} (${source.byteLength} -> ${encoded.byteLength} bytes)`);
  }
}

if (invalid) process.exitCode = 1;

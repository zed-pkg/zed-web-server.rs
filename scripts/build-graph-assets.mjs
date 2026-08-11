#!/usr/bin/env node

import { constants, brotliCompressSync } from "node:zlib";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const checkOnly = process.argv.slice(2).includes("--check");

const assets = [
  ["assets/dependency-graph.js", "static/dependency-graph.js.br"],
  ["assets/dependency-graph.css", "static/dependency-graph.css.br"],
];

let stale = false;
for (const [sourceName, outputName] of assets) {
  const sourcePath = path.join(repositoryRoot, sourceName);
  const outputPath = path.join(repositoryRoot, outputName);
  const source = readFileSync(sourcePath);
  const encoded = brotliCompressSync(source, {
    params: {
      [constants.BROTLI_PARAM_MODE]: constants.BROTLI_MODE_TEXT,
      [constants.BROTLI_PARAM_QUALITY]: 11,
      [constants.BROTLI_PARAM_SIZE_HINT]: source.byteLength,
    },
  });

  if (checkOnly) {
    const committed = readFileSync(outputPath);
    if (!committed.equals(encoded)) {
      console.error(`${outputName} is stale; run node scripts/build-graph-assets.mjs`);
      stale = true;
    }
  } else {
    writeFileSync(outputPath, encoded);
    console.log(`${sourceName} -> ${outputName} (${source.byteLength} -> ${encoded.byteLength} bytes)`);
  }
}

if (stale) process.exitCode = 1;

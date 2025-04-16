#!/usr/bin/env node

import archiver from "archiver";
import { stat, mkdir } from "fs/promises";
import * as path from "path";
import { createWriteStream } from "fs";
import { spawn } from "child_process";
import process from "process";
import pino from "pino";

/**
 * Find the project root by looking upwards from this script until we find
 * `Cargo.lock` or hit the root.
 */
async function findProjectRoot(startDir: string): Promise<string> {
  let dir = startDir;
  for (;;) {
    const cargoLock = path.join(dir, "Cargo.lock");
    try {
      await stat(cargoLock);

      return dir;
    } catch (err: unknown) {
      if (
        !(err instanceof Error) ||
        !("code" in err) ||
        err.code !== "ENOENT"
      ) {
        throw err;
      }
    }

    const parent = path.dirname(dir);
    // We got to the root
    if (parent === dir) {
      throw new Error("Could not find Cargo.lock in any parent directory");
    }

    dir = parent;
  }
}

const __filename = new URL(import.meta.url).pathname;
const __dirname = path.dirname(__filename);

const PROJECT_ROOT = await findProjectRoot(__dirname);
const DIST_DIR = path.resolve(__dirname, "..", "dist");
const TARGET = "aarch64-unknown-linux-gnu";
const PACKAGE_NAME = "lambda-rssfilter";
const BINARY_PATH = path.join(
  PROJECT_ROOT,
  "target",
  TARGET,
  "release",
  PACKAGE_NAME,
);
const ZIP_PATH = path.join(DIST_DIR, "lambda-rssfilter.zip");

const transport = process.stdout.isTTY
  ? {
      target: "pino-pretty",
      options: {
        ignore: "pid,hostname,time",
      },
    }
  : undefined;

const logger = pino({
  transport,
});

async function ensureDistDir(): Promise<void> {
  try {
    const dirStat = await stat(DIST_DIR);

    if (!dirStat.isDirectory()) {
      throw new Error(
        `${DIST_DIR} already exists but is not a directory - remove it to continue.`,
      );
    }

    return;
  } catch (err: unknown) {
    if (!(err instanceof Error) || !("code" in err) || err.code !== "ENOENT") {
      throw err;
    }

    await mkdir(DIST_DIR, { recursive: true });
  }
}

function buildRustBinary(): Promise<void> {
  return new Promise((resolve, reject) => {
    logger.info(`Building Rust binary for target ${TARGET}...`);

    const args = ["build", "--target", TARGET, "--package", PACKAGE_NAME];

    if (
      process.env.GITHUB_EVENT_NAME === "push" &&
      process.env.GITHUB_REF === "refs/heads/main"
    ) {
      args.push("--release");
    }

    const cargo = spawn("cargo", args, { cwd: PROJECT_ROOT, stdio: "inherit" });

    cargo.on("exit", (code) => {
      if (code !== 0) {
        reject(new Error(`cargo build failed with code ${code}`));
        return;
      }

      resolve();
    });
  });
}

async function zipBinary(): Promise<void> {
  const output = createWriteStream(ZIP_PATH);
  const archive = archiver("zip", { zlib: { level: 9 } });

  const closePromise = new Promise<void>((resolve, reject) => {
    logger.info(`Zipping binary for AWS Lambda...`);

    output.on("close", () => {
      resolve();
    });

    archive.on("error", (err) => {
      logger.error("while zipping", err);
      reject(err);
    });

    archive.pipe(output);
    archive.file(BINARY_PATH, { name: "bootstrap" });
  });

  await archive.finalize();
  await closePromise;
}

try {
  await ensureDistDir();
  await buildRustBinary();
  await zipBinary();

  logger.info(`Build and zip complete: ${ZIP_PATH}`);
} catch (err: unknown) {
  logger.error(err instanceof Error ? err.message : err);

  process.exit(1);
}

import * as pulumi from "@pulumi/pulumi";
import archiver from "archiver";
import { buffer } from "stream/consumers";
import { hashElement } from "folder-hash";
import type {
  HashElementNode as DirectoryHash,
  HashElementOptions,
} from "folder-hash";

interface BuildRustProviderInputs {
  readonly directory: string;
  readonly packageName: string;
  readonly target?: string;
}

export type BuildRustInputs = {
  [K in keyof BuildRustProviderInputs]: pulumi.Input<
    BuildRustProviderInputs[K]
  >;
};

type Hash = string;

interface BuildRustProviderOutputs extends BuildRustProviderInputs {
  readonly name: string;
  readonly zipData: string;
  readonly directoryHash: Hash;
}

// Note: This will be in es2024, so we can remove this when we upgrade to that
function withResolvers<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
} {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });

  return { promise, resolve, reject };
}

class BuildRustProvider implements pulumi.dynamic.ResourceProvider {
  private async zipFiles(filePath: string): Promise<string> {
    const fs = await import("fs");
    const stream = await import("stream");

    await pulumi.log.debug(`zipping ${filePath}`);

    // create an output stream in memory for the zip file
    const passthrough = new stream.PassThrough();
    const bufferStream = buffer(passthrough);

    // create a zip file
    const zip = archiver("zip");

    // Resolve when the zip is finished
    const finishPromise = new Promise<void>((resolve, reject) => {
      zip.on("error", reject);
      zip.on("finish", resolve);
    });

    zip.pipe(passthrough);

    // add the file to the zip
    zip.append(fs.createReadStream(filePath), {
      name: "bootstrap",
    });

    await zip.finalize();
    await finishPromise;

    await pulumi.log.debug("zip file created");

    passthrough.end();

    await pulumi.log.debug("zip file finished");

    return (await bufferStream).toString("base64");
  }

  private async build(
    tempDir: string,
    inputs: BuildRustProviderInputs,
  ): Promise<{ zipData: string }> {
    const child_process = await import("child_process");

    const { directory, packageName, target } = inputs;
    const targetArg = target ? ["--target", target] : [];

    await pulumi.log.debug("building rust binary");

    const cargo = child_process.spawn(
      "cargo",
      [
        "build",
        "--release",
        "--package",
        inputs.packageName,
        "--target-dir",
        tempDir,
        ...targetArg,
      ],
      {
        cwd: directory,
        // Close stdin, pipe stdout and stderr to the parent process
        stdio: ["ignore", "inherit", "inherit"],
      },
    );

    // https://github.com/typescript-eslint/typescript-eslint/issues/8113
    // eslint-disable-next-line @typescript-eslint/no-invalid-void-type
    const { promise, resolve, reject } = withResolvers<void>();

    // Resolve when the cargo process exits
    cargo.on("exit", (code) => {
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`cargo build failed with code ${code}`));
      }
    });

    await promise;

    await pulumi.log.debug("cargo build succeeded");

    const targetDir = target ? `${target}/` : "";

    const zipData = await this.zipFiles(
      `${tempDir}/${targetDir}release/${packageName}`,
    );

    return {
      zipData: zipData,
    };
  }

  private async hashDirectory(
    base: string,
  ): Promise<BuildRustProviderOutputs["directoryHash"]> {
    const options: HashElementOptions = {
      algo: "sha256",
      encoding: "base64",
      files: { exclude: [".*"] },
      folders: { exclude: [".*", "pulumi", "static", "target"] },
    };

    await pulumi.log.debug(`hashing ${base}`);

    const hashes = await hashElement(base, options);

    await pulumi.log.debug(`hash result: ${JSON.stringify(hashes)}`);

    return hashes.hash;
  }

  public check(
    _olds: BuildRustProviderInputs,
    news: BuildRustProviderInputs,
  ): Promise<pulumi.dynamic.CheckResult<BuildRustProviderInputs>> {
    return Promise.resolve({ inputs: news });
  }

  public read(
    id: pulumi.ID,
    inputs: BuildRustProviderOutputs,
  ): Promise<pulumi.dynamic.ReadResult<BuildRustProviderOutputs>> {
    return Promise.resolve({ id, props: inputs });
  }

  public async create(
    inputs: BuildRustProviderInputs,
  ): Promise<pulumi.dynamic.CreateResult> {
    const { mkdtemp, rm } = await import("fs/promises");
    const join = (await import("path")).join.bind(null);
    const tmpdir = (await import("os")).tmpdir;

    const tempDir = await mkdtemp(join(tmpdir(), "build-rust-"));

    try {
      const { directory, packageName, target } = inputs;

      const directoryHash = await this.hashDirectory(directory);
      const { zipData } = await this.build(tempDir, inputs);

      return {
        id: `${this.name}-${directory}-${packageName}-${target ?? "default-target"}`,
        outs: {
          ...inputs,
          name: this.name,
          zipData,
          directoryHash,
        } satisfies BuildRustProviderOutputs,
      };
    } finally {
      await rm(tempDir, { recursive: true });
    }
  }

  constructor(private readonly name: string) {}

  public async diff(
    _id: pulumi.ID,
    olds: BuildRustProviderOutputs,
    news: BuildRustProviderInputs,
  ): Promise<pulumi.dynamic.DiffResult> {
    const { directoryHash: oldDirectoryHash } = olds;

    const replaces = [];

    if (this.name !== olds.name) {
      replaces.push("name");
    }

    if (olds.directory !== news.directory) {
      replaces.push("directory");
    }

    if (olds.packageName !== news.packageName) {
      replaces.push("packageName");
    }

    if (olds.target !== news.target) {
      replaces.push("target");
    }

    if (replaces.length > 0) {
      return {
        changes: true,
        replaces: [...replaces, "directoryHash", "zipData"],
      };
    }

    await pulumi.log.debug(`diffing ${news.directory}`);

    const newHash = await this.hashDirectory(news.directory);

    await pulumi.log.debug(
      `old hash: ${oldDirectoryHash}, new hash: ${newHash}`,
    );

    const changes = newHash !== oldDirectoryHash;

    if (changes) {
      await pulumi.log.info("project has changes, rebuilding");
    }

    return {
      changes,
      replaces: ["directoryHash", "zipData"],
    };
  }

  public async update(
    _id: pulumi.ID,
    _olds: BuildRustProviderOutputs,
    news: BuildRustProviderInputs,
  ): Promise<pulumi.dynamic.UpdateResult> {
    return this.create(news);
  }
}

export class ZippedRustBinary extends pulumi.dynamic.Resource {
  // Inputs
  public readonly name!: pulumi.Output<string>;
  public readonly directory!: pulumi.Output<string>;
  public readonly packageName!: pulumi.Output<string>;
  public readonly target!: pulumi.Output<string>;

  // Outputs
  public readonly zipData!: pulumi.Output<string>;
  public readonly directoryHash!: pulumi.Output<DirectoryHash>;

  constructor(
    name: string,
    args: BuildRustInputs,
    opts?: pulumi.CustomResourceOptions,
  ) {
    super(
      new BuildRustProvider(name),
      `build-rust-lambda:${name}`,
      {
        directoryHash: undefined,
        zipData: undefined,
        ...args,
      },
      opts,
    );
  }
}

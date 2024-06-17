import type { Stats } from "fs";

export const SkipDir = Symbol("skipDir");

/// Walk walks a directory tree, calling the provided callback with the path to
/// each file or directory it finds. For directories, the callback is called
/// before the directory's contents are walked. If the callback returns
/// SkipDir, the directory's contents are not walked. This allows for efficient
/// skipping of directory hierarchies that are not of interest.
///
/// @param dir The directory to start walking from.
/// @param callback The callback to call with the path to each file found. The
/// callback is called with the path to the file and its stats.
export async function walk(
  dir: string,
  callback: (
    path: string,
    stats: Stats,
  ) => undefined | typeof SkipDir | Promise<undefined | typeof SkipDir>,
): Promise<void> {
  const fs = (await import("fs")).promises;
  const path = await import("path");

  const files = await fs.readdir(dir);

  for (const file of files) {
    const filePath = path.join(dir, file);
    const stat = await fs.lstat(filePath);

    if (stat.isDirectory()) {
      // can skip the directory if the callback returns SkipDir
      if (callback(filePath, stat) === SkipDir) {
        continue;
      }

      await walk(filePath, callback);
      continue;
    }

    await callback(filePath, stat);
  }
}

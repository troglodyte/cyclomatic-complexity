import { accessSync, constants } from "node:fs";
import { homedir } from "node:os";

/** Everything resolution touches outside itself, injected so it can be tested. */
export interface ResolveEnv {
  /** The `PATH` variable, unsplit. */
  path: string;
  /** The user's home directory. */
  home: string;
  windows: boolean;
  isExecutable: (candidate: string) => boolean;
}

/** The real environment. */
export function realEnv(): ResolveEnv {
  return {
    path: process.env.PATH ?? "",
    home: homedir(),
    windows: process.platform === "win32",
    isExecutable: (candidate) => {
      try {
        accessSync(candidate, constants.X_OK);
        return true;
      } catch {
        return false;
      }
    },
  };
}

/**
 * Locate the `rustcx` executable: the configured path, then `PATH`, then Cargo's
 * bin directory.
 *
 * The Cargo fallback is not redundant. A VS Code launched from the Dock or the
 * Start menu does not inherit the shell's `PATH`, so `~/.cargo/bin` is routinely
 * invisible to the second step even though the binary is installed and works
 * fine in a terminal.
 *
 * A configured path that is not executable resolves to `undefined` rather than
 * silently falling through, so a typo surfaces as an error instead of as
 * mysteriously different behavior.
 */
/**
 * Path joining and `PATH` splitting driven by [ResolveEnv.windows] rather than by
 * `node:path`, whose `join` and `delimiter` are fixed to the *running* platform.
 * Going through the injected flag is what makes the Windows branch testable from
 * a Linux checkout; on a real host the flag matches the platform, so the
 * behavior is the same as `node:path`'s.
 */
function child(windows: boolean, dir: string, ...rest: string[]): string {
  const sep = windows ? "\\" : "/";
  return rest.reduce(
    (acc, part) => (acc.endsWith(sep) ? acc + part : acc + sep + part),
    dir,
  );
}

export function resolveBinary(configured: string, env: ResolveEnv): string | undefined {
  const name = env.windows ? "rustcx.exe" : "rustcx";

  const explicit = configured.trim();
  if (explicit.length > 0) {
    return env.isExecutable(explicit) ? explicit : undefined;
  }

  for (const dir of env.path.split(env.windows ? ";" : ":")) {
    if (dir.length === 0) continue;
    const candidate = child(env.windows, dir, name);
    if (env.isExecutable(candidate)) return candidate;
  }

  const cargo = child(env.windows, env.home, ".cargo", "bin", name);
  return env.isExecutable(cargo) ? cargo : undefined;
}

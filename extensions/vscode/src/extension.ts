import * as vscode from "vscode";
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node";
import { realEnv, resolveBinary } from "./resolve";

let client: LanguageClient | undefined;

const NOT_FOUND =
  "cmplx was not found on PATH or in ~/.cargo/bin. Install it with " +
  "'cargo install --path crates/cmplx', or set cmplx.path.";

const NOT_EXECUTABLE = (configured: string) =>
  `cmplx is not executable at '${configured}'. Fix cmplx.path.`;

/**
 * The threshold to send, or `undefined` to send nothing.
 *
 * Sending nothing is what lets `cmplx.toml` apply: the server resolves
 * initialization options *over* the file, so a default sent from here would
 * silently disable that key in every project.
 */
function threshold(): number | undefined {
  const configured = vscode.workspace.getConfiguration("cmplx").get<number | null>("threshold");
  return typeof configured === "number" && configured > 0 ? configured : undefined;
}

async function start(): Promise<void> {
  const configured = vscode.workspace.getConfiguration("cmplx").get<string>("path", "");
  const binary = resolveBinary(configured, realEnv());

  if (binary === undefined) {
    const message = configured.trim().length > 0 ? NOT_EXECUTABLE(configured) : NOT_FOUND;
    const action = await vscode.window.showWarningMessage(message, "Open Settings");
    if (action === "Open Settings") {
      await vscode.commands.executeCommand("workbench.action.openSettings", "cmplx");
    }
    return;
  }

  const serverOptions: ServerOptions = { command: binary, args: ["--lsp"] };

  const configured_threshold = threshold();
  const clientOptions: LanguageClientOptions = {
    documentSelector: ["javascript", "javascriptreact", "typescript", "typescriptreact", "rust"].map(
      (language) => ({ scheme: "file", language }),
    ),
    initializationOptions:
      configured_threshold === undefined ? undefined : { threshold: configured_threshold },
  };

  client = new LanguageClient("cmplx", "cmplx", serverOptions, clientOptions);
  await client.start();
}

async function stop(): Promise<void> {
  const running = client;
  client = undefined;
  await running?.stop();
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  await start();

  // The server reads its options once, at `initialize`, and implements no
  // workspace/didChangeConfiguration handler — so a settings change means a
  // restart. Editing cmplx.toml does not: the server watches its mtime.
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (!event.affectsConfiguration("cmplx")) return;
      await stop();
      await start();
    }),
  );
}

export async function deactivate(): Promise<void> {
  await stop();
}

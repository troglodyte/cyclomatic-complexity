/** The VS Code language ids `cmplx --lsp` scores. */
const LANGUAGES = ["javascript", "javascriptreact", "typescript", "typescriptreact", "rust"] as const;

/**
 * The URI schemes documents are scored in: saved files, and unsaved
 * (`untitled:`) buffers.
 *
 * The server picks a document's language from the extension in its URI, so an
 * unsaved buffer is scored when its URI has one (`code new.rs` opens
 * `untitled:/…/new.rs`); a bare `untitled:Untitled-1` gets no diagnostics.
 */
const SCHEMES = ["file", "untitled"] as const;

/** One entry of a language client's document selector. */
export interface DocumentFilter {
  readonly scheme: string;
  readonly language: string;
}

/** Every scheme paired with every language. */
export function documentSelector(): DocumentFilter[] {
  return SCHEMES.flatMap((scheme) => LANGUAGES.map((language) => ({ scheme, language })));
}

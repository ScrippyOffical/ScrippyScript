import * as vscode from "vscode";

// Grammar-only for now: VS Code loads the TextMate file itself, so activation
// just marks the language as live. A WASM-backed language client or a formatter
// would be registered here later.
export function activate(context: vscode.ExtensionContext): void {
    console.log(`ScrippyScript ${context.extension.packageJSON.version} activated`);
}

export function deactivate(): void {}

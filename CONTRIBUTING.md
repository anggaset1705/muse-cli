# Contributing to Muse CLI

Thanks for wanting to help. Keep it simple:

## How to contribute

1. **Fork** this repo.
2. Create a branch for your change (`git checkout -b fix/short-name`).
3. Make your change.
4. Run the tests (`cargo test` in the Rust workspace) and make sure the app still builds (`npm run tauri build` in `muse-tauri/`).
5. Open a **pull request** against `main`, describing what changed and why.

Small, focused pull requests are easier to review than big ones. If you are unsure about a bigger change, open an issue first to discuss it.

## Reporting bugs

Open an **Issue** and include:

- The app version (see the version shown in the app, e.g. `v0.1.0`)
- Your Windows version
- Steps to reproduce the problem
- What you expected vs. what actually happened
- A screenshot, if it helps

## Good first issues

Issues labeled **`good first issue`** are picked for newcomers: small, well-defined fixes that don't need deep knowledge of the codebase. If you are new here, start with one of those.

## Ground rules

- **Do not commit build output** (`target/`, `node_modules/`, installers, `dist/`).
- **Do not commit secrets** — no API keys, tokens, cookies, or account credentials of any kind. If you need a value for local testing, keep it in a local, untracked file.
- **Do not commit other people's data** — no chat history, screenshots with personal information, or account details.
- Run the tests before opening a pull request.
- Write commit messages that say what the change does, in plain words.

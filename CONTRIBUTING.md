# Contributing

Thanks for helping! A couple of practical notes.

## License of contributions

Capicorn is dual-licensed: **GPL-3.0-or-later** for the public, plus a **commercial
license** the author can offer for closed-source use. For that to be possible, every
contribution must be usable under both. By opening a pull request you confirm that:

- you wrote the code (or otherwise have the right to submit it), and
- you license it to the project under **GPL-3.0-or-later**, and
- you allow the author to also license your contribution under the **commercial license**.

If you are not comfortable with that, please open an issue to discuss first.

## Development

See the [README](README.md) for prerequisites. The short version:

```bash
pnpm install
pnpm tauri dev          # run in development
```

Before a PR: keep `tsc --noEmit` clean, run `cargo test` for the backend, and describe
what you tested. Screenshots or a short clip of UI changes are very welcome.

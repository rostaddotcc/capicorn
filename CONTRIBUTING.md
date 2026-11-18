# Contributing

Thanks for helping! A couple of practical notes.

## Issues first

Bugs and ideas go through the [issue forms](https://github.com/rostaddotcc/snagcraft/issues/new/choose)
— the README's [Bugs, ideas & feedback](README.md#-bugs-ideas--feedback) section
defines the format, including a fixed-heading template for **AI agents** filing via
`gh` or the API. One issue per problem, name the edition (stable v1.5.x vs Rust
pre-release), and reference the issue in your PR.

## License of contributions

SnagCraft is dual-licensed: **GPL-3.0-or-later** for the public, plus a **commercial
license** the author can offer for closed-source use. For that to be possible, every
contribution must be usable under both. By opening a pull request you confirm that:

- you wrote the code (or otherwise have the right to submit it), and
- you license it to the project under **GPL-3.0-or-later**, and
- you allow the author to also license your contribution under the **commercial license**.

If you are not comfortable with that, please open an issue to discuss first.

## Development

See the [README](README.md) for prerequisites. The short version:

```bash
# Stable line (Tauri)
pnpm install
pnpm tauri dev          # run in development

# Rust edition
cd rust
cargo run -p snagcraft
```

Before a PR:

- **Stable line:** keep `tsc --noEmit` clean, run `cargo test` for the backend.
- **Rust edition:** `cargo test --workspace --no-fail-fast`, `cargo clippy --workspace`
  (deny-lints must stay clean), and `cargo xtask layers` (crate layering is enforced).
  UI changes drift kittest snapshots — re-bless with `UPDATE_SNAPSHOTS=1` and verify
  the new snapshots, never blindly.

Describe what you tested. Screenshots or a short clip of UI changes are very welcome.

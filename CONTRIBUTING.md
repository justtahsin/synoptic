# Contributing to Synoptic

Thanks for your interest! Synoptic is young and contributions of every size
are welcome.

## Development setup

A recent stable Rust toolchain is all you need:

```sh
cargo build            # debug build
cargo run -p synoptic-app -- --page 1   # open on a specific page (0-5)
cargo run -p synoptic-core --example top        # data layer without the GUI
cargo run -p synoptic-core --example services   # systemd service listing
```

## Before opening a PR

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI enforces all three.

## Project layout

- `core/` — `synoptic-core`: UI-independent data collection (`/proc`,
  systemd, XDG autostart). MIT OR Apache-2.0. No UI dependencies, ever.
- `app/` — `synoptic-app`: the Slint UI. GPL-3.0-or-later.
  UI definition lives in `app/ui/app.slint`.

## Guidelines

- The UI must never require root; privileged actions go through polkit.
- Blocking calls never run on the UI thread.
- Match the surrounding code style; comments in English.
- UI strings are Turkish today; a gettext-based i18n workflow is on the
  roadmap — help welcome.

## License of contributions

Contributions to `core/` are accepted under MIT OR Apache-2.0, to `app/`
under GPL-3.0-or-later.

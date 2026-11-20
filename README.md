# MacMischer

Keyboard-first, fast desktop controller for the Behringer **XR18** (X Air family), written in Rust (egui).
Runs on macOS, Linux and Windows.

## Run

```sh
cargo run --release -p macmischer            # finds the mixer automatically
cargo run --release -p macmischer -- --ip 10.5.5.48
cargo run --release -p macmischer -- --mock  # built-in fake mixer, no hardware needed
```

macOS app bundle: `cargo install cargo-bundle && cargo bundle --release -p macmischer`
→ `target/release/bundle/osx/MacMischer.app`.

## Keyboard

| Keys | Action |
|---|---|
| `1`…`16` | select channel (or bank position, per bank) — commits automatically |
| `a1`–`a6`, `f1`–`f4`, `x1`–`x4`, `d1`–`d4`, `r`, `l` | bus, FX return, FX send, DCA, aux in, main |
| `↑` `↓` | fader ±1 dB (Shift ±3, Alt ±0.1); `0 0` = 0 dB |
| `←` `→`, Tab | previous / next strip in the bank |
| `,` `.` (`<` `>`) | pan |
| `m` / `s` | mute / solo |
| `[` `]`, F1–F12 | switch fader bank |
| Ctrl/Cmd+K / R / L | scenes / routing / lock |
| `?` | help |

## Concepts

- **Fader banks**: any strips in any order; a bank can be bound to a bus (sends-on-fader monitor mix) and can use position numbering. Double-click a bank tab to edit.
- **Shows**: banks are saved per show (TOML, autosaved). Show menu: new / copy / import banks / rename / open folder.
- **Safety**: *Safe* mode asks before scene recall; *Lock* lets only mutes, solo and the main fader through.
- Stereo-linked pairs appear as one wide strip.

## Layout

```
crates/xair-proto  OSC protocol model (params, fader law, meters, routing codes) + xair-probe CLI
crates/xair-core   network thread: sync, keepalive, coalescing, reconnect, state mirror
crates/xair-mock   in-process fake XR18 for tests
crates/app         egui application
docs/PLAN.md       plan + progress log      docs/PROTOCOL.md  verified protocol notes
```

# MacMischer

Keyboard-first, fast desktop controller for the Behringer **XR18** (X Air family), written in Rust (egui).
Runs on macOS, Linux and Windows.

## Run

```sh
cargo run --release -p macmischer            # finds the mixer automatically
cargo run --release -p macmischer -- --ip 10.5.5.48
cargo run --release -p macmischer -- --mock  # built-in fake mixer, no hardware needed
```

macOS one-step install (build, sign, copy to /Applications, remove build bundles): `tools/install-macos.sh` (`--clean` also wipes `target/`).

macOS app bundle by hand: `cargo install cargo-bundle && cargo bundle --release -p macmischer`
→ `target/release/bundle/osx/MacMischer.app`.

Linux desktop integration (icon in launcher/title bar under Wayland):
`install -Dm644 assets/icon/icon-256.png ~/.local/share/icons/hicolor/256x256/apps/macmischer.png`
and `install -Dm644 assets/macmischer.desktop ~/.local/share/applications/macmischer.desktop`.

## Releases and updates

Pushing a `v<ver>` tag builds and publishes a DMG (macOS), an installer (Windows) and an AppImage (Linux). The app checks GitHub at launch and, after you confirm, installs the new version for your platform in place. Details: [docs/RELEASING.md](docs/RELEASING.md).

The icon source is `assets/icon/icon.svg`; regenerate PNGs with `cargo run -p icon-gen`.

## Keyboard

| Keys | Action |
|---|---|
| `1`…`16` | select the n-th strip of the bank (or mixer channel, per bank) — commits automatically |
| `a1`–`a6`, `f1`–`f4`, `x1`–`x4`, `d1`–`d4`, `r`, `l` | bus, FX return, FX send, DCA, aux in, main |
| `↑` `↓` | fader ±1 dB (Shift ±3, Alt ±0.1); `0 0` = 0 dB |
| `←` `→`, Tab | previous / next strip in the bank |
| `,` `.` (`<` `>`) | pan |
| `m` / `s` | mute / solo |
| `b` | A/B compare: stores the selected strip's HPF/EQ/gate/comp as A and B on first press, then toggles (undoable; cleared on mixer switch; blocked when locked) |
| `[` `]`, F1–F12 | switch fader bank |
| Ctrl/Cmd+K / R / F / L | scenes / routing / effects / lock |
| (EQ / dynamics view) **Presets** menu | save the selected strip's HPF/EQ/gate/comp as a named preset (TOML in `<config>/MacMischer/presets/`), load it onto any strip (only parameters that exist there are applied; the message shows applied/skipped; key-source selections and polarity are not stored; values are range-checked), delete, or "Undo preset" (also restores the linked partner; dropped by later edits of that strip); loading asks first in Safe mode or whenever the preset comes from another strip family or has skipped parameters; overwriting an existing name asks first; blocked when locked |
| Ctrl/Cmd+Z / Y | undo / redo your last edits (fader drags count as one step; cleared on scene recall) |
| `?` | help |

## Concepts

- **Fader banks**: any strips in any order; a bank can be bound to a bus (sends-on-fader monitor mix) and can use position numbering. Double-click a bank tab to edit.
- **Shows**: banks are saved per show (TOML, autosaved). Show menu: new / copy / import banks / rename / open folder.
- **Safety**: *Safe* mode asks before scene recall; *Lock* lets only mutes, solo and the main fader through.
- Stereo-linked pairs appear as one wide strip.
- **Web remote** (Ctrl/Cmd+P): phones/tablets on the LAN control selected strips — e.g. a musician's
  own monitor mix (page bound to a bus) or a capped TV-room volume. Scan the QR code; optional PIN;
  "Add to Home Screen" for a full-screen app. Try it without hardware:
  `cargo run -p xair-remote --example serve_mock` → http://localhost:8019/. Protocol: docs/REMOTE.md.
- **MIDI** (Ctrl/Cmd+M, toolbar "MIDI"): a hardware controller drives faders, pans, mute, solo, strip
  select and scene recall. Pick the device, choose a target, press *Learn* and move a control.
  CC faders/pans use soft takeover (move past the current position first; fader CC 95 is about unity;
  default cap +6 dB per fader). Notes toggle mute/solo, CCs set them (>= 64 = on). Lock applies to MIDI;
  scene recall over MIDI is refused in Safe mode. Settings live in `prefs.toml` (not the show file);
  a vanished device is re-attached automatically. Build without it: `--no-default-features`
  (Linux needs ALSA headers for the default build). Not tested with real hardware yet.
- **RTA overlay** (EQ view, "RTA" toggle, off by default): live spectrum from the mixer's `/meters/4` drawn
  behind the EQ curve (display only, -100..0 dB). Subscribed only while shown (switching off just stops renewing; the mixer stops streaming within ~10 s). The blob layout and band
  frequencies are unverified against hardware; the analysed source is chosen on the mixer.

## Layout

```
crates/xair-proto  OSC protocol model (params, fader law, meters, routing codes) + xair-probe CLI
crates/xair-core   network thread: sync, keepalive, coalescing, reconnect, state mirror
crates/xair-mock   in-process fake XR18 for tests
crates/app         egui application
docs/PLAN.md       plan + progress log      docs/PROTOCOL.md  verified protocol notes
```

# MacMischer — Rust control app for Behringer XR18 — Plan

> Progress file, kept current across sessions. On resume: read **Status**, then continue at the first unchecked milestone.

## Status
- [x] Requirements gathered (3 rounds, 2026-11-20) — see Decisions
- [x] Architecture + milestones written
- [x] User approval (2026-11-20)
- [x] M0 Scaffold (2026-11-20): workspace builds, 3 fader-law tests pass, clippy clean, dark window launches
- [x] M1 Protocol spike (2026-11-20): see docs/PROTOCOL.md
- [x] M2 Core (2026-11-20): proto model + xair-core + xair-mock; 4 integration tests; hardware full sync 693 params in ~300 ms, meters 20 Hz
- [x] M3 Mixing UI (2026-11-20): strips, meter bridge, detail panel (gain/+48V/pan/sends), Main strip fixed right, bank tabs; verified on hardware (read-only)
- [x] M4 Keyboard + banks (2026-11-20): number entry w/ early commit, keymap, nudges, bank editor (rename/reorder/add/remove/bind to bus), monitor banks, help (?). Keyboard NOT yet tested by user
- [x] M5 Show files + connection UX (2026-11-20): Show menu (open/new/copy/import banks/rename/open folder), autosave 0.8 s after change + on exit, remembered mixer auto-connect, mixer picker, auto-retry while mixer off, Switch mixer menu
- [x] M6 Scenes & routing + safety → v1 (2026-11-20): scenes window (recall/save/rename, confirm), routing window (inputs analog/USB, USB sends, aux outs, Ultranet, main read-only), Safe/Simple mode, Lock (Ctrl/Cmd+L), README + cargo-bundle metadata. NOT verified on hardware: snapshot save/recall, routing writes (permission to change mixer routing was denied during probing; inferred enum codes flagged in PROTOCOL.md)
- [~] M7+ post-v1 — DONE: channel processing (2026-11-20: EQ view with draggable curve + low cut, gate/compressor view with transfer curves; keys v/e/c). OPEN: FX rack, web remote, RTA overlay, undo/A-B, presets, MIDI

## Context
Keyboard-first, fast, good-looking desktop app to control a Behringer XR18 (X Air) over the network.
Usability + performance over customisability (explicitly not a Mixing Station clone).
Main target: new MacBook (Apple Silicon); must also build/run on Windows and Linux (dev machine is Linux).
Real XR18 is always reachable during development.

## Decisions (from Q&A)
- **Use cases:** all — live FOH, monitor mixes, studio/recording, rehearsal/church/podcast.
- **UI stack:** `eframe`/`egui` with wgpu backend: GPU immediate-mode → smooth 60–120 fps meters, easy custom widgets (faders, meters, later EQ curve/RTA), one codebase for macOS/Win/Linux, good keyboard handling.
- **Input:** keyboard FIRST; trackpad/mouse; touch-friendly sizing; MIDI later (none owned yet → `Action` abstraction now so MIDI just maps to actions later).
- **Channel select:** type digits + auto-commit (~400 ms or Enter): `12` → ch 12. Prefixes: `a`=bus 1–6, `f`=FX return 1–4, `x`=FX send 1–4, `d`=DCA 1–4, `l`=main LR, `r`=aux in (`m`/`s` are mute/solo). Esc cancels.
- **Level keys:** ↑/↓ ±1 dB, Shift ±3 dB, Alt ±0.1 dB; ←/→ previous/next strip in bank (user feedback); pan on `,` `.` (`<` `>` coarse); `M` mute, `S` solo; `0` `0` = unity; scroll/drag with mouse.
- **Number keys per bank:** each bank chooses *bank position* (DEFAULT, user choice 2026-11-20: 3 = 3rd fader, shown as #3 under strips) or *mixer channel* (12 = Ch 12). Prefixed entries stay global.
- **Stereo links:** linked ch/bus pairs (`/config/chlink/1-2`, `/config/buslink/1-2`) show as one wide strip, two meters, one fader. Mixer mirrors linked fader/mute itself but doesn't echo to sender → `Mixer::set` mirrors locally + sends both; solo mirrored in app.
- **DCA editing:** inputs show D1–D4 assignment toggles; selecting a DCA shows a member grid (bitmask `/…/grp/dca`, bit0 = DCA1 — verified on hardware).
- **Fader banks:** user-defined, any mix of strips (inputs, aux in, FX returns, buses, DCAs, main), any order. Rotate with `[` / `]`, jump with F1–F12. A bank may be **bound to a bus** → its faders control sends to that bus (sends-on-fader monitor bank), shown with strong bus colour.
- **Layout:** top = detail panel of selected strip (v1: name/colour, gain, phantom, pan, sends, routing/tap; later gate/EQ/comp); middle = current bank strips with meters; fixed right area = buses + main; bottom = bank tabs. **Always-visible meter bridge** for all inputs + buses + main.
- **Look:** dark pro console — dark grey, coloured channel tags (mixer colours), bright meters, large readable numbers.
- **Safety:** selectable mode — *Simple* (no guards) or *Safe* (scene recall needs confirm keypress; Lock mode toggle allowing only main fader + mutes).
- **Connect:** auto-discover via broadcast, list mixers, remember last, auto-reconnect; manual IP fallback.
- **Storage:** banks/layout/keymap per **show file** (TOML), with "new show from template / import banks from other show". Global app prefs (last mixer, safety mode, recent shows) in OS config dir (`directories` crate).
- **v1 scope:** core mixing + scenes & routing. **Later:** channel processing (HPF/gate/EQ/comp), FX rack, RTA overlay, undo + A/B compare, offline show editing, channel preset library, MIDI (MCU + MIDI-learn).

## Protocol facts (verify each on hardware in M1 spike → docs/PROTOCOL.md)
- OSC over UDP, mixer port **10024**. Discovery: broadcast `/xinfo` → reply with IP, name, model, firmware.
- `/xremote` every <10 s to receive change pushes.
- Meters: `/meters/<n>` subscription, blob of int16 LE (1/256 dB), renew every <10 s.
- Addresses (X32-like): `/ch/NN/mix/fader|on|pan`, `/ch/NN/mix/MM/level`, `/ch/NN/config/name|color`, `/headamp/NN/gain|phantom`, `/rtn/aux/...`, `/rtn/N/...`, `/bus/N/...`, `/dca/N/...`, `/lr/...`, `/routing/...`, `/-snap/load|save|name`, `/-stat/solosw/NN`.
- Fader law (float 0..1 ↔ dB): ≥0.5: 40f−30; ≥0.25: 80f−50; ≥0.0625: 160f−70; else 480f−90; 0 = −∞.
- Full-state sync on connect by querying each address (paced); test whether `/node` bulk queries work on XR.

## Architecture
```
crates/
  xair-proto/   OSC encode/decode (rosc), typed address model, value conversions, meter parsing. Pure, unit-tested.
  xair-core/    Connection thread: UdpSocket, discovery, keepalives, initial sync, outgoing coalescing
                (latest-wins, ~50 Hz), MixerState, commands in / events out via crossbeam, repaint hook.
  xair-mock/    In-process fake XR18 for tests/offline UI dev.
  app/          eframe app: views, Action enum + keymap, digit-input state machine, show files, safety, theme.
```
- UI never blocks on network; optimistic updates; meters in separate latest-value buffer.
- Every input (key, mouse, later MIDI) → `Action` → single dispatcher.

## Milestones
- **M0 Scaffold:** workspace, `cargo test`/`clippy`, empty eframe window w/ dark theme.
- **M1 Protocol spike (hardware):** CLI `xair-probe` — discover, info, set/get fader, meters dump, `/node`, snapshots, routing. Findings → `docs/PROTOCOL.md`.
- **M2 Core:** xair-proto + xair-core + mock; full state sync; reconnect; tests against mock.
- **M3 Mixing UI:** strips, bus/main area, meter bridge, detail panel.
- **M4 Keyboard + banks:** digit selection, nudges, keymap, banks editor, bus-bound banks, rotation.
- **M5 Show files + connection UX.**
- **M6 Scenes & routing + safety** → **v1 release** (macOS `.app` via cargo-bundle, Linux/Windows builds).
- **Web remote (added 2026-11-20, user request):** the app serves a small web page so phones/tablets on the LAN control *selected* strips.
  - Inspiration only: ~/xrvol (Python TV-volume remote for DCA 2; buggy — do not copy). Keep its good ideas: per-control max cap (speaker protection), big touch targets, iPhone home-screen/PWA fullscreen, honest status ("mixer not responding"), never jump before first sync, don't overwrite a slider while dragging, real dB readout, muted = red.
  - Avoid its flaws: no polling (push via WebSocket from xair-core state), absolute values only (no relative/toggle races: mute sends the target value), one shared mixer connection (the app's), no hardcoded IP/channel, validate input, optional PIN, bind to LAN, origin check (CSRF).
  - Config in the show file: "remote pages" (name, list of strips or a bank, optional bus = personal monitor mix, per-strip max level, PIN). QR code in the app to open a page on a phone. Embedded server (axum + tokio on its own thread, or tiny_http + tungstenite).
- **M7+:** processing, FX, RTA, undo/A-B, offline editing, presets, MIDI.

## Verification
- Unit tests: OSC round-trip, address mapping, fader law, meter parsing, digit-input state machine, keymap, show files.
- Integration: xair-core vs xair-mock (sync, coalescing, reconnect).
- Hardware per milestone: app ↔ official X Air app sync both ways; meters; snapshot recall; latency <20 ms, 60 fps.
- Run on macOS + Linux each milestone; Windows build check before v1.

## Session log
- 2026-11-20: planning done, M0 done. Notes: egui/eframe 0.37 API = `App::ui(&mut self, ui, frame)`, `egui::Panel::top(..).show(ui, ..)`; eframe needs `x11`+`wayland` features on Linux with default-features off. Deps: rosc 0.11, toml 1, directories 6. Next: M1 probe CLI against real XR18.
- 2026-11-20: M1 done — probe CLI (cargo run -p xair-proto --bin xair-probe), findings in PROTOCOL.md. Next: M2 core.
- 2026-11-20: M2 done. Learned: Wi-Fi drops packets on query bursts → sliding-window sync (24 in flight, 250 ms retry ×3); headamp 17/18 have no phantom; DCA/LR have config/name+color. Hardware check: `cargo run -p xair-core --example sync_check -- 10.5.5.48` (read-only). Next: M3 mixing UI.
- 2026-11-20: M3 done. Run: `cargo run -p macmischer` (discovers), `-- --ip X`, `-- --mock`. Right area holds Main only (buses live in a bank; 7 fixed strips too wide on laptop). Solo index mapping in StripId::solo_index is UNVERIFIED — ask user to press solos in X Air app while listening. Screenshots on Wayland: `spectacle -b -n -a -o file.png`; kill app with `pkill -x macmischer` (NOT -f). Next: M4 keyboard + banks.
- 2026-11-20: M4 done. Number entry commits early when no longer number is possible (e.g. `2`, `a3`). Tab/Shift+Tab steps strips in bank; monitor bank → right strip shows that bus master + header banner. Don't inject keys with ydotool (could hit other windows) — ask user to test. Next: M5 show files + connection UX.
- 2026-11-20: User feedback round 1 → per-bank positional numbering, ←/→ strip stepping, bank editor fits screen + painted icons (default egui font lacks arrow glyphs: avoid ↑↓←→✕＋●), DCA assignment UI. Dev aid: MM_SHOW=editor|help|dca. Ideas offered: DCA spill bank, rename/colour strips.
- 2026-11-20: Buses + FX sends have grp/dca + grp/mute (verified); LR has none. Added mute-group membership + 4 mute-group masters (/config/mute/N) and phones/monitor level (/config/solo/level) in right column. Not yet handled: channel/bus stereo links (/config/chlink/1-2, /config/buslink/1-2).
- 2026-11-20: Stereo-linked pairs as wide strips (verified on hardware: 3-4, 5-6, 7-8, 9-10, 15-16, bus 3-4 linked). Next: M5.
- 2026-11-20: M5 done. Files: shows in data dir (`~/.local/share/macmischer/shows/*.toml`, macOS `~/Library/Application Support/MacMischer/shows`), prefs in config dir (`~/.config/macmischer/prefs.toml`). Keymap is not yet stored in show files (deferred until keymap editing exists). Next: M6 scenes, routing, safety.
- 2026-11-20: M6 done → v1 feature-complete. Open items before calling v1 released: user test of keyboard, scene recall/save, routing edits on hardware; solo index mapping; build .app on the MacBook. Next: M7 (processing, FX, RTA, undo, MIDI) on user's priority.
- 2026-11-20: Icon (assets/icon/icon.svg → `cargo run -p icon-gen`). Bank-position numbering default. Channel processing done; hardware-verified scales in PROTOCOL.md. Processing edits not yet exercised on hardware (read-only checks only). Next: user picks between FX rack / web remote / RTA / undo / MIDI.

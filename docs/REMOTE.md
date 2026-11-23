# Web remote — design & protocol

The desktop app serves a small web page so phones/tablets on the LAN can control
**selected strips** (e.g. a musician's own monitor mix, or the TV-room volume on a DCA).

## Principles (lessons from the ~/xrvol prototype)
- Push, never poll: one WebSocket per client; the server sends changes.
- Clients send **absolute** values only (level 0..1, mute on/off) — no toggles/relative steps → no races.
- Clients never name OSC addresses. They address strips *of their page*; the server maps to
  fader or bus send, applies the per-page level cap, and rejects anything else.
- One mixer connection (the app's). The remote follows the app when it switches mixer.
- Optional PIN; Origin check against the Host header (CSRF); bind to the LAN; validate all input.
- The page never jumps before the first state arrives, and never overwrites a control while
  the user touches it.

## Configuration (in the show file)

```toml
[remote]
enabled = true
port = 8018
pin = "1234"          # optional; empty = no PIN

[[remote.pages]]
name = "Drums monitor"
strips = ["1", "2", "3-4", "FX1"]   # strip ids as in banks ("3-4" not special: use "3"; linked pairs collapse)
bus = "B2"            # optional: faders control the sends to Bus 2 (personal monitor mix)
max_db = 4.0          # optional: level cap in dB (fader law), default +10
meters = true         # show meters (default true)
```

A linked pair is shown as one strip (id = the odd member, `"stereo": true`); fader and mute move both,
and it has two meters (`meters` carries both sides: `3` is L, `3:R` is R). Strips that are stereo by nature (Aux In, FX returns, Main) are `"stereo": true` too and also get two meters. Double-tapping a fader resets it to 0 dB, or to `max_db` if that is lower;
mute is left as it is.

## HTTP
| Path | |
|---|---|
| `GET /` | the single-page app (HTML/CSS/JS inline) |
| `GET /manifest.webmanifest`, `GET /icon.png` | PWA install (iOS/Android home screen) |
| `GET /ws` | WebSocket upgrade (Origin must match Host) |

## WebSocket messages (JSON, one object per text frame)

### Client → server
```json
{"t":"auth","pin":"1234"}                 // first message if the server requires a PIN
{"t":"open","page":0}                     // subscribe to a page (index from "pages")
{"t":"level","strip":"3","value":0.62}    // absolute fader/send position 0..1 (fader law)
{"t":"mute","strip":"3","muted":true}     // absolute
```

### Server → client
```json
{"t":"hello","name":"SEBIS-XR18","pin":true}          // pin = PIN required
{"t":"auth","ok":false}                                // wrong PIN (connection stays open for retry)
{"t":"pages","pages":[{"name":"Drums monitor"},{"name":"TV"}]}   // after auth (or immediately without PIN)
{"t":"page","index":0,"name":"Drums monitor","bus":"Bus 2","max":0.85,
 "strips":[{"id":"1","name":"Kick","color":"#e84a4a","stereo":false}, …]}
{"t":"state","connected":true,
 "strips":{"1":{"value":0.75,"db":"0.0","muted":false}, …}}     // full state of the open page; sent on open and on every change
{"t":"meters","m":{"1":-23.5,"3":-40.1}}                         // dBFS, ~15 Hz, only if page.meters
{"t":"error","msg":"…"}
```
- `value` is the raw 0..1 position (fader law; `db` is its formatted dB, "-inf" for 0).
- `max` is the cap as raw position; the client must not offer values above it (server clamps anyway).
- `muted` reflects the strip's mute (`mix/on` inverted). In a bus page, mute still mutes the strip
  itself (affects the main mix too) — the client labels it accordingly.
- `connected:false` → the app has no live mixer; show a banner, disable controls.

# XR18 / X Air OSC protocol notes (hardware-verified)

Verified on XR18V2 "SEBIS-XR18", firmware 1.19, IP 10.5.5.48, UDP port 10024 (M1 spike, 2026-11-20, tool: `xair-probe`). Items marked **TBD** are not yet confirmed.

## Basics

- Transport: UDP, mixer port 10024. Query = send an address with no args; reply = same address carrying the value.
- Never send "set" while probing; reads are side-effect free.
- `/xremote` must be re-sent every <10 s to receive pushes of changes made elsewhere. **TBD**: not yet observed live (verify in M2).

## Discovery / identity

| Request | Reply |
|---|---|
| `/xinfo` (broadcast to 10.5.255.255:10024) | `/xinfo s:ip s:name s:model s:firmware` |
| `/status` | `s:"active" s:ip s:name` |
| `/info` | `s:"V0.04" s:name s:model s:fw` |

## Typed per-address queries (exact floats; use for state sync)

| Address | Reply example | Notes |
|---|---|---|
| `/ch/01/config/name` | `s:"Sub"` | |
| `/ch/01/config/color` | `i:2` | |
| `/ch/01/config/insrc`, `/ch/01/config/rtnsrc`, `/ch/01/preamp/rtnsw` | `i:0` | |
| `/ch/01/mix/fader` | `f:0.7497556` | = 767/1023; 1024 steps, quantize to n/1023 |
| `/ch/01/mix/on` | `i:1` | 1 = unmuted |
| `/ch/01/mix/pan` | `f:0.5` | |
| `/ch/01/mix/lr` | `i:1` | assigned to main |
| `/ch/01/mix/01/level`, `/ch/01/mix/01/tap` | `f`, `i:4` | send to bus 1 |
| `/ch/01/grp/dca` | `i:1` | bitmask |
| `/headamp/01/gain`, `/headamp/01/phantom` | `f:0.4167`, `i:0` | |
| `/lr/mix/fader`, `/bus/1/mix/fader`, `/fxsend/1/mix/fader` | `f` | |
| `/rtn/aux/mix/fader`, `/rtn/1/mix/fader` | `f` | aux return / FX return |
| `/dca/1/fader` | `f` | |
| `/-snap/index` | `i:1` | current snapshot |
| `/-snap/01/name` | `s:"Home"` | 64 slots |
| `/-stat/solosw/01`, `/config/solo/source`, `/-prefs/name` | `i:0`, `i:3`, `s` | |
| `/-stat/lock`, `/-stat/selidx` | (no reply) | |

### Routing

| Address | Reply |
|---|---|
| `/routing/main/01` | `i:0` |
| `/routing/aux/01` | `i:26 i:10` (2 ints) |
| `/routing/usb/01` | `i:0 i:0` |
| `/routing/usb/01/src` | `i:0` |
| `/routing/p16/01` | `i:0 i:3` |

#### Routing parameters (fw 1.19)

Typed per-address reads/writes (single value each):

- Channel input (verified): `/ch/NN/preamp/rtnsw` 0 = analog, 1 = USB return; `/ch/NN/config/insrc` 0..15 = In01..In16; `/ch/NN/config/rtnsrc` 0..17 = U01..U18.
- Aux in (verified): `/rtn/aux/preamp/rtnsw` 0/1; `/rtn/aux/config/rtnsrc` 0..8 = USB pairs U01-02..U17-18. No `insrc`.
- Outputs: `/routing/{aux/01..06, usb/01..18, p16/01..16}/{src,pos}`; `/routing/main/01..02/src` (no pos; main/01 = 0 "LR", main/02 = 1 "MON").
- Source codes: verified 0..17 (Ch01..Ch16, AuxL, AuxR) and 26..31 (Bus1..6). Inferred: 18..25 FX1L..FX4R, 32..35 Send1..4, 36/37 L/R, 38..55 U01..U18.
- Tap (`pos`) codes: verified 0 AIN, 3 IN+M, 10 POST. Inferred: 1 AIN+M, 2 IN, 4 PREEQ, 5 PREEQ+M, 6 POSTEQ, 7 POSTEQ+M, 8 PRE, 9 PRE+M.

## `/node` bulk read

- Request: `/node ,s "<path without leading slash>"`, e.g. `ch/01/mix`.
- Reply quirk: the OSC address is `node` **without a leading slash**; `rosc` rejects it as BadPacket, so a custom tolerant decoder is required.
- Payload: one text string, newline-separated lines, values in human/display units. Large nodes are split across several packets.

```
/ch/01/mix ON   0.0 ON 0.0000
/ch/01/mix/01   0.0 OFF POST 0.0000
/ch/01/mix/02   -oo OFF POST
/ch/01/config "Sub" 2 In01 U01
/routing/aux/01 Bus1 POST
/-snap "Home" 1 0 0 0
/-snap/01 "Home" "+++..."
```

- Conclusion: `/node` is good for fast snapshot-name listing (slot 02 = "PA 2M + BT") and discovery/text info. Use typed per-address queries for state sync (exact floats).

## Meters

- Subscribe: `/meters ,s "/meters/<n>"`. Mixer streams `/meters/<n>` with one blob at ~20 Hz (40 packets / 2 s).
- Renew every <10 s (standard X Air behaviour; not explicitly timed out here).
- Blob: i32 LE count, then count x i16 LE in 1/256 dB (-128 dB = silence).

| Bank | Count | Content |
|---|---|---|
| `/meters/0` | 8 | selected channel strip meters |
| `/meters/1` | 40 | likely: 0-15 ch 1-16, 16-17 aux L/R, 18-25 FX returns 1-4 L/R, 26-31 buses 1-6, 32-35 FX sends 1-4, 36-37 main L/R, 38-39 monitor/phones L/R (observed buses ~-95 at 26-31, rest -128; **confirm grouping with signal**) |
| `/meters/2` | 36 | 18 inputs (16 ch preamp + aux L/R) then 18 USB returns |
| `/meters/4` | 100 | RTA bins (all -128 while RTA inactive); see "RTA (TBD)" below |
| `/meters/5` | 44 | dynamics/other, **TBD** |
| `/meters/6` | 39 | first 4 = -60, rest 0; **TBD** |

### RTA (TBD, unverified; implemented in M8.4)

- Subscribed with the same request as the other banks (`/meters ,s "/meters/4"`), renewed with them, and only while the app's RTA overlay is shown. No unsubscribe is sent when it is switched off: renewals stop and the mixer stops streaming when the subscription lapses (~10 s); the client discards the data meanwhile.
- Observed: only that `/meters/4` carries a header value of 100 and 100 values, all -128 while the RTA is inactive (earlier probe). Not observed: any real spectrum data, so whether the layout is 100 i16 as assumed or the X Air docs' 50 packed i32 (two 16-bit values each) is unknown; the header value 100 suggests 100 shorts.
- `xair_proto::meters::parse_rta` assumes: i32 count header, then >= 100 i16 LE values in 1/256 dB; first 100 used, rest ignored, count header not trusted, shorter blobs rejected. Values clamped to -128..0 dB.
- Band centres are ASSUMED log-spaced 20 Hz..20 kHz over 100 bands (`rta_band_hz`); the real band layout (and whether the X Air docs' blob carries extra trailing values) is unchecked. Verify with a sine sweep on hardware.
- The analysed source is chosen on the mixer (`/-prefs/rta/source`, int, per X Air docs; also unverified). The app does NOT read or set it (no `Param`); change it on the mixer or via xair-probe.
- UI mapping: -100..0 dB across the EQ graph height, frequency via the EQ graph's x-axis.

## Fader law (float 0..1 to dB)

| f range | dB |
|---|---|
| >= 0.5 | 40f - 30 |
| >= 0.25 | 80f - 50 |
| >= 0.0625 | 160f - 70 |
| else | 480f - 90 |

0 = -inf; 0.75 = 0 dB. Implemented in `crates/xair-proto/src/fader.rs`.

## Channel processing (`xair_proto::proc`, `scale`)

Paths are `<strip prefix><suffix>`. Float values are 0..1 on the wire; scales below were verified against hardware (`/node` text).

| Block | Suffix | Type / scale |
|---|---|---|
| HPF (Ch only) | `/preamp/hpon`, `/preamp/invert` | int 0/1 |
| | `/preamp/hpf` | log 20..400 Hz (`20*20^x`); no `hpslope` |
| EQ (Ch, AuxIn, FxRtn: 4 bands; Bus, Main: 6; FX send/DCA: none) | `/eq/on` | int |
| | `/eq/<b>/type` | int 0 LCut, 1 LShv, 2 PEQ, 3 VEQ, 4 HShv, 5 HCut |
| | `/eq/<b>/f` | log 20..20000 Hz (`20*1000^x`), 201 steps |
| | `/eq/<b>/g` | lin -15..+15 dB, 61 steps |
| | `/eq/<b>/q` | log 10..0.3 (`10*(0.03)^x`), 72 steps (x = n/71) |
| | `/eq/mode` (6-band) | int, 0 = PEQ; read-only for us, not modelled |
| Gate (Ch only) | `/gate/on`, `mode` | int; mode 0 EXP2, 1 EXP3, 2 EXP4, 3 GATE, 4 DUCK |
| | `thr` / `range` / `attack` | lin -80..0 dB / 3..60 dB / 0..120 ms |
| | `hold` / `release` | log 0.02..2000 ms (`0.02*1e5^x`) / 5..4000 ms (`5*800^x`) |
| | `keysrc`, `filter/on`, `filter/type` | int; filter type 0 LC6, 1 LC12, 2 HC6, 3 HC12, 4 1.0, 5 2.0, 6 3.0, 7 5.0, 8 10.0 |
| | `filter/f` | as EQ frequency |
| Compressor `/dyn` (Ch, Bus, Main) | `on`, `mode` (0 COMP, 1 EXP), `det` (0 PEAK, 1 RMS), `env` (0 LIN, 1 LOG), `auto`, `keysrc`, `filter/on`, `filter/type` | int |
| | `ratio` | int enum [1.1, 1.3, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 7.0, 10, 20, 100] |
| | `thr` / `knee` / `mgain` / `mix` | lin -60..0 dB / 0..5 / 0..24 dB / 0..100 % |
| | `attack` / `hold` / `release` | as gate |
| | `filter/f` | as EQ frequency |

Notes: `/lr/dyn/keysrc` does not answer on the XR18 (excluded from the sync set). `/ch/NN/insert/on|sel` are not modelled yet.

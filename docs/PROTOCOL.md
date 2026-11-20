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
| `/meters/4` | 100 | RTA bins (all -128 while RTA inactive) |
| `/meters/5` | 44 | dynamics/other, **TBD** |
| `/meters/6` | 39 | first 4 = -60, rest 0; **TBD** |

## Fader law (float 0..1 to dB)

| f range | dB |
|---|---|
| >= 0.5 | 40f - 30 |
| >= 0.25 | 80f - 50 |
| >= 0.0625 | 160f - 70 |
| else | 480f - 90 |

0 = -inf; 0.75 = 0 dB. Implemented in `crates/xair-proto/src/fader.rs`.

# MacMischer

A keyboard-first desktop control surface for Behringer X-Air mixers: an operator selects strips, moves faders, recalls scenes and edits channel processing, from keyboard, mouse, MIDI or a web remote.

## Language

**Console**:
The module that applies operator Actions to the mixer under the Lock, Safe mode and undo rules.
_Avoid_: controller, engine, manager

**Action**:
One operator intent (nudge the selected fader, toggle mute, recall a scene), regardless of whether it came from keyboard, mouse or MIDI.
_Avoid_: command, event

**Outcome**:
What the Console reports back after applying Actions: the message to show the operator.
_Avoid_: result, response

**Lock**:
An operator-set state in which only navigation, mutes, solo and the main fader respond; every other Action is refused with a message.
_Avoid_: freeze, read-only

**Safe mode**:
A preference under which risky Actions (loading a preset, recalling a scene) ask for confirmation first.
_Avoid_: Simple mode

## Mixer concepts

**Strip**:
One controllable slot on the mixer: an input channel, bus, FX return, DCA or the main output.
_Avoid_: channel (only an input strip is a channel)

**Linked pair**:
Two adjacent Strips (1-2, 3-4, …) that the mixer treats as stereo, so fader, mute and send writes apply to both. Shown as one wide Strip under its odd (left) member.
_Avoid_: stereo channel

**RTA**:
Real-time analyzer: a live spectrum of one selectable source on the mixer (default: main LR). _Auto gain_ makes the visible dB window follow the signal level.
_Avoid_: spectrum meter

**Bank**:
An operator-defined, named list of Strips shown together; a bus-bound bank makes the level controls act on sends to that bus.
_Avoid_: page, layer

**Scene**:
A stored mixer snapshot in a numbered slot, with a name.
_Avoid_: snapshot, memory

**Preset**:
A named, saved set of one Strip's processing settings that can be loaded onto another Strip.
_Avoid_: template

**A/B compare**:
Switching one Strip between two held snapshots of its processing to compare them.
_Avoid_: toggle compare

## Releases

**Release**:
A published, versioned build of MacMischer on GitHub, with one asset per platform (macOS, Windows, Linux).
_Avoid_: version (for the published artifact), build

**Update**:
Replacing the installed app with a newer Release, only after the Operator confirms.
_Avoid_: upgrade, patch, auto-update

# ACID RECALL changelog

## Unreleased

- **MIDI on Linux:** controller input, MIDI Learn and MIDI Clock IN/OUT now work on Linux through
  the ALSA sequencer (`midir` crate). It delivers the same packed message words as the Windows
  and macOS backends, so mappings and routing files are unchanged.

## 1.0.0 — 22 November 2026

First stable ACID RECALL release, built on OpenRB-338.

[Official website](https://acidrecall.com) · [Release notes](docs/releases/1.0.0.md) ·
[GitHub release](https://github.com/smldms/Acid-Recall/releases/tag/acid-recall-v1.0.0)

### Rack and patterns

- Fixed Graphite rack with two TB-303s, TR-808, TR-909, eight-lane sampler and right-hand effects.
- TR patterns default to 16 steps; 32/64-step patterns reveal pages on the left with
  COPY, PASTE, DUP and FOLLOW. Changing length keeps all rack controls in place and retains
  hidden steps. Groove Generator follows the full phrase length; Undo Random restores it.
- TB step menus use a note-name grid, including upper C. Independent A/S marks show
  Accent and Slide on the keys; Rest and octave options remain available.
- Techno, IDM and Break generation, linked TB patterns and two independent arpeggiators.
- Compact routing labels and CON/CLA/MAR switches; MASTER has a separate COMP button.

### Sampler and effects

- Focus layout with waveform, playheads, IN/OUT trimming, Pitch/Level/Reverb knobs and
  ONE SHOT, AUTO CUT, GATE, LOOP and REVERSE modes. Solo/Mute and right-click unloading.
- Eight lanes with sequences up to 64 bars and WAV/AIFF/MP3 import.
- Pitch-preserving WARP with numbered musical grid, local stretch anchors, snap and zoom.
- Optional BPM/transient analysis, half/double BPM controls and suggested anchors.
- Sampler/WARP undo and redo, including loading and unloading samples.
- Independent GATER and SIDECHAIN source/voice choices; GLOBAL sidechain compatibility.
- Stereo convolution reverb with IR import and clearly separated SPACE controls.
- Delay STEPS knob and STRAIGHT/TRIPLET timing directly on the rack.

### MIDI, audio and sessions

- Right-click MIDI Learn/removal, multiple controller inputs, soft takeover and optional
  per-control MAX from 0 to 127. Machine/effect buttons follow absolute 0/127 states.
- MIDI Clock MASTER/OUT and SLAVE/IN with 24 PPQN, Start/Continue/Stop and Song Position Pointer.
- New OUT configurations default to 30 ms delay; IN has a signed phase offset and stable
  nominal-tempo display. Stop/Space returns to measure 1.
- Every-sample peak metering, orange at −6 dBFS, red at 0 dBFS, peak hold and CLIP latch;
  stereo master dBFS values are measured before clipping.
- Smoothed TR output trims after insert effects, with both drums at 0 dB by default.
- Saved ASIO/WASAPI preferences on Windows x64: device, stereo outputs, sample rate and buffer.
- New portable sessions default to .recall. Legacy .orb338 projects still open, and Save
  keeps their filenames. The binary container format is unchanged.
- Studio audio export includes sampler, effects, sidechain and extended drum patterns.

The official binary is Windows x64 portable. Other source-build targets are not additional
official 1.0 binary packages; Android and iPad apps are not included. Hardware timing and
controller behavior still depend on the interface, driver and connected equipment.

The permanent download URL retains its historical `windows-preview` path for compatibility.
Earlier trial releases remain historical test builds, not separate stable versions.

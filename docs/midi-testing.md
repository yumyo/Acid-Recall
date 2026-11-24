# Testing the MIDI backends

How the three device backends are checked. Windows is WinMM (`crates/orb-ui/src/midi/winmm.rs`),
macOS is CoreMIDI (`coremidi.rs`), Linux is the ALSA sequencer through `midir` (`alsa.rs`). All
three deliver WinMM's packed short-message word, so everything above them (`Cc::decode`, the
router, MIDI Learn, the clock worker) is shared and tested once.

## What a backend needs

Not hardware: a MIDI *service*. CoreMIDI is always present on macOS. On Linux the service is the
ALSA sequencer, kernel module `snd-seq`, which a desktop has loaded and a server or container
usually does not. The loopback tests create virtual ports on that service and send messages
through the backend, the same path a controller takes.

| Backend | Unit tests | Loopback tests | Runs where |
|---|---|---|---|
| CoreMIDI | byte parser: packing, running status, interleaved real-time bytes, SysEx skipped | `input_and_output_round_trip` through the CoreMIDI server | every `cargo test` on macOS, including CI |
| ALSA (midir) | `packs_as_winmm_does`, `short_message_lengths` | `loopback_through_a_virtual_source` (CC in; Clock and Note On filtered out), `clock_out_to_a_virtual_destination` (Start, Clock, Song Position Pointer out as 1, 1, 3 bytes) | unit tests everywhere on Linux; loopbacks are `#[ignore]` and need `/dev/snd/seq` |

The ALSA loopbacks are ignored because a headless runner may have no sequencer. Run them on a
Linux machine that has one:

```sh
cargo test -p orb-ui --lib midi::alsa -- --include-ignored
```

## Tier 1: build on Linux, from CI

`ci.yml` runs on every pull request: clippy with `-D warnings` and `cargo test --workspace` on
`ubuntu-latest`, `windows-latest` and `macos-latest`. Opening a PR shows that midir links
against ALSA (the runner installs `libasound2-dev` for cpal; midir needs nothing more) and that
the `cfg(target_os = "linux")` split leaves Windows and macOS unchanged: `cargo tree` for those
targets contains no midir.

This tier does not show that a message crosses the ALSA sequencer.

## Tier 2: run the loopback tests in a Linux VM

A plain Ubuntu VM has the sequencer module. Docker does not work for this: a container cannot
load `snd-seq`, and Docker Desktop's VM kernel is not ours to change.

On a Mac with Homebrew (Lima gives an arm64 Ubuntu in a few minutes):

```sh
brew install lima
limactl start --name acid template://ubuntu-lts
limactl shell acid
```

Inside the VM:

```sh
sudo apt install -y build-essential pkg-config libasound2-dev curl
sudo modprobe snd-seq snd-seq-dummy
ls /dev/snd/seq                       # must exist before the tests run
curl https://sh.rustup.rs -sSf | sh -s -- -y && . "$HOME/.cargo/env"
git clone https://github.com/smldms/Acid-Recall.git && cd Acid-Recall
cargo test -p orb-ui --lib midi::alsa -- --include-ignored
```

Expected: `4 passed`. `aconnect -l` (package `alsa-utils`) lists the virtual ports while a test
runs, under the client name `ACID RECALL`.

## Tier 3: a controller on a Linux desktop

1. Plug the controller in. `aconnect -l` should list it.
2. Run the AppImage, or `cargo run --release -p orb-ui`.
3. MIDI button → Enable MIDI input → choose the port → right-click a knob → MIDI LEARN → move a
   control.

Expect one difference from the other platforms: a PipeWire desktop with `pipewire-alsa` exposes
sequencer ports twice, raw and through PipeWire. Either works; the names differ.

For MIDI Clock OUT, pick a destination under MASTER / OUT and check the receiving machine, or
`aseqdump -p <port>` (alsa-utils) for `Clock`, `Start` and `Song Position Pointer`.

## macOS

The CoreMIDI round trip runs with the ordinary test suite. Audio MIDI Setup (Window → Show MIDI
Studio) shows the virtual endpoints while it runs. A controller appears in the app's MIDI window
under its CoreMIDI name.

## What has been run (23 November 2026)

| Backend | Build | Loopback / round trip | Physical controller |
|---|---|---|---|
| CoreMIDI, macOS 27.2 arm64 | yes | pass | not yet |
| ALSA, Linux | CI only, pending | not yet | not yet |
| WinMM, Windows | CI, release ZIP | n/a | UC-33e (the author) |

The ALSA backend was developed and loopback-tested on macOS through CoreMIDI, with the same
midir code paths (`midir::os::unix` virtual ports exist on both). What remains untested is the
ALSA transport itself, which Tier 2 covers.

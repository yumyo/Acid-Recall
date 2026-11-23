# Testing the MIDI backend on macOS and Linux

How the `midir` backend (`crates/orb-ui/src/midi/portable.rs`) is checked on the two
platforms it serves. Windows keeps the WinMM backend and is not covered here.

## What the backend needs

Not hardware: a MIDI *service*. On macOS that is CoreMIDI, always present. On Linux it is the
ALSA sequencer, the kernel module `snd-seq`, which a desktop has loaded and a server or
container usually does not. The two loopback tests create virtual ports on that service and
send messages through the backend, so they exercise the same path a controller would.

Both loopback tests are `#[ignore]`, so a headless runner without `/dev/snd/seq` does not fail
them. They run on demand:

```sh
cargo test -p orb-ui --lib midi::portable -- --include-ignored
```

| Test | Direction | Asserts |
|---|---|---|
| `loopback_through_a_virtual_source` | virtual source → `Input` | CC 7 = 100 arrives as `0x006407b0`; Clock (`0xf8`) and Note On are kept out of the CC queue; `dropped()` is 0 |
| `clock_out_to_a_virtual_destination` | `Output` → virtual destination | Start, Clock and Song Position Pointer go out as 1, 1 and 3 bytes |

## Tier 1: build on Linux, from CI

`ci.yml` runs on every pull request: clippy with `-D warnings` and `cargo test --workspace` on
`ubuntu-latest`, `windows-latest` and `macos-latest`. Opening a PR is enough to show that
midir links against ALSA and that the `cfg(not(windows))` split leaves Windows unchanged.

The Linux runner installs `libasound2-dev` for cpal; midir needs nothing more.

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
cd /path/to/Acid-Recall               # Lima mounts the Mac home read-only; clone if writing
cargo test -p orb-ui --lib midi::portable -- --include-ignored
```

Expected: `4 passed` (two packing tests, two loopback tests).

`aconnect -l` (package `alsa-utils`) lists the virtual ports while a test runs, under the
client name `ACID RECALL`.

## Tier 3: a controller on a Linux desktop

1. Plug the controller in. `aconnect -l` should list it.
2. Run the AppImage, or `cargo run --release -p orb-ui`.
3. MIDI button → Enable MIDI input → choose the port → right-click a knob → MIDI LEARN → move a
   control.

Expect one difference from macOS: a PipeWire desktop with `pipewire-alsa` exposes sequencer
ports twice, raw and through PipeWire. Either works; the names differ.

For MIDI Clock OUT, pick a destination under MASTER / OUT and check the receiving machine or
`aseqdump -p <port>` (alsa-utils) for `Clock`, `Start` and `Song Position Pointer`.

## macOS

CoreMIDI is always present, so the loopback tests run as they are:

```sh
cargo test -p orb-ui --lib midi::portable -- --include-ignored
```

Audio MIDI Setup (Window → Show MIDI Studio) shows the virtual ports while a test runs. A real
controller appears in the app's MIDI window under its CoreMIDI name.

## What has been run (23 November 2026)

| Platform | Build | Loopback tests | Controller |
|---|---|---|---|
| macOS 26.7.3 arm64 | yes | 4 pass | virtual port only (UC-33e not attached) |
| Linux | CI only, pending | not yet | not yet |
| Windows | CI only, pending | n/a (WinMM) | n/a |

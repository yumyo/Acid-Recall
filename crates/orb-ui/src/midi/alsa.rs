//! The Linux MIDI backend: the ALSA sequencer through `midir`. Same contract as the WinMM and
//! CoreMIDI adapters: only short messages, delivered as WinMM's packed word (status in the low
//! byte, then the two data bytes), so `Cc::decode` and the clock `Sender` read every backend
//! alike. The callback never calls egui or the audio player: it forwards realtime bytes to the
//! clock and enqueues Control Changes for the UI thread to drain.
//!
//! The sequencer decodes the wire protocol in the kernel, so every event arrives as one whole
//! message with its status byte: running status never reaches this code. SysEx is ignored.
//!
//! Ports are addressed by their index in the current enumeration, as WinMM device ids are. The
//! UI matches ports by name and opens them by that index right after listing; a device plugged
//! in while the app runs appears after Refresh / reconnect.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

use super::Port;

/// The client name the sequencer shows (`aconnect -l`).
const CLIENT: &str = "ACID RECALL";

fn input() -> Result<MidiInput, String> {
    let mut input = MidiInput::new(CLIENT).map_err(|e| format!("Open MIDI system: {e}"))?;
    // Short messages only, as WinMM delivers them; SysEx is never queued.
    input.ignore(Ignore::Sysex);
    Ok(input)
}

/// WinMM's packed short message from the message bytes; a message longer than three bytes is
/// not a short message and is dropped (SysEx is ignored before it gets here).
fn pack(bytes: &[u8]) -> Option<u32> {
    let mut word = [0u8; 4];
    match bytes.len() {
        1..=3 => {
            word[..bytes.len()].copy_from_slice(bytes);
            Some(u32::from_le_bytes(word))
        }
        _ => None,
    }
}

/// The byte length of a short message from its status byte (the MIDI 1.0 specification).
fn message_len(status: u8) -> usize {
    match status {
        0xc0..=0xdf | 0xf1 | 0xf3 => 2,
        0x80..=0xbf | 0xe0..=0xef | 0xf2 => 3,
        _ => 1,
    }
}

pub fn ports() -> Result<Vec<Port>, String> {
    let input = input()?;
    Ok(input
        .ports()
        .iter()
        .enumerate()
        .map(|(id, port)| Port {
            id: id as u32,
            name: input
                .port_name(port)
                .unwrap_or_else(|_| format!("MIDI input {id}")),
        })
        .collect())
}

struct Shared {
    clock: Option<super::sync::Sender>,
    sender: SyncSender<u32>,
    dropped: Arc<AtomicUsize>,
}

pub struct Input {
    // Kept for its Drop, which closes the port and stops the callback.
    _connection: MidiInputConnection<Shared>,
    receiver: Receiver<u32>,
    dropped: Arc<AtomicUsize>,
}

impl Input {
    pub fn open(id: u32, clock: Option<super::sync::Sender>) -> Result<Self, String> {
        let input = input()?;
        let ports = input.ports();
        let port = ports
            .get(id as usize)
            .ok_or("Open MIDI input: the port is no longer present")?;
        let (sender, receiver) = sync_channel(1024);
        let dropped = Arc::new(AtomicUsize::new(0));
        let shared = Shared {
            clock,
            sender,
            dropped: dropped.clone(),
        };
        let connection = input
            .connect(
                port,
                "input",
                |_timestamp, bytes, shared| {
                    let Some(word) = pack(bytes) else {
                        return;
                    };
                    if let Some(clock) = &shared.clock {
                        clock.receive(word);
                    }
                    if super::Cc::decode(word).is_some() && shared.sender.try_send(word).is_err() {
                        shared.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                },
                shared,
            )
            .map_err(|e| {
                format!("Open MIDI input (close other applications using this port): {e}")
            })?;
        Ok(Self {
            _connection: connection,
            receiver,
            dropped,
        })
    }

    pub fn drain(&self) -> Vec<u32> {
        // Bound work even if a device sends continuously while the UI drains its queue.
        self.receiver.try_iter().take(1024).collect()
    }

    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }
}

fn output() -> Result<MidiOutput, String> {
    MidiOutput::new(CLIENT).map_err(|e| format!("Open MIDI system: {e}"))
}

pub fn output_ports() -> Result<Vec<Port>, String> {
    let output = output()?;
    Ok(output
        .ports()
        .iter()
        .enumerate()
        .map(|(id, port)| Port {
            id: id as u32,
            name: output
                .port_name(port)
                .unwrap_or_else(|_| format!("MIDI output {id}")),
        })
        .collect())
}

/// Opened, used and dropped on the clock worker only, as the WinMM one is.
pub struct Output {
    connection: MidiOutputConnection,
}

impl Output {
    pub fn open(id: u32) -> Result<Self, String> {
        let output = output()?;
        let ports = output.ports();
        let port = ports
            .get(id as usize)
            .ok_or("Open MIDI Clock output: the port is no longer present")?;
        let connection = output
            .connect(port, "clock")
            .map_err(|e| format!("Open MIDI Clock output: {e}"))?;
        Ok(Self { connection })
    }

    pub fn send(&mut self, word: u32) -> Result<(), String> {
        let bytes = word.to_le_bytes();
        let len = message_len(bytes[0]);
        self.connection
            .send(&bytes[..len])
            .map_err(|e| format!("Send MIDI Clock: {e}"))
    }
}

/// WinMM needs `timeBeginPeriod(1)` for a 1 ms sleep; Linux sleeps at that resolution as it
/// is, so there is nothing to request or release here.
pub struct Timer;
impl Timer {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::{message_len, pack};

    #[test]
    fn packs_as_winmm_does() {
        // CC 7 = 100 on channel 1: status 0xb0, controller 7, value 100.
        assert_eq!(pack(&[0xb0, 7, 100]), Some(0x0064_07b0));
        assert_eq!(
            super::super::Cc::decode(pack(&[0xb0, 7, 100]).unwrap()).map(|cc| cc.value),
            Some(100)
        );
        // A realtime byte is the word's low byte, as the clock Sender reads it.
        assert_eq!(pack(&[0xf8]), Some(0xf8));
        assert_eq!(pack(&[0xf2, 0x10, 0x02]).map(|w| w as u8), Some(0xf2));
        assert_eq!(pack(&[]), None);
        assert_eq!(pack(&[0xf0, 1, 2, 3, 0xf7]), None);
    }

    #[test]
    fn short_message_lengths() {
        assert_eq!(message_len(0xf8), 1); // Clock
        assert_eq!(message_len(0xfa), 1); // Start
        assert_eq!(message_len(0xfc), 1); // Stop
        assert_eq!(message_len(0xf2), 3); // Song Position Pointer
        assert_eq!(message_len(0xb0), 3); // Control Change
        assert_eq!(message_len(0xc0), 2); // Program Change
    }

    /// A Control Change through the system's MIDI service and this backend, as a controller
    /// would send it. Needs the ALSA sequencer (`/dev/snd/seq`, module `snd-seq`), which a
    /// headless runner may not have: `cargo test -p orb-ui --lib midi::alsa -- --include-ignored`.
    #[test]
    #[ignore]
    fn loopback_through_a_virtual_source() {
        use midir::os::unix::VirtualOutput;
        let name = format!("ACID RECALL loopback {}", std::process::id());
        let mut source = midir::MidiOutput::new("ACID RECALL test")
            .unwrap()
            .create_virtual(&name)
            .unwrap();
        // The sequencer may publish a new port a moment after it is created.
        let port = (0..50)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                super::ports().unwrap().into_iter().find(|p| p.name == name)
            })
            .expect("the virtual source is listed");
        let input = super::Input::open(port.id, None).unwrap();
        source.send(&[0xb0, 7, 100]).unwrap(); // CC 7 = 100, channel 1
        source.send(&[0xf8]).unwrap(); // Clock: realtime, not a CC, never queued
        source.send(&[0x90, 60, 64]).unwrap(); // Note On: not a CC, never queued
        let words = (0..50)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let words = input.drain();
                (!words.is_empty()).then_some(words)
            })
            .expect("the CC arrives");
        assert_eq!(words, vec![0x0064_07b0]);
        assert_eq!(input.dropped(), 0);
    }

    /// MIDI Clock out through this backend's `Output`, received by a virtual destination:
    /// each packed word goes out with the length its status byte gives. Same requirement and
    /// invocation as the test above.
    #[test]
    #[ignore]
    fn clock_out_to_a_virtual_destination() {
        use std::sync::mpsc::channel;

        use midir::os::unix::VirtualInput;
        let name = format!("ACID RECALL clock {}", std::process::id());
        let (sender, receiver) = channel();
        let _sink = midir::MidiInput::new("ACID RECALL test")
            .unwrap()
            .create_virtual(
                &name,
                move |_, bytes, _| sender.send(bytes.to_vec()).unwrap(),
                (),
            )
            .unwrap();
        let port = (0..50)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                super::output_ports()
                    .unwrap()
                    .into_iter()
                    .find(|p| p.name == name)
            })
            .expect("the virtual destination is listed");
        let mut output = super::Output::open(port.id).unwrap();
        output.send(0xfa).unwrap(); // Start
        output.send(0xf8).unwrap(); // Clock
        output.send(0x0002_10f2).unwrap(); // Song Position Pointer, two data bytes
        let timeout = std::time::Duration::from_secs(5);
        assert_eq!(receiver.recv_timeout(timeout).unwrap(), vec![0xfa]);
        assert_eq!(receiver.recv_timeout(timeout).unwrap(), vec![0xf8]);
        assert_eq!(
            receiver.recv_timeout(timeout).unwrap(),
            vec![0xf2, 0x10, 0x02]
        );
    }
}

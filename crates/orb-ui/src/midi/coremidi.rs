//! Small CoreMIDI adapter for macOS, with the interface of the WinMM one (winmm.rs).
//!
//! API: Apple's CoreMIDI (MIDIServices.h): MIDIClientCreate, MIDIInputPortCreate,
//! MIDIOutputPortCreate, MIDIPortConnectSource, MIDIPacketListAdd and MIDISend. The
//! MIDIPacketList API is used because it needs no Objective-C block. Incoming bytes are
//! reassembled into WinMM's packed short messages (status | data1 << 8 | data2 << 16), the form
//! the router (`Cc::decode`) and the clock worker (`sync::Sender`) read; SysEx is skipped.
//!
//! A port's id is its endpoint's kMIDIPropertyUniqueID, which stays the same while devices are
//! plugged in or out, where an index would not. The read callback runs on CoreMIDI's own
//! thread, never calls egui or the audio player, and only queues, like WinMM's. Its context
//! stays allocated until MIDIPortDispose returns.

use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Mutex, OnceLock};

use super::Port;

type OsStatus = i32;
type ObjectRef = u32;
type CfStringRef = *const c_void;
type ReadProc = unsafe extern "C" fn(list: *const c_void, port: *mut c_void, source: *mut c_void);
type NotifyProc = unsafe extern "C" fn(message: *const c_void, context: *mut c_void);

const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8
const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        s: *const c_char,
        encoding: u32,
    ) -> CfStringRef;
    fn CFStringGetLength(s: CfStringRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(s: CfStringRef, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFRelease(object: *const c_void);
}

#[allow(non_upper_case_globals)]
#[link(name = "CoreMIDI", kind = "framework")]
unsafe extern "C" {
    static kMIDIPropertyDisplayName: CfStringRef;
    static kMIDIPropertyName: CfStringRef;
    static kMIDIPropertyUniqueID: CfStringRef;
    fn MIDIClientCreate(
        name: CfStringRef,
        notify: Option<NotifyProc>,
        context: *mut c_void,
        client: *mut ObjectRef,
    ) -> OsStatus;
    fn MIDIInputPortCreate(
        client: ObjectRef,
        name: CfStringRef,
        read: ReadProc,
        context: *mut c_void,
        port: *mut ObjectRef,
    ) -> OsStatus;
    fn MIDIOutputPortCreate(client: ObjectRef, name: CfStringRef, port: *mut ObjectRef)
    -> OsStatus;
    fn MIDIPortConnectSource(port: ObjectRef, source: ObjectRef, context: *mut c_void) -> OsStatus;
    fn MIDIPortDispose(port: ObjectRef) -> OsStatus;
    fn MIDIGetNumberOfSources() -> usize;
    fn MIDIGetSource(index: usize) -> ObjectRef;
    fn MIDIGetNumberOfDestinations() -> usize;
    fn MIDIGetDestination(index: usize) -> ObjectRef;
    fn MIDIObjectGetStringProperty(
        object: ObjectRef,
        property: CfStringRef,
        value: *mut CfStringRef,
    ) -> OsStatus;
    fn MIDIObjectGetIntegerProperty(
        object: ObjectRef,
        property: CfStringRef,
        value: *mut i32,
    ) -> OsStatus;
    fn MIDIPacketListInit(list: *mut c_void) -> *mut c_void;
    fn MIDIPacketListAdd(
        list: *mut c_void,
        list_size: usize,
        current: *mut c_void,
        time: u64,
        length: usize,
        data: *const u8,
    ) -> *mut c_void;
    fn MIDISend(port: ObjectRef, destination: ObjectRef, list: *const c_void) -> OsStatus;
}

unsafe extern "C" {
    // libSystem (pthread/qos.h).
    fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
}

fn check(code: OsStatus, action: &str) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("{action}: CoreMIDI error {code}"))
    }
}

/// An owned CFString, released on drop.
struct CfString(CfStringRef);

impl CfString {
    fn new(s: &CStr) -> Self {
        // SAFETY: a NUL-terminated UTF-8 string; the default allocator (null).
        Self(unsafe { CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) })
    }
}

impl Drop for CfString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: created or copied for this owner (Create/Copy rule), released once.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// The process's one CoreMIDI client, never disposed. It is created on the first listing,
/// which runs on the UI thread: CoreMIDI tracks devices on the run loop current at creation.
fn client() -> Result<ObjectRef, String> {
    static CLIENT: OnceLock<Result<ObjectRef, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            let name = CfString::new(c"ACID RECALL");
            let mut client = 0;
            // SAFETY: a valid name and a writable client reference; no notification callback.
            check(
                unsafe { MIDIClientCreate(name.0, None, std::ptr::null_mut(), &mut client) },
                "Start CoreMIDI",
            )?;
            Ok(client)
        })
        .clone()
}

#[derive(Clone, Copy)]
enum Direction {
    Source,
    Destination,
}

fn endpoints(direction: Direction) -> Vec<ObjectRef> {
    // SAFETY: index queries without ownership; an endpoint removed meanwhile reads as 0.
    unsafe {
        match direction {
            Direction::Source => (0..MIDIGetNumberOfSources())
                .map(|i| MIDIGetSource(i))
                .collect(),
            Direction::Destination => (0..MIDIGetNumberOfDestinations())
                .map(|i| MIDIGetDestination(i))
                .collect::<Vec<_>>(),
        }
    }
    .into_iter()
    .filter(|&endpoint| endpoint != 0)
    .collect()
}

fn unique_id(endpoint: ObjectRef) -> Option<u32> {
    let mut id = 0;
    // SAFETY: a CoreMIDI property constant and a writable integer.
    let status = unsafe { MIDIObjectGetIntegerProperty(endpoint, kMIDIPropertyUniqueID, &mut id) };
    (status == 0).then_some(id as u32)
}

fn string_property(endpoint: ObjectRef, property: CfStringRef) -> Option<String> {
    let mut value = std::ptr::null();
    // SAFETY: a writable reference; on success the caller owns the copied string.
    if unsafe { MIDIObjectGetStringProperty(endpoint, property, &mut value) } != 0 {
        return None;
    }
    let value = CfString(value);
    if value.0.is_null() {
        return None;
    }
    // SAFETY: a valid CFString; the buffer is as large as the encoding can need, plus NUL.
    unsafe {
        let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(value.0), UTF8) + 1;
        let mut buffer = vec![0u8; usize::try_from(size).ok()?];
        if CFStringGetCString(value.0, buffer.as_mut_ptr().cast(), size, UTF8) == 0 {
            return None;
        }
        let text = CStr::from_bytes_until_nul(&buffer).ok()?;
        Some(text.to_string_lossy().into_owned())
    }
}

fn list(direction: Direction) -> Result<Vec<Port>, String> {
    client()?;
    Ok(endpoints(direction)
        .into_iter()
        .filter_map(|endpoint| {
            // SAFETY: CoreMIDI's immutable property-name constants.
            let (display, plain) = unsafe { (kMIDIPropertyDisplayName, kMIDIPropertyName) };
            let name =
                string_property(endpoint, display).or_else(|| string_property(endpoint, plain))?;
            Some(Port {
                id: unique_id(endpoint)?,
                name,
            })
        })
        .collect())
}

fn find(direction: Direction, id: u32) -> Result<ObjectRef, String> {
    endpoints(direction)
        .into_iter()
        .find(|&endpoint| unique_id(endpoint) == Some(id))
        .ok_or_else(|| "MIDI port unavailable. Connect it, then Refresh / reconnect.".into())
}

pub fn ports() -> Result<Vec<Port>, String> {
    list(Direction::Source)
}

pub fn output_ports() -> Result<Vec<Port>, String> {
    list(Direction::Destination)
}

/// The number of data bytes after a status byte; 0 for real-time and undefined statuses.
fn data_len(status: u8) -> usize {
    match status {
        0x80..=0xbf | 0xe0..=0xef | 0xf2 => 2,
        0xc0..=0xdf | 0xf1 | 0xf3 => 1,
        _ => 0,
    }
}

/// Reassembles a byte stream into packed short messages. Running status is honored for channel
/// messages; real-time bytes may arrive anywhere, even inside another message; SysEx and
/// undefined statuses are skipped up to the next status byte. The state spans packets.
#[derive(Default)]
struct Parser {
    status: u8,
    data: [u8; 2],
    len: usize,
}

impl Parser {
    fn push(&mut self, byte: u8, emit: &mut impl FnMut(u32)) {
        if byte >= 0xf8 {
            emit(u32::from(byte));
            return;
        }
        if byte & 0x80 != 0 {
            self.len = 0;
            self.status = if data_len(byte) == 0 { 0 } else { byte };
            if byte == 0xf6 {
                emit(0xf6);
            }
            return;
        }
        if self.status == 0 {
            return;
        }
        self.data[self.len] = byte;
        self.len += 1;
        if self.len == data_len(self.status) {
            emit(
                u32::from(self.status)
                    | u32::from(self.data[0]) << 8
                    | if self.len == 2 {
                        u32::from(self.data[1]) << 16
                    } else {
                        0
                    },
            );
            self.len = 0;
            if self.status >= 0xf0 {
                self.status = 0; // System common messages set no running status.
            }
        }
    }
}

/// Calls `f` with each packet's bytes. MIDIPacketList and MIDIPacket are declared under
/// `#pragma pack(4)`: the list is a UInt32 count, then the packets; a packet is a UInt64 time
/// stamp, a UInt16 length and its data. On arm64 MIDIPacketNext rounds the next packet's address
/// up to 4 bytes, on x86_64 it does not (MIDIServices.h).
///
/// # Safety
/// `list` points to a valid MIDIPacketList for the duration of the call.
unsafe fn for_each_packet(list: *const u8, mut f: impl FnMut(&[u8])) {
    // SAFETY: the caller's valid list, read field by field with its packed offsets.
    unsafe {
        let count = list.cast::<u32>().read_unaligned();
        let mut packet = list.add(4);
        for _ in 0..count {
            let len = usize::from(packet.add(8).cast::<u16>().read_unaligned());
            let data = packet.add(10);
            f(std::slice::from_raw_parts(data, len));
            let end = data.add(len);
            packet = if cfg!(target_arch = "aarch64") {
                end.add(end.addr().wrapping_neg() & 3)
            } else {
                end
            };
        }
    }
}

struct Callback {
    clock: Option<super::sync::Sender>,
    sender: SyncSender<u32>,
    dropped: AtomicUsize,
    parser: Mutex<Parser>,
}

unsafe extern "C" fn receive(list: *const c_void, context: *mut c_void, _source: *mut c_void) {
    if list.is_null() || context.is_null() {
        return;
    }
    // SAFETY: Input owns the context until MIDIPortDispose returns. Callback only shares it.
    let callback = unsafe { &*(context as *const Callback) };
    // Only this callback locks the parser, and CoreMIDI calls it on one thread at a time.
    let Ok(mut parser) = callback.parser.lock() else {
        return;
    };
    let mut emit = |word: u32| {
        if let Some(clock) = &callback.clock {
            clock.receive(word);
        }
        if super::Cc::decode(word).is_some() && callback.sender.try_send(word).is_err() {
            callback.dropped.fetch_add(1, Ordering::Relaxed);
        }
    };
    // SAFETY: CoreMIDI passes a valid packet list for the duration of the call.
    unsafe {
        for_each_packet(list.cast(), |bytes| {
            for &byte in bytes {
                parser.push(byte, &mut emit);
            }
        });
    }
}

pub struct Input {
    port: ObjectRef,
    context: Option<Box<Callback>>,
    receiver: Receiver<u32>,
}

impl Input {
    pub fn open(id: u32, clock: Option<super::sync::Sender>) -> Result<Self, String> {
        let client = client()?;
        let source = find(Direction::Source, id)?;
        let (sender, receiver) = sync_channel(1024);
        let context = Box::new(Callback {
            clock,
            sender,
            dropped: AtomicUsize::new(0),
            parser: Mutex::new(Parser::default()),
        });
        let name = CfString::new(c"ACID RECALL input");
        let mut port = 0;
        // SAFETY: a writable port reference; the C-ABI callback and the boxed context outlive
        // the port. The context is not moved by moving its Box.
        check(
            unsafe {
                MIDIInputPortCreate(
                    client,
                    name.0,
                    receive,
                    (&*context as *const Callback).cast_mut().cast(),
                    &mut port,
                )
            },
            "Open MIDI input",
        )?;
        let input = Self {
            port,
            context: Some(context),
            receiver,
        };
        // SAFETY: the port created above and a source endpoint. On error Input::drop disposes.
        check(
            unsafe { MIDIPortConnectSource(port, source, std::ptr::null_mut()) },
            "Connect MIDI input",
        )?;
        Ok(input)
    }

    pub fn drain(&self) -> Vec<u32> {
        // Bound work even if a device sends continuously while the UI drains its queue.
        self.receiver.try_iter().take(1024).collect()
    }

    pub fn dropped(&self) -> usize {
        self.context
            .as_ref()
            .map_or(0, |c| c.dropped.load(Ordering::Relaxed))
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        // SAFETY: this is the only owner of the port; disposing it also disconnects its source.
        if unsafe { MIDIPortDispose(self.port) } != 0
            && let Some(context) = self.context.take()
        {
            // A port that refuses disposal could still call receive: never free its context.
            let _ = Box::leak(context);
        }
    }
}

/// Opened, used and dropped on the clock worker only.
pub struct Output {
    port: ObjectRef,
    destination: ObjectRef,
}

impl Output {
    pub fn open(id: u32) -> Result<Self, String> {
        let client = client()?;
        let destination = find(Direction::Destination, id)?;
        let name = CfString::new(c"ACID RECALL clock");
        let mut port = 0;
        // SAFETY: a writable port reference.
        check(
            unsafe { MIDIOutputPortCreate(client, name.0, &mut port) },
            "Open MIDI Clock output",
        )?;
        Ok(Self { port, destination })
    }

    pub fn send(&mut self, word: u32) -> Result<(), String> {
        let bytes = word.to_le_bytes();
        let len = 1 + data_len(bytes[0]);
        // A 4-byte aligned list: a 4-byte count and one 10-byte packet header before the data.
        let mut buffer = [0u32; 16];
        let list = buffer.as_mut_ptr().cast::<c_void>();
        // SAFETY: the list buffer is writable, aligned and 64 bytes long; time 0 sends now.
        unsafe {
            let packet = MIDIPacketListInit(list);
            if MIDIPacketListAdd(list, 64, packet, 0, len, bytes.as_ptr()).is_null() {
                return Err("Send MIDI Clock: message does not fit".into());
            }
            check(
                MIDISend(self.port, self.destination, list),
                "Send MIDI Clock",
            )
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        // SAFETY: only owner; MIDISend has no buffers in flight that refer to this memory.
        unsafe {
            MIDIPortDispose(self.port);
        }
    }
}

/// The clock worker's 1 ms loop needs no process-wide timer change on macOS, unlike WinMM's
/// timeBeginPeriod: sleeps already wake within about a millisecond. The worker thread asks for
/// the user-interactive QoS class instead, so a loaded system wakes it as promptly as the UI.
pub struct Timer;

impl Timer {
    pub fn new() -> Self {
        // SAFETY: changes only the calling thread's QoS class; failure keeps the default.
        unsafe {
            pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
        }
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::Parser;

    fn parse(bytes: &[u8]) -> Vec<u32> {
        let mut parser = Parser::default();
        let mut words = Vec::new();
        for &byte in bytes {
            parser.push(byte, &mut |word| words.push(word));
        }
        words
    }

    #[test]
    fn control_change_packs_like_winmm() {
        assert_eq!(parse(&[0xb2, 7, 100]), vec![0x0064_07b2]);
    }

    #[test]
    fn running_status_repeats_the_channel_message() {
        assert_eq!(parse(&[0xb0, 1, 2, 3, 4]), vec![0x0002_01b0, 0x0004_03b0]);
    }

    #[test]
    fn real_time_bytes_interleave_without_breaking_a_message() {
        assert_eq!(
            parse(&[0xb0, 0xf8, 1, 0xfa, 2]),
            vec![0xf8, 0xfa, 0x0002_01b0]
        );
    }

    #[test]
    fn song_position_and_program_change() {
        assert_eq!(parse(&[0xf2, 16, 0]), vec![0xf2 | 16 << 8]);
        assert_eq!(parse(&[0xc1, 5]), vec![0x05c1]);
    }

    #[test]
    fn sysex_is_skipped_and_system_common_sets_no_running_status() {
        assert_eq!(parse(&[0xf0, 1, 2, 3, 0xf7, 0xb0, 1, 2]), vec![0x0002_01b0]);
        assert_eq!(parse(&[0xf2, 1, 2, 3, 4]), vec![0x0002_01f2]);
    }

    /// Through the real CoreMIDI server, with this process's own virtual endpoints: a source
    /// feeds Input, and Output feeds a destination. No device is needed.
    mod virtual_ports {
        use std::ffi::c_void;
        use std::sync::Mutex;
        use std::time::{Duration, Instant};

        use super::super::*;

        #[link(name = "CoreMIDI", kind = "framework")]
        unsafe extern "C" {
            fn MIDISourceCreate(
                client: ObjectRef,
                name: CfStringRef,
                out: *mut ObjectRef,
            ) -> OsStatus;
            fn MIDIDestinationCreate(
                client: ObjectRef,
                name: CfStringRef,
                read: ReadProc,
                context: *mut c_void,
                out: *mut ObjectRef,
            ) -> OsStatus;
            fn MIDIReceived(source: ObjectRef, list: *const c_void) -> OsStatus;
            fn MIDIEndpointDispose(endpoint: ObjectRef) -> OsStatus;
        }

        static RECEIVED: Mutex<Vec<u8>> = Mutex::new(Vec::new());

        unsafe extern "C" fn collect(list: *const c_void, _: *mut c_void, _: *mut c_void) {
            // SAFETY: CoreMIDI's valid list for the duration of the call.
            unsafe {
                for_each_packet(list.cast(), |bytes| {
                    RECEIVED.lock().unwrap().extend_from_slice(bytes)
                });
            }
        }

        fn wait<T>(mut f: impl FnMut() -> Option<T>) -> Option<T> {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(3) {
                if let Some(value) = f() {
                    return Some(value);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            None
        }

        #[test]
        fn input_and_output_round_trip() {
            let Ok(client) = client() else {
                eprintln!("skipped: the CoreMIDI server is unavailable");
                return;
            };
            let name = format!("ACID RECALL test {}", std::process::id());
            let cname = std::ffi::CString::new(name.clone()).unwrap();
            let cf = CfString::new(&cname);
            let (mut source, mut destination) = (0, 0);
            // SAFETY: valid client, name and writable endpoint references.
            unsafe {
                check(MIDISourceCreate(client, cf.0, &mut source), "source").unwrap();
                check(
                    MIDIDestinationCreate(
                        client,
                        cf.0,
                        collect,
                        std::ptr::null_mut(),
                        &mut destination,
                    ),
                    "destination",
                )
                .unwrap();
            }

            let find = |list: fn() -> Result<Vec<Port>, String>| {
                wait(|| {
                    list()
                        .unwrap()
                        .into_iter()
                        .find(|p| p.name == name)
                        .map(|p| p.id)
                })
            };
            let input = Input::open(find(ports).expect("virtual source listed"), None).unwrap();
            let bytes = [0xb0, 7, 100, 0xf8, 1, 2, 0xc0, 3];
            let mut buffer = [0u32; 16];
            let list = buffer.as_mut_ptr().cast::<c_void>();
            // SAFETY: a writable, aligned 64-byte list; the source belongs to this client.
            unsafe {
                let packet = MIDIPacketListInit(list);
                MIDIPacketListAdd(list, 64, packet, 0, bytes.len(), bytes.as_ptr());
                check(MIDIReceived(source, list), "received").unwrap();
            }
            let mut words = Vec::new();
            wait(|| {
                words.extend(input.drain());
                (words.len() >= 2).then_some(())
            });
            assert_eq!(
                words,
                vec![0x0064_07b0, 0x0002_01b0],
                "only control changes queue"
            );

            let mut output = Output::open(find(output_ports).expect("destination listed")).unwrap();
            output.send(0xf8).unwrap();
            output.send(0xf2 | 16 << 8).unwrap();
            let received = wait(|| {
                let received = RECEIVED.lock().unwrap().clone();
                (received.len() >= 4).then_some(received)
            });
            assert_eq!(received, Some(vec![0xf8, 0xf2, 16, 0]));

            drop((input, output));
            // SAFETY: endpoints created above, disposed once.
            unsafe {
                MIDIEndpointDispose(source);
                MIDIEndpointDispose(destination);
            }
        }
    }
}

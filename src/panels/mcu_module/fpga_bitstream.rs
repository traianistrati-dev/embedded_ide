//! The bitstream a pico2-ice project loads into its FPGA, checked before the
//! firmware is built around it.
//!
//! `include_bytes!` takes any file. A wrong one - icestorm's `.asc` text, a
//! UF2, an HX8K build, a copy cut short - compiles cleanly, flashes cleanly,
//! and only shows on the bench as a CDONE that never rises. So the file is read
//! here the way the FPGA will read it, command by command and CRC included,
//! following icestorm's `icepack` (`read_bits`) and Lattice TN1248 appendix B.
//!
//! A [`BitError`] stops Build and Flash. What merely looks unusual - a size
//! icepack would not write, the image the FPGA's own flash already holds - is
//! reported on [`BitInfo`] and never blocks.

use super::project_gen::{self, ProjectFiles};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Where a project keeps the bitstream its firmware loads.
pub const BITSTREAM_PATH: &str = "fpga/top.bin";

/// The one FPGA the loader targets: the pico2-ice's.
pub const DEVICE: &str = "iCE40UP5K";

/// What icepack writes for a UP5K: an empty comment, then CRAM and BRAM.
pub const UP5K_IMAGE_LEN: usize = 104_090;

/// FNV-1a-64 of tinyVision's `rgb_blink`, from the sync word to the wake-up
/// command - the image the pico2-ice's FPGA flash ships with. Only the hash is
/// kept: it is enough to recognise the file, and the IDE does not ship theirs.
const FACTORY_RGB_BLINK: u64 = 0x5d30_65cb_848f_e8ee;

/// icepack's part table, keyed on the CRAM size it adds up over every bank:
/// the widest bank, and the highest `offset + height`. The HX4K and LP4K are
/// the 8K die, which is why they share a row.
const PARTS: [((u32, u32), &str); 6] = [
    ((182, 80), "iCE40LP384"),
    ((332, 144), "iCE40HX1K / LP1K"),
    ((872, 272), "iCE40HX8K / HX4K / LP8K / LP4K"),
    ((692, 336), DEVICE),
    ((692, 176), "iCE5LP4K (iCE40 Ultra)"),
    ((656, 176), "iCE40LM4K"),
];

/// The sync word every iCE40 image starts its commands with.
const PREAMBLE: [u8; 4] = [0x7E, 0xAA, 0x99, 0x7E];

/// How far into the file the sync word may sit: past a vendor comment, but
/// not so far that a random binary finds one by chance.
const PREAMBLE_WINDOW: usize = 4096;

/// The largest data block any iCE40 bank holds: the widest bank (the 8K's
/// 872) by the tallest (the UP5K's 336). BRAM banks are far smaller.
const MAX_BANK_BYTES: usize = 872 * 336 / 8;

/// A bitstream the FPGA can load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitInfo {
    /// File size in bytes.
    pub len: usize,
    /// The image checks its own CRC before waking up, and the check passed.
    /// icepack always writes one; without it a damaged copy loads silently.
    pub crc_checked: bool,
    /// Byte for byte the `rgb_blink` the pico2-ice's FPGA flash holds.
    pub factory_image: bool,
}

impl BitInfo {
    /// What should give the user pause, most important first. Never blocks.
    pub fn warnings(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.factory_image {
            out.push(
                "This is tinyVision's rgb_blink, the same image the pico2-ice's own FPGA \
                 flash holds. The FPGA can boot that copy by itself, so a blinking LED \
                 does not prove this file was loaded.",
            );
        }
        if !self.crc_checked {
            out.push(
                "The image carries no CRC check, so a damaged copy would load without \
                 complaint.",
            );
        }
        out
    }

    /// What is merely unusual.
    pub fn notes(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.len != UP5K_IMAGE_LEN {
            out.push(format!(
                "icepack writes {} B for a UP5K; this file is {} B. A longer comment \
                 header or an image without BRAM data explains that.",
                thousands(UP5K_IMAGE_LEN),
                thousands(self.len)
            ));
        }
        out
    }
}

/// Why the FPGA would not take a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BitError {
    Empty,
    /// Every byte is the same `0x00` or `0xFF`, like a zeroed or erased flash.
    Blank(u8),
    /// A UF2 container: firmware for the RP2350's USB bootloader.
    Uf2,
    /// icestorm's `.asc` text, before `icepack` turned it into a bitstream.
    AscText,
    /// No sync word in the first [`PREAMBLE_WINDOW`] bytes.
    NoPreamble,
    /// The file ends at `len` bytes, before the image does.
    Truncated {
        len: usize,
    },
    /// An `icemulti` image, whose header only makes sense in the FPGA's flash.
    MultiBoot,
    /// The CRC check at byte `at` failed.
    Crc {
        at: usize,
    },
    /// A command icepack does not know, or a data block that is malformed.
    Corrupt {
        at: usize,
        what: String,
    },
    /// The image configures no logic at all.
    NoCram,
    /// Built for another iCE40 part.
    WrongDevice(&'static str),
    /// A CRAM size that matches no iCE40.
    UnknownGeometry {
        width: u32,
        height: u32,
    },
    /// The file exists but could not be read.
    Unreadable(String),
}

impl fmt::Display for BitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BitError::Empty => write!(f, "the file is empty"),
            BitError::Blank(b) => write!(
                f,
                "every byte is 0x{b:02X} - there is no image in it, like a zeroed or \
                 erased flash"
            ),
            BitError::Uf2 => write!(
                f,
                "this is a UF2 file for the RP2350's USB bootloader, not an FPGA \
                 bitstream - use the .bin that icepack writes"
            ),
            BitError::AscText => write!(
                f,
                "this is icestorm's .asc text - run `icepack top.asc top.bin` and use \
                 the .bin"
            ),
            BitError::NoPreamble => write!(
                f,
                "there is no iCE40 sync word (7E AA 99 7E) in the first 4 KiB, so this \
                 is not an iCE40 bitstream"
            ),
            BitError::Truncated { len } => write!(
                f,
                "the file stops at byte {} in the middle of the image - a copy or a \
                 download cut short",
                thousands(*len)
            ),
            BitError::MultiBoot => write!(
                f,
                "this is a multi-boot image (icemulti), which only works from the FPGA's \
                 own flash - use the single image icepack writes"
            ),
            BitError::Crc { at } => write!(
                f,
                "the CRC check at byte {} fails, so the file is damaged",
                thousands(*at)
            ),
            BitError::Corrupt { at, what } => {
                write!(f, "{what} at byte {} - the file is damaged", thousands(*at))
            }
            BitError::NoCram => write!(f, "the image configures no logic"),
            BitError::WrongDevice(part) => write!(
                f,
                "it was built for an {part}, but the pico2-ice carries an {DEVICE} - \
                 rebuild with `nextpnr-ice40 --up5k --package sg48`"
            ),
            BitError::UnknownGeometry { width, height } => write!(
                f,
                "its CRAM is {width} x {height}, which matches no iCE40 part"
            ),
            BitError::Unreadable(e) => write!(f, "it could not be read: {e}"),
        }
    }
}

/// Check that `data` is an image the pico2-ice's FPGA will take.
pub fn inspect(data: &[u8]) -> Result<BitInfo, BitError> {
    inspect_with(data, FACTORY_RGB_BLINK)
}

/// [`inspect`], against a given factory-image hash so a test can inject one.
fn inspect_with(data: &[u8], factory: u64) -> Result<BitInfo, BitError> {
    let Some(&first) = data.first() else {
        return Err(BitError::Empty);
    };
    if (first == 0x00 || first == 0xFF) && data.iter().all(|&b| b == first) {
        return Err(BitError::Blank(first));
    }
    if data.starts_with(b"UF2\n") {
        return Err(BitError::Uf2);
    }
    let text = data.trim_ascii_start();
    if text.starts_with(b".comment") || text.starts_with(b".device") {
        return Err(BitError::AscText);
    }
    let window = &data[..data.len().min(PREAMBLE_WINDOW)];
    let Some(start) = window.windows(4).position(|w| w == PREAMBLE) else {
        return Err(BitError::NoPreamble);
    };

    // icepack runs the CRC over every byte from the start of the file. The
    // image resets it before anything it checks, so the comment never counts.
    let mut i = start + PREAMBLE.len();
    let mut crc = crc16(0, &data[..i]);
    let (mut width, mut height, mut offset) = (0u32, 0u32, 0u32);
    let (mut cram_w, mut cram_h) = (0u32, 0u32);
    let mut crc_checked = false;
    // icemulti's header comes before any CRC reset (`01 05`). A boot address
    // or a reboot after it is a damaged command, not a multi-boot image: one
    // flipped bit turns `01 06` into `41 06`.
    let mut crc_reset = false;
    let truncated = BitError::Truncated { len: data.len() };
    loop {
        let at = i;
        let Some(&cmd) = data.get(i) else {
            return Err(truncated);
        };
        // The low nibble is the payload length, big endian.
        let n = usize::from(cmd & 0x0F);
        let Some(payload) = data.get(i + 1..i + 1 + n) else {
            return Err(truncated);
        };
        let value = payload
            .iter()
            .fold(0u32, |v, &b| v.wrapping_shl(8) | u32::from(b));
        crc = crc16(crc, &data[i..i + 1 + n]);
        i += 1 + n;
        match cmd >> 4 {
            0x0 => match value {
                // CRAM (1) or BRAM (3) data: one bank's bits, then 00 00.
                1 | 3 => {
                    if width == 0 || height == 0 {
                        return Err(BitError::Corrupt {
                            at,
                            what: "a data block before its bank size".to_owned(),
                        });
                    }
                    let len = (u64::from(width) * u64::from(height) / 8) as usize;
                    // A flipped bit in a size field, not a file cut short.
                    if len > MAX_BANK_BYTES {
                        return Err(BitError::Corrupt {
                            at,
                            what: format!("a {width} x {height} bank, larger than any iCE40's"),
                        });
                    }
                    let Some(block) = data.get(i..i + len + 2) else {
                        return Err(truncated);
                    };
                    crc = crc16(crc, block);
                    if block[len..] != [0, 0] {
                        return Err(BitError::Corrupt {
                            at: i + len,
                            what: "a data block not closed by 00 00".to_owned(),
                        });
                    }
                    i += len + 2;
                    if value == 1 {
                        cram_w = cram_w.max(width);
                        cram_h = cram_h.max(offset.saturating_add(height));
                    }
                }
                0x05 => {
                    crc = 0xFFFF;
                    crc_reset = true;
                }
                0x06 => break,
                // Reboot: the tail of every icemulti header.
                0x08 if !crc_reset => return Err(BitError::MultiBoot),
                _ => {
                    return Err(BitError::Corrupt {
                        at,
                        what: format!("an unknown command 0x{cmd:02X} 0x{value:02X}"),
                    });
                }
            },
            0x1 => {} // bank number
            0x2 => {
                if crc != 0 {
                    return Err(BitError::Crc { at });
                }
                crc_checked = true;
            }
            // Boot address (`44 03 aa aa aa`): icemulti's pointer to an image.
            0x4 if cmd == 0x44 && !crc_reset => return Err(BitError::MultiBoot),
            0x5 | 0x9 => {} // oscillator range; warm boot and no-sleep flags
            0x6 => width = value.wrapping_add(1),
            0x7 => height = value,
            0x8 => offset = value,
            _ => {
                return Err(BitError::Corrupt {
                    at,
                    what: format!("an unknown command 0x{cmd:02X}"),
                });
            }
        }
    }

    if cram_w == 0 {
        return Err(BitError::NoCram);
    }
    match PARTS.iter().find(|(size, _)| *size == (cram_w, cram_h)) {
        Some((_, part)) if *part == DEVICE => {}
        Some((_, part)) => return Err(BitError::WrongDevice(part)),
        None => {
            return Err(BitError::UnknownGeometry {
                width: cram_w,
                height: cram_h,
            });
        }
    }
    Ok(BitInfo {
        len: data.len(),
        crc_checked,
        factory_image: fnv1a64(&data[start..i]) == factory,
    })
}

/// CRC-16/CCITT as the iCE40 runs it: polynomial 0x1021, MSB first.
fn crc16(mut crc: u16, bytes: &[u8]) -> u16 {
    for &b in bytes {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// `104090` as `104,090`.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The bitstream a build embeds, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// The project's own file, or `None` when the build uses the IDE's default.
    pub file: Option<PathBuf>,
    pub result: Result<BitInfo, BitError>,
}

/// Check the bitstream a build of the project in `project_dir` embeds.
///
/// That is its own `fpga/top.bin`, or the IDE's default when it has none or
/// has no folder yet - the same choice `project_gen::sync_included_blobs`
/// makes when it writes the build copy.
pub fn check_project(project_dir: Option<&Path>) -> Verdict {
    if let Some(dir) = project_dir {
        let path = dir.join(BITSTREAM_PATH);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let result = inspect(&bytes);
                return Verdict {
                    file: Some(path),
                    result,
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Verdict {
                    file: Some(path),
                    result: Err(BitError::Unreadable(e.to_string())),
                };
            }
        }
    }
    Verdict {
        file: None,
        result: inspect(project_gen::default_bitstream()),
    }
}

/// Refuse to build firmware around a bitstream the FPGA would reject.
///
/// Read fresh on every call: it runs once per Build or Flash click, and a
/// panel's cached verdict can be a second old.
pub fn preflight(files: &ProjectFiles) -> Result<(), String> {
    if !files.main_rs.contains(project_gen::FPGA_BITSTREAM_INCLUDE) {
        return Ok(());
    }
    check_project(files.blob_source.as_deref())
        .result
        .map(|_| ())
        .map_err(|e| blocking_reason(&e))
}

/// A [`BitError`] as the reason a Build or Flash cannot run, in one sentence.
pub fn blocking_reason(e: &BitError) -> String {
    match e {
        // Nothing is known about the bytes, so nothing is said against them.
        BitError::Unreadable(err) => format!(
            "{BITSTREAM_PATH} could not be read: {err}. Close the program that has it \
             open, or check its permissions"
        ),
        _ => format!(
            "{BITSTREAM_PATH} is not a bitstream the FPGA can load: {e}. Replace it, or \
             delete it to go back to the IDE's default"
        ),
    }
}

/// The first line of a refusal shown in a one-line status badge, which the
/// full reason would overflow. The reason itself follows on the next line,
/// where the tabs' own failure panes and hover texts show it.
pub fn refusal(why: &str) -> String {
    format!("FPGA bitstream rejected ({BITSTREAM_PATH})\n{why}")
}

/// How often a panel drawn every frame looks at the file.
const POLL_EVERY: Duration = Duration::from_secs(1);

/// [`check_project`] for a panel drawn every frame.
///
/// The file is looked at no more than once a second, and read again only when
/// its size or mtime moved, so an idle panel costs one `stat` a second.
#[derive(Default)]
pub struct BitstreamWatch {
    polled: Option<Instant>,
    /// What `verdict` was taken from: the folder, and the file's size and mtime
    /// (`None` when it has no file).
    key: Option<(Option<PathBuf>, Option<(u64, Option<SystemTime>)>)>,
    verdict: Option<Verdict>,
}

impl BitstreamWatch {
    pub fn get(&mut self, project_dir: Option<&Path>) -> &Verdict {
        self.get_at(project_dir, Instant::now())
    }

    fn get_at(&mut self, project_dir: Option<&Path>, now: Instant) -> &Verdict {
        // Another project is checked at once, not a second later.
        let same_dir = matches!(&self.key, Some((dir, _)) if dir.as_deref() == project_dir);
        let due = self
            .polled
            .is_none_or(|t| now.saturating_duration_since(t) >= POLL_EVERY);
        if due || !same_dir || self.verdict.is_none() {
            self.polled = Some(now);
            let stamp = project_dir
                .and_then(|d| std::fs::metadata(d.join(BITSTREAM_PATH)).ok())
                .map(|m| (m.len(), m.modified().ok()));
            let key = (project_dir.map(Path::to_path_buf), stamp);
            // A read that failed says nothing about the bytes, so the stamp
            // cannot vouch for it: a released lock or a fixed ACL moves neither
            // size nor mtime. Retried at every poll until it succeeds.
            let unreadable = matches!(
                self.verdict.as_ref().map(|v| &v.result),
                Some(Err(BitError::Unreadable(_)))
            );
            if unreadable || self.key.as_ref() != Some(&key) || self.verdict.is_none() {
                self.verdict = Some(check_project(project_dir));
                self.key = Some(key);
            }
        }
        self.verdict
            .get_or_insert_with(|| check_project(project_dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> &'static [u8] {
        project_gen::default_bitstream()
    }

    /// A minimal image: `banks` CRAM banks of `width` x `height`, zero bits,
    /// with a correct CRC - enough to reach the device check.
    fn image(width: u32, height: u32, banks: u8) -> Vec<u8> {
        let mut d = vec![0xFF, 0x00, 0x00, 0xFF];
        d.extend(PREAMBLE);
        d.extend([0x51, 0x00, 0x01, 0x05, 0x92, 0x00, 0x00]);
        let w = width - 1;
        d.extend([0x62, (w >> 8) as u8, w as u8]);
        d.extend([0x72, (height >> 8) as u8, height as u8]);
        d.extend([0x82, 0x00, 0x00]);
        for bank in 0..banks {
            d.extend([0x11, bank, 0x01, 0x01]);
            d.extend(std::iter::repeat_n(0u8, (width * height / 8) as usize + 2));
        }
        // The check command, then the CRC of everything since the reset (01 05)
        // up to and including it: appended big endian, it leaves residue 0.
        let reset = 4 + PREAMBLE.len() + 4;
        d.push(0x22);
        let crc = crc16(0xFFFF, &d[reset..]);
        d.extend(crc.to_be_bytes());
        d.extend([0x01, 0x06, 0x00]);
        d
    }

    #[test]
    fn the_shipped_default_is_a_clean_up5k_image() {
        let info = inspect(shipped()).expect("the default gateware loads");
        assert_eq!(info.len, UP5K_IMAGE_LEN);
        assert!(info.crc_checked, "icepack writes a CRC check");
        assert!(!info.factory_image, "the default must not be rgb_blink");
        assert!(info.warnings().is_empty() && info.notes().is_empty());
    }

    #[test]
    fn one_flipped_bit_fails_the_crc() {
        let mut d = shipped().to_vec();
        d[50_000] ^= 0x10;
        assert!(matches!(inspect(&d), Err(BitError::Crc { .. })));
    }

    #[test]
    fn each_kind_of_wrong_file_gets_its_own_error() {
        assert_eq!(inspect(&[]), Err(BitError::Empty));
        assert_eq!(inspect(&[0u8; 1000]), Err(BitError::Blank(0x00)));
        assert_eq!(inspect(&[0xFFu8; 1000]), Err(BitError::Blank(0xFF)));
        let mut uf2 = b"UF2\nWQ]\x9e".to_vec();
        uf2.resize(512, 0);
        assert_eq!(inspect(&uf2), Err(BitError::Uf2));
        assert_eq!(
            inspect(b".comment Generated by nextpnr\n.device 5k\n"),
            Err(BitError::AscText)
        );
        assert_eq!(inspect(b"just some text"), Err(BitError::NoPreamble));
        assert_eq!(
            inspect(&shipped()[..60_000]),
            Err(BitError::Truncated { len: 60_000 })
        );
        // The file ends `22 hi lo 01 06 00`: the CRC check, then the wake-up.
        // Cut between the two, the CRC still passes and the FPGA never wakes.
        let d = shipped();
        let n = d.len();
        assert_eq!(&d[n - 3..], &[0x01, 0x06, 0x00]);
        assert_eq!(
            inspect(&d[..n - 3]),
            Err(BitError::Truncated { len: n - 3 })
        );
        // Cut inside a command: `01` is there, its payload `06` is not.
        assert_eq!(
            inspect(&d[..n - 2]),
            Err(BitError::Truncated { len: n - 2 })
        );
    }

    /// What icepack writes when the .asc's `.comment` has lines: FF 00, the
    /// text, 00 FF, then the image from its sync word on.
    fn with_comment(body: usize) -> Vec<u8> {
        let mut d = vec![0xFF, 0x00];
        d.extend(std::iter::repeat_n(b'x', body));
        d.extend([0x00, 0xFF]);
        d.extend(&shipped()[4..]);
        d
    }

    #[test]
    fn the_sync_word_may_end_at_byte_4096_and_no_later() {
        // A few `.comment` lines, or iCEcube2's Lattice/Part/Date header.
        let d = with_comment(300);
        assert_eq!(d.windows(4).position(|w| w == PREAMBLE), Some(304));
        assert!(inspect(&d).is_ok(), "{:?}", inspect(&d));
        // Literal 4096, not PREAMBLE_WINDOW: the error text says "4 KiB".
        let edge = with_comment(4088);
        assert_eq!(edge.windows(4).position(|w| w == PREAMBLE), Some(4092));
        assert!(inspect(&edge).is_ok(), "the sync word ends at byte 4096");
        assert_eq!(inspect(&with_comment(4089)), Err(BitError::NoPreamble));
    }

    #[test]
    fn a_damaged_single_image_is_not_called_multi_boot_or_cut_short() {
        // `01 06` -> `41 06`, and the CRC reset `01 05` -> `41 05`: a boot
        // address command only in the eyes of a careless reader.
        for at in [n_minus(3), 10] {
            let mut d = shipped().to_vec();
            d[at] ^= 0x40;
            assert!(
                matches!(inspect(&d), Err(BitError::Corrupt { .. })),
                "byte {at}: {:?}",
                inspect(&d)
            );
        }
        // A high bit in the CRAM width: a full-length file, not a truncated one.
        let mut d = shipped().to_vec();
        d[16] ^= 0x80;
        assert!(
            matches!(inspect(&d), Err(BitError::Corrupt { .. })),
            "{:?}",
            inspect(&d)
        );
    }

    /// Index of the `n`-th byte from the end of the shipped image.
    fn n_minus(n: usize) -> usize {
        shipped().len() - n
    }

    #[test]
    fn an_icemulti_header_is_refused() {
        // What icemulti writes at offset 0: boot mode, boot address, bank
        // offset, reboot.
        let mut d = PREAMBLE.to_vec();
        d.extend([0x92, 0x00, 0x00, 0x44, 0x03, 0x00, 0x01, 0x00]);
        d.extend([0x82, 0x00, 0x00, 0x01, 0x08]);
        d.resize(160, 0);
        d.extend(shipped());
        assert_eq!(inspect(&d), Err(BitError::MultiBoot));
    }

    #[test]
    fn a_synthetic_image_passes_its_own_crc() {
        // The helper below is what the device tests stand on.
        let d = image(692, 336, 1);
        assert!(inspect(&d).is_ok(), "{:?}", inspect(&d));
    }

    #[test]
    fn another_part_is_named_in_the_error() {
        let err = inspect(&image(872, 272, 1)).unwrap_err();
        assert_eq!(err, BitError::WrongDevice("iCE40HX8K / HX4K / LP8K / LP4K"));
        assert!(err.to_string().contains("HX8K"), "{err}");
        assert_eq!(
            inspect(&image(182, 80, 1)),
            Err(BitError::WrongDevice("iCE40LP384"))
        );
        assert_eq!(
            inspect(&image(100, 40, 1)),
            Err(BitError::UnknownGeometry {
                width: 100,
                height: 40
            })
        );
    }

    #[test]
    fn the_hash_is_fnv_1a_64() {
        // The published test vectors, so FACTORY_RGB_BLINK (taken with the
        // same function from tinyVision's file) still means what it did.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn the_factory_hash_runs_from_the_sync_word_to_the_wake_up() {
        let d = shipped();
        // The image proper: the sync word at 4, the wake-up ends one byte
        // before the file does.
        let hash = fnv1a64(&d[4..d.len() - 1]);
        let info = inspect_with(d, hash).unwrap();
        assert!(info.factory_image);
        assert!(info.warnings()[0].contains("rgb_blink"));
        // Another comment header, or other padding after the wake-up, is still
        // the same image.
        let mut c = with_comment(80);
        assert!(inspect_with(&c, hash).unwrap().factory_image);
        c.extend([0u8; 16]);
        assert!(inspect_with(&c, hash).unwrap().factory_image);
    }

    #[test]
    fn a_vendor_comment_changes_the_size_and_only_adds_a_note() {
        let mut d = vec![0xFF, 0x00];
        d.extend(b"Lattice iCEcube2 2020.12.27914\0Part: iCE40UP5K-SG48\0");
        d.extend([0x00, 0xFF]);
        d.extend(&shipped()[4..]);
        let info = inspect(&d).expect("a comment changes nothing the FPGA reads");
        assert!(info.crc_checked && !info.factory_image);
        assert!(info.warnings().is_empty());
        let notes = info.notes();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("104,090"), "{notes:?}");
    }

    #[test]
    fn the_project_file_wins_over_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let none = check_project(Some(dir.path()));
        assert_eq!(none.file, None, "no file: the build embeds the default");
        assert!(none.result.is_ok());

        std::fs::create_dir_all(dir.path().join("fpga")).unwrap();
        std::fs::write(dir.path().join(BITSTREAM_PATH), b"UF2\n....").unwrap();
        let own = check_project(Some(dir.path()));
        assert_eq!(own.file, Some(dir.path().join(BITSTREAM_PATH)));
        assert_eq!(own.result, Err(BitError::Uf2));
    }

    #[test]
    fn only_a_project_that_loads_the_fpga_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("fpga")).unwrap();
        std::fs::write(dir.path().join(BITSTREAM_PATH), [0u8; 64]).unwrap();
        let mut files = ProjectFiles {
            blob_source: Some(dir.path().to_path_buf()),
            ..ProjectFiles::default()
        };
        assert_eq!(preflight(&files), Ok(()), "no include_bytes!, no check");
        files.main_rs = format!(
            "static B: &[u8] = {};\n",
            project_gen::FPGA_BITSTREAM_INCLUDE
        );
        let why = preflight(&files).unwrap_err();
        assert!(why.starts_with("fpga/top.bin is not"), "{why}");
        assert!(why.contains("0x00"), "{why}");
    }

    #[test]
    fn the_watch_rereads_only_when_due_and_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(BITSTREAM_PATH);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, shipped()).unwrap();
        let mut watch = BitstreamWatch::default();
        let t0 = Instant::now();
        assert!(watch.get_at(Some(dir.path()), t0).result.is_ok());

        std::fs::write(&path, &shipped()[..1000]).unwrap();
        let soon = t0 + Duration::from_millis(500);
        assert!(
            watch.get_at(Some(dir.path()), soon).result.is_ok(),
            "not due yet: the last verdict stands"
        );
        let later = t0 + Duration::from_millis(1100);
        assert_eq!(
            watch.get_at(Some(dir.path()), later).result,
            Err(BitError::Truncated { len: 1000 })
        );

        // Another folder is looked at straight away.
        let other = tempfile::tempdir().unwrap();
        assert_eq!(watch.get_at(Some(other.path()), later).file, None);
    }

    /// Overwrite `path` with `bytes`, then pin its mtime to `stamp` - the
    /// filesystem clock may not tick between two writes.
    fn write_stamped(path: &Path, bytes: &[u8], stamp: SystemTime) {
        std::fs::write(path, bytes).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(stamp)
            .unwrap();
    }

    #[test]
    fn the_watch_key_is_the_files_size_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(BITSTREAM_PATH);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        write_stamped(&path, shipped(), old);
        let mut watch = BitstreamWatch::default();
        let t0 = Instant::now();
        assert!(watch.get_at(Some(dir.path()), t0).result.is_ok());

        // Every UP5K image icepack writes is 104,090 B, so a real replacement
        // keeps its size: only the mtime tells.
        let mut flipped = shipped().to_vec();
        flipped[50_000] ^= 0x10;
        write_stamped(&path, &flipped, old);
        let t1 = t0 + POLL_EVERY;
        assert!(
            watch.get_at(Some(dir.path()), t1).result.is_ok(),
            "same size, same mtime: not read again"
        );
        write_stamped(&path, &flipped, old + Duration::from_secs(1));
        let t2 = t1 + POLL_EVERY;
        assert!(matches!(
            watch.get_at(Some(dir.path()), t2).result,
            Err(BitError::Crc { .. })
        ));

        // The size alone tells too.
        write_stamped(&path, &shipped()[..2000], old + Duration::from_secs(1));
        let t3 = t2 + POLL_EVERY;
        assert_eq!(
            watch.get_at(Some(dir.path()), t3).result,
            Err(BitError::Truncated { len: 2000 })
        );
    }

    /// A file another program holds without sharing it cannot be read, and
    /// letting go moves neither its size nor its mtime.
    #[cfg(windows)]
    #[test]
    fn an_unreadable_file_is_tried_again_once_it_is_let_go() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(BITSTREAM_PATH);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, shipped()).unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let mut watch = BitstreamWatch::default();
        let t0 = Instant::now();
        let first = watch.get_at(Some(dir.path()), t0).result.clone();
        assert!(matches!(first, Err(BitError::Unreadable(_))), "{first:?}");
        let why = blocking_reason(first.as_ref().unwrap_err());
        assert!(why.contains("could not be read"), "{why}");
        assert!(
            !why.contains("delete it"),
            "a good file is not to be deleted: {why}"
        );

        drop(held);
        assert!(
            watch
                .get_at(Some(dir.path()), t0 + POLL_EVERY)
                .result
                .is_ok()
        );
    }

    #[test]
    fn a_refusal_leads_with_a_line_short_enough_for_a_badge() {
        let why = blocking_reason(&BitError::WrongDevice(PARTS[2].1));
        let shown = refusal(&why);
        let first = shown.lines().next().unwrap();
        assert!(first.len() <= 60, "{first}");
        assert!(shown.ends_with(&why), "the full reason follows");
    }
}

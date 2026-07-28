//! Converter: STMicroelectronics **STM32_open_pin_data** XML → [`McuForm`]s.
//!
//! ST publishes one XML per MCU in
//! <https://github.com/STMicroelectronics/STM32_open_pin_data> (`mcu/*.xml`):
//! vendor pin / alternate-function data, the same source CubeMX uses. This is
//! the deterministic bulk-import path (no AI, no guessing) for adding many
//! STM32 chips at once — the biggest lever for growing the chip catalog.
//!
//! One file can describe a **range** of flash variants — `RefName` looks like
//! `STM32F103C(8-B)Tx` with one `<Flash>` element per code — so a single XML
//! expands into several concrete chips. Each becomes a [`McuForm`] (reusing its
//! token grammar, validation and clock model); the caller saves the ones whose
//! [`McuForm::errors`] is empty as `.ron`, exactly like the New MCU form.
//!
//! Signals that don't map to the IDE's function-token grammar (SDMMC, FMC,
//! `TIMx_CHyN`, `ADCx_EXTI…`, oscillator/JTAG, …) are simply dropped — every
//! I/O pin still carries `in out` from its `GPIO` signal, which is what the
//! Pins canvas needs.

use super::mcu_catalog::ToolchainKind;
use super::mcu_form::{ClockChoice, McuForm, PinRow};

/// One chip produced from the XML (a range file yields several), plus any
/// per-file advisories to surface after the import.
pub struct ConvertedChip {
    pub form: McuForm,
    pub warnings: Vec<String>,
}

/// Parse one STM32 open-pin-data `<Mcu>` document into one form per flash
/// variant. `Err` only on unusable XML (not a `<Mcu>` / no `RefName`).
pub fn convert_xml(xml: &str) -> Result<Vec<ConvertedChip>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("XML parse error: {e}"))?;
    let mcu = doc.root_element();
    // The document uses a default namespace (`xmlns="http://dummy.com"`), so we
    // match by LOCAL name throughout.
    if mcu.tag_name().name() != "Mcu" {
        return Err("not an STM32 open-pin-data file (root is not <Mcu>)".into());
    }
    let ref_name = mcu
        .attribute("RefName")
        .ok_or("XML has no RefName attribute")?
        .trim();
    let family = mcu.attribute("Family").unwrap_or("").trim().to_ascii_lowercase();
    let package = mcu.attribute("Package").unwrap_or("").trim().to_string();
    let line = mcu.attribute("Line").unwrap_or("").trim().to_string();

    let mut core = String::new();
    let mut rams: Vec<u64> = Vec::new();
    let mut flashes: Vec<u64> = Vec::new();
    let mut pin_rows: Vec<PinRow> = Vec::new();
    let mut skipped_positions = 0usize;

    for ch in mcu.children().filter(|n| n.is_element()) {
        match ch.tag_name().name() {
            "Core" => core = ch.text().unwrap_or("").trim().to_string(),
            "Ram" => {
                if let Some(v) = ch.text().and_then(|t| t.trim().parse::<u64>().ok()) {
                    rams.push(v);
                }
            }
            "Flash" => {
                if let Some(v) = ch.text().and_then(|t| t.trim().parse::<u64>().ok()) {
                    flashes.push(v);
                }
            }
            "Pin" => {
                let position = ch.attribute("Position").unwrap_or("").trim();
                let name_raw = ch.attribute("Name").unwrap_or("").trim();
                let ptype = ch.attribute("Type").unwrap_or("").trim();
                // The exposed thermal pad is not a pin (no package position).
                if is_exposed_pad(name_raw) {
                    continue;
                }
                // BGA / non-numeric positions ("A1") can't be a pin number here.
                if position.parse::<usize>().is_err() {
                    if !position.is_empty() {
                        skipped_positions += 1;
                    }
                    continue;
                }
                let reserved = !(ptype == "I/O" || ptype == "MonoIO");
                let mut tokens: Vec<String> = Vec::new();
                if !reserved {
                    for sig in ch
                        .children()
                        .filter(|n| n.is_element() && n.tag_name().name() == "Signal")
                    {
                        if let Some(tok) = sig.attribute("Name").and_then(map_signal) {
                            for t in tok.split_whitespace() {
                                if !tokens.iter().any(|x| x == t) {
                                    tokens.push(t.to_string());
                                }
                            }
                        }
                    }
                }
                pin_rows.push(PinRow {
                    number: position.to_string(),
                    name: clean_pin_name(name_raw),
                    reserved,
                    functions: tokens.join(" "),
                    imported: false,
                });
            }
            _ => {}
        }
    }

    pin_rows.sort_by_key(|r| r.number.parse::<usize>().unwrap_or(usize::MAX));
    let sides = distribute_sides(&pin_rows);

    let clock = clock_for_family(&family);
    let target = core_to_target(&core).to_string();
    let cpu = core.trim_start_matches("Arm ").trim().to_string();

    let mut base_warnings = Vec::new();
    if skipped_positions > 0 {
        base_warnings.push(format!(
            "{skipped_positions} pin(s) had non-numeric positions (BGA package?) and were skipped."
        ));
    }
    if pin_rows.is_empty() {
        base_warnings.push("No usable (numeric-position) pins were found.".into());
    }

    let mut chips = Vec::new();
    for (name, idx) in expand_variants(ref_name) {
        // Range codes pair 1:1, in order, with the <Flash> entries.
        let flash_k = flashes.get(idx).or_else(|| flashes.last()).copied();
        let ram_k = rams.get(idx).or_else(|| rams.last()).copied();

        let mut form = McuForm::blank();
        form.id = slugify(&name);
        form.display_name = name.clone();
        form.family = family.clone();
        form.cpu = cpu.clone();
        form.package = package.clone();
        form.toolchain = ToolchainKind::RustEmbedded;
        form.target = target.clone();
        form.flash_origin = "0x08000000".into();
        form.flash_size = flash_k.map(|k| format!("{k}K")).unwrap_or_default();
        form.ram_origin = "0x20000000".into();
        form.ram_size = ram_k.map(|k| format!("{k}K")).unwrap_or_default();
        form.probe_chip = name.clone();
        form.hal_dep = hal_dep_for(&family, &line, &name);
        form.memory_comment = format!("Imported from STM32 open-pin-data ({ref_name})");
        form.clock = clock;
        form.pins = [
            sides[0].clone(),
            sides[1].clone(),
            sides[2].clone(),
            sides[3].clone(),
        ];
        chips.push(ConvertedChip { form, warnings: base_warnings.clone() });
    }
    Ok(chips)
}

/// `true` for the exposed thermal / ground pad under QFN-style packages —
/// drawn INSIDE the package outline as "exposed pad VSS", "EPAD" or "thermal
/// pad". It carries no pin number, so it must never enter the pin list.
/// A normal numbered `VSS` pin is NOT one of these. `pub(crate)` — shared with
/// the AI datasheet import.
pub(crate) fn is_exposed_pad(name: &str) -> bool {
    let n = name.trim().to_ascii_uppercase();
    let squashed = n.replace(['-', '_'], " ");
    squashed.contains("EXPOSED PAD")
        || squashed.contains("EXPOSEDPAD")
        || squashed.contains("THERMAL PAD")
        || squashed == "EPAD"
        || squashed == "PAD"
}

/// Signals with no pin function worth modelling: the per-pin interrupt/event
/// lines every I/O carries, plus debug/JTAG, oscillator + clock-control and RTC
/// housekeeping. These are the ONLY signals dropped. `pub(crate)` — shared with
/// the AI datasheet import.
pub(crate) fn is_noise_signal(sig: &str) -> bool {
    let s = sig.trim();
    s.is_empty()
        || s == "EVENTOUT"
        || s.contains("EXTI")
        || s.contains("WKUP")
        || s.starts_with("RCC_")
        || s.starts_with("RTC_")
        || s.starts_with("SYS_")
        || s.starts_with("DEBUG")
}

/// Map one STM32 signal name to the IDE's function token(s).
///
/// Order matters: a NATIVE token wins first (so `GPIO`, `SYS_JTMS-SWDIO` and
/// `RCC_MCO` map even though the last two look like "noise"); then true noise
/// is dropped; **everything else becomes a generic `af:<name>` token** so no
/// pin function is ever silently lost — SAI, FMC, DCMI, QUADSPI, LTDC, ETH,
/// SDMMC, `TIMx_CHyN`, `I2Cx_SMBA`, … all survive with their datasheet name.
/// `pub(crate)` so the AI datasheet import maps signals the SAME way.
pub(crate) fn map_signal(sig: &str) -> Option<String> {
    if let Some(tok) = native_token(sig) {
        return Some(tok);
    }
    if is_noise_signal(sig) {
        return None;
    }
    Some(format!("af:{}", sig.trim().to_ascii_lowercase()))
}

/// The natively-modelled subset — `None` when the IDE has no dedicated
/// [`PinFunction`] for this signal.
fn native_token(sig: &str) -> Option<String> {
    // Debug: signals look like `SYS_JTMS-SWDIO` / `SYS_JTCK-SWCLK`.
    if sig.contains("SWDIO") {
        return Some("swdio".into());
    }
    if sig.contains("SWCLK") {
        return Some("swclk".into());
    }
    // Generic-IO capability → both directions.
    if sig == "GPIO" {
        return Some("in out".into());
    }
    // USB data lines (`USB_DM`/`USB_DP`, `USB_OTG_FS_DM`/`_DP`, …).
    if sig.starts_with("USB") {
        if sig.ends_with("_DM") {
            return Some("usb_dm".into());
        }
        if sig.ends_with("_DP") {
            return Some("usb_dp".into());
        }
    }
    // CAN / FDCAN — the IDE token carries no instance number.
    if sig.starts_with("CAN") || sig.starts_with("FDCAN") {
        if sig.ends_with("_RX") {
            return Some("can_rx".into());
        }
        if sig.ends_with("_TX") {
            return Some("can_tx".into());
        }
    }
    // Main clock output (`RCC_MCO`, `RCC_MCO_1`).
    if sig.contains("_MCO") {
        return Some("mco".into());
    }
    // Instance peripherals: `<WORD><n>_<ROLE>`.
    let (head, tail) = sig.split_once('_')?;
    let split = head.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (word, n) = head.split_at(split);
    if n.is_empty() {
        return None; // needs an instance number
    }
    match word {
        // Plain UART maps onto usart; LPUART is its own peripheral (below).
        "USART" | "UART" => {
            let role = match tail {
                "TX" => "tx",
                "RX" => "rx",
                "CTS" => "cts",
                // RTS_DE is the same pin as RTS (it doubles as the RS485
                // driver-enable), so both spellings map to RTS.
                "RTS" | "RTS_DE" | "DE" => "rts",
                "CK" => "ck",
                _ => return None,
            };
            Some(format!("usart{n}_{role}"))
        }
        "LPUART" => {
            let role = match tail {
                "TX" => "tx",
                "RX" => "rx",
                "CTS" => "cts",
                "RTS" | "RTS_DE" | "DE" => "rts",
                _ => return None,
            };
            Some(format!("lpuart{n}_{role}"))
        }
        "SPI" => {
            let role = match tail {
                "NSS" => "nss",
                "SCK" => "sck",
                "MISO" => "miso",
                "MOSI" => "mosi",
                "RDY" => "rdy",
                _ => return None,
            };
            Some(format!("spi{n}_{role}"))
        }
        "I2C" => {
            let role = match tail {
                "SCL" => "scl",
                "SDA" => "sda",
                _ => return None, // e.g. I2Cx_SMBA
            };
            Some(format!("i2c{n}_{role}"))
        }
        "ADC" => {
            let ch: u32 = tail.strip_prefix("IN")?.parse().ok()?; // drops ADCx_EXTIy
            // Combined instances (`ADC12_IN5`) → the first instance only, so the
            // token stays valid (`adc1_5`, never a non-existent `adc12`).
            let inst = n.chars().next()?;
            Some(format!("adc{inst}_{ch}"))
        }
        "TIM" => {
            let ch = tail.strip_prefix("CH")?;
            // Plain channels only — skip complementary `CHyN`, `_ETR`, `_BKIN`.
            if ch.is_empty() || !ch.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let ch: u32 = ch.parse().ok()?;
            Some(format!("tim{n}_{ch}"))
        }
        _ => None,
    }
}

/// Strip the extra tags STM32 pin names carry (`PC13-TAMPER-RTC` → `PC13`,
/// `PA0-WKUP` → `PA0`), but leave non-port names (`VBAT`, `NRST`) untouched.
fn clean_pin_name(raw: &str) -> String {
    let head = raw.split('-').next().unwrap_or(raw);
    let b = head.as_bytes();
    let looks_like_port = b.len() >= 3
        && b[0] == b'P'
        && b[1].is_ascii_uppercase()
        && b[2..].iter().all(u8::is_ascii_digit);
    if looks_like_port {
        head.to_string()
    } else {
        raw.to_string()
    }
}

/// Expand a (possibly range) `RefName` into `(concrete name, flash index)`
/// pairs. `STM32F103C(8-B)Tx` → `[(…C8Tx, 0), (…CBTx, 1)]`; a plain name →
/// one entry at index 0.
fn expand_variants(ref_name: &str) -> Vec<(String, usize)> {
    if let (Some(open), Some(close)) = (ref_name.find('('), ref_name.find(')')) {
        if open < close {
            let prefix = &ref_name[..open];
            let inside = &ref_name[open + 1..close];
            let suffix = &ref_name[close + 1..];
            return inside
                .split('-')
                .enumerate()
                .map(|(i, code)| (format!("{prefix}{code}{suffix}"), i))
                .collect();
        }
    }
    vec![(ref_name.to_string(), 0)]
}

/// Split pins (sorted by number) across the four sides QFP-style, matching the
/// bundled F103 layout: left = first quarter, then bottom, then right, and top
/// last **reversed** (physical counter-clockwise numbering). Returns them in
/// `McuForm::pins` order `[top, bottom, left, right]`. `pub(crate)` so the AI
/// datasheet import lays pins out the SAME way (never "all on one side").
pub(crate) fn distribute_sides(rows: &[PinRow]) -> [Vec<PinRow>; 4] {
    let n = rows.len();
    let base = n / 4;
    let rem = n % 4;
    let mut sizes = [base; 4];
    for s in sizes.iter_mut().take(rem) {
        *s += 1;
    }
    let mut it = rows.iter().cloned();
    let left: Vec<_> = it.by_ref().take(sizes[0]).collect();
    let bottom: Vec<_> = it.by_ref().take(sizes[1]).collect();
    let right: Vec<_> = it.by_ref().take(sizes[2]).collect();
    let mut top: Vec<_> = it.by_ref().take(sizes[3]).collect();
    top.reverse();
    [top, bottom, left, right]
}

/// Cortex core string → Rust target triple. `pub(crate)` so [`mcu_identity`]
/// reuses the same mapping.
pub(crate) fn core_to_target(core: &str) -> &'static str {
    let c = core.to_ascii_lowercase();
    if c.contains("cortex-m0") {
        "thumbv6m-none-eabi"
    } else if c.contains("cortex-m33") {
        "thumbv8m.main-none-eabihf"
    } else if c.contains("cortex-m23") {
        "thumbv8m.base-none-eabi"
    } else if c.contains("cortex-m4") || c.contains("cortex-m7") {
        "thumbv7em-none-eabihf"
    } else if c.contains("cortex-m3") {
        "thumbv7m-none-eabi"
    } else {
        "thumbv7m-none-eabi" // safe default
    }
}

/// Which built-in clock model fits a family (others get a plain reset clock).
/// Delegates to the single source of truth so XML import and AI import agree.
fn clock_for_family(family: &str) -> ClockChoice {
    ClockChoice::for_family(family)
}

/// Per-chip F4 clock ceilings (embassy's `max` table): SYSCLK varies by model
/// and the two PCLK ceilings follow the chip's bus-split rule. Applied by the
/// import handler over the form's F411-class default.
pub fn f4_limits_for_chip(id: &str) -> crate::panels::mcu_module::clock::model::ClockLimits {
    use crate::panels::mcu_module::clock::graph::stm32f4_limits;
    let m = 1_000_000;
    // `id` is the slug, e.g. "stm32f411re" → model "f411".
    let model = id.get(5..9).unwrap_or("");
    let (sysclk, high_split) = match model {
        "f401" => (84 * m, false),
        "f405" | "f407" | "f415" | "f417" => (168 * m, true),
        "f427" | "f429" | "f437" | "f439" | "f446" | "f469" | "f479" => (180 * m, true),
        // f410/f411/f412/f413/f423 and any unrecognised F4 → the 100 MHz class.
        _ => (100 * m, false),
    };
    if high_split {
        stm32f4_limits(sysclk, sysclk / 4, sysclk / 2) // PCLK1 = HCLK/4, PCLK2 = HCLK/2
    } else {
        stm32f4_limits(sysclk, sysclk / 2, sysclk) // PCLK1 = HCLK/2, PCLK2 = HCLK
    }
}

/// The HAL dependency line. STM32F1 keeps its dedicated `stm32f1xx-hal`; every
/// other STM32 family uses `embassy-stm32` (the generic embassy backend) with
/// the per-chip feature; non-STM32 families get a TODO (Cargo.toml is editable).
fn hal_dep_for(family: &str, line: &str, name: &str) -> String {
    if family == "stm32f1" {
        return format!(
            "stm32f1xx-hal = {{ version = \"0.10\", features = [\"{}\", \"rt\"] }}",
            line.to_ascii_lowercase()
        );
    }
    if family.starts_with("stm32") {
        // Generic embassy backend. The chip feature is just the part number —
        // verified to compile with only `features = ["<chip>"]`.
        return format!(
            "embassy-stm32 = {{ version = \"0.4\", features = [\"{}\"] }}",
            embassy_chip_feature(name)
        );
    }
    format!("# TODO: add the HAL / PAC dependency for family {family}")
}

/// The HAL dependency line derived from a chip NAME alone — the path taken by
/// the AI datasheet import and the form's "Auto-fill from name", which (unlike
/// the XML importer) have no `<Line>` attribute to hand to [`hal_dep_for`].
/// STM32F1 keys `stm32f1xx-hal` on its `stm32f1NN` device feature, recovered
/// here from the part number; every other STM32 family uses the embassy
/// per-chip feature; non-STM32 families get the editable TODO line.
pub fn hal_dep_for_name(family: &str, name: &str) -> String {
    if family == "stm32f1" {
        return match f1_line_from_name(name) {
            Some(line) => hal_dep_for(family, &line, name),
            // An F1 family with a name we can't pin to a device feature — leave
            // an editable TODO rather than an empty `features = ["", "rt"]`.
            None => format!("# TODO: set the stm32f1xx-hal device feature for {name}"),
        };
    }
    // `line` is unused outside the F1 branch of `hal_dep_for`.
    hal_dep_for(family, "", name)
}

/// The `stm32f1xx-hal` device feature (`stm32f103`) implied by an STM32F1 part
/// name, or `None` when the name isn't a recognisable F1 part number. The HAL
/// keys the feature on the 3-digit line (`stm32f100/101/103/…`) — the
/// `stm32f1` prefix plus the next two digits.
fn f1_line_from_name(name: &str) -> Option<String> {
    let slug = slugify(name); // lower-case a–z0–9 only → ASCII, byte-indexable
    let ok = slug.len() >= 9
        && slug.starts_with("stm32f1")
        && slug.as_bytes()[7].is_ascii_digit()
        && slug.as_bytes()[8].is_ascii_digit();
    ok.then(|| slug[..9].to_string())
}

/// The embassy-stm32 chip feature for a concrete part: the part number without
/// the trailing package + temperature code (open-pin-data names end in a
/// `<PackageLetter>x` pair — `STM32F411RETx` → `stm32f411re`).
fn embassy_chip_feature(name: &str) -> String {
    let slug = slugify(name);
    if slug.len() > 2 && slug.ends_with('x') {
        slug[..slug.len() - 2].to_string()
    } else {
        slug
    }
}

/// Lower-case, keep only `a–z 0–9` — a valid registry id / file stem.
fn slugify(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compact but format-accurate fixture (default namespace, range RefName,
    /// two `<Flash>`, and pins exercising every signal-mapping branch — plus a
    /// `<Condition>` child and a reserved power pin, as in the real files).
    const F103: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Mcu Family="STM32F1" Line="STM32F103" Package="LQFP48" RefName="STM32F103C(8-B)Tx" xmlns="http://dummy.com">
    <Core>Arm Cortex-M3</Core>
    <Ram>20</Ram>
    <Flash>64</Flash>
    <Flash>128</Flash>
    <Pin Name="VBAT" Position="1" Type="Power"/>
    <Pin Name="PC13-TAMPER-RTC" Position="2" Type="I/O">
        <Signal Name="RTC_OUT"/>
        <Signal Name="GPIO"/>
    </Pin>
    <Pin Name="PA9" Position="3" Type="I/O">
        <Signal Name="TIM1_CH2"/>
        <Signal Name="USART1_TX"/>
        <Signal Name="GPIO"/>
    </Pin>
    <Pin Name="PA11" Position="4" Type="I/O">
        <Signal Name="ADC1_EXTI11"/>
        <Signal Name="CAN_RX"/>
        <Signal Name="TIM1_CH4"/>
        <Signal Name="USART1_CTS"/>
        <Signal Name="USB_DM"/>
        <Signal Name="GPIO"/>
    </Pin>
    <Pin Name="PB6" Position="5" Type="I/O">
        <Signal Name="I2C1_SCL"/>
        <Signal Name="I2C1_SMBA"/>
        <Signal Name="TIM4_CH1"/>
        <Signal Name="GPIO"/>
        <Condition Diagnostic="BZ#1" Expression="(!x)"/>
    </Pin>
    <Pin Name="PB13" Position="6" Type="I/O">
        <Signal Name="SPI2_SCK"/>
        <Signal Name="TIM1_CH1N"/>
        <Signal Name="GPIO"/>
    </Pin>
    <Pin Name="PA13" Position="7" Type="I/O">
        <Signal Name="SYS_JTMS-SWDIO"/>
        <Signal Name="GPIO"/>
    </Pin>
    <Pin Name="PA5" Position="8" Type="I/O">
        <Signal Name="ADC1_IN5"/>
        <Signal Name="ADC2_IN5"/>
        <Signal Name="SPI1_SCK"/>
        <Signal Name="GPIO"/>
    </Pin>
</Mcu>"#;

    fn find<'a>(form: &'a McuForm, name: &str) -> &'a PinRow {
        form.pins
            .iter()
            .flatten()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("pin {name} not found"))
    }

    #[test]
    fn range_expands_into_two_flash_variants() {
        let chips = convert_xml(F103).unwrap();
        assert_eq!(chips.len(), 2);
        assert_eq!(chips[0].form.display_name, "STM32F103C8Tx");
        assert_eq!(chips[0].form.id, "stm32f103c8tx");
        assert_eq!(chips[0].form.flash_size, "64K");
        assert_eq!(chips[1].form.display_name, "STM32F103CBTx");
        assert_eq!(chips[1].form.flash_size, "128K");
        // Shared identity across variants.
        for c in &chips {
            assert_eq!(c.form.family, "stm32f1");
            assert_eq!(c.form.cpu, "Cortex-M3");
            assert_eq!(c.form.package, "LQFP48");
            assert_eq!(c.form.target, "thumbv7m-none-eabi");
            assert_eq!(c.form.ram_size, "20K");
            assert_eq!(c.form.flash_origin, "0x08000000");
            assert_eq!(c.form.clock, ClockChoice::Stm32f1);
        }
    }

    #[test]
    fn signals_map_to_tokens_and_noise_is_dropped() {
        let form = &convert_xml(F103).unwrap()[0].form;
        // GPIO → in out; peripheral signals mapped; EXTI/SMBA/CHyN dropped.
        assert_eq!(find(form, "PA9").functions, "tim1_2 usart1_tx in out");
        assert_eq!(find(form, "PA11").functions, "can_rx tim1_4 usart1_cts usb_dm in out");
        // Not-natively-modelled signals are CARRIED as generic `af:` tokens.
        assert_eq!(find(form, "PB6").functions, "i2c1_scl af:i2c1_smba tim4_1 in out");
        assert_eq!(find(form, "PB13").functions, "spi2_sck af:tim1_ch1n in out");
        assert_eq!(find(form, "PA13").functions, "swdio in out");
        assert_eq!(find(form, "PA5").functions, "adc1_5 adc2_5 spi1_sck in out");
        // Power pin: reserved, no functions, name kept verbatim.
        let vbat = find(form, "VBAT");
        assert!(vbat.reserved);
        assert!(vbat.functions.is_empty());
        // Suffix stripped on the port pin.
        assert!(form.pins.iter().flatten().any(|p| p.name == "PC13"));
    }

    #[test]
    fn built_form_validates_and_builds() {
        for chip in convert_xml(F103).unwrap() {
            assert!(chip.form.errors().is_empty(), "{:?}", chip.form.errors());
            let def = chip.form.to_definition();
            assert!(def.build_mcu().iter_all_pins().count() >= 7);
        }
    }

    #[test]
    fn qfp_side_distribution_matches_the_bundled_layout() {
        // 8 pins → 2 per side: left 1-2, bottom 3-4, right 5-6, top 8-7.
        let rows: Vec<PinRow> = (1..=8)
            .map(|i| PinRow { number: i.to_string(), ..Default::default() })
            .collect();
        let [top, bottom, left, right] = distribute_sides(&rows);
        let nums = |s: &[PinRow]| s.iter().map(|r| r.number.clone()).collect::<Vec<_>>();
        assert_eq!(nums(&left), ["1", "2"]);
        assert_eq!(nums(&bottom), ["3", "4"]);
        assert_eq!(nums(&right), ["5", "6"]);
        assert_eq!(nums(&top), ["8", "7"]); // top is reversed
    }

    #[test]
    fn helpers_behave() {
        assert_eq!(clean_pin_name("PC13-TAMPER-RTC"), "PC13");
        assert_eq!(clean_pin_name("PA0-WKUP"), "PA0");
        assert_eq!(clean_pin_name("VBAT"), "VBAT");
        assert_eq!(clean_pin_name("NRST"), "NRST");
        // Exposed thermal pad — not a pin; a numbered VSS still is.
        assert!(is_exposed_pad("exposed pad VSS"));
        assert!(is_exposed_pad("EPAD"));
        assert!(is_exposed_pad("Thermal-Pad"));
        assert!(is_exposed_pad("PAD"));
        assert!(!is_exposed_pad("VSS"));
        assert!(!is_exposed_pad("VSSA"));
        assert!(!is_exposed_pad("PA0"));
        assert_eq!(map_signal("USART2_RX").as_deref(), Some("usart2_rx"));
        assert_eq!(map_signal("UART4_TX").as_deref(), Some("usart4_tx")); // UART→usart
        assert_eq!(map_signal("ADC123_IN10").as_deref(), Some("adc1_10")); // combined→first
        // Anything the IDE doesn't model natively is CARRIED as a generic
        // alternate function — never dropped.
        assert_eq!(map_signal("FMC_A0").as_deref(), Some("af:fmc_a0"));
        assert_eq!(map_signal("SAI1_SD_A").as_deref(), Some("af:sai1_sd_a"));
        assert_eq!(map_signal("DCMI_D3").as_deref(), Some("af:dcmi_d3"));
        assert_eq!(map_signal("QUADSPI_CLK").as_deref(), Some("af:quadspi_clk"));
        assert_eq!(map_signal("TIM1_CH1N").as_deref(), Some("af:tim1_ch1n"));
        assert_eq!(map_signal("I2C1_SMBA").as_deref(), Some("af:i2c1_smba"));
        // Only true noise is dropped — and natives still win over it.
        assert_eq!(map_signal("EVENTOUT"), None);
        assert_eq!(map_signal("ADC1_EXTI11"), None);
        assert_eq!(map_signal("RCC_OSC_IN"), None);
        assert_eq!(map_signal("SYS_JTDI"), None);
        assert_eq!(map_signal("SYS_JTMS-SWDIO").as_deref(), Some("swdio"));
        assert_eq!(map_signal("RCC_MCO").as_deref(), Some("mco"));
        // Grammar extension: LPUART / RTS_DE / SPI_RDY are no longer dropped.
        assert_eq!(map_signal("LPUART1_TX").as_deref(), Some("lpuart1_tx"));
        assert_eq!(map_signal("LPUART1_RTS_DE").as_deref(), Some("lpuart1_rts"));
        assert_eq!(map_signal("USART2_RTS_DE").as_deref(), Some("usart2_rts"));
        assert_eq!(map_signal("SPI1_RDY").as_deref(), Some("spi1_rdy"));
        assert_eq!(map_signal("SPI3_RDY").as_deref(), Some("spi3_rdy"));
        assert_eq!(core_to_target("Arm Cortex-M33"), "thumbv8m.main-none-eabihf");
        assert_eq!(core_to_target("Arm Cortex-M0+"), "thumbv6m-none-eabi");
        assert_eq!(expand_variants("STM32F103CBTx"), vec![("STM32F103CBTx".to_string(), 0)]);
        assert_eq!(embassy_chip_feature("STM32F411RETx"), "stm32f411re");
        assert_eq!(embassy_chip_feature("STM32G0B1RETx"), "stm32g0b1re");
    }

    #[test]
    fn hal_dep_picks_the_right_crate_per_family() {
        // F1 keeps stm32f1xx-hal; the F103 fixture proves it end-to-end.
        assert!(convert_xml(F103).unwrap()[0].form.hal_dep.contains("stm32f1xx-hal"));
        // Any other STM32 family → embassy-stm32 with the chip feature.
        let g0 = hal_dep_for("stm32g0", "STM32G0B1", "STM32G0B1RETx");
        assert!(g0.contains("embassy-stm32"), "{g0}");
        assert!(g0.contains("\"stm32g0b1re\""), "{g0}");
        // Non-STM32 falls back to a TODO the user completes.
        assert!(hal_dep_for("rp2040", "", "RP2040").contains("TODO"));
    }

    #[test]
    fn hal_dep_from_name_recovers_the_f1_device_feature() {
        // F1: the device feature is the stm32f1NN line pulled from the part
        // number (the AI / auto-fill path has no XML <Line> attribute).
        let f1 = hal_dep_for_name("stm32f1", "STM32F103RBT6");
        assert!(f1.contains("stm32f1xx-hal"), "{f1}");
        assert!(f1.contains("\"stm32f103\""), "{f1}");
        // Other STM32 families → embassy-stm32 with the per-chip feature.
        let g0 = hal_dep_for_name("stm32g0", "STM32G0B1RETx");
        assert!(g0.contains("embassy-stm32") && g0.contains("\"stm32g0b1re\""), "{g0}");
        // An F1 family with an unusable name → editable TODO, never `["", "rt"]`.
        let bad = hal_dep_for_name("stm32f1", "STM32");
        assert!(bad.contains("TODO") && !bad.contains("\"\""), "{bad}");
    }

    #[test]
    fn rejects_non_mcu_xml() {
        assert!(convert_xml("<Root/>").is_err());
        assert!(convert_xml("not xml at all <<<").is_err());
    }
}

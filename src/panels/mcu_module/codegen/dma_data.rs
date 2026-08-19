//! DMA channels a chip has, and the interrupt vector each one is served by.
//!
//! Read from the STM32Cube database at import (`db/mcu/IP/`), never guessed:
//!
//! * `NVIC-<ver>_Modes.xml` names the vectors — `DMA1_Channel1_IRQn`,
//!   `DMA1_Channel2_3_IRQn` (STM32G0 shares one vector between two channels),
//!   `GPDMA1_Channel0_IRQn` (H5 / U5, numbered from ZERO).
//! * `DMA-<ver>_Modes.xml` says whether the chip binds requests to fixed
//!   channels (F0/F1/F2/F3/F4/F7/L0/L1/L4) or muxes them — everything newer,
//!   1107 of the database's 1964 parts. A mux part needs no request table at
//!   all: any free channel can serve any peripheral.
//!
//! Both halves matter to codegen: embassy's `Channel::new` takes a `Binding` for
//! the channel's interrupt, so a channel is only usable together with its vector
//! name — and on a shared vector two channels bind handlers to the SAME key.

/// One DMA channel, under the two names generated code needs.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DmaChannel {
    /// The `embassy_stm32::peripherals` singleton — `DMA1_CH2`, `GPDMA1_CH0`.
    pub peri: String,
    /// The `bind_interrupts!` key — `DMA1_CHANNEL2`, `DMA1_CHANNEL2_3`,
    /// `DMA1_CH4_7_DMAMUX1_OVR`. Shared by every channel on the same vector.
    pub irq: String,
}

/// Every DMA channel of a chip, from its NVIC vector list.
///
/// A vector serves one channel (`DMA1_Channel1`) or a range (`DMA1_Channel2_3`,
/// `DMA1_Ch4_7_DMAMUX1_OVR`); a range is expanded here so the allocator can hand
/// the channels out one at a time.
pub fn channels_from_nvic(nvic_xml: &str) -> Vec<DmaChannel> {
    let mut out: Vec<DmaChannel> = Vec::new();
    for vector in nvic_vectors(nvic_xml) {
        let Some((ctrl, indices)) = parse_vector(&vector) else {
            continue;
        };
        let irq = vector.to_ascii_uppercase();
        for i in indices {
            let peri = format!("{ctrl}_CH{i}");
            if !out.iter().any(|c| c.peri == peri) {
                out.push(DmaChannel {
                    peri,
                    irq: irq.clone(),
                });
            }
        }
    }
    out
}

/// The interrupt names in an NVIC modes file: `Value="NAME_IRQn:…"`.
fn nvic_vectors(xml: &str) -> Vec<String> {
    let mut v = Vec::new();
    for chunk in xml.split("Value=\"").skip(1) {
        if let Some(name) = chunk.split("_IRQn").next() {
            if !name.is_empty() && !name.contains('"') {
                v.push(name.to_owned());
            }
        }
    }
    v
}

/// `("DMA1", [2, 3])` for `DMA1_Channel2_3`; `None` when the vector is not a
/// DMA channel one.
///
/// Handles the four spellings the database uses: `Channel<n>`, `Channel<a>_<b>`,
/// the abbreviated `Ch<a>_<b>` which can carry an unrelated tail
/// (`DMA1_Ch4_7_DMAMUX1_OVR`), and `Stream<n>` (F2/F4/F7). A stream still
/// becomes a `_CH<n>` peripheral, because that is what embassy calls it — only
/// the interrupt keeps the vendor's `STREAM` spelling, which is also exactly
/// what `bind_interrupts!` wants.
fn parse_vector(vector: &str) -> Option<(String, Vec<u8>)> {
    let (ctrl, rest) = vector.split_once('_')?;
    if !(ctrl.starts_with("DMA") || ctrl.starts_with("GPDMA") || ctrl.starts_with("BDMA")) {
        return None;
    }
    let rest = rest
        .strip_prefix("Channel")
        .or_else(|| rest.strip_prefix("Stream"))
        .or_else(|| rest.strip_prefix("Ch"))?;
    // The leading run of numbers is the range; anything after it belongs to the
    // vector's name, not to the channels it covers.
    let mut nums: Vec<u8> = Vec::new();
    for part in rest.split('_') {
        match part.parse::<u8>() {
            Ok(n) => nums.push(n),
            Err(_) => break,
        }
    }
    match nums.len() {
        0 => None,
        1 => Some((ctrl.to_owned(), vec![nums[0]])),
        _ => {
            let (a, b) = (nums[0], nums[nums.len() - 1]);
            (a <= b).then(|| (ctrl.to_owned(), (a..=b).collect()))
        }
    }
}

/// Does this chip MUX its DMA requests, so that any channel serves any
/// peripheral?
///
/// Decided by the SHAPE of the DMA modes file: a classic controller lists one
/// `<Mode Name="DMA1_Channel4">` block per channel, each naming the requests
/// that channel can carry; a DMAMUX / GPDMA part lists the requests flat,
/// because the mapping is programmable. Keyed off the file rather than the
/// family name, because the database has STM32L4 classic and STM32L4+ muxed.
pub fn is_mux(dma_modes_xml: &str) -> bool {
    let blocks = dma_modes_xml
        .match_indices("<Mode Name=\"")
        .filter(|(i, _)| {
            let rest = &dma_modes_xml[i + 12..];
            is_channel_name(rest.split('"').next().unwrap_or(""))
        })
        .count();
    blocks < 2
}

/// `DMA1_Channel4` / `DMA2_Stream7` — a channel, as the request table names it.
fn is_channel_name(name: &str) -> bool {
    let Some((ctrl, rest)) = name.split_once('_') else {
        return false;
    };
    (ctrl.starts_with("DMA") || ctrl.starts_with("GPDMA") || ctrl.starts_with("BDMA"))
        && rest
            .strip_prefix("Channel")
            .or_else(|| rest.strip_prefix("Stream"))
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// The chip's PRIMARY DMA controller, as `(IP name, version)`.
///
/// A chip can carry several: an H7 has `DMA` plus `BDMA`, an H5 `GPDMA1` and
/// `GPDMA2`, a U5 adds `LPDMA1`. The one picked here only decides whether the
/// chip is muxed (they agree — a part does not mix a fixed request table with a
/// mux); the channel list comes from the NVIC and covers every controller.
///
/// `DMA2D` (the 2-D graphics accelerator), `DMAMUX` (the mux itself, which owns
/// no channels) and `LPBAMLPDMA1` (a U5 low-power autonomous-mode view of
/// LPDMA1) are not DMA controllers and are skipped.
pub fn dma_ip(mcu_xml: &str) -> Option<(String, String)> {
    let doc = roxmltree::Document::parse(mcu_xml).ok()?;
    let mut found: Vec<(u8, String, String)> = doc
        .root_element()
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "IP")
        .filter_map(|n| {
            let name = n.attribute("Name")?;
            let rank = dma_rank(name)?;
            Some((
                rank,
                name.to_owned(),
                n.attribute("Version")?.trim().to_owned(),
            ))
        })
        .collect();
    found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    found.into_iter().next().map(|(_, n, v)| (n, v))
}

/// How much a controller counts as "the" DMA of the chip — lower wins. `None`
/// for an IP that is not a DMA controller at all.
fn dma_rank(name: &str) -> Option<u8> {
    let base = name.trim_end_matches(|c: char| c.is_ascii_digit());
    match base {
        "DMA" => Some(0),
        "GPDMA" => Some(1),
        "HPDMA" => Some(2),
        "BDMA" => Some(3),
        "LPDMA" => Some(4),
        _ => None,
    }
}

/// The chip's NVIC `<IP>` block. Named plainly `NVIC` on a single-core part and
/// `NVIC1` / `NVIC2` where there are two views of the vector table (H5's secure
/// and non-secure, H7's CM7 and CM4); the vectors we care about are in both, so
/// the first one is taken.
pub fn nvic_ip(mcu_xml: &str) -> Option<(String, String)> {
    let doc = roxmltree::Document::parse(mcu_xml).ok()?;
    let mut found: Vec<(String, String)> = doc
        .root_element()
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "IP")
        .filter_map(|n| {
            let name = n.attribute("Name")?;
            if !name.starts_with("NVIC") {
                return None;
            }
            Some((name.to_owned(), n.attribute("Version")?.trim().to_owned()))
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Where an `<IP Name=… Version=…>` block's modes file lives, in the order to
/// try. Normally `<name>-<version>_Modes.xml`, but the database is not
/// consistent about the instance digit: `NVIC1` and `BDMA1` keep theirs while
/// `GPDMA1` files are named `GPDMA-…`. Both spellings are offered rather than
/// guessed at.
pub fn modes_file_names(ip_name: &str, version: &str) -> Vec<String> {
    let mut v = vec![format!("{ip_name}-{version}_Modes.xml")];
    let base = ip_name.trim_end_matches(|c: char| c.is_ascii_digit());
    if base != ip_name && !base.is_empty() {
        v.push(format!("{base}-{version}_Modes.xml"));
    }
    v
}

/// Whether a controller is muxed, judged by its NAME alone.
///
/// Only a fallback for when the modes file is missing — as it is for every
/// GPDMA1/GPDMA2 part, whose files are named after the base IP. It is safe
/// where it applies: GPDMA, HPDMA and LPDMA are the DMAv3 generation, which is
/// programmable by construction. A plain `DMA` says nothing (an F4's is fixed,
/// a G4's is muxed), so that answers `None` and the caller must read the file.
pub fn mux_by_name(ip_name: &str) -> Option<bool> {
    match ip_name.trim_end_matches(|c: char| c.is_ascii_digit()) {
        "GPDMA" | "HPDMA" | "LPDMA" => Some(true),
        _ => None,
    }
}

/// The chip's DMA channels, read from the two vendor files that describe them.
///
/// `mcu_dir` is the folder the `.xml` came from; both files live in its `IP/`
/// sub-folder, exactly like the GPIO alternate-function table. Answers `None`
/// when either is missing — the public open-pin-data repo ships neither, and a
/// chip imported from it keeps working off the hand-written family tables.
///
/// `cache` is keyed by the two IP versions, because one pair of files serves a
/// whole family: importing 200 STM32G4 parts reads them once.
pub fn dma_def_for(
    mcu_xml: &str,
    mcu_dir: Option<&std::path::Path>,
    cache: &mut std::collections::HashMap<
        String,
        Option<crate::panels::mcu_module::mcu_def::DmaDef>,
    >,
) -> Option<crate::panels::mcu_module::mcu_def::DmaDef> {
    let dir = mcu_dir?.join("IP");
    let (nvic_name, nvic_ver) = nvic_ip(mcu_xml)?;
    let (dma_name, dma_ver) = dma_ip(mcu_xml)?;
    let key = format!("{nvic_name}-{nvic_ver}|{dma_name}-{dma_ver}");
    if let Some(hit) = cache.get(&key) {
        return hit.clone();
    }

    let read = |ip: &str, ver: &str| -> Option<String> {
        modes_file_names(ip, ver)
            .into_iter()
            .find_map(|f| std::fs::read_to_string(dir.join(f)).ok())
    };
    let def = (|| {
        let channels = channels_from_nvic(&read(&nvic_name, &nvic_ver)?);
        if channels.is_empty() {
            return None;
        }
        // The modes file is missing for every GPDMA part (its name carries an
        // instance digit the file does not), but those are muxed by
        // construction, so the name settles it.
        let mux = match mux_by_name(&dma_name) {
            Some(m) => m,
            None => is_mux(&read(&dma_name, &dma_ver)?),
        };
        Some(crate::panels::mcu_module::mcu_def::DmaDef { mux, channels })
    })();
    cache.insert(key, def.clone());
    def
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One vector per channel — the STM32G4 shape.
    #[test]
    fn a_dedicated_vector_gives_one_channel() {
        let xml = "<IP>\
            <PossibleValue Comment=\"c1\" Value=\"DMA1_Channel1_IRQn:Y,DMAL0:DMA:DMA1:1,1\"/>\
            <PossibleValue Comment=\"c2\" Value=\"DMA1_Channel2_IRQn:Y,DMAL0:DMA:DMA1:2,2\"/>\
        </IP>";
        let ch = channels_from_nvic(xml);
        assert_eq!(ch.len(), 2);
        assert_eq!(ch[0].peri, "DMA1_CH1");
        assert_eq!(ch[0].irq, "DMA1_CHANNEL1");
    }

    /// STM32G0 shares vectors between channels. Both channels stay usable — the
    /// macro takes several handlers per interrupt — but they carry the SAME key,
    /// which the emitter has to group instead of writing twice.
    #[test]
    fn a_shared_vector_expands_to_every_channel_it_covers() {
        let xml = "<x Value=\"DMA1_Channel1_IRQn:Y\"/>\
                   <x Value=\"DMA1_Channel2_3_IRQn:Y\"/>\
                   <x Value=\"DMA1_Ch4_7_DMAMUX1_OVR_IRQn:Y\"/>";
        let ch = channels_from_nvic(xml);
        let names: Vec<&str> = ch.iter().map(|c| c.peri.as_str()).collect();
        assert_eq!(
            names,
            [
                "DMA1_CH1", "DMA1_CH2", "DMA1_CH3", "DMA1_CH4", "DMA1_CH5", "DMA1_CH6", "DMA1_CH7"
            ]
        );
        assert_eq!(ch[1].irq, "DMA1_CHANNEL2_3");
        assert_eq!(ch[2].irq, "DMA1_CHANNEL2_3", "same vector, same key");
        assert_eq!(ch[3].irq, "DMA1_CH4_7_DMAMUX1_OVR");
    }

    /// GPDMA (H5 / U5) numbers channels from ZERO.
    #[test]
    fn gpdma_channels_start_at_zero() {
        let xml = "<x Value=\"GPDMA1_Channel0_IRQn:Y\"/><x Value=\"GPDMA1_Channel1_IRQn:Y\"/>";
        let ch = channels_from_nvic(xml);
        assert_eq!(ch[0].peri, "GPDMA1_CH0");
        assert_eq!(ch[0].irq, "GPDMA1_CHANNEL0");
    }

    /// Non-DMA vectors are ignored, whatever they look like.
    #[test]
    fn other_vectors_are_not_channels() {
        let xml = "<x Value=\"EXTI2_IRQn:Y\"/><x Value=\"USART1_IRQn:Y\"/>\
                   <x Value=\"DMAMUX1_OVR_IRQn:Y\"/>";
        assert!(channels_from_nvic(xml).is_empty());
    }

    /// The database's real IP blocks, from an STM32H563 (GPDMA + a split NVIC)
    /// and an STM32F411 (plain DMA, one NVIC).
    #[test]
    fn the_primary_controller_and_its_nvic_are_picked_out() {
        let h5 = r#"<Mcu><IP Name="GPDMA1" Version="STM32H5_dma3_Cube"/>
            <IP Name="GPDMA2" Version="Instance2_STM32H5_dma3_Cube"/>
            <IP Name="NVIC1" Version="STM32H57"/><IP Name="NVIC2" Version="STM32H57"/></Mcu>"#;
        assert_eq!(
            dma_ip(h5),
            Some(("GPDMA1".into(), "STM32H5_dma3_Cube".into()))
        );
        assert_eq!(nvic_ip(h5), Some(("NVIC1".into(), "STM32H57".into())));

        let h7 = r#"<Mcu><IP Name="BDMA" Version="STM32H753_dma1_v1_2"/>
            <IP Name="DMA2D" Version="x"/><IP Name="DMAMUX" Version="y"/>
            <IP Name="DMA" Version="STM32H753_dma1_v1_2"/></Mcu>"#;
        assert_eq!(dma_ip(h7).unwrap().0, "DMA", "DMA2D and DMAMUX are not it");
    }

    /// `GPDMA1`'s modes file is named `GPDMA-…`, `NVIC1`'s is `NVIC1-…`.
    #[test]
    fn the_instance_digit_may_or_may_not_be_in_the_file_name() {
        assert_eq!(
            modes_file_names("GPDMA1", "STM32H5_dma3_Cube"),
            [
                "GPDMA1-STM32H5_dma3_Cube_Modes.xml",
                "GPDMA-STM32H5_dma3_Cube_Modes.xml"
            ]
        );
        assert_eq!(modes_file_names("DMA", "v1"), ["DMA-v1_Modes.xml"]);
    }

    /// A DMAv3 controller is muxed whatever its file says; a plain `DMA` has to
    /// be read.
    #[test]
    fn dmav3_controllers_are_known_muxed_without_the_file() {
        assert_eq!(mux_by_name("GPDMA1"), Some(true));
        assert_eq!(mux_by_name("LPDMA1"), Some(true));
        assert_eq!(mux_by_name("DMA"), None);
        assert_eq!(mux_by_name("BDMA"), None);
    }

    /// F2/F4/F7 call a channel a stream. Same peripheral name in embassy.
    #[test]
    fn streams_are_channels() {
        let ch = channels_from_nvic("<x Value=\"DMA2_Stream5_IRQn:Y\"/>");
        assert_eq!(ch[0].peri, "DMA2_CH5");
        assert_eq!(ch[0].irq, "DMA2_STREAM5");
    }

    /// Sweep the real STM32Cube database: how many parts yield channels, and
    /// how the mux / classic split falls. Ignored — it needs the database on
    /// disk (a ~2 GB checkout) and reads a few thousand files.
    ///
    /// ```text
    /// cargo test --bin embedded_ide_0 sweep_the_vendor_database -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs the STM32Cube database on disk"]
    fn sweep_the_vendor_database() {
        let db =
            std::path::Path::new("H:/stm32cube-database-master/stm32cube-database-master/db/mcu");
        if !db.is_dir() {
            eprintln!("database not mounted at {} - nothing checked", db.display());
            return;
        }
        let mut cache = std::collections::HashMap::new();
        let (mut total, mut with, mut mux, mut classic) = (0, 0, 0, 0);
        let mut example: Option<(String, crate::panels::mcu_module::mcu_def::DmaDef)> = None;
        for entry in std::fs::read_dir(db).expect("read db").flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("xml") {
                continue;
            }
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if !name.starts_with("STM32") {
                continue;
            }
            let Ok(xml) = std::fs::read_to_string(&path) else {
                continue;
            };
            total += 1;
            if let Some(d) = dma_def_for(&xml, path.parent(), &mut cache) {
                with += 1;
                if d.mux {
                    mux += 1
                } else {
                    classic += 1
                }
                if name.starts_with("STM32G4") && example.is_none() {
                    example = Some((name, d));
                }
            }
        }
        println!("{with}/{total} parts carry DMA data - {mux} muxed, {classic} classic");
        if let Some((name, d)) = example {
            println!(
                "{name}: mux={} {} channels, first {:?}",
                d.mux,
                d.channels.len(),
                &d.channels[..d.channels.len().min(3)]
            );
        }
        assert!(
            with * 2 > total,
            "most parts should resolve, got {with}/{total}"
        );
    }

    /// The classic / mux split, decided by file shape.
    #[test]
    fn the_request_table_tells_classic_from_mux() {
        let classic = "<Mode Name=\"DMA1_Channel4\"><Mode Name=\"USART1_TX\"/></Mode>\
                       <Mode Name=\"DMA1_Channel5\"><Mode Name=\"USART1_RX\"/></Mode>";
        assert!(!is_mux(classic));
        let f4 = "<Mode Name=\"DMA2_Stream7\"/><Mode Name=\"DMA2_Stream5\"/>";
        assert!(!is_mux(f4), "F2/F4/F7 spell channels 'Stream'");
        let mux = "<RefMode BaseMode=\"DMA_Request\" Name=\"USART1_TX\"/>\
                   <RefMode BaseMode=\"DMA_Request\" Name=\"SPI1_RX\"/>\
                   <Mode Name=\"MEMTOMEM\"/>";
        assert!(is_mux(mux));
    }
}

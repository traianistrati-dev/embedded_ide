//! What a chip frame on the Board tab shows: the chip, its runtime, its Virtual
//! Modules and its devices - by name only.
//!
//! Read from the chip's files WITHOUT opening it as a project: the chip id from
//! `src/main.rs` (Cargo.toml for older projects), the modules, devices and
//! runtime from `mcu.config`. Nothing else is needed, because `@modules` is
//! written after `reconcile_modules` has made the modules match the pins, so it
//! already says what the Pins tab would show. The chip that IS open is built
//! from the live `Mcu` instead ([`ChipView::from_parts`]), so unsaved changes
//! show on the Board as they are made.

use std::collections::BTreeSet;
use std::path::Path;

use crate::panels::mcu_module::mcu::Runtime;
use crate::panels::mcu_module::mcu::gui::modules::{custom_var_name, module_base_name};
use crate::panels::mcu_module::mcu_config::{self, PinGroup};
use crate::panels::mcu_module::mcu_def::McuDefinition;
use crate::panels::mcu_module::modules::{ModuleKind, VirtualModule};

/// One Virtual Module, as a frame shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleItem {
    /// `USART1`, `I2C1`, or a custom module's own name.
    pub name: String,
    pub kind: ModuleKind,
    pub instance: u8,
    /// The pads it is wired to - what ties a device to it.
    pub pins: BTreeSet<usize>,
}

/// One device (a named group of pads on the Pins tab).
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceItem {
    pub name: String,
    /// Indices into [`ChipView::modules`] of every module sharing a pad with
    /// the device. Empty for a device on bare pads (an LED on a plain GPIO).
    pub modules: Vec<usize>,
}

/// Everything one chip frame draws.
#[derive(Clone, Debug, PartialEq)]
pub struct ChipView {
    /// The chip's folder under the system root - its identity in the system.
    pub dir: String,
    /// The chip's display name; empty when it could not be told.
    pub chip: String,
    pub runtime: Option<Runtime>,
    pub modules: Vec<ModuleItem>,
    pub devices: Vec<DeviceItem>,
    /// Why the frame has nothing to show (folder gone, not a project, …).
    pub problem: Option<String>,
}

impl ChipView {
    /// A frame for a chip whose files could not be read.
    pub fn broken(dir: &str, problem: impl Into<String>) -> Self {
        Self {
            dir: dir.to_owned(),
            chip: String::new(),
            runtime: None,
            modules: Vec::new(),
            devices: Vec::new(),
            problem: Some(problem.into()),
        }
    }

    /// Build the frame from a chip's parts - shared by the disk reader and the
    /// live chip, so the two can never describe the same project differently.
    pub fn from_parts(
        dir: &str,
        chip: &str,
        runtime: Option<Runtime>,
        modules: &[VirtualModule],
        groups: &[PinGroup],
    ) -> Self {
        let mut items: Vec<ModuleItem> = modules
            .iter()
            .map(|m| ModuleItem {
                name: module_name(m),
                kind: m.kind,
                instance: m.instance(),
                pins: m.connections.iter().map(|c| c.mcu_pin).collect(),
            })
            .collect();
        // Peripherals first, in kind-then-instance order, custom modules after
        // them by name: the order `mcu.modules` happens to hold changes with
        // every reconcile, and a frame should not reshuffle on a save.
        items.sort_by(|a, b| {
            (a.kind.is_custom(), a.kind, a.instance, &a.name).cmp(&(
                b.kind.is_custom(),
                b.kind,
                b.instance,
                &b.name,
            ))
        });
        let devices = groups
            .iter()
            .filter(|g| g.is_live())
            .map(|g| DeviceItem {
                name: g.name.trim().to_owned(),
                modules: items
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| !m.pins.is_disjoint(&g.pins))
                    .map(|(i, _)| i)
                    .collect(),
            })
            .collect();
        Self {
            dir: dir.to_owned(),
            chip: chip.to_owned(),
            runtime,
            modules: items,
            devices,
            problem: None,
        }
    }

    /// The grey line under the chip name: `stm32_main · Blocking`.
    pub fn subtitle(&self) -> String {
        match self.runtime {
            Some(r) => format!("{} · {}", self.dir, r.as_token()),
            None => self.dir.clone(),
        }
    }
}

/// A module's name on the Board: the peripheral instance (`USART1`), or for a
/// custom module the name its code uses (`irq_in`) - the only name it has.
pub fn module_name(m: &VirtualModule) -> String {
    if m.kind.is_custom() {
        custom_var_name(m)
    } else {
        module_base_name(m).to_owned()
    }
}

/// Read the chip in `<root>/<dir>` for its frame.
pub fn read_chip(root: &Path, dir: &str, defs: &[McuDefinition]) -> ChipView {
    let path = root.join(dir);
    if !path.is_dir() {
        return ChipView::broken(dir, "Folder not found");
    }
    let read = |rel: &Path| std::fs::read_to_string(path.join(rel)).ok();
    let main_rs = read(Path::new("src/main.rs"));
    let cargo = read(Path::new("Cargo.toml"));
    if main_rs.is_none() && cargo.is_none() {
        return ChipView::broken(dir, "Not a project - no Cargo.toml or src/main.rs");
    }
    let id = crate::panels::mcu_module::registry::detect_chip_id(
        defs,
        main_rs.as_deref(),
        cargo.as_deref(),
    );
    let chip = id
        .as_deref()
        .and_then(|id| defs.iter().find(|d| d.id == id))
        .map(|d| d.display_name.clone())
        .unwrap_or_default();
    let cfg = read(Path::new(mcu_config::FILE_NAME));
    let (modules, groups, runtime) = match &cfg {
        Some(text) => (
            mcu_config::parse(text).0,
            mcu_config::parse_groups(text),
            Some(mcu_config::parse_runtime(text)),
        ),
        // Saved before `mcu.config` existed: the modules were a comment
        // marker in main.rs, and there were no devices yet.
        None => (
            main_rs
                .as_deref()
                .map(crate::panels::mcu_module::modules::persist::parse_from_source)
                .unwrap_or_default(),
            Vec::new(),
            id.is_some().then_some(Runtime::Blocking),
        ),
    };
    let mut view = ChipView::from_parts(dir, &chip, runtime, &modules, &groups);
    if id.is_none() {
        view.problem = Some("Unknown chip - no marker in main.rs and no known HAL".to_owned());
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::modules::{
        Connection, ModuleConfig, ModuleSignal, UsartModuleConfig,
    };

    fn usart(instance: u8, tx: usize, rx: usize) -> VirtualModule {
        VirtualModule {
            id: format!("_usart_{instance}"),
            kind: ModuleKind::GenericInterfaceUsart,
            name: format!("_USART{instance}"),
            pos: (0.0, 0.0),
            config: ModuleConfig::Usart(UsartModuleConfig::new(instance)),
            connections: vec![
                Connection {
                    signal: ModuleSignal::Tx,
                    mcu_pin: tx,
                },
                Connection {
                    signal: ModuleSignal::Rx,
                    mcu_pin: rx,
                },
            ],
        }
    }

    fn group(name: &str, pins: &[usize]) -> PinGroup {
        PinGroup {
            name: name.to_owned(),
            pins: pins.iter().copied().collect(),
        }
    }

    /// A device hangs off every module it shares a pad with; one on bare pads
    /// hangs off none; a device the roster is still naming is not drawn.
    #[test]
    fn devices_attach_to_the_modules_they_share_a_pad_with() {
        let mods = [usart(2, 12, 13), usart(1, 30, 31)];
        let groups = [
            group("GPS", &[12, 13]),
            group("Bridge", &[13, 30]),
            group("Status LED", &[45]),
            group("  ", &[12]),
        ];
        let v = ChipView::from_parts("stm32_main", "STM32F103C8", None, &mods, &groups);
        let names: Vec<&str> = v.modules.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            ["USART1", "USART2"],
            "sorted by instance, `_` dropped"
        );
        let dev: Vec<(&str, &[usize])> = v
            .devices
            .iter()
            .map(|d| (d.name.as_str(), d.modules.as_slice()))
            .collect();
        assert_eq!(
            dev,
            [
                ("GPS", &[1][..]),
                ("Bridge", &[0, 1][..]),
                ("Status LED", &[][..])
            ]
        );
    }

    #[test]
    fn the_subtitle_names_the_folder_and_the_runtime() {
        let v = ChipView::from_parts("esp32_radio", "ESP32-C3", Some(Runtime::Async), &[], &[]);
        assert_eq!(v.subtitle(), "esp32_radio · Async");
        assert_eq!(ChipView::broken("x", "gone").subtitle(), "x");
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("roc_board_snap_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A chip read from disk says what opening it would: the marker's chip,
    /// the modules and devices of `mcu.config`, its runtime.
    #[test]
    fn a_chip_is_read_from_its_files() {
        let defs = crate::panels::mcu_module::builtin_definitions();
        let root = scratch("read");
        let chip = root.join("radio");
        std::fs::create_dir_all(chip.join("src")).unwrap();
        std::fs::write(
            chip.join("src/main.rs"),
            "// Auto-generated by RustOnChip\n// rust_on_chip:mcu=esp32c3\nfn main() {}\n",
        )
        .unwrap();
        let mods = [usart(0, 20, 21)];
        let mut cfg = mcu_config::serialize(
            &mods,
            None,
            Runtime::Async,
            crate::panels::mcu_module::modules::ApiStyle::Portable,
        );
        cfg.push_str(&mcu_config::groups_section(&[group("Modem", &[20, 21])]));
        std::fs::write(chip.join(mcu_config::FILE_NAME), cfg).unwrap();

        let v = read_chip(&root, "radio", &defs);
        let esp = defs.iter().find(|d| d.id == "esp32c3").unwrap();
        assert_eq!(v.chip, esp.display_name);
        assert_eq!(v.runtime, Some(Runtime::Async));
        assert_eq!(v.modules.len(), 1);
        assert_eq!(v.modules[0].name, "USART0");
        assert_eq!(
            v.devices,
            [DeviceItem {
                name: "Modem".into(),
                modules: vec![0]
            }]
        );
        assert_eq!(v.problem, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A frame that cannot be filled says why instead of drawing an empty chip.
    #[test]
    fn a_missing_or_foreign_folder_is_reported() {
        let defs = crate::panels::mcu_module::builtin_definitions();
        let root = scratch("broken");
        assert_eq!(
            read_chip(&root, "gone", &defs).problem.as_deref(),
            Some("Folder not found")
        );
        std::fs::create_dir_all(root.join("notes")).unwrap();
        assert!(
            read_chip(&root, "notes", &defs)
                .problem
                .unwrap()
                .starts_with("Not a project")
        );
        std::fs::create_dir_all(root.join("odd/src")).unwrap();
        std::fs::write(root.join("odd/src/main.rs"), "fn main() {}\n").unwrap();
        let odd = read_chip(&root, "odd", &defs);
        assert!(odd.problem.unwrap().starts_with("Unknown chip"));
        assert_eq!(odd.chip, "");
        let _ = std::fs::remove_dir_all(&root);
    }
}

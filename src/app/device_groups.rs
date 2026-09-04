//! The device roster: the list at the top of the Virtual-modules panel where a
//! sensor's pads are gathered under one name.
//!
//! A board part is rarely one peripheral. A radar module is a UART pair and a
//! spare interrupt line; a display is a SPI bus, a D/C pin and a reset pin. The
//! diagram could draw each of those, and the generated file could bind each of
//! them, but neither could say the three belong to ONE thing — so the reader had
//! to hold the datasheet beside the picture to know which input line went with
//! which bus.
//!
//! A group claims nothing. It takes no pad away from a peripheral, changes no
//! binding and no init call; the only thing it reaches in the generated project
//! is one comment (`codegen::common::device_comment`). That is deliberate: the
//! moment a group renamed an identifier, renaming a device in this panel would
//! rewrite names in code the user had already written against.
//!
//! Membership is by PAD NUMBER, never by module id — see
//! [`PinGroup`](crate::panels::mcu_module::mcu_config::PinGroup) for why. This
//! panel is the only place a group is created, renamed, filled or dissolved.

use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::mcu::gui::modules as mod_gui;
use egui_phosphor::regular as ph;

/// What the roster's controls asked for, applied after the UI is drawn.
///
/// Deferred rather than immediate for the usual reason: every row is drawn
/// while the group list is borrowed, and adding a pad can delete a group —
/// which would move the rows still to be drawn.
enum Act {
    /// Start a new, empty device.
    New,
    /// Take the whole device apart (its pads keep their functions).
    Dissolve(usize),
    /// Put every pad of this module in the device.
    AddModule(usize, String),
    /// Put one pad in the device.
    AddPin(usize, usize),
    /// Take one pad out of whatever device holds it.
    Drop(usize),
}

/// A pad that can be added to a device on its own — one the user configured by
/// hand, not one a virtual module already owns.
///
/// Module pads are offered through their MODULE instead: "add the UART" is the
/// gesture, and picking its two pads one at a time only invites half a bus in a
/// device.
fn loose_pins(mcu: &Mcu) -> Vec<(usize, String)> {
    use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
    let claimed: std::collections::HashSet<usize> = mcu
        .modules
        .iter()
        .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin))
        .collect();
    mcu.iter_all_pins()
        .filter(|p| {
            !p.reserved && p.selected_function != PinFunction::Unset && !claimed.contains(&p.number)
        })
        .map(|p| {
            (
                p.number,
                format!("{} — {}", p.name, p.selected_function.short_label()),
            )
        })
        .collect()
}

/// How one pad reads inside a device's row: its name, and what it carries.
fn member_label(mcu: &Mcu, pin: usize) -> String {
    let Some(p) = mcu.find_pin(pin) else {
        return format!("#{pin}");
    };
    match mcu
        .modules
        .iter()
        .find(|m| m.connections.iter().any(|c| c.mcu_pin == pin))
    {
        Some(m) => format!("{} — {}", p.name, mod_gui::module_title(m)),
        None => format!("{} — {}", p.name, p.selected_function.short_label()),
    }
}

/// Whether a roster row names a device yet.
fn named(name: &String) -> bool {
    !name.trim().is_empty()
}

/// A device name nothing else on the board answers to.
fn fresh_name(mcu: &Mcu) -> String {
    (1..)
        .map(|n| format!("Device {n}"))
        .find(|c| !mcu.groups.iter().any(|g| g.name.trim() == c.as_str()))
        .unwrap_or_else(|| "Device".to_owned())
}

/// Write the roster's edited names back to the devices.
///
/// Two things make this safe, and both were learned the hard way.
///
/// FOUND BY OLD NAME, not by row index. A merge ends in `groups.remove(idx)`, so
/// an index captured while the roster was drawn can name a different device by
/// the time it is used — and the deferred actions below can remove a row too.
/// Applying every row by index destroyed devices: the next row's unchanged name
/// was handed to whatever had shifted into its slot, and since that name still
/// existed it merged as well, swallowing the rest of the roster with no undo.
/// The row's OLD name is the one thing that still identifies it after anything
/// else has moved.
///
/// A row still being TYPED IN is set, never merged. Committing a merge on every
/// keystroke destroyed devices in passing: typing "disp" out to "display2"
/// passes through "display".
///
/// `focused[gi]` is that row's field holding the caret this frame; `lost[gi]` is
/// the frame it stops holding it — the one on which the name becomes a decision,
/// even though by then the text has long since stopped changing.
fn apply_renames(
    mcu: &mut Mcu,
    names: &[String],
    before: &[String],
    focused: &[bool],
    lost: &[bool],
) {
    for gi in 0..names.len().min(before.len()) {
        let leaving = lost.get(gi).copied().unwrap_or(false);
        // A row the user is leaving is revisited even when its text has not
        // changed this frame: the merge it may be owed was deferred on every
        // keystroke, so this is the frame that owes it.
        if names[gi] == before[gi] && !leaving {
            continue;
        }
        // The row is almost always still where it was drawn; the name search is
        // the fallback for a frame in which something before us moved it.
        let idx = if mcu.groups.get(gi).is_some_and(|g| g.name == before[gi]) {
            Some(gi)
        } else {
            mcu.groups.iter().position(|g| g.name == before[gi])
        };
        let Some(idx) = idx else { continue };
        if focused.get(gi).copied().unwrap_or(false) && !leaving {
            mcu.set_group_name(idx, &names[gi]);
        } else {
            mcu.rename_group(idx, &names[gi]);
        }
    }
}

/// Draw the roster.
///
/// Nothing is returned: a device reaches the project only through
/// `calculate_mcu_state_hash`, which sees the edit on this same frame and
/// regenerates main.rs. There is no pin file and no config file to re-sync,
/// because a group owns neither.
pub(super) fn device_roster(ui: &mut egui::Ui, mcu: &mut Mcu) {
    // Editable copies. A `TextEdit` needs a `&mut String` and the group list is
    // read all through the loop, so the names are edited here and written back
    // in one pass at the end.
    let mut names: Vec<String> = mcu.groups.iter().map(|g| g.name.clone()).collect();
    let names_before = names.clone();
    let members: Vec<Vec<usize>> = mcu
        .groups
        .iter()
        .map(|g| g.pins.iter().copied().collect())
        .collect();
    let loose = loose_pins(mcu);
    let modules: Vec<(String, String, egui::Color32)> = mcu
        .modules
        .iter()
        .map(|m| {
            (
                m.id.clone(),
                mod_gui::module_title(m),
                mod_gui::module_color(m.kind, m.instance()),
            )
        })
        .collect();
    let mut act: Option<Act> = None;
    // Which row's name field holds the caret this frame, and which one is being
    // left. A name being typed is stored but never merged; the frame it is left
    // on is the frame the merge is owed — see `apply_renames`.
    let mut focused: Vec<bool> = vec![false; names.len()];
    let mut lost: Vec<bool> = vec![false; names.len()];

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Devices:")
                .size(12.0)
                .color(egui::Color32::from_rgb(150, 150, 160)),
        );
        if ui
            .button(egui::RichText::new(format!("{} Device", ph::PLUS)).size(11.0))
            .on_hover_text(
                "Gather the pads of one board part — a sensor's bus and its spare \
                 interrupt line — under one name.\n\nA device claims nothing: it \
                 renames no binding and moves no pin. It marks the pads on the \
                 diagram and writes one comment into the generated file.",
            )
            .clicked()
        {
            act = Some(Act::New);
        }
    });

    if mcu.groups.is_empty() {
        ui.label(
            egui::RichText::new("No devices yet.")
                .size(10.0)
                .italics()
                .color(egui::Color32::from_gray(120)),
        );
    }

    let focus = &mut focused;
    let left = &mut lost;
    for (gi, name) in names.iter_mut().enumerate() {
        let c = mod_gui::group_color(name);
        egui::Frame::new()
            .fill(egui::Color32::from_rgba_unmultiplied(
                c.r(),
                c.g(),
                c.b(),
                22,
            ))
            .inner_margin(egui::Margin::symmetric(6, 3))
            .corner_radius(egui::CornerRadius::same(4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // The colour the diagram marks this device's pads with —
                    // shown here so the roster and the picture can be matched
                    // without reading either.
                    let (r, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 2.0, c);
                    let edit = ui.add(
                        egui::TextEdit::singleline(name)
                            .desired_width(ui.available_width() - 46.0)
                            .font(egui::TextStyle::Small)
                            .hint_text("name"),
                    );
                    // Still being typed in: `apply_renames` stores it without
                    // letting it merge onto a name it is only passing through.
                    focus[gi] = edit.has_focus();
                    left[gi] = edit.lost_focus();
                    edit.on_hover_text(
                        "The device's name. It appears on the generated comment and \
                         nowhere else in the code — renaming it is always safe.",
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(egui::RichText::new(ph::TRASH).size(11.0))
                            .on_hover_text("Take the device apart. Its pads keep their functions.")
                            .clicked()
                        {
                            act = Some(Act::Dissolve(gi));
                        }
                        ui.menu_button(egui::RichText::new(ph::PLUS).size(11.0), |ui| {
                            ui.set_min_width(180.0);
                            let mine: std::collections::HashSet<usize> =
                                members[gi].iter().copied().collect();
                            let mut any = false;
                            for (id, title, mc) in &modules {
                                if ui
                                    .button(egui::RichText::new(title).size(11.0).color(*mc))
                                    .clicked()
                                {
                                    act = Some(Act::AddModule(gi, id.clone()));
                                    ui.close();
                                }
                                any = true;
                            }
                            let free: Vec<&(usize, String)> =
                                loose.iter().filter(|(n, _)| !mine.contains(n)).collect();
                            if any && !free.is_empty() {
                                ui.separator();
                            }
                            for (n, label) in free {
                                if ui.button(egui::RichText::new(label).size(11.0)).clicked() {
                                    act = Some(Act::AddPin(gi, *n));
                                    ui.close();
                                }
                            }
                            if !any && loose.is_empty() {
                                ui.label(
                                    egui::RichText::new("Nothing configured to add yet.")
                                        .size(10.0)
                                        .color(egui::Color32::from_gray(130)),
                                );
                            }
                        })
                        .response
                        .on_hover_text("Add a module's pads, or one configured pin.");
                    });
                });
                for pin in &members[gi] {
                    ui.horizontal(|ui| {
                        ui.add_space(13.0);
                        ui.label(
                            egui::RichText::new(member_label(mcu, *pin))
                                .size(10.0)
                                .color(egui::Color32::from_gray(180)),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button(egui::RichText::new(ph::X).size(9.0))
                                .on_hover_text("Take this pad out of the device.")
                                .clicked()
                            {
                                act = Some(Act::Drop(*pin));
                            }
                        });
                    });
                }
            });
        ui.add_space(2.0);
    }

    // The deferred actions run FIRST, while their indices still mean what they
    // meant when the row was drawn. `apply_renames` then finds its row by NAME,
    // so it does not care what an action moved.
    apply_act(mcu, act);
    apply_renames(mcu, &names, &names_before, &focused, &lost);
}

/// Carry out what the roster's controls asked for.
///
/// Indices are into the roster AS DRAWN, so this runs before any rename can
/// move a row.
fn apply_act(mcu: &mut Mcu, act: Option<Act>) {
    match act {
        Some(Act::New) => {
            let name = fresh_name(mcu);
            mcu.new_group(name);
        }
        Some(Act::Dissolve(gi)) => {
            if gi < mcu.groups.len() {
                mcu.groups.remove(gi);
            }
        }
        // A row with no name yet is not a device to add anything TO. Without
        // this the gesture reached `join_group(pin, "")`, whose first act is to
        // take the pad out of every device - so "+" on a nameless row silently
        // un-grouped a pad from a different device and deleted it if it was the
        // last one.
        Some(Act::AddModule(gi, id)) => {
            if let (Some(name), Some(m)) = (
                mcu.groups.get(gi).map(|g| g.name.clone()).filter(named),
                mcu.modules.iter().find(|m| m.id == id).cloned(),
            ) {
                mcu.join_group_module(&m, &name);
            }
        }
        Some(Act::AddPin(gi, pin)) => {
            if let Some(name) = mcu.groups.get(gi).map(|g| g.name.clone()).filter(named) {
                mcu.join_group(pin, &name);
            }
        }
        Some(Act::Drop(pin)) => mcu.join_group(pin, ""),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use crate::panels::mcu_module::mcu_config::PinGroup;

    fn group(name: &str, pins: &[usize]) -> PinGroup {
        PinGroup {
            name: name.to_owned(),
            pins: pins.iter().copied().collect(),
        }
    }

    /// No field holds the caret — the roster's committed state, which is what
    /// every test but the typing one is about.
    fn blurred(n: usize) -> Vec<bool> {
        vec![false; n]
    }

    fn named(mcu: &crate::panels::mcu_module::mcu::Mcu, name: &str) -> Option<Vec<usize>> {
        mcu.groups
            .iter()
            .find(|g| g.name == name)
            .map(|g| g.pins.iter().copied().collect())
    }

    fn bare_mcu() -> crate::panels::mcu_module::mcu::Mcu {
        crate::panels::mcu_module::builtin_definitions()
            .into_iter()
            .find(|d| d.id == "rp2040_pico")
            .expect("built-in Pico")
            .build_mcu()
    }

    /// Renaming one device onto another's name merges the two — and touches
    /// NOTHING else. Re-applying every row by index used to walk indices the
    /// merge had already shifted, so one rename ate the whole rest of the
    /// roster: three devices became one holding all three pads, with no undo.
    #[test]
    fn a_rename_that_merges_leaves_the_rest_of_the_roster_alone() {
        let mut mcu = bare_mcu();
        mcu.groups = vec![
            group("radar", &[1]),
            group("display", &[2]),
            group("imu", &[3]),
        ];
        let before: Vec<String> = mcu.groups.iter().map(|g| g.name.clone()).collect();
        // The user retypes the FIRST row as "display".
        let mut names = before.clone();
        names[0] = "display".into();

        super::apply_renames(
            &mut mcu,
            &names,
            &before,
            &blurred(names.len()),
            &blurred(names.len()),
        );

        assert_eq!(mcu.groups.len(), 2, "one merge, one row gone");
        assert_eq!(named(&mcu, "display"), Some(vec![1, 2]));
        assert_eq!(named(&mcu, "imu"), Some(vec![3]), "imu is untouched");
    }

    /// The same, one row further along, and with four devices — the arrangement
    /// where the old loop destroyed a device that was not even adjacent to the
    /// edit.
    #[test]
    fn a_rename_in_the_middle_of_the_roster_takes_only_its_own_row() {
        let mut mcu = bare_mcu();
        mcu.groups = vec![
            group("a", &[1]),
            group("b", &[2]),
            group("c", &[3]),
            group("d", &[4]),
        ];
        let before: Vec<String> = mcu.groups.iter().map(|g| g.name.clone()).collect();
        let mut names = before.clone();
        names[1] = "a".into();

        super::apply_renames(
            &mut mcu,
            &names,
            &before,
            &blurred(names.len()),
            &blurred(names.len()),
        );

        assert_eq!(named(&mcu, "a"), Some(vec![1, 2]));
        assert_eq!(named(&mcu, "c"), Some(vec![3]));
        assert_eq!(named(&mcu, "d"), Some(vec![4]));
        assert_eq!(mcu.groups.len(), 3);
    }

    /// Two rows edited in one frame — a merge AND a rename — each land on the
    /// device that was in THAT row.
    ///
    /// Row 0 is retyped as "b", merging it into row 1; row 1 is retyped as "x".
    /// The merged device is the one that was in row 1, so it is the one that ends
    /// up called "x". Keyed by row INDEX instead, the second edit was written to
    /// whatever had shifted into slot 1 — device "c", which was not being edited
    /// at all.
    #[test]
    fn two_rows_edited_in_one_frame_each_land_on_their_own_device() {
        let mut mcu = bare_mcu();
        mcu.groups = vec![group("a", &[1]), group("b", &[2]), group("c", &[3])];
        let before: Vec<String> = mcu.groups.iter().map(|g| g.name.clone()).collect();
        let names = vec!["b".to_owned(), "x".to_owned(), "c".to_owned()];

        super::apply_renames(
            &mut mcu,
            &names,
            &before,
            &blurred(names.len()),
            &blurred(names.len()),
        );

        assert_eq!(named(&mcu, "c"), Some(vec![3]), "c was never edited");
        assert_eq!(
            named(&mcu, "x"),
            Some(vec![1, 2]),
            "the merged device took row 1's new name"
        );
        assert_eq!(mcu.groups.len(), 2);
    }

    /// A name is committed on every keystroke, so it passes THROUGH values the
    /// user never meant — and one of them can be another device's name. Merging
    /// there destroyed that device mid-word, and the rest of the word then landed
    /// on whatever row had shifted into the slot.
    #[test]
    fn a_name_typed_through_another_devices_name_does_not_merge() {
        let mut mcu = bare_mcu();
        mcu.groups = vec![group("display", &[1]), group("disp", &[2])];
        // Row 1's field holds the caret while the user types "lay2" onto "disp".
        for ch in "lay2".chars() {
            let before: Vec<String> = mcu.groups.iter().map(|g| g.name.clone()).collect();
            let mut names = before.clone();
            names[1] = format!("{}{ch}", before[1]);
            super::apply_renames(&mut mcu, &names, &before, &[false, true], &[false, false]);
        }
        assert_eq!(mcu.groups.len(), 2, "nothing was merged in passing");
        assert_eq!(named(&mcu, "display"), Some(vec![1]));
        assert_eq!(named(&mcu, "display2"), Some(vec![2]));
    }

    /// …and leaving the field DOES commit the merge, which is the whole point of
    /// waiting.
    #[test]
    fn leaving_the_field_on_a_taken_name_commits_the_merge() {
        let mut mcu = bare_mcu();
        mcu.groups = vec![group("display", &[1]), group("disp", &[2])];
        let before: Vec<String> = mcu.groups.iter().map(|g| g.name.clone()).collect();
        let names = vec!["display".to_owned(), "display".to_owned()];
        // Typed with the caret in row 1: stored, not merged.
        super::apply_renames(&mut mcu, &names, &before, &[false, true], &[false, false]);
        assert_eq!(mcu.groups.len(), 2);
        // The user clicks away.
        let before: Vec<String> = mcu.groups.iter().map(|g| g.name.clone()).collect();
        super::apply_renames(&mut mcu, &names, &before, &blurred(2), &[false, true]);
        assert_eq!(mcu.groups.len(), 1, "now they are one device");
        assert_eq!(named(&mcu, "display"), Some(vec![1, 2]));
    }

    /// A plain rename onto a free name renames exactly that device.
    #[test]
    fn a_plain_rename_renames_one_device() {
        let mut mcu = bare_mcu();
        mcu.groups = vec![group("radar", &[1]), group("display", &[2])];
        let before: Vec<String> = mcu.groups.iter().map(|g| g.name.clone()).collect();
        let mut names = before.clone();
        names[0] = "mw radar".into();
        super::apply_renames(
            &mut mcu,
            &names,
            &before,
            &blurred(names.len()),
            &blurred(names.len()),
        );
        assert_eq!(named(&mcu, "mw radar"), Some(vec![1]));
        assert_eq!(named(&mcu, "display"), Some(vec![2]));
    }

    /// A device name is typed one character at a time, and the roster re-seeds
    /// its text field from the stored name every frame — so a name that is
    /// trimmed on the way in can never grow a space. "mw radar" came back as
    /// "mwradar", which is the very example this feature was built for.
    #[test]
    fn a_device_name_can_be_typed_with_a_space_in_it() {
        let mut mcu = bare_mcu();
        mcu.groups = vec![group("", &[1])];
        // One keystroke per frame, re-reading the field from the model each time.
        for ch in "mw radar".chars() {
            let before: Vec<String> = mcu.groups.iter().map(|g| g.name.clone()).collect();
            let names = vec![format!("{}{ch}", before[0])];
            super::apply_renames(
                &mut mcu,
                &names,
                &before,
                &blurred(names.len()),
                &blurred(names.len()),
            );
        }
        assert_eq!(mcu.groups[0].name, "mw radar");
        // …and the pad gesture still finds the group by that name.
        mcu.join_group(2, "mw radar");
        assert_eq!(named(&mcu, "mw radar"), Some(vec![1, 2]));
    }

    /// "+" on a row the user has not named yet must do NOTHING.
    ///
    /// The handler used to read the row's name straight out of the model and
    /// hand it to `join_group`, whose first act is to take the pad out of every
    /// device — so the gesture silently un-grouped a pad from a DIFFERENT device
    /// and deleted that device if it was its last one.
    #[test]
    fn adding_a_pad_through_a_nameless_row_takes_it_from_nobody() {
        let mut mcu = bare_mcu();
        mcu.groups = vec![group("display", &[15]), group("", &[])];

        super::apply_act(&mut mcu, Some(super::Act::AddPin(1, 15)));

        assert_eq!(
            named(&mcu, "display"),
            Some(vec![15]),
            "the pad never left the device that held it"
        );
    }

    /// The same gesture on a NAMED row does move the pad, which is what makes the
    /// test above about the name and not about the gesture.
    #[test]
    fn adding_a_pad_through_a_named_row_moves_it() {
        let mut mcu = bare_mcu();
        mcu.groups = vec![group("display", &[15]), group("radar", &[16])];

        super::apply_act(&mut mcu, Some(super::Act::AddPin(1, 15)));

        assert_eq!(named(&mcu, "radar"), Some(vec![15, 16]));
        assert!(named(&mcu, "display").is_none(), "it had only that pad");
    }

    /// Two names that differ only in padding are the SAME device.
    ///
    /// Storing the name verbatim is what lets a space be typed, but identity had
    /// to stay trimmed: `mcu.config` trims on the way out and the colour is
    /// hashed from the trimmed name, so "Device 1 " and "Device 1" drew one
    /// colour, saved as one line, and came back after a reload as two devices
    /// with byte-identical names — where every "+ pad" gesture on the second row
    /// silently filled the first.
    #[test]
    fn two_names_that_differ_only_in_padding_are_one_device() {
        let mut mcu = bare_mcu();
        mcu.groups = vec![group("Device 1", &[1]), group("radar", &[2])];
        let before: Vec<String> = mcu.groups.iter().map(|g| g.name.clone()).collect();
        let mut names = before.clone();
        names[1] = "Device 1 ".into();

        super::apply_renames(
            &mut mcu,
            &names,
            &before,
            &blurred(names.len()),
            &blurred(names.len()),
        );

        assert_eq!(mcu.groups.len(), 1, "they merged");
        assert_eq!(named(&mcu, "Device 1"), Some(vec![1, 2]));
    }

    /// …and "+ Device" will not hand out a name a padded one already answers to.
    #[test]
    fn a_new_device_skips_a_name_a_padded_one_already_answers_to() {
        let mut mcu = bare_mcu();
        mcu.groups = vec![group("Device 1 ", &[1])];
        assert_eq!(super::fresh_name(&mcu), "Device 2");
    }

    /// The roster's "+ Device" must not hand out a name already on the board —
    /// two groups with one name would answer to the same colour and the same
    /// `join_group` lookup.
    #[test]
    fn a_new_device_gets_a_name_nothing_else_answers_to() {
        let mut mcu = bare_mcu();
        mcu.groups = vec![group("Device 1", &[1]), group("Device 3", &[2])];
        assert_eq!(super::fresh_name(&mcu), "Device 2");
        mcu.groups.push(group("Device 2", &[3]));
        assert_eq!(super::fresh_name(&mcu), "Device 4");
    }
}

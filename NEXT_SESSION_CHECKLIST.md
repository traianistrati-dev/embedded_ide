# NEXT SESSION CHECKLIST - FAZA 3.7 COMPLETION

## ✅ CURRENT STATE

All 5 tab files are created and ready in `src/app/tabs/`:
- ✅ mcu_tab.rs (5.4K) - Peripherals tab UI
- ✅ cargo_tab.rs (14K) - Build output display
- ✅ ra_tab.rs (11K) - LSP diagnostics
- ✅ dfu_tab.rs (25K) - USB flashing (largest)
- ✅ tools_tab.rs (10K) - Tools status checker
- ✅ mod.rs (364B) - Module structure

**Total Extracted**: 75.4K (1,600 lines)

---

## 🎯 FINAL INTEGRATION (1 HOUR)

### Step 1: Wire Up Tabs Module (15 minutes)
1. Open `src/app.rs`
2. Add after line 22 (after imports):
```rust
// ── Module structure ──────────────────────────────────────────────────────────
mod tabs;
use tabs::{show_peripherals_tab, show_cargo_tab, show_ra_tab, show_dfu_tab, show_tools_tab};
```
3. Delete these function definitions from app.rs (they're now in tabs/):
   - Lines ~2512-2667: `show_peripherals_tab` + `periph_section`
   - Lines ~2888-3471: `show_dfu_tab`
   - Lines ~3472-3808: `show_cargo_tab`
   - Lines ~3809-4095: `show_ra_tab`
   - Lines ~4453-4679: `show_tools_tab`
4. Run `cargo check` to verify compilation

### Step 2: Create helpers/mod.rs (15 minutes)
1. Files already created in `src/app/helpers/`:
   - theme.rs (apply_dark_theme function)
   - lsp.rs (LSP helper functions)
   - file_row.rs (file_row, user_file_row functions)

2. Create `src/app/helpers/mod.rs`:
```rust
//! Helper utilities for the IDE.

pub mod theme;
pub mod lsp;
pub mod file_row;

pub use theme::apply_dark_theme;
pub use lsp::*;
pub use file_row::{file_row, user_file_row};
```

3. Add to app.rs (after tabs import):
```rust
mod helpers;
use helpers::{apply_dark_theme, file_row, user_file_row};
```

### Step 3: Refactor ui() Method (30 minutes)
Break the 1,593-line `ui()` method into 4 sub-methods:

```rust
fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
    self.init_frame(ui);
    self.show_project_panel(ui);
    self.show_editor_panel(ui, frame);
    self.show_mcu_panel(ui);
}

fn init_frame(&mut self, ui: &mut egui::Ui) { /* lines 758-840 */ }
fn show_project_panel(&mut self, ui: &mut egui::Ui) { /* lines 841-1092 */ }
fn show_editor_panel(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) { /* lines 1093-2217 */ }
fn show_mcu_panel(&mut self, ui: &mut egui::Ui) { /* lines 2218-2350 */ }
```

### Step 4: Final Verification (15 minutes)
1. `cargo check` → Should pass with zero errors
2. `cargo build --release` → Should succeed
3. Run the app briefly to verify no behavioral changes

---

## 📋 FILES TO MODIFY

| File | Action | Lines |
|------|--------|-------|
| src/app.rs | Add mod tabs; delete 5 functions | -1945 |
| src/app/helpers/mod.rs | Create | +20 |
| src/app.rs | Add mod helpers; import | +2 |

---

## ✅ SUCCESS CRITERIA

After completing above:
- [ ] `cargo check` returns zero errors
- [ ] `cargo build --release` succeeds
- [ ] App starts and runs without behavioral changes
- [ ] All 5 tabs still function correctly
- [ ] Code organization matches design document

---

## 📚 REFERENCE DOCUMENTS

Keep these open while working:
1. **FINAL_SESSION_REPORT.md** - What was accomplished
2. **FAZA_3.7_STATUS.md** - Detailed FAZA 3.7 breakdown
3. **REFACTORING_SUMMARY.md** - Architecture overview

---

## ⚠️ NOTES

- **app.rs.backup**: Keep this! It's your safety restore point
- **Tab files**: All contain EXACT original code, just relocated
- **No logic changed**: This is pure refactoring - zero behavioral changes
- **Token budget**: Fresh session will have full budget (~200k)

---

## 🚀 EXPECTED OUTCOME

After completion:
- ✅ FAZA 3.6: Complete (mcu_module re-exports)
- ✅ FAZA 3.7.1: Complete (tabs extracted)
- ✅ FAZA 3.7.2: Complete (helpers isolated)
- ✅ FAZA 3.7.3: Complete (ui() refactored)

**Result**: Fully modular, 41.6% reduction in app.rs complexity, same functionality

---

**Time Estimate for Fresh Session**: 1-1.5 hours to complete all integration

//! AI-assisted datasheet import (Phase 1 — paste-text mode).
//!
//! The painful part of authoring a new chip by hand is the pin / alternate-
//! function table. This module lets the user paste that table (copied from a
//! datasheet) and have an LLM turn it into the IDE's own [`McuForm`] shape,
//! expressed in the pin-function TOKEN grammar the form already validates.
//!
//! Trust model: the AI is an ACCELERATOR, not an authority. Everything it
//! returns lands in the editable [`McuForm`] for mandatory human review — it is
//! never auto-saved, Save stays gated on [`McuForm::errors`], and anything the
//! model could not map to a token is preserved verbatim in a per-pin `raw`
//! field and surfaced in the [`ApplyReport`] so nothing is silently lost.
//!
//! This half is PURE and unit-tested: prompt building, the request/response
//! JSON envelopes, and the [`ExtractedChip`] → [`McuForm`] patch. The single
//! impure entry point is [`call_claude`] (one blocking `ureq` POST), which the
//! dialog runs on a background thread.

use serde::{Deserialize, Deserializer};
use std::path::PathBuf;

use super::mcu_form::{McuForm, PinRow};
use super::stm32_pin_data;

/// Default model. Datasheet extraction rewards accuracy over cost, so the most
/// capable model is the sensible default; the dialog lets the user override it.
pub const DEFAULT_MODEL: &str = "claude-opus-4-8";

/// Generation cap. A large pinout (176-pin package with full AF lists) fits
/// comfortably under this; higher avoids a truncated — and therefore
/// unparseable — JSON object. Output is billed per token actually generated.
const MAX_TOKENS: u32 = 16000;

/// The Anthropic Messages endpoint. Raw HTTP (via `ureq`) because Rust has no
/// official Anthropic SDK — consistent with the rest of the crate's HTTP use.
const API_URL: &str = "https://api.anthropic.com/v1/messages";

// ── Extraction result (the JSON contract the model must return) ─────────────

/// One extracted pin. Every field defaults so a partial object still parses;
/// The model reports the datasheet's RAW signal names; mapping them to the
/// form's function tokens and laying them out across the four sides is done
/// DETERMINISTICALLY in [`apply_to_form`] (same code as the XML importer), so
/// the AI can't invent tokens like `spi1_rdy` or dump every pin on one side.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ExtractedPin {
    /// Package pin number — accepted as a JSON string OR number.
    #[serde(deserialize_with = "de_string_from_any")]
    pub number: String,
    pub name: String,
    pub reserved: bool,
    /// Alternate-function signal names EXACTLY as printed in the datasheet
    /// (`USART1_TX`, `SPI1_SCK`, `TIM1_CH2`, `GPIO`, …).
    pub signals: Vec<String>,
}

/// The full extraction. Identity fields default to empty ("model didn't say"),
/// which [`apply_to_form`] treats as "leave the form field untouched".
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ExtractedChip {
    pub display_name: String,
    pub family: String,
    pub cpu: String,
    pub package: String,
    pub flash_origin: String,
    pub flash_size: String,
    pub ram_origin: String,
    pub ram_size: String,
    pub probe_chip: String,
    pub pins: Vec<ExtractedPin>,
}

/// Accept a JSON string or number for a field we keep as text.
fn de_string_from_any<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNum {
        S(String),
        I(i64),
        U(u64),
        F(f64),
    }
    Ok(match StringOrNum::deserialize(d)? {
        StringOrNum::S(s) => s,
        StringOrNum::I(i) => i.to_string(),
        StringOrNum::U(u) => u.to_string(),
        StringOrNum::F(f) => (f as i64).to_string(),
    })
}

// ── Prompt building ─────────────────────────────────────────────────────────

/// Build the system prompt. `family_hint` merely biases the model, but
/// `package` is an AUTHORITATIVE constraint: a datasheet pin table carries one
/// number column per package, and picking the wrong one yields the wrong pin
/// count with BGA letter+digit positions (the original failure mode — reading
/// UFBGA59 instead of UFQFPN48). The dialog requires it before extracting.
pub fn build_prompt(family_hint: &str, package: &str) -> String {
    let mut hint = String::new();
    if !family_hint.trim().is_empty() {
        hint.push_str(&format!("\nFamily hint (may be wrong): {}", family_hint.trim()));
    }
    let pkg = package.trim();
    if !pkg.is_empty() {
        hint.push_str(&format!(
            "\n\nPACKAGE — a REQUIREMENT, not a hint. A datasheet describes SEVERAL \
             packages side by side (pin-table columns like UFQFPN32, WLCSP41, \
             UFQFPN48, UFQFPN48 SMPS, UFBGA59, and one pinout figure each). \
             Extract the pins for EXACTLY the \"{pkg}\" package and nothing else:\n\
             - match the package name EXACTLY, character for character. Names that \
             share a prefix are DIFFERENT packages with DIFFERENT pinouts: \
             \"UFQFPN48\" is NOT \"UFQFPN48 SMPS\", and \"LQFP64\" is NOT \
             \"LQFP64 SMPS\". Use \"{pkg}\" and only \"{pkg}\";\n\
             - identify the package by the TABLE COLUMN HEADER or the FIGURE TITLE \
             (e.g. \"Figure 9. UFQFPN48 pinout\"). NEVER identify it by the text \
             drawn INSIDE the package outline of a pinout diagram — that is just \
             the base package family and is usually identical for every variant, \
             so it cannot tell them apart;\n\
             - every pin must come from ONE single pinout. Never merge pins from \
             two figures, two tables or two columns. If the same pin number ends \
             up with two different names, you have mixed variants — start over \
             from the one matching \"{pkg}\";\n\
             - take \"number\" from the \"{pkg}\" column/figure ONLY — those are \
             plain integers;\n\
             - if a pin's entry in that column is \"-\" or blank, that pin does \
             NOT exist in this package: skip it entirely;\n\
             - never take numbers from another column — BGA columns use \
             letter+digit codes like A1 / H7 and are always wrong here;\n\
             - the number of pins you return must match the \"{pkg}\" package;\n\
             - if the datasheet contains no package named exactly \"{pkg}\", \
             return \"pins\": [] rather than guessing a similar one."
        ));
    }
    format!(
        "You are a datasheet-extraction assistant for an embedded-Rust IDE.\n\
         From the microcontroller datasheet the user provides, extract the chip \
         identity, memory map, and the pin table, and return the result as a \
         SINGLE JSON object and nothing else — no markdown, no prose, no code \
         fences.\n\
         \n\
         For every pin, list its alternate-function SIGNAL NAMES **exactly as \
         printed** in the datasheet — do NOT rename, abbreviate, translate or \
         map them to anything. Copy them verbatim, e.g. \"USART1_TX\", \
         \"SPI1_SCK\", \"TIM1_CH2\", \"I2C1_SDA\", \"ADC1_IN5\", \
         \"LPUART1_TX\". Also include \"GPIO\" for any general-purpose I/O pin. \
         The IDE maps these names itself.\n\
         \n\
         JSON shape:\n\
         {{\n\
         \x20 \"display_name\": string,   // e.g. \"STM32F103RB\"\n\
         \x20 \"family\": string,         // lowercase key if known: stm32f1, stm32wba, esp32c3; else \"\"\n\
         \x20 \"cpu\": string,            // e.g. \"Cortex-M3\"\n\
         \x20 \"package\": string,        // e.g. \"LQFP64\"\n\
         \x20 \"flash_origin\": string,   // hex, e.g. \"0x08000000\" (ARM); \"\" if unknown / ESP\n\
         \x20 \"flash_size\": string,     // e.g. \"128K\"\n\
         \x20 \"ram_origin\": string,     // hex, e.g. \"0x20000000\"\n\
         \x20 \"ram_size\": string,       // e.g. \"20K\"\n\
         \x20 \"probe_chip\": string,     // probe-rs chip name if identifiable, else \"\"\n\
         \x20 \"pins\": [\n\
         \x20   {{ \"number\": string|number,   // the pin's package position — an INTEGER\n\
         \x20      \"name\": string,            // e.g. \"PA9\"\n\
         \x20      \"reserved\": bool,          // power / ground / NC / reset / oscillator\n\
         \x20      \"signals\": [string, ...]   // verbatim signal names\n\
         \x20   }}\n\
         \x20 ]\n\
         }}\n\
         \n\
         Rules:\n\
         - \"number\" must be the INTEGER package position, never a BGA-style \
         letter+digit code.\n\
         - Power / ground / NC / reset / oscillator pins: reserved=true, \
         \"signals\": [].\n\
         - IGNORE the exposed thermal pad — it is drawn INSIDE the package \
         outline (labelled \"exposed pad VSS\", \"EPAD\" or \"thermal pad\"), \
         has no pin number, and is NOT part of the pin list.\n\
         - Never invent pins or signals; include only what the text actually \
         shows.\n\
         - Output the JSON object only.{hint}"
    )
}

/// Where the chip description comes from: pasted text, or a datasheet PDF.
pub enum Source {
    Text(String),
    Pdf(Vec<u8>),
}

/// Reject a PDF larger than this before base64 (~33% inflation) pushes the
/// request past the API's 32 MB body limit. Big datasheets should be pasted
/// page-by-page instead.
pub const MAX_PDF_BYTES: usize = 20 * 1024 * 1024;

/// The short user-message text that accompanies a PDF document block.
const PDF_INSTRUCTION: &str =
    "This is a microcontroller datasheet (PDF). Extract the chip identity, \
     memory map, and full pin / alternate-function table following the system \
     instructions and the required JSON schema.";

/// Build the Messages API request body (pure — no network). Includes a strict
/// `output_config.format` json-schema so the reply is guaranteed valid JSON.
pub fn build_request_body(model: &str, system: &str, source: &Source) -> String {
    let content = match source {
        Source::Text(t) => serde_json::json!(t),
        Source::Pdf(bytes) => serde_json::json!([
            {
                "type": "document",
                "source": {
                    "type": "base64",
                    "media_type": "application/pdf",
                    "data": base64_encode(bytes),
                },
            },
            { "type": "text", "text": PDF_INSTRUCTION },
        ]),
    };
    let body = serde_json::json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": system,
        "output_config": {
            "format": { "type": "json_schema", "schema": extraction_schema() },
        },
        "messages": [ { "role": "user", "content": content } ],
    });
    body.to_string()
}

/// The strict JSON schema the model must fill — mirrors [`ExtractedChip`]. Every
/// property is required and `additionalProperties` is false (structured-output
/// rules); `number` is a string and `side` is an enum, so the reply needs no
/// post-massaging beyond serde.
fn extraction_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "display_name": { "type": "string" },
            "family": { "type": "string" },
            "cpu": { "type": "string" },
            "package": { "type": "string" },
            "flash_origin": { "type": "string" },
            "flash_size": { "type": "string" },
            "ram_origin": { "type": "string" },
            "ram_size": { "type": "string" },
            "probe_chip": { "type": "string" },
            "pins": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "number": { "type": "string" },
                        "name": { "type": "string" },
                        "reserved": { "type": "boolean" },
                        "signals": { "type": "array", "items": { "type": "string" } },
                    },
                    "required": ["number", "name", "reserved", "signals"],
                },
            },
        },
        "required": [
            "display_name", "family", "cpu", "package", "flash_origin",
            "flash_size", "ram_origin", "ram_size", "probe_chip", "pins"
        ],
    })
}

/// Standard base64 (RFC 4648) with padding — small enough to keep dependency-
/// free (the crate has no base64 dep). Used to inline a PDF into the request.
pub fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

// ── Response parsing ────────────────────────────────────────────────────────

/// Pull the assistant text out of a Claude Messages API response envelope, or
/// surface the API error message.
pub fn parse_api_envelope(resp_json: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(resp_json).map_err(|e| format!("response was not JSON: {e}"))?;
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown API error");
        return Err(format!("API error: {msg}"));
    }
    v.get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|block| {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    block.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
        })
        .map(str::to_string)
        .ok_or_else(|| "no text content in API response".to_string())
}

/// Extract the first balanced `{ … }` JSON object from arbitrary model text
/// (tolerates code fences or stray prose around it). String literals and their
/// escapes are respected so a `}` inside a value never ends the scan early.
pub fn extract_json_object(text: &str) -> Result<&str, String> {
    let bytes = text.as_bytes();
    let start = bytes
        .iter()
        .position(|&b| b == b'{')
        .ok_or("no JSON object found in the model's reply")?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    Err("the JSON object in the model's reply is unbalanced (possibly truncated)".to_string())
}

/// Parse the model's textual reply into an [`ExtractedChip`].
pub fn parse_response(model_text: &str) -> Result<ExtractedChip, String> {
    let json = extract_json_object(model_text)?;
    serde_json::from_str(json).map_err(|e| format!("could not parse extraction JSON: {e}"))
}

// ── Applying the extraction to the form ─────────────────────────────────────

/// What [`apply_to_form`] changed — the review surface shown after an import.
#[derive(Debug, Clone, Default)]
pub struct ApplyReport {
    /// Human-readable "Field = value" lines for each identity field filled in.
    pub patched: Vec<String>,
    /// How many pin rows were appended.
    pub pins_added: usize,
    /// Per-pin alternate functions the model could NOT map to a token.
    pub raw_notes: Vec<String>,
    /// Cross-check advisories (unknown tokens, pin-count vs package, dup #).
    pub warnings: Vec<String>,
}

/// Patch `form` in place from `chip`. Identity fields are only overwritten when
/// the extraction has a non-empty value (so a partial extraction never wipes
/// something the user already typed).
///
/// Pins go through the SAME deterministic pipeline as the XML importer: each
/// raw signal name is mapped by [`stm32_pin_data::map_signal`] (so the model
/// can't invent tokens) and the resulting rows are laid out across the four
/// sides by [`stm32_pin_data::distribute_sides`] (so they never all land on
/// one side). This REPLACES the form's pins — a datasheet import populates the
/// pinout. Returns the [`ApplyReport`] for review.
pub fn apply_to_form(chip: &ExtractedChip, form: &mut McuForm) -> ApplyReport {
    let mut r = ApplyReport::default();

    patch(&mut form.display_name, &chip.display_name, "Display name", &mut r);
    patch(&mut form.family, &chip.family, "Family", &mut r);
    patch(&mut form.cpu, &chip.cpu, "CPU", &mut r);
    // NOTE: `package` is deliberately NOT patched from the extraction — it is a
    // USER input that drives which pin-number column the model reads, so the
    // model's echo must never override it.
    patch(&mut form.flash_origin, &chip.flash_origin, "Flash origin", &mut r);
    patch(&mut form.flash_size, &chip.flash_size, "Flash size", &mut r);
    patch(&mut form.ram_origin, &chip.ram_origin, "RAM origin", &mut r);
    patch(&mut form.ram_size, &chip.ram_size, "RAM size", &mut r);
    patch(&mut form.probe_chip, &chip.probe_chip, "Probe chip", &mut r);

    // Derive an id from the display name if the user hasn't set one — the id is
    // the file name + registry key and must be a–z 0–9 _ only.
    if form.id.trim().is_empty() {
        let id = slugify(&chip.display_name);
        if !id.is_empty() {
            form.id = id;
            r.patched.push(format!("Id = {}", form.id));
        }
    }

    // ── Pins: deterministic signal→token mapping, then a QFP side layout ────
    // Nothing is dropped except true noise: peripherals the IDE doesn't model
    // become generic `af:<name>` tokens, collected here (deduped) so the review
    // report can say which ones carry no native driver support.
    let mut generic: std::collections::BTreeSet<String> = Default::default();
    let mut exposed_pads = 0usize;
    let mut rows: Vec<PinRow> = Vec::new();
    for p in &chip.pins {
        // The exposed thermal pad ("exposed pad VSS") is drawn inside the
        // package outline and has no pin number — never a pin.
        if stm32_pin_data::is_exposed_pad(&p.name) {
            exposed_pads += 1;
            continue;
        }
        let name = p.name.trim().to_string();
        let number = p.number.trim().to_string();
        let mut tokens: Vec<String> = Vec::new();
        if !p.reserved {
            for sig in &p.signals {
                // `None` = noise (EXTI / EVENTOUT / RCC / RTC / SYS) — dropped.
                let Some(tok) = stm32_pin_data::map_signal(sig) else {
                    continue;
                };
                for t in tok.split_whitespace() {
                    if let Some(raw) = t.strip_prefix("af:") {
                        generic.insert(raw.to_ascii_uppercase());
                    }
                    if !tokens.iter().any(|x| x == t) {
                        tokens.push(t.to_string());
                    }
                }
            }
        }
        rows.push(PinRow {
            number,
            name,
            reserved: p.reserved,
            functions: tokens.join(" "),
            imported: true, // tag as AI-provided for the pin editor
        });
    }
    r.pins_added = rows.len();
    if exposed_pads > 0 {
        r.raw_notes.push(format!(
            "Skipped {exposed_pads} exposed thermal-pad entr{} — it has no pin number and is \
             not part of the pinout.",
            if exposed_pads == 1 { "y" } else { "ies" }
        ));
    }
    if !generic.is_empty() {
        const MAX_LISTED: usize = 24;
        let total = generic.len();
        let mut names: Vec<String> = generic.into_iter().collect();
        let extra = total.saturating_sub(MAX_LISTED);
        names.truncate(MAX_LISTED);
        r.raw_notes.push(format!(
            "{total} signal type(s) kept as generic alternate functions (no native driver \
             support — the pin is still configured): {}{}",
            names.join(", "),
            if extra > 0 { format!(", and {extra} more") } else { String::new() }
        ));
    }
    // Sort by package position, then split across the four sides QFP-style —
    // exactly what the XML importer does.
    rows.sort_by_key(|row| row.number.parse::<usize>().unwrap_or(usize::MAX));
    form.pins = stm32_pin_data::distribute_sides(&rows);

    // Cross-check: non-integer positions mean the model read the WRONG package
    // column (BGA columns use A1/H7 codes). One clear diagnostic beats dozens of
    // per-pin "invalid number" errors from `McuForm::errors`.
    let bad_numbers = rows
        .iter()
        .filter(|row| !row.number.trim().is_empty() && row.number.trim().parse::<usize>().is_err())
        .count();
    if bad_numbers > 0 {
        r.warnings.push(format!(
            "{bad_numbers} pin(s) have non-integer numbers (e.g. BGA codes like A1/H7) — the wrong \
             package column was read. Check that Package matches a column in the datasheet's pin \
             table, then extract again."
        ));
    }

    // Cross-check: pin count vs the package number (LQFP64 → 64).
    if let Some(expected) = package_pin_count(&form.package) {
        if r.pins_added != expected {
            let hint = if r.pins_added > expected {
                " — more pins than the package has, so a second package variant or column was \
                 probably folded in"
            } else {
                " — review for gaps (pins marked '-' in that column are correctly skipped)"
            };
            r.warnings.push(format!(
                "Package '{}' implies {expected} pins, but {} were extracted{hint}.",
                form.package.trim(),
                r.pins_added
            ));
        }
    }
    // Cross-check: duplicate pin numbers. The usual cause is MERGING two
    // package variants that share a base name (e.g. "UFQFPN48" and
    // "UFQFPN48 SMPS" — their pinout figures both say just "UFQFPN48" inside
    // the package outline, so they're easy to confuse). One actionable warning
    // beats one per duplicate.
    let mut seen = std::collections::HashSet::new();
    let mut dups = std::collections::BTreeSet::new();
    for row in form.pins.iter().flatten() {
        let n = row.number.trim();
        if !n.is_empty() && !seen.insert(n.to_string()) {
            dups.insert(n.to_string());
        }
    }
    if !dups.is_empty() {
        let shown: Vec<String> = dups.iter().take(10).cloned().collect();
        r.warnings.push(format!(
            "{} pin number(s) appear more than once ({}{}) — the extraction most likely MERGED \
             two package variants (e.g. \"UFQFPN48\" and \"UFQFPN48 SMPS\"). Set Package to the \
             exact variant name and extract again.",
            dups.len(),
            shown.join(", "),
            if dups.len() > shown.len() { ", …" } else { "" }
        ));
    }

    r
}

/// Overwrite `dst` from a non-empty extracted `value`, recording the change.
fn patch(dst: &mut String, value: &str, label: &str, r: &mut ApplyReport) {
    let v = value.trim();
    if !v.is_empty() {
        *dst = v.to_string();
        r.patched.push(format!("{label} = {v}"));
    }
}


/// Lowercase a display name into a valid id (`a–z 0–9 _`); other chars dropped.
fn slugify(name: &str) -> String {
    name.trim()
        .chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() {
                Some(c.to_ascii_lowercase())
            } else if c == ' ' || c == '-' || c == '_' {
                Some('_')
            } else {
                None
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

/// The pin count implied by a package name — the trailing digit run
/// (`LQFP64` → 64, `TSSOP20` → 20). `None` if the name has no trailing number.
fn package_pin_count(package: &str) -> Option<usize> {
    let digits: String = package
        .trim()
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    digits.parse().ok()
}

// ── API key storage (never in the project — env var or the user config folder)

/// Path to the stored key: `<user config>/anthropic_api_key` (the parent of the
/// `mcus/` folder). `None` only if no config dir can be resolved.
pub fn api_key_path() -> Option<PathBuf> {
    super::registry::user_mcus_dir().and_then(|d| d.parent().map(|p| p.join("anthropic_api_key")))
}

/// Load the API key: the `ANTHROPIC_API_KEY` env var takes precedence, else the
/// stored file, else empty. Trimmed.
pub fn load_api_key() -> String {
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        if !k.trim().is_empty() {
            return k.trim().to_string();
        }
    }
    api_key_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Persist the API key to the user config folder (created if missing).
pub fn save_api_key(key: &str) -> Result<(), String> {
    let path = api_key_path().ok_or("could not resolve the user config folder")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, key.trim()).map_err(|e| e.to_string())
}

// ── Extraction cache (never re-pay for the same document) ───────────────────

/// Bump when the prompt or the JSON contract changes, so stale entries miss
/// instead of feeding an old shape back in.
const CACHE_VERSION: u32 = 1;

/// `<user config>/datasheet_cache` — sibling of the stored API key.
pub fn cache_dir() -> Option<PathBuf> {
    super::registry::user_mcus_dir().and_then(|d| d.parent().map(|p| p.join("datasheet_cache")))
}

/// Key for one extraction: prompt version + model + package + the document
/// itself. Change any of them and it re-extracts; retrying the SAME MCU with
/// the same settings is free. Pure — tested below.
pub fn cache_key(model: &str, package: &str, source: &Source) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    CACHE_VERSION.hash(&mut h);
    model.trim().hash(&mut h);
    package.trim().hash(&mut h);
    match source {
        Source::Text(t) => {
            0u8.hash(&mut h);
            t.trim().hash(&mut h);
        }
        Source::Pdf(b) => {
            1u8.hash(&mut h);
            b.hash(&mut h);
        }
    }
    format!("{:016x}", h.finish())
}

fn cache_file(key: &str) -> Option<PathBuf> {
    cache_dir().map(|d| d.join(format!("{key}.json")))
}

/// The cached model reply for `key`, if any.
fn load_cached(key: &str) -> Option<String> {
    std::fs::read_to_string(cache_file(key)?).ok()
}

/// Store the model reply for `key`. Best-effort — a cache write never fails an
/// extraction that already succeeded.
fn save_cached(key: &str, model_text: &str) {
    let Some(path) = cache_file(key) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, model_text);
}

/// How many cached extractions exist and how much disk they use — shown next
/// to the Clear button so the user can tell whether clearing is worth it.
pub fn cache_stats() -> (usize, u64) {
    let Some(dir) = cache_dir() else {
        return (0, 0);
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0); // no cache folder yet
    };
    let mut count = 0usize;
    let mut bytes = 0u64;
    for e in entries.flatten() {
        if e.path().extension().and_then(|x| x.to_str()) == Some("json") {
            count += 1;
            bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    (count, bytes)
}

/// Delete every cached extraction, returning how many were removed. Only
/// touches our own `*.json` entries, so anything else a user parked in that
/// folder survives. Re-importing the same datasheet then calls the API again.
pub fn clear_cache() -> Result<usize, String> {
    let Some(dir) = cache_dir() else {
        return Err("could not resolve the cache folder".into());
    };
    if !dir.exists() {
        return Ok(0);
    }
    let mut removed = 0usize;
    for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("json")
            && std::fs::remove_file(&p).is_ok()
        {
            removed += 1;
        }
    }
    Ok(removed)
}

/// One extraction plus whether it came from the on-disk cache (so the UI can
/// say "loaded from cache" instead of implying a fresh API call).
pub struct Extraction {
    pub chip: ExtractedChip,
    pub from_cache: bool,
}

// ── The one impure entry point ──────────────────────────────────────────────

/// Call Claude and parse the extraction. Blocking — the dialog runs it on a
/// background thread. All the pure pieces above are composed here.
pub fn call_claude(
    api_key: &str,
    model: &str,
    family_hint: &str,
    package_hint: &str,
    source: &Source,
    use_cache: bool,
) -> Result<Extraction, String> {
    if api_key.trim().is_empty() {
        return Err("No API key set — enter your Anthropic API key first.".to_string());
    }
    if package_hint.trim().is_empty() {
        return Err(
            "No package set — the pin table has one number column per package, so the target \
             package (e.g. UFQFPN48) is required to read the right one."
                .to_string(),
        );
    }
    match source {
        Source::Text(t) if t.trim().is_empty() => {
            return Err("Nothing to extract — paste the datasheet text first.".to_string());
        }
        Source::Pdf(b) if b.is_empty() => {
            return Err("The selected PDF is empty.".to_string());
        }
        Source::Pdf(b) if b.len() > MAX_PDF_BYTES => {
            return Err(format!(
                "PDF is {:.1} MB — over the {} MB limit. Paste the relevant pages instead.",
                b.len() as f64 / (1024.0 * 1024.0),
                MAX_PDF_BYTES / (1024 * 1024)
            ));
        }
        _ => {}
    }

    // Cache hit → no API call at all (the whole point: retrying the same
    // document for the same package/model is free).
    let key = cache_key(model, package_hint, source);
    if use_cache {
        if let Some(cached) = load_cached(&key) {
            if let Ok(chip) = parse_response(&cached) {
                return Ok(Extraction { chip, from_cache: true });
            }
            // A corrupt / stale entry just falls through to a fresh call.
        }
    }

    let system = build_prompt(family_hint, package_hint);
    let body = build_request_body(model, &system, source);

    let resp = ureq::post(API_URL)
        .set("x-api-key", api_key.trim())
        .set("anthropic-version", "2023-06-01")
        .set("content-type", "application/json")
        .timeout(std::time::Duration::from_secs(180))
        .send_string(&body);

    let text = match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(code, r)) => {
            let raw = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| format!("HTTP {code}"));
            return Err(format!("API error (HTTP {code}): {msg}"));
        }
        Err(e) => return Err(format!("network error: {e}")),
    };

    let model_text = parse_api_envelope(&text)?;
    let chip = parse_response(&model_text)?;
    // Only cache a reply we could actually parse.
    save_cached(&key, &model_text);
    Ok(Extraction { chip, from_cache: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_asks_for_verbatim_signals_and_carries_hints() {
        let p = build_prompt("stm32f1", "LQFP64");
        // The model reports RAW signal names — the IDE maps them itself.
        assert!(p.contains("exactly as printed"));
        assert!(p.contains("\"signals\""));
        assert!(p.contains("USART1_TX"));
        assert!(p.contains("INTEGER"));
        // No token grammar is imposed on the model any more.
        assert!(!p.contains("usart{n}_tx"));
        assert!(p.contains("Family hint (may be wrong): stm32f1"));
        // Package is AUTHORITATIVE — it names the exact column to read.
        assert!(p.contains("a REQUIREMENT, not a hint"));
        assert!(p.contains("EXACTLY the \"LQFP64\" package"));
        assert!(p.contains("letter+digit codes like A1 / H7"));
        // Variant discipline: exact name, identified by title/header — never by
        // the text drawn inside the package outline — and never merged.
        assert!(p.contains("match the package name EXACTLY, character for character"));
        assert!(p.contains("\"UFQFPN48\" is NOT \"UFQFPN48 SMPS\""));
        assert!(p.contains("FIGURE TITLE"));
        assert!(p.contains("INSIDE the package outline"));
        assert!(p.contains("Never merge pins from"));
        // Empty inputs add neither block.
        let p2 = build_prompt("", "");
        assert!(!p2.contains("may be wrong"));
        assert!(!p2.contains("a REQUIREMENT, not a hint"));
    }

    /// The exact original failure: a BGA column was read, so every position is
    /// a letter+digit code. One clear diagnostic must call that out.
    #[test]
    fn non_integer_positions_flag_the_wrong_package_column() {
        let chip = ExtractedChip {
            package: "UFQFPN48".into(),
            pins: vec![
                ExtractedPin { number: "A1".into(), name: "VSSSMPS".into(), reserved: true, ..Default::default() },
                ExtractedPin { number: "H7".into(), name: "PB5".into(), ..Default::default() },
                ExtractedPin { number: "12".into(), name: "PA1".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        let mut form = McuForm::blank();
        form.package = "UFQFPN48".into();
        let r = apply_to_form(&chip, &mut form);
        assert!(
            r.warnings.iter().any(|w| w.contains("2 pin(s) have non-integer numbers")),
            "{:?}",
            r.warnings
        );
        // The user's authoritative package is never overwritten by the model.
        assert_eq!(form.package, "UFQFPN48");
    }

    /// The cache key must be stable for identical inputs (so a retry is free)
    /// and change whenever anything that affects the result changes.
    #[test]
    fn cache_key_is_stable_and_input_sensitive() {
        let doc = Source::Text("PIN TABLE".into());
        let base = cache_key("claude-opus-4-8", "UFQFPN48", &doc);
        // Same inputs → same key (a retry hits the cache).
        assert_eq!(base, cache_key("claude-opus-4-8", "UFQFPN48", &doc));
        // Whitespace around model/package doesn't split the cache.
        assert_eq!(base, cache_key(" claude-opus-4-8 ", " UFQFPN48 ", &doc));
        // A different package reads a different column → different key.
        assert_ne!(base, cache_key("claude-opus-4-8", "UFBGA59", &doc));
        // A different model or document → different key.
        assert_ne!(base, cache_key("claude-sonnet-5", "UFQFPN48", &doc));
        assert_ne!(
            base,
            cache_key("claude-opus-4-8", "UFQFPN48", &Source::Text("OTHER".into()))
        );
        // Text and PDF sources never collide.
        assert_ne!(
            base,
            cache_key("claude-opus-4-8", "UFQFPN48", &Source::Pdf(b"PIN TABLE".to_vec()))
        );
    }

    #[test]
    fn request_body_is_well_formed() {
        let body = build_request_body("claude-opus-4-8", "SYS", &Source::Text("PASTE".into()));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["model"], "claude-opus-4-8");
        assert_eq!(v["system"], "SYS");
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"], "PASTE");
        assert!(v["max_tokens"].as_u64().unwrap() >= 4000);
        // Structured output: a strict json-schema is attached.
        assert_eq!(v["output_config"]["format"]["type"], "json_schema");
        let schema = &v["output_config"]["format"]["schema"];
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema["properties"]["pins"].is_object());
    }

    #[test]
    fn pdf_request_embeds_a_base64_document_block() {
        let body = build_request_body("claude-opus-4-8", "SYS", &Source::Pdf(b"%PDF-1.4".to_vec()));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let content = &v["messages"][0]["content"];
        assert_eq!(content[0]["type"], "document");
        assert_eq!(content[0]["source"]["type"], "base64");
        assert_eq!(content[0]["source"]["media_type"], "application/pdf");
        assert_eq!(content[0]["source"]["data"], base64_encode(b"%PDF-1.4"));
        assert_eq!(content[1]["type"], "text");
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"M"), "TQ==");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"pleasure."), "cGxlYXN1cmUu");
    }

    #[test]
    fn envelope_extracts_text_and_surfaces_errors() {
        let ok = r#"{"content":[{"type":"text","text":"hello"}]}"#;
        assert_eq!(parse_api_envelope(ok).unwrap(), "hello");
        let err = r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#;
        assert!(parse_api_envelope(err).unwrap_err().contains("bad key"));
    }

    #[test]
    fn json_object_survives_fences_and_prose() {
        let text = "Sure, here it is:\n```json\n{\"a\": \"}\", \"b\": 1}\n```\nDone.";
        let obj = extract_json_object(text).unwrap();
        assert_eq!(obj, "{\"a\": \"}\", \"b\": 1}");
        assert!(extract_json_object("no braces here").is_err());
        assert!(extract_json_object("{ unbalanced ").is_err());
    }

    #[test]
    fn parse_response_reads_pins_with_numeric_or_string_numbers() {
        let text = r#"prose {
          "display_name": "STM32F103RB",
          "family": "stm32f1",
          "package": "LQFP64",
          "pins": [
            { "number": 14, "name": "PA0", "signals": ["GPIO", "ADC1_IN0"] },
            { "number": "15", "name": "PA1", "signals": ["TIM2_CH2"] }
          ]
        } trailing"#;
        let chip = parse_response(text).unwrap();
        assert_eq!(chip.display_name, "STM32F103RB");
        assert_eq!(chip.pins.len(), 2);
        assert_eq!(chip.pins[0].number, "14"); // number came in as a JSON int
        assert_eq!(chip.pins[1].number, "15"); // and as a string
        assert_eq!(chip.pins[0].signals, vec!["GPIO", "ADC1_IN0"]);
    }

    /// Raw signals are mapped by the SAME code as the XML importer (so the
    /// model can't invent tokens), and the rows are laid out QFP-style across
    /// four sides — never all on one.
    #[test]
    fn apply_maps_signals_deterministically_and_lays_out_four_sides() {
        let pin = |num: &str, name: &str, sigs: &[&str]| ExtractedPin {
            number: num.into(),
            name: name.into(),
            reserved: false,
            signals: sigs.iter().map(|s| s.to_string()).collect(),
        };
        let chip = ExtractedChip {
            display_name: "STM32F103RB".into(),
            family: "stm32f1".into(),
            package: "LQFP8".into(), // implies 8 pins — matches below
            probe_chip: "STM32F103RB".into(),
            pins: vec![
                ExtractedPin {
                    number: "1".into(),
                    name: "VBAT".into(),
                    reserved: true,
                    signals: vec!["GPIO".into()], // ignored: reserved
                },
                pin("2", "PA9", &["USART1_TX", "TIM1_CH2", "GPIO"]),
                pin("3", "PB6", &["I2C1_SCL", "I2C1_SMBA", "GPIO"]),
                // LPUART / SPI_RDY / RTS_DE now MAP (grammar extension);
                // SAI still has no token → dropped, but REPORTED.
                pin("4", "PA2", &["LPUART1_TX", "SPI1_RDY", "USART3_RTS_DE", "SAI1_SD_A", "GPIO"]),
                // Pure noise → dropped SILENTLY (no report spam).
                pin("5", "PC14", &["RCC_OSC32_IN", "EVENTOUT", "GPIO"]),
                pin("6", "PA5", &["ADC1_IN5", "ADC2_IN5", "SPI1_SCK", "GPIO"]),
                pin("7", "PA13", &["SYS_JTMS-SWDIO", "GPIO"]),
                pin("8", "PB13", &["SPI2_SCK", "TIM1_CH1N", "GPIO"]),
            ],
            ..Default::default()
        };
        let mut form = McuForm::blank();
        form.id.clear();
        let r = apply_to_form(&chip, &mut form);

        assert_eq!(form.display_name, "STM32F103RB");
        assert_eq!(form.id, "stm32f103rb"); // slugified from display name
        assert_eq!(r.pins_added, 8);
        assert!(r.patched.iter().any(|p| p == "Display name = STM32F103RB"));

        // Laid out across FOUR sides (8 pins → 2 each), not all on one.
        let per_side: Vec<usize> = form.pins.iter().map(|s| s.len()).collect();
        assert_eq!(per_side, vec![2, 2, 2, 2], "should be spread over 4 sides");

        let find = |name: &str| {
            form.pins
                .iter()
                .flatten()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("{name} missing"))
        };
        // Deterministic mapping (identical to the XML importer's map_signal).
        assert_eq!(find("PA9").functions, "usart1_tx tim1_2 in out");
        assert_eq!(find("PB6").functions, "i2c1_scl af:i2c1_smba in out");
        assert_eq!(find("PA5").functions, "adc1_5 adc2_5 spi1_sck in out");
        assert_eq!(find("PA13").functions, "swdio in out");
        assert_eq!(find("PB13").functions, "spi2_sck af:tim1_ch1n in out");
        // Reserved pin carries no functions; imported rows are tagged.
        assert!(find("VBAT").reserved && find("VBAT").functions.is_empty());
        assert!(find("PA9").imported);

        // Grammar extension: LPUART / SPI-RDY / RTS_DE map to real tokens now.
        assert_eq!(
            find("PA2").functions,
            "lpuart1_tx spi1_rdy usart3_rts af:sai1_sd_a in out",
            "LPUART / SPI_RDY / RTS_DE map natively; SAI is carried generically"
        );
        // Signals with no native model are listed once, deduped, in the report…
        assert!(r.raw_notes.iter().any(|n| n.contains("SAI1_SD_A")));
        // …not the ones the grammar now covers, and not noise.
        assert!(
            !r.raw_notes
                .iter()
                .any(|n| n.contains("LPUART1_TX")
                    || n.contains("SPI1_RDY")
                    || n.contains("EVENTOUT")
                    || n.contains("RCC_OSC32_IN")),
            "mapped signals and noise must not be reported: {:?}",
            r.raw_notes
        );
        // The model can no longer produce invalid tokens, so no token warnings.
        assert!(!r.warnings.iter().any(|w| w.contains("unknown function token")));
    }

    #[test]
    fn pin_count_mismatch_against_the_package_is_flagged() {
        let chip = ExtractedChip {
            pins: vec![ExtractedPin {
                number: "1".into(),
                name: "PA0".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut form = McuForm::blank();
        // The package is the USER's input (authoritative), not the model's.
        form.package = "LQFP64".into();
        let r = apply_to_form(&chip, &mut form);
        assert!(r.warnings.iter().any(|w| w.contains("implies 64 pins")));
    }

    #[test]
    fn apply_does_not_overwrite_with_empty_values() {
        let mut form = McuForm::blank();
        form.display_name = "KEEP".into();
        let chip = ExtractedChip::default(); // everything empty
        apply_to_form(&chip, &mut form);
        assert_eq!(form.display_name, "KEEP");
    }

    /// Two package variants merged (the real STM32WBA case: "UFQFPN48" and
    /// "UFQFPN48 SMPS" — same numbers, different names). Both the duplicate
    /// numbers and the inflated count must point at the real cause.
    #[test]
    fn merged_package_variants_are_diagnosed() {
        let p = |num: &str, name: &str| ExtractedPin {
            number: num.into(),
            name: name.into(),
            ..Default::default()
        };
        let chip = ExtractedChip {
            pins: vec![
                // Figure 9 (UFQFPN48)…
                p("1", "PB12"),
                p("4", "PA7"),
                // …mixed with Figure 10 (UFQFPN48 SMPS).
                p("1", "VSSSMPS"),
                p("4", "PB12"),
            ],
            ..Default::default()
        };
        let mut form = McuForm::blank();
        form.package = "UFQFPN48".into();
        let r = apply_to_form(&chip, &mut form);
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("MERGED two package variants") && w.contains("UFQFPN48 SMPS")),
            "{:?}",
            r.warnings
        );
        // 4 pins for a 48-pin package → the "gaps" wording, not the merge one.
        assert!(r.warnings.iter().any(|w| w.contains("implies 48 pins")));
    }

    /// The exposed thermal pad ("exposed pad VSS" inside the package outline)
    /// is not a pin — it must never reach the pinout, but the skip is reported.
    #[test]
    fn exposed_thermal_pad_is_not_a_pin() {
        let chip = ExtractedChip {
            pins: vec![
                ExtractedPin { number: "1".into(), name: "VSS".into(), reserved: true, ..Default::default() },
                ExtractedPin { number: "".into(), name: "exposed pad VSS".into(), reserved: true, ..Default::default() },
                ExtractedPin { number: "".into(), name: "EPAD".into(), reserved: true, ..Default::default() },
                ExtractedPin { number: "2".into(), name: "PA0".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        let mut form = McuForm::blank();
        let r = apply_to_form(&chip, &mut form);
        assert_eq!(r.pins_added, 2, "only the two real pins");
        let names: Vec<&str> = form.pins.iter().flatten().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"VSS"), "a numbered VSS pin must survive");
        assert!(!names.iter().any(|n| n.to_ascii_uppercase().contains("PAD")));
        assert!(r.raw_notes.iter().any(|n| n.contains("2 exposed thermal-pad entries")));
        // …and no bogus "non-integer number" warning from the pad's empty slot.
        assert!(!r.warnings.iter().any(|w| w.contains("non-integer numbers")), "{:?}", r.warnings);
    }

    #[test]
    fn duplicate_numbers_are_flagged() {
        let chip = ExtractedChip {
            pins: vec![
                ExtractedPin { number: "1".into(), name: "A".into(), ..Default::default() },
                ExtractedPin { number: "1".into(), name: "B".into(), ..Default::default() },
                ExtractedPin { number: "2".into(), name: "C".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        let mut form = McuForm::blank();
        let r = apply_to_form(&chip, &mut form);
        // One consolidated warning naming the duplicates (not one per dupe).
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("1 pin number(s) appear more than once (1)")),
            "{:?}",
            r.warnings
        );
    }

    #[test]
    fn helpers_behave() {
        assert_eq!(slugify("STM32F103RB"), "stm32f103rb");
        assert_eq!(slugify("ESP32-C6 (rev 3)"), "esp32_c6_rev_3");
        assert_eq!(package_pin_count("LQFP64"), Some(64));
        assert_eq!(package_pin_count("TSSOP20"), Some(20));
        assert_eq!(package_pin_count("BGA"), None);
        // (signal noise-filtering + mapping now live in `stm32_pin_data` and
        // are covered by its own tests)
    }
}

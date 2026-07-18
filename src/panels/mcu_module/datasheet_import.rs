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

use super::mcu_form::{unknown_function_tokens, McuForm, PinRow};

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
/// `raw` carries any alternate-function text the model could not map to a
/// token, so the human reviewer sees exactly what was dropped.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ExtractedPin {
    /// Package pin number — accepted as a JSON string OR number.
    #[serde(deserialize_with = "de_string_from_any")]
    pub number: String,
    pub name: String,
    pub reserved: bool,
    /// `top` / `bottom` / `left` / `right` if the pinout shows it, else empty
    /// (→ placed on the Left side for the reviewer to redistribute).
    pub side: String,
    /// Space-separated tokens from the form's function grammar.
    pub functions: String,
    /// Alternate-function text that did NOT fit a token, kept verbatim.
    pub raw: String,
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

/// Build the system prompt. `family_hint` / `package_hint` come from whatever
/// the user already typed in the form (may be empty) and merely bias the model.
pub fn build_prompt(family_hint: &str, package_hint: &str) -> String {
    let mut hint = String::new();
    if !family_hint.trim().is_empty() {
        hint.push_str(&format!("\nFamily hint (may be wrong): {}", family_hint.trim()));
    }
    if !package_hint.trim().is_empty() {
        hint.push_str(&format!("\nPackage hint (may be wrong): {}", package_hint.trim()));
    }
    format!(
        "You are a datasheet-extraction assistant for an embedded-Rust IDE.\n\
         From the microcontroller datasheet text the user provides, extract the \
         chip identity, memory map, and pin / alternate-function table, and \
         return the result as a SINGLE JSON object and nothing else — no \
         markdown, no prose, no code fences.\n\
         \n\
         JSON shape:\n\
         {{\n\
         \x20 \"display_name\": string,   // e.g. \"STM32F103RB\"\n\
         \x20 \"family\": string,         // lowercase key if known: stm32f1, stm32wba, esp32c3; else best guess\n\
         \x20 \"cpu\": string,            // e.g. \"Cortex-M3\"\n\
         \x20 \"package\": string,        // e.g. \"LQFP64\"\n\
         \x20 \"flash_origin\": string,   // hex, e.g. \"0x08000000\" (ARM); \"\" if unknown / ESP\n\
         \x20 \"flash_size\": string,     // e.g. \"128K\"\n\
         \x20 \"ram_origin\": string,     // hex, e.g. \"0x20000000\"\n\
         \x20 \"ram_size\": string,       // e.g. \"20K\"\n\
         \x20 \"probe_chip\": string,     // probe-rs chip name if identifiable, else \"\"\n\
         \x20 \"pins\": [\n\
         \x20   {{ \"number\": string|number, \"name\": string, \"reserved\": bool,\n\
         \x20      \"side\": \"top\"|\"bottom\"|\"left\"|\"right\"|\"\",\n\
         \x20      \"functions\": string, \"raw\": string }}\n\
         \x20 ]\n\
         }}\n\
         \n\
         Function token grammar — use ONLY these tokens in \"functions\" \
         (space-separated); put anything you cannot map into \"raw\":\n\
         \x20 in out\n\
         \x20 usart{{n}}_tx usart{{n}}_rx usart{{n}}_cts usart{{n}}_rts usart{{n}}_ck\n\
         \x20 spi{{n}}_nss spi{{n}}_sck spi{{n}}_miso spi{{n}}_mosi\n\
         \x20 i2c{{n}}_scl i2c{{n}}_sda\n\
         \x20 adc{{a}}_{{channel}}   tim{{t}}_{{channel}}\n\
         \x20 swdio swclk   usb_dm usb_dp   can_rx can_tx   mco\n\
         (n/a/t/channel are integers: usart1_tx, spi2_sck, i2c1_scl, adc1_5, tim2_1)\n\
         \n\
         Rules:\n\
         - Map UART to usart (uartN_tx -> usartN_tx).\n\
         - A general-purpose pin usable as input and output gets \"in out\".\n\
         - Power / ground / NC / reset / oscillator pins: reserved=true, functions=\"\".\n\
         - Preserve EVERY alternate function: if it doesn't fit a token, append it to \"raw\".\n\
         - Never invent pins; include only pins present in the provided text.\n\
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
                        "side": { "type": "string", "enum": ["top", "bottom", "left", "right", ""] },
                        "functions": { "type": "string" },
                        "raw": { "type": "string" },
                    },
                    "required": ["number", "name", "reserved", "side", "functions", "raw"],
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
/// something the user already typed). Pins are APPENDED, each onto the side the
/// model named (default Left). Returns the [`ApplyReport`] for review.
pub fn apply_to_form(chip: &ExtractedChip, form: &mut McuForm) -> ApplyReport {
    let mut r = ApplyReport::default();

    patch(&mut form.display_name, &chip.display_name, "Display name", &mut r);
    patch(&mut form.family, &chip.family, "Family", &mut r);
    patch(&mut form.cpu, &chip.cpu, "CPU", &mut r);
    patch(&mut form.package, &chip.package, "Package", &mut r);
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

    for p in &chip.pins {
        let side = side_index(&p.side);
        // Reserved pins never carry functions.
        let functions = if p.reserved {
            String::new()
        } else {
            p.functions.trim().to_string()
        };
        let name = p.name.trim().to_string();
        let number = p.number.trim().to_string();
        for bad in unknown_function_tokens(&functions) {
            r.warnings
                .push(format!("Pin '{name}' has an unknown function token '{bad}'."));
        }
        if !p.raw.trim().is_empty() {
            r.raw_notes
                .push(format!("{name} (pin {number}): unmapped → {}", p.raw.trim()));
        }
        form.pins[side].push(PinRow {
            number,
            name,
            reserved: p.reserved,
            functions,
            imported: true, // tag as AI-provided for the pin editor
        });
        r.pins_added += 1;
    }

    // Cross-check: pin count vs the package number (LQFP64 → 64).
    if let Some(expected) = package_pin_count(&form.package) {
        if r.pins_added != expected {
            r.warnings.push(format!(
                "Package '{}' implies {expected} pins, but {} were extracted — review for gaps.",
                form.package.trim(),
                r.pins_added
            ));
        }
    }
    // Cross-check: duplicate pin numbers across all sides.
    let mut seen = std::collections::HashSet::new();
    let mut dups = std::collections::BTreeSet::new();
    for row in form.pins.iter().flatten() {
        let n = row.number.trim();
        if !n.is_empty() && !seen.insert(n.to_string()) {
            dups.insert(n.to_string());
        }
    }
    for d in dups {
        r.warnings.push(format!("Pin number {d} appears more than once."));
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

/// `top`/`bottom`/`left`/`right` → the `McuForm::pins` index; default Left (2).
fn side_index(side: &str) -> usize {
    match side.trim().to_ascii_lowercase().as_str() {
        "top" => 0,
        "bottom" => 1,
        "right" => 3,
        _ => 2, // left (and the unknown / empty default)
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

// ── The one impure entry point ──────────────────────────────────────────────

/// Call Claude and parse the extraction. Blocking — the dialog runs it on a
/// background thread. All the pure pieces above are composed here.
pub fn call_claude(
    api_key: &str,
    model: &str,
    family_hint: &str,
    package_hint: &str,
    source: &Source,
) -> Result<ExtractedChip, String> {
    if api_key.trim().is_empty() {
        return Err("No API key set — enter your Anthropic API key first.".to_string());
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
    parse_response(&model_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_carries_grammar_and_hints() {
        let p = build_prompt("stm32f1", "LQFP64");
        assert!(p.contains("usart{n}_tx"));
        assert!(p.contains("adc{a}_{channel}"));
        assert!(p.contains("Family hint (may be wrong): stm32f1"));
        assert!(p.contains("Package hint (may be wrong): LQFP64"));
        // Empty hints add nothing.
        let p2 = build_prompt("", "");
        assert!(!p2.contains("hint (may be wrong)"));
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
            { "number": 14, "name": "PA0", "functions": "in out adc1_0" },
            { "number": "15", "name": "PA1", "raw": "TIM2_CH2_ETR" }
          ]
        } trailing"#;
        let chip = parse_response(text).unwrap();
        assert_eq!(chip.display_name, "STM32F103RB");
        assert_eq!(chip.pins.len(), 2);
        assert_eq!(chip.pins[0].number, "14"); // number came in as a JSON int
        assert_eq!(chip.pins[1].number, "15"); // and as a string
        assert_eq!(chip.pins[1].raw, "TIM2_CH2_ETR");
    }

    #[test]
    fn apply_patches_identity_and_appends_pins_with_provenance() {
        let chip = ExtractedChip {
            display_name: "STM32F103RB".into(),
            family: "stm32f1".into(),
            package: "LQFP4".into(), // deliberately implies 4 pins
            probe_chip: "STM32F103RB".into(),
            pins: vec![
                ExtractedPin {
                    number: "14".into(),
                    name: "PA0".into(),
                    functions: "in out".into(),
                    side: "left".into(),
                    ..Default::default()
                },
                ExtractedPin {
                    number: "1".into(),
                    name: "VBAT".into(),
                    reserved: true,
                    side: "top".into(),
                    functions: "in".into(), // dropped because reserved
                    ..Default::default()
                },
                ExtractedPin {
                    number: "42".into(),
                    name: "PB4".into(),
                    functions: "in out wat".into(), // 'wat' is unknown
                    raw: "FSMC_NADV".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut form = McuForm::blank();
        form.id.clear();
        let r = apply_to_form(&chip, &mut form);

        assert_eq!(form.display_name, "STM32F103RB");
        assert_eq!(form.probe_chip, "STM32F103RB");
        assert_eq!(form.id, "stm32f103rb"); // slugified from display name
        assert_eq!(r.pins_added, 3);
        // Sides honoured: PA0 left(2), VBAT top(0), PB4 left(2).
        assert_eq!(form.pins[0].len(), 1);
        assert_eq!(form.pins[2].len(), 2);
        // Reserved pin has no functions.
        assert!(form.pins[0][0].functions.is_empty());
        // Appended rows are tagged as AI-provided for review.
        assert!(form.pins[2][0].imported);
        // Provenance + cross-checks reported.
        assert!(r.raw_notes.iter().any(|n| n.contains("FSMC_NADV")));
        assert!(r.warnings.iter().any(|w| w.contains("unknown function token 'wat'")));
        assert!(r.warnings.iter().any(|w| w.contains("implies 4 pins")));
        assert!(r.patched.iter().any(|p| p == "Display name = STM32F103RB"));
    }

    #[test]
    fn apply_does_not_overwrite_with_empty_values() {
        let mut form = McuForm::blank();
        form.display_name = "KEEP".into();
        let chip = ExtractedChip::default(); // everything empty
        apply_to_form(&chip, &mut form);
        assert_eq!(form.display_name, "KEEP");
    }

    #[test]
    fn duplicate_numbers_are_flagged() {
        let chip = ExtractedChip {
            pins: vec![
                ExtractedPin { number: "1".into(), name: "A".into(), ..Default::default() },
                ExtractedPin { number: "1".into(), name: "B".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        let mut form = McuForm::blank();
        let r = apply_to_form(&chip, &mut form);
        assert!(r.warnings.iter().any(|w| w.contains("Pin number 1 appears more than once")));
    }

    #[test]
    fn helpers_behave() {
        assert_eq!(slugify("STM32F103RB"), "stm32f103rb");
        assert_eq!(slugify("ESP32-C6 (rev 3)"), "esp32_c6_rev_3");
        assert_eq!(package_pin_count("LQFP64"), Some(64));
        assert_eq!(package_pin_count("TSSOP20"), Some(20));
        assert_eq!(package_pin_count("BGA"), None);
        assert_eq!(side_index("Right"), 3);
        assert_eq!(side_index(""), 2);
    }
}

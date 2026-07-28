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

/// Generation cap. A large pinout (176-pin package with full AF lists) fits
/// comfortably under this; higher avoids a truncated — and therefore
/// unparseable — JSON object. Output is billed per token actually generated.
const MAX_TOKENS: u32 = 16000;

// ── Providers ───────────────────────────────────────────────────────────────

/// Which AI backend performs the extraction.
///
/// Deliberately only three, and deliberately these three: reading a pin table
/// needs the model to see the PDF's 2D LAYOUT. Providers without native PDF
/// input can only be fed locally-extracted text, which flattens a pin table
/// into a column-scrambled stream — the model then produces confident,
/// plausible, WRONG pinouts. A silent quality cliff is worse than an absent
/// option, so backends that cannot take a PDF are not offered at all.
///
/// An enum rather than `dyn Trait`: the set is closed, no state is carried, and
/// keeping the per-provider request/response shapes as plain `match` arms over
/// pure functions is what makes them unit-testable without a network.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Provider {
    #[default]
    Anthropic,
    Gemini,
    OpenAi,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Anthropic, Provider::Gemini, Provider::OpenAi];

    /// Shown in the dialog's provider picker.
    pub fn label(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic (Claude)",
            Provider::Gemini => "Google (Gemini)",
            Provider::OpenAi => "OpenAI",
        }
    }

    /// Stable identifier used in the key filename and the cache key. Never
    /// change these: `anthropic` keeps the pre-existing `anthropic_api_key`
    /// file working untouched.
    pub fn slug(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Gemini => "gemini",
            Provider::OpenAi => "openai",
        }
    }

    /// Model used until the user overrides it. Extraction rewards accuracy over
    /// cost, so each is the provider's most capable current model — and each is
    /// only a default: model ids move, and the dialog's field is editable.
    pub fn default_model(self) -> &'static str {
        match self {
            Provider::Anthropic => "claude-opus-4-8",
            Provider::Gemini => "gemini-3.5-flash",
            Provider::OpenAi => "gpt-5.6",
        }
    }

    /// Environment variable consulted before the stored key file.
    pub fn env_var(self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::Gemini => "GEMINI_API_KEY",
            Provider::OpenAi => "OPENAI_API_KEY",
        }
    }

    /// Where the user gets a key — shown under the key field.
    pub fn console_url(self) -> &'static str {
        match self {
            Provider::Anthropic => "https://console.anthropic.com/settings/keys",
            Provider::Gemini => "https://aistudio.google.com/apikey",
            Provider::OpenAi => "https://platform.openai.com/api-keys",
        }
    }

    /// Example model id for the field's tooltip.
    pub fn model_hint(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic model id — e.g. claude-opus-4-8",
            Provider::Gemini => "Gemini model id — e.g. gemini-3.5-flash",
            Provider::OpenAi => "OpenAI model id — e.g. gpt-5.6",
        }
    }

    /// Restore from [`Self::slug`]; unknown text falls back to the default.
    pub fn from_slug(s: &str) -> Provider {
        Provider::ALL
            .into_iter()
            .find(|p| p.slug() == s.trim())
            .unwrap_or_default()
    }

    /// The endpoint for one extraction. Gemini names the model in the PATH,
    /// which is why this takes it.
    fn endpoint(self, model: &str) -> String {
        match self {
            Provider::Anthropic => "https://api.anthropic.com/v1/messages".to_string(),
            Provider::Gemini => format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
                model.trim()
            ),
            Provider::OpenAi => "https://api.openai.com/v1/responses".to_string(),
        }
    }
}

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
             - WHERE TO LOOK for a small package (≤32 pins, e.g. TSSOP20, SO8N, \
             UFQFPN28): its pins are in the \"Pin definitions\" TABLE as a \
             per-package pin-NUMBER column, AND in its own pinout FIGURE. If the \
             table column is hard to read, read the pin numbers and names \
             DIRECTLY off the package's pinout figure instead — do NOT give up \
             and return an empty list just because the table is awkward;\n\
             - MATCHING is tolerant of formatting: a PDF mangles headers, so \
             \"{pkg}\" also matches the same name with different spacing, \
             hyphenation or case (\"TSSOP20\" = \"TSSOP 20\" = \"TSSOP-20\"). Only \
             a DIFFERENT trailing word makes it a different package (\"UFQFPN48\" \
             ≠ \"UFQFPN48 SMPS\");\n\
             - return \"pins\": [] ONLY when \"{pkg}\" genuinely does not appear \
             in the datasheet at all — never because its column/figure is hard \
             to parse or its header is formatted differently."
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
         - IDENTITY FROM THE PINOUT FIGURE TITLE. The pinout diagram's title — \
         e.g. \"Figure 5. STM32G031Fx TSSOP20 pinout\" — is usually the most \
         reliable source of the chip family AND the package, especially when \
         the surrounding text is sparse or the part number appears nowhere \
         else. Read \"display_name\" and \"package\" from it: that title gives \
         display_name \"STM32G031Fx\" (or the exact part if the title names one) \
         and package \"TSSOP20\". A title ending in a wildcard like \"Fx\", \
         \"xx\" or \"(x)\" means a FAMILY that shares this pinout across several \
         flash/pin variants — keep the wildcard in display_name rather than \
         inventing a specific part. Never return empty identity when a pinout \
         figure title is present.\n\
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
/// `Clone` so one picked source can feed two extractions (pins + clock) in
/// parallel worker threads — see the combined import in `datasheet_import_dialog`.
#[derive(Clone)]
pub enum Source {
    Text(String),
    Pdf(Vec<u8>),
}

/// Reject a PDF larger than this before base64 (~33% inflation) pushes the
/// request past the API's 32 MB body limit. Big datasheets should be pasted
/// page-by-page instead.
pub const MAX_PDF_BYTES: usize = 20 * 1024 * 1024;

/// The short user-message text that accompanies a PDF document block.
/// PDF instruction for a CLOCK-tree extraction.
const CLOCK_PDF_INSTRUCTION: &str =
    "This is a microcontroller datasheet (PDF). Extract the main clock-tree \
     SPINE (sources, PLL chain, SYSCLK mux, AHB and APB dividers) following the \
     system instructions and the required JSON schema. Do not model the \
     peripheral kernel clocks.";

const PDF_INSTRUCTION: &str =
    "This is a microcontroller datasheet (PDF). Extract the chip identity, \
     memory map, and full pin / alternate-function table following the system \
     instructions and the required JSON schema.";

/// PDF instruction for the PACKAGE-LIST pre-pass.
const PACKAGES_PDF_INSTRUCTION: &str =
    "This is a microcontroller datasheet (PDF). List the DISTINCT packages it \
     describes (pin-table column headers and pinout figure titles) following the \
     system instructions and the required JSON schema. Do not extract pins.";

/// Which extraction this request is for — selects the JSON schema and the PDF
/// instruction. The provider plumbing (auth, PDF encoding, structured-output
/// mechanism) is identical for both; only these two pieces differ.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ExtractKind {
    /// The pin / alternate-function table (`ExtractedChip`).
    Pins,
    /// The clock tree spine (`clock::graph::extract::ExtractedClock`).
    Clock,
    /// Just the list of package names, for the pick-a-package pre-pass
    /// (`ExtractedPackages`).
    Packages,
}

impl ExtractKind {
    /// The structured-output schema. `additional_properties` follows the
    /// per-provider rule (Gemini false, Anthropic/OpenAI true).
    fn schema(self, additional_properties: bool) -> serde_json::Value {
        match self {
            ExtractKind::Pins => extraction_schema(additional_properties),
            ExtractKind::Clock => {
                crate::panels::mcu_module::clock::graph::extract::clock_extraction_schema(
                    additional_properties,
                )
            }
            ExtractKind::Packages => packages_schema(additional_properties),
        }
    }

    /// The short text accompanying a PDF document block.
    fn pdf_instruction(self) -> &'static str {
        match self {
            ExtractKind::Pins => PDF_INSTRUCTION,
            ExtractKind::Clock => CLOCK_PDF_INSTRUCTION,
            ExtractKind::Packages => PACKAGES_PDF_INSTRUCTION,
        }
    }
}

/// Build the provider's request body (pure — no network).
///
/// All three are asked for schema-constrained JSON via their own native
/// mechanism, so the reply is valid by construction rather than by parsing
/// luck. The shapes have nothing in common beyond that intent, which is why
/// this is a `match` and not a shared builder with holes punched in it.
pub fn build_request_body(
    provider: Provider,
    model: &str,
    system: &str,
    source: &Source,
    kind: ExtractKind,
) -> String {
    match provider {
        Provider::Anthropic => anthropic_body(model, system, source, kind),
        Provider::Gemini => gemini_body(system, source, kind),
        Provider::OpenAi => openai_body(model, system, source, kind),
    }
}

/// Anthropic Messages API: `system` is top-level, the PDF is a `document`
/// content block, and `output_config.format` constrains the decoder.
fn anthropic_body(model: &str, system: &str, source: &Source, kind: ExtractKind) -> String {
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
            { "type": "text", "text": kind.pdf_instruction() },
        ]),
    };
    serde_json::json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": system,
        "output_config": {
            "format": { "type": "json_schema", "schema": kind.schema(true) },
        },
        "messages": [ { "role": "user", "content": content } ],
    })
    .to_string()
}

/// Gemini `generateContent`: the model is in the URL (not the body), the PDF is
/// an `inlineData` part, and JSON is requested through `responseMimeType` +
/// `responseSchema`.
///
/// The schema is emitted WITHOUT `additionalProperties`: `responseSchema` takes
/// an OpenAPI-flavoured subset of JSON Schema, and an unsupported keyword is
/// rejected outright with a 400 rather than ignored. Nothing is lost — the
/// response is schema-constrained regardless.
fn gemini_body(system: &str, source: &Source, kind: ExtractKind) -> String {
    let parts = match source {
        Source::Text(t) => serde_json::json!([{ "text": t }]),
        Source::Pdf(bytes) => serde_json::json!([
            {
                "inlineData": {
                    "mimeType": "application/pdf",
                    "data": base64_encode(bytes),
                },
            },
            { "text": kind.pdf_instruction() },
        ]),
    };
    serde_json::json!({
        "systemInstruction": { "parts": [{ "text": system }] },
        "contents": [ { "role": "user", "parts": parts } ],
        "generationConfig": {
            "responseMimeType": "application/json",
            "responseSchema": kind.schema(false),
            "maxOutputTokens": MAX_TOKENS,
        },
    })
    .to_string()
}

/// OpenAI Responses API: the system prompt is `instructions`, the PDF is an
/// `input_file` whose `file_data` is a data: URI (not bare base64), and
/// `text.format` carries the strict json_schema.
fn openai_body(model: &str, system: &str, source: &Source, kind: ExtractKind) -> String {
    let content = match source {
        Source::Text(t) => serde_json::json!([{ "type": "input_text", "text": t }]),
        Source::Pdf(bytes) => serde_json::json!([
            {
                "type": "input_file",
                "filename": "datasheet.pdf",
                "file_data": format!(
                    "data:application/pdf;base64,{}",
                    base64_encode(bytes)
                ),
            },
            { "type": "input_text", "text": kind.pdf_instruction() },
        ]),
    };
    serde_json::json!({
        "model": model,
        "instructions": system,
        "max_output_tokens": MAX_TOKENS,
        "input": [ { "role": "user", "content": content } ],
        "text": {
            "format": {
                "type": "json_schema",
                "name": "mcu_extraction",
                "strict": true,
                "schema": kind.schema(true),
            },
        },
    })
    .to_string()
}

/// The strict JSON schema the model must fill — mirrors [`ExtractedChip`]. Every
/// property is required; `number` is a string and `side` is an enum, so the
/// reply needs no post-massaging beyond serde.
///
/// `additional_properties` emits the `additionalProperties: false` keyword,
/// which Anthropic and OpenAI's strict mode REQUIRE and Gemini's
/// `responseSchema` subset rejects. See [`gemini_body`].
fn extraction_schema(additional_properties: bool) -> serde_json::Value {
    let mut root = serde_json::json!({
        "type": "object",
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
    });

    if additional_properties {
        root["additionalProperties"] = serde_json::json!(false);
        root["properties"]["pins"]["items"]["additionalProperties"] = serde_json::json!(false);
    }
    root
}

// ── Package-list pre-pass ───────────────────────────────────────────────────

/// The distinct package names a datasheet describes — the cheap pre-pass that
/// lets the user PICK the exact package (which drives the pin extraction) from a
/// list instead of typing it character-for-character (the #1 failure mode).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ExtractedPackages {
    pub packages: Vec<String>,
}

/// System prompt for the package-list pre-pass. Deliberately narrow: it only
/// enumerates package names, so the model doesn't spend effort (or output) on
/// pins here.
pub fn build_packages_prompt() -> String {
    "You are a datasheet-extraction assistant for an embedded-Rust IDE.\n\
     List EVERY distinct package the microcontroller datasheet describes, so the \
     user can pick the exact one to extract pins for. Return a SINGLE JSON object \
     and nothing else — no markdown, no prose, no code fences.\n\
     \n\
     JSON shape:\n\
     { \"packages\": [string, ...] }   // e.g. [\"UFQFPN32\", \"WLCSP41\", \"UFQFPN48\", \"UFQFPN48 SMPS\", \"UFBGA59\"]\n\
     \n\
     Rules:\n\
     - a package name comes from a pin-table COLUMN HEADER or a pinout FIGURE \
     TITLE (e.g. \"Figure 9. UFQFPN48 pinout\"), NEVER from the text drawn inside \
     the package outline (that is the base family and is identical across \
     variants);\n\
     - copy each name EXACTLY as printed, character for character. Names that \
     share a prefix are DIFFERENT packages — list BOTH \"UFQFPN48\" and \
     \"UFQFPN48 SMPS\";\n\
     - include EVERY package the datasheet shows — QFP / QFN / CSP AND BGA;\n\
     - each name at most once, in the order the datasheet presents them;\n\
     - list only packages the datasheet actually shows — never invent one."
        .to_string()
}

/// The structured-output schema for [`build_packages_prompt`]. `additional_
/// properties` follows the same per-provider rule as [`extraction_schema`].
fn packages_schema(additional_properties: bool) -> serde_json::Value {
    let mut root = serde_json::json!({
        "type": "object",
        "properties": {
            "packages": { "type": "array", "items": { "type": "string" } },
        },
        "required": ["packages"],
    });
    if additional_properties {
        root["additionalProperties"] = serde_json::json!(false);
    }
    root
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

/// Pull the model's reply text out of the provider's response envelope, or
/// surface the API error message. All three report failures under a top-level
/// `error.message`, so only the success path differs.
pub fn parse_api_envelope(provider: Provider, resp_json: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(resp_json).map_err(|e| format!("response was not JSON: {e}"))?;
    if let Some(err) = v.get("error") {
        // Gemini nests it; Anthropic/OpenAI use a plain string message.
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .or_else(|| err.as_str())
            .unwrap_or("unknown API error");
        return Err(format!("API error: {msg}"));
    }

    let text = match provider {
        // { "content": [ { "type": "text", "text": … } ] }
        Provider::Anthropic => v
            .get("content")
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
            .map(str::to_string),

        // { "candidates": [ { "content": { "parts": [ { "text": … } ] } } ] }
        // Parts are CONCATENATED: a long JSON object can be split across
        // several, and taking only the first would truncate the extraction
        // into unparseable garbage.
        Provider::Gemini => v
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|cand| cand.pointer("/content/parts"))
            .and_then(|parts| parts.as_array())
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                    .collect::<String>()
            })
            .filter(|s| !s.is_empty()),

        // { "output": [ { "type": "message",
        //                 "content": [ { "type": "output_text", "text": … } ] } ] }
        // Reasoning models put other item types in `output` first, so this
        // scans for the message rather than indexing [0].
        Provider::OpenAi => v
            .get("output")
            .and_then(|o| o.as_array())
            .and_then(|items| {
                items.iter().find_map(|item| {
                    item.get("content")
                        .and_then(|c| c.as_array())
                        .and_then(|blocks| {
                            blocks.iter().find_map(|b| {
                                if b.get("type").and_then(|t| t.as_str()) == Some("output_text") {
                                    b.get("text").and_then(|t| t.as_str())
                                } else {
                                    None
                                }
                            })
                        })
                })
            })
            .map(str::to_string),
    };

    text.ok_or_else(|| {
        format!(
            "no text content in the {} API response",
            provider.label()
        )
    })
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

/// Parse the package-list pre-pass reply into [`ExtractedPackages`].
pub fn parse_packages_reply(model_text: &str) -> Result<ExtractedPackages, String> {
    let json = extract_json_object(model_text)?;
    serde_json::from_str(json).map_err(|e| format!("could not parse packages JSON: {e}"))
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
    // NOTE: `package` is deliberately NOT patched from the extraction — it is a
    // USER input that drives which pin-number column the model reads, so the
    // model's echo must never override it.
    patch(&mut form.flash_origin, &chip.flash_origin, "Flash origin", &mut r);
    // Sizes go through a sanitizer: the model occasionally trails junk onto the
    // value (e.g. "8K probe_chip"), which would fail the memory-value check and
    // block Save. Keep just the leading `0x…` / `<n>` / `<n>K|M` token.
    patch(&mut form.flash_size, &sanitize_mem_size(&chip.flash_size), "Flash size", &mut r);
    patch(&mut form.ram_origin, &chip.ram_origin, "RAM origin", &mut r);
    patch(&mut form.ram_size, &sanitize_mem_size(&chip.ram_size), "RAM size", &mut r);
    patch(&mut form.probe_chip, &chip.probe_chip, "Probe chip", &mut r);

    // Identity + the build-critical fields the model doesn't reliably return.
    // `auto_fill_identity` derives family, CPU, toolchain, target AND the HAL
    // dependency line DETERMINISTICALLY from the (just-patched) part name — the
    // same helper the form's "Auto-fill from name" button uses. Without it an
    // AI-imported non-F1 STM32 kept the blank form's `stm32f1xx-hal` + F1 target
    // and wouldn't compile. It only fires for recognised STM32 names; for
    // anything else (ESP, unknown) we fall back to the model's own family/cpu.
    if form.auto_fill_identity() {
        r.patched.push(format!("Family = {}", form.family));
        r.patched.push(format!("CPU = {}", form.cpu));
        r.patched.push(format!("Target = {}", form.target));
        r.patched.push(format!("HAL dependency = {}", form.hal_dep));
        // Give the chip its family's clock tree so the Clock tab works and real
        // RCC codegen is emitted — same mapping the XML importer uses. Only when
        // the family actually has one (else leave the user's current choice).
        let clk = crate::panels::mcu_module::mcu_form::ClockChoice::for_family(&form.family);
        if clk != crate::panels::mcu_module::mcu_form::ClockChoice::None {
            form.clock = clk;
            r.patched.push(format!("Clock = {}", clk.label()));
        }
    } else {
        patch(&mut form.family, &chip.family, "Family", &mut r);
        patch(&mut form.cpu, &chip.cpu, "CPU", &mut r);
    }

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
    // Sort by package position, then lay out: a dual-in-line package (SO8N,
    // TSSOP, …) goes on the LEFT+RIGHT edges only; everything else splits across
    // the four sides QFP-style. Same choice the XML importer makes.
    rows.sort_by_key(|row| row.number.parse::<usize>().unwrap_or(usize::MAX));
    form.pins = if stm32_pin_data::is_two_row_package(&form.package) {
        stm32_pin_data::distribute_sides_2row(&rows)
    } else {
        stm32_pin_data::distribute_sides(&rows)
    };

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


/// Keep only the leading memory-size token from a possibly-noisy model value:
/// a `0x…` hex literal, or `<digits>` optionally followed (across spaces) by a
/// `K`/`M` suffix — matching what [`mcu_form::parse_ld_number`] accepts. Trailing
/// junk ("8K probe_chip", "64 Kbytes") is dropped; empty in → empty out.
fn sanitize_mem_size(raw: &str) -> String {
    let s = raw.trim();
    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let hex: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if !hex.is_empty() {
            return format!("0x{hex}");
        }
    }
    let chars: Vec<char> = s.chars().collect();
    let Some(i) = chars.iter().position(|c| c.is_ascii_digit()) else {
        return String::new();
    };
    let mut j = i;
    while j < chars.len() && chars[j].is_ascii_digit() {
        j += 1;
    }
    let num: String = chars[i..j].iter().collect();
    // Skip spaces, then take a K/M suffix if one follows (e.g. "8 Kbytes" → 8K).
    let mut k = j;
    while k < chars.len() && chars[k] == ' ' {
        k += 1;
    }
    let suffix = match chars.get(k) {
        Some('K') | Some('k') => "K",
        Some('M') | Some('m') => "M",
        _ => "",
    };
    format!("{num}{suffix}")
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

/// Path to one provider's stored key: `<user config>/<slug>_api_key` (the
/// parent of the `mcus/` folder). `None` only if no config dir can be resolved.
///
/// Anthropic's slug yields `anthropic_api_key` — the exact name used before
/// providers existed, so keys already on disk keep working with no migration.
pub fn api_key_path(provider: Provider) -> Option<PathBuf> {
    super::registry::user_mcus_dir()
        .and_then(|d| d.parent().map(|p| p.join(format!("{}_api_key", provider.slug()))))
}

/// Load a provider's API key: its env var takes precedence, else the stored
/// file, else empty. Trimmed. Keys are never written into the project.
pub fn load_api_key(provider: Provider) -> String {
    if let Ok(k) = std::env::var(provider.env_var()) {
        if !k.trim().is_empty() {
            return k.trim().to_string();
        }
    }
    api_key_path(provider)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Persist a provider's API key to the user config folder (created if missing).
pub fn save_api_key(provider: Provider, key: &str) -> Result<(), String> {
    let path = api_key_path(provider).ok_or("could not resolve the user config folder")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, key.trim()).map_err(|e| e.to_string())
}

/// Where the last-used provider is remembered, so the dialog reopens on the one
/// you actually use instead of resetting to Anthropic every time.
fn last_provider_path() -> Option<PathBuf> {
    super::registry::user_mcus_dir().and_then(|d| d.parent().map(|p| p.join("ai_provider")))
}

pub fn load_last_provider() -> Provider {
    last_provider_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| Provider::from_slug(&s))
        .unwrap_or_default()
}

/// Best-effort — failing to remember the choice must never break an import.
pub fn save_last_provider(provider: Provider) {
    let Some(path) = last_provider_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, provider.slug());
}

/// Which extraction dialog a persisted supplementary prompt belongs to. The two
/// have different base prompts, so their extra guidance is stored separately.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum PromptSlot {
    Pins,
    Clock,
}

fn extra_prompt_path(slot: PromptSlot) -> Option<PathBuf> {
    let name = match slot {
        PromptSlot::Pins => "datasheet_extra_prompt",
        PromptSlot::Clock => "clock_extra_prompt",
    };
    super::registry::user_mcus_dir().and_then(|d| d.parent().map(|p| p.join(name)))
}

/// The user's persisted supplementary prompt for `slot` (empty if none).
pub fn load_extra_prompt(slot: PromptSlot) -> String {
    extra_prompt_path(slot)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default()
}

/// Persist the supplementary prompt for `slot`. Best-effort.
pub fn save_extra_prompt(slot: PromptSlot, text: &str) {
    let Some(path) = extra_prompt_path(slot) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, text);
}

/// Append the user's supplementary guidance to a base prompt, clearly
/// delimited. Empty `extra` returns the base unchanged.
///
/// The block explicitly says it does NOT override the shape or rules, so a
/// vague addition can't silently erode the extraction contract — and structured
/// output enforces the JSON shape regardless of what the user writes here.
pub fn with_extra_prompt(base: &str, extra: &str) -> String {
    let extra = extra.trim();
    if extra.is_empty() {
        return base.to_string();
    }
    format!(
        "{base}\n\nADDITIONAL USER GUIDANCE (extra hints for THIS datasheet; it does NOT \
         override the JSON shape or the rules above):\n{extra}"
    )
}

// ── Extraction cache (never re-pay for the same document) ───────────────────

/// Bump when the prompt or the JSON contract changes, so stale entries miss
/// instead of feeding an old shape back in.
const CACHE_VERSION: u32 = 2;

/// `<user config>/datasheet_cache` — sibling of the stored API key.
pub fn cache_dir() -> Option<PathBuf> {
    super::registry::user_mcus_dir().and_then(|d| d.parent().map(|p| p.join("datasheet_cache")))
}

/// Key for one extraction: prompt version + PROVIDER + model + package + the
/// document itself. Change any of them and it re-extracts; retrying the SAME
/// MCU with the same settings is free. Pure — tested below.
///
/// The provider is part of the key because a model id alone is not unique
/// across backends, and two providers' answers for the same document are not
/// interchangeable.
pub fn cache_key(
    provider: Provider,
    model: &str,
    package: &str,
    extra_prompt: &str,
    source: &Source,
) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    CACHE_VERSION.hash(&mut h);
    provider.slug().hash(&mut h);
    model.trim().hash(&mut h);
    package.trim().hash(&mut h);
    // MANDATORY: without this, changing the supplementary prompt returns the
    // stale cached reply for the same document — the field would look inert.
    extra_prompt.trim().hash(&mut h);
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

/// Sidecar next to the `.json` reply holding a human label for the cache list.
/// The reply file is hash-named, so without this the cache is unreadable.
fn cache_label_file(key: &str) -> Option<PathBuf> {
    cache_dir().map(|d| d.join(format!("{key}.label")))
}

/// A one-line human label for a cached extraction — what the datasheet the
/// entry came from was. Pure so it is testable and stays stable.
///
/// `chip.display_name` is the best name (it is what the model actually
/// identified); the package prefers the chip's own over the requested hint.
pub fn cache_label(
    provider: Provider,
    model: &str,
    package_hint: &str,
    source: &Source,
    chip: &ExtractedChip,
) -> String {
    let name = if !chip.display_name.trim().is_empty() {
        chip.display_name.trim()
    } else {
        "(unnamed chip)"
    };
    let pkg = if !chip.package.trim().is_empty() {
        chip.package.trim()
    } else {
        package_hint.trim()
    };
    let src = match source {
        Source::Text(_) => "text".to_string(),
        Source::Pdf(b) => format!("PDF {}", human_bytes(b.len() as u64)),
    };
    let mut s = name.to_string();
    if !pkg.is_empty() {
        s.push_str(&format!(" · {pkg}"));
    }
    s.push_str(&format!(" · {}/{} · {src}", provider.label(), model.trim()));
    s
}

/// Compact byte size (`1.2 MB`, `840 KB`, `12 B`).
fn human_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

/// The cached model reply for `key`, if any.
fn load_cached(key: &str) -> Option<String> {
    std::fs::read_to_string(cache_file(key)?).ok()
}

/// Store the model reply for `key`, plus a human `label` sidecar. Best-effort —
/// a cache write never fails an extraction that already succeeded.
fn save_cached(key: &str, model_text: &str, label: &str) {
    let Some(path) = cache_file(key) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, model_text);
    if let Some(lpath) = cache_label_file(key) {
        let _ = std::fs::write(lpath, label);
    }
}

/// One cached extraction, for the dialog's list.
pub struct CacheEntry {
    /// Human label (from the `.label` sidecar), or the hash key if it is
    /// missing (an entry saved before labels existed).
    pub label: String,
    pub bytes: u64,
    /// File mtime, for newest-first ordering.
    pub modified: Option<std::time::SystemTime>,
}

/// Every cached extraction, newest first, each with its human label. Only the
/// reply `.json` files count as entries; the `.label` sidecars are read for
/// their text, not listed on their own.
pub fn cache_entries() -> Vec<CacheEntry> {
    let Some(dir) = cache_dir() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let key = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let meta = e.metadata().ok();
        let label = cache_label_file(&key)
            .and_then(|l| std::fs::read_to_string(l).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("(unlabelled · {key})"));
        out.push(CacheEntry {
            label,
            bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            modified: meta.and_then(|m| m.modified().ok()),
        });
    }
    // Newest first; entries without an mtime sink to the bottom.
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
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
        // Count the reply files; the `.label` sidecars are removed with them
        // but not double-counted.
        match p.extension().and_then(|x| x.to_str()) {
            Some("json") => {
                if std::fs::remove_file(&p).is_ok() {
                    removed += 1;
                }
            }
            Some("label") => {
                let _ = std::fs::remove_file(&p);
            }
            _ => {}
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

// ── The impure entry points ──────────────────────────────────────────────────

/// POST a prepared request body to `provider` and return the model's reply
/// text (the assistant message, unwrapped from the provider envelope).
///
/// The transport — auth header per provider, 180 s timeout, HTTP-error message
/// extraction, `parse_api_envelope` — is identical for the pin and clock
/// extractions, so both share this.
fn post_and_parse(
    provider: Provider,
    api_key: &str,
    model: &str,
    body: &str,
) -> Result<String, String> {
    let req = ureq::post(&provider.endpoint(model))
        .set("content-type", "application/json")
        .timeout(std::time::Duration::from_secs(180));
    let req = match provider {
        Provider::Anthropic => req
            .set("x-api-key", api_key.trim())
            .set("anthropic-version", "2023-06-01"),
        Provider::Gemini => req.set("x-goog-api-key", api_key.trim()),
        Provider::OpenAi => req.set("authorization", &format!("Bearer {}", api_key.trim())),
    };
    let text = match req.send_string(body) {
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
    parse_api_envelope(provider, &text)
}

/// A clock-tree extraction result plus whether it came from the cache.
pub struct ClockExtraction {
    pub clock: crate::panels::mcu_module::clock::graph::GraphClock,
    pub from_cache: bool,
}

/// Extract the CLOCK TREE from a datasheet: same providers, key storage, PDF
/// handling and cache as the pin importer, but the clock prompt/schema and the
/// [`clock::graph::extract`] conversion + numeric self-check.
///
/// Blocking — the dialog runs it on a worker thread.
pub fn call_ai_clock(
    provider: Provider,
    api_key: &str,
    model: &str,
    extra_prompt: &str,
    source: &Source,
    use_cache: bool,
) -> Result<ClockExtraction, String> {
    use crate::panels::mcu_module::clock::graph::extract as ce;

    if api_key.trim().is_empty() {
        return Err(format!(
            "No API key set — enter your {} API key first.",
            provider.label()
        ));
    }
    if model.trim().is_empty() {
        return Err("No model set — enter a model id first.".to_string());
    }
    match source {
        Source::Text(t) if t.trim().is_empty() => {
            return Err("Nothing to extract — paste the clock-tree text first.".to_string());
        }
        Source::Pdf(b) if b.is_empty() => return Err("The selected PDF is empty.".to_string()),
        Source::Pdf(b) if b.len() > MAX_PDF_BYTES => {
            return Err(format!(
                "PDF is {:.1} MB — over the {} MB limit. Paste the relevant pages instead.",
                b.len() as f64 / (1024.0 * 1024.0),
                MAX_PDF_BYTES / (1024 * 1024)
            ));
        }
        _ => {}
    }

    // Cache: keyed like the pin path but with a "clock" package tag so a clock
    // and a pin extraction of the same PDF never collide.
    let key = cache_key(provider, model, "clock-tree", extra_prompt, source);
    let build_from =
        |model_text: &str| -> Result<crate::panels::mcu_module::clock::graph::GraphClock, String> {
            let ex = ce::parse_clock_reply(model_text)?;
            ce::to_graph_clock(&ex)
        };
    if use_cache {
        if let Some(cached) = load_cached(&key) {
            if let Ok(gc) = build_from(&cached) {
                return Ok(ClockExtraction { clock: gc, from_cache: true });
            }
        }
    }

    let system = with_extra_prompt(&ce::build_clock_prompt(), extra_prompt);
    let body = build_request_body(provider, model, &system, source, ExtractKind::Clock);
    let model_text = post_and_parse(provider, api_key, model, &body)?;
    // Convert + self-check before caching, so only a usable reply is stored.
    let gc = build_from(&model_text)?;
    let label = format!(
        "clock tree · {}/{} · {}",
        provider.label(),
        model.trim(),
        match source {
            Source::Text(_) => "text".to_string(),
            Source::Pdf(b) => format!("PDF {}", human_bytes(b.len() as u64)),
        }
    );
    save_cached(&key, &model_text, &label);
    Ok(ClockExtraction { clock: gc, from_cache: false })
}

/// A package-list pre-pass result plus whether it came from the cache.
pub struct PackageList {
    pub packages: Vec<String>,
    pub from_cache: bool,
}

/// The pick-a-package pre-pass: a cheap call that returns the datasheet's
/// distinct package names, so the user selects the exact one (which drives the
/// pin extraction) instead of typing it. Same providers / key storage / PDF
/// handling / cache as the other two; keyed with a `"package-list"` tag so it
/// never collides with a pin or clock extraction of the same document.
///
/// Blocking — the dialog runs it on a worker thread.
pub fn call_ai_packages(
    provider: Provider,
    api_key: &str,
    model: &str,
    source: &Source,
    use_cache: bool,
) -> Result<PackageList, String> {
    if api_key.trim().is_empty() {
        return Err(format!(
            "No API key set — enter your {} API key first.",
            provider.label()
        ));
    }
    if model.trim().is_empty() {
        return Err("No model set — enter a model id first.".to_string());
    }
    match source {
        Source::Text(t) if t.trim().is_empty() => {
            return Err("Nothing to scan — paste the datasheet text or pick a PDF first.".to_string());
        }
        Source::Pdf(b) if b.is_empty() => return Err("The selected PDF is empty.".to_string()),
        Source::Pdf(b) if b.len() > MAX_PDF_BYTES => {
            return Err(format!(
                "PDF is {:.1} MB — over the {} MB limit. Paste the relevant pages instead.",
                b.len() as f64 / (1024.0 * 1024.0),
                MAX_PDF_BYTES / (1024 * 1024)
            ));
        }
        _ => {}
    }

    let key = cache_key(provider, model, "package-list", "", source);
    let build_from = |model_text: &str| -> Result<Vec<String>, String> {
        let mut v: Vec<String> = parse_packages_reply(model_text)?
            .packages
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        v.dedup(); // the prompt already asks for uniqueness; drop adjacent repeats
        if v.is_empty() {
            return Err("no packages found in the datasheet".to_string());
        }
        Ok(v)
    };
    if use_cache {
        if let Some(cached) = load_cached(&key) {
            if let Ok(v) = build_from(&cached) {
                return Ok(PackageList { packages: v, from_cache: true });
            }
        }
    }

    let body = build_request_body(provider, model, &build_packages_prompt(), source, ExtractKind::Packages);
    let model_text = post_and_parse(provider, api_key, model, &body)?;
    let v = build_from(&model_text)?;
    let label = format!(
        "packages · {}/{} · {}",
        provider.label(),
        model.trim(),
        match source {
            Source::Text(_) => "text".to_string(),
            Source::Pdf(b) => format!("PDF {}", human_bytes(b.len() as u64)),
        }
    );
    save_cached(&key, &model_text, &label);
    Ok(PackageList { packages: v, from_cache: false })
}

/// Call the selected provider and parse the extraction. Blocking — the dialog
/// runs it on a background thread. All the pure pieces above are composed here.
#[allow(clippy::too_many_arguments)]
pub fn call_ai(
    provider: Provider,
    api_key: &str,
    model: &str,
    family_hint: &str,
    package_hint: &str,
    extra_prompt: &str,
    source: &Source,
    use_cache: bool,
) -> Result<Extraction, String> {
    if api_key.trim().is_empty() {
        return Err(format!(
            "No API key set — enter your {} API key first.",
            provider.label()
        ));
    }
    if model.trim().is_empty() {
        return Err("No model set — enter a model id first.".to_string());
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
    let key = cache_key(provider, model, package_hint, extra_prompt, source);
    if use_cache {
        if let Some(cached) = load_cached(&key) {
            if let Ok(chip) = parse_response(&cached) {
                return Ok(Extraction { chip, from_cache: true });
            }
            // A corrupt / stale entry just falls through to a fresh call.
        }
    }

    let system = with_extra_prompt(&build_prompt(family_hint, package_hint), extra_prompt);
    let body = build_request_body(provider, model, &system, source, ExtractKind::Pins);
    let model_text = post_and_parse(provider, api_key, model, &body)?;
    let chip = parse_response(&model_text)?;
    // Only cache a reply we could actually parse; the label names the chip so
    // the cache list is readable.
    let label = cache_label(provider, model, package_hint, source, &chip);
    save_cached(&key, &model_text, &label);
    Ok(Extraction { chip, from_cache: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_mem_size_keeps_only_the_value_token() {
        assert_eq!(sanitize_mem_size("8K probe_chip"), "8K"); // the reported bug
        assert_eq!(sanitize_mem_size("64K"), "64K");
        assert_eq!(sanitize_mem_size("8 Kbytes"), "8K");
        assert_eq!(sanitize_mem_size("128"), "128");
        assert_eq!(sanitize_mem_size("0x20000000"), "0x20000000");
        assert_eq!(sanitize_mem_size(""), "");
        assert_eq!(sanitize_mem_size("unknown"), "");
        // The result must round-trip through the form's validator.
        assert!(crate::panels::mcu_module::mcu_form::parse_ld_number(&sanitize_mem_size("8K probe_chip")).is_some());
    }

    #[test]
    fn packages_prepass_prompt_schema_and_parse() {
        let p = build_packages_prompt();
        assert!(p.contains("\"packages\""));
        assert!(p.contains("EXACTLY")); // exact, character-for-character names
        assert!(p.contains("UFQFPN48 SMPS")); // variant discipline demonstrated
        // Schema: additionalProperties present for strict providers, absent for
        // Gemini's responseSchema subset (same rule as the pin schema).
        assert_eq!(packages_schema(true)["additionalProperties"], serde_json::json!(false));
        assert!(packages_schema(false).get("additionalProperties").is_none());
        // Parse tolerates a fenced reply and keeps variant names distinct.
        let reply = "```json\n{ \"packages\": [\"UFQFPN48\", \"UFQFPN48 SMPS\", \"UFBGA59\"] }\n```";
        let ex = parse_packages_reply(reply).unwrap();
        assert_eq!(ex.packages, ["UFQFPN48", "UFQFPN48 SMPS", "UFBGA59"]);
    }

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
        // Identity can be read from the pinout figure title (the case where the
        // part number appears nowhere else) — including a family wildcard.
        assert!(p.contains("IDENTITY FROM THE PINOUT FIGURE TITLE"));
        assert!(p.contains("STM32G031Fx TSSOP20 pinout"));
        assert!(p.contains("wildcard"));
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
    fn cache_label_names_the_chip_package_provider_and_source() {
        let chip = ExtractedChip {
            display_name: "STM32G031Fx".into(),
            package: "TSSOP20".into(),
            ..Default::default()
        };
        let label = cache_label(
            Provider::Gemini,
            "gemini-3.5-flash",
            "LQFP48", // hint is IGNORED when the chip has its own package
            &Source::Pdf(vec![0u8; 2 * 1024 * 1024]),
            &chip,
        );
        assert!(label.contains("STM32G031Fx"), "{label}");
        assert!(label.contains("TSSOP20"), "{label}");
        assert!(!label.contains("LQFP48"), "chip package must win: {label}");
        assert!(label.contains("Google (Gemini)/gemini-3.5-flash"), "{label}");
        assert!(label.contains("PDF 2.0 MB"), "{label}");
    }

    #[test]
    fn cache_label_falls_back_to_hint_and_marks_text_source() {
        // Chip gave no package → the requested hint fills in; text source.
        let chip = ExtractedChip {
            display_name: "STM32F103RB".into(),
            package: "".into(),
            ..Default::default()
        };
        let label = cache_label(
            Provider::Anthropic,
            "claude-opus-4-8",
            "LQFP64",
            &Source::Text("pins…".into()),
            &chip,
        );
        assert!(label.contains("STM32F103RB · LQFP64"), "{label}");
        assert!(label.contains("· text"), "{label}");
    }

    #[test]
    fn cache_key_is_stable_and_input_sensitive() {
        use Provider::*;
        let doc = Source::Text("PIN TABLE".into());
        let base = cache_key(Anthropic, "claude-opus-4-8", "UFQFPN48", "", &doc);
        // Same inputs → same key (a retry hits the cache).
        assert_eq!(base, cache_key(Anthropic, "claude-opus-4-8", "UFQFPN48", "", &doc));
        // Whitespace around model/package doesn't split the cache.
        assert_eq!(base, cache_key(Anthropic, " claude-opus-4-8 ", " UFQFPN48 ", "", &doc));
        // A different package reads a different column → different key.
        assert_ne!(base, cache_key(Anthropic, "claude-opus-4-8", "UFBGA59", "", &doc));
        // A different model or document → different key.
        assert_ne!(base, cache_key(Anthropic, "claude-sonnet-5", "UFQFPN48", "", &doc));
        assert_ne!(
            base,
            cache_key(Anthropic, "claude-opus-4-8", "UFQFPN48", "", &Source::Text("OTHER".into()))
        );
        // Text and PDF sources never collide.
        assert_ne!(
            base,
            cache_key(Anthropic, "claude-opus-4-8", "UFQFPN48", "", &Source::Pdf(b"PIN TABLE".to_vec()))
        );
        // Same model NAME on a different provider is a different extraction —
        // model ids are not unique across backends.
        assert_ne!(base, cache_key(Gemini, "claude-opus-4-8", "UFQFPN48", "", &doc));
        assert_ne!(base, cache_key(OpenAi, "claude-opus-4-8", "UFQFPN48", "", &doc));
        // A different supplementary prompt must re-extract, not reuse the reply.
        assert_ne!(
            base,
            cache_key(Anthropic, "claude-opus-4-8", "UFQFPN48", "ignore SMPS", &doc)
        );
        // Whitespace around the extra prompt doesn't split the cache.
        assert_eq!(base, cache_key(Anthropic, "claude-opus-4-8", "UFQFPN48", "  ", &doc));
    }

    #[test]
    fn with_extra_prompt_appends_only_when_nonempty() {
        assert_eq!(with_extra_prompt("BASE", ""), "BASE");
        assert_eq!(with_extra_prompt("BASE", "   "), "BASE");
        let out = with_extra_prompt("BASE", "prefer the non-SMPS variant");
        assert!(out.starts_with("BASE"));
        assert!(out.contains("ADDITIONAL USER GUIDANCE"));
        assert!(out.contains("prefer the non-SMPS variant"));
    }

    #[test]
    fn anthropic_request_body_is_well_formed() {
        let body = build_request_body(
            Provider::Anthropic,
            "claude-opus-4-8",
            "SYS",
            &Source::Text("PASTE".into()),
            ExtractKind::Pins,
        );
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
    fn anthropic_pdf_request_embeds_a_base64_document_block() {
        let body = build_request_body(
            Provider::Anthropic,
            "claude-opus-4-8",
            "SYS",
            &Source::Pdf(b"%PDF-1.4".to_vec()),
            ExtractKind::Pins,
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let content = &v["messages"][0]["content"];
        assert_eq!(content[0]["type"], "document");
        assert_eq!(content[0]["source"]["type"], "base64");
        assert_eq!(content[0]["source"]["media_type"], "application/pdf");
        assert_eq!(content[0]["source"]["data"], base64_encode(b"%PDF-1.4"));
        assert_eq!(content[1]["type"], "text");
    }

    #[test]
    fn gemini_request_uses_inline_data_and_a_response_schema() {
        let body = build_request_body(
            Provider::Gemini,
            "gemini-3.5-flash",
            "SYS",
            &Source::Pdf(b"%PDF-1.4".to_vec()),
            ExtractKind::Pins,
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        // The model goes in the URL, never the body.
        assert!(v.get("model").is_none());
        assert_eq!(v["systemInstruction"]["parts"][0]["text"], "SYS");
        let parts = &v["contents"][0]["parts"];
        assert_eq!(parts[0]["inlineData"]["mimeType"], "application/pdf");
        assert_eq!(parts[0]["inlineData"]["data"], base64_encode(b"%PDF-1.4"));
        assert!(parts[1]["text"].is_string());
        assert_eq!(v["generationConfig"]["responseMimeType"], "application/json");
        assert!(v["generationConfig"]["responseSchema"]["properties"]["pins"].is_object());
    }

    #[test]
    fn gemini_schema_omits_additional_properties() {
        // `responseSchema` takes an OpenAPI-flavoured SUBSET of JSON Schema and
        // rejects unknown keywords with a 400 — so this must not leak in.
        let body = build_request_body(
            Provider::Gemini,
            "gemini-3.5-flash",
            "SYS",
            &Source::Text("PASTE".into()),
            ExtractKind::Pins,
        );
        assert!(
            !body.contains("additionalProperties"),
            "additionalProperties leaked into the Gemini schema"
        );
        // …while the providers that REQUIRE it still get it.
        for p in [Provider::Anthropic, Provider::OpenAi] {
            let b = build_request_body(p, "m", "SYS", &Source::Text("PASTE".into()), ExtractKind::Pins);
            assert!(b.contains("additionalProperties"), "{p:?} lost the keyword");
        }
    }

    #[test]
    fn openai_request_uses_input_file_with_a_data_uri() {
        let body = build_request_body(
            Provider::OpenAi,
            "gpt-5.6",
            "SYS",
            &Source::Pdf(b"%PDF-1.4".to_vec()),
            ExtractKind::Pins,
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["model"], "gpt-5.6");
        assert_eq!(v["instructions"], "SYS");
        let content = &v["input"][0]["content"];
        assert_eq!(content[0]["type"], "input_file");
        // file_data is a data: URI, NOT bare base64 — the API rejects bare.
        assert_eq!(
            content[0]["file_data"],
            format!("data:application/pdf;base64,{}", base64_encode(b"%PDF-1.4"))
        );
        assert_eq!(content[1]["type"], "input_text");
        assert_eq!(v["text"]["format"]["type"], "json_schema");
        assert_eq!(v["text"]["format"]["strict"], true);
        assert!(v["text"]["format"]["name"].is_string());
    }

    #[test]
    fn endpoints_are_provider_shaped() {
        // Gemini is the odd one: the model is part of the path.
        assert!(Provider::Gemini
            .endpoint("gemini-3.5-flash")
            .ends_with("/models/gemini-3.5-flash:generateContent"));
        assert!(Provider::Anthropic.endpoint("x").ends_with("/v1/messages"));
        assert!(Provider::OpenAi.endpoint("x").ends_with("/v1/responses"));
    }

    #[test]
    fn anthropic_key_file_name_is_unchanged() {
        // Keys already on disk must keep working — this slug IS the old
        // filename (`anthropic_api_key`).
        assert_eq!(Provider::Anthropic.slug(), "anthropic");
    }

    #[test]
    fn provider_slug_round_trips_and_tolerates_junk() {
        for p in Provider::ALL {
            assert_eq!(Provider::from_slug(p.slug()), p);
        }
        assert_eq!(Provider::from_slug(" gemini "), Provider::Gemini);
        // An unreadable/stale file must not break the dialog.
        assert_eq!(Provider::from_slug("wat"), Provider::default());
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
        assert_eq!(parse_api_envelope(Provider::Anthropic, ok).unwrap(), "hello");
        let err = r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#;
        assert!(parse_api_envelope(Provider::Anthropic, err)
            .unwrap_err()
            .contains("bad key"));
    }

    #[test]
    fn gemini_envelope_concatenates_every_part() {
        // A long extraction arrives split across parts; taking only the first
        // would truncate the JSON into something unparseable.
        let ok = r#"{"candidates":[{"content":{"parts":[
            {"text":"{\"a\":"},{"text":"1}"}]}}]}"#;
        assert_eq!(
            parse_api_envelope(Provider::Gemini, ok).unwrap(),
            "{\"a\":1}"
        );
        let err = r#"{"error":{"code":400,"message":"bad schema"}}"#;
        assert!(parse_api_envelope(Provider::Gemini, err)
            .unwrap_err()
            .contains("bad schema"));
    }

    #[test]
    fn openai_envelope_skips_non_message_output_items() {
        // Reasoning models emit other item types before the message.
        let ok = r#"{"output":[
            {"type":"reasoning","summary":[]},
            {"type":"message","content":[{"type":"output_text","text":"hello"}]}]}"#;
        assert_eq!(parse_api_envelope(Provider::OpenAi, ok).unwrap(), "hello");
        let err = r#"{"error":{"message":"bad key"}}"#;
        assert!(parse_api_envelope(Provider::OpenAi, err)
            .unwrap_err()
            .contains("bad key"));
    }

    #[test]
    fn empty_envelopes_name_the_provider() {
        for (p, body) in [
            (Provider::Anthropic, "{}"),
            (Provider::Gemini, r#"{"candidates":[]}"#),
            (Provider::OpenAi, r#"{"output":[]}"#),
        ] {
            let e = parse_api_envelope(p, body).unwrap_err();
            assert!(e.contains(p.label()), "{p:?} error was unhelpful: {e}");
        }
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

    /// A non-F1 STM32 import must derive the build-critical fields from the part
    /// name, not leave the blank form's F1 defaults — otherwise the generated
    /// project won't compile. Regression guard for the `hal_dep`/`target` gap.
    #[test]
    fn apply_derives_target_and_hal_for_non_f1_stm32() {
        let chip = ExtractedChip {
            display_name: "STM32G0B1RE".into(),
            family: "".into(), // model didn't say — name-derivation must fill it
            pins: vec![ExtractedPin { number: "1".into(), name: "PA0".into(), ..Default::default() }],
            ..Default::default()
        };
        let mut form = McuForm::blank(); // starts as stm32f1 / thumbv7m / stm32f1xx-hal
        let r = apply_to_form(&chip, &mut form);
        assert_eq!(form.family, "stm32g0");
        assert_eq!(form.cpu, "Cortex-M0+");
        assert_eq!(form.target, "thumbv6m-none-eabi");
        assert!(form.hal_dep.contains("embassy-stm32"), "{}", form.hal_dep);
        assert!(form.hal_dep.contains("stm32g0b1re"), "{}", form.hal_dep);
        assert!(r.patched.iter().any(|p| p.starts_with("HAL dependency = ")));
    }

    /// A non-STM32 (ESP) name isn't recognised by the deterministic derivation,
    /// so the model's own family/cpu must survive as the fallback.
    #[test]
    fn apply_keeps_model_identity_for_non_stm32() {
        let chip = ExtractedChip {
            display_name: "ESP32-C3".into(),
            family: "esp32c3".into(),
            cpu: "RISC-V".into(),
            ..Default::default()
        };
        let mut form = McuForm::blank();
        apply_to_form(&chip, &mut form);
        assert_eq!(form.family, "esp32c3");
        assert_eq!(form.cpu, "RISC-V");
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

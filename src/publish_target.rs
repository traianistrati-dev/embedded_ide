//! Choosing WHERE to publish, and building the cargo invocation for it.
//!
//! # Why `--registry` cannot just take the URL you typed
//!
//! `cargo publish --registry` takes a registry NAME defined in a config file,
//! not a URL. The sibling flag `--index` does take a URL, and it is the worse
//! door: verified against cargo 1.98,
//!
//! ```text
//! error: command-line argument --index requires --token to be specified
//! ```
//!
//! so that route forces the token onto the command line, where any other user
//! on the machine can read it out of the process list. (`cargo publish` does
//! not even list `--token` in its `--help`.)
//!
//! The way out is that every cargo config key has an environment twin: the
//! documented rule is that `foo.bar` is settable as `CARGO_FOO_BAR`, keys
//! uppercased with dots and dashes turned into underscores. So an arbitrary
//! index URL becomes a registry cargo will accept, without writing a config
//! file into the user's project and without a token on the command line:
//!
//! ```text
//! CARGO_REGISTRIES_EIDE_TARGET_INDEX=sparse+https://…
//! CARGO_REGISTRIES_EIDE_TARGET_TOKEN=…
//! cargo publish --registry eide_target
//! ```
//!
//! Verified: cargo accepted the ad-hoc name with the index defined only in the
//! environment and went on to fetch that index.
//!
//! # The list to choose from
//!
//! The most useful list is not a table of vendors — it is the registries the
//! user has ALREADY configured, read out of their own cargo config. Those need
//! no index at all: cargo already knows them by name.

use std::collections::BTreeMap;

/// Where a publish should go.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    /// The default registry. No `--registry`, no index.
    CratesIo,
    /// A registry cargo already knows by name, from a config file.
    Configured { name: String, index: String },
    /// An index URL typed by the user, wired up through the environment.
    Custom { index: String },
}

impl Target {
    /// What to show in the picker.
    pub fn label(&self) -> String {
        match self {
            Self::CratesIo => "crates.io".to_owned(),
            Self::Configured { name, .. } => format!("{name} (configured)"),
            Self::Custom { .. } => "Custom registry…".to_owned(),
        }
    }
}

/// The ad-hoc registry name used for a typed-in index URL. Lowercase because
/// cargo's `--registry` argument is the config key, not the env-var spelling.
pub const ADHOC_NAME: &str = "eide_target";

/// Templates for platforms whose index shape is documented, offered as a
/// starting point for the custom field. `<…>` parts are for the user to fill.
///
/// Deliberately short: a vendor list written from memory would be exactly the
/// kind of claim that turns out wrong, and the free-text field answers the
/// general case anyway.
pub const INDEX_TEMPLATES: &[(&str, &str)] = &[(
    "Cloudsmith",
    "sparse+https://cargo.cloudsmith.io/<owner>/<repository>/",
)];

/// The environment-variable suffix cargo derives from a registry name.
///
/// Documented rule: config key `registries.<name>.index` is settable as
/// `CARGO_REGISTRIES_<NAME>_INDEX`, "keys are converted to uppercase, dots and
/// dashes are converted to underscores".
pub fn env_key(name: &str, suffix: &str) -> String {
    let n: String = name
        .to_uppercase()
        .chars()
        .map(|c| if c == '-' || c == '.' { '_' } else { c })
        .collect();
    format!("CARGO_REGISTRIES_{n}_{suffix}")
}

/// The `[registries]` table of a cargo config file: name → index URL.
///
/// Both spellings cargo accepts are read: the inline table
/// (`name = { index = "…" }`) and the section form (`[registries.name]` with
/// `index` on its own line).
pub fn parse_registries(config_toml: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut section = String::new();
    for line in config_toml.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = inner.trim().to_owned();
            continue;
        }
        let Some((k, v)) = t.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim().trim_matches('"'), v.trim());
        if let Some(name) = section.strip_prefix("registries.") {
            if k == "index" {
                out.insert(name.trim().to_owned(), unquote(v));
            }
            continue;
        }
        if section == "registries"
            && v.starts_with('{')
            && let Some(idx) = inline_value(v, "index")
        {
            out.insert(k.to_owned(), idx);
        }
    }
    out
}

/// Strip surrounding quotes from a TOML scalar.
fn unquote(v: &str) -> String {
    v.trim().trim_matches('"').trim().to_owned()
}

/// Pull `key = "…"` out of an inline table.
fn inline_value(inline: &str, key: &str) -> Option<String> {
    let at = inline.find(key)?;
    let rest = &inline[at + key.len()..];
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// The arguments and environment for publishing `crate_dir` to `target`.
///
/// `token` is `None` when the user wants cargo to use credentials it already
/// has (`cargo login`); it is never placed in the argument list.
///
/// `dry_run` keeps `--dry-run`. A real publish deliberately does NOT pass
/// `--allow-dirty`: cargo refusing to upload an uncommitted tree is a feature,
/// and a publish cannot be taken back.
pub fn publish_command(
    target: &Target,
    token: Option<&str>,
    dry_run: bool,
) -> (Vec<String>, Vec<(String, String)>) {
    let mut args: Vec<String> = vec!["publish".into()];
    let mut env: Vec<(String, String)> = Vec::new();

    match target {
        Target::CratesIo => {
            // The DEFAULT registry's token is `registry.token`, whose env twin
            // is CARGO_REGISTRY_TOKEN — singular, and not the `REGISTRIES`
            // form the named ones use.
            if let Some(t) = token {
                env.push(("CARGO_REGISTRY_TOKEN".into(), t.to_owned()));
            }
        }
        Target::Configured { name, .. } => {
            args.push("--registry".into());
            args.push(name.clone());
            if let Some(t) = token {
                env.push((env_key(name, "TOKEN"), t.to_owned()));
            }
        }
        Target::Custom { index } => {
            // The index is handed over through the environment rather than
            // `--index`, which would drag the token onto the command line.
            args.push("--registry".into());
            args.push(ADHOC_NAME.into());
            env.push((env_key(ADHOC_NAME, "INDEX"), index.trim().to_owned()));
            if let Some(t) = token {
                env.push((env_key(ADHOC_NAME, "TOKEN"), t.to_owned()));
            }
        }
    }
    if dry_run {
        args.push("--dry-run".into());
        // A rehearsal must not be refused over uncommitted edits.
        args.push("--allow-dirty".into());
    }
    args.push("--color=never".into());
    (args, env)
}

/// Why a target cannot be published to yet. `None` = ready.
pub fn target_blocker(target: &Target, token: Option<&str>) -> Option<String> {
    if let Target::Custom { index } = target {
        let i = index.trim();
        if i.is_empty() {
            return Some("Type the registry's index URL.".to_owned());
        }
        if i.contains('<') || i.contains('>') {
            return Some("Replace the `<…>` placeholders in the index URL.".to_owned());
        }
        if !(i.starts_with("sparse+http") || i.starts_with("http") || i.starts_with("git")) {
            return Some(
                "The index must be a URL - usually `sparse+https://…` for a modern registry."
                    .to_owned(),
            );
        }
    }
    if token.is_some_and(|t| t.trim().is_empty()) {
        return Some("Paste a token, or switch to the credentials cargo already has.".to_owned());
    }
    // A typed index becomes a registry named [`ADHOC_NAME`], which exists only
    // in the child process's environment - no config file defines it and
    // `cargo login` can never have stored a credential under it. "Use the
    // credentials cargo already has" therefore has nothing to find, and cargo
    // would package and build the whole crate before failing at the upload.
    if matches!(target, Target::Custom { .. }) && token.is_none() {
        return Some(
            "A typed-in registry has no saved credentials - paste a token for it.".to_owned(),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_env_name_follows_cargos_documented_transform() {
        // "keys are converted to uppercase, dots and dashes are converted to
        // underscores".
        assert_eq!(
            env_key("my-registry", "TOKEN"),
            "CARGO_REGISTRIES_MY_REGISTRY_TOKEN"
        );
        assert_eq!(
            env_key("eide_target", "INDEX"),
            "CARGO_REGISTRIES_EIDE_TARGET_INDEX"
        );
        assert_eq!(env_key("a.b", "TOKEN"), "CARGO_REGISTRIES_A_B_TOKEN");
    }

    #[test]
    fn registries_are_read_in_both_config_spellings() {
        let inline = "[registries]\nmy-reg = { index = \"sparse+https://x/index/\" }\n";
        assert_eq!(
            parse_registries(inline).get("my-reg").map(String::as_str),
            Some("sparse+https://x/index/")
        );
        let section = "[registries.other]\nindex = \"https://git/idx\"\ntoken = \"secret\"\n";
        let got = parse_registries(section);
        assert_eq!(
            got.get("other").map(String::as_str),
            Some("https://git/idx")
        );
        assert_eq!(got.len(), 1, "a token line is not a registry");
    }

    #[test]
    fn unrelated_config_sections_are_ignored() {
        let cfg = "[build]\ntarget = \"thumbv7m-none-eabi\"\n\n[net]\nretry = 3\n";
        assert!(parse_registries(cfg).is_empty());
        // …and a commented-out registry is not one.
        assert!(parse_registries("[registries]\n# x = { index = \"y\" }\n").is_empty());
    }

    /// crates.io is the DEFAULT registry: no `--registry`, and its token is the
    /// singular `CARGO_REGISTRY_TOKEN`, not the `REGISTRIES` form.
    #[test]
    fn crates_io_passes_no_registry_flag() {
        let (args, env) = publish_command(&Target::CratesIo, Some("tok"), false);
        assert!(!args.iter().any(|a| a == "--registry"), "{args:?}");
        assert_eq!(
            env,
            vec![("CARGO_REGISTRY_TOKEN".to_owned(), "tok".to_owned())]
        );
    }

    /// A configured registry needs only its name — cargo already has the index.
    #[test]
    fn a_configured_registry_is_named_not_re_indexed() {
        let t = Target::Configured {
            name: "my-reg".to_owned(),
            index: "sparse+https://x/".to_owned(),
        };
        let (args, env) = publish_command(&t, Some("tok"), false);
        assert!(
            args.windows(2).any(|w| w == ["--registry", "my-reg"]),
            "{args:?}"
        );
        assert!(!env.iter().any(|(k, _)| k.ends_with("_INDEX")), "{env:?}");
        assert_eq!(env[0].0, "CARGO_REGISTRIES_MY_REG_TOKEN");
    }

    /// The whole point of the ad-hoc name: a typed URL becomes a registry
    /// through the environment, so the token never reaches the command line.
    #[test]
    fn a_custom_index_travels_in_the_environment_never_in_the_args() {
        let t = Target::Custom {
            index: "sparse+https://cargo.example.com/o/r/".to_owned(),
        };
        let (args, env) = publish_command(&t, Some("secret"), false);
        assert!(
            args.windows(2).any(|w| w == ["--registry", ADHOC_NAME]),
            "{args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "--index"),
            "--index would force --token"
        );
        assert!(
            !args.iter().any(|a| a.contains("secret")),
            "the token must never be an argument: {args:?}"
        );
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(
            keys.contains(&"CARGO_REGISTRIES_EIDE_TARGET_INDEX"),
            "{keys:?}"
        );
        assert!(
            keys.contains(&"CARGO_REGISTRIES_EIDE_TARGET_TOKEN"),
            "{keys:?}"
        );
    }

    /// Using cargo's own saved credentials means passing no token at all.
    #[test]
    fn no_token_means_no_token_variable() {
        let (_, env) = publish_command(&Target::CratesIo, None, false);
        assert!(env.is_empty(), "{env:?}");
        let t = Target::Custom {
            index: "sparse+https://x/".to_owned(),
        };
        let (_, env) = publish_command(&t, None, false);
        assert_eq!(env.len(), 1, "only the index: {env:?}");
    }

    /// A rehearsal tolerates a dirty tree. A real publish must not — it cannot
    /// be undone, and cargo refusing an uncommitted tree is a safety rail.
    #[test]
    fn allow_dirty_belongs_to_the_dry_run_only() {
        let (dry, _) = publish_command(&Target::CratesIo, None, true);
        assert!(dry.iter().any(|a| a == "--dry-run"));
        assert!(dry.iter().any(|a| a == "--allow-dirty"));
        let (real, _) = publish_command(&Target::CratesIo, None, false);
        assert!(!real.iter().any(|a| a == "--allow-dirty"), "{real:?}");
        assert!(!real.iter().any(|a| a == "--dry-run"), "{real:?}");
    }

    #[test]
    fn a_custom_target_is_blocked_until_the_url_is_real() {
        let empty = Target::Custom {
            index: String::new(),
        };
        assert!(target_blocker(&empty, None).is_some());
        let templated = Target::Custom {
            index: "sparse+https://cargo.cloudsmith.io/<owner>/<repository>/".to_owned(),
        };
        assert!(
            target_blocker(&templated, None).is_some_and(|m| m.contains("placeholders")),
            "an unfilled template must be refused"
        );
        let junk = Target::Custom {
            index: "my registry".to_owned(),
        };
        assert!(target_blocker(&junk, None).is_some());
        let ok = Target::Custom {
            index: "sparse+https://cargo.example.com/o/r/".to_owned(),
        };
        // An empty token field is not the same as "use saved credentials".
        assert!(target_blocker(&ok, Some("  ")).is_some());
        assert_eq!(target_blocker(&ok, Some("tok")), None);
        // …and "use saved credentials" cannot work for a TYPED index: the
        // ad-hoc registry name exists only in the child's environment, so
        // nothing has ever stored a credential under it. Without this, cargo
        // packaged and built the whole crate before failing at the upload.
        assert!(
            target_blocker(&ok, None).is_some_and(|m| m.contains("no saved credentials")),
            "a custom registry must ask for a token"
        );
        // A configured registry is different - `cargo login` can have stored
        // one under its real name.
        let cfg = Target::Configured {
            name: "my-reg".to_owned(),
            index: "sparse+https://x/".to_owned(),
        };
        assert_eq!(target_blocker(&cfg, None), None);
        assert_eq!(target_blocker(&Target::CratesIo, None), None);
    }
}

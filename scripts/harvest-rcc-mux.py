#!/usr/bin/env python3
"""Harvest embassy's per-peripheral clock mux tables from stm32-metapac.

The Clock tab lets a user pick a source for a peripheral's kernel clock
(USART1SEL, I2C1SEL, ADCSEL, ...). To emit that as embassy config we need three
names per selector, and NONE of them is guessable:

    config.rcc.mux.usart1sel = mux::Usart1sel::HSI;
                     ^field         ^enum      ^variant

The field is not the enum: on WBA, `usart2sel` and `usart3sel` both use
`Usartsel`, and `i2c2sel`/`i2c4sel` both use `I2c1sel`. Deriving one from the
other produces code that names a type which does not exist.

# Keyed by RCC VERSION, not by family

A family is not one register block. Measured across metapac's 1533 chips:
STM32H7 has four RCC versions, F0 and F3 have four each, C0/G0/H5/L0/WL two, and
even F4 has two — 143 parts on `f4` and six F410s on `f410`. Emitting a family's
"usual" table for the odd part out would name a field that chip does not have.

So the tables are keyed by RCC version, and a chip is resolved to its version by
part-number prefix — 153 rules, computed here as the SHORTEST prefix that is
unambiguous across every chip metapac knows. `FAMILY_RCC` is the fallback for a
part metapac has never heard of, and lists only families whose chips all share
one version.

    python scripts/harvest-rcc-mux.py [path-to-stm32-metapac]
"""

import glob
import os
import re
import sys

OUT = os.path.join("src", "panels", "mcu_module", "codegen", "rcc_mux_data.rs")


def find_metapac(argv):
    if len(argv) > 1:
        return argv[1]
    home = os.environ.get("USERPROFILE") or os.environ.get("HOME") or ""
    hits = sorted(glob.glob(os.path.join(home, ".cargo", "registry", "src", "*", "stm32-metapac-*")))
    if not hits:
        sys.exit("stm32-metapac not found in the cargo registry; pass its path")
    return hits[-1]


def enums_of(path):
    """enum name -> [(variant, value)] for one RCC register version."""
    text = open(path, encoding="utf-8", errors="replace").read()
    fields, enums = {}, {}
    for m in re.finditer(
        r'Enum \{\s*name: "([A-Za-z0-9_]+)",.*?variants: &\[(.*?)\n            \],',
        text,
        re.S,
    ):
        enums[m.group(1)] = re.findall(
            r'name: "([A-Za-z0-9_]+)",\s*description:.*?value: (\d+)', m.group(2), re.S
        )
    for m in re.finditer(
        r'Field \{\s*name: "([a-z0-9_]+)".*?enumm: Some\(\s*"([A-Za-z0-9_]+)",?\s*\)', text, re.S
    ):
        fields.setdefault(m.group(1), m.group(2))
    return fields, enums


def selectors(regs_dir, version, want_fields):
    """(field, enum, [(value, variant)]) for the fields this version's chips use.

    `want_fields` comes from the chips themselves — the `kernel_clock: Mux(...)`
    entries embassy's own build script reads. Filtering the register file by a
    name pattern instead would silently drop the older families: F0/F1/F3 call
    the selector `I2C1SW`, not `I2C1SEL`.
    """
    path = os.path.join(regs_dir, "rcc_" + version + ".rs")
    if not os.path.isfile(path):
        return []
    fields, enums = enums_of(path)
    out = []
    for field in sorted(want_fields):
        enum = fields.get(field)
        if not enum:
            continue
        # DISABLE is skipped by embassy's generator too — it is the "off" state,
        # not a clock source.
        variants = [(n, v) for (n, v) in enums.get(enum, []) if n != "DISABLE"]
        if variants:
            out.append((field, enum, variants))
    return out


def chip_versions(chips_dir):
    """chip -> (rcc version, family), plus version -> the mux fields its chips use."""
    cache, out = {}, {}
    muxes = {}
    for d in sorted(glob.glob(os.path.join(chips_dir, "stm32*"))):
        meta = os.path.join(d, "metadata.rs")
        if not os.path.isfile(meta):
            continue
        head = open(meta, encoding="utf-8", errors="replace").read()
        inc = re.search(r'include!\("\.\./(metadata_\d+)\.rs"\)', head)
        fam = re.search(r'family: "([^"]+)"', head)
        if not inc or not fam:
            continue
        shared = inc.group(1)
        if shared not in cache:
            p = os.path.join(chips_dir, shared + ".rs")
            version, fields = None, set()
            if os.path.isfile(p):
                body = open(p, encoding="utf-8", errors="replace").read()
                m = re.search(
                    r'name: "RCC",.*?kind: "rcc",\s*version: "([a-z0-9_]+)"', body, re.S
                )
                version = m.group(1) if m else None
                # THE authoritative list: what each peripheral says its kernel
                # clock selector is.
                fields = {
                    f.lower()
                    for f in re.findall(
                        r'kernel_clock: Mux\(PeripheralRccRegister \{\s*register: "[A-Z0-9_]+",\s*field: "([A-Z0-9_]+)"',
                        body,
                    )
                }
            cache[shared] = (version, fields)
        version, fields = cache[shared]
        if version:
            # The IDE's family key is the vendor string lowercased, exactly as
            # `stm32_pin_data::convert_xml` produces it — `STM32L4+` stays
            # `stm32l4+` rather than being tidied into `stm32l4`.
            out[os.path.basename(d)] = (version, fam.group(1).lower())
            muxes.setdefault(version, set()).update(fields)
    return out, muxes


def prefix_rules(chips):
    """Shortest part-number prefix that pins a chip's RCC version."""
    rules = {}
    for chip, (version, _) in chips.items():
        for n in range(6, len(chip) + 1):
            pre = chip[:n]
            if len({v for c, (v, _) in chips.items() if c.startswith(pre)}) == 1:
                rules[pre] = version
                break
    # Drop any rule a shorter one already implies.
    final = {}
    for pre in sorted(rules, key=len):
        if not any(pre.startswith(p) and final[p] == rules[pre] for p in final):
            final[pre] = rules[pre]
    return final


def family_rules(chips):
    """Families whose every chip shares one version — the fallback."""
    by_family = {}
    for _, (version, family) in chips.items():
        by_family.setdefault(family, set()).add(version)
    return {f: next(iter(v)) for f, v in by_family.items() if len(v) == 1}


def main():
    root = find_metapac(sys.argv)
    regs = os.path.join(root, "src", "registers")
    chips_dir = os.path.join(root, "src", "chips")
    if not os.path.isdir(regs) or not os.path.isdir(chips_dir):
        sys.exit("not a stm32-metapac checkout: " + root)

    chips, muxes = chip_versions(chips_dir)
    if not chips:
        sys.exit("no chip metadata found under " + chips_dir)

    versions = {}
    for name in sorted(muxes):
        found = selectors(regs, name, muxes[name])
        if found:
            versions[name] = found

    L = [
        "//! Per-peripheral clock mux tables, harvested from stm32-metapac.",
        "//!",
        "//! GENERATED by `scripts/harvest-rcc-mux.py` — do not edit by hand.",
        "//! Source: " + os.path.basename(root),
        "//!",
        "//! Keyed by RCC register VERSION, because a family is not one register",
        "//! block: STM32H7 has four, F0 and F3 four each, and even F4 has two",
        "//! (`f4`, and `f410` for six parts). A chip reaches its version through",
        "//! [`CHIP_RCC`] by part-number prefix; [`FAMILY_RCC`] is the fallback",
        "//! for a part metapac does not know, and only lists families whose",
        "//! chips all share one version.",
        "",
        "use super::rcc_mux::MuxField;",
        "",
    ]

    total = 0
    for version in sorted(versions):
        fields = versions[version]
        total += len(fields)
        L.append("/// `rcc_%s` — %d selectors." % (version, len(fields)))
        L.append("static V_%s: &[MuxField] = &[" % version.upper().replace("-", "_"))
        for field, enum, variants in fields:
            vs = ", ".join('(%s, "%s")' % (v, n) for (n, v) in variants)
            L.append(
                '    MuxField { field: "%s", enum_name: "%s", variants: &[%s] },'
                % (field, enum, vs)
            )
        L.append("];")
        L.append("")

    L.append("/// Every RCC version that exposes at least one selector.")
    L.append("pub static VERSIONS: &[(&str, &[MuxField])] = &[")
    for version in sorted(versions):
        L.append('    ("%s", V_%s),' % (version, version.upper().replace("-", "_")))
    L.append("];")
    L.append("")

    rules = prefix_rules(chips)
    L.append("/// Part-number prefix -> RCC version. Longest match wins, so the")
    L.append("/// order here does not matter; %d rules cover %d chips." % (len(rules), len(chips)))
    L.append("pub static CHIP_RCC: &[(&str, &str)] = &[")
    for pre in sorted(rules):
        L.append('    ("%s", "%s"),' % (pre, rules[pre]))
    L.append("];")
    L.append("")

    fams = family_rules(chips)
    L.append("/// Families whose chips all share one RCC version — the fallback")
    L.append("/// when a part number matches no prefix (a chip newer than this")
    L.append("/// harvest). Ambiguous families are absent ON PURPOSE.")
    L.append("pub static FAMILY_RCC: &[(&str, &str)] = &[")
    for fam in sorted(fams):
        L.append('    ("%s", "%s"),' % (fam, fams[fam]))
    L.append("];")
    L.append("")

    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(L))
    print(
        "wrote %s — %d versions, %d selectors, %d prefix rules, %d single-version families"
        % (OUT, len(versions), total, len(rules), len(fams))
    )


if __name__ == "__main__":
    main()

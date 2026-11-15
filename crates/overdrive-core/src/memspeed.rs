//! Memory speed inference.
//!
//! Windows only exposes SMBIOS data: `Speed` is usually the JEDEC speed (e.g. 2133/2666),
//! NOT the XMP/EXPO speed the kit is sold for. Without an SMBus driver (forbidden: no
//! kernel drivers) the only honest signal for the rated speed is the **part number**.
//! Everything inferred here is labelled `Inferred` by the profiler.
//!
//! Also: some platforms lock memory speed regardless of the kit (Intel ≤ 10th gen on
//! non-Z boards, 11th gen on H510). Recommending XMP there would be impossible advice.

/// Common DDR3/4/5 speed grades, ascending.
pub const STANDARD_MTS: &[u32] = &[
    1333, 1600, 1866, 2133, 2400, 2666, 2933, 3000, 3200, 3333, 3466, 3600, 3733, 3866, 4000, 4133,
    4266, 4400, 4800, 5200, 5600, 6000, 6200, 6400, 6600, 6800, 7200, 7600, 8000,
];

/// Two-digit speed codes used by Kingston (KF432…, KVR26…) and Crucial Ballistix (BL…32C16).
fn code2(code: u32) -> Option<u32> {
    Some(match code {
        13 => 1333,
        16 => 1600,
        18 => 1866,
        21 => 2133,
        24 => 2400,
        26 => 2666,
        29 => 2933,
        30 => 3000,
        32 => 3200,
        34 => 3466,
        36 => 3600,
        37 => 3733,
        38 => 3866,
        40 => 4000,
        42 => 4266,
        44 => 4400,
        48 => 4800,
        52 => 5200,
        56 => 5600,
        60 => 6000,
        62 => 6200,
        64 => 6400,
        66 => 6600,
        68 => 6800,
        72 => 7200,
        76 => 7600,
        80 => 8000,
        _ => return None,
    })
}

fn digits_at(b: &[u8], i: usize, n: usize) -> Option<u32> {
    if i + n > b.len() || !b[i..i + n].iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(&b[i..i + n]).ok()?.parse().ok()
}

/// Rated (XMP/EXPO) speed encoded in a retail part number, if recognisable.
///
/// Recognised: Corsair `CMK16GX4M2B3200C16`, G.Skill `F4-3200C16-8GVKB` / `F5-6000J3038F16G`,
/// TeamGroup `TF3D48G3200HC16C01`, Kingston Fury/HyperX `KF432C16BB/8` / `HX436C17PB3/8` /
/// `KF556C40BBK2-32`, Kingston ValueRAM `KVR26N19S8/8`, Crucial Ballistix `BL8G32C16U4B`.
/// OEM parts (Samsung `M378A…-CTD`, Hynix `HMA…-VK`, Micron `MTA…`) return `None`:
/// they are JEDEC parts and SMBIOS `Speed` is already correct for them.
pub fn rated_from_part_number(part: &str) -> Option<u32> {
    let p = part.trim().to_ascii_uppercase();
    let b = p.as_bytes();
    if b.is_empty() {
        return None;
    }
    // Kingston Fury / HyperX: KF4|KF5|HX4|HX3 + 2-digit code + 'C'
    for prefix in ["KF", "HX"] {
        if let Some(rest) = p.strip_prefix(prefix) {
            let rb = rest.as_bytes();
            // 'C' = desktop DIMM (KF432C16…), 'S' = SO-DIMM (Fury Impact KF432S20…)
            if rb.len() >= 4
                && matches!(rb[0], b'3' | b'4' | b'5')
                && matches!(rb.get(3), Some(b'C' | b'S'))
            {
                if let Some(v) = digits_at(rb, 1, 2).and_then(code2) {
                    return Some(v);
                }
            }
        }
    }
    // Kingston ValueRAM: KVR + 2-digit code
    if let Some(rest) = p.strip_prefix("KVR") {
        if let Some(v) = digits_at(rest.as_bytes(), 0, 2).and_then(code2) {
            return Some(v);
        }
    }
    // Crucial Ballistix: BL<cap>G<code>C…
    if p.starts_with("BL") {
        if let Some(g) = p.find('G') {
            if let Some(v) = digits_at(b, g + 1, 2)
                .filter(|_| b.get(g + 3) == Some(&b'C'))
                .and_then(code2)
            {
                return Some(v);
            }
        }
    }
    // Generic: a standard 4-digit speed followed by a latency marker (C, J, HC, CL) or '-'.
    let mut i = 0;
    while i + 4 <= b.len() {
        let boundary_before = i == 0 || !b[i - 1].is_ascii_digit();
        if boundary_before {
            if let Some(v) = digits_at(b, i, 4) {
                let after = b.get(i + 4).copied();
                let marker = matches!(after, Some(b'C' | b'J' | b'H'));
                if marker && STANDARD_MTS.contains(&v) {
                    return Some(v);
                }
            }
        }
        i += 1;
    }
    None
}

/// The next standard grade strictly below `mts` — "one step below" for stability.
pub fn step_below(mts: u32) -> Option<u32> {
    STANDARD_MTS.iter().rev().copied().find(|&s| s < mts)
}

/// Nearest standard grade (SMBIOS sometimes reports 3192 for 3200, 2667 for 2666).
pub fn snap(mts: u32) -> u32 {
    *STANDARD_MTS
        .iter()
        .min_by_key(|&&s| (s as i64 - mts as i64).abs())
        .filter(|&&s| (s as i64 - mts as i64).abs() <= 40)
        .unwrap_or(&mts)
}

/// Memory channel letter from a DIMM locator/bank label ("ChannelA-DIMM0", "DIMM_B1",
/// "A1_DIMM0", "P0 CHANNEL A"). `None` when the label doesn't encode it.
pub fn channel_from_locator(locator: &str, bank: &str) -> Option<char> {
    for raw in [locator, bank] {
        let s: String = raw
            .to_ascii_uppercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        if let Some(pos) = s.find("CHANNEL") {
            if let Some(c) = s[pos + 7..]
                .chars()
                .next()
                .filter(|c| c.is_ascii_alphabetic())
            {
                return Some(c);
            }
        }
        // DIMM_A1 / DIMMA1
        if let Some(pos) = s.find("DIMM") {
            let tail: Vec<char> = s[pos + 4..].chars().collect();
            if tail.len() >= 2 && tail[0].is_ascii_alphabetic() && tail[1].is_ascii_digit() {
                return Some(tail[0]);
            }
        }
        // A1_DIMM0 / B2DIMM1
        let chars: Vec<char> = s.chars().collect();
        if chars.len() >= 6
            && chars[0].is_ascii_alphabetic()
            && chars[1].is_ascii_digit()
            && s[2..].starts_with("DIMM")
        {
            return Some(chars[0]);
        }
    }
    None
}

/// Channel count inference. Returns `(channels, reason)`.
pub fn infer_channels(locators: &[(String, String)]) -> Option<u8> {
    match locators.len() {
        0 => None,
        1 => Some(1),
        _ => {
            let letters: Vec<char> = locators
                .iter()
                .filter_map(|(l, b)| channel_from_locator(l, b))
                .collect();
            if letters.len() != locators.len() {
                return None;
            }
            let mut uniq = letters.clone();
            uniq.sort_unstable();
            uniq.dedup();
            Some(uniq.len().min(4) as u8)
        }
    }
}

/// Intel generation + tier from a CPU brand string: "Intel(R) Core(TM) i3-10100F" → (10, 3, "F").
pub fn intel_core_model(name: &str) -> Option<(u32, u32, String)> {
    let up = name.to_ascii_uppercase();
    let b = up.as_bytes();
    let mut i = 0;
    while i + 3 < b.len() {
        if b[i] == b'I' && matches!(b[i + 1], b'3' | b'5' | b'7' | b'9') && b[i + 2] == b'-' {
            let tier = (b[i + 1] - b'0') as u32;
            let start = i + 3;
            let mut end = start;
            while end < b.len() && b[end].is_ascii_digit() {
                end += 1;
            }
            let digits = &up[start..end];
            let mut sfx_end = end;
            while sfx_end < b.len() && b[sfx_end].is_ascii_alphabetic() {
                sfx_end += 1;
            }
            let suffix = up[end..sfx_end].to_string();
            let gen = match digits.len() {
                5 => digits[..2].parse().ok()?,
                4 => digits[..1].parse().ok()?,
                _ => return None,
            };
            return Some((gen, tier, suffix));
        }
        i += 1;
    }
    None
}

/// AMD Ryzen series digit from "AMD Ryzen 5 5600X" → 5; "Ryzen 3 3200G" → 3.
pub fn ryzen_series(name: &str) -> Option<u32> {
    let up = name.to_ascii_uppercase();
    let pos = up.find("RYZEN")?;
    up[pos..]
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 4 && t.as_bytes()[..4].iter().all(u8::is_ascii_digit))
        .find_map(|t| t[..1].parse().ok())
}

/// Chipset from a board product name: "B460M PRO-VDH WIFI (MS-7C83)" → "B460".
pub fn chipset_from_board(product: &str) -> Option<String> {
    product
        .to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find_map(|t| {
            let b = t.as_bytes();
            (b.len() >= 4
                && matches!(b[0], b'A' | b'B' | b'H' | b'Q' | b'X' | b'Z' | b'W')
                && b[1..4].iter().all(u8::is_ascii_digit))
            .then(|| t[..4].to_string())
        })
}

/// Memory speed cap imposed by an Intel desktop CPU on a board that doesn't allow memory
/// overclocking. `None` = no known cap (or not enough information to claim one).
pub fn intel_platform_cap(gen: u32, tier: u32, suffix: &str, chipset: Option<&str>) -> Option<u32> {
    let mobile = suffix.contains('H')
        || suffix.contains('U')
        || suffix.contains('P')
        || suffix.contains('Y');
    if mobile {
        return None; // laptops are handled separately (BIOS rarely exposes XMP)
    }
    let chip = chipset?;
    let oc_board = chip.starts_with('Z') || chip.starts_with('W') || chip.starts_with('X');
    match gen {
        6..=10 if !oc_board => Some(match gen {
            6 => 2133,
            7 => 2400,
            8 | 9 => {
                if tier == 3 {
                    2400
                } else {
                    2666
                }
            }
            _ => {
                if tier <= 5 {
                    2666
                } else {
                    2933
                }
            }
        }),
        11 if chip.starts_with("H51") => Some(3200),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_numbers() {
        let cases = [
            ("CMK16GX4M2B3200C16", Some(3200)),
            ("CMT32GX5M2B5600C36", Some(5600)),
            ("F4-3200C16-8GVKB", Some(3200)),
            ("F4-3600C18-16GVK", Some(3600)),
            ("F5-6000J3038F16G", Some(6000)),
            ("TF3D48G3200HC16C01", Some(3200)),
            ("KF432C16BB/8", Some(3200)),
            ("KF556C40BBK2-32", Some(5600)),
            ("KF432S20IB/8", Some(3200)),
            ("HX436C17PB3/8", Some(3600)),
            ("KVR26N19S8/8", Some(2666)),
            ("BL8G32C16U4B.M8FE1", Some(3200)),
            ("M378A1K43CB2-CTD", None),
            ("HMA81GU6CJR8N-VK", None),
            ("", None),
            ("   CMK16GX4M2B3200C16   ", Some(3200)),
        ];
        for (p, want) in cases {
            assert_eq!(rated_from_part_number(p), want, "{p}");
        }
    }

    #[test]
    fn channels() {
        let l = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(infer_channels(&[l("DIMM_A1", "")]), Some(1));
        assert_eq!(
            infer_channels(&[l("DIMM_A2", "BANK 0"), l("DIMM_B2", "BANK 1")]),
            Some(2)
        );
        assert_eq!(
            infer_channels(&[l("ChannelA-DIMM0", "BANK 0"), l("ChannelB-DIMM0", "BANK 2")]),
            Some(2)
        );
        assert_eq!(
            infer_channels(&[l("A1_DIMM0", ""), l("A1_DIMM1", "")]),
            Some(1)
        );
        assert_eq!(
            infer_channels(&[l("P0 CHANNEL A", ""), l("P0 CHANNEL B", "")]),
            Some(2)
        );
        assert_eq!(
            infer_channels(&[l("DIMM 0", "BANK 0"), l("DIMM 1", "BANK 1")]),
            None
        );
        assert_eq!(infer_channels(&[]), None);
    }

    #[test]
    fn cpu_and_board_parsing() {
        assert_eq!(
            intel_core_model("Intel(R) Core(TM) i3-10100F CPU @ 3.60GHz"),
            Some((10, 3, "F".into()))
        );
        assert_eq!(
            intel_core_model("Intel(R) Core(TM) i7-8700K CPU @ 3.70GHz"),
            Some((8, 7, "K".into()))
        );
        assert_eq!(
            intel_core_model("13th Gen Intel(R) Core(TM) i5-13400F"),
            Some((13, 5, "F".into()))
        );
        assert_eq!(
            intel_core_model("Intel(R) Core(TM) i7-10750H CPU @ 2.60GHz"),
            Some((10, 7, "H".into()))
        );
        assert_eq!(intel_core_model("AMD Ryzen 5 5600X 6-Core Processor"), None);
        assert_eq!(
            ryzen_series("AMD Ryzen 5 5600H with Radeon Graphics"),
            Some(5)
        );
        assert_eq!(
            ryzen_series("AMD Ryzen 3 3200G with Radeon Vega Graphics"),
            Some(3)
        );
        assert_eq!(
            chipset_from_board("B460M PRO-VDH WIFI (MS-7C83)").as_deref(),
            Some("B460")
        );
        assert_eq!(
            chipset_from_board("TUF GAMING X570-PLUS (WI-FI)").as_deref(),
            Some("X570")
        );
        assert_eq!(chipset_from_board("PRIME A320M-K").as_deref(), Some("A320"));
        assert_eq!(chipset_from_board("LNVNB161216"), None);
    }

    #[test]
    fn platform_caps() {
        assert_eq!(intel_platform_cap(10, 3, "F", Some("B460")), Some(2666));
        assert_eq!(intel_platform_cap(10, 7, "", Some("H470")), Some(2933));
        assert_eq!(intel_platform_cap(10, 5, "", Some("Z490")), None);
        assert_eq!(
            intel_platform_cap(10, 5, "", None),
            None,
            "no chipset → don't claim a cap"
        );
        assert_eq!(intel_platform_cap(12, 5, "F", Some("B660")), None);
        assert_eq!(intel_platform_cap(11, 5, "", Some("H510")), Some(3200));
        assert_eq!(
            intel_platform_cap(10, 7, "H", Some("HM470")),
            None,
            "mobile"
        );
    }

    #[test]
    fn steps() {
        assert_eq!(step_below(3200), Some(3000));
        assert_eq!(step_below(3600), Some(3466));
        assert_eq!(snap(3192), 3200);
        assert_eq!(snap(2667), 2666);
        assert_eq!(snap(1234), 1234);
    }
}

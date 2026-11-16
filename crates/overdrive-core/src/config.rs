//! Game config text editing (goals.md §5.5): change only the keys we were asked to, keep
//! every other byte exactly as it was.
//!
//! - Encoding is detected from the BOM (UTF-8, UTF-8 BOM, UTF-16 LE/BE) and written back the
//!   same way. Anything that isn't valid text in that encoding is refused, never "fixed".
//! - Line endings are kept per line; new lines use the file's dominant ending.
//! - Only the value part of an existing line changes (`key = value` keeps its spacing).
//! - Formats: `ini` (`[Section]` + `key=value`, Unreal/most PC games) and `colon`
//!   (`key:value`, no sections — Minecraft `options.txt`).
//!
//! Pure functions over bytes: the engine computes `patch(original)` again at revert time
//! to know whether the file is still exactly what we wrote.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFormat {
    Ini,
    Colon,
}

/// Set `key` (inside `section` for ini) to `value`; added if missing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Enc {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("the file is not valid text in its encoding")]
    Encoding,
    #[error("edit without a section in an ini file: {0}")]
    MissingSection(String),
}

fn decode(bytes: &[u8]) -> Result<(Enc, String), ConfigError> {
    let utf16 = |b: &[u8], le: bool| -> Result<String, ConfigError> {
        if b.len() % 2 != 0 {
            return Err(ConfigError::Encoding);
        }
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16(&units).map_err(|_| ConfigError::Encoding)
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => std::str::from_utf8(rest)
            .map(|s| (Enc::Utf8Bom, s.to_string()))
            .map_err(|_| ConfigError::Encoding),
        [0xFF, 0xFE, rest @ ..] => Ok((Enc::Utf16Le, utf16(rest, true)?)),
        [0xFE, 0xFF, rest @ ..] => Ok((Enc::Utf16Be, utf16(rest, false)?)),
        _ => std::str::from_utf8(bytes)
            .map(|s| (Enc::Utf8, s.to_string()))
            .map_err(|_| ConfigError::Encoding),
    }
}

fn encode(enc: Enc, text: &str) -> Vec<u8> {
    match enc {
        Enc::Utf8 => text.as_bytes().to_vec(),
        Enc::Utf8Bom => [&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat(),
        Enc::Utf16Le => [0xFF, 0xFE]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect(),
        Enc::Utf16Be => [0xFE, 0xFF]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
            .collect(),
    }
}

/// One physical line: content + its own terminator ("", "\n" or "\r\n").
struct Line<'a> {
    body: &'a str,
    end: &'a str,
}

fn lines(text: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find('\n') {
            Some(i) => {
                let (body, end) = if i > 0 && rest.as_bytes()[i - 1] == b'\r' {
                    (&rest[..i - 1], &rest[i - 1..=i])
                } else {
                    (&rest[..i], &rest[i..=i])
                };
                out.push(Line { body, end });
                rest = &rest[i + 1..];
            }
            None => {
                out.push(Line { body: rest, end: "" });
                rest = "";
            }
        }
    }
    out
}

fn dominant_eol(ls: &[Line]) -> &'static str {
    let crlf = ls.iter().filter(|l| l.end == "\r\n").count();
    let lf = ls.iter().filter(|l| l.end == "\n").count();
    if crlf >= lf && crlf > 0 {
        "\r\n"
    } else {
        "\n"
    }
}

fn sep(format: ConfigFormat) -> char {
    match format {
        ConfigFormat::Ini => '=',
        ConfigFormat::Colon => ':',
    }
}

fn section_header(body: &str) -> Option<&str> {
    let t = body.trim();
    (t.starts_with('[') && t.ends_with(']') && t.len() >= 2).then(|| t[1..t.len() - 1].trim())
}

/// `(key, byte offset where the value starts)` for a `key<sep>value` line.
fn key_of(body: &str, format: ConfigFormat) -> Option<(&str, usize)> {
    let t = body.trim_start();
    if t.is_empty() || t.starts_with(';') || t.starts_with('#') || t.starts_with("//") {
        return None;
    }
    let i = body.find(sep(format))?;
    let key = body[..i].trim();
    if key.is_empty() {
        return None;
    }
    // Keep the whitespace right after the separator ("key = value").
    let after = &body[i + 1..];
    let ws = after.len() - after.trim_start().len();
    Some((key, i + 1 + ws))
}

/// Which section each line belongs to (`None` = before any header / colon format).
fn sections<'a>(ls: &[Line<'a>], format: ConfigFormat) -> Vec<Option<&'a str>> {
    let mut cur = None;
    ls.iter()
        .map(|l| {
            if format == ConfigFormat::Ini {
                if let Some(h) = section_header(l.body) {
                    cur = Some(h);
                }
            }
            cur
        })
        .collect()
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn value_trimmed(body: &str, at: usize) -> &str {
    body[at..].trim_end()
}

/// Current value of each edit's key (`None` = not present). Values are compared trimmed.
pub fn read(bytes: &[u8], format: ConfigFormat, edits: &[ConfigEdit]) -> Result<Vec<Option<String>>, ConfigError> {
    let (_, text) = decode(bytes)?;
    let ls = lines(&text);
    let secs = sections(&ls, format);
    Ok(edits
        .iter()
        .map(|e| {
            ls.iter()
                .zip(&secs)
                .filter(|(_, s)| section_matches(format, **s, e))
                .filter_map(|(l, _)| key_of(l.body, format).filter(|(k, _)| same(k, &e.key)).map(|(_, at)| value_trimmed(l.body, at).to_string()))
                .next_back() // last occurrence wins, like the games' own parsers
        })
        .collect())
}

fn section_matches(format: ConfigFormat, line_section: Option<&str>, e: &ConfigEdit) -> bool {
    match format {
        ConfigFormat::Colon => true,
        ConfigFormat::Ini => match (&e.section, line_section) {
            (Some(want), Some(have)) => same(want, have),
            _ => false,
        },
    }
}

/// Do all edits hold in `bytes`?
pub fn holds(bytes: &[u8], format: ConfigFormat, edits: &[ConfigEdit]) -> Result<bool, ConfigError> {
    Ok(read(bytes, format, edits)?
        .iter()
        .zip(edits)
        .all(|(v, e)| v.as_deref() == Some(e.value.trim())))
}

/// Apply the edits. Deterministic: same input → same output, byte for byte.
/// Every untouched line (and the encoding, BOM and line endings) is preserved exactly.
pub fn patch(bytes: &[u8], format: ConfigFormat, edits: &[ConfigEdit]) -> Result<Vec<u8>, ConfigError> {
    let (enc, text) = if bytes.is_empty() { (Enc::Utf8, String::new()) } else { decode(bytes)? };
    if format == ConfigFormat::Ini {
        if let Some(e) = edits.iter().find(|e| e.section.is_none()) {
            return Err(ConfigError::MissingSection(e.key.clone()));
        }
    }
    let ls = lines(&text);
    let eol = dominant_eol(&ls);
    let secs = sections(&ls, format);
    let s = sep(format);

    // Replacement bodies for existing lines + new lines to insert after a given index.
    let mut bodies: Vec<String> = ls.iter().map(|l| l.body.to_string()).collect();
    let mut inserts: Vec<(Option<usize>, String)> = Vec::new(); // (after line idx, body)
    let mut new_sections: Vec<(String, Vec<String>)> = Vec::new();

    for e in edits {
        let hits: Vec<usize> = (0..ls.len())
            .filter(|&i| section_matches(format, secs[i], e))
            .filter(|&i| key_of(ls[i].body, format).is_some_and(|(k, _)| same(k, &e.key)))
            .collect();
        if !hits.is_empty() {
            for i in hits {
                let (_, at) = key_of(ls[i].body, format).expect("matched above");
                let trailing = &ls[i].body[ls[i].body.trim_end().len()..];
                bodies[i] = format!("{}{}{}", &ls[i].body[..at], e.value, trailing);
            }
            continue;
        }
        let line = format!("{}{s}{}", e.key, e.value);
        match format {
            ConfigFormat::Colon => inserts.push((ls.len().checked_sub(1), line)),
            ConfigFormat::Ini => {
                let want = e.section.as_deref().expect("checked above");
                // Last non-blank line of the (last) matching section.
                let in_sec: Vec<usize> = (0..ls.len()).filter(|&i| secs[i].is_some_and(|h| same(h, want))).collect();
                match in_sec.iter().rev().find(|&&i| !ls[i].body.trim().is_empty()) {
                    Some(&i) => inserts.push((Some(i), line)),
                    None => match new_sections.iter_mut().find(|(n, _)| same(n, want)) {
                        Some((_, v)) => v.push(line),
                        None => new_sections.push((want.to_string(), vec![line])),
                    },
                }
            }
        }
    }

    let mut out = String::with_capacity(text.len() + 64);
    let push_inserts = |out: &mut String, after: Option<usize>| {
        for (_, b) in inserts.iter().filter(|(a, _)| *a == after) {
            out.push_str(b);
            out.push_str(eol);
        }
    };
    push_inserts(&mut out, None);
    for (i, l) in ls.iter().enumerate() {
        out.push_str(&bodies[i]);
        let has_inserts = inserts.iter().any(|(a, _)| *a == Some(i));
        // A last line without terminator gets one only if something follows it.
        if l.end.is_empty() && (has_inserts || !new_sections.is_empty()) {
            out.push_str(eol);
        } else {
            out.push_str(l.end);
        }
        push_inserts(&mut out, Some(i));
    }
    for (name, kv) in &new_sections {
        if !out.is_empty() && !out.ends_with(&format!("{eol}{eol}")) {
            out.push_str(eol);
        }
        out.push_str(&format!("[{name}]{eol}"));
        for b in kv {
            out.push_str(b);
            out.push_str(eol);
        }
    }
    Ok(encode(enc, &out))
}

/// Remove every line that defines one of these keys (values ignored). Used at revert time
/// for keys that did not exist before we added them.
pub fn remove(bytes: &[u8], format: ConfigFormat, keys: &[ConfigEdit]) -> Result<Vec<u8>, ConfigError> {
    let (enc, text) = decode(bytes)?;
    let ls = lines(&text);
    let secs = sections(&ls, format);
    let mut out = String::with_capacity(text.len());
    for (i, l) in ls.iter().enumerate() {
        let drop = keys.iter().any(|e| {
            section_matches(format, secs[i], e) && key_of(l.body, format).is_some_and(|(k, _)| same(k, &e.key))
        });
        if !drop {
            out.push_str(l.body);
            out.push_str(l.end);
        }
    }
    Ok(encode(enc, &out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(section: Option<&str>, key: &str, value: &str) -> ConfigEdit {
        ConfigEdit { section: section.map(String::from), key: key.into(), value: value.into() }
    }

    #[test]
    fn colon_replaces_value_and_keeps_everything_else() {
        let src = b"version:3955\nmaxFps:260\nenableVsync:true\nrenderDistance:12\n";
        let out = patch(src, ConfigFormat::Colon, &[ed(None, "maxFps", "120")]).unwrap();
        assert_eq!(out, b"version:3955\nmaxFps:120\nenableVsync:true\nrenderDistance:12\n");
        assert!(holds(&out, ConfigFormat::Colon, &[ed(None, "maxFps", "120")]).unwrap());
    }

    #[test]
    fn ini_keeps_crlf_spacing_bom_and_adds_missing_keys_in_the_right_section() {
        let src = "\u{feff}[/Script/Engine.GameUserSettings]\r\nbUseVSync = True\r\nFrameRateLimit=0.000000\r\n\r\n[ScalabilityGroups]\r\nsg.ShadowQuality=3\r\n";
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(src.trim_start_matches('\u{feff}').as_bytes());
        let edits = [
            ed(Some("/Script/Engine.GameUserSettings"), "bUseVSync", "False"),
            ed(Some("/script/engine.gameusersettings"), "FrameRateLimit", "117.000000"),
            ed(Some("ScalabilityGroups"), "sg.EffectsQuality", "2"),
            ed(Some("SystemSettings"), "r.MotionBlurQuality", "0"),
        ];
        let out = patch(&bytes, ConfigFormat::Ini, &edits).unwrap();
        let want = "\u{feff}[/Script/Engine.GameUserSettings]\r\nbUseVSync = False\r\nFrameRateLimit=117.000000\r\n\r\n[ScalabilityGroups]\r\nsg.ShadowQuality=3\r\nsg.EffectsQuality=2\r\n\r\n[SystemSettings]\r\nr.MotionBlurQuality=0\r\n";
        assert_eq!(String::from_utf8(out.clone()).unwrap(), want);
        assert!(holds(&out, ConfigFormat::Ini, &edits).unwrap());
        // Deterministic and idempotent.
        assert_eq!(patch(&bytes, ConfigFormat::Ini, &edits).unwrap(), out);
        assert_eq!(patch(&out, ConfigFormat::Ini, &edits).unwrap(), out);
    }

    #[test]
    fn same_key_in_another_section_is_not_touched() {
        let src = b"[A]\nk=1\n[B]\nk=1\n";
        let out = patch(src, ConfigFormat::Ini, &[ed(Some("B"), "k", "2")]).unwrap();
        assert_eq!(out, b"[A]\nk=1\n[B]\nk=2\n");
    }

    #[test]
    fn utf16_round_trips_and_invalid_text_is_refused() {
        let text = "[S]\r\nk=1\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let out = patch(&bytes, ConfigFormat::Ini, &[ed(Some("S"), "k", "0")]).unwrap();
        assert_eq!(&out[..2], &[0xFF, 0xFE]);
        assert_eq!(read(&out, ConfigFormat::Ini, &[ed(Some("S"), "k", "")]).unwrap(), vec![Some("0".into())]);
        assert_eq!(patch(&[0x61, 0xFF, 0x0A], ConfigFormat::Colon, &[]), Err(ConfigError::Encoding));
    }

    #[test]
    fn missing_final_newline_and_empty_file() {
        let out = patch(b"a:1", ConfigFormat::Colon, &[ed(None, "b", "2")]).unwrap();
        assert_eq!(out, b"a:1\nb:2\n");
        let out = patch(b"", ConfigFormat::Ini, &[ed(Some("S"), "k", "1")]).unwrap();
        assert_eq!(out, b"[S]\nk=1\n");
        // Nothing to change → identical bytes.
        assert_eq!(patch(b"a:1", ConfigFormat::Colon, &[ed(None, "a", "1")]).unwrap(), b"a:1");
    }

    #[test]
    fn comments_are_never_keys() {
        let src = b"[S]\n; k=5\nk=1\n";
        assert_eq!(read(src, ConfigFormat::Ini, &[ed(Some("S"), "k", "")]).unwrap(), vec![Some("1".into())]);
        let out = patch(src, ConfigFormat::Ini, &[ed(Some("S"), "k", "0")]).unwrap();
        assert_eq!(out, b"[S]\n; k=5\nk=0\n");
    }
}

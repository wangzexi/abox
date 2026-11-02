use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};

// filename and UTF-8 filename* follow the parameter syntax in RFC 6266/8187.
pub fn filename(header: &str) -> Result<Option<String>, &'static str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in header.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&header[start..i]);
                start = i + 1;
            }
            _ => (),
        }
    }
    if quoted || escaped {
        return Err("Use a complete quoted filename.");
    }
    parts.push(&header[start..]);
    let mut plain = None;
    let mut extended = None;
    for part in parts.into_iter().skip(1) {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !key.eq_ignore_ascii_case("filename") && !key.eq_ignore_ascii_case("filename*") {
            continue;
        }
        let raw = value.trim();
        let value = if raw.starts_with('"') {
            let inner = raw
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .ok_or("Use a complete quoted filename.")?;
            let mut result = String::new();
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    result.push(chars.next().ok_or("Use a complete quoted filename.")?);
                } else {
                    result.push(c);
                }
            }
            result
        } else {
            raw.to_owned()
        };
        if key.eq_ignore_ascii_case("filename*") {
            if extended.is_some() {
                return Err("Supply filename* once.");
            }
            let mut fields = value.splitn(3, '\'');
            if !fields.next().unwrap_or("").eq_ignore_ascii_case("UTF-8") {
                return Err("Use UTF-8 for filename*.");
            }
            fields.next().ok_or("Use filename*=UTF-8''encoded-name.")?;
            let encoded = fields.next().ok_or("Use filename*=UTF-8''encoded-name.")?;
            let bytes = encoded.as_bytes();
            for (i, &b) in bytes.iter().enumerate() {
                if b == b'%'
                    && (i + 2 >= bytes.len()
                        || !bytes[i + 1].is_ascii_hexdigit()
                        || !bytes[i + 2].is_ascii_hexdigit())
                {
                    return Err("Use percent-encoded UTF-8 in filename*.");
                }
            }
            extended = Some(
                percent_decode_str(encoded)
                    .decode_utf8()
                    .map_err(|_| "Use valid UTF-8 in filename*.")?
                    .into_owned(),
            );
        } else {
            if plain.is_some() {
                return Err("Supply filename once.");
            }
            plain = Some(value);
        }
    }
    let result = extended.or(plain);
    if let Some(name) = &result {
        if name.is_empty()
            || name.len() > 255
            || name == "."
            || name == ".."
            || name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
        {
            return Err(
                "Supply a filename of 1–255 UTF-8 bytes using a base name, for example photo.png.",
            );
        }
    }
    Ok(result)
}

pub fn response_header(kind: &str, name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " ._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "{kind}; filename=\"{fallback}\"; filename*=UTF-8''{}",
        utf8_percent_encode(name, NON_ALPHANUMERIC)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_names_with_semicolons_quotes_and_unicode() {
        assert_eq!(
            filename(r#"attachment; filename="photo; 1.png""#)
                .unwrap()
                .as_deref(),
            Some("photo; 1.png")
        );
        assert_eq!(
            filename(r#"attachment; filename="say\"hi.txt""#)
                .unwrap()
                .as_deref(),
            Some("say\"hi.txt")
        );
        assert_eq!(
            filename("attachment; filename=photo.png; filename*=UTF-8''%E7%85%A7%E7%89%87.png")
                .unwrap()
                .as_deref(),
            Some("照片.png")
        );
        assert_eq!(filename("attachment").unwrap(), None);
    }
    #[test]
    fn rejects_paths_controls_and_bad_encoding() {
        for value in [
            "attachment; filename=../file",
            "attachment; filename*=UTF-8''%0A",
            "attachment; filename*=UTF-8''%FF",
            "attachment; filename*=UTF-8''%XX",
            "attachment; filename=\"unterminated",
            "attachment; filename=a; filename=b",
        ] {
            assert!(filename(value).is_err(), "{value}");
        }
    }
    #[test]
    fn response_filename_roundtrips() {
        for name in ["photo.png", "照片 1.png", "semi; quote\".txt"] {
            let header = response_header("attachment", name);
            assert!(header.is_ascii());
            assert_eq!(filename(&header).unwrap().as_deref(), Some(name));
        }
    }
}

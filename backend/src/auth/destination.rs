use super::AuthError;
use url::Url;

// Form decoding must be strict: the URL library's form parser replaces invalid UTF-8.
fn decode(value: &str) -> Result<String, AuthError> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        bytes.push(match byte {
            b'+' => b' ',
            b'%' => {
                let hi = input.next().and_then(|b| (b as char).to_digit(16));
                let lo = input.next().and_then(|b| (b as char).to_digit(16));
                (hi.ok_or(AuthError::BadRequest)? * 16 + lo.ok_or(AuthError::BadRequest)?) as u8
            }
            _ => byte,
        });
    }
    String::from_utf8(bytes).map_err(|_| AuthError::BadRequest)
}

pub fn parse(query: Option<&str>) -> Result<String, AuthError> {
    let mut destination = None;
    for pair in query
        .unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
    {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode(key)?;
        let value = decode(value)?;
        if key == "return_to" {
            if destination.is_some() {
                return Err(AuthError::BadRequest);
            }
            destination = Some(normalize(&value)?);
        }
    }
    Ok(destination.unwrap_or_else(|| "/".into()))
}

fn normalize(value: &str) -> Result<String, AuthError> {
    if value.len() > 2048
        || !value.starts_with('/')
        || value.starts_with("//")
        || value
            .chars()
            .any(|c| c == '\\' || c.is_control() || c.is_whitespace())
    {
        return Err(AuthError::BadRequest);
    }
    let path = value.split(['?', '#']).next().unwrap_or_default();
    // Validate escapes everywhere, but only the path is subject to separator restrictions.
    let _ = decode(value)?;
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let hi = bytes
                .next()
                .and_then(|b| (b as char).to_digit(16))
                .ok_or(AuthError::BadRequest)?;
            let lo = bytes
                .next()
                .and_then(|b| (b as char).to_digit(16))
                .ok_or(AuthError::BadRequest)?;
            let decoded = (hi * 16 + lo) as u8;
            if matches!(decoded, b'/' | b'\\' | b'%')
                || decoded.is_ascii_control()
                || decoded.is_ascii_whitespace()
            {
                return Err(AuthError::BadRequest);
            }
        }
    }
    let base = Url::parse("https://destination.invalid/").expect("fixed origin");
    let parsed = base.join(value).map_err(|_| AuthError::BadRequest)?;
    if parsed.origin() != base.origin() || parsed.path().starts_with("//") {
        return Err(AuthError::BadRequest);
    }
    let relative = &parsed[url::Position::BeforePath..];
    // Re-parsing the emitted reference must not reinterpret a normalized path as authority.
    if base
        .join(relative)
        .map_err(|_| AuthError::BadRequest)?
        .origin()
        != base.origin()
    {
        return Err(AuthError::BadRequest);
    }
    Ok(relative.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_local_destinations() {
        assert_eq!(parse(None).unwrap(), "/");
        assert_eq!(parse(Some("unrelated=value")).unwrap(), "/");
        for (input, expected) in [
            ("/", "/"),
            ("/lobbies/create", "/lobbies/create"),
            (
                "/a/../b?next=https%3A%2F%2Fexample.com#part",
                "/b?next=https%3A%2F%2Fexample.com#part",
            ),
            ("/a/%2e%2e/b", "/b"),
            ("/x?q=%252F#%2F", "/x?q=%252F#%2F"),
        ] {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("return_to", input)
                .finish();
            assert_eq!(parse(Some(&query)).unwrap(), expected);
        }
        for input in [
            "",
            "https://evil.test/",
            "//evil.test",
            "/\\evil",
            "/a b",
            "/a\n",
            "/%2fhost",
            "/%5Chost",
            "/%00",
            "/%7f",
            "/%20",
            "/%252f",
            "/a/..//evil.test",
            "/a/%2e%2e//evil.test",
            "/%",
            "/%GG",
        ] {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("return_to", input)
                .finish();
            assert!(parse(Some(&query)).is_err(), "accepted {input:?}");
        }
        for query in [
            "return_to=",
            "return_to=/&return_to=/x",
            "return_to=/&return%5Fto=/",
            "return_to=%FF",
            "return_to=%",
            "return_to=/%0G",
        ] {
            assert!(parse(Some(query)).is_err(), "accepted {query}");
        }
        assert!(normalize(&format!("/{}", "a".repeat(2047))).is_ok());
        assert!(normalize(&format!("/{}", "a".repeat(2048))).is_err());
    }
}

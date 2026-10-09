use std::fmt;

use crate::Error;

/// The `Cookie` request header from a logged-in browser session on ebag.bg.
#[derive(Clone)]
pub struct Cookie(String);

impl Cookie {
    pub fn parse(input: &str) -> Result<Self, Error> {
        let trimmed = input.trim();
        let value = match trimmed.get(..7) {
            Some(prefix) if prefix.eq_ignore_ascii_case("cookie:") => trimmed[7..].trim(),
            _ => trimmed,
        };
        if value.is_empty() {
            return Err(Error::InvalidCookie("empty"));
        }
        if value.contains(['\n', '\r']) {
            return Err(Error::InvalidCookie("must be a single header line"));
        }
        let pairs_ok = value
            .split(';')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .all(|p| p.find('=').is_some_and(|i| i > 0));
        if !pairs_ok {
            return Err(Error::InvalidCookie("expected name=value pairs"));
        }
        let cookie = Self(value.to_string());
        if cookie.get("sessionid").is_none() {
            return Err(Error::InvalidCookie(
                "no sessionid; is this from a logged-in tab?",
            ));
        }
        Ok(cookie)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .split(';')
            .map(str::trim)
            .find_map(|p| p.strip_prefix(name)?.strip_prefix('='))
    }

    pub fn header(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Cookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Cookie(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_header_name_and_finds_values() {
        let c = Cookie::parse("Cookie: csrftoken=abc; sessionid=xyz; other=1").unwrap();
        assert_eq!(c.get("csrftoken"), Some("abc"));
        assert_eq!(c.get("sessionid"), Some("xyz"));
        assert_eq!(c.get("session"), None);
    }

    #[test]
    fn rejects_cookie_without_session() {
        assert!(Cookie::parse("csrftoken=abc").is_err());
        assert!(Cookie::parse("garbage").is_err());
        assert!(Cookie::parse("sessionid=a\nb=c").is_err());
    }
}

/// Extracts a trycloudflare.com HTTPS URL from a line of cloudflared output.
/// Returns None if the line contains no such URL.
pub fn parse_tunnel_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    // The URL ends at the first whitespace or control character.
    let end = rest
        .find(|c: char| c.is_whitespace())
        .unwrap_or(rest.len());
    let url = &rest[..end];
    if url.contains("trycloudflare.com") {
        Some(url.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_url_from_banner_line() {
        let line = "2024-01-01 INF |  https://brave-fox-1234.trycloudflare.com  |";
        assert_eq!(
            parse_tunnel_url(line),
            Some("https://brave-fox-1234.trycloudflare.com".to_string())
        );
    }

    #[test]
    fn ignores_non_tunnel_url() {
        let line = "Visit https://developers.cloudflare.com for docs";
        assert_eq!(parse_tunnel_url(line), None);
    }

    #[test]
    fn returns_none_when_no_url() {
        assert_eq!(parse_tunnel_url("starting tunnel..."), None);
    }
}

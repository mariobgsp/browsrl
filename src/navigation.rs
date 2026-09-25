use url::{Host, Url};

const DEFAULT_SEARCH_ENDPOINT: &str = "https://duckduckgo.com/";

pub fn normalize_input(input: &str) -> Result<String, String> {
    normalize_input_with_search(input, None)
}

pub fn normalize_input_with_search(
    input: &str,
    search_endpoint: Option<&str>,
) -> Result<String, String> {
    let value = input.trim();
    if value.is_empty() {
        return Ok("about:blank".to_string());
    }
    if value == "about:blank" {
        return Ok(value.to_string());
    }
    if value.starts_with("about:") {
        return Err("only about:blank is supported".to_string());
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        return validate_explicit_url(value);
    }
    if value.starts_with("//") {
        // A protocol-relative reference is an attempt to navigate, not a query,
        // so it is rejected as a URL rather than pushed towards search.
        return Err("only http, https, and about:blank are supported".to_string());
    }
    if let Some(query) = value.strip_prefix("search:") {
        let endpoint = search_endpoint.ok_or_else(|| {
            "search is disabled; configure an explicit --search-endpoint".to_string()
        })?;
        return search_url_with_endpoint(query, endpoint);
    }
    if has_unsupported_scheme(value) {
        return Err("only http, https, and about:blank are supported".to_string());
    }
    if looks_like_host(value) {
        return validate_explicit_url(&format!("https://{value}"));
    }
    Err("enter an http(s) address or configure an explicit search endpoint".to_string())
}

pub fn validate_explicit_url(value: &str) -> Result<String, String> {
    if value == "about:blank" {
        return Ok(value.to_string());
    }
    let url = Url::parse(value).map_err(|error| format!("invalid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http, https, and about:blank are supported".to_string());
    }
    let host = url
        .host()
        .ok_or_else(|| "URL is missing a valid host".to_string())?;
    if !valid_host(&host, url.host_str()) {
        return Err("URL has a malformed host".to_string());
    }
    Ok(url.to_string())
}

pub fn search_url(query: &str) -> String {
    search_url_with_endpoint(query, DEFAULT_SEARCH_ENDPOINT)
        .expect("the default search endpoint is valid")
}

pub fn search_url_with_endpoint(query: &str, endpoint: &str) -> Result<String, String> {
    let endpoint = validate_explicit_url(endpoint)?;
    let mut url =
        Url::parse(&endpoint).map_err(|error| format!("invalid search endpoint: {error}"))?;
    url.query_pairs_mut().append_pair("q", query.trim());
    Ok(url.to_string())
}

fn has_unsupported_scheme(value: &str) -> bool {
    let Some((prefix, _)) = value.split_once(':') else {
        return false;
    };
    if prefix.is_empty()
        || !prefix.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
    {
        return false;
    }
    // A host:port is not a URL scheme. Everything else with a scheme is rejected
    // before it can be mistaken for a search query or a local file.
    !prefix.eq_ignore_ascii_case("localhost")
        && !prefix.chars().all(|character| character.is_ascii_digit())
        && !prefix.contains('.')
}

fn looks_like_host(value: &str) -> bool {
    if value.is_empty()
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        || value.starts_with('/')
        || value.starts_with('?')
        || value.starts_with('#')
    {
        return false;
    }
    value.starts_with('[')
        || value.starts_with("localhost")
        || value.contains('.')
        || (value.contains(':') && value.split(':').count() >= 2)
}

fn valid_host(host: &Host<&str>, host_str: Option<&str>) -> bool {
    let Some(host_str) = host_str else {
        return false;
    };
    if host_str.is_empty()
        || host_str.chars().any(|character| {
            character.is_whitespace()
                || character.is_control()
                || matches!(character, '/' | '\\' | '?' | '#' | '@')
        })
    {
        return false;
    }
    let Host::Domain(domain) = host else {
        return true;
    };
    if domain.eq_ignore_ascii_case("localhost") {
        return true;
    }
    !domain.is_empty()
        && domain.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

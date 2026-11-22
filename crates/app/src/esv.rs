//! ESV Bible API (api.esv.org), for scripture presentations: the API token in the
//! system credential store, and fetching a passage's verses. Every call here blocks
//! (network, or the credential store asking the desktop), so run them on a worker
//! thread.

use std::time::Duration;

use opcore::bible::{Passage, esv_verses};

const SERVICE: &str = "OpenPresenter";
const ACCOUNT: &str = "esv-api-token";
const URL: &str = "https://api.esv.org/v3/passage/text/";

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, ACCOUNT).map_err(store_error)
}

fn store_error(e: keyring::Error) -> String {
    format!("The system's password store couldn't be used ({e}).")
}

/// The saved API token, if there is one.
pub fn load_token() -> Result<Option<String>, String> {
    match entry()?.get_password() {
        Ok(t) if !t.trim().is_empty() => Ok(Some(t)),
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(store_error(e)),
    }
}

pub fn save_token(token: &str) -> Result<(), String> {
    entry()?.set_password(token.trim()).map_err(store_error)
}

pub fn delete_token() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(store_error(e)),
    }
}

/// The passage's verses, one string each, without numbers, headings or footnotes.
pub fn fetch(token: &str, passage: &Passage) -> Result<Vec<String>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent
        .get(URL)
        .query("q", passage.query())
        .query_pairs([
            ("include-passage-references", "false"),
            // Verse numbers mark where each verse starts (they're taken out again).
            ("include-verse-numbers", "true"),
            ("include-first-verse-numbers", "true"),
            ("include-footnotes", "false"),
            ("include-footnote-body", "false"),
            ("include-headings", "false"),
            ("include-short-copyright", "false"),
            ("include-copyright", "false"),
            ("include-passage-horizontal-lines", "false"),
            ("include-heading-horizontal-lines", "false"),
            ("include-selahs", "true"),
            ("indent-paragraphs", "0"),
            ("indent-poetry", "false"),
            ("indent-declares", "0"),
            ("indent-psalm-doxology", "0"),
            ("line-length", "0"),
        ])
        .header("Authorization", format!("Token {}", token.trim()))
        .call()
        .map_err(|e| format!("Couldn't reach the ESV API ({e}). Check the internet connection."))?;
    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("The ESV API's answer couldn't be read ({e})."))?;
    match status {
        200 => {}
        401 | 403 => {
            return Err(
                "The ESV API didn't accept the token. Check it in Settings → Integrations.".into(),
            );
        }
        429 => {
            return Err("The ESV API's request limit was reached. Try again later.".into());
        }
        s => {
            return Err(format!(
                "The ESV API answered with an error ({s}): {}",
                body.chars().take(200).collect::<String>()
            ));
        }
    }
    let verses = parse(&body)?;
    if verses.is_empty() {
        return Err(format!(
            "The ESV API returned no text for {}.",
            passage.label()
        ));
    }
    Ok(verses)
}

fn parse(body: &str) -> Result<Vec<String>, String> {
    let v: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| format!("The ESV API's answer isn't what was expected ({e})."))?;
    let passages = v
        .get("passages")
        .and_then(|p| p.as_array())
        .ok_or("The ESV API's answer has no passage text.")?;
    let text: Vec<&str> = passages.iter().filter_map(|p| p.as_str()).collect();
    Ok(esv_verses(&text.join("\n")))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn reads_the_api_answer() {
        let body = r#"{"query":"John 3:16-17","canonical":"John 3:16–17","parsed":[[43003016,43003017]],
            "passage_meta":[],"passages":["  [16] “For God so loved the world, that he gave his only Son, that whoever believes in him should not perish but have eternal life. [17] For God did not send his Son into the world to condemn the world, but in order that the world might be saved through him.\n\n"]}"#;
        let v = parse(body).unwrap();
        assert_eq!(v.len(), 2);
        assert!(v[0].starts_with("“For God so loved"));
        assert!(v[1].ends_with("saved through him."));
        assert!(parse("{}").is_err());
    }
}

//! Clue writing with a language model through an OpenAI-compatible
//! chat-completions endpoint, called with `curl` (no extra dependencies;
//! the API key is passed on stdin, never on the command line).

use crate::clues::{normalize_answer, normalize_letters_lossy};
use std::collections::HashMap;
use std::io::Write;

/// Endpoint and credentials, read from a file: first line the base URL (or
/// the full `/chat/completions` URL), second line the API key.
#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub url: String,
    pub key: String,
    pub model: String,
}

impl LlmConfig {
    pub fn load(path: &str, model: &str) -> std::io::Result<LlmConfig> {
        let text = std::fs::read_to_string(path)?;
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let (Some(url), Some(key)) = (lines.next(), lines.next()) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "expected the URL on the first line and the API key on the second",
            ));
        };
        Ok(LlmConfig {
            url: url.to_string(),
            key: key.to_string(),
            model: model.to_string(),
        })
    }

    pub fn endpoint(&self) -> String {
        if self.url.ends_with("/chat/completions") {
            self.url.clone()
        } else {
            format!("{}/chat/completions", self.url.trim_end_matches('/'))
        }
    }
}

/// Our prompt: Italian crossword clues, one "PAROLA: definizione" per line.
pub fn build_prompt(words: &[String], max_chars: usize) -> String {
    format!(
        "Sei un autore di cruciverba italiani. Scrivi una definizione per ciascuna \
         di queste parole.\n\
         Regole:\n\
         - rispondi solo con righe nel formato PAROLA: definizione, una per riga, \
         nello stesso ordine, senza altro testo;\n\
         - definizioni brevi e precise, al massimo {max_chars} caratteri;\n\
         - non usare la parola stessa né parole della stessa famiglia;\n\
         - niente anagrammi né giochi di parole oscuri;\n\
         - se una parola non la conosci, salta la riga.\n\
         Parole:\n{}",
        words.join("\n")
    )
}

fn esc_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Chat-completions request body.
pub fn request_body(model: &str, prompt: &str) -> String {
    format!(
        r#"{{"model": "{}", "messages": [{{"role": "user", "content": "{}"}}], "temperature": 0.7}}"#,
        esc_json(model),
        esc_json(prompt)
    )
}

/// Parse the JSON string starting at the opening quote `s[start]`.
fn json_string(s: &str, start: usize) -> Option<String> {
    let bytes = s.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = s[start + 1..].chars();
    let hex4 = |it: &mut std::str::Chars| -> Option<u32> {
        let h: String = it.by_ref().take(4).collect();
        u32::from_str_radix(&h, 16).ok()
    };
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    let hi = hex4(&mut chars)?;
                    let code = if (0xD800..0xDC00).contains(&hi) {
                        // surrogate pair
                        if chars.next()? != '\\' || chars.next()? != 'u' {
                            return None;
                        }
                        let lo = hex4(&mut chars)?;
                        0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                    } else {
                        hi
                    };
                    out.push(char::from_u32(code)?);
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// Value (a JSON string) of the first `"key":` after `from`.
fn string_field(s: &str, key: &str, from: usize) -> Option<String> {
    let pat = format!("\"{key}\"");
    let at = s[from..].find(&pat)? + from + pat.len();
    let rest = &s[at..];
    let colon = rest.find(':')?;
    let after = at + colon + 1;
    let skip = s[after..].len() - s[after..].trim_start().len();
    json_string(s, after + skip)
}

/// The assistant text of a chat-completions response, or the API error.
pub fn extract_content(response: &str) -> Result<String, String> {
    if let Some(at) = response.find("\"error\"") {
        let msg = string_field(response, "message", at).unwrap_or_else(|| "unknown error".into());
        return Err(format!("LLM error: {msg}"));
    }
    let choices = response
        .find("\"choices\"")
        .ok_or("no choices in the response")?;
    string_field(response, "content", choices).ok_or_else(|| "no content in the response".into())
}

fn capitalize(s: &str) -> String {
    let mut cs = s.chars();
    match cs.next() {
        Some(f) => f.to_uppercase().chain(cs).collect(),
        None => String::new(),
    }
}

/// "PAROLA: definizione" lines → clues for the requested words; clues that
/// contain their answer are dropped.
pub fn parse_reply(text: &str, words: &[String]) -> HashMap<String, String> {
    let wanted: HashMap<String, &String> = words
        .iter()
        .filter_map(|w| normalize_answer(w).map(|n| (n, w)))
        .collect();
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line
            .trim()
            .trim_start_matches(|c: char| {
                c == '-' || c == '*' || c == '•' || c.is_ascii_digit() || c == '.' || c == ')'
            })
            .trim();
        let split = line
            .find(':')
            .map(|i| (&line[..i], &line[i + 1..]))
            .or_else(|| line.find(" - ").map(|i| (&line[..i], &line[i + 3..])))
            .or_else(|| {
                line.find(" – ")
                    .map(|i| (&line[..i], &line[i + " – ".len()..]))
            });
        let Some((w, clue)) = split else { continue };
        let Some(answer) = normalize_answer(w.trim().trim_matches('*')) else {
            continue;
        };
        let Some(&orig) = wanted.get(&answer) else {
            continue;
        };
        let clue = clue.trim().trim_matches('"').trim();
        if clue.is_empty() || (answer.len() >= 3 && normalize_letters_lossy(clue).contains(&answer))
        {
            continue;
        }
        out.insert(orig.clone(), capitalize(clue));
    }
    out
}

/// Anything that can write clues for a list of words.
pub trait ClueProvider {
    fn clues(&self, words: &[String]) -> Result<HashMap<String, String>, String>;
}

/// Fill `(answer, clue)` items: those without a clue, or all of them with
/// `all`. Asks in batches; returns how many clues were written.
pub fn fill_clues(
    items: &mut [(String, Option<String>)],
    provider: &dyn ClueProvider,
    all: bool,
) -> Result<usize, String> {
    let targets: Vec<usize> = (0..items.len())
        .filter(|&i| all || items[i].1.is_none())
        .collect();
    let mut written = 0;
    for chunk in targets.chunks(30) {
        let words: Vec<String> = chunk.iter().map(|&i| items[i].0.clone()).collect();
        let got = provider.clues(&words)?;
        for &i in chunk {
            if let Some(c) = got.get(&items[i].0) {
                items[i].1 = Some(capitalize(c));
                written += 1;
            }
        }
    }
    Ok(written)
}

/// `curl --config -` input: endpoint and headers (keeps the key off the
/// command line).
pub fn curl_config(cfg: &LlmConfig) -> String {
    let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "url = \"{}\"\nheader = \"Authorization: Bearer {}\"\nheader = \"Content-Type: application/json\"\n",
        q(&cfg.endpoint()),
        q(&cfg.key)
    )
}

/// Calls the endpoint with `curl`.
pub struct CurlClient {
    pub cfg: LlmConfig,
    pub max_chars: usize,
}

impl ClueProvider for CurlClient {
    fn clues(&self, words: &[String]) -> Result<HashMap<String, String>, String> {
        let body = request_body(&self.cfg.model, &build_prompt(words, self.max_chars));
        let path = std::env::temp_dir().join(format!(
            "xfill-llm-body-{}-{}.json",
            std::process::id(),
            words.len()
        ));
        std::fs::write(&path, body).map_err(|e| e.to_string())?;
        let mut child = std::process::Command::new("curl")
            .args(["-sS", "--max-time", "300", "--config", "-", "--data-binary"])
            .arg(format!("@{}", path.display()))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run curl: {e}"))?;
        child
            .stdin
            .take()
            .ok_or("no stdin")?
            .write_all(curl_config(&self.cfg).as_bytes())
            .map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string());
        let _ = std::fs::remove_file(&path);
        let out = out?;
        if !out.status.success() {
            return Err(format!(
                "curl failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let content = extract_content(&String::from_utf8_lossy(&out.stdout))?;
        Ok(parse_reply(&content, words))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_url_then_key() {
        let f = std::env::temp_dir().join(format!("xfill-llm-{}.txt", std::process::id()));
        std::fs::write(
            &f,
            "https://example.test/compatible-mode/v1\n  sk-test-123  ",
        )
        .unwrap();
        let c = LlmConfig::load(f.to_str().unwrap(), "qwen3.8-flash").unwrap();
        std::fs::remove_file(&f).ok();
        assert_eq!(
            c.endpoint(),
            "https://example.test/compatible-mode/v1/chat/completions"
        );
        assert_eq!(c.key, "sk-test-123");
        let full = LlmConfig {
            url: "https://x/v1/chat/completions".into(),
            key: "k".into(),
            model: "m".into(),
        };
        assert_eq!(full.endpoint(), "https://x/v1/chat/completions");
        assert!(LlmConfig::load("/nonexistent/llm", "m").is_err());
        let one = std::env::temp_dir().join(format!("xfill-llm1-{}.txt", std::process::id()));
        std::fs::write(&one, "https://only-url\n").unwrap();
        assert!(
            LlmConfig::load(one.to_str().unwrap(), "m").is_err(),
            "the key is required"
        );
        std::fs::remove_file(&one).ok();
    }

    #[test]
    fn prompt_lists_words_and_rules() {
        let p = build_prompt(&["CASA".into(), "ROMA".into()], 40);
        assert!(p.contains("CASA") && p.contains("ROMA"));
        assert!(p.contains("PAROLA: definizione"));
        assert!(p.contains("40"));
    }

    #[test]
    fn request_body_is_escaped_json() {
        let b = request_body("qwen3.8-flash", "riga 1\n\"citata\" \\ fine");
        assert!(b.contains(r#""model": "qwen3.8-flash""#));
        assert!(b.contains(r#"riga 1\n\"citata\" \\ fine"#));
        assert!(b.starts_with('{') && b.ends_with('}'));
    }

    #[test]
    fn extracts_message_content_or_error() {
        let ok = r#"{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"CASA: Abitazione\nROMA: Città eterna \"caput\""}}]}"#;
        assert_eq!(
            extract_content(ok).unwrap(),
            "CASA: Abitazione\nROMA: Città eterna \"caput\""
        );
        let err = r#"{"error":{"message":"Invalid API key","type":"auth"}}"#;
        assert!(extract_content(err)
            .unwrap_err()
            .contains("Invalid API key"));
        assert!(extract_content("not json").is_err());
    }

    #[test]
    fn parses_reply_lines() {
        let words: Vec<String> = vec!["CASA".into(), "ROMA".into(), "ETA".into()];
        let reply =
            "casa: Abitazione\n- ROMA - La capitale\nETA: Etàno\nALTRO: ignorata\n\nnote varie";
        let m = parse_reply(reply, &words);
        assert_eq!(m.get("CASA").map(String::as_str), Some("Abitazione"));
        assert_eq!(m.get("ROMA").map(String::as_str), Some("La capitale"));
        assert!(
            !m.contains_key("ETA"),
            "clue containing the answer is dropped"
        );
        assert!(!m.contains_key("ALTRO"), "only requested words");
    }

    struct Fake;
    impl ClueProvider for Fake {
        fn clues(&self, words: &[String]) -> Result<HashMap<String, String>, String> {
            Ok(words
                .iter()
                .map(|w| (w.clone(), format!("def di {}", w.to_lowercase())))
                .collect())
        }
    }

    #[test]
    fn fills_only_missing_clues_unless_all() {
        let mut items: Vec<(String, Option<String>)> = vec![
            ("CASA".into(), Some("Abitazione".into())),
            ("ROMA".into(), None),
        ];
        let n = fill_clues(&mut items, &Fake, false).unwrap();
        assert_eq!(n, 1);
        assert_eq!(items[0].1.as_deref(), Some("Abitazione"));
        assert_eq!(items[1].1.as_deref(), Some("Def di roma"));
        let n = fill_clues(&mut items, &Fake, true).unwrap();
        assert_eq!(n, 2);
        assert_eq!(items[0].1.as_deref(), Some("Def di casa"));
    }

    /// End to end against a local fake endpoint: the key travels in the
    /// Authorization header (via curl's stdin config), the body names the
    /// model, and the reply is parsed into clues.
    #[test]
    fn curl_client_talks_to_an_endpoint() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            // read until the JSON body is complete
            loop {
                let n = sock.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req);
                if n == 0 || (text.contains("\r\n\r\n") && text.trim_end().ends_with('}')) {
                    break;
                }
            }
            let body = r#"{"choices":[{"message":{"role":"assistant","content":"CASA: Abitazione\nROMA: Capitale"}}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).unwrap();
            String::from_utf8_lossy(&req).to_string()
        });
        let client = CurlClient {
            cfg: LlmConfig {
                url: format!("http://127.0.0.1:{port}/v1"),
                key: "sk-local".into(),
                model: "qwen3.8-flash".into(),
            },
            max_chars: 40,
        };
        let got = client.clues(&["CASA".into(), "ROMA".into()]).unwrap();
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /v1/chat/completions"), "{req}");
        assert!(req.contains("Authorization: Bearer sk-local"));
        assert!(req.contains(r#""model": "qwen3.8-flash""#));
        assert_eq!(got.get("CASA").map(String::as_str), Some("Abitazione"));
        assert_eq!(got.get("ROMA").map(String::as_str), Some("Capitale"));
        // an unreachable endpoint is an error, not a panic
        let dead = CurlClient {
            cfg: LlmConfig {
                url: "http://127.0.0.1:9/v1".into(),
                key: "k".into(),
                model: "m".into(),
            },
            max_chars: 40,
        };
        assert!(dead.clues(&["CASA".into()]).is_err());
    }

    #[test]
    fn curl_config_carries_url_and_key() {
        let c = LlmConfig {
            url: "https://h/v1".into(),
            key: "sk-1".into(),
            model: "m".into(),
        };
        let cfg = curl_config(&c);
        assert!(cfg.contains("url = \"https://h/v1/chat/completions\""));
        assert!(cfg.contains("header = \"Authorization: Bearer sk-1\""));
    }
}

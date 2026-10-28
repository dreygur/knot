//! JSON with `//` and `/* */` comments, as Cursor and Gemini accept in their configs.
//! Knot only reads it: writing would drop the comments, so a config that needs them is
//! left for the person to edit.

/// `text` without its comments. Text inside strings is kept as it is, escapes included.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                while chars.next_if(|&n| n != '\n').is_some() {}
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for n in chars.by_ref() {
                    if last == '*' && n == '/' {
                        break;
                    }
                    last = n;
                }
                // Keeps `1/**/2` two tokens, so it stays invalid as it was
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    out
}

/// `text` parsed as JSON once its comments are gone, or `None` when it still is not JSON.
pub fn parse(text: &str) -> Option<serde_json::Value> {
    serde_json::from_str(&strip_comments(text)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn comments_go_and_strings_stay() {
        let text = "{\n  // a line\n  \"url\": \"http://x/*y*/\", /* a block\n spanning */ \"q\": \"say \\\"//\\\"\"\n}";
        assert_eq!(parse(text), Some(json!({"url": "http://x/*y*/", "q": "say \"//\""})));
        assert_eq!(parse("{\"a\": 1}"), Some(json!({"a": 1})));
    }

    #[test]
    fn broken_json_stays_broken() {
        assert_eq!(parse("{ not json"), None);
        assert_eq!(parse("[1/**/2]"), None);
        assert_eq!(parse("{\"a\": 1 /* never closed"), None);
    }
}

//! Manifest template → container-execution strings.
//!
//! Every surface renders the same `{{param}}` token differently: argv and
//! env are handed to podman as a shell-free exec argument / env value, so
//! we serialise by [`ParamType`] without any escaping; the script surface
//! (in a sibling module) is interpreted by the tool's language, so its
//! escaping policy is chosen by `interpreter`. Param values flow through
//! the already-resolved map the caller hands in.

use std::collections::BTreeMap;

use serde_json::Value;

use super::error::{Error, Result};

/// Render a single token substitution. Unknown tokens fail with
/// [`Error::UndeclaredTemplateRef`] — load-time closure validation already
/// catches them, the renderer re-checks as defense in depth.
fn resolve_token(token: &str, resolved: &BTreeMap<String, Value>) -> Result<String> {
    let value = resolved
        .get(token)
        .ok_or_else(|| Error::UndeclaredTemplateRef {
            kind: String::new(),
            name: token.to_string(),
        })?;
    // Surface policy (argv/env): values arrive already type-checked; this
    // serialiser knows how to render each [`ParamType`].
    serialise_value(value).ok_or_else(|| Error::UndeclaredTemplateRef {
        kind: String::new(),
        name: token.to_string(),
    })
}

/// Serialise a resolved param value for the argv/env surfaces: numbers
/// and booleans render unquoted (`--time_limit 1.0`, `--force true`);
/// strings and string arrays render verbatim (podman never re-shells
/// argv, and env values pass through `execve`'s raw byte buffer).
fn serialise_value(value: &Value) -> Option<String> {
    Some(match value {
        // Optional-and-absent params resolve to null and render as an
        // empty string: env consumers see "" and `[ -n "$VAR" ]` is false.
        Value::Null => String::new(),
        // Booleans carry *presence* semantics: `false` renders as "" so
        // both consumer idioms agree — `[ -n "$VAR" ]` presence tests are
        // off, and `= "true"` equality tests are off. Rendering the
        // literal "false" made every presence test true (the flag was
        // passed whenever a user *explicitly disabled* it).
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => String::new(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(serialise_value)
            .collect::<Option<Vec<_>>>()?
            .join(" "),
        // Everything else (Null / Object / unexpected scalars) is rejected;
        // the type check upstream guarantees these are unreachable.
        _ => return None,
    })
}

/// Walk `text`, replacing every `{{token}}` with `resolve_token(...)`. The
/// scan is byte-level because the marker bytes (`{`, `}`) are ASCII and
/// never appear inside a UTF-8 multibyte continuation.
fn render_template(text: &str, resolved: &BTreeMap<String, Value>) -> Result<String> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            let tail = &text[i + 2..];
            let Some(end) = tail.find("}}") else {
                // Unclosed `{{` is a malformed template; surface the raw text
                // so the author can find it.
                return Err(Error::UndeclaredTemplateRef {
                    kind: String::new(),
                    name: text.to_string(),
                });
            };
            let token = tail[..end].trim();
            if token.is_empty() || !token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(Error::UndeclaredTemplateRef {
                    kind: String::new(),
                    name: token.to_string(),
                });
            }
            out.push_str(&resolve_token(token, resolved)?);
            i += 2 + end + 2;
            continue;
        }
        // Byte-level advance: the marker bytes `{` and `}` are ASCII and
        // never appear inside a UTF-8 multibyte continuation, so iterating
        // one byte at a time stays on UTF-8 char boundaries.
        let byte = bytes[i];
        out.push(byte as char);
        i += 1;
    }
    Ok(out)
}

/// Render each element of `argv` against `resolved`. Elements without
/// `{{token}}` are passed through verbatim so non-templated arguments
/// (`--flag`, `Rscript`) survive unchanged.
pub fn render_argv(argv: &[String], resolved: &BTreeMap<String, Value>) -> Result<Vec<String>> {
    argv.iter()
        .map(|arg| render_template(arg, resolved))
        .collect()
}

/// Render each value of `env` against `resolved`. NUL bytes are forbidden
/// inside environment values (the kernel rejects them), so a value that
/// resolves with one is rejected at the source.
pub fn render_env(
    env: &BTreeMap<String, String>,
    resolved: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, String>> {
    env.iter()
        .map(|(key, value)| {
            let rendered = render_template(value, resolved)?;
            if rendered.contains('\0') {
                return Err(Error::UndeclaredTemplateRef {
                    kind: String::new(),
                    name: format!("env value for `{key}`"),
                });
            }
            Ok((key.clone(), rendered))
        })
        .collect()
}

/// Render each value of `files` against `resolved`. **v0 pass-through**: the
/// value is what the host writes to disk inside the container, so v0
/// treats it as opaque bytes from TOML — no template substitution. This
/// keeps the simple case (a pre-baked config file) verbatim and reserves
/// substitution semantics for a later milestone if it's ever needed.
pub fn render_files(
    files: &BTreeMap<String, String>,
    _resolved: &BTreeMap<String, serde_json::Value>,
) -> BTreeMap<String, String> {
    files.clone()
}

/// Render the script body against `resolved`. Each shell-token boundary
/// becomes the substitution unit so literal whitespace, newlines, comments
/// and operators survive unchanged; only `{{param}}` occurrences resolve.
///
/// `interpreter` selects the escape strategy for substituted values: the
/// shell family wraps strings in single quotes, every other interpreter
/// (Rscript, python, perl, …) uses Rust `{:?}` double-quoting — both
/// are valid in the respective language's string-literal syntax.
pub fn render_script(
    text: &str,
    interpreter: &str,
    resolved: &BTreeMap<String, Value>,
) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Single-quoted literal: pass through untouched, quotes preserved.
        // Embedded `{{param}}` inside quotes is intentionally not resolved
        // — a quoted string is the author's exact bytes.
        if bytes[i] == b'\'' {
            let Some(end) = bytes[i + 1..].iter().position(|&b| b == b'\'') else {
                return Err(Error::UndeclaredTemplateRef {
                    kind: String::new(),
                    name: format!("unclosed single quote in script: {text}"),
                });
            };
            let end = i + 1 + end;
            out.push_str(&text[i..=end]);
            i = end + 1;
            continue;
        }

        // Double-quoted literal: same — author-set bytes are preserved.
        if bytes[i] == b'"' {
            let Some(end) = bytes[i + 1..].iter().position(|&b| b == b'"') else {
                return Err(Error::UndeclaredTemplateRef {
                    kind: String::new(),
                    name: format!("unclosed double quote in script: {text}"),
                });
            };
            let end = i + 1 + end;
            out.push_str(&text[i..=end]);
            i = end + 1;
            continue;
        }

        // Whitespace passes through one byte at a time.
        if bytes[i].is_ascii_whitespace() {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }

        // A `#` at a token boundary starts a comment: consume through the
        // end of the line verbatim. Shell semantics — quotes and template
        // markers inside comments are inert, which matters because prose
        // comments routinely contain apostrophes.
        if bytes[i] == b'#' {
            let mut end = i;
            while end < bytes.len() && bytes[end] != b'\n' {
                end += 1;
            }
            if end < bytes.len() {
                end += 1; // include the newline
            }
            out.push_str(&text[i..end]);
            i = end;
            continue;
        }

        // A lone `{` is not a template marker; it is a literal byte that
        // passes through unchanged. Treating it as the start of a bare
        // token would zero-length the token and freeze `i` here (the
        // `i = end` advance below moves nowhere), creating a loop.
        if bytes[i] == b'{' {
            // Lookahead: a true `{{...}}` template begins at `i`; treat the
            // marker plus its body plus the closing `}}` as one unit and
            // resolve it without any further shell-token splitting.
            if bytes.get(i + 1) == Some(&b'{') {
                let tail = &text[i + 2..];
                let Some(close) = tail.find("}}") else {
                    return Err(Error::UndeclaredTemplateRef {
                        kind: String::new(),
                        name: format!("unclosed template starting at `{text}`"),
                    });
                };
                let token = tail[..close].trim();
                if !token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(Error::UndeclaredTemplateRef {
                        kind: String::new(),
                        name: token.to_string(),
                    });
                }
                let substituted = resolve_token(token, resolved)?;
                out.push_str(&shell_escape(&substituted, interpreter));
                i += 2 + close + 2;
                continue;
            }
            out.push('{');
            i += 1;
            continue;
        }

        let mut end = i;
        while end < bytes.len()
            && !bytes[end].is_ascii_whitespace()
            && bytes[end] != b'\''
            && bytes[end] != b'"'
            && bytes[end] != b'#'
            && bytes[end] != b'{'
        {
            end += 1;
        }
        let token = &text[i..end];

        if token.contains("{{") {
            if !token.contains("}}") {
                return Err(Error::UndeclaredTemplateRef {
                    kind: String::new(),
                    name: token.to_string(),
                });
            }
            let substituted = render_template(token, resolved)?;
            out.push_str(&shell_escape(&substituted, interpreter));
        } else {
            out.push_str(token);
        }
        i = end;
    }

    Ok(out)
}

/// Wrap a substituted value for the script surface. Numbers and booleans
/// render as their bare representation; strings get the interpreter's
/// quoting style. Arrays render as space-joined (sh arrays are typically
/// iterated, not embedded; if a richer behaviour is needed, split the
/// array across multiple `{{param}}` references).
fn shell_escape(value: &str, interpreter: &str) -> String {
    if !value.is_empty()
        && value.bytes().all(|b| {
            b.is_ascii_digit() || b.is_ascii_alphabetic() || matches!(b, b'_' | b'.' | b'-')
        })
    {
        return value.to_string();
    }
    match interpreter {
        "sh" | "bash" | "dash" => format!("'{}'", value.replace('\'', "'\\''")),
        // Rust `{:?}` produces a double-quoted string with escapes. Both
        // R and Python accept this literal shape.
        _ => format!("{:?}", value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resolved(values: &[(&str, Value)]) -> BTreeMap<String, Value> {
        values
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect()
    }

    #[test]
    fn render_template_returns_immediately_on_plain_text() {
        // Regression: the passthrough branch used to call
        // `text[i..].chars().next()` twice in the same expression, leaving
        // `i` unchanged and looping forever. Plain text never enters the
        // substitution branch.
        let resolved = resolved(&[]);
        assert_eq!(
            render_template("no templated text here\n", &resolved).unwrap(),
            "no templated text here\n"
        );
    }

    #[test]
    fn argv_passes_through_literal_arguments() {
        let resolved = resolved(&[]);
        let argv = vec!["Rscript".into(), "--vanilla".into()];
        assert_eq!(
            render_argv(&argv, &resolved).unwrap(),
            vec!["Rscript", "--vanilla"]
        );
    }

    #[test]
    fn argv_renders_numbers_and_booleans_unquoted() {
        let resolved = resolved(&[
            ("limit", json!(2.5)),
            ("force", json!(true)),
            ("count", json!(7)),
        ]);
        let argv = vec![
            "--limit".into(),
            "{{ limit }}".into(),
            "--force".into(),
            "{{ force }}".into(),
            "--count".into(),
            "{{ count }}".into(),
        ];
        assert_eq!(
            render_argv(&argv, &resolved).unwrap(),
            vec!["--limit", "2.5", "--force", "true", "--count", "7"]
        );
    }

    #[test]
    fn bool_false_renders_empty_for_presence_semantics() {
        // Explicit false must be indistinguishable from an unset optional:
        // `[ -n "$VAR" ]` tests off and `= "true"` comparisons fail.
        // Rendering the literal "false" passed the flag whenever a user
        // explicitly disabled it (mtag/ldsc presence-style scripts).
        let resolved = resolved(&[("force", json!(false))]);
        let mut env = BTreeMap::new();
        env.insert("MTAG_FORCE".into(), "{{ force }}".into());
        let rendered = render_env(&env, &resolved).unwrap();
        assert_eq!(rendered.get("MTAG_FORCE").unwrap(), "");

        let argv = vec!["--force".into(), "{{ force }}".into()];
        assert_eq!(render_argv(&argv, &resolved).unwrap(), vec!["--force", ""]);
    }

    #[test]
    fn argv_preserves_string_content_verbatim() {
        // podman argv is shell-free; injection lives in the script surface,
        // not here.
        let resolved = resolved(&[("name", json!("O'Reilly --force;rm"))]);
        let argv = vec!["--author".into(), "{{ name }}".into()];
        assert_eq!(
            render_argv(&argv, &resolved).unwrap(),
            vec!["--author", "O'Reilly --force;rm"]
        );
    }

    #[test]
    fn argv_renders_string_arrays_as_space_joined() {
        let resolved = resolved(&[("beta", json!(["a", "b", "c"]))]);
        let argv = vec!["--beta".into(), "{{ beta }}".into()];
        assert_eq!(
            render_argv(&argv, &resolved).unwrap(),
            vec!["--beta", "a b c"]
        );
    }

    #[test]
    fn argv_rejects_undeclared_template_tokens() {
        let resolved = resolved(&[]);
        let argv = vec!["--limit".into(), "{{ missing }}".into()];
        assert!(render_argv(&argv, &resolved).is_err());
    }

    #[test]
    fn argv_rejects_non_templated_unknown_text_after_marker() {
        // `{{not valid}}` contains a space; the scan rejects it as a
        // malformed token instead of leaving it in the output.
        let resolved = resolved(&[("not_valid", json!("x"))]);
        let argv = vec!["{{not valid}}".into()];
        assert!(render_argv(&argv, &resolved).is_err());
    }

    #[test]
    fn env_substitutes_and_preserves_keys() {
        let resolved = resolved(&[("user", json!("alice"))]);
        let mut env = BTreeMap::new();
        env.insert("GREETING".into(), "hello {{ user }}".into());
        env.insert("LITERAL".into(), "no-template".into());
        let rendered = render_env(&env, &resolved).unwrap();
        assert_eq!(rendered.get("GREETING").unwrap(), "hello alice");
        assert_eq!(rendered.get("LITERAL").unwrap(), "no-template");
    }

    #[test]
    fn env_rejects_values_containing_nul_after_substitution() {
        // `serde_json` cannot carry a literal NUL, so the test reaches
        // the rejection path by injecting one into a non-templated value.
        let resolved = resolved(&[]);
        let mut env = BTreeMap::new();
        env.insert("BAD".into(), "a\0b".into());
        let error = render_env(&env, &resolved).unwrap_err();
        assert!(error.to_string().contains("env value for `BAD`"));
    }

    #[test]
    fn script_passes_through_lone_braces_as_literal_bytes() {
        // Regression: a single `{` (not part of `{{`) used to be neither
        // passthrough nor bare-collection boundary, leaving `i` stuck and
        // hanging the test runner. `render_script` must accept lone braces.
        let resolved = resolved(&[]);
        assert_eq!(
            render_script("echo {not-a-template}", "sh", &resolved).unwrap(),
            "echo {not-a-template}"
        );
    }

    #[test]
    fn script_passes_through_whitespace_and_comments() {
        let resolved = resolved(&[]);
        let script = "# header\nset -eu\nmtag --foo bar\n";
        let rendered = render_script(script, "sh", &resolved).unwrap();
        assert_eq!(rendered, script);
    }

    #[test]
    fn script_preserves_quoted_string_contents_byte_for_byte() {
        // A literal quote in a manifest string is the author's exact bytes;
        // `{{param}}` inside quotes is intentionally not resolved.
        let resolved = resolved(&[]);
        let script = "echo '{{ not_resolved }}'";
        let rendered = render_script(script, "sh", &resolved).unwrap();
        assert_eq!(rendered, script);
    }

    //
    #[test]
    fn script_substitutes_only_outside_quotes_and_escapes_for_sh() {
        let resolved = resolved(&[("name", json!("O'Reilly"))]);
        let script = "tool --author {{ name }} --fallback '{{ literal }}'";
        let rendered = render_script(script, "sh", &resolved).unwrap();
        // Single-quoted form: `'` → `'\''` to close, escape, reopen.
        // The literal `{{ literal }}` inside single quotes is preserved
        // verbatim because quoted segments are author bytes.
        assert_eq!(
            rendered,
            "tool --author 'O'\\''Reilly' --fallback '{{ literal }}'"
        );
    }

    #[test]
    fn script_uses_rust_escape_for_non_sh_interpreters() {
        let resolved = resolved(&[("name", json!("O'Reilly"))]);
        let rendered = render_script("msg <- {{ name }}", "Rscript", &resolved).unwrap();
        // R's double-quoted string with Rust escapes — `'` is fine.
        assert_eq!(rendered, "msg <- \"O'Reilly\"");
    }

    #[test]
    fn script_passes_through_safe_alphanumeric_tokens_without_quoting() {
        let resolved = resolved(&[("tag", json!("v1.0"))]);
        let rendered = render_script("{{ tag }}.tar.gz", "sh", &resolved).unwrap();
        assert_eq!(rendered, "v1.0.tar.gz");
    }

    #[test]
    fn script_renders_numbers_and_booleans_unquoted() {
        let resolved = resolved(&[("limit", json!(2.5)), ("force", json!(true))]);
        let rendered =
            render_script("--limit {{ limit }} --force {{ force }}", "sh", &resolved).unwrap();
        assert_eq!(rendered, "--limit 2.5 --force true");
    }

    #[test]
    fn script_rejects_unclosed_quote() {
        let resolved = resolved(&[]);
        let error = render_script("echo 'unterminated", "sh", &resolved).unwrap_err();
        assert!(error.to_string().contains("unclosed"));
    }
}

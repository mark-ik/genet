/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

pub(super) fn is_valid_essl_identifier(identifier: &str) -> bool {
    let mut chars = identifier.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    if !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
        return false;
    }
    !matches!(
        identifier,
        "attribute"
            | "gl_FragColor"
            | "gl_Position"
            | "main"
            | "precision"
            | "sampler2D"
            | "texture2D"
            | "vec2"
            | "vec4"
    )
}

pub(super) fn normalize_shader(source: &str) -> String {
    let source = strip_comments(source);
    let mut tokens = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        if is_glsl_punctuation(ch) {
            tokens.push(ch.to_string());
            continue;
        }

        let mut token = String::from(ch);
        while let Some(&next) = chars.peek() {
            if next.is_whitespace() || is_glsl_punctuation(next) {
                break;
            }
            token.push(next);
            chars.next();
        }
        tokens.push(token);
    }

    let mut normalized = String::new();
    for token in tokens {
        if let Some(previous) = normalized.chars().last() {
            if needs_token_space(previous, &token) {
                normalized.push(' ');
            }
        }
        normalized.push_str(&token);
    }
    normalized
}

fn is_glsl_punctuation(ch: char) -> bool {
    matches!(ch, '(' | ')' | '{' | '}' | ',' | ';' | '=')
}

fn needs_token_space(previous: char, token: &str) -> bool {
    !is_glsl_punctuation(previous) && !token.chars().next().is_some_and(is_glsl_punctuation)
}

fn strip_comments(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for ch in chars.by_ref() {
                if ch == '\n' {
                    output.push('\n');
                    break;
                }
            }
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut previous = '\0';
            for ch in chars.by_ref() {
                if previous == '*' && ch == '/' {
                    output.push(' ');
                    break;
                }
                previous = ch;
            }
        } else {
            output.push(ch);
        }
    }
    output
}

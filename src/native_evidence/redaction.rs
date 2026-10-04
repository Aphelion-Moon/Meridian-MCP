pub const PROTECTED_FIELDS: &[&str] = &[
    "player",
    "player_id",
    "client",
    "client_id",
    "account",
    "account_id",
    "key",
    "ckey",
    "mob",
    "mob_id",
    "discord",
    "discord_id",
];

pub fn protected(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase().replace('-', "_");
    PROTECTED_FIELDS.contains(&normalized.as_str())
}

pub fn sanitize_text(text: &str) -> (String, u64) {
    let mut result = text.to_owned();
    let mut count = 0;
    for field in PROTECTED_FIELDS {
        for marker in std::iter::once(format!("{field}=")).chain(
            field
                .contains('_')
                .then(|| format!("{}=", field.replace('_', "-"))),
        ) {
            let mut cursor = 0;
            while let Some(offset) = result[cursor..].to_ascii_lowercase().find(&marker) {
                let start = cursor + offset;
                if start > 0
                    && (result.as_bytes()[start - 1].is_ascii_alphanumeric()
                        || result.as_bytes()[start - 1] == b'_')
                {
                    cursor = start + 1;
                    continue;
                }
                let value_start = start + marker.len();
                let end = result[value_start..]
                    .find(|character: char| {
                        character.is_whitespace() || character == ',' || character == ';'
                    })
                    .map(|offset| value_start + offset)
                    .unwrap_or(result.len());
                result.replace_range(value_start..end, "<redacted>");
                count += 1;
                cursor = value_start + "<redacted>".len();
            }
        }
    }
    (result, count)
}

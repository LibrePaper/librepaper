//! Documents may expire: after a duration, from when they were created or last
//! published.

pub const DEFAULT_EXPIRE_FROM: &str = "updated";

/// Accepts Go-style durations and the more convenient day suffix used in the
/// command-line examples (24h and 30d are equivalent). Returns seconds; zero
/// means never.
pub fn parse_retention(value: &str) -> Result<i64, String> {
    let value = value.trim().to_lowercase();
    if value.is_empty() || value == "never" || value == "off" {
        return Ok(0);
    }
    let invalid = || format!("invalid retention {value:?}");
    let seconds = if let Some(days) = value.strip_suffix('d') {
        let days: f64 = days.parse().map_err(|_| invalid())?;
        days * 86_400.0
    } else {
        parse_duration(&value).ok_or_else(invalid)?
    };
    if seconds <= 0.0 || !seconds.is_finite() {
        return Err(invalid());
    }
    Ok(seconds as i64)
}

/// A duration in the shape Go prints them: "1h30m", "45m", "90s". Seconds out.
fn parse_duration(value: &str) -> Option<f64> {
    let mut total = 0.0;
    let mut number = String::new();
    let mut any = false;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() || c == '.' {
            number.push(c);
            continue;
        }
        let amount: f64 = number.parse().ok()?;
        number.clear();
        let unit = match c {
            'h' => 3600.0,
            'm' => {
                if chars.peek() == Some(&'s') {
                    chars.next();
                    0.001
                } else {
                    60.0
                }
            }
            's' => 1.0,
            _ => return None,
        };
        total += amount * unit;
        any = true;
    }
    if !number.is_empty() || !any {
        return None;
    }
    Some(total)
}

pub fn parse_expire_from(value: &str) -> Result<String, String> {
    let value = value.trim().to_lowercase();
    if value.is_empty() {
        return Ok(DEFAULT_EXPIRE_FROM.to_string());
    }
    if value != "created" && value != "updated" {
        return Err(format!(
            "--expire-from must be 'created' or 'updated', not {value:?}"
        ));
    }
    Ok(value)
}

/// A duration, printed the way the startup line wants it.
pub fn describe_seconds(seconds: i64) -> String {
    if seconds % 86_400 == 0 {
        format!("{}d", seconds / 86_400)
    } else if seconds % 3600 == 0 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{seconds}s")
    }
}

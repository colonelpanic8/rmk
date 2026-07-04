//! Time-value parsing shared across config sections: `DurationMillis` fields
//! and the flexible integer-or-"20ms"-string deserializers.

use serde::{Deserialize, de};

/// Duration in milliseconds
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct DurationMillis(#[serde(deserialize_with = "parse_duration_millis")] pub u64);

fn parse_duration_millis<'de, D: de::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let input: String = de::Deserialize::deserialize(deserializer)?;
    parse_duration_str(&input).map_err(de::Error::custom)
}

fn parse_duration_str(input: &str) -> Result<u64, String> {
    let num = input.trim_end_matches(|c: char| !c.is_numeric());
    let unit = &input[num.len()..];
    let num: u64 = num
        .parse()
        .map_err(|_| format!("Invalid number \"{num}\" in duration: number part must be a u64"))?;

    match unit {
        "s" => num
            .checked_mul(1000)
            .ok_or_else(|| format!("Duration \"{input}\" is too large")),
        "ms" => Ok(num),
        other => Err(format!(
            "Invalid duration unit \"{other}\": unit part must be either \"s\" or \"ms\""
        )),
    }
}

/// A time value that is either a bare integer (in the field's native unit) or
/// a `"300ms"`/`"1s"`-style string.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawDuration {
    Int(u64),
    Text(String),
}

impl RawDuration {
    fn into_millis(self) -> Result<u64, String> {
        match self {
            RawDuration::Int(ms) => Ok(ms),
            RawDuration::Text(text) => parse_duration_str(&text),
        }
    }

    fn into_seconds(self) -> Result<u64, String> {
        match self {
            RawDuration::Int(secs) => Ok(secs),
            RawDuration::Text(text) => {
                let ms = parse_duration_str(&text)?;
                if !ms.is_multiple_of(1000) {
                    return Err(format!(
                        "\"{text}\": this field has whole-second resolution, sub-second values are not supported"
                    ));
                }
                Ok(ms / 1000)
            }
        }
    }
}

fn convert_duration_range<T: TryFrom<u64>>(value: u64) -> Result<T, String> {
    T::try_from(value).map_err(|_| format!("duration value {value} is out of range for this field"))
}

pub(crate) fn de_millis<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: de::Deserializer<'de>,
    T: TryFrom<u64>,
{
    RawDuration::deserialize(deserializer)?
        .into_millis()
        .and_then(convert_duration_range)
        .map_err(de::Error::custom)
}

pub(crate) fn de_opt_millis<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: de::Deserializer<'de>,
    T: TryFrom<u64>,
{
    Option::<RawDuration>::deserialize(deserializer)?
        .map(|raw| raw.into_millis().and_then(convert_duration_range))
        .transpose()
        .map_err(de::Error::custom)
}

pub(crate) fn de_secs<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: de::Deserializer<'de>,
    T: TryFrom<u64>,
{
    RawDuration::deserialize(deserializer)?
        .into_seconds()
        .and_then(convert_duration_range)
        .map_err(de::Error::custom)
}

pub(crate) fn de_opt_secs<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: de::Deserializer<'de>,
    T: TryFrom<u64>,
{
    Option::<RawDuration>::deserialize(deserializer)?
        .map(|raw| raw.into_seconds().and_then(convert_duration_range))
        .transpose()
        .map_err(de::Error::custom)
}
